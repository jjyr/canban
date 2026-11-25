//! Scenario #5 — Global Environment & Secrets synchronization under clock skew,
//! real competing LWW writes across skewed hosts, `Unset` project-scope overrides (`env.preview`),
//! network partitions, secret rotation, deletion tombstones, and zero plaintext secret leaks.
use crate::{
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use canban_types::{EnvEntryMeta, EnvPreviewRow};
use serde_json::json;
use std::time::Duration;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "env_sync");
    cfg.host_clock_skew_ms.insert("remote-a".into(), -15_000);
    cfg.host_clock_skew_ms.insert("remote-b".into(), 20_000);

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        let secret_v1 = format!("sk-sim-topsecret-v1-{seed}");
        let secret_v2 = format!("sk-sim-rotated-v2-{seed}");
        oracle.track_secret(secret_v1.clone());
        oracle.track_secret(secret_v2.clone());

        // 1. Initial global env + secret save on `local` -> converges to both remotes.
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "global",
                "target_id": "",
                "entries": {
                    "SHARED_PLAIN": { "kind": "plain", "value": "v1" },
                    "SHARED_LWW": { "kind": "plain", "value": "initial" },
                    "API_TOKEN": { "kind": "secret", "value": secret_v1 },
                }
            }),
            None,
        )?;
        world.client_sync_global_env_now()?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        for host in ["local-server", "remote-a", "remote-b"] {
            let snap = world.server(host).unwrap().snapshot()?;
            ensure!(
                snap.global_env.get("SHARED_PLAIN")
                    == Some(&EnvEntryMeta::Plain { value: "v1".into() }),
                "Host {host} missing SHARED_PLAIN=v1: {:?}",
                snap.global_env
            );
            ensure!(
                matches!(
                    snap.global_env.get("API_TOKEN"),
                    Some(EnvEntryMeta::Secret { .. })
                ),
                "Host {host} missing masked API_TOKEN secret: {:?}",
                snap.global_env
            );
            oracle.record_committed_env(
                host,
                "SHARED_PLAIN",
                EnvEntryMeta::Plain { value: "v1".into() },
            );
        }

        // 2. Real competing writes across skewed hosts (`remote-b` is +20s ahead of `local`):
        //    Partition `remote-b`, write `SHARED_LWW = "remote-b-newer"` directly on `remote-b`
        //    at `local_now + 20s`, and write `SHARED_LWW = "local-older"` on `local` at `local_now`.
        //    When reconnected, LWW (`rec.updated_at < existing_ts`) MUST protect `remote-b`'s newer
        //    write against `local`'s older timestamp!
        world.partition("ui-client", "remote-b");
        world.advance(Duration::from_millis(120))?;
        oracle.mark_secret_queued_offline();

        world.server_call(
            "remote-b",
            "env.save",
            json!({
                "scope": "global",
                "target_id": "",
                "entries": {
                    "SHARED_PLAIN": { "kind": "plain", "value": "v1" },
                    "SHARED_LWW": { "kind": "plain", "value": "remote-b-newer" },
                    "API_TOKEN": { "kind": "secret", "keep_existing": true },
                }
            }),
        )?;
        oracle.record_committed_env(
            "remote-b",
            "SHARED_LWW",
            EnvEntryMeta::Plain {
                value: "remote-b-newer".into(),
            },
        );
        for host in ["local-server", "remote-a", "remote-b"] {
            oracle.remove_committed_env(host, "SHARED_PLAIN");
        }
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "global",
                "target_id": "",
                "entries": {
                    "SHARED_PLAIN": { "kind": "plain", "value": "v2-rotated" },
                    "SHARED_LWW": { "kind": "plain", "value": "local-older" },
                    "API_TOKEN": { "kind": "secret", "value": secret_v2 },
                }
            }),
            None,
        )?;
        oracle.record_committed_env(
            "local-server",
            "SHARED_PLAIN",
            EnvEntryMeta::Plain {
                value: "v2-rotated".into(),
            },
        );
        world.client_sync_global_env_now()?;
        oracle.check_step_invariants(world)?;

        // Heal link and let global env sync push `local`'s older write to `remote-b`.
        world.heal_and_quiesce(20_000)?;
        let b_snap = world.server("remote-b").unwrap().snapshot()?;
        ensure!(
            b_snap.global_env.get("SHARED_PLAIN")
                == Some(&EnvEntryMeta::Plain {
                    value: "v2-rotated".into(),
                }),
            "remote-b did not receive SHARED_PLAIN=v2-rotated after heal: {:?}",
            b_snap.global_env
        );
        // Confirm LWW protected `remote-b`'s newer timestamp on `SHARED_LWW`!
        ensure!(
            b_snap.global_env.get("SHARED_LWW")
                == Some(&EnvEntryMeta::Plain {
                    value: "remote-b-newer".into(),
                }),
            "LWW violation: stale local push overwrote newer remote-b write! got {:?}",
            b_snap.global_env.get("SHARED_LWW")
        );
        for host in ["remote-a", "remote-b"] {
            oracle.record_committed_env(
                host,
                "SHARED_PLAIN",
                EnvEntryMeta::Plain {
                    value: "v2-rotated".into(),
                },
            );
        }
        world.hit("env_sync.lww_stale_rejected");

        // Now advance virtual clock past `remote-b`'s +20s skew so `local` issues a strictly newer
        // timestamp and ALL hosts converge identically!
        world.advance(Duration::from_secs(30))?;
        oracle.remove_committed_env("remote-b", "SHARED_LWW");
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "global",
                "target_id": "",
                "entries": {
                    "SHARED_PLAIN": { "kind": "plain", "value": "v2-rotated" },
                    "SHARED_LWW": { "kind": "plain", "value": "local-newest" },
                    "API_TOKEN": { "kind": "secret", "value": secret_v2 },
                }
            }),
            None,
        )?;
        world.client_sync_global_env_now()?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;
        for host in ["local-server", "remote-a", "remote-b"] {
            let snap = world.server(host).unwrap().snapshot()?;
            ensure!(
                snap.global_env.get("SHARED_LWW")
                    == Some(&EnvEntryMeta::Plain {
                        value: "local-newest".into(),
                    }),
                "Host {host} failed to converge to local-newest: {:?}",
                snap.global_env.get("SHARED_LWW")
            );
            oracle.record_committed_env(
                host,
                "SHARED_LWW",
                EnvEntryMeta::Plain {
                    value: "local-newest".into(),
                },
            );
        }
        world.hit("env_sync.lww_converged");

        // 3. Project-scope `Unset` override + `env.preview` provenance & secret masking check.
        let dir = world.project_dir("env-unset")?;
        let proj = world.client_call(
            "project.create",
            json!({ "host_id": "local", "name": "Env Override Proj", "directory": dir }),
            None,
        )?;
        let pid = proj["id"].as_str().unwrap().to_owned();
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "project",
                "target_id": pid,
                "entries": {
                    "SHARED_LWW": { "kind": "unset" },
                    "PROJECT_ONLY": { "kind": "plain", "value": "from-project" },
                }
            }),
            Some(&pid),
        )?;
        let preview = world.client_call(
            "env.preview",
            json!({ "project_id": pid, "profile_id": "developer" }),
            Some(&pid),
        )?;
        let rows: Vec<EnvPreviewRow> = serde_json::from_value(preview["rows"].clone())?;
        let unset_row = rows
            .iter()
            .find(|r| r.key == "SHARED_LWW")
            .expect("SHARED_LWW missing from env.preview");
        ensure!(
            unset_row.is_unset,
            "Project-scope Unset override must mark SHARED_LWW as is_unset"
        );
        let proj_row = rows
            .iter()
            .find(|r| r.key == "PROJECT_ONLY")
            .expect("PROJECT_ONLY missing from env.preview");
        ensure!(proj_row.source_layer == "Project");
        let sec_row = rows
            .iter()
            .find(|r| r.key == "API_TOKEN")
            .expect("API_TOKEN missing from env.preview");
        ensure!(
            sec_row.is_secret && sec_row.configured && !sec_row.display_value.contains(&secret_v2),
            "env.preview leaked secret plaintext!"
        );
        world.hit("env_sync.project_unset_override");

        // 4. Tombstone deletion across partition + `remote-b` crash/bounce.
        for host in ["local-server", "remote-a", "remote-b"] {
            oracle.remove_committed_env(host, "SHARED_PLAIN");
            oracle.remove_committed_env(host, "SHARED_LWW");
        }
        world.partition("ui-client", "remote-b");
        world.advance(Duration::from_millis(120))?;
        let _ = world.client_call(
            "env.save",
            json!({
                "scope": "global",
                "target_id": "",
                "entries": {
                    "SHARED_PLAIN": { "kind": "plain", "value": "v3-final" },
                }
            }),
            None,
        )?;
        oracle.record_committed_env(
            "local-server",
            "SHARED_PLAIN",
            EnvEntryMeta::Plain {
                value: "v3-final".into(),
            },
        );
        world.client_sync_global_env_now()?;
        world.crash_server("remote-b")?;
        world.advance(Duration::from_millis(200))?;
        world.bounce_server("remote-b")?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        for host in ["local-server", "remote-a", "remote-b"] {
            let snap = world.server(host).unwrap().snapshot()?;
            ensure!(
                !snap.global_env.contains_key("API_TOKEN"),
                "Host {host} failed to apply tombstone deletion for API_TOKEN"
            );
            ensure!(
                !snap.global_env.contains_key("SHARED_LWW"),
                "Host {host} failed to apply tombstone deletion for SHARED_LWW"
            );
            ensure!(
                snap.global_env.get("SHARED_PLAIN")
                    == Some(&EnvEntryMeta::Plain {
                        value: "v3-final".into(),
                    }),
                "Host {host} did not converge to SHARED_PLAIN=v3-final"
            );
            oracle.record_committed_env(
                host,
                "SHARED_PLAIN",
                EnvEntryMeta::Plain {
                    value: "v3-final".into(),
                },
            );
        }
        oracle.check_committed_durability(world)?;
        world.hit("env_sync.tombstone_bounce_converged");

        Ok(finalize_digest(world))
    })
}
