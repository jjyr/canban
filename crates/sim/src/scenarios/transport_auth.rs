//! Scenario #13 — Real transport authentication & handshake security over `turmoil::net`,
//! driven end-to-end through real `MultiHostClient`:
//! (a) Real `MultiHostClient::pair_internal` pairing from a `device.invite.create` URI
//!     (`parse_pairing_uri` + `parse_pairing_invite` -> authenticated `yamux/1` handshake ->
//!     upgraded `dev_...` token persisted to `hosts.json` -> `HostConnectionState::Connected`),
//!     single-use invite replay & TTL expiry rejection, duplicate `server_id` + self-loopback
//!     rejection, and persisted-token reconnection across `crash_client` + `bounce_client`,
//! (b) Live `device.revoke` + `RevocationBus` kicking an active `MultiHostClient` `yamux/1`
//!     session -> transitioning to `HostConnectionState::AuthFailed`, halting retry loops over
//!     +16s virtual time (including background wakes/polls), failing routed RPCs cleanly with
//!     an auth error (with zero orphaned outbox items) while unaffected hosts stay `Connected`,
//! (c) Real `MultiHostClient` Tailscale WhoIs (`TAILSCALE_AUTH_SENTINEL` / `ts_auto`, no static
//!     device token) against `SimTailscaleVerifier`: unauthorized peer rejected, authorized peer
//!     negotiates deterministic ephemeral `ts_sess_...` token, `env.save` through `MultiHostClient`
//!     auto-seals via `slot.tcp_token()` + `seal_params_for_tcp_with_id_gen` over `enc:v1:` and
//!     unseals verbatim into the server's vault while snapshots mask it,
//! (d) `ApiMismatch` via `desktop_api_override` (`SingleHostSession` typed error + `MultiHostClient`
//!     `HostConnectionState::ApiMismatch` surfacing + clean recovery once restored).
use crate::{
    net::TurmoilConnector,
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SIM_PORT, SimTailscaleVerifier, SimWorldConfig, run_with_world},
};
use anyhow::{Context, Result, anyhow, ensure};
use canban_client::{
    ApiMismatchError, SingleHostSession, format_pairing_uri as format_client_pairing_uri,
    parse_pairing_invite,
};
use canban_transport::TailscalePeerIdentity;
use canban_types::{
    HostConnectionState, HostTransport, HostsFile, SavedHost, TAILSCALE_AUTH_SENTINEL,
    format_pairing_uri, is_device_token, is_pairing_code, parse_pairing_uri,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::{collections::BTreeSet, time::Duration};

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "transport_auth");
    cfg.remote_hosts = vec!["remote-a".into(), "remote-b".into()];
    cfg.min_latency_ms = 2;
    cfg.max_latency_ms = 12;

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();
        let addr_a = format!("remote-a:{SIM_PORT}");
        let addr_b = format!("remote-b:{SIM_PORT}");
        let addr_local = format!("local-server:{SIM_PORT}");
        let hosts_path = world.client_data_dir.join("hosts.json");

        // Create real projects on both remote-a and remote-b for authenticated RPCs and project-scoped secrets.
        let dir_a = world.project_dir(&format!("auth-proj-a-{seed}"))?;
        let proj_a = world.client_call(
            "project.create",
            json!({ "host_id": "remote-a", "name": format!("Auth Proj A {seed}"), "directory": dir_a }),
            None,
        )?;
        let pid_a = proj_a["id"].as_str().unwrap().to_owned();

        let dir_b = world.project_dir(&format!("auth-proj-b-{seed}"))?;
        let proj_b = world.client_call(
            "project.create",
            json!({ "host_id": "remote-b", "name": format!("Auth Proj B {seed}"), "directory": dir_b }),
            None,
        )?;
        let pid_b = proj_b["id"].as_str().unwrap().to_owned();
        let raw_pid_b = canban_types::split_qualified_host_id(&pid_b)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| pid_b.clone());

        let initial_remote_b_token = world.servers["remote-b"]
            .token()
            .expect("initial remote-b token");
        let remote_b_server_id = world.server_call("remote-b", "app.ping", json!({}))?["server_id"]
            .as_str()
            .expect("remote-b server_id")
            .to_owned();

        // -------------------------------------------------------------------------
        // (a) Real `MultiHostClient::pair_internal` end-to-end over `turmoil::net`:
        //     invite URI -> `parse_pairing_uri` -> `hosts.json` persistence ->
        //     single-use replay & TTL rejection -> duplicate `server_id` rejection ->
        //     client restart (`bounce_client`) reconnect using persisted token
        // -------------------------------------------------------------------------
        world.client_run_on("ui-client", 15_000, |client| async move {
            client.remove_host("remote-b")
        })?;
        ensure!(
            !world
                .client_host_summaries()
                .iter()
                .any(|s| s.id == "remote-b"),
            "remote-b must be removed before fresh pairing"
        );

        let pre_pair_devs = world.server_call("remote-b", "device.list", json!({}))?;
        let pre_pair_ids: BTreeSet<String> = pre_pair_devs["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| d["id"].as_str().map(str::to_owned))
            .collect();

        let invite = world.server_call(
            "remote-b",
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

        let pairing_uri = invite["uri"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format_pairing_uri(&addr_b, &code));
        let parsed_invite = parse_pairing_invite(&pairing_uri)
            .with_context(|| format!("parse_pairing_invite failed on {pairing_uri}"))?;
        let (parsed_addr_check, parsed_code_check) = parse_pairing_uri(&pairing_uri)
            .with_context(|| format!("parse_pairing_uri failed on {pairing_uri}"))?;
        ensure!(
            parsed_invite.addr == parsed_addr_check && parsed_invite.secret == parsed_code_check,
            "parse_pairing_uri and parse_pairing_invite diverged on {pairing_uri}"
        );

        // Pair `remote-b` through real `MultiHostClient::add_host_async` -> `pair_internal`.
        let uri_for_pair = pairing_uri.clone();
        let (paired_summary, active_paired_token) =
            world.client_run_on("ui-client", 30_000, move |client| async move {
                let (addr, token) = parse_pairing_uri(&uri_for_pair)
                    .ok_or_else(|| anyhow!("Failed to parse pairing URI: {uri_for_pair}"))?;
                let summary = client
                    .add_host_async(SavedHost {
                        id: "remote-b".into(),
                        name: "Paired Remote B".into(),
                        transport: HostTransport::Tcp { addr, token },
                        ssh_target: None,
                        server_id: None,
                        enabled: true,
                        added_at: 0,
                    })
                    .await?;
                let active_tok = client
                    .transport_for_host(&summary.id)?
                    .active_token()
                    .map(str::to_owned);
                Ok((summary, active_tok))
            })?;

        ensure!(
            paired_summary.id == "remote-b" && paired_summary.is_connected(),
            "MultiHostClient pairing must reach Connected for remote-b: {paired_summary:?}"
        );
        ensure!(
            paired_summary.server_id.as_deref() == Some(remote_b_server_id.as_str()),
            "Paired host summary missing expected server_id: {paired_summary:?}"
        );
        let upgraded_token = active_paired_token
            .expect("paired MultiHostClient transport must hold upgraded device token");
        ensure!(
            is_device_token(&upgraded_token) && upgraded_token != initial_remote_b_token,
            "Expected fresh per-device token from pairing, got {upgraded_token}"
        );
        world.trace.mix_str(&upgraded_token);

        // Verify `hosts.json` persisted the upgraded `dev_...` token (and never the single-use `pair_...` code).
        let raw_hosts_json = std::fs::read_to_string(&hosts_path)
            .with_context(|| format!("read {}", hosts_path.display()))?;
        ensure!(
            !raw_hosts_json.contains(&code),
            "hosts.json must not retain single-use pairing code"
        );
        let persisted_hosts: HostsFile = serde_json::from_str(&raw_hosts_json)?;
        let saved_b = persisted_hosts
            .hosts
            .iter()
            .find(|h| h.id == "remote-b")
            .context("remote-b missing from persisted hosts.json")?;
        match &saved_b.transport {
            HostTransport::Tcp { addr, token } => {
                ensure!(
                    addr == &addr_b && token == &upgraded_token,
                    "hosts.json persisted unexpected transport for remote-b: addr={addr} token={token}"
                );
            }
            other => anyhow::bail!("Expected Tcp transport in hosts.json, got {other:?}"),
        }

        // Verify routed RPCs work immediately over the newly paired MultiHostClient session and capture paired device ID.
        let post_pair_devs =
            world.client_call("device.list", json!({ "host_id": "remote-b" }), None)?;
        let new_dev_ids: Vec<String> = post_pair_devs["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| d["id"].as_str().map(str::to_owned))
            .filter(|id| !pre_pair_ids.contains(id))
            .collect();
        ensure!(
            new_dev_ids.len() == 1,
            "Expected exactly 1 newly paired device on remote-b, found {new_dev_ids:?}"
        );
        let paired_dev_id = new_dev_ids[0].clone();

        // Single-use invite replay via REAL MultiHostClient MUST be rejected with `auth_failed`.
        let replay_uri = pairing_uri.clone();
        let replay_err = world.client_run_on("ui-client", 15_000, move |client| async move {
            let (addr, token) =
                parse_pairing_uri(&replay_uri).ok_or_else(|| anyhow!("parse replay URI failed"))?;
            let res = client
                .add_host_async(SavedHost {
                    id: "remote-b-replay".into(),
                    name: "Replay Attempt".into(),
                    transport: HostTransport::Tcp { addr, token },
                    ssh_target: None,
                    server_id: None,
                    enabled: true,
                    added_at: 0,
                })
                .await;
            match res {
                Ok(s) => anyhow::bail!("Expected invite replay to fail, got {s:?}"),
                Err(e) => Ok(format!("{e:#}")),
            }
        })?;
        ensure!(
            replay_err.contains("auth_failed"),
            "Expected auth_failed on MultiHostClient invite replay, got: {replay_err}"
        );

        // Short-lived invite TTL expiry via REAL MultiHostClient MUST also fail with `auth_failed`.
        let expiring = world.server_call(
            "remote-b",
            "device.invite.create",
            json!({ "ttl_secs": 30 }),
        )?;
        let exp_code = expiring["code"].as_str().unwrap().to_owned();
        let exp_uri = format_pairing_uri(&addr_b, &exp_code);
        world.advance(Duration::from_secs(35))?;
        let exp_err = world.client_run_on("ui-client", 15_000, move |client| async move {
            let inv = parse_pairing_invite(&exp_uri)
                .ok_or_else(|| anyhow!("parse expiring invite failed"))?;
            match client.pair_with_invite(&inv, Some("Late Device")).await {
                Ok(s) => anyhow::bail!("Expected expired invite to fail, got {s:?}"),
                Err(e) => Ok(format!("{e:#}")),
            }
        })?;
        ensure!(
            exp_err.contains("auth_failed"),
            "Expected auth_failed on expired invite via MultiHostClient, got: {exp_err}"
        );
        world.hit("auth.invite_single_use");

        // Duplicate `server_id` rejection via REAL MultiHostClient:
        // 1) Pre-handshake rejection when `?sid=...` is encoded in the URI (must NOT consume the invite).
        let dup_sid_invite = world.server_call(
            "remote-b",
            "device.invite.create",
            json!({ "ttl_secs": 300 }),
        )?;
        let dup_sid_code = dup_sid_invite["code"].as_str().unwrap().to_owned();
        let dup_sid_uri = format_client_pairing_uri(
            &addr_b,
            &dup_sid_code,
            Some(&remote_b_server_id),
            Some("Dup Remote B"),
        );
        let dup_sid_err = world.client_run_on("ui-client", 15_000, move |client| async move {
            let inv = parse_pairing_invite(&dup_sid_uri)
                .ok_or_else(|| anyhow!("parse dup sid URI failed"))?;
            match client
                .add_host_async(SavedHost {
                    id: "remote-b-dup-sid".into(),
                    name: "Duplicate Remote B (SID)".into(),
                    transport: HostTransport::Tcp {
                        addr: inv.addr,
                        token: inv.secret,
                    },
                    ssh_target: None,
                    server_id: inv.server_id,
                    enabled: true,
                    added_at: 0,
                })
                .await
            {
                Ok(s) => anyhow::bail!("Expected duplicate server_id rejection, got {s:?}"),
                Err(e) => Ok(format!("{e:#}")),
            }
        })?;
        ensure!(
            dup_sid_err.contains("already connected"),
            "Expected duplicate server_id error for URI with sid, got: {dup_sid_err}"
        );

        // 2) Post-handshake rejection when URI omits `?sid=` (reuses `dup_sid_code` proving pre-check did not consume it!).
        let dup_bare_uri = format_pairing_uri(&addr_b, &dup_sid_code);
        let dup_bare_err = world.client_run_on("ui-client", 15_000, move |client| async move {
            let (addr, token) =
                parse_pairing_uri(&dup_bare_uri).ok_or_else(|| anyhow!("parse bare dup URI"))?;
            match client
                .add_host_async(SavedHost {
                    id: "remote-b-dup".into(),
                    name: "Duplicate Remote B".into(),
                    transport: HostTransport::Tcp { addr, token },
                    ssh_target: None,
                    server_id: None,
                    enabled: true,
                    added_at: 0,
                })
                .await
            {
                Ok(s) => anyhow::bail!("Expected duplicate server_id rejection, got {s:?}"),
                Err(e) => Ok(format!("{e:#}")),
            }
        })?;
        ensure!(
            dup_bare_err.contains("already connected"),
            "Expected duplicate server_id error after handshake, got: {dup_bare_err}"
        );

        // 3) Self-loopback guard: attempting to pair local-server via MultiHostClient is rejected.
        let local_invite = world.server_call(
            "local-server",
            "device.invite.create",
            json!({ "ttl_secs": 300 }),
        )?;
        let local_uri = format_pairing_uri(&addr_local, local_invite["code"].as_str().unwrap());
        let self_err = world.client_run_on("ui-client", 15_000, move |client| async move {
            let (addr, token) =
                parse_pairing_uri(&local_uri).ok_or_else(|| anyhow!("parse local URI"))?;
            match client
                .add_host_async(SavedHost {
                    id: "self-loop".into(),
                    name: "Self Loopback".into(),
                    transport: HostTransport::Tcp { addr, token },
                    ssh_target: None,
                    server_id: None,
                    enabled: true,
                    added_at: 0,
                })
                .await
            {
                Ok(s) => anyhow::bail!("Expected self-loopback rejection, got {s:?}"),
                Err(e) => Ok(format!("{e:#}")),
            }
        })?;
        ensure!(
            self_err.contains("already connected locally"),
            "Expected self-loopback error, got: {self_err}"
        );
        ensure!(
            !world.client_host_summaries().iter().any(|s| matches!(
                s.id.as_str(),
                "remote-b-dup-sid" | "remote-b-dup" | "self-loop"
            )),
            "Rejected duplicate/self hosts must not be added to host_summaries"
        );
        world.hit("auth.duplicate_server_rejected");

        // Persistence across client restart: crash + bounce `ui-client` on the same `client_data_dir`,
        // confirming `remote-b` reconnects automatically using the persisted paired token in `hosts.json`.
        world.crash_client()?;
        world.advance(Duration::from_millis(150))?;
        world.bounce_client()?;
        world.wait_until(15_000, |w| {
            let summaries = w.client_host_summaries();
            let a_ok = summaries
                .iter()
                .any(|s| s.id == "remote-a" && s.is_connected());
            let b_ok = summaries
                .iter()
                .any(|s| s.id == "remote-b" && s.is_connected());
            Ok(a_ok && b_ok)
        })?;
        let post_bounce_token = world.client_run_on("ui-client", 10_000, |client| async move {
            Ok(client
                .transport_for_host("remote-b")?
                .active_token()
                .map(str::to_owned))
        })?;
        ensure!(
            post_bounce_token.as_deref() == Some(upgraded_token.as_str()),
            "Client bounce must reconnect remote-b with persisted paired token ({upgraded_token}), got {post_bounce_token:?}"
        );
        let post_bounce_projs =
            world.client_call("project.list", json!({ "host_id": "remote-b" }), None)?;
        ensure!(
            post_bounce_projs
                .as_array()
                .is_some_and(|arr| arr.iter().any(|p| p["id"].as_str() == Some(pid_b.as_str()))),
            "Routed project.list on persisted paired remote-b failed after client restart: {post_bounce_projs}"
        );
        world.hit("auth.client_pair_persisted");

        // Let the supervisor settle its active `app.subscribe` push stream on `remote-b`.
        world.advance(Duration::from_millis(120))?;

        // -------------------------------------------------------------------------
        // (b) Real `MultiHostClient` live revocation -> `HostConnectionState::AuthFailed`
        //     + retry-loop halting + clean RPC auth errors + unaffected host health
        // -------------------------------------------------------------------------
        let revoke_res = world.server_call(
            "remote-b",
            "device.revoke",
            json!({ "device_id": paired_dev_id }),
        )?;
        ensure!(
            revoke_res["revoked"].as_bool() == Some(true),
            "Expected device.revoke to return revoked=true, got {revoke_res}"
        );

        // Wait for `RevocationBus` to kick the live `yamux/1` stream and for `MultiHostClient`
        // to observe the rejected reconnect and transition `remote-b` to `HostConnectionState::AuthFailed`.
        world.wait_until(10_000, |w| {
            let summaries = w.client_host_summaries();
            Ok(summaries.iter().any(|s| {
                s.id == "remote-b" && matches!(s.connection, HostConnectionState::AuthFailed { .. })
            }))
        })?;
        world.hit("auth.revoke_kicks_live_stream");

        oracle.check_auth_failed_zero_dials(world)?;
        let dials_at_auth_failed = world.client_dial_count("ui-client", "remote-b");
        let reconnect_count_at_auth_failed = world
            .client_host_summaries()
            .into_iter()
            .find(|s| s.id == "remote-b")
            .map(|s| s.reconnect_count)
            .expect("remote-b summary present");

        // Advance +16s virtual time and exercise background wakes/foreground/polls to verify
        // `MultiHostClient` halts retry loops and does NOT keep hammering reconnects!
        world.advance(Duration::from_secs(8))?;
        world.client_run_on("ui-client", 10_000, |client| async move {
            client.wake_host("remote-b");
            client.wake_all();
            client.on_foreground();
            client.poll_remotes_once_async().await;
            Ok(())
        })?;
        world.advance(Duration::from_secs(8))?;

        let summary_b_after_wait = world
            .client_host_summaries()
            .into_iter()
            .find(|s| s.id == "remote-b")
            .expect("remote-b summary after wait");
        ensure!(
            matches!(
                summary_b_after_wait.connection,
                HostConnectionState::AuthFailed { .. }
            ),
            "Revoked host must stay in AuthFailed after +16s, got {:?}",
            summary_b_after_wait.connection
        );
        ensure!(
            summary_b_after_wait.reconnect_count == reconnect_count_at_auth_failed,
            "MultiHostClient kept retrying revoked host! reconnect_count grew from {reconnect_count_at_auth_failed} to {}",
            summary_b_after_wait.reconnect_count
        );
        oracle.check_auth_failed_zero_dials(world)?;
        oracle.assert_auth_failed_zero_dials_for(
            world,
            "ui-client",
            "remote-b",
            dials_at_auth_failed,
            reconnect_count_at_auth_failed,
        )?;

        // RPCs targeted at the revoked host must fail cleanly with an auth error (and NEVER enqueue offline outbox items).
        let list_err = world
            .client_call("project.list", json!({ "host_id": "remote-b" }), None)
            .expect_err("project.list against revoked host must fail");
        ensure!(
            format!("{list_err:#}")
                .to_ascii_lowercase()
                .contains("auth_failed"),
            "Expected auth_failed for project.list on revoked host, got: {list_err:#}"
        );
        let card_err = world
            .client_call(
                "card.create",
                json!({
                    "project_id": pid_b,
                    "title": "Rejected On Revoked Host",
                    "body": "must fail auth cleanly",
                }),
                Some(&pid_b),
            )
            .expect_err("card.create against revoked host must fail");
        ensure!(
            format!("{card_err:#}")
                .to_ascii_lowercase()
                .contains("auth_failed"),
            "Expected auth_failed for card.create on revoked host, got: {card_err:#}"
        );
        ensure!(
            world.client_sync_items().is_empty(),
            "card.create on AuthFailed host must not enqueue orphaned outbox items: {:?}",
            world.client_sync_items()
        );
        let env_err = world
            .client_call(
                "env.save",
                json!({
                    "host_id": "remote-b",
                    "scope": "project",
                    "target_id": raw_pid_b,
                    "entries": { "BLOCKED_KEY": { "kind": "plain", "value": "nope" } },
                }),
                Some(&pid_b),
            )
            .expect_err("env.save against revoked host must fail");
        ensure!(
            format!("{env_err:#}")
                .to_ascii_lowercase()
                .contains("auth_failed"),
            "Expected auth_failed for env.save on revoked host, got: {env_err:#}"
        );
        oracle.assert_auth_failed_zero_dials_for(
            world,
            "ui-client",
            "remote-b",
            dials_at_auth_failed,
            reconnect_count_at_auth_failed,
        )?;

        // Unaffected hosts (`local` and `remote-a`) stay Connected and healthy!
        let healthy_summaries = world.client_host_summaries();
        ensure!(
            healthy_summaries
                .iter()
                .any(|s| s.id == "local" && s.is_connected()),
            "local host should remain Connected during remote-b revocation"
        );
        ensure!(
            healthy_summaries
                .iter()
                .any(|s| s.id == "remote-a" && s.is_connected()),
            "remote-a must remain Connected while remote-b is revoked"
        );
        let healthy_card = world.client_call(
            "card.create",
            json!({
                "project_id": pid_a,
                "title": format!("Healthy While B Revoked {seed}"),
                "body": "remote-a unaffected",
            }),
            Some(&pid_a),
        )?;
        oracle.record_committed_card(
            "remote-a",
            healthy_card["id"].as_str().unwrap(),
            &pid_a,
            healthy_card["revision"].as_u64().unwrap_or(1),
        );
        world.hit("auth.client_revoked_auth_failed");
        oracle.check_step_invariants(world)?;

        // -------------------------------------------------------------------------
        // (c) Real `MultiHostClient` Tailscale WhoIs (`TAILSCALE_AUTH_SENTINEL` +
        //     deterministic `ts_sess_...` + automatic `enc:v1:` sealing via `slot.tcp_token()`)
        // -------------------------------------------------------------------------
        let verifier = SimTailscaleVerifier::rejecting();
        world.set_tailscale_verifier("remote-b", Some(verifier.clone()))?;

        // Remove the revoked static-token slot for `remote-b` and re-register using ONLY `TAILSCALE_AUTH_SENTINEL`.
        world.client_run_on("ui-client", 15_000, |client| async move {
            client.remove_host("remote-b")
        })?;

        // Unauthorized Tailscale peer MUST be rejected through real `MultiHostClient`.
        let addr_b_unauth = addr_b.clone();
        let ts_unauth_err = world.client_run_on("ui-client", 15_000, move |client| async move {
            match client
                .add_host_async(SavedHost {
                    id: "remote-b".into(),
                    name: "Tailscale Remote B".into(),
                    transport: HostTransport::Tcp {
                        addr: addr_b_unauth,
                        token: TAILSCALE_AUTH_SENTINEL.into(),
                    },
                    ssh_target: None,
                    server_id: None,
                    enabled: true,
                    added_at: 0,
                })
                .await
            {
                Ok(s) => anyhow::bail!("Expected unauthorized Tailscale peer to fail, got {s:?}"),
                Err(e) => Ok(format!("{e:#}")),
            }
        })?;
        ensure!(
            ts_unauth_err.contains("auth_failed"),
            "Expected auth_failed for unauthorized Tailscale peer via MultiHostClient, got: {ts_unauth_err}"
        );
        ensure!(
            verifier.attempts().iter().any(|(_, ok)| !ok),
            "SimTailscaleVerifier must record rejected peer attempt"
        );
        ensure!(
            !world
                .client_host_summaries()
                .iter()
                .any(|s| s.id == "remote-b"),
            "Rejected Tailscale host must not be persisted in host_summaries"
        );

        // Authorize the Tailscale peer identity and connect `remote-b` through `MultiHostClient` with `ts_auto`.
        let ts_node = format!("nSimNode-{seed}");
        let ts_dev = format!("Sim iPad {seed}");
        verifier.set_default_identity(Some(TailscalePeerIdentity {
            node_id: ts_node.clone(),
            device_name: ts_dev.clone(),
            login_name: "sim-owner@example.invalid".into(),
        }));
        verifier.set_enabled(true);

        let addr_b_ts = addr_b.clone();
        let (ts_summary, ts_active_tok) =
            world.client_run_on("ui-client", 30_000, move |client| async move {
                let sum = client
                    .add_host_async(SavedHost {
                        id: "remote-b".into(),
                        name: "Tailscale Remote B".into(),
                        transport: HostTransport::Tcp {
                            addr: addr_b_ts,
                            token: TAILSCALE_AUTH_SENTINEL.into(),
                        },
                        ssh_target: None,
                        server_id: None,
                        enabled: true,
                        added_at: 0,
                    })
                    .await?;
                let active_tok = client
                    .transport_for_host(&sum.id)?
                    .active_token()
                    .map(str::to_owned);
                Ok((sum, active_tok))
            })?;
        ensure!(
            ts_summary.id == "remote-b" && ts_summary.is_connected(),
            "Tailscale MultiHostClient host must reach Connected: {ts_summary:?}"
        );
        let sess_tok = ts_active_tok
            .expect("Tailscale MultiHostClient session must hold ephemeral ts_sess_ token");
        ensure!(
            sess_tok.starts_with("ts_sess_"),
            "Expected ephemeral ts_sess_ token on MultiHostClient transport, got {sess_tok}"
        );
        world.trace.mix_str(&sess_tok);

        // Verify `hosts.json` preserves `TAILSCALE_AUTH_SENTINEL` (no static `dev_` or ephemeral `ts_sess_` persisted).
        let ts_hosts: HostsFile = serde_json::from_slice(&std::fs::read(&hosts_path)?)?;
        let saved_ts_b = ts_hosts
            .hosts
            .iter()
            .find(|h| h.id == "remote-b")
            .context("Tailscale remote-b missing from hosts.json")?;
        match &saved_ts_b.transport {
            HostTransport::Tcp { token, .. } => ensure!(
                token == TAILSCALE_AUTH_SENTINEL,
                "Tailscale host must persist ts_auto sentinel in hosts.json, got {token}"
            ),
            other => anyhow::bail!("Expected Tcp transport for Tailscale host, got {other:?}"),
        }

        // Verify Tailscale peer appears in `device.list` routed via `MultiHostClient` with `auth_kind == "tailscale"`.
        let ts_devs = world.client_call("device.list", json!({ "host_id": "remote-b" }), None)?;
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

        // Save a plaintext secret THROUGH `MultiHostClient::call_routed_async("env.save", ...)`:
        // `slot.tcp_token()` MUST pick the live `ts_sess_...` token (since `ts_auto` is only 7 bytes
        // and would fail `transport_key`'s >=16-byte check!) and auto-seal via `seal_params_for_tcp_with_id_gen`.
        let secret_plain = format!("sk-ts-sealed-{seed}");
        oracle.track_secret(secret_plain.clone());
        let save_res = world.client_call(
            "env.save",
            json!({
                "host_id": "remote-b",
                "scope": "project",
                "target_id": raw_pid_b,
                "entries": {
                    "TS_SEALED_KEY": { "kind": "secret", "value": secret_plain },
                    "TS_PLAIN_KEY": { "kind": "plain", "value": "tailscale-ok" },
                },
            }),
            Some(&pid_b),
        )?;
        ensure!(
            save_res["env"]["TS_SEALED_KEY"]["kind"].as_str() == Some("secret")
                && save_res["env"]["TS_SEALED_KEY"]["configured"].as_bool() == Some(true),
            "Expected MultiHostClient auto-sealed secret to save cleanly over Tailscale: {save_res}"
        );

        // Verify the server actually unsealed `enc:v1:` and persisted the exact plaintext into its vault,
        // while both server and merged client snapshots mask it.
        let srv_b_home = world.servers["remote-b"].home.clone();
        let db_conn = Connection::open_with_flags(
            srv_b_home.join("canban.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let decrypted_scope = canban_storage::env::list_decrypted_scope(
            &db_conn,
            &srv_b_home,
            "project",
            &raw_pid_b,
        )?;
        let stored_plain = decrypted_scope
            .get("TS_SEALED_KEY")
            .map(|(v, _)| v.as_str().to_owned())
            .context("TS_SEALED_KEY missing from remote-b decrypted vault")?;
        ensure!(
            stored_plain == secret_plain,
            "Server vault did not unseal Tailscale secret verbatim: expected {secret_plain:?}, got {stored_plain:?}"
        );

        let srv_snap_json = serde_json::to_string(&world.server("remote-b").unwrap().snapshot()?)?;
        let merged_snap_json = serde_json::to_string(&world.client_merged_snapshot()?)?;
        ensure!(
            !srv_snap_json.contains(&secret_plain) && !merged_snap_json.contains(&secret_plain),
            "Snapshot leaked plaintext Tailscale secret"
        );
        let preview = world.client_call(
            "env.preview",
            json!({ "host_id": "remote-b", "project_id": raw_pid_b }),
            Some(&pid_b),
        )?;
        let preview_has_secret = preview["rows"].as_array().is_some_and(|rows| {
            rows.iter().any(|r| {
                r["key"].as_str() == Some("TS_SEALED_KEY")
                    && r["is_secret"].as_bool() == Some(true)
                    && r["configured"].as_bool() == Some(true)
            })
        });
        ensure!(
            preview_has_secret,
            "Expected TS_SEALED_KEY in routed project env.preview: {preview}"
        );

        world.hit("auth.tailscale_ephemeral_enc");
        world.hit("auth.client_tailscale_autoseal");
        oracle.check_step_invariants(world)?;

        // -------------------------------------------------------------------------
        // (d) `ApiMismatch` via `desktop_api_override` + clean recovery
        // -------------------------------------------------------------------------
        let old_api = canban_app_server::API_VERSION.saturating_sub(2);
        world.set_desktop_api_override("remote-a", Some(old_api))?;

        // Direct `SingleHostSession::connect_checked_async_with_clock` must return typed `ApiMismatchError`.
        let remote_tok = world.servers["remote-a"].token().expect("remote-a token");
        let addr_check = addr_a.clone();
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

        // `MultiHostClient` reconnect must surface `HostConnectionState::ApiMismatch` cleanly
        // while Tailscale-authenticated `remote-b` remains Connected!
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
        ensure!(
            world
                .client_host_summaries()
                .iter()
                .any(|s| s.id == "remote-b" && s.is_connected()),
            "Tailscale remote-b must remain Connected during remote-a ApiMismatch"
        );
        world.hit("auth.api_mismatch");

        // Restore matching API version and verify clean recovery & full RPC operation across both hosts.
        world.set_desktop_api_override("remote-a", None)?;
        world.client_reconnect(Some("remote-a"))?;
        world.heal_and_quiesce(20_000)?;

        let recovered_card_a = world.client_call(
            "card.create",
            json!({
                "project_id": pid_a,
                "title": format!("Post-ApiMismatch Card A {seed}"),
                "body": "recovered",
            }),
            Some(&pid_a),
        )?;
        oracle.record_committed_card(
            "remote-a",
            recovered_card_a["id"].as_str().unwrap(),
            &pid_a,
            recovered_card_a["revision"].as_u64().unwrap_or(1),
        );

        let recovered_card_b = world.client_call(
            "card.create",
            json!({
                "project_id": pid_b,
                "title": format!("Post-Tailscale Card B {seed}"),
                "body": "tailscale-active",
            }),
            Some(&pid_b),
        )?;
        oracle.record_committed_card(
            "remote-b",
            recovered_card_b["id"].as_str().unwrap(),
            &pid_b,
            recovered_card_b["revision"].as_u64().unwrap_or(1),
        );

        world.heal_and_quiesce(15_000)?;
        oracle.check_quiescent_invariants(world)?;

        for host in ["remote-a", "remote-b"] {
            ensure!(
                world.servers[host].exec.blocked_commands().is_empty(),
                "transport_auth leaked unexpected OS commands on {host}: {:?}",
                world.servers[host].exec.blocked_commands()
            );
        }
        Ok(finalize_digest(world))
    })
}
