//! Scenario #4 — Page data fetching & mutations at the `canban-client` / UI boundary:
//! write receipt hydration, multi-host routing, colliding raw IDs, offline read-cache + pending
//! comment timeline augmentation, and bounded merged snapshot/runtime projections.
use crate::{
    oracle::InvariantOracle,
    world::{SimWorld, SimWorldConfig},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Duration;

pub fn run(seed: u64) -> Result<u64> {
    let cfg = SimWorldConfig::desktop_default(seed, "client_pages");
    let mut world = SimWorld::new(cfg)?;
    let mut oracle = InvariantOracle::default();

    // Create projects across all 3 hosts (local, remote-a, remote-b).
    let dir_local = world.project_dir("local")?;
    let dir_a = world.project_dir("a")?;
    let dir_b = world.project_dir("b")?;
    let p_local = world.client_call(
        "project.create",
        json!({ "host_id": "local", "name": "Local Proj", "directory": dir_local }),
        None,
    )?;
    let p_a = world.client_call(
        "project.create",
        json!({ "host_id": "remote-a", "name": "Remote A Proj", "directory": dir_a }),
        None,
    )?;
    let p_b = world.client_call(
        "project.create",
        json!({ "host_id": "remote-b", "name": "Remote B Proj", "directory": dir_b }),
        None,
    )?;
    let pid_local = p_local["id"].as_str().unwrap().to_owned();
    let pid_a = p_a["id"].as_str().unwrap().to_owned();
    let pid_b = p_b["id"].as_str().unwrap().to_owned();
    world.advance(Duration::from_millis(200))?;

    // Online write receipt hydration: immediately after `card.create` returns, `resolve_target_host`
    // must route subsequent calls for that card ID to its owning host without waiting on resync.
    let c_a = world.client_call(
        "card.create",
        json!({ "project_id": pid_a, "title": "Page Card A", "body": "initial", "stage": "backlog" }),
        Some(&pid_a),
    )?;
    let cid_a = c_a["id"].as_str().unwrap().to_owned();
    ensure!(
        c_a["revision"].as_u64().unwrap_or(0) >= 1,
        "card.create write receipt must include monotonic revision"
    );
    let routed =
        world
            .client()
            .resolve_target_host("card.update", &json!({ "card_id": cid_a }), None);
    ensure!(
        routed == "remote-a",
        "Write receipt must immediately hydrate owner routing (got {routed:?})"
    );

    let c_b = world.client_call(
        "card.create",
        json!({ "project_id": pid_b, "title": "Page Card B", "body": "initial", "stage": "backlog" }),
        Some(&pid_b),
    )?;
    let cid_b = c_b["id"].as_str().unwrap().to_owned();
    let c_local = world.client_call(
        "card.create",
        json!({ "project_id": pid_local, "title": "Page Card Local", "body": "initial", "stage": "backlog" }),
        Some(&pid_local),
    )?;
    let cid_local = c_local["id"].as_str().unwrap().to_owned();

    // Add committed comments on remote-a and warm timeline read-cache online.
    let _ = world.client_call(
        "comment.add",
        json!({ "card_id": cid_a, "body": "online-comment-1" }),
        Some(&pid_a),
    )?;
    world.advance(Duration::from_millis(150))?;

    let tl_online = world.client_call(
        "timeline.list",
        json!({ "card_id": cid_a, "tail": true, "limit": 200, "view": "full" }),
        Some(&pid_a),
    )?;
    let online_events = tl_online["events"].as_array().unwrap();
    ensure!(
        online_events.iter().any(|e| {
            e["kind"] == "comment.created"
                && (e["body"] == "online-comment-1"
                    || e["summary"] == "online-comment-1"
                    || e["data"]["body"] == "online-comment-1")
        }),
        "Online timeline.list must include committed comment (got {:?})",
        online_events
    );

    // Partition remote-a, add an offline comment, and verify offline `timeline.list` serves the
    // cached committed timeline augmented with the pending comment and zero duplicate event IDs.
    world.partition("ui-client", "remote-a");
    world.advance(Duration::from_millis(100))?;

    let _ = world.client_call(
        "comment.add",
        json!({ "card_id": cid_a, "body": "offline-pending-comment" }),
        Some(&pid_a),
    )?;
    let tl_offline = world.client_call(
        "timeline.list",
        json!({ "card_id": cid_a, "tail": true, "limit": 200, "view": "full" }),
        Some(&pid_a),
    )?;
    let offline_events = tl_offline["events"].as_array().unwrap();
    let mut seen_ids = BTreeSet::new();
    for ev in offline_events {
        let eid = ev["id"].as_str().unwrap_or("");
        ensure!(
            seen_ids.insert(eid.to_owned()),
            "Duplicate event ID {eid} in offline timeline.list"
        );
    }
    ensure!(
        offline_events.iter().any(|e| {
            e["body"] == "online-comment-1"
                || e["summary"] == "online-comment-1"
                || e["data"]["body"] == "online-comment-1"
        }) && offline_events.iter().any(|e| {
            e["body"] == "offline-pending-comment"
                || e["summary"] == "offline-pending-comment"
                || e["data"]["body"] == "offline-pending-comment"
        }),
        "Offline timeline.list must merge cached committed events with pending comments (got {:?})",
        offline_events
    );

    // Local & remote-b remain fully operational while remote-a is partitioned.
    let local_rev = world
        .client_merged_snapshot()?
        .cards
        .iter()
        .find(|c| c.id == cid_local)
        .unwrap()
        .revision;
    let _ = world.client_call(
        "card.move",
        json!({ "card_id": cid_local, "revision": local_rev, "stage": "review" }),
        Some(&pid_local),
    )?;
    let b_rev = world
        .client_merged_snapshot()?
        .cards
        .iter()
        .find(|c| c.id == cid_b)
        .unwrap()
        .revision;
    let _ = world.client_call(
        "card.update",
        json!({ "card_id": cid_b, "revision": b_rev, "title": "Page Card B Updated" }),
        Some(&pid_b),
    )?;

    oracle.check_step_invariants(&world)?;

    // Heal & quiesce: verify timeline has both comments once (zero duplicates) and all 3 hosts converge.
    world.heal_and_quiesce(20_000)?;
    oracle.check_quiescent_invariants(&world)?;

    let tl_after = world.client_call(
        "timeline.list",
        json!({ "card_id": cid_a, "tail": true, "limit": 200, "view": "full" }),
        Some(&pid_a),
    )?;
    let comment_msgs: Vec<&str> = tl_after["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "comment.created")
        .filter_map(|e| {
            e["body"]
                .as_str()
                .or_else(|| e["summary"].as_str())
                .or_else(|| e["data"]["body"].as_str())
        })
        .collect();
    ensure!(
        comment_msgs == vec!["online-comment-1", "offline-pending-comment"],
        "Post-heal timeline.list comments mismatch: {:?}",
        comment_msgs
    );

    // Verify merged_snapshot & merged_runtime_state include all 3 hosts cleanly.
    let merged = world.client_merged_snapshot()?;
    ensure!(merged.projects.len() == 3);
    ensure!(merged.cards.len() == 3);
    let _rt = world.client_merged_runtime()?;

    Ok(world.trace.digest())
}
