//! Scenario #5 — Environment variable & secret synchronization:
//! Global env outbox coalescing, tombstones, LWW convergence under host clock skew,
//! zero plaintext secret leaks in `.outbox.json`, and strict Agent/Project host isolation.
use crate::{
    oracle::InvariantOracle,
    world::{SimWorld, SimWorldConfig},
};
use anyhow::{Result, ensure};
use canban_types::EnvEntryMeta;
use serde_json::json;
use std::time::Duration;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "env_sync");
    // Give remote-b a clock skew so LWW relies on authoritative timestamps cleanly.
    cfg.host_clock_skew_ms.insert("remote-b".into(), -5_000);
    let mut world = SimWorld::new(cfg)?;
    let mut oracle = InvariantOracle::default();

    let secret_v1 = format!("sk-sim-secret-v1-{seed}");
    let secret_v2 = format!("sk-sim-secret-v2-{seed}");
    let proj_secret = format!("sk-proj-isolated-{seed}");
    oracle.track_secret(secret_v1.clone());
    oracle.track_secret(secret_v2.clone());
    oracle.track_secret(proj_secret.clone());

    // Create isolated projects on local and remote-a.
    let dir_local = world.project_dir("env-local")?;
    let dir_rem = world.project_dir("env-rem")?;
    let p_local = world.client_call(
        "project.create",
        json!({ "host_id": "local", "name": "Env Local", "directory": dir_local }),
        None,
    )?;
    let p_rem = world.client_call(
        "project.create",
        json!({ "host_id": "remote-a", "name": "Env Remote", "directory": dir_rem }),
        None,
    )?;
    let pid_local = p_local["id"].as_str().unwrap().to_owned();
    let pid_rem = p_rem["id"].as_str().unwrap().to_owned();
    world.advance(Duration::from_millis(150))?;

    // 1. Save initial Global env (plain + secret + key to be tombstoned later) on local.
    world.client_call(
        "env.save",
        json!({
            "host_id": "local",
            "scope": "global",
            "target_id": "",
            "entries": {
                "SHARED_PLAIN": { "kind": "plain", "value": "v1" },
                "SHARED_TEMP": { "kind": "plain", "value": "ephemeral" },
                "SHARED_SECRET": { "kind": "secret", "value": secret_v1 },
            }
        }),
        None,
    )?;
    world.client_sync_global_env_now();
    world.heal_and_quiesce(15_000)?;
    oracle.check_quiescent_invariants(&world)?;

    // 2. Partition remote-b, mutate Global env multiple times (updating secret + deleting SHARED_TEMP),
    //    and save host-isolated Project env on local & remote-a.
    world.partition("ui-client", "remote-b");
    world.advance(Duration::from_millis(100))?;

    // Advance virtual time so new LWW timestamps are strictly newer than v1.
    world.advance(Duration::from_secs(2))?;
    world.client_call(
        "env.save",
        json!({
            "host_id": "local",
            "scope": "global",
            "target_id": "",
            "entries": {
                "SHARED_PLAIN": { "kind": "plain", "value": "v2" },
                "SHARED_SECRET": { "kind": "secret", "value": secret_v2 },
            }
        }),
        None,
    )?;
    world.client_sync_global_env_now();
    world.advance(Duration::from_millis(250))?;

    // Save Project-scoped env on local (must NOT replicate to remote-a/remote-b)
    // and on remote-a (must NOT leak to local/remote-b).
    world.client_call(
        "env.save",
        json!({
            "host_id": "local",
            "scope": "project",
            "target_id": pid_local,
            "entries": {
                "LOCAL_ONLY_KEY": { "kind": "plain", "value": "local-val" },
                "LOCAL_ONLY_SECRET": { "kind": "secret", "value": proj_secret },
            }
        }),
        Some(&pid_local),
    )?;
    world.client_call(
        "env.save",
        json!({
            "host_id": "remote-a",
            "scope": "project",
            "target_id": pid_rem,
            "entries": {
                "REMOTE_A_ONLY": { "kind": "plain", "value": "rem-a-val" },
            }
        }),
        Some(&pid_rem),
    )?;

    // Mid-partition invariant check: `.outbox.json` MUST contain ZERO plaintext secrets!
    oracle.check_step_invariants(&world)?;

    // 3. Heal remote-b and quiesce: all remote hosts must converge to v2 + tombstone.
    world.heal_and_quiesce(20_000)?;
    oracle.check_quiescent_invariants(&world)?;

    for host in ["remote-a", "remote-b"] {
        let srv = world.server(host).unwrap();
        let snap = srv.snapshot()?;
        let global_meta = &snap.global_env;
        ensure!(
            global_meta.get("SHARED_PLAIN") == Some(&EnvEntryMeta::Plain { value: "v2".into() }),
            "[{host}] SHARED_PLAIN did not converge to v2: {:?}",
            global_meta
        );
        ensure!(
            !global_meta.contains_key("SHARED_TEMP"),
            "[{host}] SHARED_TEMP tombstone did not replicate: {:?}",
            global_meta
        );
        ensure!(
            matches!(
                global_meta.get("SHARED_SECRET"),
                Some(EnvEntryMeta::Secret {
                    configured: true,
                    ..
                })
            ),
            "[{host}] SHARED_SECRET not configured after AEAD sync: {:?}",
            global_meta
        );
        // Verify host isolation: LOCAL_ONLY_KEY and LOCAL_ONLY_SECRET must NOT exist on remotes.
        for (scope_key, entries) in &snap.scoped_env {
            ensure!(
                !entries.contains_key("LOCAL_ONLY_KEY")
                    && !entries.contains_key("LOCAL_ONLY_SECRET"),
                "[{host}] local project env leaked into scope {scope_key}: {:?}",
                entries
            );
            if host == "remote-b" {
                ensure!(
                    !entries.contains_key("REMOTE_A_ONLY"),
                    "[remote-b] remote-a project env leaked into scope {scope_key}"
                );
            }
        }
    }

    // Verify local did not receive REMOTE_A_ONLY.
    let local_snap = world.server("local-server").unwrap().snapshot()?;
    for (scope_key, entries) in &local_snap.scoped_env {
        ensure!(
            !entries.contains_key("REMOTE_A_ONLY"),
            "[local] remote-a project env leaked into scope {scope_key}"
        );
    }

    let status = world.client_global_env_status();
    ensure!(
        status.synced_hosts.len() == 2
            && status.queued_hosts.is_empty()
            && status.syncing_hosts.is_empty()
            && status.unsynced_hosts.is_empty()
            && status.failed_hosts.is_empty(),
        "Expected all 2 remote hosts synced, got {:?}",
        status
    );

    Ok(world.trace.digest())
}
