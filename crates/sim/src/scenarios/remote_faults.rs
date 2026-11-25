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

    Ok(world.trace.digest())
}
