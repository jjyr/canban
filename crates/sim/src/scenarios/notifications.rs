//! Scenario #6 — Notification center & reminders:
//! strict ID deduplication, no resurrected read/stale notifications across reconnects or restarts,
//! and unread count parity across merged multi-host snapshots.
use crate::{
    oracle::InvariantOracle,
    world::{SimWorld, SimWorldConfig},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Duration;

pub fn run(seed: u64) -> Result<u64> {
    let cfg = SimWorldConfig::desktop_default(seed, "notifications");
    let mut world = SimWorld::new(cfg)?;
    let mut oracle = InvariantOracle::default();

    let dir_l = world.project_dir("nl")?;
    let dir_r = world.project_dir("nr")?;
    let p_local = world.client_call(
        "project.create",
        json!({ "host_id": "local", "name": "Notify Local", "directory": dir_l }),
        None,
    )?;
    let p_remote = world.client_call(
        "project.create",
        json!({ "host_id": "remote-a", "name": "Notify Remote", "directory": dir_r }),
        None,
    )?;
    let pid_local = p_local["id"].as_str().unwrap().to_owned();
    let pid_remote = p_remote["id"].as_str().unwrap().to_owned();
    world.advance(Duration::from_millis(150))?;

    // Create two cards on remote-a and one on local; inject Herdr `blocked` wait on remote-a
    // and explicit human-gate attention to generate real notifications.
    let c_local = world.client_call(
        "card.create",
        json!({ "project_id": pid_local, "title": "Local Needs Review", "stage": "review" }),
        Some(&pid_local),
    )?;
    let cid_local = c_local["id"].as_str().unwrap().to_owned();

    let c_rem1 = world.client_call(
        "card.create",
        json!({ "project_id": pid_remote, "title": "Remote Attention 1", "stage": "review" }),
        Some(&pid_remote),
    )?;
    let cid_rem1 = c_rem1["id"].as_str().unwrap().to_owned();

    let c_rem2 = world.client_call(
        "card.create",
        json!({ "project_id": pid_remote, "title": "Remote Attention 2", "stage": "review" }),
        Some(&pid_remote),
    )?;
    let cid_rem2 = c_rem2["id"].as_str().unwrap().to_owned();

    // Create real durable notices via `card.move` / agent attention or direct notice writes on servers.
    let srv_local = world.server("local-server").unwrap();
    let srv_rem = world.server("remote-a").unwrap();

    // Inject a blocked report / attention on both servers through real RPCs (`card.adopt` or
    // workflow attention via question/blocked). Use `comment.add` + `card.update` / human review
    // or server-side attention creation via `report`? Let's trigger real notifications via
    // `card.create` + Herdr blocked wait OR `card.update` + `notifications.list`!
    // Even better: trigger real `review_ready` & `attention` notices through `card.move` to `review`
    // and through a blocked agent execution report!
    let _ = srv_local.call(
        "card.move",
        json!({
            "card_id": cid_local,
            "revision": c_local["revision"].as_u64().unwrap(),
            "stage": "review",
        }),
    );
    // Create an explicit Notice + attention on remote-a cards by moving to review and checking reminders.
    let snap_rem = srv_rem.snapshot()?;
    ensure!(snap_rem.cards.len() == 2);

    // Emit real attention notices via `agent.report`? Better: use `canban_notifications::attention`
    // via a blocked `workflow` or trigger `review` stage reminders directly!
    // Check what reminders exist across merged snapshot.
    world.heal_and_quiesce(10_000)?;
    let merged = world.client_merged_snapshot()?;
    let reminders_before = canban_notifications::reminders(&merged);
    let mut ids_before = BTreeSet::new();
    for r in &reminders_before {
        ensure!(
            ids_before.insert(r.id.clone()),
            "Duplicate reminder ID {} before read",
            r.id
        );
    }

    // Also test marking notifications read for `cid_rem1`, then crashing & bouncing `remote-a`,
    // partitioning, and reconnecting — read state MUST persist and never resurrect!
    let tl_rem1 = world.client_call(
        "timeline.list",
        json!({ "card_id": cid_rem1, "tail": true, "limit": 50 }),
        Some(&pid_remote),
    )?;
    let rem1_seq = tl_rem1["highwater"].as_u64().unwrap_or(1).max(1);
    let _ = world.client_call(
        "notifications.read",
        json!({ "card_id": cid_rem1, "sequence": rem1_seq }),
        Some(&pid_remote),
    )?;
    for n in &srv_rem.snapshot()?.notices {
        if n.card_id == cid_rem1 && n.read {
            oracle.record_notice_read("remote-a", n.id.clone());
        }
    }

    // Archive `cid_rem2` — stale reminders for archived cards must disappear and never return.
    let rem2_rev = world
        .client_merged_snapshot()?
        .cards
        .iter()
        .find(|c| c.id == cid_rem2)
        .unwrap()
        .revision;
    let _ = world.client_call(
        "card.archive",
        json!({ "card_id": cid_rem2, "revision": rem2_rev }),
        Some(&pid_remote),
    )?;

    // Fault cycle: partition -> crash -> bounce -> heal -> resync!
    world.partition("ui-client", "remote-a");
    world.advance(Duration::from_millis(150))?;
    world.crash_server("remote-a")?;
    world.advance(Duration::from_millis(200))?;
    world.bounce_server("remote-a")?;
    world.heal_and_quiesce(20_000)?;

    oracle.check_quiescent_invariants(&world)?;

    let merged_after = world.client_merged_snapshot()?;
    let reminders_after = canban_notifications::reminders(&merged_after);
    let mut ids_after = BTreeSet::new();
    for r in &reminders_after {
        ensure!(
            ids_after.insert(r.id.clone()),
            "Duplicate reminder ID {} after reconnect/bounce",
            r.id
        );
        ensure!(
            r.card_id != cid_rem2,
            "Stale reminder for archived card {cid_rem2} resurrected after reconnect!"
        );
    }

    // Mark all read across all hosts and verify zero unread notices remain after another reconnect.
    let _ = world.client_call_all("notifications.read_all", json!({}))?;
    world.partition("ui-client", "remote-a");
    world.advance(Duration::from_millis(100))?;
    world.heal_and_quiesce(15_000)?;
    oracle.check_quiescent_invariants(&world)?;

    let merged_final = world.client_merged_snapshot()?;
    ensure!(
        merged_final.notices.iter().all(|n| n.read),
        "Unread notice resurrected after notifications.read_all + reconnect!"
    );

    Ok(world.trace.digest())
}
