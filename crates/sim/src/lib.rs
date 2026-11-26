//! Full-scenario Deterministic Simulation Testing (DST) framework for Canban built on `turmoil`.
pub mod clock;
pub mod exec;
pub mod github;
pub mod herdr;
pub mod net;
pub mod oracle;
pub mod scenarios;
pub mod trace;
pub mod world;

pub use scenarios::{Scenario, run_scenario};

#[cfg(test)]
mod tests {
    use super::{
        oracle::{InvariantOracle, canonical_state_digest},
        scenarios,
        world::{SimWorld, SimWorldConfig},
    };
    use canban_exec::{CommandRunner, CommandSpec};
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn determinism_same_seed_produces_identical_digest() {
        let d1 = scenarios::chaos::run(777_001, 40).expect("first chaos run");
        let d2 = scenarios::chaos::run(777_001, 40).expect("second chaos run");
        assert_eq!(
            d1, d2,
            "Determinism violation: identical chaos seed produced different trace+canonical digests ({d1:#x} vs {d2:#x})"
        );
        let d_other = scenarios::chaos::run(777_002, 40).expect("different seed chaos run");
        assert_ne!(
            d1, d_other,
            "Distinct chaos seeds unexpectedly produced identical digests ({d1:#x})"
        );
    }

    #[test]
    fn chaos_shrink_smoke_test() {
        // Synthetic 30-op sequence failing iff op 7 and op 22 are both present in order.
        let synthetic_ops: Vec<usize> = (0..30).collect();
        let (minimized, attempts, err) = scenarios::chaos::shrink_ops(
            synthetic_ops,
            "synthetic invariant failure".into(),
            64,
            |cand| {
                let p7 = cand.iter().position(|&x| x == 7);
                let p22 = cand.iter().position(|&x| x == 22);
                match (p7, p22) {
                    (Some(a), Some(b)) if a < b => Some("synthetic invariant failure".into()),
                    _ => None,
                }
            },
        );
        assert_eq!(
            minimized,
            vec![7, 22],
            "shrink_ops failed to minimize sequence to exact 2-op failure core (attempts={attempts})"
        );
        assert_eq!(err, "synthetic invariant failure");

        // Running `chaos::shrink` against a passing seed returns `Ok(None)`.
        let passing = scenarios::chaos::shrink(777_001, 12).expect("shrink passing seed");
        assert!(
            passing.is_none(),
            "Passing seed should produce None ShrinkReport"
        );
    }

    #[test]
    fn sim_exec_loads_simulated_agent_profiles_and_blocks_os_spawns() {
        let mut cfg = SimWorldConfig::desktop_default(888_001, "unit_exec");
        cfg.remote_hosts = vec!["remote-a".into()];
        let mut world = SimWorld::new(cfg).expect("world init");

        // Verify `config/agents.json` was written (not dead `config.json`) and loaded by AppServer
        // with `/bin/echo` argv across every default profile!
        for host in ["local-server", "remote-a"] {
            let handle = &world.servers[host];
            assert!(
                handle.home.join("config/agents.json").exists(),
                "[{host}] expected config/agents.json to exist"
            );
            assert!(
                !handle.home.join("config.json").exists(),
                "[{host}] legacy dead config.json should not be created"
            );
            let srv = handle.server().expect("server running");
            let loaded_cfg = srv.config();
            assert!(!loaded_cfg.profiles.is_empty());
            for prof in &loaded_cfg.profiles {
                assert_eq!(
                    prof.command().first().map(String::as_str),
                    Some("/bin/echo"),
                    "[{host}] profile {} did not load simulated /bin/echo command: {:?}",
                    prof.id,
                    prof.command()
                );
            }
            assert_eq!(handle.exec.hostname(), host);
            assert!(
                handle.exec.blocked_commands().is_empty(),
                "[{host}] unexpected blocked commands during boot: {:?}",
                handle.exec.blocked_commands()
            );
            // Verify fail-closed behavior on unexpected OS commands (e.g. `gh`, `ssh`, `hc`).
            for unexpected in ["gh", "ssh", "python3", "hc"] {
                let err = handle
                    .exec
                    .run(&CommandSpec::new(unexpected).arg("--version"))
                    .expect_err("SimExec must fail closed on unexpected OS commands");
                assert!(
                    err.to_string().contains("blocked unexpected OS command"),
                    "unexpected error message: {err}"
                );
            }
            assert_eq!(handle.exec.blocked_commands().len(), 4);
            handle.exec.clear_recorded_commands();
        }

        // Creating a project calls `git` through SimExec cleanly with zero blocked OS spawns.
        let dir = world.project_dir("exec-proj").expect("dir");
        let _ = world
            .client_call(
                "project.create",
                json!({ "host_id": "remote-a", "name": "Exec Proj", "directory": dir }),
                None,
            )
            .expect("project.create");
        assert!(
            world.servers["remote-a"].exec.blocked_commands().is_empty(),
            "project.create leaked unexpected OS command"
        );
    }

    #[test]
    fn bounce_client_recovers_persisted_outbox_after_crash() {
        let mut cfg = SimWorldConfig::mobile_default(888_002, "unit_bounce_client");
        cfg.remote_hosts = vec!["remote-a".into()];
        let mut world = SimWorld::new(cfg).expect("world init");
        let mut oracle = InvariantOracle::default();

        let dir = world.project_dir("bounce-cli").expect("dir");
        let proj = world
            .client_call(
                "project.create",
                json!({ "host_id": "remote-a", "name": "Bounce Client Proj", "directory": dir }),
                None,
            )
            .expect("project.create");
        let pid = proj["id"].as_str().unwrap().to_owned();
        world.advance(Duration::from_millis(150)).expect("advance");

        // Partition remote-a and queue an offline card creation into `.outbox.json`.
        world.partition("ui-client", "remote-a");
        world.advance(Duration::from_millis(100)).expect("advance");
        let _ = world
            .client_call(
                "card.create",
                json!({ "project_id": pid, "title": "Survived Client Crash", "body": "outbox-durable" }),
                Some(&pid),
            )
            .expect("offline card.create");
        assert!(
            !world.client_sync_items().is_empty(),
            "expected queued offline outbox item before client crash"
        );

        // Abruptly crash and bounce `"ui-client"` on the SAME `client_data_dir`.
        world.crash_client().expect("crash_client");
        world.advance(Duration::from_millis(200)).expect("advance");
        world.bounce_client().expect("bounce_client");

        // The persisted outbox item must be recovered on reboot and drain cleanly once healed!
        assert!(
            !world.client_sync_items().is_empty(),
            "persisted outbox item disappeared across client crash/bounce!"
        );
        world.heal_and_quiesce(20_000).expect("heal_and_quiesce");
        oracle
            .check_quiescent_invariants(&world)
            .expect("quiescent invariants");

        let srv_snap = world.server("remote-a").unwrap().snapshot().unwrap();
        assert_eq!(srv_snap.cards.len(), 1);
        assert_eq!(srv_snap.cards[0].title, "Survived Client Crash");
    }

    #[test]
    fn oracle_sqlite_integrity_and_recursive_outbox_secret_scan() {
        let mut cfg = SimWorldConfig::mobile_default(888_003, "unit_oracle_secret");
        cfg.remote_hosts = vec!["remote-a".into()];
        let world = SimWorld::new(cfg).expect("world init");
        let mut oracle = InvariantOracle::default();

        // SQLite PRAGMA foreign_key_check + quick_check passes on healthy server DBs.
        InvariantOracle::check_sqlite_integrity(&world).expect("healthy sqlite integrity");
        assert!(
            world
                .coverage_counts()
                .get("oracle.sqlite_integrity_checked")
                .copied()
                .unwrap_or(0)
                > 0
        );

        // Inject a plaintext secret deep inside `cache/hosts/remote-a.outbox.json` and confirm
        // the recursive scanner catches it AND records `oracle.outbox_file_scanned`.
        let leaked_secret = "sk-injected-leak-test-888003";
        oracle.track_secret(leaked_secret);
        oracle.mark_secret_queued_offline();
        let outbox_dir = world.client_data_dir.join("cache").join("hosts");
        std::fs::create_dir_all(&outbox_dir).expect("mkdir cache/hosts");
        let outbox_path = outbox_dir.join("remote-a.outbox.json");
        std::fs::write(
            &outbox_path,
            format!(r#"{{"ops":[{{"secret":"{leaked_secret}"}}]}}"#),
        )
        .expect("write mock outbox");

        let err = oracle.check_step_invariants(&world).expect_err(
            "recursive secret scan must catch leaked secret in cache/hosts/*.outbox.json",
        );
        assert!(
            err.to_string().contains("SECURITY VIOLATION"),
            "unexpected error: {err:#}"
        );
        assert!(oracle.outbox_files_scanned > 0);
        assert!(
            world
                .coverage_counts()
                .get("oracle.outbox_file_scanned")
                .copied()
                .unwrap_or(0)
                > 0
        );

        // Clean up injected file and verify clean pass after removal.
        std::fs::write(&outbox_path, r#"{"ops":[]}"#).expect("clear mock outbox");
        oracle
            .check_step_invariants(&world)
            .expect("clean outbox passes secret scan");
    }

    #[test]
    fn canonical_state_digest_is_stable_across_identical_seed_runs() {
        fn run_once(seed: u64) -> u64 {
            let mut cfg = SimWorldConfig::desktop_default(seed, "unit_canonical_digest");
            cfg.remote_hosts = vec!["remote-a".into()];
            let mut world = SimWorld::new(cfg).expect("world init");
            let dir_l = world.project_dir("proj-local").expect("dir_l");
            let dir_r = world.project_dir("proj-remote").expect("dir_r");
            let pl = world
                .client_call(
                    "project.create",
                    json!({ "host_id": "local", "name": "Alpha", "directory": dir_l }),
                    None,
                )
                .expect("pl");
            let pr = world
                .client_call(
                    "project.create",
                    json!({ "host_id": "remote-a", "name": "Beta", "directory": dir_r }),
                    None,
                )
                .expect("pr");
            let pid_l = pl["id"].as_str().unwrap().to_owned();
            let pid_r = pr["id"].as_str().unwrap().to_owned();
            let _ = world
                .client_call(
                    "card.create",
                    json!({ "project_id": pid_l, "title": "Local Card", "body": "hello" }),
                    Some(&pid_l),
                )
                .expect("c1");
            let _ = world
                .client_call(
                    "card.create",
                    json!({ "project_id": pid_r, "title": "Remote Card", "body": "world" }),
                    Some(&pid_r),
                )
                .expect("c2");
            world.heal_and_quiesce(10_000).expect("quiesce");
            canonical_state_digest(&world)
        }

        let d1 = run_once(888_004);
        let d2 = run_once(888_004);
        assert_eq!(
            d1, d2,
            "canonical_state_digest must be UUID-independent and deterministic across identical runs"
        );
    }

    #[test]
    fn multi_client_concurrent_spawn_abrupt_crash_durability_and_natural_quiesce() {
        let mut cfg = SimWorldConfig::mobile_default(888_005, "unit_multi_client_durability");
        cfg.remote_hosts = vec!["remote-a".into()];
        cfg.extra_clients = vec!["ui-client-2".into()];
        let mut world = SimWorld::new(cfg).expect("world init");
        let mut oracle = InvariantOracle::default();

        // Register a scripted git repo fixture on remote-a and verify deterministic SimExec response.
        let dir = world.project_dir("mc-repo").expect("dir");
        world.servers["remote-a"]
            .exec
            .register_git_repo(&dir, crate::exec::SimGitRepo::default());
        let git_out = world.servers["remote-a"]
            .exec
            .run(&CommandSpec::new("git").args(["-C", &dir, "remote", "get-url", "origin"]))
            .expect("scripted git remote");
        assert_eq!(git_out.status_code, 0);
        assert_eq!(
            String::from_utf8_lossy(&git_out.stdout).trim(),
            "https://github.com/acme/canban-sim.git"
        );

        let proj = world
            .client_call(
                "project.create",
                json!({ "host_id": "remote-a", "name": "Multi Client", "directory": dir }),
                None,
            )
            .expect("project.create");
        let pid = proj["id"].as_str().unwrap().to_owned();
        world.quiesce_natural(15_000).expect("natural quiesce");

        // Issue concurrent non-blocking RPCs from primary and secondary clients simultaneously.
        let h1 = world
            .client_spawn_on(
                "ui-client",
                "card.create",
                json!({ "project_id": pid, "title": "From Client 1", "body": "c1" }),
                Some(&pid),
            )
            .expect("spawn c1");
        let h2 = world
            .client_spawn_on(
                "ui-client-2",
                "card.create",
                json!({ "project_id": pid, "title": "From Client 2", "body": "c2" }),
                Some(&pid),
            )
            .expect("spawn c2");
        let r1 = world.await_call(h1, 30_000).expect("await c1");
        let r2 = world.await_call(h2, 30_000).expect("await c2");

        let c1_id = r1["id"].as_str().unwrap().to_owned();
        let c2_id = r2["id"].as_str().unwrap().to_owned();
        oracle.record_committed_card("remote-a", &c1_id, &pid, r1["revision"].as_u64().unwrap());
        oracle.record_committed_card("remote-a", &c2_id, &pid, r2["revision"].as_u64().unwrap());

        let _ = world
            .client_call(
                "comment.add",
                json!({ "card_id": c1_id, "body": "durable-comment-888005" }),
                Some(&pid),
            )
            .expect("comment.add");
        oracle.record_committed_comment("remote-a", &c1_id, "durable-comment-888005");

        // Abruptly SIGKILL and bounce remote-a; committed writes and startup recovery must hold!
        world.crash_server("remote-a").expect("abrupt crash");
        world.advance(Duration::from_millis(200)).expect("advance");
        world.bounce_server("remote-a").expect("bounce");
        InvariantOracle::check_post_bounce_recovery(&world, "remote-a")
            .expect("post-bounce recovery");

        // Wait for unassisted reconnect via `quiesce_natural` (no forceful reconnect kicks!).
        world
            .quiesce_natural(30_000)
            .expect("unassisted reconnect quiesce");
        world.heal_and_quiesce(10_000).expect("final poll");
        oracle
            .check_quiescent_invariants(&world)
            .expect("quiescent invariants after abrupt crash + bounce");
    }

    #[test]
    fn secondary_client_oracle_catches_stuck_outbox_and_divergence() {
        let mut cfg = SimWorldConfig::mobile_default(888_006, "unit_secondary_oracle");
        cfg.remote_hosts = vec!["remote-a".into()];
        cfg.extra_clients = vec!["ui-client-2".into()];
        let mut world = SimWorld::new(cfg).expect("world init");
        let mut oracle = InvariantOracle::default();

        let dir = world.project_dir("sec-oracle").expect("dir");
        let proj = world
            .client_call(
                "project.create",
                json!({ "host_id": "remote-a", "name": "Sec Oracle Proj", "directory": dir }),
                None,
            )
            .expect("project.create");
        let pid = proj["id"].as_str().unwrap().to_owned();
        world.heal_and_quiesce(15_000).expect("initial quiesce");
        oracle
            .check_quiescent_invariants(&world)
            .expect("initial quiescent invariants");

        // Partition ONLY `ui-client-2` while primary `ui-client` remains completely healthy,
        // and queue an offline card on `ui-client-2`.
        world.partition("ui-client-2", "remote-a");
        world.advance(Duration::from_millis(120)).expect("advance");
        let _ = world
            .client_call_on(
                "ui-client-2",
                "card.create",
                json!({ "project_id": pid, "title": "Stuck On Client 2", "body": "offline" }),
                Some(&pid),
            )
            .expect("offline card.create on ui-client-2");

        // Primary client has zero outbox items, yet the oracle MUST fail because `ui-client-2`
        // has a queued outbox item and diverged view!
        assert!(world.client_sync_items().is_empty());
        let err = oracle
            .check_quiescent_invariants(&world)
            .expect_err("oracle must catch stuck outbox item on secondary client ui-client-2");
        assert!(
            err.to_string().contains("[ui-client-2]"),
            "expected error mentioning [ui-client-2], got: {err:#}"
        );

        // Once healed and quiesced, both clients converge and the oracle passes cleanly.
        world.heal_and_quiesce(20_000).expect("heal and quiesce");
        oracle
            .check_quiescent_invariants(&world)
            .expect("quiescent invariants after secondary client heals");
    }

    #[test]
    fn sim_world_emits_herdr_heartbeats_unless_suppressed() {
        let mut cfg = SimWorldConfig::mobile_default(888_007, "unit_herdr_heartbeats");
        cfg.remote_hosts = vec!["remote-a".into()];
        let mut world = SimWorld::new(cfg).expect("world init");

        let herdr = world.herdr("remote-a").expect("remote-a herdr");
        world
            .wait_until(5_000, |w| {
                Ok(w.herdr("remote-a").unwrap().subscriber_count() > 0)
            })
            .expect("initial herdr observer subscribed");

        let subs_before = herdr.subscribe_count();
        let hb_before = herdr.heartbeat_count();

        // 1. Default mode: advancing 60s virtual time emits periodic heartbeats every 15s and
        //    keeps the Herdr observer connected without hitting the 45s heartbeat timeout!
        world
            .advance(Duration::from_secs(60))
            .expect("advance 60s with heartbeats");
        assert!(
            herdr.heartbeat_count() >= hb_before + 3,
            "expected >=3 virtual-time Herdr heartbeats over 60s, got {}",
            herdr.heartbeat_count() - hb_before
        );
        assert_eq!(
            herdr.subscribe_count(),
            subs_before,
            "Herdr observer unexpectedly reconnected despite periodic heartbeats"
        );

        // 2. Suppressed mode: disabling automatic heartbeats causes the 45s timeout to trigger
        //    and the Herdr observer to disconnect & resubscribe cleanly.
        world.set_suppress_herdr_heartbeats(true);
        let hb_suppressed_start = herdr.heartbeat_count();
        let subs_suppressed_start = herdr.subscribe_count();
        world
            .advance(Duration::from_secs(60))
            .expect("advance 60s with heartbeats suppressed");
        assert_eq!(
            herdr.heartbeat_count(),
            hb_suppressed_start,
            "no heartbeats should be emitted while suppressed"
        );
        assert!(
            herdr.subscribe_count() > subs_suppressed_start,
            "expected Herdr observer to hit 45s heartbeat timeout and resubscribe when heartbeats are suppressed"
        );
    }
}
