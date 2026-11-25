//! Scenario #13 — Real transport authentication & handshake security over `turmoil::net`:
//! (a) Single-use pairing invites (`device.invite.create` -> `transport.mux` pairing ->
//!     upgraded per-device token works -> invite replay & expired invite rejected),
//! (b) Live `device.revoke` + `RevocationBus` kicking an active `yamux/1` stream immediately
//!     and rejecting subsequent reconnects with the revoked token,
//! (c) Tailscale WhoIs via `SimTailscaleVerifier` (unauthorized peer rejected, authorized peer
//!     issued deterministic ephemeral `ts_sess_...` token, `enc:v1:` sealed secret RPC round-trip),
//! (d) `ApiMismatch` via `desktop_api_override` (`SingleHostSession` typed error + `MultiHostClient`
//!     `HostConnectionState::ApiMismatch` surfacing + clean recovery once restored).
use crate::{
    net::TurmoilConnector,
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SIM_PORT, SimTailscaleVerifier, SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use canban_client::{ApiMismatchError, SingleHostSession};
use canban_transport::TailscalePeerIdentity;
use canban_types::{
    HostConnectionState, HostTransport, TAILSCALE_AUTH_SENTINEL, is_device_token, is_pairing_code,
};
use serde_json::json;
use std::time::Duration;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "transport_auth");
    cfg.remote_hosts = vec!["remote-a".into()];
    cfg.min_latency_ms = 2;
    cfg.max_latency_ms = 12;

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();
        let addr = format!("remote-a:{SIM_PORT}");

        // Create a project on remote-a so we have a real entity for authenticated RPCs and project-scoped secrets.
        let dir = world.project_dir(&format!("auth-proj-{seed}"))?;
        let proj = world.client_call(
            "project.create",
            json!({ "host_id": "remote-a", "name": format!("Auth Proj {seed}"), "directory": dir }),
            None,
        )?;
        let pid = proj["id"].as_str().unwrap().to_owned();
        let raw_pid = canban_types::split_qualified_host_id(&pid)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| pid.clone());

        // -------------------------------------------------------------------------
        // (a) Single-use pairing invite + replay rejection + TTL expiration
        // -------------------------------------------------------------------------
        let invite = world.server_call(
            "remote-a",
            "device.invite.create",
            json!({ "ttl_secs": 300 }),
        )?;
        let code = invite["code"]
            .as_str()
            .expect("device.invite.create must return code")
            .to_owned();
        ensure!(
            is_pairing_code(&code),
            "Expected pairing code prefix, got {code}"
        );
        world.trace.mix_str(&code);

        // Pair a new device over real turmoil::net TCP using the single-use invite code.
        let paired_name = format!("Paired Laptop {seed}");
        let paired_transport = world.connect_raw_transport(&addr, &code, &paired_name)?;
        let upgraded_token = paired_transport
            .upgraded_token()
            .expect("pairing handshake must exchange invite code for a per-device token")
            .to_owned();
        ensure!(
            is_device_token(&upgraded_token),
            "Upgraded token must be a device token, got {upgraded_token}"
        );
        world.trace.mix_str(&upgraded_token);

        // Verify the active paired yamux/1 session can execute RPCs.
        let t_check = paired_transport.clone();
        let ping_res = world.client_exec(15_000, move |_| async move {
            t_check.call_async("app.ping", &json!({})).await
        })?;
        ensure!(
            ping_res["api_version"].as_u64() == Some(u64::from(canban_app_server::API_VERSION)),
            "Unexpected paired app.ping reply: {ping_res}"
        );

        // Verify a fresh connection using the issued per-device token works.
        let second_conn = world.connect_raw_transport(&addr, &upgraded_token, &paired_name)?;
        let t_second = second_conn.clone();
        let _ = world.client_exec(15_000, move |_| async move {
            t_second.call_async("project.list", &json!({})).await
        })?;
        second_conn.close();

        // Replaying the same single-use invite code a second time MUST be rejected!
        let replay_err = world
            .connect_raw_transport(&addr, &code, "Replay Attacker")
            .err()
            .expect("Replaying a consumed pairing invite code must fail");
        ensure!(
            replay_err.to_string().contains("auth_failed"),
            "Expected auth_failed on invite replay, got: {replay_err:#}"
        );

        // Create a short-lived invite (30s minimum TTL), advance virtual time past expiry, and verify rejection.
        let expiring = world.server_call(
            "remote-a",
            "device.invite.create",
            json!({ "ttl_secs": 30 }),
        )?;
        let exp_code = expiring["code"].as_str().unwrap().to_owned();
        world.advance(Duration::from_secs(35))?;
        let exp_err = world
            .connect_raw_transport(&addr, &exp_code, "Late Device")
            .err()
            .expect("Expired pairing invite code must fail");
        ensure!(
            exp_err.to_string().contains("auth_failed"),
            "Expected auth_failed on expired invite, got: {exp_err:#}"
        );
        world.hit("auth.invite_single_use");

        // -------------------------------------------------------------------------
        // (b) `device.revoke` + live `RevocationBus` stream kick
        // -------------------------------------------------------------------------
        let devs = world.server_call("remote-a", "device.list", json!({}))?;
        let paired_dev_id = devs["devices"]
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .find(|d| d["name"].as_str() == Some(paired_name.as_str()))
            })
            .and_then(|d| d["id"].as_str())
            .expect("Paired device must appear in device.list")
            .to_owned();

        // Revoke the device while `paired_transport` is still live & authenticated!
        let revoke_res = world.server_call(
            "remote-a",
            "device.revoke",
            json!({ "device_id": paired_dev_id }),
        )?;
        ensure!(
            revoke_res["revoked"].as_bool() == Some(true),
            "Expected device.revoke to return revoked=true, got {revoke_res}"
        );

        // Advance virtual time so the server's `RevocationBus` listener tears down the live yamux/1 session.
        world.advance(Duration::from_millis(250))?;
        let t_kicked = paired_transport.clone();
        let kicked_res = world.client_exec(15_000, move |_| async move {
            t_kicked.call_async("app.ping", &json!({})).await
        });
        ensure!(
            kicked_res.is_err(),
            "Revoked live yamux/1 session must reject subsequent RPCs, got {kicked_res:?}"
        );
        paired_transport.close();

        // Attempting to reconnect with the revoked device token must fail with `auth_failed`.
        let reconn_err = world
            .connect_raw_transport(&addr, &upgraded_token, &paired_name)
            .err()
            .expect("Reconnect with revoked device token must fail");
        ensure!(
            reconn_err.to_string().contains("auth_failed"),
            "Expected auth_failed reconnecting with revoked token, got: {reconn_err:#}"
        );
        world.hit("auth.revoke_kicks_live_stream");
        oracle.check_step_invariants(world)?;

        // -------------------------------------------------------------------------
        // (c) Tailscale WhoIs (`SimTailscaleVerifier`) + deterministic `ts_sess_` + `enc:v1:`
        // -------------------------------------------------------------------------
        let verifier = SimTailscaleVerifier::rejecting();
        world.set_tailscale_verifier("remote-a", Some(verifier.clone()))?;

        // Unauthorized peer MUST be rejected.
        let unauth_err = world
            .connect_raw_transport(&addr, TAILSCALE_AUTH_SENTINEL, "Unknown Tailscale Peer")
            .err()
            .expect("Unauthorized Tailscale peer must be rejected");
        ensure!(
            unauth_err.to_string().contains("auth_failed"),
            "Expected auth_failed for unauthorized Tailscale peer, got: {unauth_err:#}"
        );
        ensure!(
            verifier.attempts().iter().any(|(_, ok)| !ok),
            "SimTailscaleVerifier must record rejected peer attempt"
        );

        // Authorize the peer identity and verify deterministic `ts_sess_...` ephemeral token issuance.
        let ts_node = format!("nSimNode-{seed}");
        let ts_dev = format!("Sim iPad {seed}");
        verifier.set_default_identity(Some(TailscalePeerIdentity {
            node_id: ts_node.clone(),
            device_name: ts_dev.clone(),
            login_name: "sim-owner@example.invalid".into(),
        }));
        verifier.set_enabled(true);

        let ts_transport = world.connect_raw_transport(&addr, TAILSCALE_AUTH_SENTINEL, &ts_dev)?;
        let sess_tok = ts_transport
            .active_token()
            .expect("Tailscale session must receive an ephemeral session token")
            .to_owned();
        ensure!(
            sess_tok.starts_with("ts_sess_"),
            "Expected ephemeral ts_sess_ token, got {sess_tok}"
        );
        world.trace.mix_str(&sess_tok);

        // Verify Tailscale peer appears in `device.list` with `auth_kind == "tailscale"`.
        let ts_devs = world.server_call("remote-a", "device.list", json!({}))?;
        let found_ts = ts_devs["devices"].as_array().is_some_and(|arr| {
            arr.iter().any(|d| {
                d["id"].as_str() == Some(&format!("ts:{ts_node}"))
                    && d["auth_kind"].as_str() == Some("tailscale")
            })
        });
        ensure!(
            found_ts,
            "Authorized Tailscale session missing from device.list: {ts_devs}"
        );

        // Exchange an `enc:v1:` sealed secret over the Tailscale session using deterministic sealing.
        let secret_plain = format!("sk-ts-sealed-{seed}");
        oracle.track_secret(secret_plain.clone());
        let seal_id_gen = canban_types::deterministic_id_gen(seed ^ 0x5EA1_7A11_5CA1_E001);
        let mut save_params = json!({
            "scope": "project",
            "target_id": raw_pid,
            "entries": {
                "TS_SEALED_KEY": { "kind": "secret", "value": secret_plain },
                "TS_PLAIN_KEY": { "kind": "plain", "value": "tailscale-ok" },
            },
        });
        canban_client::store::seal_params_for_tcp_with_id_gen(
            "env.save",
            &mut save_params,
            Some(&sess_tok),
            seal_id_gen.as_ref(),
        )?;
        let sealed = save_params["entries"]["TS_SEALED_KEY"]["value"]
            .as_str()
            .expect("sealed secret value must be a string");
        ensure!(
            sealed.starts_with(canban_storage::vault::TRANSPORT_SEAL_PREFIX),
            "Expected enc:v1: sealed ciphertext, got {sealed}"
        );
        ensure!(
            !sealed.contains(&secret_plain),
            "Sealed ciphertext leaked plaintext secret"
        );
        world.trace.mix_str(sealed);

        let ts_t = ts_transport.clone();
        let save_res = world.client_exec(15_000, move |_| async move {
            ts_t.call_async("env.save", &save_params).await
        })?;
        ensure!(
            save_res["env"]["TS_SEALED_KEY"]["kind"].as_str() == Some("secret")
                && save_res["env"]["TS_SEALED_KEY"]["configured"].as_bool() == Some(true),
            "Expected sealed secret to be saved cleanly over Tailscale session: {save_res}"
        );

        // Verify the server unsealed and persisted the exact secret in its vault, while snapshots mask it.
        let srv_snap = world.server("remote-a").unwrap().snapshot()?;
        let srv_snap_json = serde_json::to_string(&srv_snap)?;
        ensure!(
            !srv_snap_json.contains(&secret_plain),
            "Server snapshot leaked plaintext Tailscale secret"
        );
        let preview =
            world.server_call("remote-a", "env.preview", json!({ "project_id": raw_pid }))?;
        let preview_has_secret = preview["rows"].as_array().is_some_and(|rows| {
            rows.iter().any(|r| {
                r["key"].as_str() == Some("TS_SEALED_KEY")
                    && r["is_secret"].as_bool() == Some(true)
                    && r["configured"].as_bool() == Some(true)
            })
        });
        ensure!(
            preview_has_secret,
            "Expected TS_SEALED_KEY in project env.preview: {preview}"
        );
        ts_transport.close();
        world.hit("auth.tailscale_ephemeral_enc");
        oracle.check_step_invariants(world)?;

        // -------------------------------------------------------------------------
        // (d) `ApiMismatch` via `desktop_api_override` + clean recovery
        // -------------------------------------------------------------------------
        let old_api = canban_app_server::API_VERSION.saturating_sub(2);
        world.set_desktop_api_override("remote-a", Some(old_api))?;

        // Direct `SingleHostSession::connect_checked_async_with_clock` must return typed `ApiMismatchError`.
        let remote_tok = world.servers["remote-a"].token().expect("remote-a token");
        let addr_check = addr.clone();
        let clock_check = world.servers["remote-a"].clock.clone();
        let mismatch_check = world.client_run_on("ui-client", 15_000, move |_| async move {
            let transport = HostTransport::Tcp {
                addr: addr_check,
                token: remote_tok,
            };
            let res = SingleHostSession::connect_checked_async_with_clock(
                &TurmoilConnector,
                "remote-a",
                &transport,
                "ApiMismatch Probe",
                json!(null),
                Some(canban_app_server::API_VERSION),
                clock_check.as_ref(),
            )
            .await;
            match res {
                Ok(_) => anyhow::bail!("Expected ApiMismatchError for downgraded remote-a"),
                Err(err) => {
                    let mismatch = err.downcast_ref::<ApiMismatchError>().ok_or_else(|| {
                        anyhow::anyhow!("Expected ApiMismatchError, got: {err:#}")
                    })?;
                    Ok((mismatch.server_api, mismatch.app_api))
                }
            }
        })?;
        ensure!(
            mismatch_check == (old_api, canban_app_server::API_VERSION),
            "Unexpected ApiMismatchError tuple: {mismatch_check:?}"
        );

        // `MultiHostClient` reconnect must surface `HostConnectionState::ApiMismatch` cleanly!
        world.client_reconnect(Some("remote-a"))?;
        world.wait_until(15_000, |w| {
            let summaries = w.client_host_summaries();
            Ok(summaries.iter().any(|s| {
                s.id == "remote-a"
                    && matches!(
                        s.connection,
                        HostConnectionState::ApiMismatch {
                            server_api,
                            app_api,
                            ..
                        } if server_api == old_api && app_api == canban_app_server::API_VERSION
                    )
            }))
        })?;
        world.hit("auth.api_mismatch");

        // Restore matching API version and verify clean recovery & full RPC operation.
        world.set_desktop_api_override("remote-a", None)?;
        world.client_reconnect(Some("remote-a"))?;
        world.heal_and_quiesce(20_000)?;

        let recovered_card = world.client_call(
            "card.create",
            json!({
                "project_id": pid,
                "title": format!("Post-ApiMismatch Card {seed}"),
                "body": "recovered",
            }),
            Some(&pid),
        )?;
        let recovered_cid = recovered_card["id"].as_str().unwrap().to_owned();
        oracle.record_committed_card(
            "remote-a",
            &recovered_cid,
            &pid,
            recovered_card["revision"].as_u64().unwrap_or(1),
        );

        world.heal_and_quiesce(15_000)?;
        oracle.check_quiescent_invariants(world)?;

        ensure!(
            world.servers["remote-a"].exec.blocked_commands().is_empty(),
            "transport_auth leaked unexpected OS commands: {:?}",
            world.servers["remote-a"].exec.blocked_commands()
        );
        Ok(finalize_digest(world))
    })
}
