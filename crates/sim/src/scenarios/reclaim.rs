//! Scenario #8 — Automatic idle reclamation:
//! safety under active viewer presence (`paused == true`, zero sends/kills),
//! race rollback on Herdr `status_mismatch` (cleanly clears `close_state` without stuck `closing`),
//! and liveness once unviewed and idle (sends `/exit` and transitions to `idle_closed`).
use crate::{
    oracle::InvariantOracle,
    world::{SimWorld, SimWorldConfig},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::time::Duration;

const RECLAIM_FLOW: &str = r#"schema: 1
id: reclaim-flow
name: Reclaim Flow
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
      instruction: Finish quickly
    next: finish
  - id: finish
    uses: complete
"#;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "reclaim");
    cfg.remote_hosts = vec!["remote-a".into()];
    let mut world = SimWorld::new(cfg)?;
    let mut oracle = InvariantOracle::default();

    let srv_a = world.server("remote-a").unwrap();
    srv_a.call("scheduler.configure", json!({ "idle_minutes": 1 }))?;

    let dir = world.project_dir("reclaim-proj")?;
    let proj = world.client_call(
        "project.create",
        json!({ "host_id": "remote-a", "name": "Reclaim Project", "directory": dir }),
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
            "sources": [{ "filename": "reclaim-flow.yaml", "source": RECLAIM_FLOW }],
        }),
        Some(&pid),
    )?;

    let card = world.client_call(
        "card.create",
        json!({ "project_id": pid, "title": "Idle Reclaim Card" }),
        Some(&pid),
    )?;
    let cid = card["id"].as_str().unwrap().to_owned();
    let raw_cid = canban_types::split_qualified_host_id(&cid)
        .map(|(_, r)| r.to_owned())
        .unwrap_or_else(|| cid.clone());
    let _ = world.client_call(
        "card.move",
        json!({
            "card_id": cid,
            "revision": card["revision"].as_u64().unwrap(),
            "stage": "develop",
        }),
        Some(&pid),
    )?;

    world.wait_until(10_000, |w| {
        let snap = w.server("remote-a").unwrap().snapshot()?;
        Ok(snap.terminals.iter().any(|t| {
            t.agent.as_ref().is_some_and(|a| {
                a.card_id == raw_cid && a.terminal.is_some() && a.state == "running"
            })
        }))
    })?;

    let (tid, herdr_addr) = {
        let snap = srv_a.snapshot()?;
        let t = snap
            .terminals
            .iter()
            .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid))
            .unwrap();
        (
            t.id.clone(),
            t.agent.as_ref().unwrap().terminal.clone().unwrap(),
        )
    };
    let herdr_a = world.herdr("remote-a").unwrap();

    // Mark the execution completed AND Herdr terminal idle so it enters the 30s completed reclaim window.
    srv_a.call(
        "execution.report",
        json!({
            "terminal_id": tid,
            "state": "completed",
            "message": "Done",
            "request_key": format!("reclaim-done-{seed}"),
        }),
    )?;
    herdr_a.set_status(&herdr_addr, "idle", Some(1));
    world.wait_until(5_000, |w| {
        let snap = w.server("remote-a").unwrap().snapshot()?;
        Ok(snap.terminals.iter().any(|t| {
            t.id == tid
                && t.agent
                    .as_ref()
                    .is_some_and(|a| a.state == "completed" && a.lifecycle.idle_since.is_some())
        }))
    })?;

    // PHASE A — Safety: active viewer presence MUST prevent reclamation for 90s+ virtual time!
    for _ in 0..6 {
        herdr_a.emit_heartbeat();
        srv_a.call(
            "viewer.presence",
            json!({
                "client_id": "sim-desktop-viewer",
                "card_id": raw_cid,
                "terminal_id": tid,
            }),
        )?;
        world.advance(Duration::from_secs(15))?;
        herdr_a.emit_heartbeat();
        let snap = srv_a.snapshot()?;
        let rs = snap.reclaim_status.iter().find(|r| r.terminal_id == tid);
        ensure!(
            rs.is_some_and(|r| r.paused),
            "Expected reclaim_status.paused=true while viewer is active, got {rs:?}"
        );
        ensure!(
            herdr_a.sent_inputs().is_empty() && herdr_a.killed_terminals().is_empty(),
            "SAFETY VIOLATION: idle reclaimer sent input or killed a viewed terminal!"
        );
    }

    // PHASE B — Race Rollback: clear presence, inject `status_mismatch` on next `terminal.send`,
    // and step until `execution.idle_close_requested` fires -> `close_state` MUST roll back cleanly
    // to `""` (not stuck in `"closing"` and not `"error"`), with zero `idle_close_failed` events!
    srv_a.call(
        "viewer.presence",
        json!({
            "client_id": "sim-desktop-viewer",
            "card_id": null,
            "terminal_id": null,
        }),
    )?;
    herdr_a.inject_next_send_error(
        r#"{"kind":"status_mismatch","expected":"idle","actual":"working"}"#,
    );
    let mut hb_ticks = 0u64;
    world.wait_until(40_000, |w| {
        hb_ticks += 1;
        if hb_ticks.is_multiple_of(100) {
            herdr_a.emit_heartbeat();
        }
        let evs = w.server("remote-a").unwrap().events()?;
        Ok(evs
            .iter()
            .any(|e| e.card_id == raw_cid && e.kind == "execution.idle_close_requested"))
    })?;
    {
        let snap = srv_a.snapshot()?;
        let run = snap
            .terminals
            .iter()
            .find(|t| t.id == tid)
            .unwrap()
            .agent
            .as_ref()
            .unwrap();
        ensure!(
            run.close_state.is_empty() && run.error.is_none(),
            "Expected close_state rolled back cleanly after status_mismatch, got close_state={:?} error={:?}",
            run.close_state,
            run.error
        );
        ensure!(
            herdr_a.sent_inputs().is_empty(),
            "status_mismatch error should have prevented exit delivery"
        );
        let evs = srv_a.events()?;
        ensure!(
            !evs.iter()
                .any(|e| e.card_id == raw_cid && e.kind == "execution.idle_close_failed"),
            "status_mismatch must NOT emit execution.idle_close_failed!"
        );
    }

    // PHASE C — Liveness: agent settles back to `idle` -> after the idle window elapses,
    // reclaimer sends `/exit\r`, terminal exits cleanly, and state becomes `idle_closed`!
    herdr_a.set_status(&herdr_addr, "idle", None);
    let mut tick_idx = 0u64;
    world.wait_until(120_000, |w| {
        tick_idx += 1;
        if tick_idx.is_multiple_of(100) {
            herdr_a.emit_heartbeat();
        }
        let snap = w.server("remote-a").unwrap().snapshot()?;
        Ok(snap.terminals.iter().any(|t| {
            t.id == tid
                && t.agent
                    .as_ref()
                    .is_some_and(|a| a.state == "idle_closed" && a.finished())
        }))
    })?;
    ensure!(
        herdr_a
            .sent_inputs()
            .iter()
            .any(|s| s.terminal == herdr_addr && s.text.contains("/exit")),
        "Expected reclaimer to send /exit to {herdr_addr}, got {:?}",
        herdr_a.sent_inputs()
    );

    world.heal_and_quiesce(15_000)?;
    oracle.check_quiescent_invariants(&world)?;

    Ok(world.trace.digest())
}
