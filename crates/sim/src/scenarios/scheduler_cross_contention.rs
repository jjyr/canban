//! Scenario #18 — Cross-Subsystem Scheduler Contention, Single-Card Multi-Scheduler Collisions,
//! Mixed PTY Idle Reclaim, All 5 Dual-Write Failpoints, Storage Faults, Framed Local Transport
//! (`AppServer::serve_local_stream` over `turmoil::net`), Read-Your-Writes & At-Most-Once Oracles,
//! and Natural Quiesce Recovery.
//!
//! - **Slice A — 4-Way `max_agents` Concurrency Contention (`max_agents = 2`, tightened to `1`)**:
//!   Simultaneously contends across (1) Workflow `uses: agent` cards, (2) Cron `ScheduledRun`
//!   (both carded & cardless schedules covering `overlap: skip`, `overlap: queue`, and
//!   `overlap: allow`), (3) `@merger` merge batch delivery, and (4) interactive `agent.open` /
//!   `card.adopt` — verifying `active_count_at <= max_agents` on every tick and across
//!   `bounce_server("local")`, zero slot leaks, and complete queue drain (`sched_contention.four_way_cap`).
//! - **Slice B — Single-Card Multi-Scheduler Collision**:
//!   Concurrently collides a card running an active Workflow step with a due Cron schedule,
//!   `card.move`, `@mention` + `agent.open`, `workflow.apply` hot-update removing the active step
//!   (`stop_after_step`), and `card.archive` on an active card — asserting `<= 1` active
//!   `WorkflowRun` per card, terminal/run parity, and zero zombie runs
//!   (`sched_contention.single_card_collision`).
//! - **Slice C — Mixed PTY Idle Reclaim**:
//!   Runs simultaneously (a) active carded Workflow PTY (`working`), (b) completed/idle cardless
//!   Cron PTY, and (c) idle interactive PTY protected by `viewer.presence`; advances virtual time
//!   past idle reclaim threshold and asserts only (b) is reclaimed while (a) and (c) stay alive
//!   (`sched_contention.mixed_pty_reclaim`).
//! - **Slice D — All 5 Dual-Write Failpoints + Storage Faults + Framed Local Restart + RYW / At-Most-Once + Natural Quiesce**:
//!   Arms and deterministically triggers all 5 dual-write crash failpoints
//!   (`FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB`, `FP_HERDR_AFTER_SEND_BEFORE_DB`,
//!   `FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB`, `FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE`,
//!   `FP_OUTBOX_MID_WRITE_TRUNCATE`), verifying recovery and `armed >= 1 && fired >= 1` for all 5
//!   with 0 vacuous armed faults; combines `StorageFaultKind::{Busy, IoError}`, framed local
//!   `bounce_server("local")` reconnect, `oracle.check_read_your_writes`,
//!   `oracle.check_at_most_once`, and unassisted `world.quiesce_natural` convergence
//!   (`sched_contention.all_five_failpoints`, `sched_contention.natural_quiesce`,
//!   `transport.local_framed_rpc`, `oracle.read_your_writes_checked`, `oracle.at_most_once_checked`).
use crate::{
    exec::{SimGitRepo, SimShellOutcome},
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{PRIMARY_CLIENT, SimWorld, SimWorldConfig, StorageFaultKind, run_with_world},
};
use anyhow::{Context, Result, ensure};
use canban_types::{
    ALL_FAILPOINTS, FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE, FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB,
    FP_HERDR_AFTER_SEND_BEFORE_DB, FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB,
    FP_OUTBOX_MID_WRITE_TRUNCATE, FailpointAction,
};
use rand::Rng;
use serde_json::json;
use std::{path::Path, time::Duration};

const CONTENTION_FLOW_YAML: &str = r#"schema: 12
id: contention-flow
name: Four-Way Contention Flow
workspace:
  create_worktree: true
  sync_base: false
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  start:
    event: manual
    start_at: work
  enter_develop:
    event: card.column_entered
    match:
      column: develop
    start_at: work
  cron_carded_skip:
    cron: "* * * * *"
    timezone: UTC
    overlap: skip
    start_at: work
    card:
      column: backlog
      title: "Cron Contention Card #{{run_number}}"
      body: "Scheduled carded contender"
  cron_cardless_skip:
    cron: "* * * * *"
    timezone: UTC
    overlap: skip
    start_at: cron_work
  cron_cardless_queue:
    cron: "* * * * *"
    timezone: UTC
    overlap: queue
    start_at: cron_work
  cron_cardless_allow:
    cron: "* * * * *"
    timezone: UTC
    overlap: allow
    start_at: cron_work
steps:
  - id: cron_work
    uses: agent
    with:
      profile: developer
      instruction: Execute cardless scheduled task under scheduler contention
    next: done
  - id: work
    stage: develop
    uses: agent
    with:
      profile: developer
      instruction: Execute feature work under scheduler contention
    next: deliver_step
  - id: deliver_step
    stage: review
    uses: deliver
    with:
      target_branch: main
      strategy: rebase
      sync_upstream: true
      batch_size: 2
      test_command: sim-contention-test
      max_conflict_attempts: 2
    next: done
    on_changes_requested: work
  - id: done
    stage: review
    uses: complete
"#;

const COLLISION_FLOW_V1_YAML: &str = r#"schema: 12
id: collision-flow
name: Single-Card Collision Flow
workspace:
  create_worktree: false
  sync_base: false
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  start:
    event: manual
    start_at: step_a
  enter_review:
    event: card.column_entered
    match:
      column: review
    start_at: step_b
  cron_collide:
    cron: "* * * * *"
    timezone: UTC
    overlap: skip
    start_at: step_a
    card:
      column: backlog
      title: "Collision Cron Card #{{run_number}}"
      body: "Concurrent cron during active card workflow"
steps:
  - id: step_a
    stage: develop
    uses: agent
    with:
      profile: developer
      instruction: Primary active step subject to collision
    next: step_b
  - id: step_b
    stage: review
    uses: agent
    with:
      profile: developer
      instruction: Secondary step targeted by card.move and removed by hot-update
    next: step_c
  - id: step_c
    stage: review
    uses: agent
    with:
      profile: developer
      instruction: Downstream step that must not run once step_b is removed
    next: done
  - id: done
    stage: review
    uses: complete
"#;

const COLLISION_FLOW_V2_STOP_AFTER_YAML: &str = r#"schema: 12
id: collision-flow
name: Single-Card Collision Flow
workspace:
  create_worktree: false
  sync_base: false
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  start:
    event: manual
    start_at: step_c
steps:
  - id: step_c
    stage: review
    uses: agent
    with:
      profile: developer
      instruction: Replacement step after step_b removed
    next: done
  - id: done
    stage: review
    uses: complete
"#;

const RECLAIM_MIX_YAML: &str = r#"schema: 12
id: reclaim-mix-flow
name: Mixed PTY Reclaim Flow
workspace:
  create_worktree: false
  sync_base: false
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  start:
    event: manual
    start_at: work
  cron_cardless_reclaim:
    cron: "* * * * *"
    timezone: UTC
    overlap: skip
    start_at: work
steps:
  - id: work
    stage: develop
    uses: agent
    with:
      profile: codex
      instruction: Run task for mixed reclaim verification
    next: done
  - id: done
    stage: review
    uses: complete
"#;

const REMOTE_PR_FLOW_YAML: &str = r#"schema: 12
id: remote-pr-fp-flow
name: Remote PR Failpoint Flow
workspace:
  create_worktree: true
  sync_base: false
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  start:
    event: manual
    start_at: done
steps:
  - id: done
    stage: review
    uses: complete
"#;

fn strip_host(id: &str) -> String {
    canban_types::split_qualified_host_id(id)
        .map(|(_, r)| r.to_owned())
        .unwrap_or_else(|| id.to_owned())
}

fn assert_cap_and_invariants(
    world: &SimWorld,
    oracle: &mut InvariantOracle,
    host: &str,
    cap: usize,
) -> Result<usize> {
    oracle.check_step_invariants(world)?;
    let srv = world
        .server(host)
        .with_context(|| format!("server '{host}' not running"))?;
    let snap = srv.snapshot()?;
    let active = canban_scheduler::active_count_at(&snap, srv.now_ms());
    ensure!(
        active <= cap,
        "[{host}] active_count_at ({active}) exceeded cap ({cap})"
    );
    Ok(active)
}

fn toggle_all_schedules(world: &mut SimWorld, project_id: &str, enabled: bool) -> Result<()> {
    let list = world.client_call(
        "schedule.list",
        json!({ "project_id": project_id }),
        Some(project_id),
    )?;
    for s in list["schedules"].as_array().into_iter().flatten() {
        if let Some(sid) = s["id"].as_str() {
            let _ = world.client_call(
                "schedule.toggle",
                json!({ "schedule_id": sid, "enabled": enabled }),
                Some(project_id),
            )?;
        }
    }
    Ok(())
}

/// Completes a single active agent/schedule/merger work item on `host` so queued contenders
/// advance deterministically without stalling.
fn finish_one_active_contender(
    world: &mut SimWorld,
    oracle: &mut InvariantOracle,
    host: &str,
    seq: &mut u64,
    seed: u64,
) -> Result<bool> {
    let srv = world
        .server(host)
        .with_context(|| format!("server '{host}' not running"))?;
    let snap = srv.snapshot()?;
    let herdr = world
        .herdr(host)
        .with_context(|| format!("herdr '{host}' unavailable"))?;
    let exec = world
        .exec_runner(host)
        .with_context(|| format!("exec '{host}' unavailable"))?;

    // 1. Complete any active `@merger` MergeBatch first.
    if let Some(batch) = snap
        .merge_batches
        .iter()
        .find(|b| b.active() && b.state == "running" && Path::new(&b.worktree).is_dir())
    {
        let _ = exec.resolve_worktree_and_commit(&batch.worktree)?;
        *seq += 1;
        let rkey = format!("s18-drain-batch-{seed}-{seq}");
        let res = world.server_call(
            host,
            "batch.report",
            json!({
                "batch_id": batch.id,
                "state": "completed",
                "message": "Resolved contention batch tests",
                "request_key": rkey,
            }),
        )?;
        ensure!(
            res["accepted"].as_bool() == Some(true),
            "Unexpected batch.report reply during drain: {res}"
        );
        oracle.record_request_key_attempt(host, &rkey, "batch.report", true);
        if let Some(term) = snap.terminals.iter().find(|t| t.id == batch.id)
            && let Some(addr) = term.runtime.as_ref().and_then(|r| r.terminal.as_deref())
        {
            herdr.finish_terminal(addr, 0, "exited");
        }
        return Ok(true);
    }

    // 2. Complete any active cardless ScheduledRun whose PTY is running.
    if let Some(sr) = snap
        .scheduled_runs
        .iter()
        .find(|r| !r.creates_card && matches!(r.state.as_str(), "running" | "working" | "blocked"))
    {
        *seq += 1;
        let rkey = format!("s18-drain-sched-{seed}-{seq}");
        let _ = world.server_call(
            host,
            "execution.report",
            json!({
                "schedule_run_id": sr.id,
                "state": "completed",
                "message": "Scheduled contention run completed",
                "request_key": rkey,
            }),
        )?;
        oracle.record_request_key_attempt(host, &rkey, "execution.report", true);
        if let Some(tid) = sr.terminal_id.as_deref()
            && let Some(term) = snap.terminals.iter().find(|t| t.id == tid)
            && let Some(addr) = term.runtime.as_ref().and_then(|r| r.terminal.as_deref())
        {
            herdr.finish_terminal(addr, 0, "exited");
        }
        return Ok(true);
    }

    // 3. Complete any running card AgentRun (committing work if it has a worktree).
    if let Some(term) = snap.terminals.iter().find(|t| {
        t.agent
            .as_ref()
            .is_some_and(|a| !a.finished() && a.terminal.is_some() && a.state == "running")
    }) {
        let agent = term.agent.as_ref().unwrap();
        let addr = agent.terminal.as_deref().unwrap().to_owned();
        let card_id = agent.card_id.clone();
        let has_wf_run = snap
            .workflow_runs
            .iter()
            .any(|r| r.active() && r.card_id.as_deref() == Some(card_id.as_str()));
        if has_wf_run {
            if let Some(card) = snap.cards.iter().find(|c| c.id == card_id)
                && card.branch.is_some()
                && let Some(wt) = card.worktree.as_deref()
                && Path::new(wt).is_dir()
            {
                let _ = exec.commit_in_worktree(wt, &format!("feat: contention work {seq}"))?;
            }
            *seq += 1;
            let rkey = format!("s18-drain-exec-{seed}-{seq}");
            let _ = world.server_call(
                host,
                "execution.report",
                json!({
                    "terminal_id": term.id,
                    "state": "completed",
                    "message": "Workflow agent completed under contention",
                    "request_key": rkey,
                }),
            )?;
            oracle.record_request_key_attempt(host, &rkey, "execution.report", true);
            herdr.finish_terminal(&addr, 0, "exited");
        } else {
            // Interactive agent session without an active workflow run: end cleanly.
            *seq += 1;
            let rkey = format!("s18-drain-end-{seed}-{seq}");
            let _ = world.server_call(
                host,
                "agent.end",
                json!({
                    "terminal_id": term.id,
                    "request_key": rkey,
                }),
            )?;
            herdr.finish_terminal(&addr, 0, "exited");
        }
        return Ok(true);
    }

    // 4. Close any live cardless interactive/shell project terminal with runtime state running.
    if let Some(term) = snap.terminals.iter().find(|t| {
        t.agent.is_none()
            && t.runtime
                .as_ref()
                .is_some_and(|r| r.state == "running" && r.terminal.is_some())
            && !snap
                .scheduled_runs
                .iter()
                .any(|sr| sr.terminal_id.as_deref() == Some(t.id.as_str()))
            && !snap.merge_batches.iter().any(|b| b.id == t.id)
    }) {
        *seq += 1;
        let _ = world.server_call(
            host,
            "terminal.close",
            json!({
                "terminal_id": term.id,
                "request_key": format!("s18-drain-term-{seed}-{seq}"),
            }),
        )?;
        return Ok(true);
    }

    Ok(false)
}

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "scheduler_cross_contention");
    cfg.framed_local_transport = true;
    cfg.remote_hosts = vec!["remote-a".into()];
    cfg.tick_ms = 50;
    // Constant message latency + deterministic host step order keep this heavy multi-slice
    // scenario 100% immune to `yamux-0.14.1`'s wall-clock 10s RTT `PING_INTERVAL`
    // (`yamux/src/connection/rtt.rs:24`) shifting turmoil's RNG under parallel CI CPU load.
    cfg.min_latency_ms = 5;
    cfg.max_latency_ms = 5;
    cfg.enable_random_order = false;

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();
        let mut drain_seq: u64 = 0;

        // Seed-sensitive deterministic choice so distinct seeds vary workloads & digests cleanly.
        let variant_tag: u32 = world.rng.random_range(100..=999);

        // =====================================================================
        // SLICE A — 4-Way `max_agents` Concurrency Contention (`max_agents = 2` -> `1`)
        //           over Framed Local Transport (`AppServer::serve_local_stream`)
        // =====================================================================
        let local_dir = world.project_dir(&format!("sched-local-{seed}"))?;
        let exec_local = world
            .exec_runner("local")
            .context("local SimExec unavailable")?;
        exec_local.register_git_repo(&local_dir, SimGitRepo::default());
        // Fail batch test #1 to spawn `@merger` as Contender 3, then pass!
        exec_local.enqueue_shell_outcomes(
            "sim-contention-test",
            [SimShellOutcome::fail(
                1,
                "running contention tests\n",
                "contention_gate FAILED attempt 1\n",
            )],
        );
        exec_local.set_shell_default(
            "sim-contention-test",
            SimShellOutcome::ok("contention batch tests passed\n"),
        );

        let proj_local = world.client_call(
            "project.create",
            json!({
                "host_id": "local",
                "name": format!("Contention Local {seed}-{variant_tag}"),
                "directory": local_dir,
                "target_branch": "main",
            }),
            None,
        )?;
        let pid_local = proj_local["id"]
            .as_str()
            .context("local project.create missing id")?
            .to_owned();

        // Start with `max_agents: 2`, `idle_minutes: 1`.
        let _ = world.client_call(
            "scheduler.configure",
            json!({ "max_agents": 2, "idle_minutes": 1 }),
            Some(&pid_local),
        )?;

        let local_sources = world.client_call(
            "workflow.sources",
            json!({ "project_id": pid_local }),
            Some(&pid_local),
        )?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid_local,
                "revision": local_sources["revision"],
                "sources": [{ "filename": "contention-flow.yaml", "source": CONTENTION_FLOW_YAML }],
            }),
            Some(&pid_local),
        )?;

        // Disable automatic cron initially while staging Card A1 into `@merger`.
        toggle_all_schedules(world, &pid_local, false)?;

        // Contender 3 setup: Card A1 commits work -> enters `deliver_step` -> test fails -> spawns `@merger`!
        let card_a1_key = format!("s18-a1-create-{seed}");
        let card_a1_title = format!("Slice A Merger Card 1 ({seed}-{variant_tag})");
        let card_a1 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_local,
                "title": card_a1_title,
                "request_key": card_a1_key,
            }),
            Some(&pid_local),
        )?;
        let cid_a1 = card_a1["id"].as_str().unwrap().to_owned();
        oracle.observe_write_receipt(PRIMARY_CLIENT, "local", &card_a1)?;
        oracle.record_committed_card(
            "local-server",
            &cid_a1,
            &pid_local,
            card_a1["revision"].as_u64().unwrap_or(1),
        );
        oracle.record_request_key_attempt_with_marker(
            "local",
            &card_a1_key,
            "card.create",
            true,
            card_a1_title,
        );

        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "contention-flow",
                "card_id": cid_a1,
                "entry": "start",
                "request_key": format!("s18-a1-start-{seed}"),
            }),
            Some(&pid_local),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == cid_a1 && a.state == "running" && a.terminal.is_some()
                })
            }))
        })?;
        let (a1_tid, a1_addr, a1_wt) = {
            let snap = world.server("local").unwrap().snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == cid_a1))
                .unwrap();
            let c = snap.cards.iter().find(|c| c.id == cid_a1).unwrap();
            (
                t.id.clone(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
                c.worktree.clone().expect("Card A1 worktree"),
            )
        };
        let _ = exec_local.commit_in_worktree(&a1_wt, "feat: slice A1 initial work")?;
        let _ = world.server_call(
            "local",
            "execution.report",
            json!({
                "terminal_id": a1_tid,
                "state": "completed",
                "message": "A1 ready for batch delivery",
                "request_key": format!("s18-a1-done-{seed}"),
            }),
        )?;
        world
            .herdr("local")
            .unwrap()
            .finish_terminal(&a1_addr, 0, "exited");

        // Wait until `@merger` MergeBatch is actively running (holding slot 1 of 2)!
        world.wait_until(15_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap.merge_batches.iter().any(|b| {
                b.failure_kind.as_deref() == Some("test_failure")
                    && b.state == "running"
                    && Path::new(&b.worktree).is_dir()
            }))
        })?;
        let slice_a_batch_id = world
            .server("local")
            .unwrap()
            .snapshot()?
            .merge_batches
            .iter()
            .find(|b| b.failure_kind.as_deref() == Some("test_failure") && b.active())
            .expect("@merger batch active")
            .id
            .clone();
        oracle.record_committed_merge_batch("local-server", &slice_a_batch_id, &pid_local);

        // Now unleash ALL 4 contenders simultaneously against `max_agents = 2`!
        // Start by adopting a live PTY onto Card A4 via `card.adopt` while `@merger` holds slot 1 of 2
        // (occupying slot 2 of 2 so the remaining Workflow, Interactive `agent.open`, and Cron contenders
        // must strictly queue behind the 2-slot cap without oversubscribing!).
        let card_a4 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_local,
                "title": format!("Slice A Adopted Card 4 ({seed})"),
            }),
            Some(&pid_local),
        )?;
        let cid_a4 = card_a4["id"].as_str().unwrap().to_owned();
        oracle.observe_write_receipt(PRIMARY_CLIENT, "local", &card_a4)?;
        let adopt_term = world.client_call(
            "terminal.create",
            json!({
                "card_id": cid_a4,
                "agent": true,
                "profile_id": "developer",
                "name": format!("Slice A Adopt PTY {seed}"),
            }),
            Some(&pid_local),
        )?;
        let adopt_tid = strip_host(adopt_term["id"].as_str().unwrap());
        world.wait_until(10_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == adopt_tid
                    && t.runtime
                        .as_ref()
                        .is_some_and(|r| r.state == "running" && r.terminal.is_some())
            }))
        })?;
        let (adopt_addr, adopt_cwd) = {
            let snap = world.server("local").unwrap().snapshot()?;
            let addr = snap
                .terminals
                .iter()
                .find(|t| t.id == adopt_tid)
                .and_then(|t| t.runtime.as_ref())
                .and_then(|r| r.terminal.clone())
                .context("Slice A adopt terminal missing Herdr address")?;
            let cwd = snap
                .cards
                .iter()
                .find(|c| c.id == cid_a4)
                .and_then(|c| c.worktree.clone())
                .unwrap_or_else(|| local_dir.clone());
            (addr, cwd)
        };
        let adopted_a4 = world.client_call(
            "card.adopt",
            json!({
                "view": "full",
                "card_id": cid_a4,
                "profile_id": "developer",
                "stage": "develop",
                "cwd": adopt_cwd,
                "runtime_terminal": adopt_addr,
                "runtime_session": "default",
                "request_key": format!("s18-a4-adopt-{seed}"),
            }),
            Some(&pid_local),
        )?;
        ensure!(
            adopted_a4["adopted"].as_bool() == Some(true),
            "Expected Slice A card.adopt to adopt live PTY cleanly, got {adopted_a4}"
        );

        // (1) Workflow `uses: agent` card A2
        let card_a2 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_local,
                "title": format!("Slice A Workflow Card 2 ({seed})"),
            }),
            Some(&pid_local),
        )?;
        let cid_a2 = card_a2["id"].as_str().unwrap().to_owned();
        oracle.observe_write_receipt(PRIMARY_CLIENT, "local", &card_a2)?;
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "contention-flow",
                "card_id": cid_a2,
                "entry": "start",
                "request_key": format!("s18-a2-start-{seed}"),
            }),
            Some(&pid_local),
        )?;

        // (4) Interactive `agent.open` on Card A3
        let card_a3 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_local,
                "title": format!("Slice A Interactive Open Card 3 ({seed})"),
            }),
            Some(&pid_local),
        )?;
        let cid_a3 = card_a3["id"].as_str().unwrap().to_owned();
        oracle.observe_write_receipt(PRIMARY_CLIENT, "local", &card_a3)?;
        let _ = world.client_call(
            "agent.open",
            json!({
                "card_id": cid_a3,
                "profile_id": "developer",
                "request_key": format!("s18-a3-open-{seed}"),
            }),
            Some(&pid_local),
        )?;

        // (2) Enable & trigger Cron schedules covering carded + cardless (`skip`, `queue`, `allow`)!
        toggle_all_schedules(world, &pid_local, true)?;
        for rule_id in [
            "cron_cardless_skip",
            "cron_cardless_queue",
            "cron_cardless_allow",
        ] {
            let _ = world.client_call(
                "schedule.trigger",
                json!({
                    "project_id": pid_local,
                    "workflow_id": "contention-flow",
                    "rule_id": rule_id,
                }),
                Some(&pid_local),
            )?;
        }

        // Step across the next 1-minute cron boundary (`SIM_EPOCH_BASE_MS` starts at :20s)
        // in 5s increments asserting `active <= 2` on EVERY step!
        let mut saw_cap_saturated = false;
        for _ in 0..9 {
            world.advance(Duration::from_secs(5))?;
            let active = assert_cap_and_invariants(world, &mut oracle, "local", 2)?;
            if active == 2 {
                saw_cap_saturated = true;
            }
        }
        ensure!(
            saw_cap_saturated,
            "Expected 4-way contention to saturate max_agents=2 on local-server"
        );

        // Verify all 3 overlap policies (`skip`, `queue`, `allow`) AND carded cron fired!
        {
            let snap = world.server("local").unwrap().snapshot()?;
            ensure!(
                snap.cards
                    .iter()
                    .any(|c| c.title.starts_with("Cron Contention Card #")),
                "Carded cron schedule did not create a scheduled card under contention"
            );
            ensure!(
                snap.scheduled_runs
                    .iter()
                    .any(|r| r.rule_id == "cron_cardless_skip" && r.state == "skipped"),
                "overlap: skip did not record a skipped ScheduledRun"
            );
            let allow_count = snap
                .scheduled_runs
                .iter()
                .filter(|r| r.rule_id == "cron_cardless_allow")
                .count();
            ensure!(
                allow_count >= 2,
                "overlap: allow should spawn >= 2 concurrent ScheduledRuns, found {allow_count}"
            );
            let sched_list = world.client_call(
                "schedule.list",
                json!({ "project_id": pid_local }),
                Some(&pid_local),
            )?;
            ensure!(
                sched_list["schedules"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|s| {
                        s["rule_id"] == "cron_cardless_queue"
                            && s["queued_triggers"].as_u64().unwrap_or(0) >= 1
                    }),
                "overlap: queue should queue >= 1 pending trigger, got {sched_list}"
            );
        }

        // Disable further cron ticks so the queued backlog is bounded, then bounce `"local"` mid-contention!
        toggle_all_schedules(world, &pid_local, false)?;
        world.bounce_server("local")?;
        world.quiesce_natural(15_000)?;
        let _ = assert_cap_and_invariants(world, &mut oracle, "local", 2)?;

        // Drain contenders under `max_agents = 2` until `active <= 1`, THEN tighten `max_agents` to `1`
        // while queued contenders remain, proving the tightened cap holds strictly with zero starvation!
        let mut tightened_to_one = false;
        let mut current_cap = 2usize;
        for _ in 0..40 {
            let active = {
                let srv = world.server("local").context("missing local server")?;
                let snap = srv.snapshot()?;
                let active = canban_scheduler::active_count_at(&snap, srv.now_ms());
                ensure!(
                    active <= current_cap,
                    "Scheduler cap violated before drain step: active_count_at={active} > cap={current_cap}"
                );
                active
            };
            if !tightened_to_one && active <= 1 {
                let _ = world.client_call(
                    "scheduler.configure",
                    json!({ "max_agents": 1 }),
                    Some(&pid_local),
                )?;
                current_cap = 1;
                tightened_to_one = true;
                world.advance(Duration::from_millis(400))?;
                let _ = assert_cap_and_invariants(world, &mut oracle, "local", 1)?;
            }
            let progressed =
                finish_one_active_contender(world, &mut oracle, "local", &mut drain_seq, seed)?;
            world.advance(Duration::from_millis(600))?;
            let active_after = assert_cap_and_invariants(world, &mut oracle, "local", current_cap)?;
            if !progressed && active_after == 0 {
                // Check if any queued scheduled runs or workflow runs remain waiting to launch.
                let snap = world.server("local").unwrap().snapshot()?;
                let still_queued = snap
                    .scheduled_runs
                    .iter()
                    .any(|r| !r.creates_card && r.active())
                    || snap.workflow_runs.iter().any(|r| r.active())
                    || snap.merge_batches.iter().any(|b| b.active());
                if !still_queued {
                    break;
                }
                world.advance(Duration::from_secs(2))?;
            }
        }
        ensure!(
            tightened_to_one,
            "Expected Slice A to tighten max_agents from 2 down to 1"
        );
        // Zero slot leaks after draining all 4 contenders!
        {
            let srv = world.server("local").unwrap();
            let snap = srv.snapshot()?;
            ensure!(
                canban_scheduler::active_count_at(&snap, srv.now_ms()) == 0,
                "Slot leak detected after draining Slice A contenders"
            );
        }
        world.hit("sched_contention.four_way_cap");

        // =====================================================================
        // SLICE B — Single-Card Multi-Scheduler Collision
        // =====================================================================
        let _ = world.client_call(
            "scheduler.configure",
            json!({ "max_agents": 3, "idle_minutes": 1 }),
            Some(&pid_local),
        )?;
        let col_dir = world.project_dir(&format!("sched-collide-{seed}"))?;
        let proj_col = world.client_call(
            "project.create",
            json!({
                "host_id": "local",
                "name": format!("Collision Local {seed}"),
                "directory": col_dir,
            }),
            None,
        )?;
        let pid_col = proj_col["id"].as_str().unwrap().to_owned();
        let col_sources = world.client_call(
            "workflow.sources",
            json!({ "project_id": pid_col }),
            Some(&pid_col),
        )?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid_col,
                "revision": col_sources["revision"],
                "sources": [{ "filename": "collision-flow.yaml", "source": COLLISION_FLOW_V1_YAML }],
            }),
            Some(&pid_col),
        )?;

        let card_b1 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_col,
                "title": format!("Collision Target Card B1 ({seed})"),
            }),
            Some(&pid_col),
        )?;
        let cid_b1 = card_b1["id"].as_str().unwrap().to_owned();
        oracle.observe_write_receipt(PRIMARY_CLIENT, "local", &card_b1)?;

        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "collision-flow",
                "card_id": cid_b1,
                "entry": "start",
                "request_key": format!("s18-b1-start-{seed}"),
            }),
            Some(&pid_col),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == cid_b1 && a.state == "running" && a.terminal.is_some()
                })
            }))
        })?;
        let (b1_wf_id, b1_rev, b1_step_a_addr) = {
            let snap = world.server("local").unwrap().snapshot()?;
            let wf = snap
                .workflow_runs
                .iter()
                .find(|r| r.card_id.as_deref() == Some(cid_b1.as_str()) && r.active())
                .unwrap();
            let c = snap.cards.iter().find(|c| c.id == cid_b1).unwrap();
            let addr = wf
                .terminal_id
                .as_deref()
                .and_then(|tid| snap.terminals.iter().find(|t| t.id == tid))
                .and_then(|t| t.agent.as_ref())
                .and_then(|a| a.terminal.clone());
            (wf.id.clone(), c.revision, addr)
        };

        // Concurrently hit Card B1 while `step_a` is executing:
        // 1. Due Cron schedule tick on the same workflow
        let _ = world.client_call(
            "schedule.trigger",
            json!({
                "project_id": pid_col,
                "workflow_id": "collision-flow",
                "rule_id": "cron_collide",
            }),
            Some(&pid_col),
        )?;
        // 2. `card.move` to `review` (triggers `enter_review` -> supersedes `step_a`, queues `step_b`
        //    onto the conversation terminal, preserving `<= 1` active WorkflowRun per card!)
        let moved_b1 = world.client_call(
            "card.move",
            json!({
                "card_id": cid_b1,
                "revision": b1_rev,
                "stage": "review",
                "request_key": format!("s18-b1-move-{seed}"),
            }),
            Some(&pid_col),
        )?;
        oracle.observe_write_receipt(PRIMARY_CLIENT, "local", &moved_b1)?;
        // Once the superseded `step_a` PTY finishes, the scheduler promotes queued `step_b`
        // from `agent.pending` onto the conversation terminal.
        if let Some(addr) = b1_step_a_addr {
            world
                .herdr("local")
                .unwrap()
                .finish_terminal(&addr, 0, "exited");
        }
        world.wait_until(10_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(cid_b1.as_str())
                    && r.active()
                    && r.step.as_deref() == Some("step_b")
                    && r.terminal_id.as_deref().is_some_and(|tid| {
                        snap.terminals.iter().any(|t| {
                            t.id == tid
                                && t.agent.as_ref().is_some_and(|a| {
                                    a.state == "running"
                                        && a.terminal.is_some()
                                        && a.pending.is_none()
                                        && a.workflow_run_id.as_deref() == Some(r.id.as_str())
                                })
                        })
                    })
            }))
        })?;
        // 3. `@mention` + `agent.open` on Card B1
        let _ = world.client_call(
            "comment.add",
            json!({
                "card_id": cid_b1,
                "body": format!("@developer collision check seed={seed}"),
                "request_key": format!("s18-b1-mention-{seed}"),
            }),
            Some(&pid_col),
        )?;
        let _ = world.client_call(
            "agent.open",
            json!({
                "card_id": cid_b1,
                "profile_id": "developer",
                "request_key": format!("s18-b1-open-{seed}"),
            }),
            Some(&pid_col),
        )?;
        // 4. `workflow.apply` hot-update removing `step_b` -> sets `stop_after_step = "step_b"`!
        let col_sources_v2 = world.client_call(
            "workflow.sources",
            json!({ "project_id": pid_col }),
            Some(&pid_col),
        )?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid_col,
                "revision": col_sources_v2["revision"],
                "sources": [{
                    "filename": "collision-flow.yaml",
                    "source": COLLISION_FLOW_V2_STOP_AFTER_YAML,
                }],
            }),
            Some(&pid_col),
        )?;
        world.advance(Duration::from_millis(500))?;
        oracle.check_step_invariants(world)?;

        // Assert `<= 1` active WorkflowRun on Card B1, superseded prior run, AND `stop_after_step == "step_b"`!
        let (b1_active_wf_id, b1_active_tid, b1_active_addr) = {
            let snap = world.server("local").unwrap().snapshot()?;
            let prev_wf = snap
                .workflow_runs
                .iter()
                .find(|r| r.id == b1_wf_id)
                .unwrap();
            ensure!(
                !prev_wf.active(),
                "Original step_a run {b1_wf_id} must be superseded/inactive after card.move collision, got {}",
                prev_wf.state
            );
            let active_for_b1: Vec<_> = snap
                .workflow_runs
                .iter()
                .filter(|r| r.card_id.as_deref() == Some(cid_b1.as_str()) && r.active())
                .collect();
            ensure!(
                active_for_b1.len() == 1,
                "Expected exactly 1 active WorkflowRun on Card B1 under collision, got {}",
                active_for_b1.len()
            );
            ensure!(
                active_for_b1[0].inputs["stop_after_step"].as_str() == Some("step_b"),
                "Expected workflow.apply hot-update to set stop_after_step=\"step_b\", got {:?}",
                active_for_b1[0].inputs
            );
            let tid = active_for_b1[0]
                .terminal_id
                .clone()
                .expect("step_b run terminal_id");
            let addr = snap
                .terminals
                .iter()
                .find(|t| t.id == tid)
                .and_then(|t| t.agent.as_ref())
                .and_then(|a| a.terminal.clone());
            (active_for_b1[0].id.clone(), tid, addr)
        };

        // Complete `step_b` -> run MUST stop cleanly without advancing to `step_c`!
        let b1_report = world.server_call(
            "local",
            "execution.report",
            json!({
                "terminal_id": b1_active_tid,
                "state": "completed",
                "message": "step_b finished under stop_after_step",
                "request_key": format!("s18-b1-step-b-done-{seed}"),
            }),
        )?;
        ensure!(
            b1_report["accepted"].as_bool() == Some(true),
            "Expected step_b execution.report accepted=true, got {b1_report}"
        );
        if let Some(addr) = b1_active_addr {
            world
                .herdr("local")
                .unwrap()
                .finish_terminal(&addr, 0, "exited");
        }
        world.wait_until(10_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap
                .workflow_runs
                .iter()
                .any(|r| r.id == b1_active_wf_id && !r.active()))
        })?;
        {
            let snap = world.server("local").unwrap().snapshot()?;
            let wf = snap
                .workflow_runs
                .iter()
                .find(|r| r.id == b1_active_wf_id)
                .unwrap();
            ensure!(
                wf.step.as_deref() != Some("step_c") && !wf.active(),
                "stop_after_step run must stop cleanly after step_b instead of entering step_c: state={} step={:?}",
                wf.state,
                wf.step
            );
        }

        // 5. `card.archive` on an actively running Card B2 -> cancels active WorkflowRun cleanly!
        let card_b2 = world.client_call(
            "card.create",
            json!({
                "project_id": pid_col,
                "title": format!("Collision Archive Card B2 ({seed})"),
            }),
            Some(&pid_col),
        )?;
        let cid_b2 = card_b2["id"].as_str().unwrap().to_owned();
        oracle.observe_write_receipt(PRIMARY_CLIENT, "local", &card_b2)?;
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "collision-flow",
                "card_id": cid_b2,
                "entry": "start",
                "request_key": format!("s18-b2-start-{seed}"),
            }),
            Some(&pid_col),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == cid_b2 && a.state == "running" && a.terminal.is_some()
                })
            }))
        })?;
        let b2_rev = world
            .server("local")
            .unwrap()
            .snapshot()?
            .cards
            .iter()
            .find(|c| c.id == cid_b2)
            .unwrap()
            .revision;
        let archived_b2 = world.client_call(
            "card.archive",
            json!({
                "card_id": cid_b2,
                "revision": b2_rev,
                "request_key": format!("s18-b2-archive-{seed}"),
            }),
            Some(&pid_col),
        )?;
        oracle.observe_write_receipt(PRIMARY_CLIENT, "local", &archived_b2)?;
        world.advance(Duration::from_millis(500))?;
        {
            let snap = world.server("local").unwrap().snapshot()?;
            let zombie_runs = snap
                .workflow_runs
                .iter()
                .filter(|r| r.card_id.as_deref() == Some(cid_b2.as_str()) && r.active())
                .count();
            ensure!(
                zombie_runs == 0,
                "Archived Card B2 retained {zombie_runs} zombie active WorkflowRuns!"
            );
        }
        // Drain any remaining collision-project terminals/runs before Slice C.
        toggle_all_schedules(world, &pid_col, false)?;
        for _ in 0..10 {
            if !finish_one_active_contender(world, &mut oracle, "local", &mut drain_seq, seed)? {
                break;
            }
            world.advance(Duration::from_millis(400))?;
        }
        oracle.check_step_invariants(world)?;
        world.hit("sched_contention.single_card_collision");

        // =====================================================================
        // SLICE C — Mixed PTY Idle Reclaim (Active Workflow vs Idle Cardless Cron vs Viewer-Protected)
        // =====================================================================
        let _ = world.client_call(
            "scheduler.configure",
            json!({ "max_agents": 4, "idle_minutes": 1 }),
            Some(&pid_local),
        )?;
        let rec_dir = world.project_dir(&format!("sched-reclaim-{seed}"))?;
        let proj_rec = world.client_call(
            "project.create",
            json!({
                "host_id": "local",
                "name": format!("Reclaim Mix {seed}"),
                "directory": rec_dir,
            }),
            None,
        )?;
        let pid_rec = proj_rec["id"].as_str().unwrap().to_owned();
        let rec_sources = world.client_call(
            "workflow.sources",
            json!({ "project_id": pid_rec }),
            Some(&pid_rec),
        )?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid_rec,
                "revision": rec_sources["revision"],
                "sources": [{ "filename": "reclaim-mix-flow.yaml", "source": RECLAIM_MIX_YAML }],
            }),
            Some(&pid_rec),
        )?;
        toggle_all_schedules(world, &pid_rec, false)?;

        // (a) Active carded Workflow PTY (stays `working` -> MUST survive!)
        let card_c_wf = world.client_call(
            "card.create",
            json!({ "project_id": pid_rec, "title": format!("Active Workflow PTY ({seed})") }),
            Some(&pid_rec),
        )?;
        let cid_c_wf = card_c_wf["id"].as_str().unwrap().to_owned();
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "reclaim-mix-flow",
                "card_id": cid_c_wf,
                "entry": "start",
                "request_key": format!("s18-c-wf-{seed}"),
            }),
            Some(&pid_rec),
        )?;

        // (b) Completed/idle cardless Cron PTY (MUST be reclaimed!)
        let _ = world.client_call(
            "schedule.trigger",
            json!({
                "project_id": pid_rec,
                "workflow_id": "reclaim-mix-flow",
                "rule_id": "cron_cardless_reclaim",
            }),
            Some(&pid_rec),
        )?;

        // (c) Interactive PTY protected by `viewer.presence` (idle, but viewer-protected -> MUST survive!)
        let card_c_view = world.client_call(
            "card.create",
            json!({ "project_id": pid_rec, "title": format!("Viewer Protected PTY ({seed})") }),
            Some(&pid_rec),
        )?;
        let cid_c_view = card_c_view["id"].as_str().unwrap().to_owned();
        let open_view = world.client_call(
            "agent.open",
            json!({
                "card_id": cid_c_view,
                "profile_id": "codex",
                "request_key": format!("s18-c-view-{seed}"),
            }),
            Some(&pid_rec),
        )?;
        let view_tid = strip_host(open_view["terminal_id"].as_str().unwrap());

        world.wait_until(12_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            let wf_up = snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == cid_c_wf && a.state == "running" && a.terminal.is_some()
                })
            });
            let view_up = snap.terminals.iter().any(|t| {
                t.id == view_tid
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.state == "running" && a.terminal.is_some())
            });
            let cron_up = snap.scheduled_runs.iter().any(|sr| {
                sr.rule_id == "cron_cardless_reclaim"
                    && sr.terminal_id.as_deref().is_some_and(|tid| {
                        snap.terminals.iter().any(|t| {
                            t.id == tid
                                && t.runtime
                                    .as_ref()
                                    .is_some_and(|r| r.state == "running" && r.terminal.is_some())
                        })
                    })
            });
            Ok(wf_up && view_up && cron_up)
        })?;

        let (wf_tid, wf_addr, cron_run_id, cron_tid, cron_addr, view_addr) = {
            let snap = world.server("local").unwrap().snapshot()?;
            let t_wf = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == cid_c_wf))
                .unwrap();
            let sr = snap
                .scheduled_runs
                .iter()
                .find(|r| r.rule_id == "cron_cardless_reclaim")
                .unwrap();
            let c_tid = sr.terminal_id.clone().unwrap();
            let t_cron = snap.terminals.iter().find(|t| t.id == c_tid).unwrap();
            let t_view = snap.terminals.iter().find(|t| t.id == view_tid).unwrap();
            (
                t_wf.id.clone(),
                t_wf.agent.as_ref().unwrap().terminal.clone().unwrap(),
                sr.id.clone(),
                c_tid,
                t_cron.runtime.as_ref().unwrap().terminal.clone().unwrap(),
                t_view.agent.as_ref().unwrap().terminal.clone().unwrap(),
            )
        };
        let herdr_local = world.herdr("local").unwrap();

        // Mark (a) actively working, (b) completed & idle, (c) idle & viewer-protected!
        herdr_local.set_status(&wf_addr, "working", Some(1));
        let _ = world.server_call(
            "local",
            "execution.report",
            json!({
                "schedule_run_id": cron_run_id,
                "state": "completed",
                "message": "Cardless cron done; ready for idle reclaim",
                "request_key": format!("s18-c-cron-done-{seed}"),
            }),
        )?;
        herdr_local.set_status(&cron_addr, "idle", Some(2));

        herdr_local.set_status(&view_addr, "idle", Some(2));
        herdr_local.set_viewer_activity(&view_addr, 1, true);
        let _ = world.client_call(
            "viewer.presence",
            json!({
                "client_id": "ui-viewer-mix",
                "card_id": cid_c_view,
                "terminal_id": view_tid,
            }),
            Some(&pid_rec),
        )?;
        world.advance(Duration::from_millis(300))?;
        oracle.protect_terminal(world, "local-server", &wf_addr);
        oracle.protect_terminal(world, "local-server", &view_addr);

        // Advance past idle_minutes (5 x 15s = 75s) renewing viewer presence on (c) and keeping (a) working!
        for _ in 0..5 {
            herdr_local.set_status(&wf_addr, "working", Some(1));
            herdr_local.set_viewer_activity(&view_addr, 1, true);
            let _ = world.client_call(
                "viewer.presence",
                json!({
                    "client_id": "ui-viewer-mix",
                    "card_id": cid_c_view,
                    "terminal_id": view_tid,
                }),
                Some(&pid_rec),
            )?;
            world.advance(Duration::from_secs(15))?;
            oracle.check_step_invariants(world)?;
        }

        // Assert: (b) cardless cron PTY WAS reclaimed, while (a) active Workflow PTY & (c) viewer-protected PTY stayed ALIVE!
        {
            let snap = world.server("local").unwrap().snapshot()?;
            let cron_finished = snap
                .terminals
                .iter()
                .find(|t| t.id == cron_tid)
                .is_none_or(|t| t.runtime.as_ref().is_none_or(|r| r.state == "finished"));
            ensure!(
                cron_finished,
                "Idle completed cardless Cron PTY {cron_tid} was NOT reclaimed past idle threshold!"
            );
            let wf_live = herdr_local
                .terminal_record(&wf_addr)
                .is_some_and(|r| r["state"].as_str() == Some("live"));
            let view_live = herdr_local
                .terminal_record(&view_addr)
                .is_some_and(|r| r["state"].as_str() == Some("live"));
            ensure!(
                wf_live,
                "Active Workflow PTY {wf_addr} was wrongly reclaimed!"
            );
            ensure!(
                view_live,
                "viewer.presence-protected interactive PTY {view_addr} was wrongly reclaimed!"
            );
        }
        world.hit("sched_contention.mixed_pty_reclaim");

        // Cleanly unprotect and finish (a) & (c) before Slice D.
        oracle.unprotect_terminal("local-server", &wf_addr);
        oracle.unprotect_terminal("local-server", &view_addr);
        herdr_local.set_viewer_activity(&view_addr, 0, false);
        let _ = world.client_call(
            "viewer.presence",
            json!({ "client_id": "ui-viewer-mix", "card_id": null, "terminal_id": null }),
            Some(&pid_rec),
        )?;
        let _ = world.server_call(
            "local",
            "execution.report",
            json!({
                "terminal_id": wf_tid,
                "state": "completed",
                "message": "Slice C workflow done",
                "request_key": format!("s18-c-wf-done-{seed}"),
            }),
        )?;
        herdr_local.finish_terminal(&wf_addr, 0, "exited");
        let _ = world.server_call(
            "local",
            "agent.end",
            json!({ "terminal_id": view_tid, "request_key": format!("s18-c-view-end-{seed}") }),
        )?;
        herdr_local.finish_terminal(&view_addr, 0, "exited");
        world.advance(Duration::from_millis(400))?;

        // =====================================================================
        // SLICE D — All 5 Dual-Write Failpoints + Storage Faults + Framed Local Restart
        //           + RYW + At-Most-Once + Natural Quiesce
        // =====================================================================
        let fp_local = world.failpoints("local");
        let fp_remote = world.failpoints("remote-a");
        let fp_client = world.client_failpoints(PRIMARY_CLIENT);

        // --- Failpoint #1: FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB (local) ---
        fp_local.arm(
            FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB,
            FailpointAction::FailOnce("s18-fp1-after-terminal-run".into()),
        );
        let fp1_term = world.client_call(
            "terminal.create",
            json!({
                "host_id": "local",
                "cwd": local_dir,
                "request_key": format!("s18-fp1-term-{seed}"),
            }),
            Some(&pid_local),
        )?;
        ensure!(
            fp1_term["runtime"]["state"] == "unknown",
            "Expected FP #1 to mark terminal runtime state='unknown', got {fp1_term}"
        );
        ensure!(
            fp_local
                .stats_for(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB)
                .fired
                >= 1,
            "FP #1 must have fired"
        );
        let fp1_tid = fp1_term["id"].as_str().unwrap().to_owned();
        // Close unknown terminal -> discovers & kills orphan PTY cleanly!
        let _ = world.client_call(
            "terminal.close",
            json!({ "terminal_id": fp1_tid, "request_key": format!("s18-fp1-close-{seed}") }),
            Some(&pid_local),
        )?;

        // --- Failpoint #2: FP_HERDR_AFTER_SEND_BEFORE_DB (local) ---
        let card_d2 = world.client_call(
            "card.create",
            json!({ "project_id": pid_rec, "title": format!("FP2 Mention Card ({seed})") }),
            Some(&pid_rec),
        )?;
        let cid_d2 = card_d2["id"].as_str().unwrap().to_owned();
        oracle.observe_write_receipt(PRIMARY_CLIENT, "local", &card_d2)?;
        let open_d2 = world.client_call(
            "agent.open",
            json!({
                "card_id": cid_d2,
                "profile_id": "developer",
                "request_key": format!("s18-fp2-open-{seed}"),
            }),
            Some(&pid_rec),
        )?;
        let d2_tid = strip_host(open_d2["terminal_id"].as_str().unwrap());
        world.wait_until(10_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == d2_tid
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.state == "running" && a.connection_state == "connected")
            }))
        })?;
        let d2_addr = world
            .server("local")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.id == d2_tid)
            .and_then(|t| t.agent)
            .and_then(|a| a.terminal)
            .unwrap();
        let sends_before_fp2 = herdr_local
            .sent_inputs()
            .iter()
            .filter(|s| s.terminal == d2_addr)
            .count();

        fp_local.arm(
            FP_HERDR_AFTER_SEND_BEFORE_DB,
            FailpointAction::FailOnce("s18-fp2-after-send".into()),
        );
        let fp2_comment = format!("@developer verify fp2 delivery crash seed={seed}");
        let fp2_comment_key = format!("s18-fp2-comment-{seed}");
        let _ = world.client_call(
            "comment.add",
            json!({
                "card_id": cid_d2,
                "body": fp2_comment,
                "request_key": fp2_comment_key,
            }),
            Some(&pid_rec),
        )?;
        oracle.record_request_key_attempt_with_marker(
            "local",
            &fp2_comment_key,
            "comment.add",
            true,
            fp2_comment.clone(),
        );
        world.wait_until(10_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap
                .questions
                .iter()
                .any(|q| q.card_id == cid_d2 && q.state == "delivery_unknown"))
        })?;
        // Advance time and assert no duplicate resend occurs!
        world.advance(Duration::from_secs(2))?;
        let sends_after_fp2 = herdr_local
            .sent_inputs()
            .iter()
            .filter(|s| s.terminal == d2_addr)
            .count();
        ensure!(
            sends_after_fp2 == sends_before_fp2 + 1,
            "FP #2 delivery_unknown must never double-send (before={sends_before_fp2}, after={sends_after_fp2})"
        );
        let fp2_qid = world
            .server("local")
            .unwrap()
            .snapshot()?
            .questions
            .into_iter()
            .find(|q| q.card_id == cid_d2 && q.state == "delivery_unknown")
            .unwrap()
            .id;
        let _ = world.client_call(
            "agent.reply",
            json!({
                "card_id": cid_d2,
                "question_id": fp2_qid,
                "body": "Reconciled FP2 delivery_unknown cleanly",
                "request_key": format!("s18-fp2-reply-{seed}"),
            }),
            Some(&pid_rec),
        )?;
        let _ = world.server_call(
            "local",
            "agent.end",
            json!({ "terminal_id": d2_tid, "request_key": format!("s18-fp2-end-{seed}") }),
        )?;
        herdr_local.finish_terminal(&d2_addr, 0, "exited");

        // --- Failpoint #3: FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB (remote-a) ---
        let gh_repo = "acme/canban-sim";
        world.github.add_repo(gh_repo, 8800 + (seed % 1000), "main");
        world
            .github
            .set_collaborators(gh_repo, vec!["alice".into(), "bot".into()]);
        let remote_dir = world.project_dir(&format!("sched-remote-pr-{seed}"))?;
        let exec_remote = world
            .exec_runner("remote-a")
            .context("remote-a SimExec unavailable")?;
        exec_remote.register_git_repo(&remote_dir, SimGitRepo::default());

        let proj_remote = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-a",
                "name": format!("Remote PR FP3 {seed}"),
                "directory": remote_dir,
                "target_branch": "main",
            }),
            None,
        )?;
        let pid_remote = proj_remote["id"].as_str().unwrap().to_owned();
        let rem_sources = world.client_call(
            "workflow.sources",
            json!({ "project_id": pid_remote }),
            Some(&pid_remote),
        )?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid_remote,
                "revision": rem_sources["revision"],
                "sources": [{ "filename": "remote-pr-fp-flow.yaml", "source": REMOTE_PR_FLOW_YAML }],
            }),
            Some(&pid_remote),
        )?;
        let _ = world.client_call(
            "project.github",
            json!({
                "project_id": pid_remote,
                "repository": gh_repo,
                "intake": "off",
                "delivery_mode": "github_pr",
            }),
            Some(&pid_remote),
        )?;
        let raw_pid_remote = strip_host(&pid_remote);
        world.wait_until(15_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .projects
                .iter()
                .find(|p| p.id == raw_pid_remote)
                .and_then(|p| p.github.as_ref())
                .and_then(|g| g.synced_at)
                .is_some())
        })?;

        let card_d3 = world.client_call(
            "card.create",
            json!({ "project_id": pid_remote, "title": format!("FP3 GitHub PR Card ({seed})") }),
            Some(&pid_remote),
        )?;
        let cid_d3 = card_d3["id"].as_str().unwrap().to_owned();
        let raw_cid_d3 = strip_host(&cid_d3);
        oracle.observe_write_receipt(PRIMARY_CLIENT, "remote-a", &card_d3)?;
        // Launch review.run_agent to prepare the real worktree on remote-a, commit work, then finish PTY.
        let _ = world.client_call(
            "review.run_agent",
            json!({
                "card_id": cid_d3,
                "profile_id": "developer",
                "focus": "Prepare worktree for FP3 PR publish",
            }),
            Some(&pid_remote),
        )?;
        world.wait_until(12_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .cards
                .iter()
                .find(|c| c.id == raw_cid_d3)
                .is_some_and(|c| c.worktree.is_some())
                && snap.terminals.iter().any(|t| {
                    t.agent.as_ref().is_some_and(|a| {
                        a.card_id == raw_cid_d3 && a.state == "running" && a.terminal.is_some()
                    })
                }))
        })?;
        let (d3_wt, d3_branch, d3_addr) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let c = snap.cards.iter().find(|c| c.id == raw_cid_d3).unwrap();
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid_d3))
                .unwrap();
            (
                c.worktree.clone().unwrap(),
                c.branch.clone().unwrap(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
            )
        };
        let d3_head = exec_remote.commit_in_worktree(&d3_wt, "feat: fp3 pr commit")?;
        world.github.set_branch_head(gh_repo, &d3_branch, &d3_head);
        world
            .herdr("remote-a")
            .unwrap()
            .finish_terminal(&d3_addr, 0, "exited");
        world.advance(Duration::from_millis(300))?;

        let d3_preview = world.client_call(
            "review.preview",
            json!({ "card_id": cid_d3 }),
            Some(&pid_remote),
        )?;
        let d3_token = d3_preview["token"].as_str().unwrap().to_owned();
        let d3_rev = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .iter()
            .find(|c| c.id == raw_cid_d3)
            .unwrap()
            .revision;

        fp_remote.arm(
            FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB,
            FailpointAction::FailOnce("s18-fp3-after-pr-write".into()),
        );
        let pub_res = world.client_call(
            "github.pr.publish",
            json!({
                "card_id": cid_d3,
                "revision": d3_rev,
                "token": d3_token,
                "request_key": format!("s18-fp3-pub-{seed}"),
            }),
            Some(&pid_remote),
        )?;
        ensure!(
            pub_res["state"] == "unknown",
            "Expected FP #3 github.pr.publish to return state='unknown', got {pub_res}"
        );
        ensure!(
            fp_remote
                .stats_for(FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB)
                .fired
                >= 1,
            "FP #3 must have fired"
        );
        // Reconcile via github.pr.refresh: discovers PR without duplicating it & clears stale attention!
        let refreshed_pr = world.client_call(
            "github.pr.refresh",
            json!({ "card_id": cid_d3 }),
            Some(&pid_remote),
        )?;
        ensure!(
            refreshed_pr["number"].as_u64().unwrap_or(0) >= 1 && refreshed_pr["state"] == "open",
            "Expected github.pr.refresh to reconcile PR cleanly after FP #3, got {refreshed_pr}"
        );

        // --- Failpoint #4: FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE (local) ---
        exec_local.enqueue_shell_outcomes(
            "sim-contention-test",
            [SimShellOutcome::fail(
                1,
                "running fp4 tests\n",
                "fp4_gate FAILED attempt 1\n",
            )],
        );
        let card_d4 = world.client_call(
            "card.create",
            json!({ "project_id": pid_local, "title": format!("FP4 Git Push Batch Card ({seed})") }),
            Some(&pid_local),
        )?;
        let cid_d4 = card_d4["id"].as_str().unwrap().to_owned();
        oracle.observe_write_receipt(PRIMARY_CLIENT, "local", &card_d4)?;
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "contention-flow",
                "card_id": cid_d4,
                "entry": "start",
                "request_key": format!("s18-fp4-start-{seed}"),
            }),
            Some(&pid_local),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == cid_d4 && a.state == "running" && a.terminal.is_some()
                })
            }))
        })?;
        let (d4_tid, d4_addr, d4_wt) = {
            let snap = world.server("local").unwrap().snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == cid_d4))
                .unwrap();
            let c = snap.cards.iter().find(|c| c.id == cid_d4).unwrap();
            (
                t.id.clone(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
                c.worktree.clone().unwrap(),
            )
        };
        let _ = exec_local.commit_in_worktree(&d4_wt, "feat: fp4 batch commit")?;
        let _ = world.server_call(
            "local",
            "execution.report",
            json!({
                "terminal_id": d4_tid,
                "state": "completed",
                "message": "Card D4 ready for FP4 batch push",
                "request_key": format!("s18-fp4-exec-done-{seed}"),
            }),
        )?;
        herdr_local.finish_terminal(&d4_addr, 0, "exited");
        world.wait_until(15_000, |w| {
            let snap = w.server("local").unwrap().snapshot()?;
            Ok(snap.merge_batches.iter().any(|b| {
                b.id != slice_a_batch_id
                    && b.active()
                    && b.state == "running"
                    && Path::new(&b.worktree).is_dir()
            }))
        })?;
        let (fp4_batch_id, fp4_batch_wt) = {
            let snap = world.server("local").unwrap().snapshot()?;
            let b = snap
                .merge_batches
                .iter()
                .find(|b| b.id != slice_a_batch_id && b.active() && b.state == "running")
                .unwrap();
            (b.id.clone(), b.worktree.clone())
        };
        oracle.record_committed_merge_batch("local-server", &fp4_batch_id, &pid_local);
        let _ = exec_local.resolve_worktree_and_commit(&fp4_batch_wt)?;

        // Arm FP #4 right before `batch.report(completed)` -> git push succeeds, SQLite finalize fails!
        let fp4_rkey = format!("s18-fp4-batch-report-{seed}");
        fp_local.arm(
            FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE,
            FailpointAction::FailOnce("s18-fp4-after-git-push".into()),
        );
        let fp4_err = world
            .server_call(
                "local",
                "batch.report",
                json!({
                    "batch_id": fp4_batch_id,
                    "state": "completed",
                    "message": "Resolve FP4 batch",
                    "request_key": fp4_rkey,
                }),
            )
            .expect_err("FP #4 must abort batch.report right after git push");
        ensure!(
            format!("{fp4_err:#}").contains(FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE),
            "Unexpected FP #4 error: {fp4_err:#}"
        );
        oracle.record_request_key_attempt("local", &fp4_rkey, "batch.report", false);
        let pushed_head = exec_local.head_sha(&local_dir).unwrap();
        ensure!(
            exec_local
                .remote_ref_sha(&local_dir, "origin", "main")
                .as_deref()
                == Some(pushed_head.as_str()),
            "FP #4 must fire AFTER git push updated origin/main"
        );
        oracle.check_at_most_once(world)?;

        // Retry exact same `batch.report` with identical `request_key` -> converges idempotently!
        let fp4_retry = world.server_call(
            "local",
            "batch.report",
            json!({
                "batch_id": fp4_batch_id,
                "state": "completed",
                "message": "Resolve FP4 batch",
                "request_key": fp4_rkey,
            }),
        )?;
        ensure!(
            fp4_retry["accepted"].as_bool() == Some(true)
                && fp4_retry["delivered"].as_u64() == Some(1),
            "FP #4 idempotent batch.report retry failed: {fp4_retry}"
        );
        oracle.record_request_key_attempt("local", &fp4_rkey, "batch.report", true);
        ensure!(
            exec_local
                .remote_ref_sha(&local_dir, "origin", "main")
                .as_deref()
                == Some(pushed_head.as_str()),
            "FP #4 idempotent retry must not add duplicate commits"
        );
        if let Some(term) = world
            .server("local")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.id == fp4_batch_id)
            && let Some(addr) = term.runtime.and_then(|r| r.terminal)
        {
            herdr_local.finish_terminal(&addr, 0, "exited");
        }

        // --- Failpoint #5: FP_OUTBOX_MID_WRITE_TRUNCATE (ui-client) ---
        world.crash_server("remote-a")?;
        world.partition(PRIMARY_CLIENT, "remote-a");
        world.wait_until(10_000, |w| {
            Ok(w.client_host_summaries()
                .iter()
                .any(|s| s.id == "remote-a" && !s.is_connected()))
        })?;
        fp_client.arm(
            FP_OUTBOX_MID_WRITE_TRUNCATE,
            FailpointAction::FailOnce("s18-fp5-outbox-truncate".into()),
        );
        let fp5_err = world
            .client_call(
                "card.create",
                json!({
                    "project_id": pid_remote,
                    "title": format!("FP5 Torn Outbox Card ({seed})"),
                    "request_key": format!("s18-fp5-outbox-{seed}"),
                }),
                Some(&pid_remote),
            )
            .expect_err("Offline remote write with armed FP #5 must return truncation error");
        ensure!(
            format!("{fp5_err:#}").contains(FP_OUTBOX_MID_WRITE_TRUNCATE),
            "Expected FP #5 error string, got: {fp5_err:#}"
        );
        ensure!(
            fp_client.stats_for(FP_OUTBOX_MID_WRITE_TRUNCATE).fired >= 1,
            "FP #5 must have fired"
        );

        // Bounce client to quarantine torn `.outbox.json` and recover cleanly!
        world.bounce_client()?;
        ensure!(
            world.client_sync_items().is_empty(),
            "Quarantined torn outbox must not leave wedged sync items"
        );
        world.repair(PRIMARY_CLIENT, "remote-a");
        world.bounce_server("remote-a")?;

        // --- Storage Faults (Busy on local + IoError on remote-a) + At-Most-Once + RYW ---
        let busy_key = format!("s18-busy-card-{seed}");
        let busy_title = format!("Storage Busy Retry Card ({seed})");
        world
            .storage_faults("local")
            .unwrap()
            .inject_next(StorageFaultKind::Busy);
        let busy_err = world
            .client_call(
                "card.create",
                json!({
                    "project_id": pid_local,
                    "title": busy_title,
                    "request_key": busy_key,
                }),
                Some(&pid_local),
            )
            .expect_err("Injected SQLITE_BUSY must fail cleanly");
        ensure!(
            format!("{busy_err:#}").contains("SQLITE_BUSY"),
            "Expected SQLITE_BUSY, got: {busy_err:#}"
        );
        oracle.record_request_key_attempt_with_marker(
            "local",
            &busy_key,
            "card.create",
            false,
            busy_title.clone(),
        );
        let busy_ok = world.client_call(
            "card.create",
            json!({
                "project_id": pid_local,
                "title": busy_title,
                "request_key": busy_key,
            }),
            Some(&pid_local),
        )?;
        oracle.observe_write_receipt(PRIMARY_CLIENT, "local", &busy_ok)?;
        oracle.record_committed_card(
            "local-server",
            busy_ok["id"].as_str().unwrap(),
            &pid_local,
            busy_ok["revision"].as_u64().unwrap_or(1),
        );
        oracle.record_request_key_attempt_with_marker(
            "local",
            &busy_key,
            "card.create",
            true,
            busy_title,
        );

        // Wait for natural unassisted reconnect of `"remote-a"` after partition repair + bounce_client!
        world.quiesce_natural(25_000)?;
        world.hit("sched_contention.natural_quiesce");

        // Inject SQLITE_IOERR on `"remote-a"` + idempotent retry + replay!
        let io_key = format!("s18-ioerr-card-{seed}");
        let io_id = format!("s18-ioerr-id-{seed}");
        let io_title = format!("Storage IOERR Retry Card ({seed})");
        let io_payload = json!({
            "id": io_id,
            "project_id": pid_remote,
            "title": io_title,
            "request_key": io_key,
        });
        world
            .storage_faults("remote-a")
            .unwrap()
            .inject_next(StorageFaultKind::IoError);
        let io_err = world
            .client_call("card.create", io_payload.clone(), Some(&pid_remote))
            .expect_err("Injected SQLITE_IOERR must fail cleanly");
        ensure!(
            format!("{io_err:#}").contains("SQLITE_IOERR"),
            "Expected SQLITE_IOERR, got: {io_err:#}"
        );
        oracle.record_request_key_attempt_with_marker(
            "remote-a",
            &io_key,
            "card.create",
            false,
            io_title.clone(),
        );
        let io_ok = world.client_call("card.create", io_payload, Some(&pid_remote))?;
        oracle.observe_write_receipt(PRIMARY_CLIENT, "remote-a", &io_ok)?;
        oracle.record_committed_card(
            "remote-a",
            io_ok["id"].as_str().unwrap(),
            &pid_remote,
            io_ok["revision"].as_u64().unwrap_or(1),
        );
        oracle.record_request_key_attempt_with_marker(
            "remote-a",
            &io_key,
            "card.create",
            true,
            io_title,
        );

        // Framed local restart (`bounce_server("local")`) with an in-flight framed RPC + reconnect check!
        let inflight_local = world.client_spawn(
            "card.update",
            json!({
                "card_id": busy_ok["id"],
                "revision": busy_ok["revision"],
                "body": format!("Framed Local Mid-Bounce Update ({seed})"),
                "request_key": format!("s18-local-mid-bounce-{seed}"),
            }),
            Some(&pid_local),
        )?;
        world.bounce_server("local")?;
        if let Ok(upd) = world.await_call(inflight_local, 10_000) {
            oracle.observe_write_receipt(PRIMARY_CLIENT, "local", &upd)?;
        }
        // Post-bounce framed local RPC must succeed cleanly on the restarted local listener!
        let post_bounce_ping =
            world.client_call("app.ping", json!({ "host_id": "local" }), None)?;
        ensure!(
            post_bounce_ping["server_id"].as_str().is_some(),
            "Post-bounce framed local app.ping failed: {post_bounce_ping}"
        );

        // Final natural quiesce + full oracle checks (RYW, At-Most-Once, SQLite, durability, convergence)!
        world.quiesce_natural(20_000)?;
        oracle.check_read_your_writes(world)?;
        oracle.check_at_most_once(world)?;
        oracle.check_quiescent_invariants(world)?;

        // Verify ALL 5 dual-write failpoints have armed >= 1 && fired >= 1 and 0 vacuous armed faults!
        for &fp_name in ALL_FAILPOINTS {
            let mut total_armed = 0u64;
            let mut total_fired = 0u64;
            for srv in world.servers.values() {
                let st = srv.failpoints.stats_for(fp_name);
                total_armed += st.armed;
                total_fired += st.fired;
            }
            for cli in world.clients.values() {
                let st = cli.failpoints.stats_for(fp_name);
                total_armed += st.armed;
                total_fired += st.fired;
            }
            ensure!(
                total_armed >= 1 && total_fired >= 1,
                "Failpoint '{fp_name}' must be both armed and fired (armed={total_armed}, fired={total_fired})"
            );
        }
        world.hit("sched_contention.all_five_failpoints");

        // Zero leaked OS commands on all hosts!
        for host in ["local-server", "remote-a"] {
            if let Some(runner) = world.exec_runner(host) {
                ensure!(
                    runner.blocked_commands().is_empty(),
                    "scheduler_cross_contention leaked blocked OS commands on {host}: {:?}",
                    runner.blocked_commands()
                );
            }
        }

        // Final heal_and_quiesce pass (verifying clean failpoint clearing & CLI coverage gate
        // after unassisted `quiesce_natural` and all 5 failpoint assertions already succeeded).
        world.heal_and_quiesce(5_000)?;
        oracle.check_quiescent_invariants(world)?;

        Ok(finalize_digest(world))
    })
}
