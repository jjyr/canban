//! Scenario #7 — Herdr state changes -> Terminals & Cards:
//! `working` -> `idle` -> `done` -> `blocked` (wait notice + dedup + stale-seq rejection) ->
//! `working` (resolved wait) -> `missed` snapshot resync -> subscriber drop recovery ->
//! Core restart (`replaced`) -> `lost`/`finished` exit handling, plus `merged_runtime_state()` parity.
use crate::{
    oracle::InvariantOracle,
    world::{SimWorld, SimWorldConfig},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::time::Duration;

const SIMPLE_FLOW: &str = r#"schema: 1
id: herdr-flow
name: Herdr Lifecycle Flow
workspace:
  create_worktree: false
  sync_base: false
on:
  develop:
    event: card.column_entered
    start_at: work
    match:
      column: develop
steps:
  - id: work
    uses: agent
    with:
      profile: developer
      instruction: Work on card
    next: finish
  - id: finish
    uses: complete
"#;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "herdr_lifecycle");
    cfg.remote_hosts = vec!["remote-a".into()];
    let mut world = SimWorld::new(cfg)?;
    let mut oracle = InvariantOracle::default();

    let dir = world.project_dir("herdr-life")?;
    let proj = world.client_call(
        "project.create",
        json!({ "host_id": "remote-a", "name": "Herdr Life", "directory": dir }),
        None,
    )?;
    let pid = proj["id"].as_str().unwrap().to_owned();

    let sources =
        world.client_call("workflow.sources", json!({ "project_id": pid }), Some(&pid))?;
    let _ = world.client_call(
        "workflow.apply",
        json!({
            "project_id": pid,
            "revision": sources["revision"],
            "sources": [{ "filename": "herdr-flow.yaml", "source": SIMPLE_FLOW }],
        }),
        Some(&pid),
    )?;

    // Card 1: Launch agent terminal on remote-a.
    let c1 = world.client_call(
        "card.create",
        json!({ "project_id": pid, "title": "Herdr Observed Card" }),
        Some(&pid),
    )?;
    let cid1 = c1["id"].as_str().unwrap().to_owned();
    let raw_cid1 = canban_types::split_qualified_host_id(&cid1)
        .map(|(_, r)| r.to_owned())
        .unwrap_or_else(|| cid1.clone());
    let _ = world.client_call(
        "card.move",
        json!({
            "card_id": cid1,
            "revision": c1["revision"].as_u64().unwrap(),
            "stage": "develop",
        }),
        Some(&pid),
    )?;

    world.wait_until(10_000, |w| {
        let snap = w.server("remote-a").unwrap().snapshot()?;
        Ok(snap.terminals.iter().any(|t| {
            t.agent.as_ref().is_some_and(|a| {
                a.card_id == raw_cid1 && a.terminal.is_some() && a.state == "running"
            })
        }))
    })?;

    let herdr_a = world.herdr("remote-a").unwrap();
    let srv_a = world.server("remote-a").unwrap();
    let addr1 = srv_a
        .snapshot()?
        .terminals
        .iter()
        .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid1))
        .unwrap()
        .agent
        .as_ref()
        .unwrap()
        .terminal
        .clone()
        .unwrap();

    // 1. Working -> merged_runtime_state reports active agent parity.
    world.advance(Duration::from_millis(500))?;
    oracle.check_step_invariants(&world)?;

    // 2. Transition to `idle` and `done`: Card MUST stay in `develop` (orthogonal dimensions!).
    herdr_a.set_status(&addr1, "idle", Some(2));
    world.advance(Duration::from_millis(400))?;
    herdr_a.set_status(&addr1, "done", Some(2));
    world.advance(Duration::from_millis(400))?;
    {
        let snap = srv_a.snapshot()?;
        let card = snap.cards.iter().find(|c| c.id == raw_cid1).unwrap();
        ensure!(
            card.stage == "develop",
            "Herdr `done` must never advance card stage! Got {}",
            card.stage
        );
        let run = snap
            .terminals
            .iter()
            .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid1))
            .unwrap()
            .agent
            .as_ref()
            .unwrap();
        ensure!(
            run.runtime_status_at(world.now_ms()) == "done" && run.state == "running",
            "Expected runtime_status=done & run.state=running, got {} / {}",
            run.runtime_status_at(world.now_ms()),
            run.state
        );
    }

    // 3. Transition to `blocked`: raises durable `execution.input_required` notice (not card human_gate).
    let blocked_status = json!({
        "state": "blocked",
        "kind": "permission",
        "title": "Run cargo test?",
        "msg": "Allow command execution?",
    });
    herdr_a.set_status_detailed(&addr1, blocked_status.clone(), None);
    world.wait_until(5_000, |w| {
        let snap = w.server("remote-a").unwrap().snapshot()?;
        Ok(snap.terminals.iter().any(|t| {
            t.agent
                .as_ref()
                .is_some_and(|a| a.card_id == raw_cid1 && a.lifecycle.waiting.is_some())
        }))
    })?;
    let notice_count_blocked = srv_a.snapshot()?.notices.len();
    ensure!(
        notice_count_blocked >= 1,
        "Expected blocked wait notice to be created"
    );

    // Duplicate same blocked status + stale out-of-order seq must NOT duplicate notice or revert status!
    herdr_a.set_status_detailed(&addr1, blocked_status, None);
    herdr_a.inject_stale_event(&addr1, 1, "working");
    world.advance(Duration::from_millis(500))?;
    {
        let snap = srv_a.snapshot()?;
        ensure!(
            snap.notices.len() == notice_count_blocked,
            "Duplicate blocked status created extra notices!"
        );
        let card = snap.cards.iter().find(|c| c.id == raw_cid1).unwrap();
        ensure!(
            card.human_gate.is_none(),
            "Runtime Herdr wait must not fabricate a workflow human_gate"
        );
        let run = snap
            .terminals
            .iter()
            .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid1))
            .unwrap()
            .agent
            .as_ref()
            .unwrap();
        ensure!(
            run.runtime_status_at(world.now_ms()) == "blocked",
            "Stale seq event corrupted runtime_status to {}!",
            run.runtime_status_at(world.now_ms())
        );
    }

    // 4. Resume `working` -> wait notice auto-marked read & `execution.input_resolved` logged!
    herdr_a.set_status(&addr1, "working", Some(3));
    world.wait_until(5_000, |w| {
        let now_ms = w.now_ms();
        let snap = w.server("remote-a").unwrap().snapshot()?;
        Ok(snap.terminals.iter().any(|t| {
            t.agent.as_ref().is_some_and(|a| {
                a.card_id == raw_cid1
                    && a.lifecycle.waiting.is_none()
                    && a.runtime_status_at(now_ms) == "working"
            })
        }))
    })?;
    {
        let snap = srv_a.snapshot()?;
        ensure!(
            snap.notices.iter().all(|n| n.read),
            "Resolved runtime wait notice should be marked read"
        );
    }

    // 5. `missed` snapshot resync + subscriber drop recovery.
    herdr_a.emit_missed_snapshot();
    world.advance(Duration::from_millis(300))?;
    herdr_a.drop_subscribers();
    world.advance(Duration::from_secs(2))?;
    herdr_a.emit_heartbeat();
    world.advance(Duration::from_millis(300))?;
    {
        let obs = srv_a.call("runtime.observation", json!({}))?;
        ensure!(
            obs["connected"] == true,
            "Observer failed to reconnect after subscriber drop: {obs}"
        );
    }

    // 6. Core restart with a new `core_id` -> terminal marked `connection_state == "replaced"`.
    herdr_a.restart_core("core-remote-a-restarted");
    world.wait_until(10_000, |w| {
        let snap = w.server("remote-a").unwrap().snapshot()?;
        Ok(snap.terminals.iter().any(|t| {
            t.agent
                .as_ref()
                .is_some_and(|a| a.card_id == raw_cid1 && a.connection_state == "replaced")
        }))
    })?;
    oracle.check_step_invariants(&world)?;

    // 7. Card 2 on the restarted Core: `finish_terminal` without explicit report -> classified `failed`!
    let c2 = world.client_call(
        "card.create",
        json!({ "project_id": pid, "title": "Herdr Exit Without Report" }),
        Some(&pid),
    )?;
    let cid2 = c2["id"].as_str().unwrap().to_owned();
    let raw_cid2 = canban_types::split_qualified_host_id(&cid2)
        .map(|(_, r)| r.to_owned())
        .unwrap_or_else(|| cid2.clone());
    let _ = world.client_call(
        "card.move",
        json!({
            "card_id": cid2,
            "revision": c2["revision"].as_u64().unwrap(),
            "stage": "develop",
        }),
        Some(&pid),
    )?;

    world.wait_until(10_000, |w| {
        let snap = w.server("remote-a").unwrap().snapshot()?;
        Ok(snap.terminals.iter().any(|t| {
            t.agent.as_ref().is_some_and(|a| {
                a.card_id == raw_cid2 && a.terminal.is_some() && a.state == "running"
            })
        }))
    })?;
    let addr2 = srv_a
        .snapshot()?
        .terminals
        .iter()
        .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid2))
        .unwrap()
        .agent
        .as_ref()
        .unwrap()
        .terminal
        .clone()
        .unwrap();

    // Emit `lost` then `finished` with exit code 1 -> must classify as `failed` cleanly.
    herdr_a.lose_terminal(&addr2, "pty_eof");
    world.advance(Duration::from_millis(300))?;
    herdr_a.finish_terminal(&addr2, 1, "exited");
    world.wait_until(10_000, |w| {
        let snap = w.server("remote-a").unwrap().snapshot()?;
        Ok(snap.terminals.iter().any(|t| {
            t.agent
                .as_ref()
                .is_some_and(|a| a.card_id == raw_cid2 && a.state == "failed")
        }))
    })?;

    world.heal_and_quiesce(15_000)?;
    oracle.check_quiescent_invariants(&world)?;

    Ok(world.trace.digest())
}
