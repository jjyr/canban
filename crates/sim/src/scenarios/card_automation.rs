//! Scenario #1 — Card state automation triggers:
//! v1->v12 workflow migration check, v12 workflow publication, manual + column-entry triggers,
//! idempotent `request_key`, approval `rework` + `approve`, mid-workflow partition & server
//! crash/bounce, `workflow.stop`, orthogonal Herdr `done` vs explicit `execution.report`.
use crate::{
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use rand::Rng;
use serde_json::json;
use std::time::Duration;

const LEGACY_V1_YAML: &str = r#"schema: 1
id: legacy-migrate
name: Legacy Migration Flow
workspace:
  create_worktree: false
  sync_base: false
on:
  start:
    event: manual
    start_at: finish
steps:
  - id: finish
    uses: complete
"#;

const WORKFLOW_YAML_V12: &str = r#"schema: 12
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
    cfg.min_latency_ms = 5;
    cfg.max_latency_ms = 35;

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        // Use seed to vary mid-workflow fault placement and timing jitter.
        let bounce_before_rework = world.rng.random_bool(0.5);
        let jitter_ms = world.rng.random_range(150..=600);

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

        // Exercise sequential workflow schema migration (schema 1 -> CURRENT v12) alongside v12.
        let sources =
            world.client_call("workflow.sources", json!({ "project_id": pid }), Some(&pid))?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid,
                "revision": sources["revision"],
                "sources": [
                    { "filename": "legacy-migrate.yaml", "source": LEGACY_V1_YAML },
                    { "filename": "sim-flow.yaml", "source": WORKFLOW_YAML_V12 },
                ],
            }),
            Some(&pid),
        )?;
        let migrated_sources =
            world.client_call("workflow.sources", json!({ "project_id": pid }), Some(&pid))?;
        let migrated_yaml = migrated_sources["sources"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| s["filename"] == "legacy-migrate.yaml")
            .and_then(|s| s["source"].as_str())
            .unwrap_or("");
        ensure!(
            migrated_yaml.contains("schema: 12"),
            "Expected schema: 1 workflow to migrate to schema: 12 on publish, got:\n{migrated_yaml}"
        );
        world.hit("workflow.schema_migrated_v1_to_v12");

        // Card 1: manual start + idempotent `request_key` -> plan agent -> Herdr done (must NOT
        // advance!) -> blocked report -> card.continue -> completed report (idempotent replay) ->
        // approval gate -> `rework` loop back to `plan` -> mid-workflow partition & server bounce ->
        // approve -> develop -> build agent -> review -> complete!
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

        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid1 && a.terminal.is_some() && a.state == "running"
                })
            }))
        })?;
        oracle.check_step_invariants(world)?;

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

        // Orthogonal check: Herdr `done` MUST NOT mark execution completed or advance workflow.
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

        // Optional seed-varied mid-workflow partition & server crash/bounce before blocked/rework.
        if bounce_before_rework {
            world.partition("ui-client", "remote-a");
            world.advance(Duration::from_millis(jitter_ms))?;
            world.crash_server("remote-a")?;
            world.advance(Duration::from_millis(200))?;
            world.bounce_server("remote-a")?;
            world.heal_and_quiesce(15_000)?;
            world.hit("workflow.mid_flight_bounce");
        }

        // Blocked execution report pauses workflow and raises Card attention.
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": plan_tid,
                "state": "blocked",
                "message": "Need clarification",
                "request_key": format!("rep-blocked-{seed}"),
            }),
        )?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let card = snap.cards.iter().find(|c| c.id == raw_cid1).unwrap();
            ensure!(
                card.attention.is_some(),
                "Blocked report must set attention"
            );
            let wf = snap
                .workflow_runs
                .iter()
                .find(|r| r.card_id.as_deref() == Some(raw_cid1.as_str()))
                .unwrap();
            ensure!(wf.state == "blocked");
        }

        // Finish blocked terminal, then resume via `card.continue`.
        let cur_plan_addr = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .terminals
            .iter()
            .find(|t| t.id == plan_tid)
            .and_then(|t| t.agent.as_ref()?.terminal.clone());
        if let Some(addr) = cur_plan_addr {
            world
                .herdr("remote-a")
                .unwrap()
                .finish_terminal(&addr, 0, "exited");
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap
                    .terminals
                    .iter()
                    .any(|t| t.id == plan_tid && t.agent.as_ref().is_some_and(|a| a.finished())))
            })?;
        }
        let blocked_rev = world
            .server("remote-a")
            .unwrap()
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

        // Explicit `execution.report(completed)` + duplicate idempotent replay -> `waiting_approval`.
        let rep_params = json!({
            "terminal_id": plan_tid,
            "state": "completed",
            "message": "Plan ready v1",
            "body": "Initial plan",
            "request_key": format!("rep-plan-done-v1-{seed}"),
        });
        world.server_call("remote-a", "execution.report", rep_params.clone())?;
        world.server_call("remote-a", "execution.report", rep_params)?;
        world.advance(Duration::from_millis(300))?;

        let (wf_id, wf_visit_1, card_rev_1) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
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
            (wf.id.clone(), wf.visit, card.revision)
        };
        let qualified_wf_id = canban_types::qualify_host_id("remote-a", &wf_id);

        // Exercise `workflow.decide(rework)` -> routes back to `plan` with incremented visit!
        let _ = world.client_call(
            "workflow.decide",
            json!({
                "run_id": qualified_wf_id,
                "visit": wf_visit_1,
                "revision": card_rev_1,
                "decision": "rework",
                "feedback": "Add failure recovery plan",
                "request_key": format!("wf-rework-{seed}"),
            }),
            Some(&pid),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let wf = snap.workflow_runs.iter().find(|r| r.id == wf_id);
            Ok(wf.is_some_and(|r| {
                r.state == "waiting_agent"
                    && r.step.as_deref() == Some("plan")
                    && r.visit > wf_visit_1
            }))
        })?;
        world.hit("workflow.rework");
        oracle.check_step_invariants(world)?;

        // Seed-varied mid-workflow fault during rework if not bounced earlier.
        if !bounce_before_rework {
            world.partition("ui-client", "remote-a");
            world.advance(Duration::from_millis(jitter_ms))?;
            world.crash_server("remote-a")?;
            world.advance(Duration::from_millis(200))?;
            world.bounce_server("remote-a")?;
            world.heal_and_quiesce(15_000)?;
            world.hit("workflow.mid_flight_bounce");
        }

        // Complete reworked plan -> back to `waiting_approval` -> approve!
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid1 && a.profile.id == "codex" && !a.finished()
                })
            }))
        })?;
        let rework_tid = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == raw_cid1 && a.profile.id == "codex")
            })
            .unwrap()
            .id;
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": rework_tid,
                "state": "completed",
                "message": "Plan v2 with failure recovery",
                "request_key": format!("rep-plan-done-v2-{seed}"),
            }),
        )?;
        world.advance(Duration::from_millis(300))?;

        let (wf_visit_2, card_rev_2) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let wf = snap.workflow_runs.iter().find(|r| r.id == wf_id).unwrap();
            ensure!(
                wf.state == "waiting_approval" && wf.step.as_deref() == Some("approve"),
                "Expected waiting_approval after rework, got state={} step={:?}",
                wf.state,
                wf.step
            );
            let card = snap.cards.iter().find(|c| c.id == raw_cid1).unwrap();
            (wf.visit, card.revision)
        };

        let _ = world.client_call(
            "workflow.decide",
            json!({
                "run_id": qualified_wf_id,
                "visit": wf_visit_2,
                "revision": card_rev_2,
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
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let card = snap.cards.iter().find(|c| c.id == raw_cid1).unwrap();
            ensure!(card.stage == "develop", "Expected card in develop");
            let active_runs: Vec<_> = snap
                .workflow_runs
                .iter()
                .filter(|r| r.card_id.as_deref() == Some(raw_cid1.as_str()) && r.active())
                .collect();
            ensure!(active_runs.len() == 1);
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

        world.server_call(
            "remote-a",
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
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let card = snap.cards.iter().find(|c| c.id == raw_cid1).unwrap();
            ensure!(card.stage == "review");
            let wf = snap.workflow_runs.iter().find(|r| r.id == wf_id).unwrap();
            ensure!(wf.state == "completed");
        }
        world.hit("workflow.full_happy_path");

        // Card 2: Direct `card.move` into `develop` triggers `card.column_entered` ONCE,
        // then exercise `workflow.stop` mid-execution!
        let c2 = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": "Direct Develop Trigger + Stop" }),
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

        let c2_run_id = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .workflow_runs
            .into_iter()
            .find(|r| r.card_id.as_deref() == Some(raw_cid2.as_str()) && r.active())
            .unwrap()
            .id;
        let qualified_c2_run = canban_types::qualify_host_id("remote-a", &c2_run_id);
        let _ = world.client_call(
            "workflow.stop",
            json!({ "run_id": qualified_c2_run }),
            Some(&pid),
        )?;
        world.hit("workflow.stop");

        world.heal_and_quiesce(15_000)?;
        oracle.check_quiescent_invariants(world)?;

        let final_snap = world.server("remote-a").unwrap().snapshot()?;
        let c2_runs: Vec<_> = final_snap
            .workflow_runs
            .iter()
            .filter(|r| r.card_id.as_deref() == Some(raw_cid2.as_str()))
            .collect();
        ensure!(
            c2_runs.len() == 1 && !c2_runs[0].active(),
            "Column-entry trigger must spawn 1 run and workflow.stop must terminate it; got {c2_runs:?}"
        );
        ensure!(final_snap.projects.iter().any(|p| p.id == raw_pid));

        Ok(finalize_digest(world))
    })
}
