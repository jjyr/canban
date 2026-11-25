//! Scenario #7 — Idle Agent terminal reclamation safety:
//! active viewer presence (`oracle.protect_terminal`), post-view grace, virtual clock expiry,
//! `bounce_server` mid-`closing`, stuck `/exit` timeout fallback to `terminal.kill`,
//! and manual Shell immunity.
use crate::{
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::time::Duration;

const RECLAIM_YAML: &str = r#"schema: 12
id: reclaim-flow
name: Reclaim Test Flow
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
      instruction: Finish work then idle
    next: finish
  - id: finish
    uses: complete
"#;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "reclaim");
    cfg.remote_hosts = vec!["remote-a".into()];

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        let dir = world.project_dir("reclaim-proj")?;
        let proj = world.client_call(
            "project.create",
            json!({ "host_id": "remote-a", "name": "Reclaim Proj", "directory": dir }),
            None,
        )?;
        let pid = proj["id"].as_str().unwrap().to_owned();

        let _ = world.client_call(
            "scheduler.configure",
            json!({ "idle_minutes": 1 }),
            Some(&pid),
        )?;

        let sources =
            world.client_call("workflow.sources", json!({ "project_id": pid }), Some(&pid))?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid,
                "revision": sources["revision"],
                "sources": [{ "filename": "reclaim-flow.yaml", "source": RECLAIM_YAML }],
            }),
            Some(&pid),
        )?;

        let card = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": "Idle Reclaim Card 1" }),
            Some(&pid),
        )?;
        let cid = card["id"].as_str().unwrap().to_owned();
        let raw_cid = canban_types::split_qualified_host_id(&cid)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| cid.clone());

        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "reclaim-flow",
                "card_id": cid,
                "entry": "start",
                "request_key": format!("reclaim-start-{seed}"),
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

        // Complete the work explicitly so the terminal becomes reclaim-eligible once idle.
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": tid,
                "state": "completed",
                "message": "Done working",
                "request_key": format!("reclaim-done-{seed}"),
            }),
        )?;

        // Phase A: Active viewer presence (`oracle.protect_terminal`) prevents reclamation
        //          even well past `idle_minutes`!
        herdr.set_status(&herdr_addr, "idle", Some(2));
        herdr.set_viewer_activity(&herdr_addr, 1, true);
        world.advance(Duration::from_millis(300))?;
        oracle.protect_terminal(world, "remote-a", &herdr_addr);

        for _ in 0..8 {
            herdr.set_viewer_activity(&herdr_addr, 1, true);
            let _ = world.server_call(
                "remote-a",
                "viewer.presence",
                json!({ "client_id": "ui-viewer-1", "card_id": raw_cid, "terminal_id": tid }),
            )?;
            world.advance(Duration::from_secs(15))?;
            oracle.check_step_invariants(world)?;
        }
        world.hit("reclaim.protected_viewer");

        // Phase B: Detach viewer & lift protection -> idle deadline fires `/exit` (entering
        //          `closing`) -> `bounce_server` mid-`closing` -> Herdr exits cleanly!
        oracle.unprotect_terminal("remote-a", &herdr_addr);
        herdr.set_viewer_activity(&herdr_addr, 0, false);
        let _ = world.server_call(
            "remote-a",
            "viewer.presence",
            json!({ "client_id": "ui-viewer-1", "card_id": null, "terminal_id": null }),
        )?;
        // Hold exit briefly so we observe `closing` and bounce `remote-a` mid-`closing`!
        herdr.set_hang_exit(&herdr_addr, true);

        world.wait_until(180_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == tid && t.agent.as_ref().is_some_and(|a| a.close_state == "closing")
            }))
        })?;
        ensure!(
            herdr
                .sent_inputs()
                .iter()
                .any(|s| s.terminal == herdr_addr && s.text.contains("/exit")),
            "Expected `/exit` sent when entering closing"
        );

        // Bounce AppServer during `closing`, then let Herdr finish the exited terminal.
        world.bounce_server("remote-a")?;
        world.advance(Duration::from_millis(500))?;
        herdr.set_hang_exit(&herdr_addr, false);
        herdr.finish_terminal(&herdr_addr, 0, "exited");

        world.wait_until(15_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .terminals
                .iter()
                .any(|t| t.id == tid && t.agent.as_ref().is_some_and(|a| a.finished())))
        })?;
        world.hit("reclaim.bounce_during_closing");
        oracle.check_step_invariants(world)?;

        // Phase C: Manual Shell terminal is completely immune to idle reclamation.
        let shell = world.client_call(
            "terminal.create",
            json!({ "project_id": pid, "name": "Manual Shell" }),
            Some(&pid),
        )?;
        let shell_tid = shell["id"].as_str().unwrap().to_owned();
        let raw_shell_tid = canban_types::split_qualified_host_id(&shell_tid)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| shell_tid.clone());
        world.advance(Duration::from_secs(2))?;
        let shell_addr = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.id == raw_shell_tid)
            .and_then(|t| t.runtime.and_then(|r| r.terminal))
            .expect("Manual shell missing runtime terminal binding");

        // Phase D: Stuck `/exit` timeout fallback -> `terminal.kill`!
        //          Launch a second agent card, complete it, hang `/exit`, advance past idle_minutes
        //          + 30s close timeout -> AppServer must escalate to `terminal.kill`!
        let card2 = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": "Stuck Exit Fallback Card 2" }),
            Some(&pid),
        )?;
        let cid2 = card2["id"].as_str().unwrap().to_owned();
        let raw_cid2 = canban_types::split_qualified_host_id(&cid2)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| cid2.clone());
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "reclaim-flow",
                "card_id": cid2,
                "entry": "start",
                "request_key": format!("reclaim-stuck-{seed}"),
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
        let (tid2, herdr_addr2) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid2))
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
                "message": "Done but will hang on /exit",
                "request_key": format!("reclaim-stuck-done-{seed}"),
            }),
        )?;
        herdr.set_hang_exit(&herdr_addr2, true);
        herdr.set_status(&herdr_addr2, "idle", Some(2));

        // Wait for `/exit` -> 30s stuck-exit timeout -> `terminal.kill` fallback!
        world.wait_until(240_000, |w| {
            Ok(w.herdr("remote-a")
                .unwrap()
                .killed_terminals()
                .contains(&herdr_addr2))
        })?;
        world.hit("reclaim.stuck_exit_kill_fallback");

        // Advance more virtual time to re-verify manual Shell stayed completely immune!
        for _ in 0..5 {
            world.advance(Duration::from_secs(60))?;
            oracle.check_step_invariants(world)?;
        }
        ensure!(
            !herdr.sent_inputs().iter().any(|s| s.terminal == shell_addr),
            "Manual Shell received unsolicited input!"
        );
        ensure!(
            !herdr.killed_terminals().contains(&shell_addr),
            "Manual Shell was killed by idle reclaimer!"
        );
        ensure!(herdr.terminal_addresses().contains(&shell_addr));
        world.hit("reclaim.manual_shell_immune");

        world.heal_and_quiesce(15_000)?;
        oracle.check_quiescent_invariants(world)?;

        Ok(finalize_digest(world))
    })
}
