//! Scenario #2 — Remote host operations under network drops, mid-flight partitions,
//! one-way splits (after-commit drops), server & client crashes/bounces (`.outbox.json` reload),
//! 3-way disjoint-field auto-merge, and all conflict resolution actions (`UseMine`, `Retry`, `UseEdited`).
use crate::{
    net::{StreamFaultConfig, StreamFaultPolicy},
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use canban_types::{SyncAction, SyncItemState};
use rand::Rng;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::mobile_default(seed, "remote_faults");
    cfg.remote_hosts = vec!["remote-a".into()];
    cfg.min_latency_ms = 5;
    cfg.max_latency_ms = 40;
    cfg.stream_fault_policy = Some(StreamFaultPolicy::new(StreamFaultConfig::default()));

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        let repo_dir = world.project_dir("fault-repo")?;
        let proj = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-a",
                "name": "Fault Project",
                "directory": repo_dir,
            }),
            None,
        )?;
        let project_id = proj["id"].as_str().unwrap().to_owned();
        let raw_pid = canban_types::split_qualified_host_id(&project_id)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| project_id.clone());
        world.advance(Duration::from_millis(150))?;

        // 1. Offline FIFO ordering + zero duplicate cards across bidirectional partition.
        world.partition("ui-client", "remote-a");
        world.advance(Duration::from_millis(100))?;

        let created = world.client_call(
            "card.create",
            json!({
                "project_id": project_id,
                "title": "FIFO v0",
                "body": "initial",
            }),
            Some(&project_id),
        )?;
        let card_id = created["id"].as_str().unwrap().to_owned();

        let expected_steps = 5usize;
        for idx in 1..=expected_steps {
            let rev = world
                .client_merged_snapshot()?
                .cards
                .iter()
                .find(|c| c.id == card_id)
                .map(|c| c.revision)
                .unwrap_or(1);
            let _ = world.client_call(
                "card.update",
                json!({
                    "card_id": card_id,
                    "revision": rev,
                    "title": format!("FIFO v{idx}"),
                    "body": format!("body-{idx}"),
                }),
                Some(&project_id),
            )?;
            let _ = world.client_call(
                "comment.add",
                json!({
                    "card_id": card_id,
                    "body": format!("comment-{idx}"),
                }),
                Some(&project_id),
            )?;
        }

        oracle.check_step_invariants(world)?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        let srv_snap = world.server("remote-a").unwrap().snapshot()?;
        ensure!(srv_snap.cards.len() == 1);
        ensure!(srv_snap.cards[0].title == format!("FIFO v{expected_steps}"));
        let timeline = world.client_call(
            "timeline.list",
            json!({ "card_id": card_id, "tail": true, "limit": 200, "view": "full" }),
            Some(&project_id),
        )?;
        let comments: Vec<&str> = timeline["events"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["kind"] == "comment.created")
            .filter_map(|e| {
                e["body"]
                    .as_str()
                    .or_else(|| e["summary"].as_str())
                    .or_else(|| e["data"]["body"].as_str())
            })
            .collect();
        ensure!(comments.len() == expected_steps);
        for (idx, body) in comments.iter().enumerate() {
            ensure!(*body == format!("comment-{}", idx + 1));
        }
        oracle.record_committed_card(
            "remote-a",
            &srv_snap.cards[0].id,
            &srv_snap.cards[0].project_id,
            srv_snap.cards[0].revision,
        );
        for idx in 1..=expected_steps {
            oracle.record_committed_comment(
                "remote-a",
                &srv_snap.cards[0].id,
                format!("comment-{idx}"),
            );
        }

        // 2. True mid-flight fault injection via `client_spawn` + TCP RST / one-way drop +
        //    server crash/bounce rounds -> zero duplicate cards!
        let rounds = 4usize;
        for r in 0..rounds {
            world.set_link_latency("ui-client", "remote-a", Duration::from_millis(80));
            if r % 2 == 0 {
                // Mid-flight stream reset after initial request bytes are written!
                world.stream_fault_policy().set_for_host(
                    "remote-a",
                    StreamFaultConfig {
                        rst_after_write_bytes: Some(32),
                        ..StreamFaultConfig::default()
                    },
                );
            }
            let handle = world.client_spawn(
                "card.create",
                json!({
                    "project_id": project_id,
                    "title": format!("Resilient card {r}"),
                    "body": format!("seed-{seed}-round-{r}"),
                }),
                Some(&project_id),
            )?;
            // Advance into the middle of the RPC flight before cutting the return link!
            world.advance(Duration::from_millis(40))?;
            let oneway = world.rng.random_bool(0.5);
            if oneway {
                world.partition_oneway("remote-a", "ui-client");
            } else {
                world.partition("ui-client", "remote-a");
            }
            let _ = world.await_call(handle, 120_000)?;
            world.stream_fault_policy().clear_all();
            world.hit("remote_faults.mid_flight_drop");
            world.advance(Duration::from_millis(120))?;
            if r % 2 == 1 {
                world.crash_server("remote-a")?;
                world.advance(Duration::from_millis(200))?;
                world.bounce_server("remote-a")?;
            }
            world.heal_and_quiesce(20_000)?;
            oracle.check_quiescent_invariants(world)?;
        }

        let srv_snap = world.server("remote-a").unwrap().snapshot()?;
        ensure!(
            srv_snap.cards.len() == 1 + rounds,
            "Zero-duplicate card invariant violated: expected {} cards, found {}",
            1 + rounds,
            srv_snap.cards.len()
        );
        for c in &srv_snap.cards {
            oracle.record_committed_card("remote-a", &c.id, &c.project_id, c.revision);
        }

        // 3. 3-way disjoint-field auto-merge:
        //    Partition client; client edits ONLY `body` while server edits ONLY `title` ->
        //    heal & quiesce -> ZERO conflicts and BOTH edits preserved!
        let srv_card_id = srv_snap.cards[0].id.clone();
        world.partition("ui-client", "remote-a");
        world.advance(Duration::from_millis(100))?;

        let base_rev = world
            .client_merged_snapshot()?
            .cards
            .iter()
            .find(|c| c.id == card_id)
            .unwrap()
            .revision;
        let _ = world.client_call(
            "card.update",
            json!({
                "card_id": card_id,
                "revision": base_rev,
                "body": "disjoint-client-body",
            }),
            Some(&project_id),
        )?;
        world.server_call(
            "remote-a",
            "card.update",
            json!({
                "card_id": srv_card_id,
                "revision": base_rev,
                "title": "Disjoint Server Title",
            }),
        )?;

        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;
        let merged_card = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .into_iter()
            .find(|c| c.id == srv_card_id)
            .unwrap();
        ensure!(
            merged_card.title == "Disjoint Server Title"
                && merged_card.body == "disjoint-client-body",
            "3-way disjoint auto-merge failed: title={:?} body={:?}",
            merged_card.title,
            merged_card.body
        );
        world.hit("remote_faults.three_way_auto_merge");

        // 4. Same-field conflict resolution: exercise `UseMine`, `Retry`, and `UseEdited`!
        world.partition("ui-client", "remote-a");
        world.advance(Duration::from_millis(100))?;

        let c_rev1 = world
            .client_merged_snapshot()?
            .cards
            .iter()
            .find(|c| c.id == card_id)
            .unwrap()
            .revision;
        let _ = world.client_call(
            "card.update",
            json!({
                "card_id": card_id,
                "revision": c_rev1,
                "title": "Client Offline Title",
                "body": "client-body",
            }),
            Some(&project_id),
        )?;
        world.server_call(
            "remote-a",
            "card.update",
            json!({
                "card_id": srv_card_id,
                "revision": c_rev1,
                "title": "Server Concurrent Title",
                "body": "server-body",
            }),
        )?;

        world.heal_and_quiesce(20_000)?;
        let conflicts: Vec<_> = world
            .client_sync_items()
            .into_iter()
            .filter(|i| matches!(i.state, SyncItemState::Conflict))
            .collect();
        ensure!(
            !conflicts.is_empty(),
            "Expected concurrent offline/server edit to surface SyncItemState::Conflict"
        );
        for c in &conflicts {
            world.client_sync_action(&c.host_id, &c.mutation_id, SyncAction::UseMine)?;
        }
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;
        world.hit("remote_faults.sync_action_use_mine");

        let after_use_mine = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .into_iter()
            .find(|c| c.id == srv_card_id)
            .unwrap();
        ensure!(after_use_mine.title == "Client Offline Title");

        // Second same-field conflict: exercise `SyncAction::Retry` first, then `SyncAction::UseEdited`!
        world.partition("ui-client", "remote-a");
        world.advance(Duration::from_millis(100))?;
        let c_rev2 = after_use_mine.revision;
        let _ = world.client_call(
            "card.update",
            json!({
                "card_id": card_id,
                "revision": c_rev2,
                "title": "Client Draft Title 2",
            }),
            Some(&project_id),
        )?;
        world.server_call(
            "remote-a",
            "card.update",
            json!({
                "card_id": srv_card_id,
                "revision": c_rev2,
                "title": "Server Concurrent Title 2",
            }),
        )?;
        world.heal_and_quiesce(20_000)?;
        let conflict2 = world
            .client_sync_items()
            .into_iter()
            .find(|i| matches!(i.state, SyncItemState::Conflict))
            .expect("Expected second conflict for Retry + UseEdited");

        // `Retry` re-queues without losing the mutation (still conflicts until edited/forced).
        world.client_sync_action(
            &conflict2.host_id,
            &conflict2.mutation_id,
            SyncAction::Retry,
        )?;
        world.advance(Duration::from_millis(400))?;
        world.heal_and_quiesce(20_000)?;
        world.hit("remote_faults.sync_action_retry");

        let conflict2_after_retry = world
            .client_sync_items()
            .into_iter()
            .find(|i| i.mutation_id == conflict2.mutation_id)
            .expect("Mutation must remain tracked after Retry on still-conflicting card");
        world.client_sync_action(
            &conflict2_after_retry.host_id,
            &conflict2_after_retry.mutation_id,
            SyncAction::UseEdited {
                fields: BTreeMap::from([("title".into(), "Manually Merged Title".into())]),
            },
        )?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;
        world.hit("remote_faults.sync_action_use_edited");

        let after_use_edited = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .into_iter()
            .find(|c| c.id == srv_card_id)
            .unwrap();
        ensure!(
            after_use_edited.title == "Manually Merged Title",
            "UseEdited did not apply expected title: got {:?}",
            after_use_edited.title
        );

        // 5. Client crash & bounce recovery reloading `.outbox.json` from disk!
        world.partition("ui-client", "remote-a");
        world.advance(Duration::from_millis(100))?;
        let bounce_created = world.client_call(
            "card.create",
            json!({
                "project_id": project_id,
                "title": "Survives Client Crash",
                "body": "persisted-in-outbox-json",
            }),
            Some(&project_id),
        )?;
        let bounce_cid = bounce_created["id"].as_str().unwrap().to_owned();
        let _ = world.client_call(
            "comment.add",
            json!({
                "card_id": bounce_cid,
                "body": "outbox-comment-before-crash",
            }),
            Some(&project_id),
        )?;
        ensure!(
            !world.client_sync_items().is_empty(),
            "Expected pending outbox items before client crash"
        );
        world.crash_client()?;
        world.advance(Duration::from_millis(250))?;
        world.bounce_client()?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        let post_bounce_snap = world.server("remote-a").unwrap().snapshot()?;
        let recovered = post_bounce_snap
            .cards
            .iter()
            .find(|c| c.title == "Survives Client Crash")
            .expect("Offline card in .outbox.json was lost across bounce_client()!");
        ensure!(recovered.body == "persisted-in-outbox-json");
        oracle.record_committed_card(
            "remote-a",
            &recovered.id,
            &recovered.project_id,
            recovered.revision,
        );
        oracle.record_committed_comment("remote-a", &recovered.id, "outbox-comment-before-crash");
        world.hit("remote_faults.client_bounce_outbox_recovery");

        // 6. Reconnect Window Injection:
        //    Create card + comment while partitioned, then repair+hold (link reconnecting while
        //    outbox is not yet drained) and open immediately — NEVER `Card not found`!
        world.partition("ui-client", "remote-a");
        world.advance(Duration::from_millis(100))?;
        let win_created = world.client_call(
            "card.create",
            json!({
                "project_id": project_id,
                "title": "Reconnect Window Card",
                "body": "offline-window-body",
            }),
            Some(&project_id),
        )?;
        let win_cid = win_created["id"].as_str().unwrap().to_owned();
        let _ = world.client_call(
            "comment.add",
            json!({
                "card_id": win_cid,
                "body": "window-pending-comment",
            }),
            Some(&project_id),
        )?;
        let off_card =
            InvariantOracle::assert_card_complete_and_readable(world, &win_cid, &project_id)?;
        ensure!(
            off_card.title == "Reconnect Window Card" && off_card.body == "offline-window-body"
        );

        world.hold("ui-client", "remote-a");
        world.repair("ui-client", "remote-a");
        world.advance(Duration::from_millis(100))?;
        let win_card =
            InvariantOracle::assert_card_complete_and_readable(world, &win_cid, &project_id)?;
        ensure!(
            win_card.title == "Reconnect Window Card" && win_card.body == "offline-window-body"
        );
        world.release("ui-client", "remote-a");
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        // 7. Instant Disconnect Right After Sync + Concurrent External Mutation (Update + Archive):
        let fresh_srv = world.server_call(
            "remote-a",
            "card.create",
            json!({
                "view": "full",
                "project_id": raw_pid,
                "title": "Instant Disconnect Card",
                "body": "synced-body-complete",
            }),
        )?;
        let fresh_raw_id = fresh_srv["id"].as_str().unwrap().to_owned();
        oracle.record_committed_card(
            "remote-a",
            &fresh_raw_id,
            &raw_pid,
            fresh_srv["revision"].as_u64().unwrap_or(1),
        );
        let fresh_cid = canban_types::qualify_host_id("remote-a", &fresh_raw_id);
        world.wait_until(10_000, |w| {
            let merged = w.client_merged_snapshot()?;
            Ok(merged
                .cards
                .iter()
                .any(|c| c.id == fresh_cid || c.id == fresh_raw_id))
        })?;
        let visible_fresh_id = world
            .client_merged_snapshot()?
            .cards
            .iter()
            .find(|c| c.id == fresh_cid || c.id == fresh_raw_id)
            .unwrap()
            .id
            .clone();
        world.partition("ui-client", "remote-a");
        let instant_card = InvariantOracle::assert_card_complete_and_readable(
            world,
            &visible_fresh_id,
            &project_id,
        )?;
        ensure!(
            instant_card.title == "Instant Disconnect Card"
                && instant_card.body == "synced-body-complete"
        );
        world.heal_and_quiesce(20_000)?;

        let ext_arch = world.server_call(
            "remote-a",
            "card.create",
            json!({
                "view": "full",
                "project_id": raw_pid,
                "title": "External Archive Target",
                "body": "will-be-archived-externally",
            }),
        )?;
        let ext_arch_raw = ext_arch["id"].as_str().unwrap().to_owned();
        let ext_arch_rev = ext_arch["revision"].as_u64().unwrap();
        world.wait_until(10_000, |w| {
            let merged = w.client_merged_snapshot()?;
            Ok(merged
                .cards
                .iter()
                .any(|c| c.id == ext_arch_raw || c.id.ends_with(&format!("::{ext_arch_raw}"))))
        })?;
        let visible_arch_id = world
            .client_merged_snapshot()?
            .cards
            .iter()
            .find(|c| c.id == ext_arch_raw || c.id.ends_with(&format!("::{ext_arch_raw}")))
            .unwrap()
            .id
            .clone();

        let fresh_rev = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .iter()
            .find(|c| c.id == fresh_raw_id)
            .unwrap()
            .revision;
        world.server_call(
            "remote-a",
            "card.update",
            json!({
                "view": "full",
                "card_id": fresh_raw_id,
                "revision": fresh_rev,
                "title": "Externally Updated Card",
                "body": "externally-updated-body",
            }),
        )?;
        world.server_call(
            "remote-a",
            "card.archive",
            json!({
                "card_id": ext_arch_raw,
                "revision": ext_arch_rev,
            }),
        )?;

        let updated_card = InvariantOracle::assert_card_complete_and_readable(
            world,
            &visible_fresh_id,
            &project_id,
        )?;
        ensure!(
            updated_card.title == "Externally Updated Card"
                && updated_card.body == "externally-updated-body"
        );
        let _ = world.client_call(
            "card.get",
            json!({ "card_id": visible_arch_id, "view": "full" }),
            Some(&project_id),
        );
        let _ = world.client_call(
            "timeline.list",
            json!({ "card_id": visible_arch_id, "tail": true, "limit": 50, "view": "full" }),
            Some(&project_id),
        );

        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;
        let final_merged = world.client_merged_snapshot()?;
        ensure!(
            !final_merged
                .cards
                .iter()
                .any(|c| c.id == visible_arch_id && c.active()),
            "Externally archived card remained active in merged_snapshot after quiescence!"
        );
        ensure!(final_merged.cards.iter().any(|c| c.id == visible_fresh_id));

        Ok(finalize_digest(world))
    })
}
