//! Scenario #13 — Real Noise_XX E2EE transport authentication & handshake security over `turmoil::net`,
//! driven end-to-end through real `MultiHostClient`:
//! (a) Strict `?pk=` host-key pinning (Option B), wrong-`?pk=` pre-Msg3 abort without consuming
//!     single-use `pair_...` code, real `MultiHostClient::pair_with_invite` over Noise_XX + `yamux/1`,
//!     wire-tap confidentiality verification (`CBN1` preface, zero plaintext secrets/codes/RPCs on wire),
//!     single-use invite replay & TTL expiry rejection, duplicate `server_id` + self-loopback
//!     rejection, and persisted `client_noise.key` + pinned `server_pubkey` reconnection across
//!     `crash_client` + `bounce_client`,
//! (b) Live `device.revoke` + `RevocationBus` kicking an active `MultiHostClient` Noise `yamux/1`
//!     session -> transitioning to `HostConnectionState::AuthFailed`, halting retry loops over
//!     +16s virtual time (0 background dials), failing routed RPCs cleanly with an auth error
//!     (zero orphaned outbox items) while unaffected hosts stay `Connected`,
//! (c) Unauthorized client Noise keypair rejection, legacy plaintext JSON-RPC rejection (`-32002`),
//!     and deterministic bit-flip ciphertext tampering fault (`FaultyStream::corrupt_read_byte_at`),
//! (d) Real `MultiHostClient` Tailscale WhoIs (`tailscale_auth: true`, `server_pubkey: None` TOFU)
//!     against `SimTailscaleVerifier`: unauthorized peer rejected, authorized peer connects & pins
//!     `server_pubkey` in `hosts.json`, `env.save` secret flows encrypted purely inside Noise AEAD
//!     (verified against wire-tap bytes) and unseals into server vault while snapshots mask it,
//! (e) Server `noise.key` rotation -> `HostConnectionState::KeyMismatch` + 0 background dials +
//!     clean re-pairing recovery,
//! (f) `ApiMismatch` via `desktop_api_override` (`SingleHostSession` typed error + `MultiHostClient`
//!     `HostConnectionState::ApiMismatch` surfacing + clean recovery),
//! (g) Push-topic separation, legacy no-`app.subscribe` polling fallback, `set_host_enabled` TCP
//!     drop, side-channel `UtilitiesChanged`/`UsageChanged` forwarding, and `host.probe`/`discover_canban`.
use crate::{
    net::{StreamFaultConfig, TurmoilConnector},
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SIM_PORT, SimTailscaleVerifier, SimWorldConfig, run_with_world},
};
use anyhow::{Context, Result, anyhow, ensure};
use canban_client::{
    ApiMismatchError, ClientEvent, SingleHostSession,
    format_pairing_uri as format_client_pairing_uri, parse_pairing_invite,
};
use canban_transport::{
    HostKeyMismatchError, NOISE_PREFACE, TailscalePeerIdentity, TcpAuthMode, decode_pubkey,
    pubkey_fingerprint,
};
use canban_types::{
    HostConnectionState, HostTransport, HostsFile, SavedHost, format_pairing_uri, is_pairing_code,
    parse_pairing_uri,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "transport_auth");
    cfg.framed_local_transport = true;
    cfg.remote_hosts = vec!["remote-a".into(), "remote-b.ts.net".into()];
    cfg.min_latency_ms = 2;
    cfg.max_latency_ms = 12;

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();
        let addr_a = format!("remote-a:{SIM_PORT}");
        let addr_b = format!("remote-b.ts.net:{SIM_PORT}");
        let addr_local = format!("local-server:{SIM_PORT}");
        let hosts_path = world.client_data_dir.join("hosts.json");

        // Create real projects on both remote-a and remote-b.ts.net for authenticated RPCs and project-scoped secrets.
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
            json!({ "host_id": "remote-b.ts.net", "name": format!("Auth Proj B {seed}"), "directory": dir_b }),
            None,
        )?;
        let pid_b = proj_b["id"].as_str().unwrap().to_owned();
        let raw_pid_b = canban_types::split_qualified_host_id(&pid_b)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| pid_b.clone());

        let remote_a_pk = world.servers["remote-a"]
            .server_pubkey()
            .expect("remote-a server_pubkey");
        let remote_b_pk = world.servers["remote-b.ts.net"]
            .server_pubkey()
            .expect("remote-b.ts.net server_pubkey");
        let local_pk = world.servers["local-server"]
            .server_pubkey()
            .expect("local-server server_pubkey");
        let remote_b_server_id =
            world.server_call("remote-b.ts.net", "app.ping", json!({}))?["server_id"]
                .as_str()
                .expect("remote-b.ts.net server_id")
                .to_owned();
        let remote_b_fp = pubkey_fingerprint(&decode_pubkey(&remote_b_pk)?);

        // -------------------------------------------------------------------------
        // (a) Strict Option B `?pk=` pinning, wrong-`?pk=` pre-Msg3 abort, real
        //     `MultiHostClient::pair_with_invite` over Noise_XX, wire-tap check,
        //     replay/TTL rejection, duplicate `server_id` + self-loopback, bounce!
        // -------------------------------------------------------------------------
        // Option B: non-Tailscale pairing URI missing `?pk=` MUST be rejected at parse time
        // AND at `MultiHostClient::add_host_async` time!
        ensure!(
            parse_pairing_uri("canban://192.168.1.50:7680#pair_abc").is_none()
                && parse_pairing_invite("canban://192.168.1.50:7680#pair_abc").is_none(),
            "Option B violation: non-Tailscale URI missing ?pk= must be rejected"
        );
        let non_ts_err = world.client_run_on("ui-client", 15_000, |client| async move {
            match client
                .add_host_async(SavedHost {
                    id: "unpinned-lan".into(),
                    name: "Unpinned LAN".into(),
                    transport: HostTransport::Tcp {
                        addr: "192.168.1.50:7680".into(),
                        server_pubkey: None,
                        tailscale_auth: false,
                    },
                    ssh_target: None,
                    server_id: None,
                    enabled: true,
                    added_at: 0,
                })
                .await
            {
                Ok(s) => anyhow::bail!("Expected unpinned non-Tailscale host to fail, got {s:?}"),
                Err(e) => Ok(format!("{e:#}")),
            }
        })?;
        ensure!(
            non_ts_err.contains("pinned host public key"),
            "Option B violation: expected pinned host public key error, got: {non_ts_err}"
        );

        // Remove `remote-b.ts.net` from `ui-client` AND revoke `ui-client`'s pre-authorized pubkey on `remote-b.ts.net`
        // so fresh pairing genuinely tests Noise_XX + one-time `pair_...` authorization!
        world.client_run_on("ui-client", 15_000, |client| async move {
            client.remove_host("remote-b.ts.net")
        })?;
        ensure!(
            !world
                .client_host_summaries()
                .iter()
                .any(|s| s.id == "remote-b.ts.net"),
            "remote-b.ts.net must be removed before fresh pairing"
        );
        let initial_devs_b = world.server_call("remote-b.ts.net", "device.list", json!({}))?;
        for dev in initial_devs_b["devices"].as_array().into_iter().flatten() {
            if let Some(id) = dev["id"].as_str() {
                world.server_call(
                    "remote-b.ts.net",
                    "device.revoke",
                    json!({ "device_id": id }),
                )?;
            }
        }
        let pre_pair_devs = world.server_call("remote-b.ts.net", "device.list", json!({}))?;
        let pre_pair_ids: BTreeSet<String> = pre_pair_devs["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| d["id"].as_str().map(str::to_owned))
            .collect();

        let invite = world.server_call(
            "remote-b.ts.net",
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
        ensure!(
            invite["server_pubkey"].as_str() == Some(remote_b_pk.as_str()),
            "device.invite.create must include server_pubkey"
        );
        world.trace.mix_str(&code);

        let pairing_uri = format_pairing_uri(&addr_b, &remote_b_pk, &code);
        let parsed_invite = parse_pairing_invite(&pairing_uri)
            .with_context(|| format!("parse_pairing_invite failed on {pairing_uri}"))?;
        let (parsed_addr_check, parsed_pk_check, parsed_code_check) =
            parse_pairing_uri(&pairing_uri)
                .with_context(|| format!("parse_pairing_uri failed on {pairing_uri}"))?;
        ensure!(
            parsed_invite.addr == parsed_addr_check
                && parsed_invite.server_pubkey == parsed_pk_check
                && parsed_invite.pairing_code.as_deref() == Some(parsed_code_check.as_str()),
            "parse_pairing_uri and parse_pairing_invite diverged on {pairing_uri}"
        );

        // Wrong `?pk=` MUST abort BEFORE Noise Msg 3 with `HostKeyMismatchError`, AND leave
        // the single-use `pair_...` code UNCONSUMED on `remote-b`!
        let wrong_kp = world.ephemeral_keypair("wrong-server-pk")?;
        let wrong_pk_b64 = wrong_kp.public_b64();
        let client_kp = world.client_keypair("ui-client")?;
        let mismatch_err = world
            .connect_raw_transport(
                &addr_b,
                Arc::clone(&client_kp),
                Some(wrong_pk_b64.clone()),
                TcpAuthMode::PairingCode(code.clone()),
                "Wrong PK Probe",
            )
            .err()
            .context("connecting with wrong expected_server_pubkey must fail")?;
        ensure!(
            mismatch_err
                .downcast_ref::<HostKeyMismatchError>()
                .is_some()
                || format!("{mismatch_err:#}").contains("Host key mismatch"),
            "Expected HostKeyMismatchError for wrong ?pk= ({wrong_pk_b64}), got: {mismatch_err:#}"
        );
        world.hit("noise.host_key_pinning");

        // Attach wire-tap on `remote-b` before pairing to verify Noise_XX wire confidentiality!
        let wire_tap = Arc::new(Mutex::new(Vec::<u8>::new()));
        world
            .stream_fault_policy()
            .set_wire_tap("remote-b.ts.net", Some(Arc::clone(&wire_tap)));

        // Pair `remote-b` through real `MultiHostClient::add_host_with_pairing_code_async`
        // reusing the SAME `code` (proving the wrong-`?pk=` abort did NOT burn it!).
        let uri_for_pair = pairing_uri.clone();
        let paired_summary =
            world.client_run_on("ui-client", 30_000, move |client| async move {
                let (addr, server_pubkey, pair_code) = parse_pairing_uri(&uri_for_pair)
                    .ok_or_else(|| anyhow!("Failed to parse pairing URI: {uri_for_pair}"))?;
                client
                    .add_host_with_pairing_code_async(
                        SavedHost {
                            id: "remote-b.ts.net".into(),
                            name: "Paired Remote B".into(),
                            transport: HostTransport::Tcp {
                                addr,
                                server_pubkey,
                                tailscale_auth: false,
                            },
                            ssh_target: None,
                            server_id: None,
                            enabled: true,
                            added_at: 0,
                        },
                        Some(pair_code),
                    )
                    .await
            })?;

        ensure!(
            paired_summary.id == "remote-b.ts.net" && paired_summary.is_connected(),
            "MultiHostClient Noise pairing must reach Connected for remote-b: {paired_summary:?}"
        );
        ensure!(
            paired_summary.server_id.as_deref() == Some(remote_b_server_id.as_str()),
            "Paired host summary missing expected server_id: {paired_summary:?}"
        );
        ensure!(
            paired_summary.server_pubkey_fingerprint.as_deref() == Some(remote_b_fp.as_str()),
            "Paired host summary missing expected key fingerprint {remote_b_fp}: {paired_summary:?}"
        );

        // Verify wire-tap captured `CBN1` preface and ZERO plaintext `pair_...` code!
        {
            let tapped = wire_tap.lock().unwrap();
            ensure!(
                contains_bytes(&tapped, NOISE_PREFACE),
                "Wire tap did not observe CBN1 Noise preface ({} bytes captured)",
                tapped.len()
            );
            ensure!(
                !contains_bytes(&tapped, code.as_bytes()),
                "CRITICAL: raw TCP wire bytes leaked plaintext pairing code!"
            );
            ensure!(
                !contains_bytes(&tapped, b"app.ping"),
                "CRITICAL: raw TCP wire bytes leaked plaintext JSON-RPC method name!"
            );
        }

        // Verify `hosts.json` persisted pinned `server_pubkey` and never retained the single-use `pair_...` code.
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
            .find(|h| h.id == "remote-b.ts.net")
            .context("remote-b missing from persisted hosts.json")?;
        match &saved_b.transport {
            HostTransport::Tcp {
                addr,
                server_pubkey,
                tailscale_auth,
            } => {
                ensure!(
                    addr == &addr_b
                        && server_pubkey.as_deref() == Some(remote_b_pk.as_str())
                        && !*tailscale_auth,
                    "hosts.json persisted unexpected transport for remote-b: addr={addr} pk={server_pubkey:?} ts={tailscale_auth}"
                );
            }
            other => anyhow::bail!("Expected Tcp transport in hosts.json, got {other:?}"),
        }

        // Verify routed RPCs work immediately over the newly paired Noise session and capture paired device ID.
        let post_pair_devs =
            world.client_call("device.list", json!({ "host_id": "remote-b.ts.net" }), None)?;
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

        // Single-use invite replay with an unauthorized client keypair MUST be rejected with `auth_failed`.
        let replay_kp = world.ephemeral_keypair("replay-client")?;
        let replay_err = world
            .connect_raw_transport(
                &addr_b,
                replay_kp,
                Some(remote_b_pk.clone()),
                TcpAuthMode::PairingCode(code.clone()),
                "Replay Attempt",
            )
            .err()
            .context("invite replay must fail")?;
        ensure!(
            format!("{replay_err:#}").contains("auth_failed"),
            "Expected auth_failed on invite replay, got: {replay_err:#}"
        );

        // Short-lived invite TTL expiry via unauthorized client keypair MUST also fail with `auth_failed`.
        let expiring = world.server_call(
            "remote-b.ts.net",
            "device.invite.create",
            json!({ "ttl_secs": 30 }),
        )?;
        let exp_code = expiring["code"].as_str().unwrap().to_owned();
        world.advance(Duration::from_secs(35))?;
        let late_kp = world.ephemeral_keypair("late-client")?;
        let exp_err = world
            .connect_raw_transport(
                &addr_b,
                late_kp,
                Some(remote_b_pk.clone()),
                TcpAuthMode::PairingCode(exp_code),
                "Late Device",
            )
            .err()
            .context("expired invite must fail")?;
        ensure!(
            format!("{exp_err:#}").contains("auth_failed"),
            "Expected auth_failed on expired invite, got: {exp_err:#}"
        );
        world.hit("auth.invite_single_use");

        // Duplicate `server_id` rejection via REAL MultiHostClient:
        // 1) Pre-handshake rejection when `?sid=...` is encoded in the URI (must NOT consume the invite).
        let dup_sid_invite = world.server_call(
            "remote-b.ts.net",
            "device.invite.create",
            json!({ "ttl_secs": 300 }),
        )?;
        let dup_sid_code = dup_sid_invite["code"].as_str().unwrap().to_owned();
        let dup_sid_uri = format_client_pairing_uri(
            &addr_b,
            &remote_b_pk,
            &dup_sid_code,
            Some(&remote_b_server_id),
            Some("Dup Remote B"),
        );
        let dup_sid_err = world.client_run_on("ui-client", 15_000, move |client| async move {
            let inv = parse_pairing_invite(&dup_sid_uri)
                .ok_or_else(|| anyhow!("parse dup sid URI failed"))?;
            match client
                .add_host_with_pairing_code_async(
                    SavedHost {
                        id: "remote-b-dup-sid".into(),
                        name: "Duplicate Remote B (SID)".into(),
                        transport: HostTransport::Tcp {
                            addr: inv.addr,
                            server_pubkey: inv.server_pubkey,
                            tailscale_auth: inv.tailscale_auth,
                        },
                        ssh_target: None,
                        server_id: inv.server_id,
                        enabled: true,
                        added_at: 0,
                    },
                    inv.pairing_code,
                )
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
        let dup_bare_uri = format_pairing_uri(&addr_b, &remote_b_pk, &dup_sid_code);
        let dup_bare_err = world.client_run_on("ui-client", 15_000, move |client| async move {
            let (addr, server_pubkey, pair_code) =
                parse_pairing_uri(&dup_bare_uri).ok_or_else(|| anyhow!("parse bare dup URI"))?;
            match client
                .add_host_with_pairing_code_async(
                    SavedHost {
                        id: "remote-b-dup".into(),
                        name: "Duplicate Remote B".into(),
                        transport: HostTransport::Tcp {
                            addr,
                            server_pubkey,
                            tailscale_auth: false,
                        },
                        ssh_target: None,
                        server_id: None,
                        enabled: true,
                        added_at: 0,
                    },
                    Some(pair_code),
                )
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
        let local_uri = format_pairing_uri(
            &addr_local,
            &local_pk,
            local_invite["code"].as_str().unwrap(),
        );
        let self_err = world.client_run_on("ui-client", 15_000, move |client| async move {
            let (addr, server_pubkey, pair_code) =
                parse_pairing_uri(&local_uri).ok_or_else(|| anyhow!("parse local URI"))?;
            match client
                .add_host_with_pairing_code_async(
                    SavedHost {
                        id: "self-loop".into(),
                        name: "Self Loopback".into(),
                        transport: HostTransport::Tcp {
                            addr,
                            server_pubkey,
                            tailscale_auth: false,
                        },
                        ssh_target: None,
                        server_id: None,
                        enabled: true,
                        added_at: 0,
                    },
                    Some(pair_code),
                )
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
        // confirming `remote-b` reconnects automatically using persisted `client_noise.key` + pinned `server_pubkey`.
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
                .any(|s| s.id == "remote-b.ts.net" && s.is_connected());
            Ok(a_ok && b_ok)
        })?;
        let post_bounce_projs = world.client_call(
            "project.list",
            json!({ "host_id": "remote-b.ts.net" }),
            None,
        )?;
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
            "remote-b.ts.net",
            "device.revoke",
            json!({ "device_id": paired_dev_id }),
        )?;
        ensure!(
            revoke_res["revoked"].as_bool() == Some(true),
            "Expected device.revoke to return revoked=true, got {revoke_res}"
        );

        // Wait for `RevocationBus` to kick the live Noise `yamux/1` stream and for `MultiHostClient`
        // to observe the rejected reconnect and transition `remote-b` to `HostConnectionState::AuthFailed`.
        world.wait_until(10_000, |w| {
            let summaries = w.client_host_summaries();
            Ok(summaries.iter().any(|s| {
                s.id == "remote-b.ts.net"
                    && matches!(s.connection, HostConnectionState::AuthFailed { .. })
            }))
        })?;
        world.hit("auth.revoke_kicks_live_stream");

        oracle.check_auth_failed_zero_dials(world)?;
        let dials_at_auth_failed = world.client_dial_count("ui-client", "remote-b.ts.net");
        let reconnect_count_at_auth_failed = world
            .client_host_summaries()
            .into_iter()
            .find(|s| s.id == "remote-b.ts.net")
            .map(|s| s.reconnect_count)
            .expect("remote-b summary present");

        // Advance +16s virtual time and exercise background wakes/foreground/polls to verify
        // `MultiHostClient` halts retry loops and does NOT keep hammering reconnects!
        world.advance(Duration::from_secs(8))?;
        world.client_run_on("ui-client", 10_000, |client| async move {
            client.wake_host("remote-b.ts.net");
            client.wake_all();
            client.on_foreground();
            client.poll_remotes_once_async().await;
            Ok(())
        })?;
        world.advance(Duration::from_secs(8))?;

        let summary_b_after_wait = world
            .client_host_summaries()
            .into_iter()
            .find(|s| s.id == "remote-b.ts.net")
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
            "remote-b.ts.net",
            dials_at_auth_failed,
            reconnect_count_at_auth_failed,
        )?;

        // RPCs targeted at the revoked host must fail cleanly with an auth error (and NEVER enqueue offline outbox items).
        let list_err = world
            .client_call(
                "project.list",
                json!({ "host_id": "remote-b.ts.net" }),
                None,
            )
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
                    "host_id": "remote-b.ts.net",
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
            "remote-b.ts.net",
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
        // (c) Unauthorized client keypair, legacy plaintext rejection (-32002),
        //     and ciphertext bit-flip tampering fault!
        // -------------------------------------------------------------------------
        let unauth_kp = world.ephemeral_keypair("unauth-client")?;
        let unauth_err = world
            .connect_raw_transport(
                &addr_b,
                unauth_kp,
                Some(remote_b_pk.clone()),
                TcpAuthMode::Key,
                "Unauthorized Client",
            )
            .err()
            .context("unauthorized client keypair must be rejected")?;
        ensure!(
            format!("{unauth_err:#}").contains("auth_failed"),
            "Expected auth_failed for unauthorized client keypair, got: {unauth_err:#}"
        );
        world.hit("noise.unauth_client_rejected");

        // Legacy plaintext JSON-RPC sent directly to Noise TCP listener -> JSON-RPC -32002 error + close.
        let addr_b_plain = addr_b.clone();
        let plain_reply = world.client_run_on("ui-client", 15_000, move |_| async move {
            let mut stream = turmoil::net::TcpStream::connect(addr_b_plain.as_str())
                .await
                .context("connect plaintext probe")?;
            stream
                .write_all(
                    b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"app.ping\",\"params\":{}}\n",
                )
                .await?;
            stream.flush().await?;
            let mut buf = Vec::new();
            let _ =
                tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf)).await;
            Ok(String::from_utf8_lossy(&buf).into_owned())
        })?;
        ensure!(
            plain_reply.contains("-32002") && plain_reply.to_ascii_lowercase().contains("noise"),
            "Expected JSON-RPC -32002 Noise upgrade error for legacy plaintext client, got: {plain_reply:?}"
        );
        world.hit("noise.plaintext_rejected");

        // Bit-flip tampering fault in Noise handshake/transport ciphertext via FaultyStream!
        world.stream_fault_policy().set_for_host(
            "remote-b.ts.net",
            StreamFaultConfig {
                corrupt_read_byte_at: Some(12),
                ..StreamFaultConfig::default()
            },
        );
        let tamper_kp = world.ephemeral_keypair("tamper-client")?;
        let tamper_err = world
            .connect_raw_transport(
                &addr_b,
                tamper_kp,
                Some(remote_b_pk.clone()),
                TcpAuthMode::Key,
                "Tamper Probe",
            )
            .err()
            .context("bit-flipped Noise handshake/ciphertext must fail AEAD")?;
        world
            .stream_fault_policy()
            .clear_for_host("remote-b.ts.net");
        ensure!(
            !format!("{tamper_err:#}").is_empty(),
            "Expected AEAD/handshake error on bit-flipped Noise stream"
        );
        world.hit("noise.tamper_rejected");

        // -------------------------------------------------------------------------
        // (d) Real `MultiHostClient` Tailscale WhoIs TOFU + key pinning +
        //     Noise-encrypted `env.save` (zero plaintext on wire!)
        // -------------------------------------------------------------------------
        let verifier = SimTailscaleVerifier::rejecting();
        world.set_tailscale_verifier("remote-b.ts.net", Some(verifier.clone()))?;

        // Remove the revoked host slot for `remote-b` and re-register using ONLY `tailscale_auth: true` (no `?pk=` -> TOFU!).
        world.client_run_on("ui-client", 15_000, |client| async move {
            client.remove_host("remote-b.ts.net")
        })?;

        // Unauthorized Tailscale peer MUST be rejected through real `MultiHostClient`.
        let addr_b_unauth = addr_b.clone();
        let ts_unauth_err = world.client_run_on("ui-client", 15_000, move |client| async move {
            match client
                .add_host_async(SavedHost {
                    id: "remote-b.ts.net".into(),
                    name: "Tailscale Remote B".into(),
                    transport: HostTransport::Tcp {
                        addr: addr_b_unauth,
                        server_pubkey: None,
                        tailscale_auth: true,
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
                .any(|s| s.id == "remote-b.ts.net"),
            "Rejected Tailscale host must not be persisted in host_summaries"
        );

        // Authorize the Tailscale peer identity and connect `remote-b` through `MultiHostClient` with TOFU (`server_pubkey: None`).
        let ts_node = format!("nSimNode-{seed}");
        let ts_dev = format!("Sim iPad {seed}");
        verifier.set_default_identity(Some(TailscalePeerIdentity {
            node_id: ts_node.clone(),
            device_name: ts_dev.clone(),
            login_name: "sim-owner@example.invalid".into(),
        }));
        verifier.set_enabled(true);

        let addr_b_ts = addr_b.clone();
        let ts_summary = world.client_run_on("ui-client", 30_000, move |client| async move {
            client
                .add_host_async(SavedHost {
                    id: "remote-b.ts.net".into(),
                    name: "Tailscale Remote B".into(),
                    transport: HostTransport::Tcp {
                        addr: addr_b_ts,
                        server_pubkey: None,
                        tailscale_auth: true,
                    },
                    ssh_target: None,
                    server_id: None,
                    enabled: true,
                    added_at: 0,
                })
                .await
        })?;
        ensure!(
            ts_summary.id == "remote-b.ts.net" && ts_summary.is_connected(),
            "Tailscale MultiHostClient host must reach Connected: {ts_summary:?}"
        );

        // Verify `hosts.json` preserved `tailscale_auth: true` AND pinned `remote-b`'s `server_pubkey` (TOFU)!
        let ts_hosts: HostsFile = serde_json::from_slice(&std::fs::read(&hosts_path)?)?;
        let saved_ts_b = ts_hosts
            .hosts
            .iter()
            .find(|h| h.id == "remote-b.ts.net")
            .context("Tailscale remote-b missing from hosts.json")?;
        match &saved_ts_b.transport {
            HostTransport::Tcp {
                server_pubkey,
                tailscale_auth,
                ..
            } => ensure!(
                *tailscale_auth && server_pubkey.as_deref() == Some(remote_b_pk.as_str()),
                "Tailscale TOFU must persist tailscale_auth=true and pinned server_pubkey={remote_b_pk}, got pk={server_pubkey:?} ts={tailscale_auth}"
            ),
            other => anyhow::bail!("Expected Tcp transport for Tailscale host, got {other:?}"),
        }
        world.hit("noise.tailscale_tofu_pinned");

        // Verify Tailscale peer appears in `device.list` routed via `MultiHostClient` with `auth_kind == "tailscale"`.
        let ts_devs =
            world.client_call("device.list", json!({ "host_id": "remote-b.ts.net" }), None)?;
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

        // Reset wire-tap and send sensitive `env.save` + `card.create` over Noise_XX -> verify zero plaintext on wire!
        wire_tap.lock().unwrap().clear();
        let secret_plain = format!("sk-noise-e2ee-secret-{seed}");
        oracle.track_secret(secret_plain.clone());
        let save_res = world.client_call(
            "env.save",
            json!({
                "host_id": "remote-b.ts.net",
                "scope": "project",
                "target_id": raw_pid_b,
                "entries": {
                    "TS_SEALED_KEY": { "kind": "secret", "value": secret_plain },
                    "TS_PLAIN_KEY": { "kind": "plain", "value": "noise-e2ee-ok" },
                },
            }),
            Some(&pid_b),
        )?;
        ensure!(
            save_res["env"]["TS_SEALED_KEY"]["kind"].as_str() == Some("secret")
                && save_res["env"]["TS_SEALED_KEY"]["configured"].as_bool() == Some(true),
            "Expected Noise-encrypted secret to save cleanly over Tailscale: {save_res}"
        );
        let secret_card_title = format!("Confidential Noise Card {seed}");
        let secret_card = world.client_call(
            "card.create",
            json!({
                "project_id": pid_b,
                "title": secret_card_title,
                "body": "must never appear plaintext on wire",
            }),
            Some(&pid_b),
        )?;
        oracle.record_committed_card(
            "remote-b.ts.net",
            secret_card["id"].as_str().unwrap(),
            &pid_b,
            secret_card["revision"].as_u64().unwrap_or(1),
        );

        // Inspect raw TCP wire bytes: MUST have captured encrypted traffic and ZERO plaintext!
        {
            let tapped = wire_tap.lock().unwrap();
            ensure!(
                !tapped.is_empty(),
                "Wire tap captured 0 bytes during env.save + card.create on remote-b"
            );
            for forbidden in [
                secret_plain.as_bytes(),
                b"TS_SEALED_KEY",
                b"noise-e2ee-ok",
                b"env.save",
                secret_card_title.as_bytes(),
            ] {
                ensure!(
                    !contains_bytes(&tapped, forbidden),
                    "CRITICAL: raw TCP wire bytes leaked plaintext {:?}",
                    String::from_utf8_lossy(forbidden)
                );
            }
        }
        world
            .stream_fault_policy()
            .set_wire_tap("remote-b.ts.net", None);
        world.hit("noise.wire_confidentiality");

        // Verify the server persisted the exact plaintext into its at-rest vault while snapshots mask it.
        let srv_b_home = world.servers["remote-b.ts.net"].home.clone();
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
            "Server vault did not store Noise-delivered secret verbatim: expected {secret_plain:?}, got {stored_plain:?}"
        );

        let srv_snap_json =
            serde_json::to_string(&world.server("remote-b.ts.net").unwrap().snapshot()?)?;
        let merged_snap_json = serde_json::to_string(&world.client_merged_snapshot()?)?;
        ensure!(
            !srv_snap_json.contains(&secret_plain) && !merged_snap_json.contains(&secret_plain),
            "Snapshot leaked plaintext secret"
        );
        let preview = world.client_call(
            "env.preview",
            json!({ "host_id": "remote-b.ts.net", "project_id": raw_pid_b }),
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
        oracle.check_step_invariants(world)?;

        // -------------------------------------------------------------------------
        // (e) Server `noise.key` rotation -> `HostConnectionState::KeyMismatch`
        //     + 0 background dials + clean re-pair recovery!
        // -------------------------------------------------------------------------
        world.crash_server("remote-b.ts.net")?;
        let noise_key_path = srv_b_home.join("noise.key");
        ensure!(
            noise_key_path.exists(),
            "remote-b must persist noise.key on disk"
        );
        std::fs::remove_file(&noise_key_path)?;
        world.bounce_server("remote-b.ts.net")?;
        let rotated_remote_b_pk = world.servers["remote-b.ts.net"]
            .server_pubkey()
            .expect("rotated remote-b server_pubkey");
        ensure!(
            rotated_remote_b_pk != remote_b_pk,
            "Deleting noise.key + bouncing server must generate a fresh server keypair"
        );
        let rotated_remote_b_fp = pubkey_fingerprint(&decode_pubkey(&rotated_remote_b_pk)?);

        // Client's pinned `server_pubkey` no longer matches -> MUST transition to `HostConnectionState::KeyMismatch`!
        world.client_reconnect(Some("remote-b.ts.net"))?;
        world.wait_until(15_000, |w| {
            let summaries = w.client_host_summaries();
            Ok(summaries.iter().any(|s| {
                s.id == "remote-b.ts.net"
                    && matches!(
                        &s.connection,
                        HostConnectionState::KeyMismatch {
                            expected_fingerprint,
                            actual_fingerprint,
                            ..
                        } if expected_fingerprint == &remote_b_fp
                            && actual_fingerprint == &rotated_remote_b_fp
                    )
            }))
        })?;

        oracle.check_auth_failed_zero_dials(world)?;
        let dials_at_mismatch = world.client_dial_count("ui-client", "remote-b.ts.net");
        let reconn_at_mismatch = world
            .client_host_summaries()
            .into_iter()
            .find(|s| s.id == "remote-b.ts.net")
            .map(|s| s.reconnect_count)
            .expect("remote-b summary present");

        // Advance +16s virtual time with wakes/polls: `KeyMismatch` MUST stay halted with 0 background dials!
        world.advance(Duration::from_secs(8))?;
        world.client_run_on("ui-client", 10_000, |client| async move {
            client.wake_host("remote-b.ts.net");
            client.wake_all();
            client.on_foreground();
            client.poll_remotes_once_async().await;
            Ok(())
        })?;
        world.advance(Duration::from_secs(8))?;
        oracle.check_auth_failed_zero_dials(world)?;
        oracle.assert_auth_failed_zero_dials_for(
            world,
            "ui-client",
            "remote-b.ts.net",
            dials_at_mismatch,
            reconn_at_mismatch,
        )?;
        world.hit("noise.key_rotation_mismatch");

        // Re-pair `remote-b` cleanly with the rotated server pubkey using a fresh single-use invite!
        world.client_run_on("ui-client", 15_000, |client| async move {
            client.remove_host("remote-b.ts.net")
        })?;
        let repair_invite = world.server_call(
            "remote-b.ts.net",
            "device.invite.create",
            json!({ "ttl_secs": 300 }),
        )?;
        let repair_code = repair_invite["code"].as_str().unwrap().to_owned();
        let addr_b_repair = addr_b.clone();
        let rotated_pk_c = rotated_remote_b_pk.clone();
        let repaired_sum = world.client_run_on("ui-client", 20_000, move |client| async move {
            client
                .add_host_with_pairing_code_async(
                    SavedHost {
                        id: "remote-b.ts.net".into(),
                        name: "Rotated Remote B".into(),
                        transport: HostTransport::Tcp {
                            addr: addr_b_repair,
                            server_pubkey: Some(rotated_pk_c),
                            tailscale_auth: false,
                        },
                        ssh_target: None,
                        server_id: None,
                        enabled: true,
                        added_at: 0,
                    },
                    Some(repair_code),
                )
                .await
        })?;
        ensure!(
            repaired_sum.is_connected()
                && repaired_sum.server_pubkey_fingerprint.as_deref()
                    == Some(rotated_remote_b_fp.as_str()),
            "Re-paired rotated remote-b must connect with updated key fingerprint: {repaired_sum:?}"
        );

        // -------------------------------------------------------------------------
        // (f) `ApiMismatch` via `desktop_api_override` + clean recovery
        // -------------------------------------------------------------------------
        let old_api = canban_app_server::API_VERSION.saturating_sub(2);
        world.set_desktop_api_override("remote-a", Some(old_api))?;

        // Direct `SingleHostSession::connect_checked_async_with_clock` must return typed `ApiMismatchError`.
        let addr_check = addr_a.clone();
        let remote_a_pk_check = remote_a_pk.clone();
        let clock_check = world.servers["remote-a"].clock.clone();
        let probe_connector = TurmoilConnector::new(
            Arc::clone(&client_kp),
            world.noise_rng("api-mismatch-probe"),
        );
        let mismatch_check = world.client_run_on("ui-client", 15_000, move |_| async move {
            let transport = HostTransport::Tcp {
                addr: addr_check,
                server_pubkey: Some(remote_a_pk_check),
                tailscale_auth: false,
            };
            let res = SingleHostSession::connect_checked_async_with_clock(
                &probe_connector,
                "remote-a",
                &transport,
                "ApiMismatch Probe",
                None,
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
        // while `remote-b` remains Connected!
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
                .any(|s| s.id == "remote-b.ts.net" && s.is_connected()),
            "remote-b must remain Connected during remote-a ApiMismatch"
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
                "title": format!("Post-Rotation Card B {seed}"),
                "body": "noise-active",
            }),
            Some(&pid_b),
        )?;
        oracle.record_committed_card(
            "remote-b.ts.net",
            recovered_card_b["id"].as_str().unwrap(),
            &pid_b,
            recovered_card_b["revision"].as_u64().unwrap_or(1),
        );

        // -------------------------------------------------------------------------
        // (g) Client Push-Topic Separation, Legacy No-`app.subscribe` Polling
        //     Fallback, `set_host_enabled` TCP Drop, Side-Channel Forwarding,
        //     and `host.probe`/`discover_canban`!
        // -------------------------------------------------------------------------
        let srv_a = world
            .server("remote-a")
            .context("remote-a server missing")?;
        let srv_local = world
            .server("local-server")
            .context("local-server missing")?;

        // g1: `runtime.state`-only push skips `app.state`; same-rev `application` push is NOT suppressed!
        world.advance(Duration::from_millis(250))?;
        let app_before_rt = srv_a.app_state_call_count();
        let rt_before_rt = srv_a.runtime_state_call_count();
        srv_a.emit_state_push_for_sim("terminals", true);
        world.wait_until(8_000, |_| {
            Ok(srv_a.runtime_state_call_count() > rt_before_rt)
        })?;
        world.advance(Duration::from_millis(300))?;
        ensure!(
            srv_a.app_state_call_count() == app_before_rt,
            "runtime-only `terminals` push unexpectedly called app.state ({} -> {})",
            app_before_rt,
            srv_a.app_state_call_count()
        );
        srv_a.emit_state_push_for_sim("application", false);
        world.wait_until(8_000, |_| Ok(srv_a.app_state_call_count() > app_before_rt))?;

        // g2: Legacy daemon lacking `app.subscribe` falls back cleanly to polling!
        let rej_before = srv_a.subscribe_rejection_count();
        srv_a.set_disable_app_subscribe(true);
        world.client_reconnect(Some("remote-a"))?;
        world.wait_until(15_000, |w| {
            Ok(srv_a.subscribe_rejection_count() > rej_before
                && w.client_host_summaries()
                    .iter()
                    .any(|s| s.id == "remote-a" && s.is_connected()))
        })?;
        let raw_pid_a = canban_types::split_qualified_host_id(&pid_a)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| pid_a.clone());
        let polled_title = format!("Polled Fallback Card {seed}");
        let polled_card = world.server_call(
            "remote-a",
            "card.create",
            json!({ "project_id": raw_pid_a, "title": polled_title }),
        )?;
        let polled_raw_id = polled_card["id"].as_str().unwrap().to_owned();
        oracle.record_committed_card(
            "remote-a",
            &polled_raw_id,
            &pid_a,
            polled_card["revision"].as_u64().unwrap_or(1),
        );
        world.wait_until(15_000, |w| {
            let snap = w.client_merged_snapshot()?;
            Ok(snap.cards.iter().any(|c| c.title == polled_title))
        })?;
        srv_a.set_disable_app_subscribe(false);
        world.client_reconnect(Some("remote-a"))?;
        world.wait_until(15_000, |w| {
            Ok(w.client_host_summaries()
                .iter()
                .any(|s| s.id == "remote-a" && s.is_connected()))
        })?;

        // g3: `set_host_enabled(false)` drops active TCP push connection / lease, re-enable reconnects!
        ensure!(
            srv_a.active_tcp_count() >= 1,
            "Expected active TCP connection on remote-a before disable"
        );
        world.client_run_on("ui-client", 10_000, |client| async move {
            client.set_host_enabled("remote-a", false)
        })?;
        world.wait_until(15_000, |w| {
            Ok(w.client_host_summaries()
                .iter()
                .any(|s| s.id == "remote-a" && !s.is_connected())
                && srv_a.active_tcp_count() == 0)
        })?;
        world.client_run_on("ui-client", 10_000, |client| async move {
            client.set_host_enabled("remote-a", true)
        })?;
        world.wait_until(15_000, |w| {
            Ok(w.client_host_summaries()
                .iter()
                .any(|s| s.id == "remote-a" && s.is_connected())
                && srv_a.active_tcp_count() >= 1)
        })?;

        // g4: Side-channel `UtilitiesChanged` (local) & `UsageChanged` (remote) push forwarding!
        world.advance(Duration::from_millis(300))?;
        let _ = world.drain_client_events();
        srv_local.emit_utilities();
        srv_a.emit_usage_for_sim(json!({ "provider": "codex", "percent": 42, "seed": seed }));
        let mut saw_utilities = false;
        let mut saw_usage = false;
        world.wait_until(10_000, |w| {
            for ev in w.drain_client_events() {
                match ev {
                    ClientEvent::UtilitiesChanged { host_id, .. } if host_id == "local" => {
                        saw_utilities = true;
                    }
                    ClientEvent::UsageChanged { host_id, state }
                        if host_id == "remote-a" && state["percent"] == 42 =>
                    {
                        saw_usage = true;
                    }
                    _ => {}
                }
            }
            if !saw_utilities {
                srv_local.emit_utilities();
            }
            if !saw_usage {
                srv_a.emit_usage_for_sim(
                    json!({ "provider": "codex", "percent": 42, "seed": seed }),
                );
            }
            Ok(saw_utilities && saw_usage)
        })?;

        // g5: `host.probe` and `host.discover_canban` with deterministic TCP probe override!
        let probe_res = world.client_call("host.probe", json!({ "host_id": "remote-a" }), None)?;
        ensure!(
            probe_res["server_id"].as_str() == Some(srv_a.server_id())
                && probe_res["hostname"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty()),
            "Unexpected host.probe response: {probe_res}"
        );
        let srv_a_hosts_path = world.servers["remote-a"].home.join("config/hosts.json");
        let saved_for_disc = HostsFile {
            hosts: vec![
                SavedHost {
                    id: "linked-b".into(),
                    name: "Linked Remote B".into(),
                    transport: HostTransport::Tcp {
                        addr: addr_b.clone(),
                        server_pubkey: Some(rotated_remote_b_pk.clone()),
                        tailscale_auth: false,
                    },
                    ssh_target: None,
                    server_id: Some(remote_b_server_id.clone()),
                    enabled: true,
                    added_at: 1,
                },
                SavedHost {
                    id: "dead-peer".into(),
                    name: "Unreachable Peer".into(),
                    transport: HostTransport::Tcp {
                        addr: "192.0.2.1:9999".into(),
                        server_pubkey: Some(remote_a_pk.clone()),
                        tailscale_auth: false,
                    },
                    ssh_target: None,
                    server_id: Some("dead0001".into()),
                    enabled: true,
                    added_at: 2,
                },
            ],
        };
        std::fs::create_dir_all(srv_a_hosts_path.parent().unwrap())?;
        std::fs::write(
            &srv_a_hosts_path,
            serde_json::to_vec_pretty(&saved_for_disc)?,
        )?;
        let addr_b_expected = canban_types::normalize_daemon_addr(&addr_b);
        srv_a.set_tcp_probe_override(Some(Arc::new(move |addr| {
            if addr == addr_b_expected {
                Ok(4)
            } else {
                anyhow::bail!("simulated unreachable host {addr}")
            }
        })));
        let disc_res = world.client_call(
            "host.discover_canban",
            json!({ "host_id": "remote-a" }),
            None,
        )?;
        srv_a.set_tcp_probe_override(None);
        let disc_hosts = disc_res["hosts"]
            .as_array()
            .context("host.discover_canban missing hosts array")?;
        ensure!(
            disc_hosts.len() == 1
                && disc_hosts[0]["addr"].as_str()
                    == Some(canban_types::normalize_daemon_addr(&addr_b).as_str())
                && disc_hosts[0]["linked_on_host"] == true
                && disc_hosts[0]["rtt_ms"] == 4,
            "Unexpected host.discover_canban result: {disc_res}"
        );
        world.hit("transport_auth.polling_fallback_and_discovery");

        world.heal_and_quiesce(15_000)?;
        oracle.check_quiescent_invariants(world)?;

        for host in ["remote-a", "remote-b.ts.net"] {
            ensure!(
                world.servers[host].exec.blocked_commands().is_empty(),
                "transport_auth leaked unexpected OS commands on {host}: {:?}",
                world.servers[host].exec.blocked_commands()
            );
        }
        Ok(finalize_digest(world))
    })
}
