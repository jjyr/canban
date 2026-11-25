//! Scenario #14 — Real `crates/workspaces` + `worktrees` + `scripts` + `merge_batch` delivery:
//! Exercises 100% real production worktree, admission, script, agent, and batch merge-queue code
//! paths with `create_worktree: true` backed exclusively by in-process `SimExec` (real on-disk
//! worktree directories inside `TempDir`, ZERO OS child processes spawned):
//! (a) Workflow combining `uses: admit` (`max_agents: 1` gating) + `uses: script` (testing both
//!     script failure -> rework retry & pass with `$CANBAN_OUTPUT`/`$CANBAN_ENV`) + `uses: agent`
//!     (`create_worktree: true` creating real isolated worktrees) + `uses: deliver` (`merge_batch`
//!     + `test_command`),
//! (b) `merge_batch` `test_command` failure -> `@merger` launch in `_merge_queue` -> `bounce_server`
//!     mid-delivery -> `@merger` `batch.report(completed)` re-running `test_command` (pass) -> clean
//!     fast-forward merge & `resolution.kind == "merged"`,
//! (c) `card.hold_auto_merge` holding Card 2 safely without merging until released, plus standalone
//!     `worktree.create` / `worktree.status` / `worktree.remove` lifecycle and strict
//!     `PRAGMA foreign_key_check` + zero blocked OS commands verification.
use crate::{
    exec::{SimGitRepo, SimShellOutcome},
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::path::Path;
use std::time::Duration;

const WORKSPACES_DELIVERY_YAML: &str = r#"schema: 12
id: ws-delivery-flow
name: Workspaces & Merge Batch Flow
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
    start_at: admit_slot
steps:
  - id: admit_slot
    stage: backlog
    uses: admit
    with:
      agent_slot: true
    next: precheck
  - id: precheck
    stage: develop
    uses: script
    with:
      run: echo sim-precheck
    next: implement
    on_failure: script_retry_gate
  - id: script_retry_gate
    stage: develop
    uses: approval
    with:
      title: Review failed precheck script
    next: implement
    on_changes_requested: precheck
  - id: implement
    stage: develop
    uses: agent
    with:
      profile: developer
      instruction: Implement feature inside isolated Git worktree
    next: ship
  - id: ship
    stage: review
    uses: deliver
    with:
      target_branch: main
      strategy: rebase
      sync_upstream: false
      batch_size: 2
      test_command: sim-cargo-test --workspace
      max_conflict_attempts: 2
    next: done
  - id: done
    stage: review
    uses: complete
"#;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "workspaces_delivery");
    cfg.remote_hosts = vec!["remote-a".into()];
    cfg.min_latency_ms = 2;
    cfg.max_latency_ms = 12;

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        // Register a deterministic in-process `SimGitRepo` on `remote-a` (zero OS git forks!).
        let dir = world.project_dir(&format!("ws-delivery-{seed}"))?;
        let exec = world.exec_runner("remote-a").unwrap();
        exec.register_git_repo(&dir, SimGitRepo::default());
        let initial_main_head = exec
            .head_sha(&dir)
            .expect("SimGitRepo must expose initial HEAD");

        // Script outcomes for `precheck`:
        // 1st run (Card 1) -> exit 2 failure -> jumps to `script_retry_gate`
        // 2nd run (Card 1 after rework) -> exit 0 pass + `$CANBAN_OUTPUT` + `$CANBAN_ENV`
        // Default (Card 2+) -> exit 0 pass
        exec.enqueue_script_outcomes(
            "precheck",
            [
                SimShellOutcome::fail(2, "running precheck", "lint check failed on attempt 1\n"),
                SimShellOutcome::ok("precheck passed on attempt 2\n")
                    .with_output("verdict", "green")
                    .with_env("SIM_PRECHECK", "ok"),
            ],
        );
        exec.set_script_default(
            "precheck",
            SimShellOutcome::ok("precheck passed\n").with_output("verdict", "green"),
        );

        // `merge_batch` `test_command` outcomes:
        // 1st run (Card 1 initial delivery) -> exit 1 failure -> hands off to `@merger`
        // 2nd run (Card 1 after `@merger` `batch.report(completed)`) -> exit 0 pass -> merges!
        // Default (Card 2 after hold release) -> exit 0 pass -> merges!
        exec.enqueue_shell_outcomes(
            "sim-cargo-test",
            [
                SimShellOutcome::fail(1, "running batch tests\n", "test_delivery_gate FAILED\n"),
                SimShellOutcome::ok("running batch tests ... 12 passed\n"),
            ],
        );
        exec.set_shell_default(
            "sim-cargo-test",
            SimShellOutcome::ok("all batch tests ok\n"),
        );

        // Create project on `remote-a` with `target_branch: "main"` and `max_agents: 1`
        // so `uses: admit` actively enforces capacity gating!
        let proj = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-a",
                "name": format!("Workspaces Proj {seed}"),
                "directory": dir,
                "target_branch": "main",
            }),
            None,
        )?;
        let pid = proj["id"].as_str().unwrap().to_owned();
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
                "sources": [{ "filename": "ws-delivery-flow.yaml", "source": WORKSPACES_DELIVERY_YAML }],
            }),
            Some(&pid),
        )?;

        // Create Card 1 (script fail -> retry -> worktree -> test fail -> @merger -> bounce -> merge)
        // and Card 2 (admit wait -> hold_auto_merge -> release -> merge).
        let c1 = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": format!("Delivery Feature 1 ({seed})") }),
            Some(&pid),
        )?;
        let cid1 = c1["id"].as_str().unwrap().to_owned();
        let raw_cid1 = canban_types::split_qualified_host_id(&cid1)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| cid1.clone());

        let c2 = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": format!("Held Auto-Merge Feature 2 ({seed})") }),
            Some(&pid),
        )?;
        let cid2 = c2["id"].as_str().unwrap().to_owned();
        let raw_cid2 = canban_types::split_qualified_host_id(&cid2)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| cid2.clone());
        // Hold auto-merge on Card 2 BEFORE starting its workflow!
        let hold_res = world.client_call(
            "card.hold_auto_merge",
            json!({ "card_id": cid2, "revision": c2["revision"], "hold": true }),
            Some(&pid),
        )?;
        ensure!(
            hold_res["accepted"].as_bool() == Some(true),
            "Expected card.hold_auto_merge accepted=true, got {hold_res}"
        );
        world.wait_until(5_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .cards
                .iter()
                .any(|c| c.id == raw_cid2 && c.hold_auto_merge))
        })?;

        // ---------------------------------------------------------------------
        // (a) Start Card 1 -> `admit_slot` passes -> `precheck` fails -> rework -> passes -> worktree created!
        // ---------------------------------------------------------------------
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "ws-delivery-flow",
                "card_id": cid1,
                "entry": "start",
                "request_key": format!("ws-c1-start-{seed}"),
            }),
            Some(&pid),
        )?;

        // Wait until `precheck` fails on attempt 1 and lands on `script_retry_gate` (`waiting_approval`).
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(raw_cid1.as_str())
                    && r.state == "waiting_approval"
                    && r.step.as_deref() == Some("script_retry_gate")
            }))
        })?;
        let (wf1_id, wf1_visit, c1_rev, wt1_path) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let run = snap
                .workflow_runs
                .iter()
                .find(|r| r.card_id.as_deref() == Some(raw_cid1.as_str()))
                .unwrap();
            ensure!(
                run.outputs["precheck"]["exit_code"].as_i64() == Some(2)
                    && run.outputs["precheck"]["outcome"].as_str() == Some("failed"),
                "Expected failed precheck output on attempt 1, got {:?}",
                run.outputs
            );
            let card = snap.cards.iter().find(|c| c.id == raw_cid1).unwrap();
            let wt = card
                .worktree
                .clone()
                .expect("Card 1 worktree must be created on disk when create_worktree=true");
            ensure!(
                Path::new(&wt).is_dir(),
                "Card 1 worktree path does not exist on disk: {wt}"
            );
            (run.id.clone(), run.visit, card.revision, wt)
        };
        world.hit("workspaces.worktree_created");

        // Request rework on `script_retry_gate` -> jumps back to `precheck` -> attempt 2 passes!
        let q_wf1_id = canban_types::qualify_host_id("remote-a", &wf1_id);
        let _ = world.client_call(
            "workflow.decide",
            json!({
                "run_id": q_wf1_id,
                "visit": wf1_visit,
                "revision": c1_rev,
                "decision": "rework",
                "feedback": "Retry precheck script after fix",
                "request_key": format!("ws-c1-script-retry-{seed}"),
            }),
            Some(&pid),
        )?;

        // Wait for `precheck` to pass and `implement` agent to start running inside Card 1's worktree!
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid1 && a.terminal.is_some() && a.state == "running"
                })
            }))
        })?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let run = snap.workflow_runs.iter().find(|r| r.id == wf1_id).unwrap();
            ensure!(
                run.outputs["precheck"]["outcome"].as_str() == Some("success")
                    && run.outputs["precheck"]["verdict"].as_str() == Some("green"),
                "Expected passing precheck outputs on retry, got {:?}",
                run.outputs
            );
            ensure!(
                run.custom_env_for_step(None)
                    .get("SIM_PRECHECK")
                    .map(String::as_str)
                    == Some("ok"),
                "Expected $CANBAN_ENV export SIM_PRECHECK=ok"
            );
        }
        world.hit("workflow.script_executed");

        // While Card 1 holds the single `max_agents: 1` slot, start Card 2:
        // Card 2 MUST pause at `admit_slot` (`waiting_capacity`)!
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "ws-delivery-flow",
                "card_id": cid2,
                "entry": "start",
                "request_key": format!("ws-c2-start-{seed}"),
            }),
            Some(&pid),
        )?;
        world.wait_until(5_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(raw_cid2.as_str())
                    && r.state == "waiting_capacity"
                    && r.step.as_deref() == Some("admit_slot")
            }))
        })?;
        world.hit("workflow.admit_checked");
        oracle.check_step_invariants(world)?;

        // ---------------------------------------------------------------------
        // (b) Card 1 commits work -> `deliver` (`merge_batch`) -> `test_command` fails -> `@merger`
        //     -> `bounce_server` mid-delivery -> `batch.report(completed)` -> passing tests -> merged!
        // ---------------------------------------------------------------------
        let c1_commit = exec.commit_in_worktree(&wt1_path, "feat: implement card 1")?;
        ensure!(c1_commit != initial_main_head);

        let (c1_tid, c1_herdr_addr) = {
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
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": c1_tid,
                "state": "completed",
                "message": "Card 1 implementation committed in worktree",
                "request_key": format!("ws-c1-impl-done-{seed}"),
            }),
        )?;
        // Exit the agent terminal cleanly so `DELIVERY_AGENT_SETTLE_MS` doesn't hold batch delivery.
        world
            .herdr("remote-a")
            .unwrap()
            .finish_terminal(&c1_herdr_addr, 0, "exited");
        // Increase `max_agents` to 2 so both `@merger` (for Card 1 batch failure) and Card 2's
        // `admit_slot` can proceed cleanly without deadlock!
        let _ = world.client_call(
            "scheduler.configure",
            json!({ "max_agents": 2 }),
            Some(&pid),
        )?;

        // Wait until `merge_batch` executes `sim-cargo-test`, fails attempt 1, and spawns `@merger`!
        world.wait_until(15_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.merge_batches.iter().any(|b| {
                b.failure_kind.as_deref() == Some("test_failure")
                    && b.state == "running"
                    && Path::new(&b.worktree).is_dir()
            }))
        })?;
        let (batch_id, batch_wt) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let b = snap
                .merge_batches
                .iter()
                .find(|b| b.failure_kind.as_deref() == Some("test_failure"))
                .unwrap();
            ensure!(
                b.test_output
                    .as_deref()
                    .is_some_and(|t| t.contains("test_delivery_gate FAILED")),
                "Expected failing batch test output captured on MergeBatch, got {:?}",
                b.test_output
            );
            (b.id.clone(), b.worktree.clone())
        };
        world.hit("delivery.test_failure_rework");
        oracle.check_step_invariants(world)?;

        // Bounce `remote-a` MID-DELIVERY while the MergeBatch and Card worktrees are active!
        world.bounce_server("remote-a")?;
        world.heal_and_quiesce(15_000)?;
        InvariantOracle::check_post_bounce_recovery(world, "remote-a")?;
        InvariantOracle::check_sqlite_integrity(world)?;
        ensure!(
            Path::new(&wt1_path).is_dir() && Path::new(&batch_wt).is_dir(),
            "Card and _merge_queue worktrees must remain intact across server bounce"
        );
        ensure!(
            world
                .server("remote-a")
                .unwrap()
                .snapshot()?
                .merge_batches
                .iter()
                .any(|b| b.id == batch_id && b.active()),
            "Active MergeBatch must survive server bounce cleanly"
        );

        // `@merger` fixes the failing test in `_merge_queue` and reports `batch.report(completed)`.
        let _fix_sha = exec.resolve_worktree_and_commit(&batch_wt)?;
        let batch_done = world.server_call(
            "remote-a",
            "batch.report",
            json!({
                "batch_id": batch_id,
                "state": "completed",
                "message": "Fixed failing test in _merge_queue",
                "request_key": format!("ws-batch-done-{seed}"),
            }),
        )?;
        ensure!(
            batch_done["accepted"].as_bool() == Some(true)
                && batch_done["delivered"].as_u64() == Some(1),
            "Unexpected batch.report(completed) reply: {batch_done}"
        );

        // Verify Card 1 is resolved (`merged`), `main` HEAD advanced, and workflow completed!
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let merged_ok = snap.cards.iter().any(|c| {
                c.id == raw_cid1 && c.resolution.as_ref().map(|r| r.kind.as_str()) == Some("merged")
            });
            let wf_done = snap
                .workflow_runs
                .iter()
                .any(|r| r.id == wf1_id && r.state == "completed");
            Ok(merged_ok && wf_done)
        })?;
        let main_after_c1 = exec.head_sha(&dir).unwrap();
        ensure!(
            main_after_c1 != initial_main_head,
            "Expected main branch HEAD to fast-forward after Card 1 merge_batch completion"
        );
        world.hit("delivery.merge_batch_passed");

        // ---------------------------------------------------------------------
        // (c) Card 2 was admitted through `admit_slot`, runs `implement` in its own worktree,
        //     enters `ship` -> HELD safely by `hold_auto_merge` -> released -> merges cleanly!
        // ---------------------------------------------------------------------
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid2 && a.terminal.is_some() && a.state == "running"
                })
            }))
        })?;
        let (c2_tid, c2_herdr_addr, wt2_path) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid2))
                .unwrap();
            let card = snap.cards.iter().find(|c| c.id == raw_cid2).unwrap();
            let wt = card
                .worktree
                .clone()
                .expect("Card 2 must have its own worktree");
            ensure!(
                wt != wt1_path && Path::new(&wt).is_dir(),
                "Card 2 worktree must be distinct on disk from Card 1"
            );
            (
                t.id.clone(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
                wt,
            )
        };
        let _c2_commit = exec.commit_in_worktree(&wt2_path, "feat: implement card 2")?;
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": c2_tid,
                "state": "completed",
                "message": "Card 2 ready for held delivery",
                "request_key": format!("ws-c2-impl-done-{seed}"),
            }),
        )?;
        world
            .herdr("remote-a")
            .unwrap()
            .finish_terminal(&c2_herdr_addr, 0, "exited");

        // Because `hold_auto_merge == true`, Card 2 MUST pause in `waiting_approval` at `ship`
        // WITHOUT advancing `main`!
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(raw_cid2.as_str())
                    && r.state == "waiting_approval"
                    && r.step.as_deref() == Some("ship")
            }))
        })?;
        world.advance(Duration::from_secs(2))?;
        {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let card2 = snap.cards.iter().find(|c| c.id == raw_cid2).unwrap();
            ensure!(
                card2.resolution.is_none() && card2.hold_auto_merge,
                "Held Card 2 must NOT auto-merge while hold_auto_merge=true!"
            );
            ensure!(
                exec.head_sha(&dir).as_deref() == Some(main_after_c1.as_str()),
                "main HEAD advanced while Card 2 was held!"
            );
        }
        world.hit("delivery.hold_auto_merge");

        // Release `hold_auto_merge` -> Card 2 automatically resumes into `merge_batch` -> passes tests -> merges!
        let c2_rev_now = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .into_iter()
            .find(|c| c.id == raw_cid2)
            .unwrap()
            .revision;
        let _ = world.client_call(
            "card.hold_auto_merge",
            json!({ "card_id": cid2, "revision": c2_rev_now, "hold": false }),
            Some(&pid),
        )?;
        world.wait_until(12_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.cards.iter().any(|c| {
                c.id == raw_cid2 && c.resolution.as_ref().map(|r| r.kind.as_str()) == Some("merged")
            }))
        })?;
        let main_after_c2 = exec.head_sha(&dir).unwrap();
        ensure!(
            main_after_c2 != main_after_c1,
            "Expected main HEAD to advance after releasing Card 2 hold_auto_merge"
        );

        // Exercise standalone `worktree.create` -> `worktree.status` -> `worktree.remove` via `client_call`!
        let wt_create = world.client_call(
            "worktree.create",
            json!({
                "project_id": pid,
                "branch": format!("spike/sim-{seed}"),
                "launch": "none",
            }),
            Some(&pid),
        )?;
        let managed_wt_id = wt_create["worktree"]["id"].as_str().unwrap().to_owned();
        let managed_wt_path = wt_create["worktree"]["path"].as_str().unwrap().to_owned();
        ensure!(
            Path::new(&managed_wt_path).is_dir(),
            "Standalone managed worktree directory missing on disk: {managed_wt_path}"
        );
        let wt_status = world.client_call(
            "worktree.status",
            json!({ "worktree_id": managed_wt_id }),
            Some(&pid),
        )?;
        ensure!(
            wt_status["state"].as_str() == Some("available")
                && wt_status["dirty"].as_bool() == Some(false),
            "Unexpected worktree.status reply: {wt_status}"
        );
        let wt_rm = world.client_call(
            "worktree.remove",
            json!({ "worktree_id": managed_wt_id, "force": false }),
            Some(&pid),
        )?;
        ensure!(
            wt_rm["removed"].as_bool() == Some(true) && !Path::new(&managed_wt_path).exists(),
            "worktree.remove failed to remove managed worktree dir: {wt_rm}"
        );

        // Final bounce + full quiescent invariants + ZERO blocked OS commands!
        world.bounce_server("remote-a")?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        ensure!(
            exec.blocked_commands().is_empty(),
            "workspaces_delivery leaked unexpected OS commands: {:?}",
            exec.blocked_commands()
        );
        Ok(finalize_digest(world))
    })
}
