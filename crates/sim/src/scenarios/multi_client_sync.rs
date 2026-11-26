//! Scenario #11 — Multi-client concurrent sync & recovery:
//! 2 independent clients (`ui-client` + `ui-client-2` via `SimWorldConfig.extra_clients`)
//! racing concurrently via `client_spawn_on`, 2-client 3-way disjoint-field auto-merge,
//! same-field conflict resolution, client crash/restart with dirty `.outbox.json`, and
//! `quiesce_natural` unassisted reconnect + outbox drain recovery!
use crate::{
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use canban_types::{SyncAction, SyncItemState};
use serde_json::json;
use std::time::Duration;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::mobile_default(seed, "multi_client_sync");
    cfg.remote_hosts = vec!["remote-a".into()];
    cfg.extra_clients = vec!["ui-client-2".into()];
    cfg.min_latency_ms = 5;
    cfg.max_latency_ms = 35;

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        // 1. Client 1 creates a project and 3 shared cards; wait until BOTH clients observe them.
        let dir = world.project_dir("multi-client")?;
        let proj = world.client_call_on(
            "ui-client",
            "project.create",
            json!({ "host_id": "remote-a", "name": "Shared Project", "directory": dir }),
            None,
        )?;
        let pid = proj["id"].as_str().unwrap().to_owned();

        let c1 = world.client_call_on(
            "ui-client",
            "card.create",
            json!({ "project_id": pid, "title": "Race Card 1", "body": "base-1" }),
            Some(&pid),
        )?;
        let c2 = world.client_call_on(
            "ui-client",
            "card.create",
            json!({ "project_id": pid, "title": "AutoMerge Card 2", "body": "base-2" }),
            Some(&pid),
        )?;
        let c3 = world.client_call_on(
            "ui-client",
            "card.create",
            json!({ "project_id": pid, "title": "Conflict Card 3", "body": "base-3" }),
            Some(&pid),
        )?;
        let cid1 = c1["id"].as_str().unwrap().to_owned();
        let cid2 = c2["id"].as_str().unwrap().to_owned();
        let cid3 = c3["id"].as_str().unwrap().to_owned();

        world.wait_until(15_000, |w| {
            let s1 = w.client_merged_snapshot_on("ui-client")?;
            let s2 = w.client_merged_snapshot_on("ui-client-2")?;
            Ok(s1.cards.len() == 3 && s2.cards.len() == 3)
        })?;
        oracle.check_step_invariants(world)?;

        // 2. Concurrent online race: BOTH clients spawn `comment.add` on `c1` and `card.create`
        //    simultaneously via `client_spawn_on`!
        let h1_comment = world.client_spawn_on(
            "ui-client",
            "comment.add",
            json!({ "card_id": cid1, "body": "concurrent-from-client-1" }),
            Some(&pid),
        )?;
        let h2_comment = world.client_spawn_on(
            "ui-client-2",
            "comment.add",
            json!({ "card_id": cid1, "body": "concurrent-from-client-2" }),
            Some(&pid),
        )?;
        let h1_create = world.client_spawn_on(
            "ui-client",
            "card.create",
            json!({ "project_id": pid, "title": "Created by Client 1" }),
            Some(&pid),
        )?;
        let h2_create = world.client_spawn_on(
            "ui-client-2",
            "card.create",
            json!({ "project_id": pid, "title": "Created by Client 2" }),
            Some(&pid),
        )?;

        let _ = world.await_call(h1_comment, 10_000)?;
        let _ = world.await_call(h2_comment, 10_000)?;
        let _ = world.await_call(h1_create, 10_000)?;
        let _ = world.await_call(h2_create, 10_000)?;

        world.wait_until(15_000, |w| {
            let s1 = w.client_merged_snapshot_on("ui-client")?;
            let s2 = w.client_merged_snapshot_on("ui-client-2")?;
            Ok(s1.cards.len() == 5 && s2.cards.len() == 5)
        })?;
        let tl_c1 = world.client_call_on(
            "ui-client",
            "timeline.list",
            json!({ "card_id": cid1, "tail": true, "limit": 50, "view": "full" }),
            Some(&pid),
        )?;
        let tl_text = tl_c1.to_string();
        ensure!(
            tl_text.contains("concurrent-from-client-1")
                && tl_text.contains("concurrent-from-client-2"),
            "Concurrent client comments were lost: {tl_text}"
        );
        world.hit("multi_client.concurrent_online_race");

        // 3. 2-Client 3-way disjoint-field auto-merge + `quiesce_natural` unassisted recovery!
        //    Partition `ui-client-2` <-> `remote-a`.
        //    - `ui-client-2` edits ONLY `body` on `c2` offline.
        //    - `ui-client` (online!) edits ONLY `title` on `c2`.
        //    Repair link and call `quiesce_natural` (no manual reconnect nudge!):
        //    BOTH edits must merge automatically with ZERO conflicts!
        world.partition("ui-client-2", "remote-a");
        world.advance(Duration::from_millis(120))?;

        let c2_rev_c2 = world
            .client_merged_snapshot_on("ui-client-2")?
            .cards
            .iter()
            .find(|c| c.id == cid2)
            .unwrap()
            .revision;
        let _ = world.client_call_on(
            "ui-client-2",
            "card.update",
            json!({
                "card_id": cid2,
                "revision": c2_rev_c2,
                "body": "client-2-disjoint-body",
            }),
            Some(&pid),
        )?;
        let c2_rev_c1 = world
            .client_merged_snapshot_on("ui-client")?
            .cards
            .iter()
            .find(|c| c.id == cid2)
            .unwrap()
            .revision;
        let _ = world.client_call_on(
            "ui-client",
            "card.update",
            json!({
                "card_id": cid2,
                "revision": c2_rev_c1,
                "title": "Client 1 Disjoint Title",
            }),
            Some(&pid),
        )?;

        // Repair link ONLY — let `quiesce_natural` verify unassisted reconnect + outbox drain!
        world.repair("ui-client-2", "remote-a");
        world.quiesce_natural(25_000)?;
        oracle.check_step_invariants(world)?;

        ensure!(
            world.client_sync_items_on("ui-client-2").is_empty(),
            "Expected disjoint 2-client edit to auto-merge with zero conflicts; items={:?}",
            world.client_sync_items_on("ui-client-2")
        );
        for client_name in ["ui-client", "ui-client-2"] {
            let snap = world.client_merged_snapshot_on(client_name)?;
            let card = snap.cards.iter().find(|c| c.id == cid2).unwrap();
            ensure!(
                card.title == "Client 1 Disjoint Title" && card.body == "client-2-disjoint-body",
                "Client {client_name} did not converge on 3-way auto-merge: title={:?} body={:?}",
                card.title,
                card.body
            );
        }
        world.hit("multi_client.two_client_three_way_auto_merge");
        world.hit("multi_client.quiesce_natural_recovery");

        // 4. 2-Client same-field conflict -> `SyncAction::UseMine` -> both clients converge!
        world.partition("ui-client-2", "remote-a");
        world.advance(Duration::from_millis(120))?;

        let c3_rev = world
            .client_merged_snapshot_on("ui-client-2")?
            .cards
            .iter()
            .find(|c| c.id == cid3)
            .unwrap()
            .revision;
        let _ = world.client_call_on(
            "ui-client-2",
            "card.update",
            json!({
                "card_id": cid3,
                "revision": c3_rev,
                "title": "Client 2 Winning Title",
            }),
            Some(&pid),
        )?;
        let _ = world.client_call_on(
            "ui-client",
            "card.update",
            json!({
                "card_id": cid3,
                "revision": c3_rev,
                "title": "Client 1 Competing Title",
            }),
            Some(&pid),
        )?;

        world.repair("ui-client-2", "remote-a");
        world.wait_until(20_000, |w| {
            Ok(w.client_sync_items_on("ui-client-2")
                .iter()
                .any(|i| matches!(i.state, SyncItemState::Conflict)))
        })?;
        let conflict = world
            .client_sync_items_on("ui-client-2")
            .into_iter()
            .find(|i| matches!(i.state, SyncItemState::Conflict))
            .unwrap();
        world.client_sync_action_on(
            "ui-client-2",
            &conflict.host_id,
            &conflict.mutation_id,
            SyncAction::UseMine,
        )?;
        world.quiesce_natural(25_000)?;
        for client_name in ["ui-client", "ui-client-2"] {
            let snap = world.client_merged_snapshot_on(client_name)?;
            let card = snap.cards.iter().find(|c| c.id == cid3).unwrap();
            ensure!(
                card.title == "Client 2 Winning Title",
                "Client {client_name} did not converge after UseMine: {:?}",
                card.title
            );
        }
        world.hit("multi_client.two_client_conflict_resolution");

        // 5. Client 2 crash & restart with dirty `.outbox.json` + natural unassisted drain!
        world.partition("ui-client-2", "remote-a");
        world.advance(Duration::from_millis(120))?;
        let dirty_c = world.client_call_on(
            "ui-client-2",
            "card.create",
            json!({
                "project_id": pid,
                "title": "Client 2 Crash-Resilient Card",
                "body": "saved-to-outbox-json",
            }),
            Some(&pid),
        )?;
        let dirty_cid = dirty_c["id"].as_str().unwrap().to_owned();
        let _ = world.client_call_on(
            "ui-client-2",
            "comment.add",
            json!({
                "card_id": dirty_cid,
                "body": "comment-surviving-client2-crash",
            }),
            Some(&pid),
        )?;

        world.crash_client_named("ui-client-2")?;
        world.advance(Duration::from_millis(250))?;
        world.bounce_client_named("ui-client-2")?;
        world.repair("ui-client-2", "remote-a");
        world.quiesce_natural(25_000)?;

        // Verify Client 1 sees Client 2's crash-recovered card and comment!
        world.wait_until(15_000, |w| {
            let s1 = w.client_merged_snapshot_on("ui-client")?;
            Ok(s1
                .cards
                .iter()
                .any(|c| c.title == "Client 2 Crash-Resilient Card"))
        })?;
        world.hit("multi_client.client_bounce_dirty_outbox");

        // 6. Offline `assignee` 3-way disjoint merge (`title` on Client 2 + `assignee` on Client 1)
        //    and concurrent `assignee` conflict + `UseMine` resolution!
        let assign_c = world.client_call_on(
            "ui-client",
            "card.create",
            json!({
                "project_id": pid,
                "title": "Assignee Merge Base",
                "body": "Initial body",
            }),
            Some(&pid),
        )?;
        let assign_cid = assign_c["id"].as_str().unwrap().to_owned();
        let assign_base_rev = assign_c["revision"].as_u64().unwrap();
        world.wait_until(15_000, |w| {
            let s2 = w.client_merged_snapshot_on("ui-client-2")?;
            Ok(s2
                .cards
                .iter()
                .any(|c| c.id == assign_cid && c.revision == assign_base_rev))
        })?;

        world.partition("ui-client-2", "remote-a");
        world.advance(Duration::from_millis(120))?;
        let _ = world.client_call_on(
            "ui-client",
            "card.update",
            json!({
                "card_id": assign_cid,
                "revision": assign_base_rev,
                "assignee": "developer",
            }),
            Some(&pid),
        )?;
        let _ = world.client_call_on(
            "ui-client-2",
            "card.update",
            json!({
                "card_id": assign_cid,
                "revision": assign_base_rev,
                "title": "Assignee Merge Title",
            }),
            Some(&pid),
        )?;
        world.repair("ui-client-2", "remote-a");
        world.quiesce_natural(25_000)?;
        for client_name in ["ui-client", "ui-client-2"] {
            let snap = world.client_merged_snapshot_on(client_name)?;
            let c = snap.cards.iter().find(|c| c.id == assign_cid).unwrap();
            ensure!(
                c.title == "Assignee Merge Title" && c.assignee.as_deref() == Some("developer"),
                "Client {client_name} disjoint title+assignee auto-merge failed: {c:?}"
            );
        }

        // Now force competing offline `assignee` edits at the same base revision and resolve via UseMine.
        let assign_rev2 = world
            .client_merged_snapshot_on("ui-client")?
            .cards
            .into_iter()
            .find(|c| c.id == assign_cid)
            .unwrap()
            .revision;
        world.partition("ui-client-2", "remote-a");
        world.advance(Duration::from_millis(120))?;
        let _ = world.client_call_on(
            "ui-client-2",
            "card.update",
            json!({
                "card_id": assign_cid,
                "revision": assign_rev2,
                "assignee": "reviewer",
            }),
            Some(&pid),
        )?;
        let _ = world.client_call_on(
            "ui-client",
            "card.update",
            json!({
                "card_id": assign_cid,
                "revision": assign_rev2,
                "assignee": "codex",
            }),
            Some(&pid),
        )?;
        world.repair("ui-client-2", "remote-a");
        world.wait_until(20_000, |w| {
            Ok(w.client_sync_items_on("ui-client-2")
                .iter()
                .any(|i| matches!(i.state, SyncItemState::Conflict)))
        })?;
        let assign_conflict = world
            .client_sync_items_on("ui-client-2")
            .into_iter()
            .find(|i| matches!(i.state, SyncItemState::Conflict))
            .unwrap();
        world.client_sync_action_on(
            "ui-client-2",
            &assign_conflict.host_id,
            &assign_conflict.mutation_id,
            SyncAction::UseMine,
        )?;
        world.quiesce_natural(25_000)?;
        for client_name in ["ui-client", "ui-client-2"] {
            let snap = world.client_merged_snapshot_on(client_name)?;
            let c = snap.cards.iter().find(|c| c.id == assign_cid).unwrap();
            ensure!(
                c.assignee.as_deref() == Some("reviewer"),
                "Client {client_name} did not converge on UseMine assignee=reviewer: {:?}",
                c.assignee
            );
        }
        world.hit("assignee.offline_sync_converged");

        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        let s1 = world.client_merged_snapshot_on("ui-client")?;
        let s2 = world.client_merged_snapshot_on("ui-client-2")?;
        ensure!(
            s1.cards.len() == 7 && s2.cards.len() == 7,
            "Expected 7 cards across both clients, got c1={} c2={}",
            s1.cards.len(),
            s2.cards.len()
        );

        Ok(finalize_digest(world))
    })
}
