//! Scenario #6 — Herdr terminal event lifecycle:
//! out-of-order / stale-seq rejection, missed-snapshot reconciliation, subscriber disconnects,
//! Herdr Core restart (`core.id` change), `terminal.read`/`execution.output` capture,
//! `herdr.set_offline(true)` outage recovery, `AppServer` `bounce_server` live-terminal reattach,
//! `max_agents` slot release on `blocked`/`idle`, UI terminal-to-project + close-fallback invariants,
//! short-burst activity drop (<2s) & non-agent broadcast suppression, `overview.list` + saved-output
//! preview while Herdr core is offline, authoritative missing terminal handling, PTY-touch followup
//! cooldown, cardless scheduled presence grace, and transient merge-marker / no-history-deserialize
//! cursor checks.
use crate::{
    exec::{SimGitRepo, SimShellOutcome},
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Context, Result, ensure};
use canban_notifications::OverviewRow;
use canban_types::{ActivityKind, ActivitySegment, AppEvent, RuntimeState};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const OVERVIEW_GATE_YAML: &str = r#"schema: 12
id: overview-gate-flow
name: Overview Gate Flow
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
  enter_develop:
    event: card.column_entered
    match:
      column: develop
    start_at: work
steps:
  - id: work
    stage: develop
    uses: agent
    with:
      profile: developer
      instruction: Work and wait for review
    next: gate
  - id: gate
    stage: review
    uses: approval
    with:
      title: Review report
    next: finish
  - id: finish
    stage: review
    uses: complete
"#;

const TRANSIENT_MERGE_YAML: &str = r#"schema: 12
id: transient-merge-flow
name: Transient Merge Flow
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
    start_at: deliver
steps:
  - id: deliver
    stage: review
    uses: deliver
    with:
      target_branch: main
      strategy: rebase
      sync_upstream: false
      batch_size: 1
      test_command: sim-merge-marker-test
    next: done
  - id: done
    stage: review
    uses: complete
"#;

const AGENT_YAML: &str = r#"schema: 12
id: herdr-test
name: Herdr Lifecycle Flow
workspace:
  create_worktree: false
  sync_base: false
on:
  start:
    event: manual
    start_at: work
steps:
  - id: work
    uses: agent
    with:
      profile: codex
      instruction: Run simulated agent
    next: finish
  - id: finish
    uses: complete
"#;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "herdr_lifecycle");
    cfg.remote_hosts = vec!["remote-a".into()];

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        let dir = world.project_dir("herdr-life")?;
        let proj = world.client_call(
            "project.create",
            json!({ "host_id": "remote-a", "name": "Herdr Proj", "directory": dir }),
            None,
        )?;
        let pid = proj["id"].as_str().unwrap().to_owned();
        let raw_pid = canban_types::split_qualified_host_id(&pid)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| pid.clone());

        let sources =
            world.client_call("workflow.sources", json!({ "project_id": pid }), Some(&pid))?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid,
                "revision": sources["revision"],
                "sources": [{ "filename": "herdr-test.yaml", "source": AGENT_YAML }],
            }),
            Some(&pid),
        )?;

        let card = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": "Herdr Resilience Card" }),
            Some(&pid),
        )?;
        let cid = card["id"].as_str().unwrap().to_owned();
        let raw_cid = canban_types::split_qualified_host_id(&cid)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| cid.clone());

        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "herdr-test",
                "card_id": cid,
                "entry": "start",
                "request_key": format!("herdr-start-{seed}"),
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
            let snap = world.server("remote-a").unwrap().snapshot()?;
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
        let herdr = world.herdr("remote-a").unwrap();

        // 1. Real-time working -> idle -> working transitions.
        herdr.set_status(&herdr_addr, "idle", Some(1));
        world.wait_until(5_000, |w| {
            let srv = w.server("remote-a").unwrap();
            let now = srv.now_ms();
            let snap = srv.snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == tid
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.runtime_status_at(now) == "idle")
            }))
        })?;

        herdr.set_status(&herdr_addr, "working", Some(2));
        world.wait_until(5_000, |w| {
            let srv = w.server("remote-a").unwrap();
            let now = srv.now_ms();
            let snap = srv.snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == tid
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.runtime_status_at(now) == "working")
            }))
        })?;
        oracle.check_step_invariants(world)?;

        // 2. Output capture via `terminal.read` / `execution.output` routed through `MultiHostClient`.
        herdr.append_output(&herdr_addr, ["sim-log-line-1", "sim-log-line-2"]);
        herdr.set_screen(&herdr_addr, "screen-snapshot-ready");
        let q_tid = canban_types::qualify_host_id("remote-a", &tid);
        let output_res = world.client_call(
            "execution.output",
            json!({ "terminal_id": q_tid }),
            Some(&pid),
        )?;
        let out_text = output_res.to_string();
        ensure!(
            out_text.contains("sim-log-line-1") && out_text.contains("sim-log-line-2"),
            "execution.output did not return SimHerdr terminal.read lines: {out_text}"
        );
        world.hit("herdr.output_capture");

        // 3. Stale/out-of-order seq event injection -> MUST be ignored!
        let current_seq = herdr.seq();
        herdr.inject_stale_event(&herdr_addr, current_seq.saturating_sub(2).max(1), "error");
        world.advance(Duration::from_millis(300))?;
        {
            let srv = world.server("remote-a").unwrap();
            let now = srv.now_ms();
            let snap = srv.snapshot()?;
            let st = snap
                .terminals
                .iter()
                .find(|t| t.id == tid)
                .and_then(|t| t.agent.as_ref())
                .map(|a| a.runtime_status_at(now));
            ensure!(
                st == Some("working"),
                "Stale seq event overwrote newer runtime state! Expected working, got {st:?}"
            );
        }

        // 4. Subscriber drop -> automatic reconnect + missed snapshot recovery!
        herdr.drop_subscribers();
        world.advance(Duration::from_millis(100))?;
        herdr.set_status(&herdr_addr, "done", Some(3));
        herdr.emit_missed_snapshot();
        world.wait_until(10_000, |w| {
            let srv = w.server("remote-a").unwrap();
            let now = srv.now_ms();
            let snap = srv.snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == tid
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.runtime_status_at(now) == "done")
            }))
        })?;
        oracle.check_step_invariants(world)?;

        // 5. `herdr.set_offline(true)` outage -> AppServer stays healthy -> restore online!
        herdr.set_offline(true);
        world.advance(Duration::from_secs(2))?;
        oracle.check_step_invariants(world)?;
        herdr.set_offline(false);
        herdr.set_status(&herdr_addr, "idle", Some(4));
        world.wait_until(10_000, |w| {
            let srv = w.server("remote-a").unwrap();
            let now = srv.now_ms();
            let snap = srv.snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == tid
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.runtime_status_at(now) == "idle")
            }))
        })?;
        world.hit("herdr.offline_recovery");

        // 6. `AppServer` `bounce_server("remote-a")` while Herdr terminal remains live ->
        //    reattaches cleanly without duplicating or killing the live Herdr terminal!
        world.bounce_server("remote-a")?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let rt = w.server("remote-a").unwrap().runtime_state()?;
            Ok(snap
                .terminals
                .iter()
                .any(|t| t.id == tid && t.agent.as_ref().is_some_and(|a| !a.finished()))
                && rt.terminals.iter().any(|t| t.id == tid))
        })?;
        ensure!(
            herdr.terminal_addresses().contains(&herdr_addr),
            "AppServer bounce must leave live Herdr PTY intact"
        );
        world.hit("herdr.appserver_bounce_reattach");

        // 7. Herdr Core restart (new `core.id`, all live PTYs vanish) -> AppServer reconciles
        //    existing terminal to `connection_state == "replaced"`!
        herdr.restart_core(format!("sim-core-restarted-{seed}"));
        world.wait_until(15_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == tid
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.connection_state == "replaced")
            }))
        })?;
        oracle.check_step_invariants(world)?;

        // 8. `max_agents` slot release: Card 1's `replaced` terminal reserves 1 uncertain slot,
        //    so setting `max_agents: 2` leaves 1 remaining slot for `c_slot1` while `c_slot2`
        //    queues until `c_slot1` transitions to `blocked` and releases its slot!
        let _ = world.client_call(
            "scheduler.configure",
            json!({ "max_agents": 2 }),
            Some(&pid),
        )?;
        let c_slot1 = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": "Slot Holder 1" }),
            Some(&pid),
        )?;
        let c_slot2 = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": "Slot Waiter 2" }),
            Some(&pid),
        )?;
        let cid_s1 = c_slot1["id"].as_str().unwrap().to_owned();
        let cid_s2 = c_slot2["id"].as_str().unwrap().to_owned();
        let raw_s1 = canban_types::split_qualified_host_id(&cid_s1)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| cid_s1.clone());
        let raw_s2 = canban_types::split_qualified_host_id(&cid_s2)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| cid_s2.clone());

        let _ = world.client_call(
            "workflow.start",
            json!({ "workflow_id": "herdr-test", "card_id": cid_s1, "entry": "start", "request_key": format!("slot1-{seed}") }),
            Some(&pid),
        )?;
        let _ = world.client_call(
            "workflow.start",
            json!({ "workflow_id": "herdr-test", "card_id": cid_s2, "entry": "start", "request_key": format!("slot2-{seed}") }),
            Some(&pid),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == raw_s1 && a.terminal.is_some())
            }))
        })?;
        // While s1 is `working`, s2 must stay queued (never exceed max_agents=1).
        let s1_addr = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_s1))
            .unwrap()
            .agent
            .unwrap()
            .terminal
            .unwrap();
        herdr.set_status(&s1_addr, "working", Some(1));
        world.advance(Duration::from_millis(600))?;
        oracle.check_step_invariants(world)?;

        // Transition s1 to `blocked` -> releases agent slot -> s2 launches while s1 stays alive!
        herdr.set_status(&s1_addr, "blocked", Some(2));
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == raw_s2 && a.terminal.is_some())
            }))
        })?;
        world.hit("herdr.max_agents_slot_release");
        // Restore default budget for cleanup.
        let _ = world.client_call(
            "scheduler.configure",
            json!({ "max_agents": 8 }),
            Some(&pid),
        )?;

        // 9. Preserved UI terminal-to-project grouping and close-fallback invariants across
        //    2 projects + manual shell terminals (from commit 725e1c6).
        let dir2 = world.project_dir("herdr-life-p2")?;
        let proj2 = world.client_call(
            "project.create",
            json!({ "host_id": "remote-a", "name": "Herdr Proj 2", "directory": dir2 }),
            None,
        )?;
        let pid2 = proj2["id"].as_str().unwrap().to_owned();
        let raw_pid2 = canban_types::split_qualified_host_id(&pid2)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| pid2.clone());

        let sh_a1 = world.client_call(
            "terminal.create",
            json!({ "project_id": pid, "name": "Proj1 Shell A" }),
            Some(&pid),
        )?;
        let sh_a2 = world.client_call(
            "terminal.create",
            json!({ "project_id": pid, "name": "Proj1 Shell B" }),
            Some(&pid),
        )?;
        let sh_b1 = world.client_call(
            "terminal.create",
            json!({ "project_id": pid2, "name": "Proj2 Shell A" }),
            Some(&pid2),
        )?;
        let sh_a1_id = sh_a1["id"].as_str().unwrap().to_owned();
        let sh_a2_id = sh_a2["id"].as_str().unwrap().to_owned();
        let sh_b1_id = sh_b1["id"].as_str().unwrap().to_owned();

        world.wait_until(10_000, |w| {
            let merged = w.client_merged_snapshot()?;
            Ok(merged.terminals.iter().any(|t| t.id == sh_a1_id)
                && merged.terminals.iter().any(|t| t.id == sh_a2_id)
                && merged.terminals.iter().any(|t| t.id == sh_b1_id))
        })?;
        oracle.check_step_invariants(world)?;

        InvariantOracle::assert_terminal_in_project(world, &sh_a1_id, &pid)?;
        InvariantOracle::assert_terminal_in_project(world, &sh_a2_id, &pid)?;
        InvariantOracle::assert_terminal_in_project(world, &sh_b1_id, &pid2)?;

        let snap_ui = world.client_merged_snapshot()?;
        let rows = canban_ui::terminals::entries(&snap_ui);
        let empty_closing = std::collections::BTreeSet::new();
        let fb_same_project = InvariantOracle::assert_close_fallback_priority(
            &snap_ui,
            &rows,
            &empty_closing,
            &sh_a1_id,
        )?;
        let fb_id = fb_same_project
            .as_deref()
            .expect("Expected same-project fallback when closing sh_a1_id");
        let fb_row = rows
            .iter()
            .find(|e| e.saved.id == fb_id)
            .expect("Fallback terminal missing from rows");
        ensure!(
            fb_row.project_id.as_deref() == Some(pid.as_str()),
            "Expected same-project fallback in {pid}, got {fb_id} in {:?}",
            fb_row.project_id
        );

        let mut closing_a2 = std::collections::BTreeSet::new();
        closing_a2.insert(sh_a2_id.clone());
        let fb_cross_project = InvariantOracle::assert_close_fallback_priority(
            &snap_ui,
            &rows,
            &closing_a2,
            &sh_a1_id,
        )?;
        ensure!(fb_cross_project.is_some());

        for sh_id in [&sh_a1_id, &sh_a2_id, &sh_b1_id] {
            let _ =
                world.client_call("terminal.end", json!({ "terminal_id": sh_id }), Some(&pid))?;
        }

        // 10. Foreground agent push tracking (`agy` + `RuntimeWait` stability across polls),
        //     clearing on exit back to `zsh` or Herdr disconnect, canonical `terminal_contexts`
        //     sorting without spurious emits, card agent context retention, and
        //     `canban_notifications::reconcile_runtime_reminders` parity!
        {
            let sh_fg = world.client_call(
                "terminal.create",
                json!({ "project_id": pid, "name": "FG Push Shell" }),
                Some(&pid),
            )?;
            let sh_fg_id = sh_fg["id"].as_str().unwrap().to_owned();
            let raw_sh_fg_id = canban_types::split_qualified_host_id(&sh_fg_id)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| sh_fg_id.clone());
            let sh_fg_addr = sh_fg["runtime"]["terminal"].as_str().unwrap().to_owned();

            // 10a: Plain `zsh` foreground -> no observed_agent.
            herdr.set_foreground(
                &sh_fg_addr,
                Some(json!({ "pid": 100, "name": "zsh", "argv": ["/bin/zsh", "-l"], "cwd": dir })),
            );
            world.advance(Duration::from_millis(250))?;
            {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let t = snap
                    .terminals
                    .iter()
                    .find(|t| t.id == raw_sh_fg_id)
                    .unwrap();
                ensure!(t.title() == "FG Push Shell");
                ensure!(t.runtime.as_ref().unwrap().observed_agent.is_none());
            }

            // 10b: Foreground switches to `agy` + permission block -> `observed_agent = Some("agy")`,
            //      `HerdrStatus::Blocked`, `RuntimeWait` captured with stable ID & `since` across polls!
            herdr.set_foreground(
                &sh_fg_addr,
                Some(json!({ "pid": 101, "name": "agy", "argv": ["/opt/homebrew/bin/agy"], "cwd": dir })),
            );
            herdr.set_status_detailed(
                &sh_fg_addr,
                json!({
                    "state": "blocked",
                    "app": "agy",
                    "kind": "permission",
                    "title": "Allow command?",
                    "msg": "cargo test",
                }),
                Some(1),
            );
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == raw_sh_fg_id
                        && t.runtime.as_ref().is_some_and(|rt| {
                            rt.observed_agent.as_deref() == Some("agy")
                                && rt.agent_status.as_deref() == Some("blocked")
                                && rt.waiting.is_some()
                        })
                }))
            })?;
            let first_wait = {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let t = snap
                    .terminals
                    .iter()
                    .find(|t| t.id == raw_sh_fg_id)
                    .unwrap();
                ensure!(t.agent_engine.is_none());
                ensure!(t.herdr_status() == canban_types::HerdrStatus::Blocked);
                let w = t.runtime.as_ref().unwrap().waiting.clone().unwrap();
                ensure!(
                    w.kind == "permission"
                        && w.title == "Allow command?"
                        && w.message == "cargo test"
                );
                w
            };
            // Re-observing the same block keeps `waiting.id` and `waiting.since` stable!
            world.advance(Duration::from_millis(500))?;
            {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let t = snap
                    .terminals
                    .iter()
                    .find(|t| t.id == raw_sh_fg_id)
                    .unwrap();
                let second_wait = t.runtime.as_ref().unwrap().waiting.clone().unwrap();
                ensure!(
                    second_wait.id == first_wait.id && second_wait.since == first_wait.since,
                    "RuntimeWait id/since drifted across polls: {first_wait:?} vs {second_wait:?}"
                );
            }

            // 10c: Verify `notifications::reconcile_runtime_reminders_at` parity against full projection
            //      while runtime holds both `replaced` agent terminal and `blocked` shell terminal!
            {
                let srv_a = world.server("remote-a").unwrap();
                let now_a = srv_a.now_ms();
                let snap_a = srv_a.snapshot()?;
                let events_a = srv_a.events()?;
                let full_reminders =
                    canban_notifications::reminders_from_events_at(&snap_a, &events_a, now_a);
                let non_runtime_only: Vec<_> = full_reminders
                    .iter()
                    .filter(|r| !canban_notifications::is_runtime_reminder_id(&r.id))
                    .cloned()
                    .collect();
                let mut reconciled_snap = snap_a.clone();
                reconciled_snap.reminders = non_runtime_only.clone();
                canban_notifications::reconcile_runtime_reminders_at(&mut reconciled_snap, now_a);
                let full_ids: std::collections::BTreeSet<&str> =
                    full_reminders.iter().map(|r| r.id.as_str()).collect();
                let rec_ids: std::collections::BTreeSet<&str> = reconciled_snap
                    .reminders
                    .iter()
                    .map(|r| r.id.as_str())
                    .collect();
                ensure!(
                    !full_ids.is_empty() && full_ids == rec_ids,
                    "reconcile_runtime_reminders_at mismatch: full={full_ids:?} vs reconciled={rec_ids:?}"
                );
                for ev_rem in &non_runtime_only {
                    ensure!(
                        reconciled_snap.reminders.iter().any(|r| r.id == ev_rem.id),
                        "reconcile_runtime_reminders_at dropped non-runtime event reminder {}",
                        ev_rem.id
                    );
                }
                let prev_times: std::collections::BTreeMap<String, u64> = reconciled_snap
                    .reminders
                    .iter()
                    .map(|r| (r.id.clone(), r.created_at))
                    .collect();
                canban_notifications::reconcile_runtime_reminders_at(
                    &mut reconciled_snap,
                    now_a + 60_000,
                );
                for r in &reconciled_snap.reminders {
                    if !canban_notifications::is_runtime_reminder_id(&r.id) {
                        ensure!(
                            prev_times.get(&r.id) == Some(&r.created_at),
                            "Repeated reconcile_runtime_reminders_at mutated event reminder {}",
                            r.id
                        );
                    }
                }
            }

            // 10d: Agent exits back to `zsh` (even with stale OSC status `done`) -> clears all agent fields!
            herdr.set_foreground(
                &sh_fg_addr,
                Some(json!({ "pid": 100, "name": "zsh", "argv": ["/bin/zsh", "-l"], "cwd": dir })),
            );
            herdr.set_status_detailed(
                &sh_fg_addr,
                json!({ "state": "done", "app": "agy" }),
                Some(1),
            );
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == raw_sh_fg_id
                        && t.herdr_status() == canban_types::HerdrStatus::PlainShell
                        && t.runtime.as_ref().is_some_and(|rt| {
                            rt.observed_agent.is_none()
                                && rt.agent_status.is_none()
                                && rt.agent_turn.is_none()
                                && rt.waiting.is_none()
                        })
                }))
            })?;

            // 10e: Canonical sorting of `terminal_contexts` (lexicographic by `terminal_id`)
            //      without spurious emits on idle ticks!
            let sh_fg_2 = world.client_call(
                "terminal.create",
                json!({ "project_id": pid, "name": "FG Push Shell 2" }),
                Some(&pid),
            )?;
            let sh_fg_2_id = sh_fg_2["id"].as_str().unwrap().to_owned();
            world.advance(Duration::from_millis(400))?;
            {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                ensure!(
                    snap.terminal_contexts
                        .windows(2)
                        .all(|w| w[0].terminal_id < w[1].terminal_id),
                    "terminal_contexts not canonically sorted: {:?}",
                    snap.terminal_contexts
                );
            }

            // 10f: Herdr disconnect clears `observed_agent` & marks `connection == "unavailable"`.
            herdr.set_foreground(
                &sh_fg_addr,
                Some(json!({ "pid": 102, "name": "codex", "argv": ["codex"], "cwd": dir })),
            );
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == raw_sh_fg_id
                        && t.runtime
                            .as_ref()
                            .is_some_and(|rt| rt.observed_agent.as_deref() == Some("codex"))
                }))
            })?;
            herdr.set_offline(true);
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == raw_sh_fg_id
                        && t.runtime.as_ref().is_some_and(|rt| {
                            rt.connection == "unavailable" && rt.observed_agent.is_none()
                        })
                }))
            })?;
            herdr.set_offline(false);
            world.advance(Duration::from_millis(300))?;
            let _ = world.client_call(
                "terminal.close",
                json!({ "terminal_id": sh_fg_id }),
                Some(&pid),
            )?;
            let _ = world.client_call(
                "terminal.close",
                json!({ "terminal_id": sh_fg_2_id }),
                Some(&pid),
            )?;
            world.hit("herdr.foreground_push_and_reminders");
        }

        // =========================================================================
        // 11. Short-burst Activity Drop (< 2s), Non-Agent Broadcast Suppression,
        //     `overview.list` Global Rules & Saved-Output Preview While Herdr Offline,
        //     Authoritative Missing Terminal Handling, PTY-Touch Followup Cooldown,
        //     Cardless Scheduled Presence Grace, and Cursor / Transient Merge-Marker Checks.
        // =========================================================================
        {
            // Finish any lingering terminals from section 8 before section 11 assertions.
            for addr in herdr.terminal_addresses() {
                if herdr
                    .terminal_record(&addr)
                    .is_some_and(|r| r["state"] == "live")
                {
                    herdr.finish_terminal(&addr, 0, "cleanup");
                }
            }
            world.advance(Duration::from_millis(300))?;

            // 11a. Short-burst working (< 2s) dropped; >= 2s through finished/missed/removed recorded!
            let sh_burst = world.client_call(
                "terminal.create",
                json!({ "project_id": pid, "name": "Burst Shell", "cwd": dir }),
                Some(&pid),
            )?;
            let burst_id = sh_burst["id"].as_str().unwrap().to_owned();
            let raw_burst_id = canban_types::split_qualified_host_id(&burst_id)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| burst_id.clone());
            let burst_addr = sh_burst["runtime"]["terminal"].as_str().unwrap().to_owned();
            let burst_from = world.server("remote-a").unwrap().now_ms();

            // Short burst (1_200ms < 2_000ms) -> dropped!
            herdr.set_status_detailed(
                &burst_addr,
                json!({ "state": "working", "app": "claude" }),
                Some(1),
            );
            world.advance(Duration::from_millis(1_200))?;
            herdr.set_status_detailed(
                &burst_addr,
                json!({ "state": "idle", "app": "claude" }),
                Some(1),
            );
            world.advance(Duration::from_millis(500))?;
            {
                let burst_to = world.server("remote-a").unwrap().now_ms() + 1_000;
                let act = world.server_call(
                    "remote-a",
                    "insights.activity",
                    json!({ "from_ms": burst_from, "to_ms": burst_to, "kinds": ["agent"] }),
                )?;
                let segs: Vec<ActivitySegment> = serde_json::from_value(act["segments"].clone())?;
                ensure!(
                    !segs
                        .iter()
                        .any(|s| s.terminal_id.as_deref() == Some(raw_burst_id.as_str())),
                    "Working burst < 2s must be dropped, got {segs:?}"
                );
            }

            // Working >= 2s through `finished` closes and records a segment!
            herdr.set_status_detailed(
                &burst_addr,
                json!({ "state": "working", "app": "codex" }),
                Some(2),
            );
            world.advance(Duration::from_millis(3_500))?;
            herdr.finish_terminal(&burst_addr, 0, "done");
            world.advance(Duration::from_millis(500))?;
            {
                let burst_to = world.server("remote-a").unwrap().now_ms() + 1_000;
                let act = world.server_call(
                    "remote-a",
                    "insights.activity",
                    json!({ "from_ms": burst_from, "to_ms": burst_to, "kinds": ["agent"] }),
                )?;
                let segs: Vec<ActivitySegment> = serde_json::from_value(act["segments"].clone())?;
                let rec = segs
                    .iter()
                    .find(|s| {
                        s.kind == ActivityKind::Agent
                            && !s.open
                            && s.terminal_id.as_deref() == Some(raw_burst_id.as_str())
                    })
                    .context("Expected closed >=2s agent segment after finished")?;
                ensure!(
                    rec.duration_ms() >= 2_000
                        && rec.agent_data().is_some_and(|a| a.provider == "codex"),
                    "Closed finished segment mismatch: {rec:?}"
                );
            }

            // Missed snapshot preserves still-working terminal (`m1`) and closes removed one (`m2`),
            // then `removed` event closes `m1` cleanly.
            let sh_m1 = world.client_call(
                "terminal.create",
                json!({ "project_id": pid, "name": "Missed Keep", "cwd": dir }),
                Some(&pid),
            )?;
            let sh_m2 = world.client_call(
                "terminal.create",
                json!({ "project_id": pid, "name": "Missed Drop", "cwd": dir }),
                Some(&pid),
            )?;
            let m1_id = sh_m1["id"].as_str().unwrap().to_owned();
            let m2_id = sh_m2["id"].as_str().unwrap().to_owned();
            let raw_m1_id = canban_types::split_qualified_host_id(&m1_id)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| m1_id.clone());
            let raw_m2_id = canban_types::split_qualified_host_id(&m2_id)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| m2_id.clone());
            let m1_addr = sh_m1["runtime"]["terminal"].as_str().unwrap().to_owned();
            let m2_addr = sh_m2["runtime"]["terminal"].as_str().unwrap().to_owned();
            herdr.set_foreground(
                &m1_addr,
                Some(json!({ "pid": 301, "name": "claude", "argv": ["claude"], "cwd": dir })),
            );
            herdr.set_foreground(
                &m2_addr,
                Some(json!({ "pid": 302, "name": "claude", "argv": ["claude"], "cwd": dir })),
            );
            herdr.set_status_detailed(
                &m1_addr,
                json!({ "state": "working", "app": "claude" }),
                Some(1),
            );
            herdr.set_status_detailed(
                &m2_addr,
                json!({ "state": "working", "app": "claude" }),
                Some(1),
            );
            world.advance(Duration::from_millis(3_000))?;
            // Drop the individual `removed` event for m2 so AppServer discovers m2 gone via `missed` snapshot!
            herdr.drop_next_events(1);
            herdr.remove_terminal(&m2_addr);
            herdr.emit_missed_snapshot();
            world.advance(Duration::from_millis(500))?;
            {
                let to_ms = world.server("remote-a").unwrap().now_ms() + 1_000;
                let act = world.server_call(
                    "remote-a",
                    "insights.activity",
                    json!({ "from_ms": burst_from, "to_ms": to_ms, "kinds": ["agent"] }),
                )?;
                let segs: Vec<ActivitySegment> = serde_json::from_value(act["segments"].clone())?;
                ensure!(
                    segs.iter()
                        .any(|s| s.open && s.terminal_id.as_deref() == Some(raw_m1_id.as_str())),
                    "Missed snapshot must keep still-working m1 open"
                );
                ensure!(
                    segs.iter()
                        .any(|s| !s.open && s.terminal_id.as_deref() == Some(raw_m2_id.as_str())),
                    "Missed snapshot must close & persist removed m2"
                );
            }
            // Removing working m1 via normal `removed` event closes & persists it too.
            herdr.remove_terminal(&m1_addr);
            world.advance(Duration::from_millis(500))?;
            {
                let to_ms = world.server("remote-a").unwrap().now_ms() + 1_000;
                let act = world.server_call(
                    "remote-a",
                    "insights.activity",
                    json!({ "from_ms": burst_from, "to_ms": to_ms, "kinds": ["agent"] }),
                )?;
                let segs: Vec<ActivitySegment> = serde_json::from_value(act["segments"].clone())?;
                ensure!(
                    segs.iter()
                        .any(|s| !s.open && s.terminal_id.as_deref() == Some(raw_m1_id.as_str())),
                    "Removing working m1 must close & persist its segment"
                );
            }
            for tid_c in [&burst_id, &m1_id, &m2_id] {
                let _ = world.client_call(
                    "terminal.close",
                    json!({ "terminal_id": tid_c }),
                    Some(&pid),
                )?;
            }

            // 11b. Non-agent broadcast suppression: OSC title spinner & same-cwd non-agent foreground
            //      events MUST NOT emit `terminals` broadcasts, while cwd change / agent launch / exit DO!
            let sh_quiet = world.client_call(
                "terminal.create",
                json!({ "project_id": pid, "name": "Quiet Broadcast Shell", "cwd": dir }),
                Some(&pid),
            )?;
            let quiet_id = sh_quiet["id"].as_str().unwrap().to_owned();
            let quiet_addr = sh_quiet["runtime"]["terminal"].as_str().unwrap().to_owned();
            herdr.set_foreground(
                &quiet_addr,
                Some(json!({ "pid": 100, "name": "zsh", "argv": ["-zsh"], "cwd": dir })),
            );
            world.advance(Duration::from_millis(300))?;
            let ev_rx = world.server("remote-a").unwrap().subscribe();
            while ev_rx.try_recv().is_ok() {}

            herdr.set_title(&quiet_addr, "⠋ Building");
            herdr.set_title(&quiet_addr, "⠙ Building");
            herdr.set_foreground(
                &quiet_addr,
                Some(json!({ "pid": 101, "name": "cargo", "argv": ["cargo", "test"], "cwd": dir })),
            );
            herdr.set_foreground(
                &quiet_addr,
                Some(json!({ "pid": 102, "name": "rustc", "argv": ["rustc", "--crate-name", "a"], "cwd": dir })),
            );
            herdr.set_foreground(
                &quiet_addr,
                Some(json!({ "pid": 100, "name": "zsh", "argv": ["-zsh"], "cwd": dir })),
            );
            world.advance(Duration::from_millis(300))?;
            let mut quiet_term_broadcasts = 0usize;
            while let Ok(ev) = ev_rx.try_recv() {
                if matches!(
                    ev,
                    AppEvent::Changed {
                        topic: "terminals",
                        ..
                    }
                ) {
                    quiet_term_broadcasts += 1;
                }
            }
            ensure!(
                quiet_term_broadcasts == 0,
                "OSC title & same-cwd non-agent foreground changes must NOT emit terminals broadcasts (got {quiet_term_broadcasts})"
            );

            // Cwd change -> MUST broadcast `terminals`!
            let sub_cwd = format!("{dir}/sub");
            herdr.set_foreground(
                &quiet_addr,
                Some(json!({ "pid": 100, "name": "zsh", "argv": ["-zsh"], "cwd": sub_cwd })),
            );
            world.advance(Duration::from_millis(300))?;
            let mut saw_cwd_broadcast = false;
            while let Ok(ev) = ev_rx.try_recv() {
                if matches!(
                    ev,
                    AppEvent::Changed {
                        topic: "terminals",
                        ..
                    }
                ) {
                    saw_cwd_broadcast = true;
                }
            }
            ensure!(
                saw_cwd_broadcast,
                "cwd change must broadcast terminals topic"
            );

            // Agent launch (`claude`) -> MUST broadcast `terminals`!
            herdr.set_foreground(
                &quiet_addr,
                Some(json!({ "pid": 104, "name": "claude", "argv": ["claude"], "cwd": sub_cwd })),
            );
            world.advance(Duration::from_millis(300))?;
            let mut saw_agent_start = false;
            while let Ok(ev) = ev_rx.try_recv() {
                if matches!(
                    ev,
                    AppEvent::Changed {
                        topic: "terminals",
                        ..
                    }
                ) {
                    saw_agent_start = true;
                }
            }
            ensure!(
                saw_agent_start,
                "foreground agent launch must broadcast terminals topic"
            );

            // Agent exit back to `zsh` -> MUST broadcast `terminals`!
            herdr.set_foreground(
                &quiet_addr,
                Some(json!({ "pid": 100, "name": "zsh", "argv": ["-zsh"], "cwd": sub_cwd })),
            );
            world.advance(Duration::from_millis(300))?;
            let mut saw_agent_exit = false;
            while let Ok(ev) = ev_rx.try_recv() {
                if matches!(
                    ev,
                    AppEvent::Changed {
                        topic: "terminals",
                        ..
                    }
                ) {
                    saw_agent_exit = true;
                }
            }
            ensure!(
                saw_agent_exit,
                "foreground agent exit must broadcast terminals topic"
            );
            let _ = world.client_call(
                "terminal.close",
                json!({ "terminal_id": quiet_id }),
                Some(&pid),
            )?;

            // 11c. `overview.list`: global cross-project rows, deduplicated conversations,
            //      unfinished idle >1h stall, interim `working` report reset, `completed` + `human_gate`
            //      (`Needs attention` with 2 reasons when `blocked`, `Ready for review` when `working`),
            //      and `unavailable` when Herdr offline!
            let dir_ov = world.project_dir("herdr-overview")?;
            let proj_ov = world.client_call(
                "project.create",
                json!({ "host_id": "remote-a", "name": "Overview Proj", "directory": dir_ov }),
                None,
            )?;
            let pid_ov = proj_ov["id"].as_str().unwrap().to_owned();
            let raw_pid_ov = canban_types::split_qualified_host_id(&pid_ov)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| pid_ov.clone());
            let ov_sources = world.client_call(
                "workflow.sources",
                json!({ "project_id": pid_ov }),
                Some(&pid_ov),
            )?;
            let _ = world.client_call(
                "workflow.apply",
                json!({
                    "project_id": pid_ov,
                    "revision": ov_sources["revision"],
                    "sources": [{ "filename": "overview-gate.yaml", "source": OVERVIEW_GATE_YAML }],
                }),
                Some(&pid_ov),
            )?;
            let c_ov = world.client_call(
                "card.create",
                json!({ "view": "full", "project_id": pid_ov, "title": "Overview Lifecycle Card" }),
                Some(&pid_ov),
            )?;
            let cid_ov = c_ov["id"].as_str().unwrap().to_owned();
            let raw_cid_ov = canban_types::split_qualified_host_id(&cid_ov)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| cid_ov.clone());
            let c_ov_num = c_ov["number"].as_u64().unwrap();
            let _ = world.client_call(
                "card.move",
                json!({ "view": "full", "card_id": cid_ov, "revision": 1, "stage": "develop" }),
                Some(&pid_ov),
            )?;
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == raw_cid_ov && a.terminal.is_some())
                }))
            })?;
            let (ov_tid, ov_addr) = {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let t = snap
                    .terminals
                    .iter()
                    .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid_ov))
                    .unwrap();
                (
                    t.id.clone(),
                    t.agent.as_ref().unwrap().terminal.clone().unwrap(),
                )
            };

            // Automatic assignment pausing in `done` (< 1h) stays quiet (`Idle`).
            herdr.set_status(&ov_addr, "done", Some(1));
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == ov_tid
                        && t.agent
                            .as_ref()
                            .is_some_and(|a| a.lifecycle.idle_since.is_some())
                }))
            })?;
            let get_ov_rows = |w: &mut crate::world::SimWorld| -> Result<Vec<OverviewRow>> {
                let val = w.server_call("remote-a", "overview.list", json!({}))?;
                Ok(serde_json::from_value(val)?)
            };
            {
                let rows = get_ov_rows(world)?;
                let row = rows
                    .iter()
                    .find(|r| r.card.id == raw_cid_ov)
                    .context("missing overview row for c_ov")?;
                ensure!(
                    row.group == "Idle" && !row.actionable() && row.reasons.is_empty(),
                    "Automatic <1h idle must stay quiet Idle: {row:?}"
                );
            }

            // Advance `remote-a` clock > 1h (`UNREPORTED_IDLE_ALERT_MS + 5s`) -> alerts `Needs attention`!
            world.servers["remote-a"]
                .clock
                .jump_ms((canban_notifications::UNREPORTED_IDLE_ALERT_MS + 5_000) as i64);
            herdr.emit_heartbeat();
            world.advance(Duration::from_millis(500))?;
            {
                let rows = get_ov_rows(world)?;
                let row = rows.iter().find(|r| r.card.id == raw_cid_ov).unwrap();
                ensure!(
                    row.group == "Needs attention"
                        && row.actionable()
                        && row.reasons.first().map(|r| r.source.as_str()) == Some("runtime"),
                    "Unreported idle >1h must surface runtime Needs attention: {row:?}"
                );
            }

            // Interim `working` report resets the 1-hour stall window back to `Idle`.
            let _ = world.client_call(
                "execution.report",
                json!({
                    "project_id": pid_ov,
                    "card": c_ov_num,
                    "state": "working",
                    "message": "Running background verification",
                    "request_key": format!("ov-interim-{seed}"),
                }),
                Some(&pid_ov),
            )?;
            {
                let rows = get_ov_rows(world)?;
                let row = rows.iter().find(|r| r.card.id == raw_cid_ov).unwrap();
                ensure!(
                    row.group == "Idle" && !row.actionable(),
                    "Interim working report must reset 1h stall alert: {row:?}"
                );
            }

            // Report `completed` -> triggers `approval` gate (`human_gate = Review report`)!
            let _ = world.client_call(
                "execution.report",
                json!({
                    "project_id": pid_ov,
                    "card": c_ov_num,
                    "state": "completed",
                    "message": "Completed work for approval",
                    "request_key": format!("ov-done-{seed}"),
                }),
                Some(&pid_ov),
            )?;
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap
                    .cards
                    .iter()
                    .any(|c| c.id == raw_cid_ov && c.human_gate.is_some()))
            })?;
            // Blocked runtime + human_gate -> deduplicated single row with 2 reasons (`Needs attention`)!
            herdr.set_status_detailed(
                &ov_addr,
                json!({ "state": "blocked", "kind": "permission", "title": "Allow?" }),
                Some(2),
            );
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == ov_tid
                        && t.agent
                            .as_ref()
                            .is_some_and(|a| a.lifecycle.waiting.is_some())
                }))
            })?;
            {
                let rows = get_ov_rows(world)?;
                let ov_matches: Vec<_> = rows.iter().filter(|r| r.card.id == raw_cid_ov).collect();
                ensure!(
                    ov_matches.len() == 1
                        && ov_matches[0].group == "Needs attention"
                        && ov_matches[0].reasons.len() == 2,
                    "Expected deduplicated single row with 2 reasons (task + runtime): {ov_matches:?}"
                );
            }
            // Transition runtime back to `working` while `human_gate` remains -> `Ready for review`!
            herdr.set_status(&ov_addr, "working", Some(2));
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == ov_tid
                        && t.agent
                            .as_ref()
                            .is_some_and(|a| a.lifecycle.waiting.is_none())
                }))
            })?;
            {
                let rows = get_ov_rows(world)?;
                let row = rows.iter().find(|r| r.card.id == raw_cid_ov).unwrap();
                ensure!(
                    row.group == "Ready for review",
                    "Completed card with human_gate + working runtime must group in Ready for review: {row:?}"
                );
            }
            // Cross-project global overview: add an attention card on `pid2` (Herdr Proj 2) too!
            let c_other = world.client_call(
                "card.create",
                json!({ "view": "full", "project_id": pid2, "title": "Other Project Attention" }),
                Some(&pid2),
            )?;
            let cid_other = c_other["id"].as_str().unwrap().to_owned();
            let raw_cid_other = canban_types::split_qualified_host_id(&cid_other)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| cid_other.clone());
            // Start an agent on `pid` (already has `herdr-test`) or give `c_other` an attention gate via `workflow.apply` + `approval`.
            let p2_sources = world.client_call(
                "workflow.sources",
                json!({ "project_id": pid2 }),
                Some(&pid2),
            )?;
            let p2_gate_wf = r#"schema: 12
id: p2-gate-flow
name: P2 Gate Flow
workspace:
  create_worktree: false
  sync_base: false
on:
  start:
    event: manual
    start_at: need_human
steps:
  - id: need_human
    uses: approval
    with:
      title: No route
    next: done
  - id: done
    uses: complete
"#;
            let _ = world.client_call(
                "workflow.apply",
                json!({
                    "project_id": pid2,
                    "revision": p2_sources["revision"],
                    "sources": [{ "filename": "p2-gate.yaml", "source": p2_gate_wf }],
                }),
                Some(&pid2),
            )?;
            let _ = world.client_call(
                "workflow.start",
                json!({
                    "workflow_id": "p2-gate-flow",
                    "card_id": cid_other,
                    "entry": "start",
                    "request_key": format!("p2-gate-{seed}"),
                }),
                Some(&pid2),
            )?;
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap
                    .cards
                    .iter()
                    .any(|c| c.id == raw_cid_other && c.human_gate.is_some()))
            })?;
            {
                let rows = get_ov_rows(world)?;
                ensure!(
                    rows.iter().any(|r| r.card.id == raw_cid_ov)
                        && rows.iter().any(|r| {
                            r.card.id == raw_cid_other
                                && r.project_name == "Herdr Proj 2"
                                && r.terminal.is_none()
                                && r.actionable()
                        }),
                    "overview.list must be global across projects and include terminal-less actionable cards: {rows:?}"
                );
            }
            // Take Herdr offline -> `connection_state == "unavailable"` -> `Needs attention`!
            herdr.set_offline(true);
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == ov_tid
                        && t.agent
                            .as_ref()
                            .is_some_and(|a| a.connection_state == "unavailable")
                }))
            })?;
            {
                let rows = get_ov_rows(world)?;
                let row = rows.iter().find(|r| r.card.id == raw_cid_ov).unwrap();
                ensure!(
                    row.group == "Needs attention",
                    "Unavailable terminal must escalate overview row to Needs attention: {row:?}"
                );
            }
            herdr.set_offline(false);
            herdr.finish_terminal(&ov_addr, 0, "done");
            world.advance(Duration::from_millis(300))?;

            // 11d. Exited agent keeps task recovery AND reads saved output (`overview.preview` &
            //      `execution.output`) while Herdr core is OFFLINE!
            let c_saved = world.client_call(
                "card.create",
                json!({ "view": "full", "project_id": pid_ov, "title": "Saved Output Recovery" }),
                Some(&pid_ov),
            )?;
            let cid_saved = c_saved["id"].as_str().unwrap().to_owned();
            let raw_cid_saved = canban_types::split_qualified_host_id(&cid_saved)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| cid_saved.clone());
            let c_saved_num = c_saved["number"].as_u64().unwrap();
            let _ = world.client_call(
                "card.move",
                json!({ "view": "full", "card_id": cid_saved, "revision": 1, "stage": "develop" }),
                Some(&pid_ov),
            )?;
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == raw_cid_saved && a.terminal.is_some())
                }))
            })?;
            let (saved_tid, saved_addr) = {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let t = snap
                    .terminals
                    .iter()
                    .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid_saved))
                    .unwrap();
                (
                    t.id.clone(),
                    t.agent.as_ref().unwrap().terminal.clone().unwrap(),
                )
            };
            let large_utf8_line = "中".repeat(12_000);
            herdr.append_output(
                &saved_addr,
                [
                    large_utf8_line.as_str(),
                    "Saved failure context: authentication required",
                ],
            );
            // Set an explicit task gate AND a blocked runtime status, then exit with exit_code=1.
            let _ = world.client_call(
                "execution.report",
                json!({
                    "project_id": pid_ov,
                    "card": c_saved_num,
                    "state": "blocked",
                    "message": "Agent exited without an explicit outcome",
                    "request_key": format!("saved-blk-{seed}"),
                }),
                Some(&pid_ov),
            )?;
            herdr.set_status_detailed(
                &saved_addr,
                json!({ "state": "blocked", "kind": "permission", "title": "Allow?" }),
                Some(1),
            );
            world.advance(Duration::from_millis(200))?;
            herdr.finish_terminal(&saved_addr, 1, "exited");
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == saved_tid
                        && t.agent
                            .as_ref()
                            .is_some_and(|a| a.finished() && a.output_path.is_some())
                }))
            })?;

            // Take Herdr core OFFLINE and verify `overview.list`, `overview.preview`, and `execution.output`!
            herdr.set_offline(true);
            world.advance(Duration::from_millis(200))?;
            {
                let rows = get_ov_rows(world)?;
                let row = rows
                    .iter()
                    .find(|r| r.card.id == raw_cid_saved)
                    .context("missing saved-output overview row")?;
                let sources: Vec<&str> = row.reasons.iter().map(|r| r.source.as_str()).collect();
                ensure!(
                    row.group == "Needs attention"
                        && row.agent().is_some_and(|a| a.finished())
                        && sources == vec!["task"],
                    "Confirmed exit must win over prior blocked status and keep task recovery: {row:?}"
                );
                let preview = world.server_call(
                    "remote-a",
                    "overview.preview",
                    json!({ "row_id": row.id, "terminal_id": saved_tid }),
                )?;
                let preview_text = preview["text"].as_str().unwrap_or("");
                ensure!(
                    preview_text.len() <= 32_768
                        && preview_text.ends_with("authentication required")
                        && preview["source"] == "Saved execution output (read-only)"
                        && preview["observed_at"].is_null(),
                    "Offline saved-output overview.preview mismatch: {preview}"
                );
                // Wrong terminal_id must be rejected.
                ensure!(
                    world
                        .server_call(
                            "remote-a",
                            "overview.preview",
                            json!({ "row_id": row.id, "terminal_id": "obsolete:1" }),
                        )
                        .is_err()
                );
                let q_saved_tid = canban_types::qualify_host_id("remote-a", &saved_tid);
                let full_out = world.client_call(
                    "execution.output",
                    json!({ "terminal_id": q_saved_tid }),
                    Some(&pid_ov),
                )?;
                let full_body = full_out["body"].as_str().unwrap_or("");
                ensure!(
                    full_body.contains(&large_utf8_line)
                        && full_body.contains("Saved failure context: authentication required"),
                    "Offline execution.output did not return full saved output"
                );
            }
            herdr.set_offline(false);
            world.wait_until(10_000, |w| {
                Ok(w.herdr("remote-a").unwrap().subscriber_count() > 0)
            })?;

            // 11e. Authoritative missing terminal: resolves runtime input wait, preserves explicit
            //      task gate, AND never overwrites known exit codes when a finished terminal vanishes!
            let c_miss = world.client_call(
                "card.create",
                json!({ "view": "full", "project_id": pid_ov, "title": "Authoritative Missing Card" }),
                Some(&pid_ov),
            )?;
            let cid_miss = c_miss["id"].as_str().unwrap().to_owned();
            let raw_cid_miss = canban_types::split_qualified_host_id(&cid_miss)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| cid_miss.clone());
            let c_miss_num = c_miss["number"].as_u64().unwrap();
            let _ = world.client_call(
                "card.move",
                json!({ "view": "full", "card_id": cid_miss, "revision": 1, "stage": "develop" }),
                Some(&pid_ov),
            )?;
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == raw_cid_miss && a.terminal.is_some())
                }))
            })?;
            let (miss_tid, miss_addr) = {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let t = snap
                    .terminals
                    .iter()
                    .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid_miss))
                    .unwrap();
                (
                    t.id.clone(),
                    t.agent.as_ref().unwrap().terminal.clone().unwrap(),
                )
            };
            herdr.set_status_detailed(
                &miss_addr,
                json!({ "state": "blocked", "kind": "permission", "title": "Allow?" }),
                Some(1),
            );
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == miss_tid
                        && t.agent
                            .as_ref()
                            .is_some_and(|a| a.lifecycle.waiting.is_some())
                }))
            })?;
            let _ = world.client_call(
                "execution.report",
                json!({
                    "project_id": pid_ov,
                    "card": c_miss_num,
                    "state": "blocked",
                    "message": "Choose the target environment",
                    "request_key": format!("miss-gate-{seed}"),
                }),
                Some(&pid_ov),
            )?;
            // Vanish from SAME Herdr core -> triggers `runtime_missing`!
            herdr.remove_terminal(&miss_addr);
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == miss_tid
                        && t.agent
                            .as_ref()
                            .is_some_and(|a| a.connection_state == "missing")
                }))
            })?;
            {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let a = snap
                    .terminals
                    .iter()
                    .find(|t| t.id == miss_tid)
                    .unwrap()
                    .agent
                    .as_ref()
                    .unwrap();
                let card = snap.cards.iter().find(|c| c.id == raw_cid_miss).unwrap();
                ensure!(
                    a.finished()
                        && a.connection_state == "missing"
                        && a.lifecycle.waiting.is_none()
                        && a.observed.as_ref().unwrap()["exit_code"].is_null()
                        && card.human_gate.as_ref().map(|g| g.reason.as_str())
                            == Some("Choose the target environment"),
                    "Authoritative missing state mismatch: agent={a:?} gate={:?}",
                    card.human_gate
                );
                let rows = get_ov_rows(world)?;
                let row = rows.iter().find(|r| r.card.id == raw_cid_miss).unwrap();
                ensure!(
                    row.reasons.len() == 1 && row.reasons[0].source == "task",
                    "Missing terminal must clear runtime wait and keep task gate: {row:?}"
                );
            }
            // Retention removing already-finished `saved_addr` MUST keep known `exit_code == 1`!
            herdr.remove_terminal(&saved_addr);
            world.advance(Duration::from_millis(400))?;
            {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let a = snap
                    .terminals
                    .iter()
                    .find(|t| t.id == saved_tid)
                    .unwrap()
                    .agent
                    .as_ref()
                    .unwrap();
                ensure!(
                    a.observed.as_ref().unwrap()["exit_code"] == 1,
                    "Retention removing finished terminal must not erase known exit_code: {a:?}"
                );
            }

            // 11f. PTY-touch followup detection ignores mere PTY touch and preserves card stage!
            let _ = world.client_call(
                "scheduler.configure",
                json!({ "idle_minutes": 15 }),
                Some(&pid_ov),
            )?;
            let c_pty = world.client_call(
                "card.create",
                json!({ "view": "full", "project_id": pid_ov, "title": "PTY Touch Followup" }),
                Some(&pid_ov),
            )?;
            let cid_pty = c_pty["id"].as_str().unwrap().to_owned();
            let raw_cid_pty = canban_types::split_qualified_host_id(&cid_pty)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| cid_pty.clone());
            let c_pty_num = c_pty["number"].as_u64().unwrap();
            let _ = world.client_call(
                "card.move",
                json!({ "view": "full", "card_id": cid_pty, "revision": 1, "stage": "develop" }),
                Some(&pid_ov),
            )?;
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == raw_cid_pty && a.terminal.is_some())
                }))
            })?;
            let (pty_tid, pty_addr) = {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let t = snap
                    .terminals
                    .iter()
                    .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid_pty))
                    .unwrap();
                (
                    t.id.clone(),
                    t.agent.as_ref().unwrap().terminal.clone().unwrap(),
                )
            };
            herdr.set_status(&pty_addr, "working", Some(2));
            world.advance(Duration::from_millis(200))?;
            let _ = world.client_call(
                "execution.report",
                json!({
                    "project_id": pid_ov,
                    "card": c_pty_num,
                    "state": "completed",
                    "message": "Done in develop",
                    "request_key": format!("pty-done-{seed}"),
                }),
                Some(&pid_ov),
            )?;
            herdr.set_status(&pty_addr, "done", Some(2));
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap
                    .reclaim_status_for(&pty_tid)
                    .is_some_and(|r| r.deadline_at.is_some()))
            })?;
            // Mere PTY interaction touch WITHOUT turn/status change must NOT flip interactive_followup!
            herdr.set_viewer_activity(&pty_addr, 0, true);
            world.advance(Duration::from_millis(400))?;
            {
                let srv = world.server("remote-a").unwrap();
                let now_ms = srv.now_ms();
                let snap = srv.snapshot()?;
                let rs = snap
                    .reclaim_status_for(&pty_tid)
                    .context("missing reclaim_status for pty_tid")?;
                ensure!(
                    !rs.interactive_followup
                        && rs.deadline_at.is_some_and(|d| d <= now_ms + 60_000),
                    "Mere PTY touch must keep 30s completed cooldown, not 15m followup: {rs:?}"
                );
            }
            let stage_before_followup = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .cards
                .into_iter()
                .find(|c| c.id == raw_cid_pty)
                .unwrap()
                .stage;
            // New Herdr turn (`turn: 3 > completion_turn: 2`) marks `interactive_followup` (15m) & preserves stage!
            herdr.set_status(&pty_addr, "done", Some(3));
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap
                    .reclaim_status_for(&pty_tid)
                    .is_some_and(|r| r.interactive_followup))
            })?;
            {
                let srv = world.server("remote-a").unwrap();
                let now_ms = srv.now_ms();
                let snap = srv.snapshot()?;
                let card = snap.cards.iter().find(|c| c.id == raw_cid_pty).unwrap();
                let rs = snap.reclaim_status_for(&pty_tid).unwrap();
                ensure!(
                    card.stage == stage_before_followup
                        && rs.interactive_followup
                        && rs.deadline_at.is_some_and(|d| d > now_ms + 10 * 60_000),
                    "Turn 3 followup must switch to 15m cooldown and preserve stage: stage={} rs={rs:?}",
                    card.stage
                );
            }
            herdr.finish_terminal(&pty_addr, 0, "done");

            // 11g. Cardless scheduled presence grace & interactive followup cooldown preserve
            //      `ScheduledRun.state == "completed"`, while unviewed run auto-reclaims cleanly!
            let created_sched = world.client_call(
                "schedule.create",
                json!({
                    "scope": "project",
                    "project_id": pid_ov,
                    "name": "Nightly Presence Grace",
                    "cron": "0 * * * *",
                    "overlap": "allow",
                    "mode": "agent",
                    "profile_id": "codex",
                    "instruction": "Run nightly audit",
                    "enabled": true,
                    "request_key": format!("sched-grace-create-{seed}"),
                }),
                Some(&pid_ov),
            )?;
            let sched_state_id = created_sched["schedule"]["id"]
                .as_str()
                .or_else(|| created_sched["id"].as_str())
                .context("schedule.create missing schedule.id")?
                .to_owned();
            let trig_1 = world.client_call(
                "schedule.trigger",
                json!({
                    "schedule_id": sched_state_id,
                    "request_key": format!("sched-grace-trig1-{seed}"),
                }),
                Some(&pid_ov),
            )?;
            let run1_id = trig_1["id"]
                .as_str()
                .or_else(|| trig_1["run"]["id"].as_str())
                .context("schedule.trigger missing run id")?
                .to_owned();
            let raw_run1_id = canban_types::split_qualified_host_id(&run1_id)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| run1_id.clone());
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == raw_run1_id
                        && t.runtime
                            .as_ref()
                            .is_some_and(|r| r.terminal.is_some() && r.state == "running")
                }))
            })?;
            let sched1_addr = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .terminals
                .into_iter()
                .find(|t| t.id == raw_run1_id)
                .unwrap()
                .runtime
                .unwrap()
                .terminal
                .unwrap();

            // View the cardless scheduled terminal BEFORE completion so presence protects it from reclaim!
            let _ = world.server_call(
                "remote-a",
                "viewer.presence",
                json!({ "client_id": "win-1", "card_id": null, "terminal_id": raw_run1_id }),
            )?;
            let _ = world.server_call(
                "remote-a",
                "schedule.report",
                json!({
                    "schedule_run_id": raw_run1_id,
                    "state": "completed",
                    "message": "Done",
                    "request_key": format!("sched-rep1-{seed}"),
                }),
            )?;
            herdr.set_status(&sched1_addr, "done", Some(1));
            world.advance(Duration::from_secs(6))?;
            {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let r1 = snap
                    .scheduled_runs
                    .iter()
                    .find(|r| r.id == raw_run1_id)
                    .unwrap();
                let rs1 = snap
                    .reclaim_status_for(&raw_run1_id)
                    .context("missing reclaim_status for viewed scheduled run")?;
                ensure!(
                    r1.close_requested_at.is_none() && rs1.paused,
                    "Viewed cardless scheduled terminal must pause reclaim: run={r1:?} rs={rs1:?}"
                );
            }

            // Manual follow-up observed by Herdr (`working`, turn 2) sets `interactive_followup`
            // without mutating `ScheduledRun.state == "completed"`.
            herdr.set_status(&sched1_addr, "working", Some(2));
            world.wait_until(5_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap
                    .reclaim_status_for(&raw_run1_id)
                    .is_some_and(|r| r.interactive_followup))
            })?;
            {
                let snap = world.server("remote-a").unwrap().snapshot()?;
                let r1 = snap
                    .scheduled_runs
                    .iter()
                    .find(|r| r.id == raw_run1_id)
                    .unwrap();
                ensure!(
                    r1.state == "completed",
                    "Manual followup must preserve ScheduledRun.state == completed, got {}",
                    r1.state
                );
            }

            // Settling back to `done` and leaving terminal honors 15m interactive followup cooldown
            // AND pushes a `"reclaim"` topic event when presence transitions from paused -> unpaused!
            let reclaim_rx = world.server("remote-a").unwrap().subscribe();
            while reclaim_rx.try_recv().is_ok() {}
            herdr.set_status(&sched1_addr, "done", Some(2));
            let _ = world.server_call(
                "remote-a",
                "viewer.presence",
                json!({ "client_id": "win-1", "card_id": null, "terminal_id": null }),
            )?;
            world.advance(Duration::from_secs(6))?;
            let mut saw_reclaim_topic = false;
            while let Ok(ev) = reclaim_rx.try_recv() {
                if matches!(
                    ev,
                    AppEvent::Changed {
                        topic: "reclaim",
                        ..
                    }
                ) {
                    saw_reclaim_topic = true;
                }
            }
            ensure!(
                saw_reclaim_topic,
                "Expected AppEvent::Changed topic=\"reclaim\" when reclaim state transitioned"
            );
            {
                let srv = world.server("remote-a").unwrap();
                let now_ms = srv.now_ms();
                let snap = srv.snapshot()?;
                let r1 = snap
                    .scheduled_runs
                    .iter()
                    .find(|r| r.id == raw_run1_id)
                    .unwrap();
                let rs1 = snap.reclaim_status_for(&raw_run1_id).unwrap();
                ensure!(
                    r1.close_requested_at.is_none()
                        && r1.state == "completed"
                        && rs1.interactive_followup
                        && rs1.deadline_at.is_some_and(|d| d > now_ms + 10 * 60_000),
                    "Settled followup scheduled run must wait 15m before reclaim: run={r1:?} rs={rs1:?}"
                );
            }

            // Unviewed 2nd scheduled run completes -> immediately enters `closing` / `auto_reclaiming`!
            let trig_2 = world.client_call(
                "schedule.trigger",
                json!({
                    "schedule_id": sched_state_id,
                    "request_key": format!("sched-grace-trig2-{seed}"),
                }),
                Some(&pid_ov),
            )?;
            let run2_id = trig_2["id"]
                .as_str()
                .or_else(|| trig_2["run"]["id"].as_str())
                .context("schedule.trigger(2) missing run id")?
                .to_owned();
            let raw_run2_id = canban_types::split_qualified_host_id(&run2_id)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| run2_id.clone());
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == raw_run2_id
                        && t.runtime
                            .as_ref()
                            .is_some_and(|r| r.terminal.is_some() && r.state == "running")
                }))
            })?;
            let sched2_addr = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .terminals
                .into_iter()
                .find(|t| t.id == raw_run2_id)
                .unwrap()
                .runtime
                .unwrap()
                .terminal
                .unwrap();
            // Hang exit briefly so we can deterministically observe `rt.state == "closing"` & `auto_reclaiming == true`!
            herdr.set_hang_exit(&sched2_addr, true);
            let _ = world.server_call(
                "remote-a",
                "schedule.report",
                json!({
                    "schedule_run_id": raw_run2_id,
                    "state": "completed",
                    "message": "Done unviewed",
                    "request_key": format!("sched-rep2-{seed}"),
                }),
            )?;
            herdr.set_status(&sched2_addr, "done", Some(1));
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap.terminals.iter().any(|t| {
                    t.id == raw_run2_id && t.runtime.as_ref().is_some_and(|r| r.state == "closing")
                }) && snap
                    .reclaim_status_for(&raw_run2_id)
                    .is_some_and(|rs| rs.auto_reclaiming))
            })?;
            herdr.set_hang_exit(&sched2_addr, false);
            herdr.finish_terminal(&sched2_addr, 0, "done");
            herdr.finish_terminal(&sched1_addr, 0, "done");
            let _ = world.client_call(
                "schedule.toggle",
                json!({ "schedule_id": sched_state_id, "enabled": false }),
                Some(&pid_ov),
            )?;
            world.servers["remote-a"].clock.reset_adjustments();
            herdr.emit_heartbeat();
            world.advance(Duration::from_millis(300))?;

            // 11h. Transient merge-marker (`merging_cards`) invalidates `app.state` without DB write,
            //      `app.watch` rejected, `reclaim` topic push verified, and unchanged `app.state` /
            //      `runtime.state` NEVER deserialize `events` history!
            let dir_mg = world.project_dir("herdr-merge-marker")?;
            let exec_a = world.exec_runner("remote-a").unwrap();
            exec_a.register_git_repo(&dir_mg, SimGitRepo::default());
            exec_a.set_shell_default("sim-merge-marker-test", SimShellOutcome::ok("ok\n"));
            let proj_mg = world.client_call(
                "project.create",
                json!({ "host_id": "remote-a", "name": "Merge Marker Proj", "directory": dir_mg }),
                None,
            )?;
            let pid_mg = proj_mg["id"].as_str().unwrap().to_owned();
            let mg_sources = world.client_call(
                "workflow.sources",
                json!({ "project_id": pid_mg }),
                Some(&pid_mg),
            )?;
            let _ = world.client_call(
                "workflow.apply",
                json!({
                    "project_id": pid_mg,
                    "revision": mg_sources["revision"],
                    "sources": [{ "filename": "transient-merge.yaml", "source": TRANSIENT_MERGE_YAML }],
                }),
                Some(&pid_mg),
            )?;
            let wt_mg = world.client_call(
                "worktree.create",
                json!({
                    "project_id": pid_mg,
                    "branch": "feat/merge-marker",
                    "launch": "none",
                }),
                Some(&pid_mg),
            )?;
            let wt_mg_id = wt_mg["worktree"]["id"].as_str().unwrap().to_owned();
            let c_mg = world.client_call(
                "worktree.promote",
                json!({
                    "worktree_id": wt_mg_id,
                    "title": "Transient Merge Card",
                    "body": "Verify transient merging_cards invalidation",
                }),
                Some(&pid_mg),
            )?;
            let cid_mg = c_mg["id"].as_str().unwrap().to_owned();
            let raw_cid_mg = canban_types::split_qualified_host_id(&cid_mg)
                .map(|(_, r)| r.to_owned())
                .unwrap_or_else(|| cid_mg.clone());
            let mg_wt = world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .cards
                .into_iter()
                .find(|c| c.id == raw_cid_mg)
                .and_then(|c| c.worktree)
                .context("merge-marker card missing worktree")?;
            let _ = exec_a.commit_in_worktree(&mg_wt, "feat: transient merge marker check")?;

            // Capture mid-merge `app.state` revision while `track_merging_many` holds `merging_cards`!
            type MidMergeObservation = (Value, Vec<String>);
            let observed_mid_merge: Arc<Mutex<Option<MidMergeObservation>>> =
                Arc::new(Mutex::new(None));
            let hook_slot = Arc::clone(&observed_mid_merge);
            let srv_weak = Arc::downgrade(&world.server("remote-a").unwrap());
            world
                .server("remote-a")
                .unwrap()
                .set_on_batch_tests_hook(Some(Arc::new(move |_bid, _path| {
                    if let Some(s) = srv_weak.upgrade()
                        && let Ok(st) = s.call("app.state", json!({}))
                    {
                        let merging: Vec<String> = st["snapshot"]["merging_cards"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect();
                        *hook_slot.lock().unwrap() = Some((st["revision"].clone(), merging));
                    }
                })));
            let _ = world.client_call(
                "workflow.start",
                json!({
                    "workflow_id": "transient-merge-flow",
                    "card_id": cid_mg,
                    "entry": "start",
                    "request_key": format!("mg-start-{seed}"),
                }),
                Some(&pid_mg),
            )?;
            world.wait_until(10_000, |w| {
                let snap = w.server("remote-a").unwrap().snapshot()?;
                Ok(snap
                    .cards
                    .iter()
                    .any(|c| c.id == raw_cid_mg && c.resolved()))
            })?;
            world
                .server("remote-a")
                .unwrap()
                .set_on_batch_tests_hook(None);
            let (mid_rev, mid_merging) = observed_mid_merge
                .lock()
                .unwrap()
                .take()
                .context("on_batch_tests_hook did not fire during merge_batch")?;
            ensure!(
                mid_merging.contains(&raw_cid_mg),
                "Expected raw_cid_mg in transient merging_cards during merge: {mid_merging:?}"
            );
            let after_merge =
                world.server_call("remote-a", "app.state", json!({ "after": mid_rev }))?;
            ensure!(
                after_merge["changed"] == true
                    && (after_merge["snapshot"]["merging_cards"].is_null()
                        || after_merge["snapshot"]["merging_cards"]
                            .as_array()
                            .is_some_and(Vec::is_empty)),
                "Transient merging_cards removal must invalidate app.state and clear merging_cards: {after_merge}"
            );

            // Unchanged `app.state`, `app.watch` rejection, `runtime.state`, and no-history-deserialize check!
            {
                let srv = world.server("remote-a").unwrap();
                let initial_state = srv.call("app.state", json!({}))?;
                let unchanged =
                    srv.call("app.state", json!({ "after": initial_state["revision"] }))?;
                ensure!(
                    unchanged["changed"] == false && unchanged.get("snapshot").is_none(),
                    "Unchanged app.state must omit snapshot: {unchanged}"
                );
                ensure!(
                    srv.call("app.watch", json!({})).is_err(),
                    "Removed app.watch RPC must fail"
                );

                // Temporarily corrupt `events` rows in SQLite while Turmoil is paused (single-threaded):
                // neither unchanged `app.state` nor `runtime.state` may ever deserialize `events`!
                let db_path = srv.home.join("canban.sqlite");
                let conn = rusqlite::Connection::open(&db_path)?;
                let backup_events: Vec<(String, String)> = {
                    let mut stmt = conn.prepare("SELECT id, data FROM events")?;
                    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<rusqlite::Result<_>>()?
                };
                ensure!(!backup_events.is_empty());
                conn.execute("UPDATE events SET data='invalid-json'", [])?;

                let check_res: Result<()> = (|| {
                    let unchanged_corrupt =
                        srv.call("app.state", json!({ "after": initial_state["revision"] }))?;
                    ensure!(
                        unchanged_corrupt["changed"] == false
                            && unchanged_corrupt.get("snapshot").is_none(),
                        "Unchanged app.state must not deserialize events history"
                    );
                    let rt_res = srv.call("runtime.state", json!({}))?;
                    let rt: RuntimeState = serde_json::from_value(rt_res["runtime"].clone())?;
                    ensure!(
                        rt.host_active_agents.contains_key("local")
                            && rt.terminals.iter().all(|t| rt
                                .terminal_hosts
                                .get(&t.id)
                                .map(String::as_str)
                                == Some("local")),
                        "runtime.state projection mismatch under corrupt events: {rt:?}"
                    );
                    Ok(())
                })();

                // Always restore original `events` rows immediately before resuming Turmoil!
                let mut restore = conn.prepare("UPDATE events SET data=?2 WHERE id=?1")?;
                for (eid, edata) in &backup_events {
                    restore.execute(rusqlite::params![eid, edata])?;
                }
                check_res?;
            }

            let _ = raw_pid_ov;
            world.hit("herdr.overview_saved_output_and_activity_rules");
        }
        ensure!(
            world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .projects
                .iter()
                .any(|p| p.id == raw_pid)
                && world
                    .server("remote-a")
                    .unwrap()
                    .snapshot()?
                    .projects
                    .iter()
                    .any(|p| p.id == raw_pid2)
        );

        world.heal_and_quiesce(15_000)?;
        oracle.check_quiescent_invariants(world)?;
        Ok(finalize_digest(world))
    })
}
