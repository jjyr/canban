//! Scenario #2 — Remote host operations under random network drops, partitions,
//! one-way splits (after-commit drops), server crashes/bounces, and concurrent edit conflicts.
use crate::{
    oracle::InvariantOracle,
    world::{SimWorld, SimWorldConfig},
};
use anyhow::{Result, ensure};
use canban_types::{SyncAction, SyncItemState};
use rand::Rng;
use serde_json::json;
use std::time::Duration;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::mobile_default(seed, "remote_faults");
    cfg.remote_hosts = vec!["remote-a".into()];
    cfg.min_latency_ms = 5;
    cfg.max_latency_ms = 40;
    let mut world = SimWorld::new(cfg)?;
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
            "stage": "backlog",
        }),
        Some(&project_id),
    )?;
    let card_id = created["id"].as_str().unwrap().to_owned();

    // Queue strict FIFO sequence of updates and comments while partitioned.
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

    oracle.check_step_invariants(&world)?;

    // Heal & drain outbox; verify exact FIFO convergence and zero duplicate cards/comments.
    world.heal_and_quiesce(20_000)?;
    oracle.check_quiescent_invariants(&world)?;

    let srv_snap = world.server("remote-a").unwrap().snapshot()?;
    ensure!(
        srv_snap.cards.len() == 1,
        "Expected exactly 1 card on remote-a after FIFO replay, got {}",
        srv_snap.cards.len()
    );
    ensure!(
        srv_snap.cards[0].title == format!("FIFO v{expected_steps}"),
        "FIFO final title mismatch: got {:?}",
        srv_snap.cards[0].title
    );
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
    ensure!(
        comments.len() == expected_steps,
        "Expected {expected_steps} unique comments after FIFO drain, got {:?}",
        comments
    );
    for (idx, body) in comments.iter().enumerate() {
        ensure!(
            *body == format!("comment-{}", idx + 1),
            "Comment order violated at index {idx}: got {body:?}"
        );
    }

    // 2. After-commit drop simulation (one-way partition dropping reply after server commits)
    //    + random server crash/bounce while issuing online/offline mutations.
    let rounds = 4usize;
    for r in 0..rounds {
        let oneway = world.rng.random_bool(0.5);
        if oneway {
            // Drop replies from remote-a -> ui-client while request may already be in flight.
            world.partition_oneway("remote-a", "ui-client");
        } else {
            world.partition("ui-client", "remote-a");
        }
        let _ = world.client_call(
            "card.create",
            json!({
                "project_id": project_id,
                "title": format!("Resilient card {r}"),
                "body": format!("seed-{seed}-round-{r}"),
                "stage": "backlog",
            }),
            Some(&project_id),
        )?;
        world.advance(Duration::from_millis(120))?;
        if r % 2 == 1 {
            world.crash_server("remote-a")?;
            world.advance(Duration::from_millis(200))?;
            world.bounce_server("remote-a")?;
        }
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(&world)?;
    }

    let srv_snap = world.server("remote-a").unwrap().snapshot()?;
    ensure!(
        srv_snap.cards.len() == 1 + rounds,
        "Zero-duplicate card invariant violated: expected {} cards, found {}",
        1 + rounds,
        srv_snap.cards.len()
    );

    // 3. Concurrent edit conflict detection and resolution (`SyncAction::UseMine`).
    world.partition("ui-client", "remote-a");
    world.advance(Duration::from_millis(100))?;

    let client_rev = world
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
            "revision": client_rev,
            "title": "Client Offline Title",
            "body": "client-body",
        }),
        Some(&project_id),
    )?;
    // Concurrently mutate the same card directly on the server.
    let srv_card = world.server("remote-a").unwrap().snapshot()?.cards[0].clone();
    world.server("remote-a").unwrap().call(
        "card.update",
        json!({
            "card_id": srv_card.id,
            "revision": srv_card.revision,
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
        "Expected concurrent offline/server edit to surface a SyncItemState::Conflict"
    );
    for c in conflicts {
        world.client_sync_action(&c.host_id, &c.mutation_id, SyncAction::UseMine)?;
    }
    world.heal_and_quiesce(20_000)?;
    oracle.check_quiescent_invariants(&world)?;

    let final_card = world
        .server("remote-a")
        .unwrap()
        .snapshot()?
        .cards
        .into_iter()
        .find(|c| c.id == srv_card.id)
        .unwrap();
    ensure!(
        final_card.title == "Client Offline Title",
        "UseMine conflict resolution did not apply: got {:?}",
        final_card.title
    );

    // 4. Reconnect Window Injection:
    //    Create card + comment while partitioned, then repair+hold (link reconnecting while outbox
    //    is not yet drained) and immediately open the visible card (`card.get` + `timeline.list`).
    //    Must return complete Card & pending comment — NEVER `Card not found`!
    world.partition("ui-client", "remote-a");
    world.advance(Duration::from_millis(100))?;
    let win_created = world.client_call(
        "card.create",
        json!({
            "project_id": project_id,
            "title": "Reconnect Window Card",
            "body": "offline-window-body",
            "stage": "backlog",
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
    // Verify readable while fully partitioned.
    let off_card =
        InvariantOracle::assert_card_complete_and_readable(&mut world, &win_cid, &project_id)?;
    ensure!(off_card.title == "Reconnect Window Card" && off_card.body == "offline-window-body");

    // Reconnect window: repair link while holding delivery so outbox cannot finish draining yet.
    world.hold("ui-client", "remote-a");
    world.repair("ui-client", "remote-a");
    world.advance(Duration::from_millis(100))?;
    let win_card =
        InvariantOracle::assert_card_complete_and_readable(&mut world, &win_cid, &project_id)?;
    ensure!(
        win_card.title == "Reconnect Window Card" && win_card.body == "offline-window-body",
        "Reconnect-window card.get returned incomplete/stale card: {win_card:?}"
    );
    world.release("ui-client", "remote-a");
    world.heal_and_quiesce(20_000)?;
    oracle.check_quiescent_invariants(&world)?;
    let synced_win_card =
        InvariantOracle::assert_card_complete_and_readable(&mut world, &win_cid, &project_id)?;
    ensure!(synced_win_card.title == "Reconnect Window Card");

    // 5. Instant Disconnect Right After Sync:
    //    Server creates a new card; on the EXACT tick it appears in `merged_snapshot()` (before
    //    any `timeline.list` open or prefetch completes), immediately partition and open the card!
    let srv_a = world.server("remote-a").unwrap();
    let raw_pid = canban_types::split_qualified_host_id(&project_id)
        .map(|(_, r)| r.to_owned())
        .unwrap_or_else(|| project_id.clone());
    let fresh_srv = srv_a.call(
        "card.create",
        json!({
            "view": "full",
            "project_id": raw_pid,
            "title": "Instant Disconnect Card",
            "body": "synced-body-complete",
            "stage": "backlog",
        }),
    )?;
    let fresh_raw_id = fresh_srv["id"].as_str().unwrap().to_owned();
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
    // Same-tick disconnect right after card became visible!
    world.partition("ui-client", "remote-a");
    let instant_card = InvariantOracle::assert_card_complete_and_readable(
        &mut world,
        &visible_fresh_id,
        &project_id,
    )?;
    ensure!(
        instant_card.title == "Instant Disconnect Card"
            && instant_card.body == "synced-body-complete",
        "Instant-disconnect card.get returned incomplete Card: {instant_card:?}"
    );
    world.heal_and_quiesce(20_000)?;

    // 6. Concurrent External Mutation (Update + Archive on server while client opens visible cards):
    let ext_arch = srv_a.call(
        "card.create",
        json!({
            "view": "full",
            "project_id": raw_pid,
            "title": "External Archive Target",
            "body": "will-be-archived-externally",
            "stage": "backlog",
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

    // Concurrently update `fresh_raw_id` AND archive `ext_arch_raw` directly on remote-a!
    let fresh_rev = srv_a
        .snapshot()?
        .cards
        .iter()
        .find(|c| c.id == fresh_raw_id)
        .unwrap()
        .revision;
    srv_a.call(
        "card.update",
        json!({
            "view": "full",
            "card_id": fresh_raw_id,
            "revision": fresh_rev,
            "title": "Externally Updated Card",
            "body": "externally-updated-body",
        }),
    )?;
    srv_a.call(
        "card.archive",
        json!({
            "card_id": ext_arch_raw,
            "revision": ext_arch_rev,
        }),
    )?;

    // Client opens both immediately: updated card must be complete & readable;
    // externally archived card must not corrupt state and must converge cleanly!
    let updated_card = InvariantOracle::assert_card_complete_and_readable(
        &mut world,
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
    oracle.check_quiescent_invariants(&world)?;
    let final_merged = world.client_merged_snapshot()?;
    ensure!(
        !final_merged
            .cards
            .iter()
            .any(|c| c.id == visible_arch_id && c.active()),
        "Externally archived card remained active in merged_snapshot after quiescence!"
    );
    ensure!(
        final_merged.cards.iter().any(|c| c.id == visible_fresh_id),
        "Active card vanished after concurrent external mutation!"
    );

    Ok(world.trace.digest())
}
