//! Scenario #1 — Card state automation triggers:
//! workflow publication, manual + column-entry triggers, idempotent `request_key`,
//! approval gates, orthogonal Herdr `done` vs explicit `execution.report`, and stage advancement.
use crate::{
    oracle::InvariantOracle,
    world::{SimWorld, SimWorldConfig},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::time::Duration;

const WORKFLOW_YAML: &str = r#"schema: 1
id: sim-flow
name: Sim Automation Flow
workspace:
  create_worktree: false
  sync_base: false
on:
  start:
    event: manual
    start_at: plan
  develop:
    event: card.column_entered
    start_at: build
    match:
      column: develop
steps:
  - id: plan
    uses: agent
    with:
      profile: codex
      instruction: Plan the simulated task
    next: approve
  - id: approve
    uses: approval
    with:
      title: Approve simulated plan
    next: to_develop
    on_changes_requested: plan
  - id: to_develop
    uses: card.move
    with:
      column: develop
    next: build
  - id: build
    uses: agent
    with:
      profile: developer
      instruction: Build the simulated task
    next: to_review
  - id: to_review
    uses: card.move
    with:
      column: review
    next: finish
  - id: finish
    uses: complete
"#;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "card_automation");
    cfg.remote_hosts = vec!["remote-a".into()];
    let mut world = SimWorld::new(cfg)?;
    let mut oracle = InvariantOracle::default();

    let dir = world.project_dir("wf-auto")?;
    let proj = world.client_call(
        "project.create",
        json!({ "host_id": "remote-a", "name": "Automation", "directory": dir }),
        None,
    )?;
    let pid = proj["id"].as_str().unwrap().to_owned();
    let raw_pid = canban_types::split_qualified_host_id(&pid)
        .map(|(_, r)| r.to_owned())
        .unwrap_or_else(|| pid.clone());

    // Publish deterministic workflow YAML on remote-a.
    let sources =
        world.client_call("workflow.sources", json!({ "project_id": pid }), Some(&pid))?;
    let _ = world.client_call(
        "workflow.apply",
        json!({
            "project_id": pid,
            "revision": sources["revision"],
            "sources": [{ "filename": "sim-flow.yaml", "source": WORKFLOW_YAML }],
        }),
        Some(&pid),
    )?;

    // Card 1: Full manual start -> agent plan -> Herdr done (must NOT advance!) ->
    // blocked report -> completed report (with duplicate idempotency check) -> approval ->
    // auto-move to develop -> build agent -> completed report -> auto-move to review!
    let c1 = world.client_call(
        "card.create",
        json!({ "project_id": pid, "title": "Automated Feature", "body": "Ship it" }),
        Some(&pid),
    )?;
    let cid1 = c1["id"].as_str().unwrap().to_owned();
    let raw_cid1 = canban_types::split_qualified_host_id(&cid1)
        .map(|(_, r)| r.to_owned())
        .unwrap_or_else(|| cid1.clone());

    let run1 = world.client_call(
        "workflow.start",
        json!({
            "workflow_id": "sim-flow",
            "card_id": cid1,
            "entry": "start",
            "request_key": format!("wf-start-{seed}"),
        }),
        Some(&pid),
    )?;
    let run1_dup = world.client_call(
        "workflow.start",
        json!({
            "workflow_id": "sim-flow",
            "card_id": cid1,
            "entry": "start",
            "request_key": format!("wf-start-{seed}"),
        }),
        Some(&pid),
    )?;
    ensure!(
        run1["id"] == run1_dup["id"],
        "Duplicate workflow.start with same request_key spawned a second run!"
    );

    // Wait for scheduler to launch the `plan` agent terminal in SimHerdr.
    world.wait_until(10_000, |w| {
        let srv = w.server("remote-a").unwrap();
        let snap = srv.snapshot()?;
        Ok(snap.terminals.iter().any(|t| {
            t.agent.as_ref().is_some_and(|a| {
                a.card_id == raw_cid1 && a.terminal.is_some() && a.state == "running"
            })
        }))
    })?;
    oracle.check_step_invariants(&world)?;

    let (plan_tid, plan_herdr_addr) = {
        let snap = world.server("remote-a").unwrap().snapshot()?;
        let t = snap
            .terminals
            .iter()
            .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid1))
            .unwrap();
        (
            t.id.clone(),
            t.agent.as_ref().unwrap().terminal.clone().unwrap(),
        )
    };

    // Orthogonal check: Herdr reporting `done` MUST NOT mark execution completed or advance workflow!
    world
        .herdr("remote-a")
        .unwrap()
        .set_status(&plan_herdr_addr, "done", Some(2));
    world.advance(Duration::from_secs(2))?;
    {
        let snap = world.server("remote-a").unwrap().snapshot()?;
        let wf = snap
            .workflow_runs
            .iter()
            .find(|r| r.card_id.as_deref() == Some(raw_cid1.as_str()))
            .unwrap();
        ensure!(
            wf.state == "waiting_agent" && wf.step.as_deref() == Some("plan"),
            "Herdr `done` alone must NEVER advance workflow! Got state={} step={:?}",
            wf.state,
            wf.step
        );
    }

    // Explicit `execution.report` (blocked) raises attention and pauses the workflow step.
    let srv_a = world.server("remote-a").unwrap();
    srv_a.call(
        "execution.report",
        json!({
            "terminal_id": plan_tid,
            "state": "blocked",
            "message": "Need clarification",
            "request_key": format!("rep-blocked-{seed}"),
        }),
    )?;
    {
        let snap = srv_a.snapshot()?;
        let card = snap.cards.iter().find(|c| c.id == raw_cid1).unwrap();
        ensure!(
            card.attention.is_some(),
            "Blocked execution.report must set Card attention"
        );
        let wf = snap
            .workflow_runs
            .iter()
            .find(|r| r.card_id.as_deref() == Some(raw_cid1.as_str()))
            .unwrap();
        ensure!(
            wf.state == "blocked",
            "Blocked report must pause workflow run in `blocked`, got {}",
            wf.state
        );
    }

    // End the blocked agent terminal, then human answers via `card.continue` ->
    // restarts the `plan` agent cleanly and resumes the blocked workflow step!
    world
        .herdr("remote-a")
        .unwrap()
        .finish_terminal(&plan_herdr_addr, 0, "exited");
    world.wait_until(5_000, |w| {
        let snap = w.server("remote-a").unwrap().snapshot()?;
        Ok(snap
            .terminals
            .iter()
            .any(|t| t.id == plan_tid && t.agent.as_ref().is_some_and(|a| a.finished())))
    })?;
    let blocked_rev = srv_a
        .snapshot()?
        .cards
        .iter()
        .find(|c| c.id == raw_cid1)
        .unwrap()
        .revision;
    let _ = world.client_call(
        "card.continue",
        json!({
            "card_id": cid1,
            "revision": blocked_rev,
            "message": "Proceed with option A",
        }),
        Some(&pid),
    )?;
    world.wait_until(10_000, |w| {
        let snap = w.server("remote-a").unwrap().snapshot()?;
        Ok(snap.terminals.iter().any(|t| {
            t.id == plan_tid
                && t.agent
                    .as_ref()
                    .is_some_and(|a| a.state == "running" && a.terminal.is_some())
        }))
    })?;

    // Explicit `execution.report` (completed) + duplicate idempotent replay advances to approval gate!
    let rep_params = json!({
        "terminal_id": plan_tid,
        "state": "completed",
        "message": "Plan ready",
        "body": "Step-by-step plan",
        "request_key": format!("rep-plan-done-{seed}"),
    });
    srv_a.call("execution.report", rep_params.clone())?;
    srv_a.call("execution.report", rep_params)?;
    world.advance(Duration::from_millis(300))?;

    let (wf_id, wf_visit, card_rev) = {
        let snap = srv_a.snapshot()?;
        let wf = snap
            .workflow_runs
            .iter()
            .find(|r| r.card_id.as_deref() == Some(raw_cid1.as_str()))
            .unwrap();
        ensure!(
            wf.state == "waiting_approval" && wf.step.as_deref() == Some("approve"),
            "Expected waiting_approval at approve, got state={} step={:?}",
            wf.state,
            wf.step
        );
        let card = snap.cards.iter().find(|c| c.id == raw_cid1).unwrap();
        ensure!(card.stage == "backlog");
        (wf.id.clone(), wf.visit, card.revision)
    };

    // Approve via client RPC -> moves card to `develop` and launches `build` agent!
    let qualified_wf_id = canban_types::qualify_host_id("remote-a", &wf_id);
    let _ = world.client_call(
        "workflow.decide",
        json!({
            "run_id": qualified_wf_id,
            "visit": wf_visit,
            "revision": card_rev,
            "decision": "approve",
            "request_key": format!("wf-approve-{seed}"),
        }),
        Some(&pid),
    )?;

    world.wait_until(10_000, |w| {
        let snap = w.server("remote-a").unwrap().snapshot()?;
        Ok(snap.terminals.iter().any(|t| {
            t.agent.as_ref().is_some_and(|a| {
                a.card_id == raw_cid1
                    && a.profile.id == "developer"
                    && a.terminal.is_some()
                    && a.state == "running"
            })
        }))
    })?;

    let build_tid = {
        let snap = srv_a.snapshot()?;
        let card = snap.cards.iter().find(|c| c.id == raw_cid1).unwrap();
        ensure!(
            card.stage == "develop",
            "Expected card in develop after approval, got {}",
            card.stage
        );
        let active_runs: Vec<_> = snap
            .workflow_runs
            .iter()
            .filter(|r| r.card_id.as_deref() == Some(raw_cid1.as_str()) && r.active())
            .collect();
        ensure!(
            active_runs.len() == 1,
            "Expected exactly 1 active workflow run for card 1, found {}",
            active_runs.len()
        );
        snap.terminals
            .iter()
            .find(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == raw_cid1 && a.profile.id == "developer")
            })
            .unwrap()
            .id
            .clone()
    };

    // Complete build step -> card auto-moves to `review` and workflow completes!
    srv_a.call(
        "execution.report",
        json!({
            "terminal_id": build_tid,
            "state": "completed",
            "message": "Build complete",
            "request_key": format!("rep-build-done-{seed}"),
        }),
    )?;
    world.advance(Duration::from_millis(300))?;
    {
        let snap = srv_a.snapshot()?;
        let card = snap.cards.iter().find(|c| c.id == raw_cid1).unwrap();
        ensure!(
            card.stage == "review",
            "Expected card in review after build completion, got {}",
            card.stage
        );
        let wf = snap.workflow_runs.iter().find(|r| r.id == wf_id).unwrap();
        ensure!(
            wf.state == "completed",
            "Expected workflow run completed, got {}",
            wf.state
        );
    }

    // Card 2: Direct `card.move` into `develop` triggers `card.column_entered` ONCE (never twice!).
    let c2 = world.client_call(
        "card.create",
        json!({ "project_id": pid, "title": "Direct Develop Trigger" }),
        Some(&pid),
    )?;
    let cid2 = c2["id"].as_str().unwrap().to_owned();
    let raw_cid2 = canban_types::split_qualified_host_id(&cid2)
        .map(|(_, r)| r.to_owned())
        .unwrap_or_else(|| cid2.clone());
    let rev2 = c2["revision"].as_u64().unwrap();

    let _ = world.client_call(
        "card.move",
        json!({ "card_id": cid2, "revision": rev2, "stage": "develop" }),
        Some(&pid),
    )?;

    world.wait_until(10_000, |w| {
        let snap = w.server("remote-a").unwrap().snapshot()?;
        Ok(snap.terminals.iter().any(|t| {
            t.agent.as_ref().is_some_and(|a| {
                a.card_id == raw_cid2 && a.profile.id == "developer" && a.terminal.is_some()
            })
        }))
    })?;

    world.heal_and_quiesce(15_000)?;
    oracle.check_quiescent_invariants(&world)?;

    let final_snap = srv_a.snapshot()?;
    let c2_runs: Vec<_> = final_snap
        .workflow_runs
        .iter()
        .filter(|r| r.card_id.as_deref() == Some(raw_cid2.as_str()))
        .collect();
    ensure!(
        c2_runs.len() == 1,
        "Column-entry trigger must spawn exactly 1 workflow run, found {}",
        c2_runs.len()
    );
    let c2_agents: Vec<_> = final_snap
        .terminals
        .iter()
        .filter(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid2))
        .collect();
    ensure!(
        c2_agents.len() == 1,
        "Column-entry trigger must spawn exactly 1 agent terminal, found {}",
        c2_agents.len()
    );
    ensure!(final_snap.projects.iter().any(|p| p.id == raw_pid));

    Ok(world.trace.digest())
}
