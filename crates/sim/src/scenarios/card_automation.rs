//! Scenario #1 — Card state automation triggers:
//! v1->v12 workflow migration check, v12 workflow publication, manual + column-entry triggers,
//! idempotent `request_key`, approval `rework` + `approve`, mid-workflow partition & server
//! crash/bounce, `workflow.stop`, orthogonal Herdr `done` vs explicit `execution.report`.
use crate::{
    exec::SimGitRepo,
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Context, Result, ensure};
use canban_types::Card;
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

const WORKFLOW_YAML_V12: &str = r#"schema: 13
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
      default_agent: codex
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
      default_agent: developer
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

const LEGACY_V12_ASSIGNEE_YAML: &str = r#"schema: 12
id: v12-assignee-migrate
name: Legacy v12 Assignee Migration
workspace:
  create_worktree: false
  sync_base: false
on:
  develop:
    event: card.column_entered
    start_at: dev
    match:
      column: develop
steps:
  - id: dev
    uses: agent
    with:
      profile: developer
      instruction: Build task
    next: review_step
  - id: review_step
    uses: agent
    with:
      profile: reviewer
      instruction: Review task
    next: finish
  - id: finish
    uses: complete
"#;

const ASSIGNEE_PRECEDENCE_YAML: &str = r#"schema: 13
id: assignee-precedence-flow
name: Assignee Precedence Flow
workspace:
  create_worktree: false
  sync_base: false
on:
  develop:
    event: card.column_entered
    start_at: build
    match:
      column: develop
  review:
    event: card.column_entered
    start_at: check
    match:
      column: review
steps:
  - id: build
    uses: agent
    with:
      default_agent: developer
      instruction: Build with card assignee or default developer
    next: to_review
  - id: to_review
    uses: card.move
    with:
      column: review
    next: check
  - id: check
    uses: agent
    with:
      override: reviewer
      instruction: Mandatory reviewer check
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
            migrated_yaml.contains("schema: 13"),
            "Expected schema: 1 workflow to migrate to schema: 13 on publish, got:\n{migrated_yaml}"
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

        {
            // ---------------------------------------------------------------------
            // Assignee & workflow (`default_agent` vs `override`) + v12->v13 + Create & Start
            // ---------------------------------------------------------------------
            let dir2 = world.project_dir("wf-assignee")?;
            let proj2 = world.client_call(
                "project.create",
                json!({ "host_id": "remote-a", "name": "Assignee Precedence", "directory": dir2 }),
                None,
            )?;
            let pid2 = proj2["id"].as_str().unwrap().to_owned();
            let src2 = world.client_call(
                "workflow.sources",
                json!({ "project_id": pid2 }),
                Some(&pid2),
            )?;

            // 1. Mutual exclusivity: reject `default_agent` + `override` specified together.
            let invalid_dual = r#"schema: 13
id: invalid-dual
name: Invalid Dual Agent Step
on:
  start:
    event: manual
    start_at: bad
steps:
  - id: bad
    uses: agent
    with:
      default_agent: developer
      override: reviewer
    next: finish
  - id: finish
    uses: complete
"#;
            let dual_err = world
                .client_call(
                    "workflow.apply",
                    json!({
                        "project_id": pid2,
                        "revision": src2["revision"],
                        "sources": [{ "filename": "bad.yaml", "source": invalid_dual }],
                    }),
                    Some(&pid2),
                )
                .expect_err("Expected workflow.apply to reject both default_agent and override");
            ensure!(
                format!("{dual_err:#}").contains("default_agent")
                    && format!("{dual_err:#}").contains("override"),
                "Unexpected dual default_agent+override error: {dual_err:#}"
            );

            // 2. Apply v12 migration fixture (`profile: developer` -> `default_agent`,
            //    `profile: reviewer` -> `override`) alongside schema v13 precedence workflow.
            let _ = world.client_call(
                "workflow.apply",
                json!({
                    "project_id": pid2,
                    "revision": src2["revision"],
                    "sources": [
                        { "filename": "v12-migrate.yaml", "source": LEGACY_V12_ASSIGNEE_YAML },
                        { "filename": "assignee-precedence.yaml", "source": ASSIGNEE_PRECEDENCE_YAML },
                    ],
                }),
                Some(&pid2),
            )?;
            let migrated2 = world.client_call(
                "workflow.sources",
                json!({ "project_id": pid2 }),
                Some(&pid2),
            )?;
            let v12_yaml = migrated2["sources"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|s| s["filename"] == "v12-migrate.yaml")
                .and_then(|s| s["source"].as_str())
                .unwrap_or("");
            ensure!(
                v12_yaml.contains("schema: 13")
                    && v12_yaml.contains("default_agent: developer")
                    && v12_yaml.contains("override: reviewer"),
                "Expected v12->v13 migration to preserve default_agent & override, got:\n{v12_yaml}"
            );

            // Wait a moment & disable v12-migrate so only `assignee-precedence-flow` fires cleanly.
            let _ = world.client_call(
                "workflow.apply",
                json!({
                    "project_id": pid2,
                    "revision": migrated2["revision"],
                    "sources": [
                        { "filename": "assignee-precedence.yaml", "source": ASSIGNEE_PRECEDENCE_YAML },
                    ],
                }),
                Some(&pid2),
            )?;

            // 3. Assigned Card (`assignee: "codex"`) created directly in `develop` ("Create & Start"):
            //    must trigger `develop` workflow ONCE running `@codex` (`default_agent: developer` defers to assignee)
            //    and zero duplicate Backlog wakes!
            let ca = world.client_call(
                "card.create",
                json!({
                    "view": "full",
                    "project_id": pid2,
                    "title": "Assigned Create & Start",
                    "body": "Implement directly in develop",
                    "assignee": "codex",
                    "stage": "develop",
                }),
                Some(&pid2),
            )?;
            ensure!(
                ca["assignee"].as_str() == Some("codex") && ca["stage"].as_str() == Some("develop"),
                "Expected Create & Start receipt with assignee=codex & stage=develop, got {ca}"
            );
            let raw_ca = canban_types::unqualified_entity_id(ca["id"].as_str().unwrap()).to_owned();
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.agent.as_ref().is_some_and(|a| {
                        a.card_id == raw_ca && a.profile.id == "codex" && a.state == "running"
                    })
                }))
            })?;
            let herdr_a = world.herdr("remote-a").unwrap();
            let initial_ca_sends = herdr_a.sent_inputs().len();
            world.advance(Duration::from_millis(200))?;
            ensure!(
                herdr_a.sent_inputs().len() == initial_ca_sends,
                "Create & Start should not enqueue a second Backlog wake after starting develop"
            );
            let ca_dev_tid = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .terminals
                .into_iter()
                .find(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == raw_ca && a.profile.id == "codex")
                })
                .unwrap()
                .id;
            world.server_call(
                "remote-a",
                "execution.report",
                json!({
                    "terminal_id": ca_dev_tid,
                    "state": "completed",
                    "message": "Built as assigned @codex",
                    "request_key": format!("rep-ca-dev-{seed}"),
                }),
            )?;
            // Entering `review` (`override: reviewer`) MUST spawn `@reviewer`, ignoring `card.assignee = codex`!
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.agent.as_ref().is_some_and(|a| {
                        a.card_id == raw_ca && a.profile.id == "reviewer" && a.state == "running"
                    })
                }))
            })?;
            let ca_rev_tid = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .terminals
                .into_iter()
                .find(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == raw_ca && a.profile.id == "reviewer")
                })
                .unwrap()
                .id;
            world.server_call(
                "remote-a",
                "execution.report",
                json!({
                    "terminal_id": ca_rev_tid,
                    "state": "completed",
                    "message": "Reviewed as override @reviewer",
                    "request_key": format!("rep-ca-rev-{seed}"),
                }),
            )?;
            world.hit("assignee.workflow_default_agent");
            world.hit("assignee.workflow_override_preserved");

            // 4. Unassigned Card entering `develop` falls back to `default_agent: developer`.
            let cb = world.client_call(
                "card.create",
                json!({
                    "view": "full",
                    "project_id": pid2,
                    "title": "Unassigned Default Agent",
                    "stage": "develop",
                }),
                Some(&pid2),
            )?;
            let raw_cb = canban_types::unqualified_entity_id(cb["id"].as_str().unwrap()).to_owned();
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.agent.as_ref().is_some_and(|a| {
                        a.card_id == raw_cb && a.profile.id == "developer" && a.state == "running"
                    })
                }))
            })?;
            let cb_dev_tid = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .terminals
                .into_iter()
                .find(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == raw_cb && a.profile.id == "developer")
                })
                .unwrap()
                .id;
            world.server_call(
                "remote-a",
                "execution.report",
                json!({
                    "terminal_id": cb_dev_tid,
                    "state": "completed",
                    "message": "Built with default developer fallback",
                    "request_key": format!("rep-cb-dev-{seed}"),
                }),
            )?;
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.agent.as_ref().is_some_and(|a| {
                        a.card_id == raw_cb && a.profile.id == "reviewer" && a.state == "running"
                    })
                }))
            })?;
            let cb_rev_tid = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .terminals
                .into_iter()
                .find(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == raw_cb && a.profile.id == "reviewer")
                })
                .unwrap()
                .id;
            world.server_call(
                "remote-a",
                "execution.report",
                json!({
                    "terminal_id": cb_rev_tid,
                    "state": "completed",
                    "message": "Reviewed unassigned card",
                    "request_key": format!("rep-cb-rev-{seed}"),
                }),
            )?;
            // Finish the 4 completed Assignee-section PTYs so they release `max_agents` slots
            // for subsequent BoardRanking & ResolveArchive phases.
            let assignee_addrs: Vec<String> = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .terminals
                .into_iter()
                .filter_map(|t| {
                    let a = t.agent?;
                    (a.card_id == raw_ca || a.card_id == raw_cb)
                        .then_some(a.terminal)
                        .flatten()
                })
                .collect();
            for addr in &assignee_addrs {
                herdr_a.finish_terminal(addr, 0, "exited");
            }
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap
                    .terminals
                    .iter()
                    .filter_map(|t| t.agent.as_ref())
                    .filter(|a| a.card_id == raw_ca || a.card_id == raw_cb)
                    .all(|a| a.finished()))
            })?;
            world.heal_and_quiesce(15_000)?;
            oracle.check_quiescent_invariants(world)?;
        }

        // =====================================================================
        // Board Ranking (`before_card_id` + rank compaction) & Durability
        // =====================================================================
        let strip = |id: &str| {
            canban_types::split_qualified_host_id(id)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| id.to_owned())
        };
        let stage_order =
            |w: &crate::world::SimWorld, project_raw: &str, stage: &str| -> Result<Vec<String>> {
                let mut cards: Vec<Card> = w
                    .server("remote-a")
                    .unwrap()
                    .snapshot()?
                    .cards
                    .into_iter()
                    .filter(|c| c.project_id == project_raw && c.stage == stage && c.active())
                    .collect();
                cards.sort_by(Card::board_cmp);
                Ok(cards.into_iter().map(|c| c.id).collect())
            };

        let board_dir = world.project_dir(&format!("board-rank-{seed}"))?;
        let exec_remote = world
            .exec_runner("remote-a")
            .context("remote-a SimExec unavailable")?;
        exec_remote.register_git_repo(&board_dir, SimGitRepo::default());
        let proj_board = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-a",
                "name": "BoardRanking",
                "directory": board_dir,
                "target_branch": "main",
            }),
            None,
        )?;
        let pid_board = proj_board["id"].as_str().unwrap().to_owned();
        let raw_pid_board = strip(&pid_board);
        let board_sources = world.client_call(
            "workflow.sources",
            json!({ "project_id": pid_board }),
            Some(&pid_board),
        )?;
        let board_wf_yaml = WORKFLOW_YAML_V12.replace("id: sim-flow", "id: board-rank-flow");
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid_board,
                "revision": board_sources["revision"],
                "sources": [{ "filename": "board-rank-flow.yaml", "source": board_wf_yaml }],
            }),
            Some(&pid_board),
        )?;

        let ca = world.client_call(
            "card.create",
            json!({ "project_id": pid_board, "title": "Rank A" }),
            Some(&pid_board),
        )?;
        let cb = world.client_call(
            "card.create",
            json!({ "project_id": pid_board, "title": "Rank B" }),
            Some(&pid_board),
        )?;
        let cc = world.client_call(
            "card.create",
            json!({ "project_id": pid_board, "title": "Rank C" }),
            Some(&pid_board),
        )?;
        let (cid_a, raw_a) = (
            ca["id"].as_str().unwrap().to_owned(),
            strip(ca["id"].as_str().unwrap()),
        );
        let (cid_b, raw_b) = (
            cb["id"].as_str().unwrap().to_owned(),
            strip(cb["id"].as_str().unwrap()),
        );
        let (cid_c, raw_c) = (
            cc["id"].as_str().unwrap().to_owned(),
            strip(cc["id"].as_str().unwrap()),
        );

        // Give C a real human_gate (blocked execution on plan) and verify same-column reorder
        // preserves human_gate, is idempotent, emits one `card.order_changed`, and survives bounce!
        let _ = world.client_call(
            "workflow.start",
            json!({ "workflow_id": "board-rank-flow", "card_id": cid_c, "entry": "start" }),
            Some(&pid_board),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_c && a.state == "running" && a.terminal.is_some()
                })
            }))
        })?;
        let (c_tid, c_addr) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_c))
                .unwrap();
            (
                t.id.clone(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
            )
        };
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": c_tid,
                "state": "blocked",
                "message": "Review first",
                "request_key": format!("rank-c-blocked-{seed}"),
            }),
        )?;
        world
            .herdr("remote-a")
            .unwrap()
            .finish_terminal(&c_addr, 0, "exited");
        world.advance(Duration::from_millis(250))?;
        let (c_rev_before, c_gate_before, terms_before) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let c = snap.cards.iter().find(|c| c.id == raw_c).unwrap();
            ensure!(
                c.human_gate.is_some(),
                "Expected blocked report to set human_gate on C"
            );
            (c.revision, c.human_gate.clone(), snap.terminals.len())
        };
        let reorder_args = json!({
            "card_id": cid_c,
            "revision": c_rev_before,
            "stage": "backlog",
            "before_card_id": cid_b,
            "request_key": format!("reorder-c-{seed}"),
        });
        let moved_c = world.client_call("card.move", reorder_args.clone(), Some(&pid_board))?;
        let moved_c_dup = world.client_call("card.move", reorder_args, Some(&pid_board))?;
        ensure!(moved_c["revision"] == moved_c_dup["revision"]);
        ensure!(
            stage_order(world, &raw_pid_board, "backlog")?
                == vec![raw_a.clone(), raw_c.clone(), raw_b.clone()],
            "Expected backlog order [A, C, B]"
        );
        {
            let srv = world.server("remote-a").unwrap();
            let snap = srv.snapshot()?;
            let c_live = snap.cards.iter().find(|c| c.id == raw_c).unwrap();
            ensure!(
                c_live.human_gate == c_gate_before,
                "Reorder must preserve human_gate"
            );
            ensure!(
                snap.terminals.len() == terms_before,
                "Same-column reorder must not dispatch agents"
            );
            let order_events = srv
                .events()?
                .into_iter()
                .filter(|e| e.card_id == raw_c && e.kind == "card.order_changed")
                .count();
            ensure!(
                order_events == 1,
                "Expected exactly 1 card.order_changed event, got {order_events}"
            );
        }
        // No-op reorder must not bump revision.
        let c_rev_after = moved_c["revision"].as_u64().unwrap();
        let noop_c = world.client_call(
            "card.move",
            json!({
                "card_id": cid_c,
                "revision": c_rev_after,
                "stage": "backlog",
                "before_card_id": cid_b,
            }),
            Some(&pid_board),
        )?;
        ensure!(
            noop_c["revision"].as_u64() == Some(c_rev_after),
            "No-op reorder must not bump revision"
        );
        // Durability across server bounce.
        world.bounce_server("remote-a")?;
        world.heal_and_quiesce(10_000)?;
        ensure!(
            stage_order(world, &raw_pid_board, "backlog")?
                == vec![raw_a.clone(), raw_c.clone(), raw_b.clone()],
            "Custom backlog order must survive restart"
        );

        // Cross-column drop with `before_card_id` + idempotent replay dispatches exactly once;
        // stale revision and missing anchor reject cleanly without mutating snapshot.
        let b_rev = cb["revision"].as_u64().unwrap();
        let _ = world.client_call(
            "card.move",
            json!({ "card_id": cid_b, "revision": b_rev, "stage": "develop" }),
            Some(&pid_board),
        )?;
        let a_rev = ca["revision"].as_u64().unwrap();
        let drop_args = json!({
            "card_id": cid_a,
            "revision": a_rev,
            "stage": "develop",
            "before_card_id": cid_b,
            "request_key": format!("drop-a-{seed}"),
        });
        let dropped_a = world.client_call("card.move", drop_args.clone(), Some(&pid_board))?;
        let _ = world.client_call("card.move", drop_args, Some(&pid_board))?;
        ensure!(
            stage_order(world, &raw_pid_board, "develop")? == vec![raw_a.clone(), raw_b.clone()],
            "Expected develop order [A, B]"
        );
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let a_runs = snap
                .terminals
                .iter()
                .filter_map(|t| t.agent.as_ref())
                .filter(|r| r.card_id == raw_a)
                .count();
            ensure!(
                a_runs == 1,
                "Cross-column idempotent drop must dispatch exactly once (got {a_runs})"
            );
        }
        let a_rev_new = dropped_a["revision"].as_u64().unwrap();
        ensure!(
            world
                .client_call(
                    "card.move",
                    json!({ "card_id": cid_a, "revision": a_rev, "stage": "develop", "before_card_id": null }),
                    Some(&pid_board),
                )
                .is_err(),
            "Stale revision card.move must fail"
        );
        ensure!(
            world
                .client_call(
                    "card.move",
                    json!({ "card_id": cid_a, "revision": a_rev_new, "stage": "backlog", "before_card_id": "missing" }),
                    Some(&pid_board),
                )
                .is_err(),
            "Missing before_card_id anchor must fail"
        );
        ensure!(
            stage_order(world, &raw_pid_board, "develop")? == vec![raw_a.clone(), raw_b.clone()],
            "Rejected moves must leave board order unchanged"
        );

        // Exhausted midpoint ranks compact cleanly without bumping peer revisions or altering order!
        let cx = world.client_call(
            "card.create",
            json!({ "project_id": pid_board, "title": "Compact X" }),
            Some(&pid_board),
        )?;
        let cy = world.client_call(
            "card.create",
            json!({ "project_id": pid_board, "title": "Compact Y" }),
            Some(&pid_board),
        )?;
        let cz = world.client_call(
            "card.create",
            json!({ "project_id": pid_board, "title": "Compact Z" }),
            Some(&pid_board),
        )?;
        let cw = world.client_call(
            "card.create",
            json!({ "project_id": pid_board, "title": "Compact W" }),
            Some(&pid_board),
        )?;
        let (cid_x, raw_x) = (
            cx["id"].as_str().unwrap().to_owned(),
            strip(cx["id"].as_str().unwrap()),
        );
        let (cid_y, raw_y) = (
            cy["id"].as_str().unwrap().to_owned(),
            strip(cy["id"].as_str().unwrap()),
        );
        let (cid_z, raw_z) = (
            cz["id"].as_str().unwrap().to_owned(),
            strip(cz["id"].as_str().unwrap()),
        );
        let (cid_w, raw_w) = (
            cw["id"].as_str().unwrap().to_owned(),
            strip(cw["id"].as_str().unwrap()),
        );
        // Give X and Y explicit board_order separated by GAP (1024): move Y before W, then X before Y.
        let _ = world.client_call(
            "card.move",
            json!({ "card_id": cid_y, "revision": cy["revision"], "stage": "backlog", "before_card_id": cid_w }),
            Some(&pid_board),
        )?;
        let _ = world.client_call(
            "card.move",
            json!({ "card_id": cid_x, "revision": cx["revision"], "stage": "backlog", "before_card_id": cid_y }),
            Some(&pid_board),
        )?;
        let (x_rev_pin, y_rev_pin) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let x = snap.cards.iter().find(|c| c.id == raw_x).unwrap();
            let y = snap.cards.iter().find(|c| c.id == raw_y).unwrap();
            (x.revision, y.revision)
        };
        // Repeatedly bisect the gap right after X using Z and W until gap == 1, then one more move compacts!
        let mut right_cid = cid_y.clone();
        let mut right_raw = raw_y.clone();
        let mut next_is_z = true;
        let mut compacted = false;
        for _ in 0..16 {
            let (mover_cid, mover_raw) = if next_is_z {
                (&cid_z, &raw_z)
            } else {
                (&cid_w, &raw_w)
            };
            let (mover_rev, x_ord_before, right_ord_before) = {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let m = snap.cards.iter().find(|c| c.id == *mover_raw).unwrap();
                let x = snap.cards.iter().find(|c| c.id == raw_x).unwrap();
                let r = snap.cards.iter().find(|c| c.id == right_raw).unwrap();
                (m.revision, x.board_order, r.board_order)
            };
            let gap_before = match (x_ord_before, right_ord_before) {
                (Some(l), Some(r)) => r - l,
                _ => i64::MAX,
            };
            let _ = world.client_call(
                "card.move",
                json!({
                    "card_id": mover_cid,
                    "revision": mover_rev,
                    "stage": "backlog",
                    "before_card_id": right_cid,
                }),
                Some(&pid_board),
            )?;
            let snap_after = world.server("remote-a").unwrap().snapshot()?;
            let x_after = snap_after.cards.iter().find(|c| c.id == raw_x).unwrap();
            let y_after = snap_after.cards.iter().find(|c| c.id == raw_y).unwrap();
            ensure!(
                x_after.revision == x_rev_pin && y_after.revision == y_rev_pin,
                "Rank compaction must never bump peer revisions!"
            );
            if gap_before == 1 {
                let m_after = snap_after
                    .cards
                    .iter()
                    .find(|c| c.id == *mover_raw)
                    .unwrap();
                ensure!(
                    m_after.board_order.unwrap() - x_after.board_order.unwrap() > 1,
                    "Expected rank compaction to re-space board_order after gap reached 1"
                );
                let ord = stage_order(world, &raw_pid_board, "backlog")?;
                let pos_x = ord.iter().position(|id| id == &raw_x).unwrap();
                let pos_m = ord.iter().position(|id| id == mover_raw).unwrap();
                let pos_r = ord.iter().position(|id| id == &right_raw).unwrap();
                ensure!(
                    pos_x < pos_m && pos_m < pos_r,
                    "Compaction must preserve relative order"
                );
                compacted = true;
                break;
            }
            right_cid = mover_cid.clone();
            right_raw = mover_raw.clone();
            next_is_z = !next_is_z;
        }
        ensure!(
            compacted,
            "Midpoint insertions never triggered rank compaction"
        );

        // A newly created card appends after existing custom-ordered cards!
        let c_tail = world.client_call(
            "card.create",
            json!({ "project_id": pid_board, "title": "Appended Tail" }),
            Some(&pid_board),
        )?;
        let raw_tail = strip(c_tail["id"].as_str().unwrap());
        let backlog_ord = stage_order(world, &raw_pid_board, "backlog")?;
        ensure!(
            backlog_ord.last().map(String::as_str) == Some(raw_tail.as_str()),
            "New card must append after existing custom order; got {backlog_ord:?}"
        );
        world.hit("card_automation.rank_move_compaction");

        // =====================================================================
        // Manual & Agent `card.resolve` / `card.reopen`, Resolved Ordering & Next-Day Auto-Archive
        // =====================================================================
        let res_dir = world.project_dir(&format!("resolve-archive-{seed}"))?;
        exec_remote.register_git_repo(&res_dir, SimGitRepo::default());
        let proj_res = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-a",
                "name": "ResolveArchive",
                "directory": res_dir,
                "target_branch": "main",
            }),
            None,
        )?;
        let pid_res = proj_res["id"].as_str().unwrap().to_owned();
        let raw_pid_res = strip(&pid_res);
        let res_sources = world.client_call(
            "workflow.sources",
            json!({ "project_id": pid_res }),
            Some(&pid_res),
        )?;
        let res_wf_yaml = WORKFLOW_YAML_V12
            .replace("id: sim-flow", "id: resolve-arch-flow")
            .replace("create_worktree: false", "create_worktree: true");
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid_res,
                "revision": res_sources["revision"],
                "sources": [{ "filename": "resolve-arch-flow.yaml", "source": res_wf_yaml }],
            }),
            Some(&pid_res),
        )?;

        let r1 = world.client_call(
            "card.create",
            json!({ "project_id": pid_res, "title": "Resolve 1" }),
            Some(&pid_res),
        )?;
        let r2 = world.client_call(
            "card.create",
            json!({ "project_id": pid_res, "title": "Resolve 2" }),
            Some(&pid_res),
        )?;
        let r3 = world.client_call(
            "card.create",
            json!({ "project_id": pid_res, "title": "Resolve 3" }),
            Some(&pid_res),
        )?;
        let (cid_r1, raw_r1) = (
            r1["id"].as_str().unwrap().to_owned(),
            strip(r1["id"].as_str().unwrap()),
        );
        let raw_r2 = strip(r2["id"].as_str().unwrap());
        let (cid_r3, raw_r3) = (
            r3["id"].as_str().unwrap().to_owned(),
            strip(r3["id"].as_str().unwrap()),
        );
        ensure!(
            stage_order(world, &raw_pid_res, "backlog")?
                == vec![raw_r1.clone(), raw_r2.clone(), raw_r3.clone()]
        );

        // 1. User manually resolves R1 without code delivery.
        let r1_res = world.client_call(
            "card.resolve",
            json!({
                "card_id": cid_r1,
                "revision": r1["revision"],
                "message": "Already answered in discussion",
                "request_key": format!("resolve-r1-{seed}"),
            }),
            Some(&pid_res),
        )?;
        let r1_res_rev = r1_res["revision"].as_u64().unwrap();
        {
            let srv = world.server("remote-a").unwrap();
            let snap = srv.snapshot()?;
            let c1 = snap.cards.iter().find(|c| c.id == raw_r1).unwrap();
            ensure!(c1.resolved() && !c1.merged());
            let res_meta = c1.resolution.as_ref().unwrap();
            ensure!(
                res_meta.kind == "manual"
                    && res_meta.actor == "user"
                    && res_meta.reason.as_deref() == Some("Already answered in discussion")
            );
            ensure!(srv.events()?.iter().any(|e| {
                e.card_id == raw_r1
                    && e.kind == "card.resolved"
                    && e.actor == "user"
                    && e.message == "Resolved: Already answered in discussion"
            }));
        }

        // Advance clock by 2s (same UTC day) and resolve R3 -> resolved cards sort most-recent-first!
        world.advance(Duration::from_secs(2))?;
        let _ = world.client_call(
            "card.resolve",
            json!({
                "card_id": cid_r3,
                "revision": r3["revision"],
                "request_key": format!("resolve-r3-{seed}"),
            }),
            Some(&pid_res),
        )?;
        ensure!(
            stage_order(world, &raw_pid_res, "backlog")?
                == vec![raw_r2.clone(), raw_r3.clone(), raw_r1.clone()],
            "Expected unresolved first then resolved most-recent-first [R2, R3, R1]"
        );

        // New card R4 appends after unresolved R2, before resolved [R3, R1].
        let r4 = world.client_call(
            "card.create",
            json!({ "project_id": pid_res, "title": "Resolve 4" }),
            Some(&pid_res),
        )?;
        let raw_r4 = strip(r4["id"].as_str().unwrap());
        ensure!(
            stage_order(world, &raw_pid_res, "backlog")?
                == vec![
                    raw_r2.clone(),
                    raw_r4.clone(),
                    raw_r3.clone(),
                    raw_r1.clone()
                ]
        );

        // Same-stage reorder of resolved R1 is a no-op and does not bump revision.
        let noop_resolved = world.client_call(
            "card.move",
            json!({
                "card_id": cid_r1,
                "revision": r1_res_rev,
                "stage": "backlog",
                "before_card_id": cid_r3,
            }),
            Some(&pid_res),
        )?;
        ensure!(noop_resolved["revision"].as_u64() == Some(r1_res_rev));

        // 2. Agent is rejected from reopening (`card.reopen` requires explicit user decision).
        let agent_reopen_err = world
            .server_call(
                "remote-a",
                "card.reopen",
                json!({
                    "card_id": raw_r1,
                    "revision": r1_res_rev,
                    "execution": { "agent": true, "profile_id": "developer" },
                }),
            )
            .expect_err("Agent must be rejected from card.reopen");
        ensure!(
            format!("{agent_reopen_err:#}").contains("explicit user decision"),
            "Unexpected agent reopen error: {agent_reopen_err:#}"
        );

        // 3. User reopens R1 -> restores original rank before R2 & R4, and does NOT auto-archive next day!
        let r1_reopened = world.client_call(
            "card.reopen",
            json!({
                "card_id": cid_r1,
                "revision": r1_res_rev,
                "request_key": format!("reopen-r1-{seed}"),
            }),
            Some(&pid_res),
        )?;
        let r1_reopen_rev = r1_reopened["revision"].as_u64().unwrap();
        ensure!(
            stage_order(world, &raw_pid_res, "backlog")?
                == vec![
                    raw_r1.clone(),
                    raw_r2.clone(),
                    raw_r4.clone(),
                    raw_r3.clone()
                ],
            "Reopening R1 must restore its original rank before R2 and R4"
        );
        // Same day: R3 must remain unarchived before midnight boundary.
        ensure!(
            world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .cards
                .iter()
                .find(|c| c.id == raw_r3)
                .unwrap()
                .archived_at
                .is_none()
        );
        // Jump virtual clock across next calendar day (+36h) -> resolved R3 auto-archives, reopened R1 does not!
        world.servers["remote-a"]
            .clock
            .jump_ms(36 * 60 * 60 * 1_000);
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .cards
                .iter()
                .find(|c| c.id == raw_r3)
                .is_some_and(|c| c.archived_at.is_some()))
        })?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let r1_live = snap.cards.iter().find(|c| c.id == raw_r1).unwrap();
            ensure!(!r1_live.resolved() && r1_live.archived_at.is_none());
        }

        // 4. Move R1 to `develop` -> automatic workflow + active agent -> `card.resolve.preview` -> agent resolves!
        let _ = world.client_call(
            "card.move",
            json!({ "card_id": cid_r1, "revision": r1_reopen_rev, "stage": "develop" }),
            Some(&pid_res),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_r1 && a.state == "running" && a.terminal.is_some()
                })
            }))
        })?;
        let (r1_num, r1_tid, r1_addr) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let c = snap.cards.iter().find(|c| c.id == raw_r1).unwrap();
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_r1))
                .unwrap();
            (
                c.number,
                t.id.clone(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
            )
        };
        let resolve_preview = world.client_call(
            "card.resolve.preview",
            json!({ "card_id": cid_r1 }),
            Some(&pid_res),
        )?;
        ensure!(
            resolve_preview["has_active_agent"] == true
                && !resolve_preview["warnings"].as_array().unwrap().is_empty(),
            "card.resolve.preview must warn about active agent: {resolve_preview}"
        );
        let _ = world.server_call(
            "remote-a",
            "card.resolve",
            json!({
                "card": r1_num,
                "project_id": raw_pid_res,
                "profile_id": "developer",
                "message": "Duplicate of #130",
                "request_key": format!("agent-resolve-r1-{seed}"),
            }),
        )?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let c = snap.cards.iter().find(|c| c.id == raw_r1).unwrap();
            let res_meta = c.resolution.as_ref().unwrap();
            ensure!(
                res_meta.kind == "agent"
                    && res_meta.actor == "agent:developer"
                    && res_meta.reason.as_deref() == Some("Duplicate of #130")
            );
            ensure!(
                snap.workflow_runs
                    .iter()
                    .filter(|r| r.card_id.as_deref() == Some(raw_r1.as_str()))
                    .all(|r| !r.active())
            );
            let t = snap.terminals.iter().find(|t| t.id == r1_tid).unwrap();
            ensure!(t.agent.as_ref().unwrap().state == "completed");
        }
        // Finish runtime PTY, verify still unarchived today, then jump +36h -> auto-archives!
        world
            .herdr("remote-a")
            .unwrap()
            .finish_terminal(&r1_addr, 0, "exited");
        world.advance(Duration::from_millis(500))?;
        ensure!(
            world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .cards
                .iter()
                .find(|c| c.id == raw_r1)
                .unwrap()
                .archived_at
                .is_none()
        );
        world.servers["remote-a"]
            .clock
            .jump_ms(36 * 60 * 60 * 1_000);
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .cards
                .iter()
                .find(|c| c.id == raw_r1)
                .is_some_and(|c| c.archived_at.is_some()))
        })?;

        // 5. Genuinely merged Card cannot be reopened (`merged code`).
        let rm = world.client_call(
            "card.create",
            json!({ "project_id": pid_res, "title": "Merged Reopen Reject" }),
            Some(&pid_res),
        )?;
        let (cid_rm, raw_rm) = (
            rm["id"].as_str().unwrap().to_owned(),
            strip(rm["id"].as_str().unwrap()),
        );
        let _ = world.client_call(
            "review.run_agent",
            json!({ "card_id": cid_rm, "profile_id": "developer", "focus": "Prepare merge commit" }),
            Some(&pid_res),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .cards
                .iter()
                .find(|c| c.id == raw_rm)
                .is_some_and(|c| c.worktree.is_some())
                && snap.terminals.iter().any(|t| {
                    t.agent.as_ref().is_some_and(|a| {
                        a.card_id == raw_rm && a.state == "running" && a.terminal.is_some()
                    })
                }))
        })?;
        let (rm_wt, rm_addr) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let c = snap.cards.iter().find(|c| c.id == raw_rm).unwrap();
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_rm))
                .unwrap();
            (
                c.worktree.clone().unwrap(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
            )
        };
        let _ = exec_remote.commit_in_worktree(&rm_wt, "feat: merged card cannot reopen")?;
        world
            .herdr("remote-a")
            .unwrap()
            .finish_terminal(&rm_addr, 0, "exited");
        world.advance(Duration::from_millis(300))?;
        let rm_preview = world.client_call(
            "review.preview",
            json!({ "card_id": cid_rm }),
            Some(&pid_res),
        )?;
        let rm_rev = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .iter()
            .find(|c| c.id == raw_rm)
            .unwrap()
            .revision;
        let _ = world.client_call(
            "review.merge",
            json!({
                "card_id": cid_rm,
                "revision": rm_rev,
                "token": rm_preview["token"],
                "request_key": format!("merge-rm-{seed}"),
            }),
            Some(&pid_res),
        )?;
        let rm_merged_rev = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .iter()
            .find(|c| c.id == raw_rm)
            .unwrap()
            .revision;
        let reopen_merged_err = world
            .client_call(
                "card.reopen",
                json!({ "card_id": cid_rm, "revision": rm_merged_rev }),
                Some(&pid_res),
            )
            .expect_err("Merged card must reject card.reopen");
        ensure!(
            format!("{reopen_merged_err:#}").contains("merged code"),
            "Unexpected merged reopen error: {reopen_merged_err:#}"
        );
        world.hit("card_automation.resolve_reopen_auto_archive");

        world.heal_and_quiesce(10_000)?;
        oracle.check_quiescent_invariants(world)?;

        Ok(finalize_digest(world))
    })
}
