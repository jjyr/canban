//! Scenario #9 — Scheduler budget, priority queueing, Herdr-offline livelock guard (#29),
//! cron schedule ticks (`overlap: skip` and `overlap: queue`), and post-bounce recovery.
use crate::{
    exec::SimGitRepo,
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, StorageFaultKind, run_with_world},
};
use anyhow::{Context, Result, ensure};
use canban_types::{GLOBAL_PROJECT_ID, ScheduleState, WorkflowStep};
use serde_json::{Value, json};
use std::time::Duration;

const SCHED_BUDGET_YAML: &str = r#"schema: 12
id: sched_budget
name: Scheduled & Budgeted Flow
workspace:
  create_worktree: false
  sync_base: false
on:
  start_backlog:
    event: manual
    start_at: work
  enter_review:
    event: card.column_entered
    match:
      column: review
    start_at: work
  cron_card:
    cron: "* * * * *"
    timezone: UTC
    overlap: skip
    start_at: work
    card:
      column: backlog
      title: "Cron Card #{{run_number}}"
      body: "Triggered via virtual clock"
  cron_skip:
    cron: "* * * * *"
    timezone: UTC
    overlap: skip
    start_at: work
  cron_queue:
    cron: "* * * * *"
    timezone: UTC
    overlap: queue
    start_at: work
steps:
  - id: work
    uses: agent
    with:
      profile: developer
      instruction: Run budgeted task
    next: done
  - id: done
    uses: complete
"#;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "schedules_and_budget");
    cfg.remote_hosts = vec!["remote-a".into()];

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        let dir = world.project_dir("sched-budget")?;
        let proj = world.client_call(
            "project.create",
            json!({ "host_id": "remote-a", "name": "Budget & Cron", "directory": dir }),
            None,
        )?;
        let pid = proj["id"].as_str().unwrap().to_owned();

        // Start with tight `max_agents: 1` to test priority & FIFO queueing strictly.
        let _ = world.client_call(
            "scheduler.configure",
            json!({ "max_agents": 1 }),
            Some(&pid),
        )?;

        let sources =
            world.client_call("workflow.sources", json!({ "project_id": pid }), Some(&pid))?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid,
                "revision": sources["revision"],
                "sources": [{ "filename": "sched_budget.yaml", "source": SCHED_BUDGET_YAML }],
            }),
            Some(&pid),
        )?;

        // Disable automatic cron rules while testing Phase 1 & Phase 2 manually so cron ticks
        // do not steal the single `max_agents: 1` slot prematurely.
        let sched_list =
            world.client_call("schedule.list", json!({ "project_id": pid }), Some(&pid))?;
        let sched_items = sched_list["schedules"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        ensure!(
            sched_items.len() >= 3,
            "Expected 3 cron schedules registered from YAML, got {sched_list}"
        );
        for s in &sched_items {
            if let Some(sid) = s["id"].as_str() {
                let _ = world.client_call(
                    "schedule.toggle",
                    json!({ "schedule_id": sid, "enabled": false }),
                    Some(&pid),
                )?;
            }
        }

        // 1. Stage-priority & FIFO queueing under `max_agents: 1`:
        //    - c1 (backlog) occupies the sole slot and stays `working`.
        //    - c2 (backlog) queues first, then c3 (review, later stage!) queues second.
        //    - When c1 finishes, later-stage c3 MUST launch before earlier-stage c2!
        let c1 = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": "Slot 1 Backlog" }),
            Some(&pid),
        )?;
        let c2 = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": "Queued Backlog FIFO" }),
            Some(&pid),
        )?;
        let c3 = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": "Queued Review Priority" }),
            Some(&pid),
        )?;
        let cid1 = c1["id"].as_str().unwrap().to_owned();
        let cid2 = c2["id"].as_str().unwrap().to_owned();
        let cid3 = c3["id"].as_str().unwrap().to_owned();
        let rev3 = c3["revision"].as_u64().unwrap();
        let raw_c1 = canban_types::split_qualified_host_id(&cid1)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| cid1.clone());
        let raw_c2 = canban_types::split_qualified_host_id(&cid2)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| cid2.clone());
        let raw_c3 = canban_types::split_qualified_host_id(&cid3)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| cid3.clone());

        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "sched_budget",
                "card_id": cid1,
                "entry": "start_backlog",
                "request_key": format!("sb-c1-{seed}"),
            }),
            Some(&pid),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == raw_c1 && a.terminal.is_some())
            }))
        })?;
        let (tid1, addr1) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_c1))
                .unwrap();
            (
                t.id.clone(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
            )
        };
        let herdr = world.herdr("remote-a").unwrap();
        herdr.set_status(&addr1, "working", Some(1));
        world.advance(Duration::from_millis(200))?;

        // Queue c2 (backlog) first, then move c3 to `review` (triggers `enter_review`).
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "sched_budget",
                "card_id": cid2,
                "entry": "start_backlog",
                "request_key": format!("sb-c2-{seed}"),
            }),
            Some(&pid),
        )?;
        let _ = world.client_call(
            "card.move",
            json!({ "card_id": cid3, "revision": rev3, "stage": "review" }),
            Some(&pid),
        )?;
        world.advance(Duration::from_millis(500))?;
        oracle.check_step_invariants(world)?;

        // Complete c1 & finish its terminal -> c3 (review) MUST acquire the slot ahead of c2!
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": tid1,
                "state": "completed",
                "message": "c1 done",
                "request_key": format!("sb-c1-done-{seed}"),
            }),
        )?;
        herdr.finish_terminal(&addr1, 0, "exited");
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == raw_c3 && a.terminal.is_some())
            }))
        })?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let c2_started = snap.terminals.iter().any(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == raw_c2 && a.terminal.is_some())
            });
            ensure!(
                !c2_started,
                "Stage priority violated: backlog card c2 launched alongside or before review card c3 under max_agents=1!"
            );
        }
        world.hit("schedules_budget.stage_priority_queue");

        // Complete c3 -> c2 now launches cleanly!
        let (tid3, addr3) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_c3))
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
                "terminal_id": tid3,
                "state": "completed",
                "message": "c3 done",
                "request_key": format!("sb-c3-done-{seed}"),
            }),
        )?;
        herdr.finish_terminal(&addr3, 0, "exited");
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == raw_c2 && a.terminal.is_some())
            }))
        })?;
        let (tid2, addr2) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_c2))
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
                "terminal_id": tid2,
                "state": "completed",
                "message": "c2 done",
                "request_key": format!("sb-c2-done-{seed}"),
            }),
        )?;
        herdr.finish_terminal(&addr2, 0, "exited");
        world.wait_until(5_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .terminals
                .iter()
                .all(|t| t.agent.as_ref().is_none_or(|a| a.finished())))
        })?;

        // 2. Herdr offline with queued terminal (explicitly guards #29 scheduler livelock!).
        herdr.set_offline(true);
        let c4 = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": "Queued While Herdr Offline" }),
            Some(&pid),
        )?;
        let cid4 = c4["id"].as_str().unwrap().to_owned();
        let raw_c4 = canban_types::split_qualified_host_id(&cid4)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| cid4.clone());
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "sched_budget",
                "card_id": cid4,
                "entry": "start_backlog",
                "request_key": format!("sb-c4-offline-{seed}"),
            }),
            Some(&pid),
        )?;
        // Advance virtual time across multiple scheduler ticks while Herdr is offline — must NOT
        // spin, hang, or violate invariants!
        for _ in 0..5 {
            world.advance(Duration::from_secs(2))?;
            oracle.check_step_invariants(world)?;
        }
        // Bring Herdr back online -> queued c4 launches promptly!
        herdr.set_offline(false);
        world.wait_until(15_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == raw_c4 && a.terminal.is_some())
            }))
        })?;
        let (tid4, addr4) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_c4))
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
                "terminal_id": tid4,
                "state": "completed",
                "message": "c4 recovered after herdr online",
                "request_key": format!("sb-c4-done-{seed}"),
            }),
        )?;
        herdr.finish_terminal(&addr4, 0, "exited");
        world.hit("schedules_budget.herdr_offline_queued_no_livelock");

        // 3. Cron schedule ticks (`overlap: skip` and `overlap: queue`) + `bounce_server` recovery!
        let _ = world.client_call(
            "scheduler.configure",
            json!({ "max_agents": 4 }),
            Some(&pid),
        )?;
        for s in &sched_items {
            if let Some(sid) = s["id"].as_str() {
                let _ = world.client_call(
                    "schedule.toggle",
                    json!({ "schedule_id": sid, "enabled": true }),
                    Some(&pid),
                )?;
            }
        }
        // Manually trigger `cron_skip` and `cron_queue` so active runs exist when the next cron
        // minute tick fires under virtual time!
        let _ = world.client_call(
            "schedule.trigger",
            json!({
                "project_id": pid,
                "workflow_id": "sched_budget",
                "rule_id": "cron_skip",
            }),
            Some(&pid),
        )?;
        let _ = world.client_call(
            "schedule.trigger",
            json!({
                "project_id": pid,
                "workflow_id": "sched_budget",
                "rule_id": "cron_queue",
            }),
            Some(&pid),
        )?;
        world.advance(Duration::from_secs(2))?;

        // Advance virtual time across two 1-minute cron boundaries (in 5s chunks to let background
        // schedule evaluator fire cleanly on virtual clock ticks!).
        for _ in 0..26 {
            world.advance(Duration::from_secs(5))?;
            oracle.check_step_invariants(world)?;
        }

        let snap_after_cron = world.server("remote-a").unwrap().snapshot()?;
        ensure!(
            snap_after_cron
                .cards
                .iter()
                .any(|c| c.title.starts_with("Cron Card #")),
            "Virtual-clock cron tick did not create Cron Card!"
        );
        ensure!(
            snap_after_cron
                .scheduled_runs
                .iter()
                .any(|r| r.state == "skipped"),
            "Expected overlap: skip to record a skipped ScheduledRun during active run; got {:?}",
            snap_after_cron
                .scheduled_runs
                .iter()
                .map(|r| (&r.rule_id, &r.state))
                .collect::<Vec<_>>()
        );
        let sched_list_after =
            world.client_call("schedule.list", json!({ "project_id": pid }), Some(&pid))?;
        ensure!(
            sched_list_after["schedules"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|s| {
                    s["rule_id"] == "cron_queue" && s["queued_triggers"].as_u64().unwrap_or(0) >= 1
                }),
            "Expected overlap: queue to set queued_triggers >= 1 while first run is active; schedules={sched_list_after}"
        );
        // Complete the active `cron_skip` run with a Markdown report body -> verify
        // `schedule.read_artifact` and automatic cardless PTY reclamation (`runtime.state == "finished"`)!
        let first_skip_run_id = snap_after_cron
            .scheduled_runs
            .iter()
            .find(|r| r.rule_id == "cron_skip" && r.state != "completed" && r.state != "skipped")
            .context("Expected active cron_skip run")?
            .id
            .clone();
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "schedule_run_id": first_skip_run_id,
                "state": "completed",
                "message": "0 vulnerabilities found",
                "body": "# Audit Report\nClean dependency tree.",
                "request_key": format!("sb-skip-report-{seed}"),
            }),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let run_done = snap.scheduled_runs.iter().any(|r| {
                r.id == first_skip_run_id && r.state == "completed" && r.report_path.is_some()
            });
            let reclaimed = snap.terminals.iter().any(|t| {
                t.id == first_skip_run_id
                    && t.runtime.as_ref().is_some_and(|rt| rt.state == "finished")
            });
            Ok(run_done && reclaimed)
        })?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let fin_run = snap
                .scheduled_runs
                .iter()
                .find(|r| r.id == first_skip_run_id)
                .unwrap();
            let rep_path = fin_run
                .report_path
                .as_deref()
                .context("Finished cardless run missing report_path")?;
            let artifact = world.server_call(
                "remote-a",
                "schedule.read_artifact",
                json!({
                    "schedule_run_id": first_skip_run_id,
                    "path": rep_path,
                }),
            )?;
            ensure!(
                artifact["body"]
                    .as_str()
                    .is_some_and(|b| b.contains("Clean dependency tree.")),
                "schedule.read_artifact mismatch: {artifact}"
            );
        }

        // Complete the active `cron_queue` run -> queued trigger must immediately spawn run #2!
        let first_q_run_id = snap_after_cron
            .scheduled_runs
            .iter()
            .find(|r| r.rule_id == "cron_queue" && r.state != "completed")
            .expect("Expected active cron_queue run")
            .id
            .clone();
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "schedule_run_id": first_q_run_id,
                "state": "completed",
                "message": "First queued cron run finished",
                "request_key": format!("sb-q1-done-{seed}"),
            }),
        )?;
        world.wait_until(15_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let q_runs = snap
                .scheduled_runs
                .iter()
                .filter(|r| r.rule_id == "cron_queue")
                .count();
            Ok(q_runs >= 2)
        })?;
        world.hit("schedules_budget.cron_ticks_and_overlap");

        // Disable schedules before bounce & quiescence so no new cron runs fire mid-drain, then
        // crash & bounce `remote-a` and verify schedules & run ledger survive durably!
        for s in &sched_items {
            if let Some(sid) = s["id"].as_str() {
                let _ = world.client_call(
                    "schedule.toggle",
                    json!({ "schedule_id": sid, "enabled": false }),
                    Some(&pid),
                )?;
            }
        }
        world.crash_server("remote-a")?;
        world.advance(Duration::from_millis(250))?;
        world.bounce_server("remote-a")?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        let post_bounce_snap = world.server("remote-a").unwrap().snapshot()?;
        ensure!(
            !post_bounce_snap.scheduled_runs.is_empty(),
            "ScheduledRun ledger vanished after server bounce!"
        );
        world.hit("schedules_budget.bounce_recovery");

        // ---------------------------------------------------------------------
        // 4. Global `schedule.{create,list,toggle,delete}` validation + Missed-Tick
        //    Grace Window (`SCHEDULE_LATE_GRACE_MS = 120s`: on-time vs large clock-jump sleep)
        // ---------------------------------------------------------------------
        // Global schedules cannot create Board Cards.
        let global_card_err = world
            .server_call(
                "remote-a",
                "schedule.create",
                json!({
                    "scope": "global",
                    "name": "Invalid Global Card",
                    "cron": "0 3 * * *",
                    "mode": "card",
                    "card_column": "develop",
                    "request_key": format!("sb-gcard-err-{seed}"),
                }),
            )
            .expect_err("Global schedule with mode=card must fail");
        ensure!(
            global_card_err
                .to_string()
                .contains("Global schedules cannot create Board Cards"),
            "Unexpected global card schedule error: {global_card_err:#}"
        );
        let g_created = world.server_call(
            "remote-a",
            "schedule.create",
            json!({
                "scope": "global",
                "name": "Worktree GC",
                "cron": "0 3 * * *",
                "timezone": "Local",
                "overlap": "skip",
                "mode": "script_agent",
                "script": "echo clean",
                "profile_id": "developer",
                "instruction": "Ask the user when dirty worktrees remain.",
                "request_key": format!("sb-gcreate-{seed}"),
            }),
        )?;
        let g_sched: ScheduleState = serde_json::from_value(g_created["schedule"].clone())?;
        ensure!(
            g_sched.project_id == GLOBAL_PROJECT_ID
                && g_sched.starts_with_script
                && g_sched.enabled,
            "Unexpected global schedule state: {g_sched:?}"
        );
        let g_list =
            world.server_call("remote-a", "schedule.list", json!({ "scope": "global" }))?;
        ensure!(
            g_list["schedules"].as_array().is_some_and(|a| a.len() == 1),
            "Expected 1 global schedule, got {g_list}"
        );
        let g_toggled = world.server_call(
            "remote-a",
            "schedule.toggle",
            json!({
                "schedule_id": g_sched.id,
                "enabled": false,
                "request_key": format!("sb-gtog-{seed}"),
            }),
        )?;
        ensure!(g_toggled["enabled"] == json!(false));
        let g_wf_src = world.server_call(
            "remote-a",
            "workflow.sources",
            json!({ "project_id": GLOBAL_PROJECT_ID }),
        )?;
        let g_col_err = world
            .server_call(
                "remote-a",
                "workflow.apply",
                json!({
                    "project_id": GLOBAL_PROJECT_ID,
                    "revision": g_wf_src["revision"],
                    "sources": [{
                        "filename": "bad.yaml",
                        "source": "schema: 12\nid: bad\nname: Bad Global\nscope: global\ncolumns:\n  - id: backlog\n    name: Backlog\non:\n  sched:\n    cron: '0 3 * * *'\n    start_at: done\nsteps:\n  - id: done\n    uses: complete\n",
                    }],
                }),
            )
            .expect_err("Global workflow declaring columns must fail");
        ensure!(
            format!("{g_col_err:#}").contains("Global workflows cannot declare Board `columns`"),
            "Unexpected global workflow columns error: {g_col_err:#}"
        );
        let g_del = world.server_call(
            "remote-a",
            "schedule.delete",
            json!({ "schedule_id": g_sched.id, "request_key": format!("sb-gdel-{seed}") }),
        )?;
        ensure!(
            g_del["deleted"] == json!(true)
                && world
                    .server_call("remote-a", "schedule.list", json!({ "scope": "global" }))?
                    ["schedules"]
                    .as_array()
                    .is_some_and(Vec::is_empty),
            "schedule.delete failed to remove global schedule"
        );

        // Missed-tick grace window (`SCHEDULE_LATE_GRACE_MS = 120s`) via virtual clock jump across
        // simulated machine sleep (`crash_server` -> clock advance -> `bounce_server`)!
        let grace_created = world.server_call(
            "remote-a",
            "schedule.create",
            json!({
                "project_id": canban_types::unqualified_entity_id(&pid),
                "name": "Grace Window Probe",
                "cron": "*/10 * * * *",
                "timezone": "UTC",
                "overlap": "skip",
                "mode": "agent",
                "profile_id": "developer",
                "instruction": "Grace window test",
                "request_key": format!("sb-grace-create-{seed}"),
            }),
        )?;
        let grace_sched: ScheduleState = serde_json::from_value(grace_created["schedule"].clone())?;
        let due_1 = grace_sched
            .next_run_at
            .context("Grace schedule missing initial next_run_at")?;
        // Case A: Jump live server wall-clock to `due_1 + 15s` (WITHIN 120s grace window, e.g., brief
        // system pause/wakeup) -> fires run #1 cleanly!
        let now_a = world.server("remote-a").unwrap().now_ms();
        let jump_a = (due_1 + 15_000).saturating_sub(now_a) as i64;
        if jump_a > 0 {
            world.servers["remote-a"].clock.jump_ms(jump_a);
        }
        // Wake supervisor loop cleanly after wall-clock jump.
        let _ = world.server_call(
            "remote-a",
            "scheduler.configure",
            json!({ "max_agents": 4 }),
        )?;
        world.wait_until(15_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .schedule_states
                .iter()
                .any(|s| s.id == grace_sched.id && s.run_count == 1))
        })?;
        // Complete run #1 cleanly so schedule is idle before testing stale missed tick.
        let grace_run_1 = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .scheduled_runs
            .into_iter()
            .find(|r| r.schedule_id == grace_sched.id)
            .context("Grace run #1 missing")?
            .id;
        let _ = world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "schedule_run_id": grace_run_1,
                "state": "completed",
                "message": "Grace run 1 done",
                "request_key": format!("sb-grace-r1-{seed}"),
            }),
        )?;
        world.advance(Duration::from_secs(2))?;

        // Case B: Jump live server wall-clock to `due_2 + 240s` (4 min > 120s grace window, simulating
        // laptop sleep across a missed cron boundary) -> advances `next_run_at` WITHOUT firing a stale run!
        let due_2 = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .schedule_states
            .into_iter()
            .find(|s| s.id == grace_sched.id)
            .and_then(|s| s.next_run_at)
            .context("Grace schedule missing due_2")?;
        let now_b = world.server("remote-a").unwrap().now_ms();
        let jump_b = (due_2 + 240_000).saturating_sub(now_b) as i64;
        if jump_b > 0 {
            world.servers["remote-a"].clock.jump_ms(jump_b);
        }
        let _ = world.server_call(
            "remote-a",
            "scheduler.configure",
            json!({ "max_agents": 4 }),
        )?;
        world.wait_until(15_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let now = w.server("remote-a").unwrap().now_ms();
            Ok(snap
                .schedule_states
                .iter()
                .any(|s| s.id == grace_sched.id && s.next_run_at.is_some_and(|t| t > now)))
        })?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let st = snap
                .schedule_states
                .iter()
                .find(|s| s.id == grace_sched.id)
                .unwrap();
            ensure!(
                st.run_count == 1,
                "Stale missed tick past 120s grace window MUST NOT fire! state={st:?}"
            );
        }
        let _ = world.server_call(
            "remote-a",
            "schedule.delete",
            json!({ "schedule_id": grace_sched.id, "request_key": format!("sb-grace-del-{seed}") }),
        )?;
        world.hit("schedules_budget.crud_and_artifact_read");

        // ---------------------------------------------------------------------
        // 5. Backlog `plans_ahead_k`, `report(state="backlog")`, Consecutive Cardless Agent
        //    Terminal Clear, Column & Running-Step Removal, Multi-Rule Event Queue & Rollback,
        //    & Auto-Commit + `workflow.deliver` After Pause / Stop
        // ---------------------------------------------------------------------
        let finish_all_remote_ptys = |w: &mut crate::world::SimWorld| -> Result<()> {
            let h = w.herdr("remote-a").unwrap();
            for addr in h.terminal_addresses() {
                if h.terminal_record(&addr)
                    .is_some_and(|r| r["state"].as_str() == Some("live"))
                {
                    h.finish_terminal(&addr, 0, "exited");
                }
            }
            w.advance(Duration::from_millis(400))?;
            Ok(())
        };
        finish_all_remote_ptys(world)?;

        let edge_dir = world.project_dir(&format!("wf-edges-{seed}"))?;
        let proj_edge = world.client_call(
            "project.create",
            json!({ "host_id": "remote-a", "name": format!("WF Edges {seed}"), "directory": edge_dir }),
            None,
        )?;
        let pid_edge = proj_edge["id"].as_str().unwrap().to_owned();
        let raw_pid_edge = canban_types::unqualified_entity_id(&pid_edge).to_owned();

        // 5A. Backlog `plans_ahead_k` admission & board-order priority under `max_agents: 1 -> 2`!
        let _ = world.server_call(
            "remote-a",
            "scheduler.configure",
            json!({ "max_agents": 1 }),
        )?;
        {
            let col = world.server_call(
                "remote-a",
                "workflow.sources",
                json!({ "project_id": raw_pid_edge }),
            )?;
            let admit_yaml = format!(
                r#"schema: 12
id: delivery:{raw_pid_edge}
name: Development
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  backlog:
    event: card.column_entered
    start_at: admit:plan
    match:
      column: backlog
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
steps:
  - id: admit:plan
    name: Plan-ahead limit
    uses: admit
    with:
      agent_slot: false
      max_ready: 1
    next: agent:plan
  - id: agent:plan
    name: Analyze task
    uses: agent
    with:
      profile: developer
    next: admit:develop
  - id: admit:develop
    name: Wait for free Agent
    uses: admit
    next: move:backlog
  - id: move:backlog
    name: Move to Develop
    uses: card.move
    with:
      column: develop
    next: agent:develop
  - id: agent:develop
    name: Implement
    uses: agent
    with:
      profile: developer
    next: move:develop
  - id: move:develop
    name: Move to Review
    uses: card.move
    with:
      column: review
    next: __end
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#
            );
            let _ = world.server_call(
                "remote-a",
                "workflow.apply",
                json!({
                    "project_id": raw_pid_edge,
                    "revision": col["revision"],
                    "sources": [{ "filename": "development.yaml", "source": admit_yaml }],
                }),
            )?;
        }
        let bc1 = world.server_call(
            "remote-a",
            "card.create",
            json!({ "view": "full", "project_id": raw_pid_edge, "title": "First", "body": "task" }),
        )?;
        let bc2 = world.server_call(
            "remote-a",
            "card.create",
            json!({ "view": "full", "project_id": raw_pid_edge, "title": "Second", "body": "task" }),
        )?;
        let bc3 = world.server_call(
            "remote-a",
            "card.create",
            json!({ "view": "full", "project_id": raw_pid_edge, "title": "Third", "body": "task" }),
        )?;
        let bc1_id = bc1["id"].as_str().unwrap().to_owned();
        let bc2_id = bc2["id"].as_str().unwrap().to_owned();
        let bc3_id = bc3["id"].as_str().unwrap().to_owned();
        // Drag Third above Second in backlog priority order!
        let _ = world.server_call(
            "remote-a",
            "card.move",
            json!({
                "view": "full",
                "card_id": bc3_id,
                "revision": bc3["revision"],
                "stage": "backlog",
                "before_card_id": bc2_id,
            }),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == bc1_id
                        && a.state == "running"
                        && a.terminal.is_some()
                        && a.workflow_step
                            .as_deref()
                            .is_some_and(|s| s.starts_with("agent:plan:"))
                })
            }))
        })?;
        let (bc1_tid, bc1_addr) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            for cid in [&bc2_id, &bc3_id] {
                let r = snap
                    .workflow_runs
                    .iter()
                    .find(|r| r.card_id.as_deref() == Some(cid.as_str()) && r.active())
                    .context("Missing waiting_capacity run for bc2/bc3")?;
                ensure!(
                    r.state == "waiting_capacity" && r.step.as_deref() == Some("admit:plan"),
                    "Expected K=1 waiting_capacity at admit:plan, got {r:?}"
                );
            }
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == bc1_id))
                .unwrap();
            (
                t.id.clone(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
            )
        };
        // Keep `bc1` occupying the sole slot (`working`) when reporting plan completed ->
        // `bc1` waits at `admit:develop` while `bc3` stays at `admit:plan`!
        let herdr = world.herdr("remote-a").unwrap();
        herdr.set_status(&bc1_addr, "working", Some(1));
        world.advance(Duration::from_millis(200))?;
        let _ = world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": bc1_tid,
                "state": "completed",
                "message": "Plan 1 ready",
                "request_key": format!("sb-plan1-{seed}"),
            }),
        )?;
        world.advance(Duration::from_millis(400))?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let r1 = snap
                .workflow_runs
                .iter()
                .find(|r| r.card_id.as_deref() == Some(bc1_id.as_str()) && r.active())
                .context("bc1 run missing")?;
            let r3 = snap
                .workflow_runs
                .iter()
                .find(|r| r.card_id.as_deref() == Some(bc3_id.as_str()) && r.active())
                .context("bc3 run missing")?;
            ensure!(
                r1.state == "waiting_capacity"
                    && r1.step.as_deref() == Some("admit:develop")
                    && snap.cards.iter().find(|c| c.id == bc1_id).unwrap().stage == "backlog"
                    && r3.step.as_deref() == Some("admit:plan"),
                "Plan-ahead K=1 window violated: r1={r1:?} r3={r3:?}"
            );
        }
        // Increase `max_agents: 2` -> First enters `develop` AND Third (above Second!) begins planning!
        let _ = world.server_call(
            "remote-a",
            "scheduler.configure",
            json!({ "max_agents": 2 }),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let c1_dev = snap
                .cards
                .iter()
                .any(|c| c.id == bc1_id && c.stage == "develop");
            let c3_plan = snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == bc3_id
                        && a.workflow_step
                            .as_deref()
                            .is_some_and(|s| s.starts_with("agent:plan:"))
                })
            });
            Ok(c1_dev && c3_plan)
        })?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let r2 = snap
                .workflow_runs
                .iter()
                .find(|r| r.card_id.as_deref() == Some(bc2_id.as_str()) && r.active())
                .context("bc2 run missing")?;
            ensure!(
                r2.state == "waiting_capacity" && r2.step.as_deref() == Some("admit:plan"),
                "Second must still wait behind Third in board order: {r2:?}"
            );
        }
        // Cleanly stop active runs in `pid_edge` and bump `max_agents: 4`.
        for r in world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .workflow_runs
            .into_iter()
            .filter(|r| r.project_id == raw_pid_edge && r.active())
        {
            let _ = world.server_call("remote-a", "workflow.stop", json!({ "run_id": r.id }));
        }
        finish_all_remote_ptys(world)?;
        let _ = world.server_call(
            "remote-a",
            "scheduler.configure",
            json!({ "max_agents": 4 }),
        )?;

        // 5B. `report(state="backlog")`: stops workflow, returns card to backlog WITHOUT
        //     retriggering `backlog_analyze`, logs events, and emits unread reminder!
        let backlog_ret_yaml = format!(
            r#"schema: 12
id: delivery:{raw_pid_edge}
name: Development
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  backlog_analyze:
    event: card.column_entered
    start_at: agent:plan
    match:
      column: backlog
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
steps:
  - id: agent:plan
    uses: agent
    with:
      profile: planner
    next: __end
  - id: agent:develop
    uses: agent
    with:
      profile: developer
    next: move:review
  - id: move:review
    uses: card.move
    with:
      column: review
    next: __end
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#
        );
        {
            let col = world.server_call(
                "remote-a",
                "workflow.sources",
                json!({ "project_id": raw_pid_edge }),
            )?;
            let _ = world.server_call(
                "remote-a",
                "workflow.apply",
                json!({
                    "project_id": raw_pid_edge,
                    "revision": col["revision"],
                    "sources": [{ "filename": "development.yaml", "source": backlog_ret_yaml }],
                }),
            )?;
        }
        let ret_card = world.server_call(
            "remote-a",
            "card.create",
            json!({ "view": "full", "project_id": raw_pid_edge, "title": "Legacy cleanup", "body": "Remove v0 shim" }),
        )?;
        let ret_cid = ret_card["id"].as_str().unwrap().to_owned();
        let _ = world.server_call(
            "remote-a",
            "card.move",
            json!({ "view": "full", "card_id": ret_cid, "revision": ret_card["revision"], "stage": "develop" }),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == ret_cid && a.stage == "develop" && a.state == "running"
                })
            }))
        })?;
        let ret_tid = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == ret_cid && a.stage == "develop")
            })
            .unwrap()
            .id;
        let ret_rep = world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": ret_tid,
                "state": "backlog",
                "message": "Already removed in PR #42",
                "body": "Verified `src/v0_shim.rs` no longer exists.",
                "request_key": format!("sb-ret-backlog-{seed}"),
            }),
        )?;
        ensure!(ret_rep["recorded_state"] == "backlog");
        world.advance(Duration::from_millis(300))?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let updated = snap.cards.iter().find(|c| c.id == ret_cid).unwrap();
            ensure!(
                updated.stage == "backlog"
                    && updated.human_gate.is_none()
                    && updated.attention.is_none()
                    && !snap
                        .workflow_runs
                        .iter()
                        .any(|r| r.card_id.as_deref() == Some(ret_cid.as_str()) && r.active())
                    && !snap
                        .terminals
                        .iter()
                        .filter_map(|t| t.agent.as_ref())
                        .any(|a| {
                            a.card_id == ret_cid && (a.state == "queued" || a.pending.is_some())
                        }),
                "report(state=backlog) must stop workflow and not retrigger backlog_analyze"
            );
            let tl = world.server_call(
                "remote-a",
                "timeline.list",
                json!({ "card_id": ret_cid, "limit": 30, "view": "full" }),
            )?;
            let evs = tl["events"].as_array().context("ret_cid events")?;
            ensure!(
                evs.iter().any(|e| {
                    e["kind"] == "execution.reported"
                        && e["data"]["state"] == "backlog"
                        && e["message"] == "Returned to Backlog: Already removed in PR #42"
                }) && evs.iter().any(|e| {
                    e["kind"] == "card.stage_changed"
                        && e["data"]["from"] == "develop"
                        && e["data"]["to"] == "backlog"
                        && e["data"]["reason"] == "agent_returned_backlog"
                }),
                "report(state=backlog) missing expected timeline events: {tl}"
            );
            ensure!(
                canban_notifications::reminders(&snap)
                    .iter()
                    .any(|r| r.card_id == ret_cid && r.unread),
                "report(state=backlog) must surface unread reminder"
            );
        }
        finish_all_remote_ptys(world)?;

        // 5C. Consecutive cardless agent steps (`triage` -> `respond` -> `done`) clear
        //     `inputs["agent_terminal"]` cleanly between steps (both on completion AND on failure)!
        let cardless_multi_yaml = r#"schema: 12
id: multi_agent_cardless
name: Multi-Agent Cardless
on:
  comment:
    event: comment.created
    start_at: triage
    cardless: true
steps:
  - id: triage
    uses: agent
    with:
      profile: reviewer
      instruction: Triage incoming comment
    next: respond
  - id: respond
    uses: agent
    with:
      profile: developer
      instruction: Draft response
    next: done
  - id: done
    uses: complete
"#;
        {
            let col = world.server_call(
                "remote-a",
                "workflow.sources",
                json!({ "project_id": raw_pid_edge }),
            )?;
            let mut srcs: Vec<Value> = col["sources"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| json!({ "filename": s["filename"], "source": s["source"] }))
                .collect();
            srcs.push(
                json!({ "filename": "multi_agent_cardless.yaml", "source": cardless_multi_yaml }),
            );
            let _ = world.server_call(
                "remote-a",
                "workflow.apply",
                json!({ "project_id": raw_pid_edge, "revision": col["revision"], "sources": srcs }),
            )?;
        }
        // Run 1: triage (exit 0) -> respond (exit 0) -> completed!
        let _ = world.server_call(
            "remote-a",
            "comment.add",
            json!({ "card_id": ret_cid, "body": "Trigger cardless run 1", "mentions": [] }),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.is_cardless()
                    && r.workflow_id == "multi_agent_cardless"
                    && r.step.as_deref() == Some("triage")
                    && r.inputs["agent_terminal"]["address"].is_string()
            }))
        })?;
        let (cl_run1_id, cl_addr_1) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let r = snap
                .workflow_runs
                .iter()
                .find(|r| r.is_cardless() && r.workflow_id == "multi_agent_cardless" && r.active())
                .unwrap();
            (
                r.id.clone(),
                r.inputs["agent_terminal"]["address"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        };
        herdr.finish_terminal(&cl_addr_1, 0, "exited");
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.id == cl_run1_id
                    && r.step.as_deref() == Some("respond")
                    && r.inputs["agent_terminal"]["address"]
                        .as_str()
                        .is_some_and(|a| a != cl_addr_1)
            }))
        })?;
        let cl_addr_2 = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .workflow_runs
            .into_iter()
            .find(|r| r.id == cl_run1_id)
            .unwrap()
            .inputs["agent_terminal"]["address"]
            .as_str()
            .unwrap()
            .to_owned();
        ensure!(
            cl_addr_2 != cl_addr_1,
            "Step 2 must launch a fresh PTY after agent_terminal cleared"
        );
        herdr.finish_terminal(&cl_addr_2, 0, "exited");
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.id == cl_run1_id
                    && r.state == "completed"
                    && r.inputs.get("agent_terminal").is_none()
            }))
        })?;

        // Run 2: triage (exit 0) -> respond (exit 1) -> failed with `agent_terminal` & `pending_agent` cleared!
        let _ = world.server_call(
            "remote-a",
            "comment.add",
            json!({ "card_id": ret_cid, "body": "Trigger cardless run 2", "mentions": [] }),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.id != cl_run1_id
                    && r.is_cardless()
                    && r.workflow_id == "multi_agent_cardless"
                    && r.step.as_deref() == Some("triage")
                    && r.inputs["agent_terminal"]["address"].is_string()
            }))
        })?;
        let (cl_run2_id, cl2_addr_1) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let r = snap
                .workflow_runs
                .iter()
                .find(|r| {
                    r.id != cl_run1_id
                        && r.is_cardless()
                        && r.workflow_id == "multi_agent_cardless"
                        && r.active()
                })
                .unwrap();
            (
                r.id.clone(),
                r.inputs["agent_terminal"]["address"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        };
        herdr.finish_terminal(&cl2_addr_1, 0, "exited");
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.id == cl_run2_id
                    && r.step.as_deref() == Some("respond")
                    && r.inputs["agent_terminal"]["address"]
                        .as_str()
                        .is_some_and(|a| a != cl2_addr_1)
            }))
        })?;
        let cl2_addr_2 = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .workflow_runs
            .into_iter()
            .find(|r| r.id == cl_run2_id)
            .unwrap()
            .inputs["agent_terminal"]["address"]
            .as_str()
            .unwrap()
            .to_owned();
        herdr.finish_terminal(&cl2_addr_2, 1, "exited");
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.id == cl_run2_id
                    && r.state == "failed"
                    && r.inputs.get("agent_terminal").is_none()
                    && r.inputs.get("pending_agent").is_none()
            }))
        })?;

        // 5D. Column removal, Running-step removal, YAML validation/conflict/StorageFault rollback,
        //     Multi-rule event queue capturing rev 1, Condition->Notify & canonical single move event,
        //     & Event workflow non-rollback on busy card!
        let clean_dev_yaml = format!(
            r#"schema: 12
id: delivery:{raw_pid_edge}
name: Development
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
  review:
    event: card.column_entered
    start_at: agent:review
    match:
      column: review
steps:
  - id: agent:develop
    uses: agent
    with:
      profile: developer
      instruction: Develop
    next: move:develop
  - id: move:develop
    uses: card.move
    with:
      column: review
    next: agent:review
  - id: agent:review
    uses: agent
    with:
      profile: reviewer
      instruction: Review
    next: __end
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#
        );
        {
            let col = world.server_call(
                "remote-a",
                "workflow.sources",
                json!({ "project_id": raw_pid_edge }),
            )?;
            let _ = world.server_call(
                "remote-a",
                "workflow.apply",
                json!({
                    "project_id": raw_pid_edge,
                    "revision": col["revision"],
                    "sources": [{ "filename": "development.yaml", "source": clean_dev_yaml }],
                }),
            )?;
        }

        // Edited default YAML (`agent:review -> __end` without approval gate): `require_human_review == false`,
        // completing leaves `human_gate.is_none()`!
        let rev_card = world.server_call(
            "remote-a",
            "card.create",
            json!({ "view": "full", "project_id": raw_pid_edge, "title": "Edited Default Review" }),
        )?;
        let rev_cid = rev_card["id"].as_str().unwrap().to_owned();
        let _ = world.server_call(
            "remote-a",
            "card.move",
            json!({ "view": "full", "card_id": rev_cid, "revision": rev_card["revision"], "stage": "review" }),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == rev_cid && a.state == "running")
            }))
        })?;
        let rev_tid = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == rev_cid))
                .unwrap();
            ensure!(!t.agent.as_ref().unwrap().require_human_review);
            t.id.clone()
        };
        let _ = world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": rev_tid,
                "state": "completed",
                "message": "Review passed",
                "request_key": format!("sb-edit-def-{seed}"),
            }),
        )?;
        ensure!(
            world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .cards
                .iter()
                .find(|c| c.id == rev_cid)
                .unwrap()
                .human_gate
                .is_none()
        );
        finish_all_remote_ptys(world)?;

        // Running step removal: move card to `develop` -> remove `agent:develop` step ->
        // `stop_after_step == "agent:develop"` -> complete -> `state == "cancelled"`, stays in `develop`!
        let step_rm_card = world.server_call(
            "remote-a",
            "card.create",
            json!({ "view": "full", "project_id": raw_pid_edge, "title": "Running Step Removal" }),
        )?;
        let step_rm_cid = step_rm_card["id"].as_str().unwrap().to_owned();
        let _ = world.server_call(
            "remote-a",
            "card.move",
            json!({ "view": "full", "card_id": step_rm_cid, "revision": step_rm_card["revision"], "stage": "develop" }),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == step_rm_cid && a.state == "running")
            }))
        })?;
        let step_rm_tid = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == step_rm_cid))
            .unwrap()
            .id;
        {
            let col = world.server_call(
                "remote-a",
                "workflow.sources",
                json!({ "project_id": raw_pid_edge }),
            )?;
            let step_removed_yaml = format!(
                r#"schema: 12
id: delivery:{raw_pid_edge}
name: Development
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: move:develop
    match:
      column: develop
  review:
    event: card.column_entered
    start_at: agent:review
    match:
      column: review
steps:
  - id: move:develop
    uses: card.move
    with:
      column: review
    next: agent:review
  - id: agent:review
    uses: agent
    with:
      profile: reviewer
      instruction: Review
    next: __end
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#
            );
            let _ = world.server_call(
                "remote-a",
                "workflow.apply",
                json!({
                    "project_id": raw_pid_edge,
                    "revision": col["revision"],
                    "sources": [{ "filename": "development.yaml", "source": step_removed_yaml }],
                }),
            )?;
        }
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let run = snap
                .workflow_runs
                .iter()
                .find(|r| r.card_id.as_deref() == Some(step_rm_cid.as_str()))
                .context("step_rm run missing")?;
            ensure!(
                run.state == "waiting_agent" && run.inputs["stop_after_step"] == "agent:develop",
                "Expected stop_after_step=agent:develop, got {run:?}"
            );
        }
        let _ = world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": step_rm_tid,
                "state": "completed",
                "message": "Step finished before cancel",
                "request_key": format!("sb-step-rm-done-{seed}"),
            }),
        )?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let run = snap
                .workflow_runs
                .iter()
                .find(|r| r.card_id.as_deref() == Some(step_rm_cid.as_str()))
                .unwrap();
            let card = snap.cards.iter().find(|c| c.id == step_rm_cid).unwrap();
            ensure!(
                run.state == "cancelled" && card.stage == "develop",
                "Removed step must finish then cancel without moving to review: run={run:?} stage={}",
                card.stage
            );
        }
        finish_all_remote_ptys(world)?;

        // Column removal: `rev_cid` is in `review` -> remove `review` column -> returns to `backlog`!
        {
            let col = world.server_call(
                "remote-a",
                "workflow.sources",
                json!({ "project_id": raw_pid_edge }),
            )?;
            let col_removed_yaml = format!(
                r#"schema: 12
id: delivery:{raw_pid_edge}
name: Development
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
on:
  develop:
    event: card.column_entered
    start_at: __end
    match:
      column: develop
steps:
  - id: __end
    uses: complete
workspace:
  create_worktree: false
  sync_base: false
"#
            );
            let _ = world.server_call(
                "remote-a",
                "workflow.apply",
                json!({
                    "project_id": raw_pid_edge,
                    "revision": col["revision"],
                    "sources": [{ "filename": "development.yaml", "source": col_removed_yaml }],
                }),
            )?;
        }
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let card = snap.cards.iter().find(|c| c.id == rev_cid).unwrap();
            let proj = snap.projects.iter().find(|p| p.id == raw_pid_edge).unwrap();
            ensure!(
                card.stage == "backlog" && !proj.workflow.stages.iter().any(|s| s.id == "review"),
                "Removing review column must move cards to backlog and drop stage"
            );
            let tl = world.server_call(
                "remote-a",
                "timeline.list",
                json!({ "card_id": rev_cid, "limit": 30, "view": "full" }),
            )?;
            ensure!(
                tl["events"].as_array().is_some_and(|evs| {
                    evs.iter().any(|e| {
                        e["kind"] == "card.stage_changed" && e["data"]["reason"] == "column_removed"
                    })
                }),
                "Column removal missing card.stage_changed(column_removed): {tl}"
            );
        }

        // YAML bytes preservation, validation error, `StorageFaultKind::IoError` atomic rollback,
        // `revision_conflict`, multi-rule event queue capturing rev 1, condition->notify, & non-rollback!
        let raw_custom = "# Preserve this comment\nschema: 13\nid: raw_source\nname: Test YAML\non:\n  start:\n    event: manual\n    start_at: gate\nsteps:\n  - id: gate\n    uses: approval\n    with:\n      title: Inspect YAML\n";
        let col0 = world.server_call(
            "remote-a",
            "workflow.sources",
            json!({ "project_id": raw_pid_edge }),
        )?;
        let mut s0: Vec<Value> = col0["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| json!({ "filename": f["filename"], "source": f["source"] }))
            .collect();
        s0.push(json!({ "filename": "custom.yaml", "source": raw_custom }));
        let col1 = world.server_call(
            "remote-a",
            "workflow.apply",
            json!({ "project_id": raw_pid_edge, "revision": col0["revision"], "sources": s0 }),
        )?;
        ensure!(
            world.server_call(
                "remote-a",
                "workflow.get",
                json!({ "workflow_id": "raw_source", "revision": 1 }),
            )?["source_yaml"]
                == raw_custom
        );
        // Reject invalid step (`uses: unsupported`) without changing revision.
        let mut bad_s = s0.clone();
        bad_s.last_mut().unwrap()["source"] =
            json!(raw_custom.replace("uses: approval", "uses: unsupported"));
        ensure!(
            world
                .server_call(
                    "remote-a",
                    "workflow.apply",
                    json!({ "project_id": raw_pid_edge, "revision": col1["revision"], "sources": bad_s }),
                )
                .is_err()
        );
        // Inject `StorageFaultKind::IoError` during `workflow.apply` -> atomic rollback of sources,
        // revisions, and event subscriptions!
        let mut rb_s = s0.clone();
        rb_s.push(json!({
            "filename": "rollback.yaml",
            "source": "schema: 12\nid: rollback_source\nname: Rollback\non:\n  c:\n    event: card.created\n    start_at: g\nsteps:\n  - id: g\n    uses: complete\n",
        }));
        world
            .storage_faults("remote-a")
            .unwrap()
            .inject_next(StorageFaultKind::IoError);
        ensure!(
            world
                .server_call(
                    "remote-a",
                    "workflow.apply",
                    json!({ "project_id": raw_pid_edge, "revision": col1["revision"], "sources": rb_s }),
                )
                .is_err()
        );
        ensure!(
            world.server_call(
                "remote-a",
                "workflow.sources",
                json!({ "project_id": raw_pid_edge }),
            )?["revision"]
                == col1["revision"]
                && !world
                    .server("remote-a")
                    .unwrap()
                    .snapshot()?
                    .event_subscriptions
                    .iter()
                    .any(|s| s.id.contains("rollback_source")),
            "StorageFault during workflow.apply must roll back sources and subscriptions atomically"
        );
        // Valid update + stale `revision_conflict` rejection while rev 1 remains intact.
        let mut s2 = s0.clone();
        s2.last_mut().unwrap()["source"] = json!(format!("{raw_custom}# Updated rev 2\n"));
        let col2 = world.server_call(
            "remote-a",
            "workflow.apply",
            json!({ "project_id": raw_pid_edge, "revision": col1["revision"], "sources": s2 }),
        )?;
        let stale_wf_err = world
            .server_call(
                "remote-a",
                "workflow.apply",
                json!({ "project_id": raw_pid_edge, "revision": col1["revision"], "sources": s0 }),
            )
            .expect_err("Stale workflow.apply revision must reject");
        ensure!(stale_wf_err.to_string().contains("revision_conflict"));
        ensure!(
            world.server_call(
                "remote-a",
                "workflow.get",
                json!({ "workflow_id": "raw_source", "revision": 1 }),
            )?["source_yaml"]
                == raw_custom
        );

        // Multi-rule YAML event queue captures rev 1 + Condition->Notify + canonical single event!
        let multi_rule_yaml = "schema: 12\nid: multiple_rules\nname: Multi Rule\non:\n  first:\n    event: comment.created\n    start_at: gate\n  second:\n    event: comment.created\n    start_at: gate\nsteps:\n  - id: gate\n    uses: approval\n    with:\n      title: Inspect YAML\n";
        let cond_yaml = format!(
            "schema: 12\nid: cond_notify\nname: Conditional\non:\n  start:\n    event: manual\n    start_at: choose\nsteps:\n  - id: choose\n    uses: condition\n    with:\n      pointer: /card/title\n      equals: {:?}\n      then: notify\n      otherwise: done\n  - id: notify\n    uses: notify\n    with:\n      message: Condition selected\n    next: done\n  - id: done\n    uses: complete\n",
            "Multi Rule Card"
        );
        let mut s3: Vec<Value> = col2["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| json!({ "filename": f["filename"], "source": f["source"] }))
            .collect();
        s3.push(json!({ "filename": "multiple.yaml", "source": multi_rule_yaml }));
        s3.push(json!({ "filename": "cond.yaml", "source": cond_yaml }));
        let col3 = world.server_call(
            "remote-a",
            "workflow.apply",
            json!({ "project_id": raw_pid_edge, "revision": col2["revision"], "sources": s3 }),
        )?;
        let mr_card = world.server_call(
            "remote-a",
            "card.create",
            json!({ "view": "full", "project_id": raw_pid_edge, "title": "Multi Rule Card" }),
        )?;
        let mr_cid = mr_card["id"].as_str().unwrap().to_owned();
        // Condition -> Notify completes synchronously without launching an agent.
        let cond_run = world.server_call(
            "remote-a",
            "workflow.start",
            json!({ "workflow_id": "cond_notify", "card_id": mr_cid, "entry": "start" }),
        )?;
        ensure!(
            cond_run["state"] == "completed"
                && world
                    .server("remote-a")
                    .unwrap()
                    .snapshot()?
                    .notices
                    .iter()
                    .any(|n| n.message == "Condition selected"),
            "Condition->Notify workflow failed: {cond_run}"
        );
        // Canonical move event produces exactly 1 delivery for `["card.column_entered", "card.column_left"]`.
        let canon_sub = world.server_call(
            "remote-a",
            "subscription.save",
            json!({
                "revision": 0,
                "enabled": true,
                "selector": { "card_id": mr_cid, "types": ["card.column_entered", "card.column_left"] },
                "target": { "type": "consumer" },
            }),
        )?;
        let _ = world.server_call(
            "remote-a",
            "card.move",
            json!({ "view": "full", "card_id": mr_cid, "revision": mr_card["revision"], "stage": "develop" }),
        )?;
        world.advance(Duration::from_millis(300))?;
        let canon_delivs = world.server_call(
            "remote-a",
            "subscription.deliveries",
            json!({ "subscription_id": canon_sub["id"] }),
        )?;
        ensure!(
            canon_delivs.as_array().is_some_and(|a| a.len() == 1),
            "Canonical move event must produce 1 delivery: {canon_delivs}"
        );

        // Post comment triggering BOTH rules of `multiple_rules`: run 1 starts at rev 1, rule 2 stays
        // pending with `definition_revision == 1` (and comment is never rolled back!).
        let _ = world.server_call(
            "remote-a",
            "comment.add",
            json!({ "view": "full", "card_id": mr_cid, "body": "Start both rules", "mentions": [] }),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(mr_cid.as_str())
                    && r.workflow_id == "multiple_rules"
                    && r.active()
            }))
        })?;
        let mr_run1_id = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .workflow_runs
            .into_iter()
            .find(|r| {
                r.card_id.as_deref() == Some(mr_cid.as_str())
                    && r.workflow_id == "multiple_rules"
                    && r.active()
            })
            .unwrap()
            .id;
        // Update `multiple.yaml` to rev 2 (`NEW TITLE`) while rule 2 delivery is pending!
        let mut s4 = s3.clone();
        s4.iter_mut()
            .find(|f| f["filename"] == "multiple.yaml")
            .unwrap()["source"] = json!(multi_rule_yaml.replace("Inspect YAML", "NEW TITLE"));
        let _ = world.server_call(
            "remote-a",
            "workflow.apply",
            json!({ "project_id": raw_pid_edge, "revision": col3["revision"], "sources": s4 }),
        )?;
        // Stop run 1 -> advance virtual clock so queued rule 2 fires -> MUST use captured rev 1 ("Inspect YAML")!
        let _ = world.server_call("remote-a", "workflow.stop", json!({ "run_id": mr_run1_id }))?;
        world.wait_until(15_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(mr_cid.as_str())
                    && r.workflow_id == "multiple_rules"
                    && r.id != mr_run1_id
            }))
        })?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let mr_run2 = snap
                .workflow_runs
                .iter()
                .find(|r| {
                    r.card_id.as_deref() == Some(mr_cid.as_str())
                        && r.workflow_id == "multiple_rules"
                        && r.id != mr_run1_id
                })
                .unwrap();
            ensure!(
                mr_run2.definition_revision == 1
                    && matches!(
                        mr_run2.definition.steps.get("gate"),
                        Some(WorkflowStep::Approval { title, .. }) if title == "Inspect YAML"
                    ),
                "Queued multi-rule delivery must preserve captured rev 1 definition: {mr_run2:?}"
            );
            let _ = world.server_call("remote-a", "workflow.stop", json!({ "run_id": mr_run2.id }));
        }

        // Event workflow final card binding (`card.created`) & non-rollback on busy card!
        let ev_wf_dir = world.project_dir(&format!("wf-ev-bind-{seed}"))?;
        let ev_wf_proj = world.server_call(
            "remote-a",
            "project.create",
            json!({ "name": format!("Event Bind {seed}"), "directory": ev_wf_dir }),
        )?;
        let raw_pid_ev = ev_wf_proj["id"].as_str().unwrap().to_owned();
        let ev_col0 = world.server_call(
            "remote-a",
            "workflow.sources",
            json!({ "project_id": raw_pid_ev }),
        )?;
        let ev_plan_yaml = "schema: 12\nid: ev_plan\nname: Plan\non:\n  start:\n    event: manual\n    start_at: plan\nsteps:\n  - id: plan\n    uses: agent\n    with:\n      profile: developer\n      instruction: Plan\n    next: __end\n  - id: __end\n    uses: complete\nworkspace:\n  create_worktree: false\n  sync_base: false\n";
        let mut ev_sources: Vec<Value> = ev_col0["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| json!({ "filename": f["filename"], "source": f["source"] }))
            .collect();
        ev_sources.push(json!({ "filename": "ev_plan.yaml", "source": ev_plan_yaml }));
        let _ = world.server_call(
            "remote-a",
            "workflow.apply",
            json!({ "project_id": raw_pid_ev, "revision": ev_col0["revision"], "sources": ev_sources }),
        )?;
        let ev_sub_create = world.server_call(
            "remote-a",
            "subscription.save",
            json!({
                "revision": 0,
                "enabled": true,
                "selector": { "project_id": raw_pid_ev, "types": ["card.created"] },
                "target": { "type": "workflow", "workflow_id": "ev_plan", "entry": "start" },
            }),
        )?;
        let ev_c = world.server_call(
            "remote-a",
            "card.create",
            json!({ "view": "full", "project_id": raw_pid_ev, "title": "Automatic" }),
        )?;
        let ev_cid = ev_c["id"].as_str().unwrap().to_owned();
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(ev_cid.as_str())
                    && r.workflow_id == "ev_plan"
                    && r.active()
            }) && snap
                .terminals
                .iter()
                .any(|t| t.agent.as_ref().is_some_and(|a| a.card_id == ev_cid)))
        })?;
        let ev_delivs = world.server_call(
            "remote-a",
            "subscription.deliveries",
            json!({ "subscription_id": ev_sub_create["id"] }),
        )?;
        ensure!(
            ev_delivs[0]["state"] == "completed",
            "card.created event workflow delivery did not complete: {ev_delivs}"
        );
        // Triggering a second workflow subscription via `comment.created` on an already-running
        // Card must NOT roll back the source comment or launch a duplicate terminal!
        let ev_sub_cmt = world.server_call(
            "remote-a",
            "subscription.save",
            json!({
                "revision": 0,
                "enabled": true,
                "selector": { "project_id": raw_pid_ev, "types": ["comment.created"] },
                "target": { "type": "workflow", "workflow_id": "ev_plan", "entry": "start" },
            }),
        )?;
        let _ = world.server_call(
            "remote-a",
            "comment.add",
            json!({ "view": "full", "card_id": ev_cid, "body": "Still committed", "mentions": [] }),
        )?;
        world.advance(Duration::from_millis(300))?;
        {
            let ev_tl = world.server_call(
                "remote-a",
                "timeline.list",
                json!({ "card_id": ev_cid, "limit": 20, "view": "full" }),
            )?;
            ensure!(
                ev_tl["events"].as_array().is_some_and(|evs| {
                    evs.iter().any(|e| {
                        e["body"] == "Still committed" || e["message"] == "Still committed"
                    })
                }),
                "Source comment must not be rolled back when workflow subscription hits busy card: {ev_tl}"
            );
            let ev_terms = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .terminals
                .into_iter()
                .filter(|t| t.agent.as_ref().is_some_and(|a| a.card_id == ev_cid))
                .count();
            ensure!(
                ev_terms == 1,
                "Busy card must keep exactly 1 terminal, got {ev_terms}"
            );
        }
        // Disable test subscriptions on `raw_pid_ev` using current revision so pending retry does not respawn after stop.
        if let Some(cur_sub) = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .event_subscriptions
            .into_iter()
            .find(|s| Some(s.id.as_str()) == ev_sub_cmt["id"].as_str())
        {
            let _ = world.server_call(
                "remote-a",
                "subscription.save",
                json!({
                    "subscription_id": cur_sub.id,
                    "revision": cur_sub.revision,
                    "enabled": false,
                    "selector": { "project_id": raw_pid_ev, "types": ["comment.created"] },
                    "target": { "type": "workflow", "workflow_id": "ev_plan", "entry": "start" },
                }),
            )?;
        }
        for r in world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .workflow_runs
            .into_iter()
            .filter(|r| r.project_id == raw_pid_ev && r.active())
        {
            let _ = world.server_call("remote-a", "workflow.stop", json!({ "run_id": r.id }));
        }
        finish_all_remote_ptys(world)?;

        // 5E. Auto-commit dirty card worktree + `workflow.deliver` after dirty-main pause &
        //     after `workflow.stop` on merge conflict (100% in-process `SimGitRepo`, zero OS git spawns)!
        let deliv_dir = world.project_dir(&format!("wf-deliv-retry-{seed}"))?;
        let exec = world.exec_runner("remote-a").context("remote-a SimExec")?;
        exec.register_git_repo(&deliv_dir, SimGitRepo::default());
        let proj_deliv = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-a",
                "name": format!("Deliver Retry {seed}"),
                "directory": deliv_dir,
                "target_branch": "main",
            }),
            None,
        )?;
        let raw_pid_deliv =
            canban_types::unqualified_entity_id(proj_deliv["id"].as_str().unwrap()).to_owned();
        {
            let col = world.server_call(
                "remote-a",
                "workflow.sources",
                json!({ "project_id": raw_pid_deliv }),
            )?;
            let deliver_yaml = format!(
                r#"schema: 12
id: delivery:{raw_pid_deliv}
name: Development
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
  review:
    event: card.column_entered
    start_at: deliver:review
    match:
      column: review
steps:
  - id: agent:develop
    name: Implement
    uses: agent
    with:
      profile: developer
    next: move:develop
  - id: move:develop
    name: Move to Review
    uses: card.move
    with:
      column: review
    next: deliver:review
  - id: deliver:review
    name: Queue merge
    uses: deliver
    with:
      sync_upstream: false
      batch_size: 1
    next: __end
    on_changes_requested: agent:develop
  - id: __end
    uses: complete
workspace:
  create_worktree: true
  sync_base: false
"#
            );
            let _ = world.server_call(
                "remote-a",
                "workflow.apply",
                json!({
                    "project_id": raw_pid_deliv,
                    "revision": col["revision"],
                    "sources": [{ "filename": "development.yaml", "source": deliver_yaml }],
                }),
            )?;
        }
        // Part 1: Dirty worktree auto-committed on report + dirty main pauses queue + `workflow.deliver` completes!
        let dc1 = world.server_call(
            "remote-a",
            "card.create",
            json!({ "view": "full", "project_id": raw_pid_deliv, "title": "AutoCommit Card" }),
        )?;
        let dc1_id = dc1["id"].as_str().unwrap().to_owned();
        let _ = world.server_call(
            "remote-a",
            "card.move",
            json!({ "view": "full", "card_id": dc1_id, "revision": dc1["revision"], "stage": "develop" }),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .cards
                .iter()
                .any(|c| c.id == dc1_id && c.worktree.is_some())
                && snap.terminals.iter().any(|t| {
                    t.agent.as_ref().is_some_and(|a| {
                        a.card_id == dc1_id && a.state == "running" && a.terminal.is_some()
                    })
                }))
        })?;
        let (dc1_wt, dc1_tid, dc1_addr) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let wt = snap
                .cards
                .iter()
                .find(|c| c.id == dc1_id)
                .unwrap()
                .worktree
                .clone()
                .unwrap();
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == dc1_id))
                .unwrap();
            (
                wt,
                t.id.clone(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
            )
        };
        let dc1_head_before = exec.head_sha(&dc1_wt).context("dc1 head before")?;
        exec.write_worktree_file(&dc1_wt, "feature_a.txt", "hello auto commit\n")?;
        exec.set_worktree_dirty(&deliv_dir, true)?;
        let _ = world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": dc1_tid,
                "state": "completed",
                "message": "Auto-commit edits",
                "request_key": format!("sb-dc1-done-{seed}"),
            }),
        )?;
        herdr.finish_terminal(&dc1_addr, 0, "exited");
        let dc1_head_after = exec.head_sha(&dc1_wt).context("dc1 head after")?;
        ensure!(
            dc1_head_after != dc1_head_before,
            "execution.report must auto-commit dirty card worktree"
        );
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(dc1_id.as_str())
                    && r.state == "waiting_delivery"
                    && r.queue_pause_reason().is_some()
            }))
        })?;
        exec.set_worktree_dirty(&deliv_dir, false)?;
        let dc1_rev = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .into_iter()
            .find(|c| c.id == dc1_id)
            .unwrap()
            .revision;
        let _ = world.server_call(
            "remote-a",
            "workflow.deliver",
            json!({ "card_id": dc1_id, "revision": dc1_rev }),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .workflow_runs
                .iter()
                .any(|r| r.card_id.as_deref() == Some(dc1_id.as_str()) && r.state == "completed")
                && snap
                    .cards
                    .iter()
                    .any(|c| c.id == dc1_id && c.delivery.is_some()))
        })?;

        // Part 2: Stopped merge conflict -> `workflow.deliver` restarts at deliver & routes to agent!
        let dc2 = world.server_call(
            "remote-a",
            "card.create",
            json!({ "view": "full", "project_id": raw_pid_deliv, "title": "Stopped Conflict Retry" }),
        )?;
        let dc2_id = dc2["id"].as_str().unwrap().to_owned();
        let _ = world.server_call(
            "remote-a",
            "card.move",
            json!({ "view": "full", "card_id": dc2_id, "revision": dc2["revision"], "stage": "develop" }),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .cards
                .iter()
                .any(|c| c.id == dc2_id && c.worktree.is_some())
                && snap.terminals.iter().any(|t| {
                    t.agent.as_ref().is_some_and(|a| {
                        a.card_id == dc2_id && a.state == "running" && a.terminal.is_some()
                    })
                }))
        })?;
        let (dc2_wt, dc2_tid, dc2_addr) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let wt = snap
                .cards
                .iter()
                .find(|c| c.id == dc2_id)
                .unwrap()
                .worktree
                .clone()
                .unwrap();
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == dc2_id))
                .unwrap();
            (
                wt,
                t.id.clone(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
            )
        };
        let dc2_commit = exec.commit_in_worktree(&dc2_wt, "feat: dc2 conflicting edit")?;
        let _ = exec.commit_in_worktree(&deliv_dir, "main: conflicting edit")?;
        exec.inject_merge_conflict_for_commit(&deliv_dir, &dc2_commit, vec!["README.md".into()]);
        let _ = world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": dc2_tid,
                "state": "completed",
                "message": "dc2 ready",
                "request_key": format!("sb-dc2-done-{seed}"),
            }),
        )?;
        herdr.finish_terminal(&dc2_addr, 0, "exited");
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(dc2_id.as_str())
                    && r.active()
                    && r.state == "waiting_agent"
            }))
        })?;
        let dc2_run_id = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .workflow_runs
            .into_iter()
            .find(|r| r.card_id.as_deref() == Some(dc2_id.as_str()) && r.active())
            .unwrap()
            .id;
        let _ = world.server_call("remote-a", "workflow.stop", json!({ "run_id": dc2_run_id }))?;
        // End any live conflict-resolution agent terminal spawned before `workflow.stop` so
        // `workflow.deliver` does not wait on a live agent turn.
        finish_all_remote_ptys(world)?;
        let dc2_rev = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .into_iter()
            .find(|c| c.id == dc2_id)
            .unwrap()
            .revision;
        let dc2_restarted = world.server_call(
            "remote-a",
            "workflow.deliver",
            json!({ "card_id": dc2_id, "revision": dc2_rev }),
        )?;
        let dc2_restarted_id = dc2_restarted["id"].as_str().unwrap().to_owned();
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .workflow_runs
                .iter()
                .any(|r| r.id == dc2_restarted_id && r.state == "waiting_agent"))
        })?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let new_req = snap
                .terminals
                .iter()
                .rfind(|t| t.agent.as_ref().is_some_and(|a| a.card_id == dc2_id))
                .context("dc2 restarted conflict agent missing")?;
            let a = new_req.agent.as_ref().unwrap();
            let instr = a
                .pending
                .as_ref()
                .map(|q| q.instruction.as_str())
                .unwrap_or(a.instruction.as_str());
            ensure!(
                instr.contains("Merge conflict to resolve before delivery"),
                "restarted delivery conflict did not route conflict instruction to agent: {instr}"
            );
        }
        let _ = world.server_call(
            "remote-a",
            "workflow.stop",
            json!({ "run_id": dc2_restarted_id }),
        );
        finish_all_remote_ptys(world)?;

        // 7. Herdr Core identity replacement & reschedule recovery:
        //    (a) Silent Core swap within the 45s observer heartbeat window must NEVER stamp
        //        the obsolete cached `core_id` onto a freshly launched scheduled run;
        //    (b) Mid-flight `restart_core` during a running cardless schedule must transition
        //        it cleanly to `failed`, mark its dead terminal finished without killing on
        //        the new Core, release its `max_agents` slot, unblock `overlap: skip`, and
        //        allow immediate manual reschedule;
        //    (c) Mid-flight `restart_core` during a Card-creating scheduled run must mark the
        //        Card Agent `replaced` and ScheduledRun `blocked`, support direct
        //        `execution.retry` on the new Core, AND support clean local `terminal.close`
        //        on both replaced Card Agents and replaced standalone terminals with zero
        //        `terminal.kill` sent to the new Core.
        let raw_pid = canban_types::unqualified_entity_id(&pid).to_owned();
        let _ = world.server_call(
            "remote-a",
            "scheduler.configure",
            json!({ "max_agents": 1 }),
        )?;
        let recovery_sched_res = world.server_call(
            "remote-a",
            "schedule.create",
            json!({
                "project_id": raw_pid,
                "name": "Core Recovery Watch",
                "cron": "* * * * *",
                "overlap": "skip",
                "enabled": false,
                "mode": "agent",
                "profile_id": "developer",
                "instruction": "Verify core recovery",
                "request_key": format!("sb-core-rec-create-{seed}"),
            }),
        )?;
        let rec_sched_id = recovery_sched_res["schedule"]["id"]
            .as_str()
            .context("recovery schedule missing id")?
            .to_owned();

        // 7a. Silent Core swap (old subscriber stays open, < 45s heartbeat window).
        let silent_core = format!("sim-core-silent-{seed}");
        herdr.restart_core_silent(&silent_core);
        let silent_run = world.server_call(
            "remote-a",
            "schedule.trigger",
            json!({ "schedule_id": rec_sched_id }),
        )?;
        let silent_run_id = silent_run["id"]
            .as_str()
            .context("silent trigger missing run id")?
            .to_owned();
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == silent_run_id
                    && t.runtime.as_ref().is_some_and(|rt| {
                        rt.core_id.as_deref() == Some(silent_core.as_str())
                            && rt.terminal.is_some()
                            && rt.connection == "connected"
                            && rt.state == "running"
                    })
            }))
        })?;
        // Advance 2s to ensure observer reconnected to `silent_core` and never falsely
        // downgraded the newly launched run to `replaced`.
        world.advance(Duration::from_secs(2))?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let sr = snap
                .scheduled_runs
                .iter()
                .find(|r| r.id == silent_run_id)
                .context("silent scheduled run missing")?;
            let rt = snap
                .terminals
                .iter()
                .find(|t| t.id == silent_run_id)
                .and_then(|t| t.runtime.as_ref())
                .context("silent scheduled terminal missing")?;
            ensure!(
                matches!(sr.state.as_str(), "running" | "working")
                    && rt.connection == "connected"
                    && rt.core_id.as_deref() == Some(silent_core.as_str()),
                "Silent Core swap stamped stale core_id or falsely flagged replaced: run={sr:?} rt={rt:?}"
            );
        }

        // 7b. Transition `silent_run_id` into `blocked` (simulating a "Run #N · Waiting"
        //     state), then trigger mid-flight `restart_core`! Verify it does NOT remain
        //     stuck in `blocked` ("Run #N · Waiting" / attention badge `1`), transitions
        //     cleanly to `failed`, finishes its dead terminal without killing on the new
        //     Core, releases its slot, and allows immediate manual reschedule!
        let silent_addr = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.id == silent_run_id)
            .and_then(|t| t.runtime)
            .and_then(|rt| rt.terminal)
            .context("silent scheduled run missing herdr address")?;
        herdr.set_status(&silent_addr, "blocked", Some(1));
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .scheduled_runs
                .iter()
                .any(|r| r.id == silent_run_id && r.state == "blocked"))
        })?;
        let kills_before_restart = herdr.killed_terminals().len();
        let replaced_core = format!("sim-core-replaced-{seed}");
        herdr.restart_core(&replaced_core);
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let run_failed = snap.scheduled_runs.iter().any(|r| {
                r.id == silent_run_id
                    && r.state == "failed"
                    && r.completed_at.is_some()
                    && r.error
                        .as_deref()
                        .is_some_and(|e| e.contains("Core identity changed"))
            });
            let no_blocked = !snap
                .scheduled_runs
                .iter()
                .any(|r| r.schedule_id == rec_sched_id && r.state == "blocked");
            let term_finished = snap.terminals.iter().any(|t| {
                t.id == silent_run_id
                    && t.runtime
                        .as_ref()
                        .is_some_and(|rt| rt.state == "finished" && rt.connection == "replaced")
            });
            let slot_freed = canban_scheduler::active_count_at(&snap, w.now_ms()) == 0;
            Ok(run_failed && no_blocked && term_finished && slot_freed)
        })?;
        ensure!(
            herdr.killed_terminals().len() == kills_before_restart,
            "Reconciling replaced scheduled run must NEVER send terminal.kill to the new Core"
        );
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            ensure!(
                !canban_scheduler::timers::schedule_has_active_run(&snap, &rec_sched_id),
                "Replaced cardless scheduled run must not keep blocking overlap: skip"
            );
        }
        // Immediate manual reschedule (`schedule.trigger`) succeeds cleanly on `replaced_core`!
        let retried_sched = world.server_call(
            "remote-a",
            "schedule.trigger",
            json!({ "schedule_id": rec_sched_id }),
        )?;
        let retried_sched_id = retried_sched["id"]
            .as_str()
            .context("retried schedule trigger missing id")?
            .to_owned();
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == retried_sched_id
                    && t.runtime.as_ref().is_some_and(|rt| {
                        rt.core_id.as_deref() == Some(replaced_core.as_str())
                            && rt.terminal.is_some()
                            && rt.connection == "connected"
                    })
            }))
        })?;
        let _ = world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "schedule_run_id": retried_sched_id,
                "state": "completed",
                "message": "rescheduled run succeeded on new core",
                "request_key": format!("sb-core-rec-done-{seed}"),
            }),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .scheduled_runs
                .iter()
                .any(|r| r.id == retried_sched_id && r.state == "completed")
                && canban_scheduler::active_count_at(&snap, w.now_ms()) == 0)
        })?;

        // 7c. Card-creating scheduled run + standalone terminal hit `restart_core` ->
        //     direct `execution.retry` AND local `terminal.close` both work cleanly without
        //     sending `terminal.kill` to the new Core!
        let _ = world.server_call(
            "remote-a",
            "scheduler.configure",
            json!({ "max_agents": 4 }),
        )?;
        let card_sched_res = world.server_call(
            "remote-a",
            "schedule.create",
            json!({
                "project_id": raw_pid,
                "name": "Core Recovery Card",
                "cron": "0 9 * * *",
                "overlap": "skip",
                "enabled": false,
                "mode": "card",
                "card_column": "develop",
                "card_title": "Core Recovery Card #{{run_number}}",
                "profile_id": "developer",
                "instruction": "Verify card recovery after core restart",
                "request_key": format!("sb-core-card-create-{seed}"),
            }),
        )?;
        let card_sched_id = card_sched_res["schedule"]["id"]
            .as_str()
            .context("card recovery schedule missing id")?
            .to_owned();
        let card_run_1 = world.server_call(
            "remote-a",
            "schedule.trigger",
            json!({ "schedule_id": card_sched_id }),
        )?;
        let card_run_1_id = card_run_1["id"].as_str().unwrap().to_owned();
        let card_1_id = card_run_1["card_id"].as_str().unwrap().to_owned();
        let card_run_2 = world.server_call(
            "remote-a",
            "schedule.trigger",
            json!({ "schedule_id": card_sched_id }),
        )?;
        let card_2_id = card_run_2["card_id"].as_str().unwrap().to_owned();
        let manual_term = world.server_call(
            "remote-a",
            "terminal.create",
            json!({ "project_id": raw_pid }),
        )?;
        let manual_tid = manual_term["id"].as_str().unwrap().to_owned();

        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let c1_live = snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == card_1_id
                        && a.terminal.is_some()
                        && a.connection_state == "connected"
                })
            });
            let c2_live = snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == card_2_id
                        && a.terminal.is_some()
                        && a.connection_state == "connected"
                })
            });
            Ok(c1_live && c2_live)
        })?;
        let c1_tid = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == card_1_id))
            .unwrap()
            .id;
        let c2_tid = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == card_2_id))
            .unwrap()
            .id;

        let final_core = format!("sim-core-final-{seed}");
        herdr.restart_core(&final_core);
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let c1_rep = snap.terminals.iter().any(|t| {
                t.id == c1_tid
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.connection_state == "replaced")
            });
            let c2_rep = snap.terminals.iter().any(|t| {
                t.id == c2_tid
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.connection_state == "replaced")
            });
            let man_rep = snap.terminals.iter().any(|t| {
                t.id == manual_tid
                    && t.runtime
                        .as_ref()
                        .is_some_and(|rt| rt.connection == "replaced")
            });
            let sr_blocked = snap
                .scheduled_runs
                .iter()
                .any(|r| r.id == card_run_1_id && r.state == "blocked");
            Ok(c1_rep && c2_rep && man_rep && sr_blocked)
        })?;
        let kills_before_recovery = herdr.killed_terminals().len();

        // Direct `execution.retry` on replaced Card Agent 1 (no prior close required)!
        let _ = world.server_call(
            "remote-a",
            "execution.retry",
            json!({
                "terminal_id": c1_tid,
                "request_key": format!("sb-core-c1-retry-{seed}"),
            }),
        )?;
        // Clean local `terminal.close` on replaced Card Agent 2 and replaced manual terminal!
        let close_c2 = world.server_call(
            "remote-a",
            "terminal.close",
            json!({
                "terminal_id": c2_tid,
                "request_key": format!("sb-core-c2-close-{seed}"),
            }),
        )?;
        ensure!(
            close_c2["state"] == "closed",
            "terminal.close on replaced Card Agent must detach cleanly: {close_c2}"
        );
        let close_man = world.server_call(
            "remote-a",
            "terminal.close",
            json!({
                "terminal_id": manual_tid,
                "request_key": format!("sb-core-man-close-{seed}"),
            }),
        )?;
        ensure!(
            close_man["state"] == "closed",
            "terminal.close on replaced standalone terminal must detach cleanly: {close_man}"
        );
        ensure!(
            herdr.killed_terminals().len() == kills_before_recovery,
            "Retrying or closing replaced terminals must NEVER send terminal.kill to the new Core"
        );

        // Card 1's retried Agent launches on `final_core` and reconciles ScheduledRun back to running!
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let c1_new = snap.terminals.iter().any(|t| {
                t.id == c1_tid
                    && t.agent.as_ref().is_some_and(|a| {
                        a.core_id.as_deref() == Some(final_core.as_str())
                            && a.connection_state == "connected"
                            && a.terminal.is_some()
                    })
            });
            let sr_running = snap
                .scheduled_runs
                .iter()
                .any(|r| r.id == card_run_1_id && r.state == "running");
            Ok(c1_new && sr_running)
        })?;
        // Complete Card 1's retried execution and stop Card 2's workflow cleanly.
        let _ = world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": c1_tid,
                "state": "completed",
                "message": "card 1 recovered after core restart",
                "request_key": format!("sb-core-c1-done-{seed}"),
            }),
        )?;
        if let Some(c2_wf) = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .workflow_runs
            .into_iter()
            .find(|r| r.card_id.as_deref() == Some(card_2_id.as_str()) && r.active())
        {
            let _ = world.server_call("remote-a", "workflow.stop", json!({ "run_id": c2_wf.id }));
        }
        let _ = world.server_call(
            "remote-a",
            "schedule.delete",
            json!({ "schedule_id": rec_sched_id }),
        );
        let _ = world.server_call(
            "remote-a",
            "schedule.delete",
            json!({ "schedule_id": card_sched_id }),
        );
        finish_all_remote_ptys(world)?;
        world.hit("schedules_budget.core_restart_recovery_and_reschedule");

        ensure!(
            exec.blocked_commands().is_empty(),
            "schedules_and_budget blocked OS commands: {:?}",
            exec.blocked_commands()
        );
        world.hit("schedules_budget.backlog_and_workflow_edges");

        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        Ok(finalize_digest(world))
    })
}
