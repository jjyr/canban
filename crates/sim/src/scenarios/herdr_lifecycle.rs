//! Scenario #6 — Herdr terminal event lifecycle:
//! out-of-order / stale-seq rejection, missed-snapshot reconciliation, subscriber disconnects,
//! Herdr Core restart (`core.id` change), `terminal.read`/`execution.output` capture,
//! `herdr.set_offline(true)` outage recovery, `AppServer` `bounce_server` live-terminal reattach,
//! `max_agents` slot release on `blocked`/`idle`, and UI terminal-to-project + close-fallback invariants.
use crate::{
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::time::Duration;

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
