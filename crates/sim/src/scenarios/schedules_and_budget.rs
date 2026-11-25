//! Scenario #9 — Scheduler budget, priority queueing, Herdr-offline livelock guard (#29),
//! cron schedule ticks (`overlap: skip` and `overlap: queue`), and post-bounce recovery.
use crate::{
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use serde_json::json;
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

        Ok(finalize_digest(world))
    })
}
