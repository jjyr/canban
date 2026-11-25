//! Scenario #14 — Real `crates/workspaces` + `worktrees` + `scripts` (inline & Herdr PTY) + `merge_batch` delivery:
//! Exercises 100% real production worktree, admission, script, agent, and batch merge-queue code
//! paths with `create_worktree: true` backed exclusively by in-process `SimExec` & `SimHerdr`
//! (real on-disk worktree directories inside `TempDir`, ZERO OS child processes spawned):
//! (a) `remote-a` (`ScriptExecutionMode::Auto` / inline): `uses: admit` (`max_agents: 1` gating) +
//!     inline `uses: script` (fail exit 2 -> rework retry -> pass with `$CANBAN_OUTPUT`/`$CANBAN_ENV`)
//!     + `uses: agent` (`create_worktree: true`),
//! (b) `remote-a`: `merge_batch` `test_command` failure -> `@merger` launch in `_merge_queue` ->
//!     `bounce_server` mid-delivery -> `@merger` `batch.report(completed)` re-running `test_command`
//!     (pass) -> clean fast-forward merge & `resolution.kind == "merged"`,
//! (c) `remote-a`: `card.hold_auto_merge` holding Card 2 safely without merging until released,
//!     plus standalone `worktree.create` / `worktree.status` / `worktree.remove`,
//! (d) `remote-a`: true multi-card batch (`items.len() == 2`, `batch_size: 2`) with targeted git
//!     merge conflict on Card 4 (`failure_kind == "conflict"`) -> `@merger` launched via `SimHerdr`
//!     -> `batch.report --evict` evicting conflicting Card 4 while clean Card 3 passes tests and
//!     merges into `main` -> Card 4 reworked and delivered cleanly,
//! (e) `remote-b` (`ScriptExecutionMode::Herdr`): REAL `pump_scripts_herdr()` PTY script execution
//!     launching script terminals through `SimHerdr`, surviving mid-PTY-script `bounce_server`
//!     without duplicate PTY launch, observing PTY output via `terminal.read` `"text"` & exit code
//!     (fail -> rework -> pass with `$CANBAN_OUTPUT`/`$CANBAN_ENV`),
//! (f) `remote-b`: non-fast-forward merge strategies (`strategy: squash` and `strategy: merge`)
//!     with `sync_upstream: true` running real `sync_target` + `squash`/`merge` + `push_target_branch`
//!     against `SimExec`/`SimGitRepo` and verifying remote `origin/main` ref & commit DAG parents!
use crate::{
    exec::{SimGitRepo, SimShellOutcome},
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{ScriptExecutionMode, SimWorldConfig, run_with_world},
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
    on_changes_requested: implement
  - id: done
    stage: review
    uses: complete
"#;

const HERDR_PTY_SQUASH_MERGE_YAML: &str = r#"schema: 12
id: herdr-pty-squash-flow
name: Herdr PTY Script & Upstream Squash/Merge Flow
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
  start_squash:
    event: manual
    start_at: pty_check
  start_merge:
    event: manual
    start_at: implement_merge
steps:
  - id: pty_check
    stage: develop
    uses: script
    with:
      run: echo herdr-pty-check
      timeout_seconds: 600
    next: implement_squash
    on_failure: pty_retry_gate
  - id: pty_retry_gate
    stage: develop
    uses: approval
    with:
      title: Review failed Herdr PTY script
    next: implement_squash
    on_changes_requested: pty_check
  - id: implement_squash
    stage: develop
    uses: agent
    with:
      profile: developer
      instruction: Implement feature for squash upstream delivery
    next: ship_squash
  - id: ship_squash
    stage: review
    uses: deliver
    with:
      target_branch: main
      strategy: squash
      sync_upstream: true
      batch_size: 2
      test_command: sim-squash-test --workspace
      max_conflict_attempts: 2
    next: done
  - id: implement_merge
    stage: develop
    uses: agent
    with:
      profile: developer
      instruction: Implement feature for no-ff merge upstream delivery
    next: ship_merge
  - id: ship_merge
    stage: review
    uses: deliver
    with:
      target_branch: main
      strategy: merge
      sync_upstream: true
      batch_size: 2
      test_command: sim-merge-test --workspace
      max_conflict_attempts: 2
    next: done
  - id: done
    stage: review
    uses: complete
"#;

fn strip_host(id: &str) -> String {
    canban_types::split_qualified_host_id(id)
        .map(|(_, r)| r.to_owned())
        .unwrap_or_else(|| id.to_owned())
}

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "workspaces_delivery");
    cfg.remote_hosts = vec!["remote-a".into(), "remote-b".into()];
    cfg.host_script_execution_modes
        .insert("remote-b".into(), ScriptExecutionMode::Herdr);
    cfg.min_latency_ms = 2;
    cfg.max_latency_ms = 12;

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        // Register deterministic in-process `SimGitRepo`s on `remote-a` and `remote-b` (zero OS git forks!).
        let dir = world.project_dir(&format!("ws-delivery-{seed}"))?;
        let exec = world.exec_runner("remote-a").unwrap();
        exec.register_git_repo(&dir, SimGitRepo::default());
        let initial_main_head = exec
            .head_sha(&dir)
            .expect("SimGitRepo must expose initial HEAD");

        let dir_b = world.project_dir(&format!("ws-pty-squash-{seed}"))?;
        let exec_b = world.exec_runner("remote-b").unwrap();
        exec_b.register_git_repo(&dir_b, SimGitRepo::default());
        let initial_b_head = exec_b
            .head_sha(&dir_b)
            .expect("SimGitRepo(remote-b) must expose initial HEAD");
        ensure!(
            exec_b.remote_ref_sha(&dir_b, "origin", "main").as_deref()
                == Some(initial_b_head.as_str()),
            "Expected initial remote origin/main ref on remote-b"
        );

        // Script outcomes for inline `precheck` on `remote-a`:
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

        // `merge_batch` `test_command` outcomes on `remote-a`:
        // 1st run (Card 1 initial delivery) -> exit 1 failure -> hands off to `@merger`
        // 2nd run (Card 1 after `@merger` `batch.report(completed)`) -> exit 0 pass -> merges!
        // Default (Card 2, Card 3/4 batch) -> exit 0 pass -> merges!
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

        // `merge_batch` `test_command` outcomes on `remote-b`:
        exec_b.set_shell_default(
            "sim-squash-test",
            SimShellOutcome::ok("squash batch tests passed\n"),
        );
        exec_b.set_shell_default(
            "sim-merge-test",
            SimShellOutcome::ok("merge batch tests passed\n"),
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
        let raw_cid1 = strip_host(&cid1);

        let c2 = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": format!("Held Auto-Merge Feature 2 ({seed})") }),
            Some(&pid),
        )?;
        let cid2 = c2["id"].as_str().unwrap().to_owned();
        let raw_cid2 = strip_host(&cid2);
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
        oracle.record_committed_merge_batch("remote-a", &batch_id, &pid);
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

        // ---------------------------------------------------------------------
        // (d) True Multi-Card Batch (`items.len() == 2`) + Git Merge Conflict on Card 4
        //     -> `@merger` launched via `SimHerdr` -> `batch.report --evict` evicts Card 4
        //     while clean Card 3 passes tests and merges into `main` -> Card 4 reworked!
        // ---------------------------------------------------------------------
        let c3 = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": format!("Multi-Batch Clean Card 3 ({seed})") }),
            Some(&pid),
        )?;
        let cid3 = c3["id"].as_str().unwrap().to_owned();
        let raw_cid3 = strip_host(&cid3);

        let c4 = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": format!("Multi-Batch Conflict Card 4 ({seed})") }),
            Some(&pid),
        )?;
        let cid4 = c4["id"].as_str().unwrap().to_owned();
        let raw_cid4 = strip_host(&cid4);

        // Start BOTH Card 3 and Card 4 concurrently (`max_agents == 2`)!
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "ws-delivery-flow",
                "card_id": cid3,
                "entry": "start",
                "request_key": format!("ws-c3-start-{seed}"),
            }),
            Some(&pid),
        )?;
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "ws-delivery-flow",
                "card_id": cid4,
                "entry": "start",
                "request_key": format!("ws-c4-start-{seed}"),
            }),
            Some(&pid),
        )?;

        // Wait until BOTH Card 3 and Card 4 have active `implement` agents running in isolated worktrees.
        world.wait_until(12_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let c3_running = snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid3 && a.terminal.is_some() && a.state == "running"
                })
            });
            let c4_running = snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid4 && a.terminal.is_some() && a.state == "running"
                })
            });
            Ok(c3_running && c4_running)
        })?;

        let (c3_tid, c3_addr, wt3_path, c4_tid, c4_addr, wt4_path, c4_num) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let t3 = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid3))
                .unwrap();
            let t4 = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid4))
                .unwrap();
            let card3 = snap.cards.iter().find(|c| c.id == raw_cid3).unwrap();
            let card4 = snap.cards.iter().find(|c| c.id == raw_cid4).unwrap();
            let wt3 = card3.worktree.clone().expect("Card 3 worktree");
            let wt4 = card4.worktree.clone().expect("Card 4 worktree");
            ensure!(
                wt3 != wt4 && Path::new(&wt3).is_dir() && Path::new(&wt4).is_dir(),
                "Card 3 and Card 4 must have distinct on-disk worktrees"
            );
            (
                t3.id.clone(),
                t3.agent.as_ref().unwrap().terminal.clone().unwrap(),
                wt3,
                t4.id.clone(),
                t4.agent.as_ref().unwrap().terminal.clone().unwrap(),
                wt4,
                card4.number,
            )
        };

        let _c3_commit = exec.commit_in_worktree(&wt3_path, "feat: clean batch card 3")?;
        let c4_commit = exec.commit_in_worktree(&wt4_path, "feat: conflicting batch card 4")?;

        // Temporarily mark the main checkout dirty so the merge queue pauses non-destructively
        // (`DeliveryError::MainCheckoutBusy`) while BOTH Card 3 and Card 4 enter `waiting_delivery`,
        // guaranteeing a deterministic 2-card batch (`items.len() == 2`).
        exec.set_worktree_dirty(&dir, true)?;

        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": c3_tid,
                "state": "completed",
                "message": "Card 3 ready for multi-card batch",
                "request_key": format!("ws-c3-impl-done-{seed}"),
            }),
        )?;
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": c4_tid,
                "state": "completed",
                "message": "Card 4 ready for multi-card batch",
                "request_key": format!("ws-c4-impl-done-{seed}"),
            }),
        )?;
        let herdr_a = world.herdr("remote-a").unwrap();
        herdr_a.finish_terminal(&c3_addr, 0, "exited");
        herdr_a.finish_terminal(&c4_addr, 0, "exited");

        // Wait until BOTH cards are in `waiting_delivery` and their agent terminals are observed finished.
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let c3_waiting = snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(raw_cid3.as_str()) && r.state == "waiting_delivery"
            });
            let c4_waiting = snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(raw_cid4.as_str()) && r.state == "waiting_delivery"
            });
            let c3_stopped = snap
                .terminals
                .iter()
                .any(|t| t.id == c3_tid && t.agent.as_ref().is_some_and(|a| a.agent_stopped()));
            let c4_stopped = snap
                .terminals
                .iter()
                .any(|t| t.id == c4_tid && t.agent.as_ref().is_some_and(|a| a.agent_stopped()));
            Ok(c3_waiting && c4_waiting && c3_stopped && c4_stopped)
        })?;

        // Inject targeted merge conflict on Card 4's commit ONLY, then unpause the main checkout!
        exec.inject_merge_conflict_for_commit(&dir, &c4_commit, vec!["src/shared_api.rs".into()]);
        exec.set_worktree_dirty(&dir, false)?;

        // Wait for the 2-card MergeBatch (`items.len() == 2`) to conflict on Card 4 and spawn `@merger`!
        world.wait_until(15_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.merge_batches.iter().any(|b| {
                b.items.len() == 2
                    && b.failure_kind.as_deref() == Some("conflict")
                    && b.state == "running"
                    && Path::new(&b.worktree).is_dir()
            }))
        })?;
        let (conflict_batch_id, conflict_batch_wt, c3_integrated_sha) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let b = snap
                .merge_batches
                .iter()
                .find(|b| b.items.len() == 2 && b.failure_kind.as_deref() == Some("conflict"))
                .unwrap();
            ensure!(
                b.items.len() == 2,
                "Expected true 2-card merge batch, got {} items",
                b.items.len()
            );
            ensure!(
                b.conflict_card_number == Some(c4_num)
                    && b.conflict_files == vec!["src/shared_api.rs".to_owned()],
                "Unexpected conflict metadata on 2-card batch: num={:?}, files={:?}",
                b.conflict_card_number,
                b.conflict_files
            );
            let item3 = b.items.iter().find(|i| i.card_id == raw_cid3).unwrap();
            let item4 = b.items.iter().find(|i| i.card_id == raw_cid4).unwrap();
            let sha3 = item3
                .integrated_commit
                .clone()
                .expect("Clean Card 3 must already be integrated before Card 4 conflicts");
            ensure!(
                item4.integrated_commit.is_none(),
                "Conflicting Card 4 must NOT have an integrated_commit"
            );
            // Verify `@merger` terminal was genuinely launched in SimHerdr for this conflict batch.
            let merger_entry = snap
                .terminals
                .iter()
                .find(|t| t.id == b.id)
                .expect("@merger TerminalEntry must exist for conflict batch");
            ensure!(
                merger_entry
                    .runtime
                    .as_ref()
                    .and_then(|r| r.terminal.as_deref())
                    .is_some(),
                "@merger runtime terminal must be bound in SimHerdr"
            );
            (b.id.clone(), b.worktree.clone(), sha3)
        };
        oracle.record_committed_merge_batch("remote-a", &conflict_batch_id, &pid);
        world.hit("delivery.multi_card_batch");
        oracle.check_step_invariants(world)?;

        // Safety check: reporting `completed --evict` BEFORE aborting the in-progress cherry-pick
        // conflict inside `_merge_queue` MUST be rejected by `verify_clean_batch_worktree`!
        let dirty_report = world.server_call(
            "remote-a",
            "batch.report",
            json!({
                "batch_id": conflict_batch_id,
                "state": "completed",
                "evict": [c4_num],
                "message": "Premature report while cherry-pick conflict still active",
                "request_key": format!("ws-conflict-evict-dirty-{seed}"),
            }),
        );
        ensure!(
            dirty_report.is_err(),
            "batch.report(completed) must reject an unresolved cherry-pick inside _merge_queue"
        );

        // `@merger` aborts Card 4's conflicting cherry-pick (leaving `_merge_queue` cleanly at
        // Card 3's integrated commit) and calls `batch.report(completed, evict: [c4_num])`.
        exec.abort_worktree_conflict(&conflict_batch_wt)?;
        let evict_reply = world.server_call(
            "remote-a",
            "batch.report",
            json!({
                "batch_id": conflict_batch_id,
                "state": "completed",
                "evict": [c4_num],
                "message": "Evicted conflicting Card 4; delivering clean Card 3",
                "request_key": format!("ws-conflict-evict-clean-{seed}"),
            }),
        )?;
        ensure!(
            evict_reply["accepted"].as_bool() == Some(true)
                && evict_reply["delivered"].as_u64() == Some(1)
                && evict_reply["evicted"].as_u64() == Some(1)
                && evict_reply["commit"].as_str() == Some(c3_integrated_sha.as_str()),
            "Unexpected batch.report --evict reply: {evict_reply}"
        );

        // Wait until Card 3 is merged cleanly AND evicted Card 4 is held in `waiting_approval`!
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            let c3_merged = snap.cards.iter().any(|c| {
                c.id == raw_cid3 && c.resolution.as_ref().map(|r| r.kind.as_str()) == Some("merged")
            });
            let c4_held = snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(raw_cid4.as_str())
                    && r.state == "waiting_approval"
                    && r.step.as_deref() == Some("ship")
            });
            Ok(c3_merged && c4_held)
        })?;
        let (wf4_id, wf4_visit, c4_rev_held) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let card4 = snap.cards.iter().find(|c| c.id == raw_cid4).unwrap();
            let run4 = snap
                .workflow_runs
                .iter()
                .find(|r| r.card_id.as_deref() == Some(raw_cid4.as_str()))
                .unwrap();
            ensure!(
                card4.resolution.is_none()
                    && card4
                        .human_gate
                        .as_ref()
                        .is_some_and(|g| g.reason.contains("Evicted from merge batch")),
                "Evicted Card 4 must remain unmerged with eviction human_gate, got {:?}",
                card4.human_gate
            );
            (run4.id.clone(), run4.visit, card4.revision)
        };
        let main_after_c3 = exec.head_sha(&dir).unwrap();
        ensure!(
            main_after_c3 == c3_integrated_sha && main_after_c3 != main_after_c2,
            "Expected main HEAD to advance to clean Card 3 integrated commit ({c3_integrated_sha}), got {main_after_c3}"
        );
        let c4_timeline = world.client_call(
            "timeline.list",
            json!({ "card_id": cid4, "tail": true, "limit": 50, "view": "full" }),
            Some(&pid),
        )?;
        ensure!(
            c4_timeline["events"]
                .as_array()
                .is_some_and(|evs| evs.iter().any(|e| e["kind"] == "review.batch_evicted")),
            "Expected review.batch_evicted timeline event on evicted Card 4: {c4_timeline}"
        );
        world.hit("delivery.conflict_evict");

        // Send evicted Card 4 back to rework (`implement`) -> commit resolved changes -> merge cleanly!
        let q_wf4_id = canban_types::qualify_host_id("remote-a", &wf4_id);
        let _ = world.client_call(
            "workflow.decide",
            json!({
                "run_id": q_wf4_id,
                "visit": wf4_visit,
                "revision": c4_rev_held,
                "decision": "rework",
                "feedback": "Rebase and resolve src/shared_api.rs conflict",
                "request_key": format!("ws-c4-evict-rework-{seed}"),
            }),
            Some(&pid),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid4
                        && a.state == "running"
                        && a.terminal
                            .as_deref()
                            .is_some_and(|addr| addr != c4_addr.as_str())
                })
            }))
        })?;
        let (c4_rework_tid, c4_rework_addr) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| {
                    t.agent.as_ref().is_some_and(|a| {
                        a.card_id == raw_cid4
                            && a.state == "running"
                            && a.terminal
                                .as_deref()
                                .is_some_and(|addr| addr != c4_addr.as_str())
                    })
                })
                .unwrap();
            (
                t.id.clone(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
            )
        };
        let _c4_fix = exec.commit_in_worktree(&wt4_path, "fix: resolve shared_api conflict")?;
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": c4_rework_tid,
                "state": "completed",
                "message": "Card 4 reworked cleanly after eviction",
                "request_key": format!("ws-c4-rework-done-{seed}"),
            }),
        )?;
        world
            .herdr("remote-a")
            .unwrap()
            .finish_terminal(&c4_rework_addr, 0, "exited");
        world.wait_until(12_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.cards.iter().any(|c| {
                c.id == raw_cid4 && c.resolution.as_ref().map(|r| r.kind.as_str()) == Some("merged")
            }))
        })?;
        let main_after_c4 = exec.head_sha(&dir).unwrap();
        ensure!(
            main_after_c4 != main_after_c3,
            "Expected main HEAD to advance after reworked Card 4 merged"
        );

        // ---------------------------------------------------------------------
        // (e) `remote-b` (`ScriptExecutionMode::Herdr`): REAL `pump_scripts_herdr()` PTY script
        //     execution + mid-PTY-script `bounce_server("remote-b")` crash recovery + `terminal.read`
        //     `"text"` observation + rework retry with `$CANBAN_OUTPUT` / `$CANBAN_ENV`!
        // ---------------------------------------------------------------------
        let proj_b = world.client_call(
            "project.create",
            json!({
                "host_id": "remote-b",
                "name": format!("Herdr PTY Squash Proj {seed}"),
                "directory": dir_b,
                "target_branch": "main",
            }),
            None,
        )?;
        let pid_b = proj_b["id"].as_str().unwrap().to_owned();
        let sources_b = world.client_call(
            "workflow.sources",
            json!({ "project_id": pid_b }),
            Some(&pid_b),
        )?;
        let _ = world.client_call(
            "workflow.apply",
            json!({
                "project_id": pid_b,
                "revision": sources_b["revision"],
                "sources": [{ "filename": "herdr-pty-squash-flow.yaml", "source": HERDR_PTY_SQUASH_MERGE_YAML }],
            }),
            Some(&pid_b),
        )?;

        let cb1 = world.client_call(
            "card.create",
            json!({ "project_id": pid_b, "title": format!("Herdr PTY + Squash Feature ({seed})") }),
            Some(&pid_b),
        )?;
        let cid_b1 = cb1["id"].as_str().unwrap().to_owned();
        let raw_cid_b1 = strip_host(&cid_b1);

        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "herdr-pty-squash-flow",
                "card_id": cid_b1,
                "entry": "start_squash",
                "request_key": format!("ws-b1-start-{seed}"),
            }),
            Some(&pid_b),
        )?;

        // Wait until `pump_scripts_herdr()` launches the script PTY in `SimHerdr` and persists `script_terminal`!
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-b").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(raw_cid_b1.as_str())
                    && r.state == "waiting_script"
                    && r.inputs
                        .get("script_terminal")
                        .and_then(|v| v["address"].as_str())
                        .is_some_and(|a| !a.is_empty())
            }))
        })?;
        let (wf_b1_id, pty_addr_1) = {
            let snap = world.server("remote-b").unwrap().snapshot()?;
            let run = snap
                .workflow_runs
                .iter()
                .find(|r| r.card_id.as_deref() == Some(raw_cid_b1.as_str()))
                .unwrap();
            let addr = run.inputs["script_terminal"]["address"]
                .as_str()
                .unwrap()
                .to_owned();
            let herdr_b = world.herdr("remote-b").unwrap();
            let rec = herdr_b
                .terminal_record(&addr)
                .expect("Herdr script PTY must exist in SimHerdr");
            ensure!(
                rec["state"].as_str() == Some("live")
                    && rec["labels"]["canban.script_step"].as_str() == Some("pty_check"),
                "Unexpected Herdr script terminal record: {rec}"
            );
            (run.id.clone(), addr)
        };

        // Bounce `remote-b` MID-PTY-SCRIPT while `pty_addr_1` is live in `SimHerdr`!
        world.bounce_server("remote-b")?;
        world.heal_and_quiesce(15_000)?;
        InvariantOracle::check_post_bounce_recovery(world, "remote-b")?;
        InvariantOracle::check_sqlite_integrity(world)?;
        {
            let snap = world.server("remote-b").unwrap().snapshot()?;
            let run = snap
                .workflow_runs
                .iter()
                .find(|r| r.id == wf_b1_id)
                .unwrap();
            ensure!(
                run.state == "waiting_script"
                    && run.inputs["script_terminal"]["address"].as_str()
                        == Some(pty_addr_1.as_str()),
                "Server restart mid-PTY-script must preserve waiting_script and script_terminal binding, got state={} inputs={:?}",
                run.state,
                run.inputs
            );
            let script_terms = world.herdr("remote-b").unwrap().script_terminals();
            ensure!(
                script_terms.len() == 1 && script_terms[0].0 == pty_addr_1,
                "Server restart mid-PTY-script must NOT launch a duplicate script PTY: {script_terms:?}"
            );
        }

        // Complete attempt 1 PTY with exit code 1 + scrollback output -> verify post-crash recovery
        // observes the exit, reads PTY output via `terminal.read` `"text"`, and routes to `pty_retry_gate`!
        world.herdr("remote-b").unwrap().complete_script_terminal(
            &pty_addr_1,
            &[
                "[pty] running herdr precheck",
                "[pty] check failed: missing header",
            ],
            1,
        );
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-b").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.id == wf_b1_id
                    && r.state == "waiting_approval"
                    && r.step.as_deref() == Some("pty_retry_gate")
            }))
        })?;
        let (wf_b1_visit, b1_rev) = {
            let srv_b = world.server("remote-b").unwrap();
            let snap = srv_b.snapshot()?;
            let run = snap
                .workflow_runs
                .iter()
                .find(|r| r.id == wf_b1_id)
                .unwrap();
            ensure!(
                run.outputs["pty_check"]["exit_code"].as_i64() == Some(1)
                    && run.outputs["pty_check"]["outcome"].as_str() == Some("failed"),
                "Expected failed Herdr PTY script output on attempt 1, got {:?}",
                run.outputs
            );
            ensure!(
                run.inputs.get("script_terminal").is_none(),
                "script_terminal metadata must be cleared after finalization"
            );
            let log_rel = run.outputs["pty_check"]["path"]
                .as_str()
                .expect("script log path missing");
            let log_text = std::fs::read_to_string(srv_b.home.join(log_rel))?;
            ensure!(
                log_text.contains("[pty] check failed: missing header"),
                "Expected PTY terminal.read scrollback captured in saved log, got: {log_text}"
            );
            let card = snap.cards.iter().find(|c| c.id == raw_cid_b1).unwrap();
            (run.visit, card.revision)
        };
        world.hit("workflow.script_herdr_crash_recovery");

        // Rework `pty_retry_gate` -> launches a fresh 2nd Herdr PTY -> writes `$CANBAN_ENV` +
        // `$CANBAN_OUTPUT` + PTY scrollback -> exits 0 -> proceeds to `implement_squash`!
        let q_wf_b1_id = canban_types::qualify_host_id("remote-b", &wf_b1_id);
        let _ = world.client_call(
            "workflow.decide",
            json!({
                "run_id": q_wf_b1_id,
                "visit": wf_b1_visit,
                "revision": b1_rev,
                "decision": "rework",
                "feedback": "Retry Herdr PTY script after fixing header",
                "request_key": format!("ws-b1-pty-retry-{seed}"),
            }),
            Some(&pid_b),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-b").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.id == wf_b1_id
                    && r.state == "waiting_script"
                    && r.inputs
                        .get("script_terminal")
                        .and_then(|v| v["address"].as_str())
                        .is_some_and(|a| !a.is_empty() && a != pty_addr_1.as_str())
            }))
        })?;
        let (pty_addr_2, pty_env_file, pty_out_file) = {
            let snap = world.server("remote-b").unwrap().snapshot()?;
            let run = snap
                .workflow_runs
                .iter()
                .find(|r| r.id == wf_b1_id)
                .unwrap();
            let meta = &run.inputs["script_terminal"];
            (
                meta["address"].as_str().unwrap().to_owned(),
                meta["env_file"].as_str().unwrap().to_owned(),
                meta["output_file"].as_str().unwrap().to_owned(),
            )
        };
        std::fs::write(&pty_env_file, "HERDR_PTY_ENV=recovered\n")?;
        std::fs::write(&pty_out_file, "pty_verdict=passed\n")?;
        world.herdr("remote-b").unwrap().complete_script_terminal(
            &pty_addr_2,
            &["[pty] attempt 2 all green"],
            0,
        );

        // Wait until `implement_squash` agent starts in Card B1's worktree!
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-b").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid_b1 && a.terminal.is_some() && a.state == "running"
                })
            }))
        })?;
        let (b1_tid, b1_herdr_addr, wt_b1_path) = {
            let srv_b = world.server("remote-b").unwrap();
            let snap = srv_b.snapshot()?;
            let run = snap
                .workflow_runs
                .iter()
                .find(|r| r.id == wf_b1_id)
                .unwrap();
            ensure!(
                run.outputs["pty_check"]["outcome"].as_str() == Some("success")
                    && run.outputs["pty_check"]["pty_verdict"].as_str() == Some("passed")
                    && run
                        .custom_env_for_step(None)
                        .get("HERDR_PTY_ENV")
                        .map(String::as_str)
                        == Some("recovered"),
                "Expected passing Herdr PTY script outputs & env, got {:?}",
                run.outputs
            );
            let log_rel = run.outputs["pty_check"]["path"].as_str().unwrap();
            let log_text = std::fs::read_to_string(srv_b.home.join(log_rel))?;
            ensure!(
                log_text.contains("[pty] attempt 2 all green"),
                "Expected attempt 2 PTY scrollback in log: {log_text}"
            );
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid_b1))
                .unwrap();
            let card = snap.cards.iter().find(|c| c.id == raw_cid_b1).unwrap();
            let wt = card
                .worktree
                .clone()
                .expect("Card B1 worktree must exist on disk");
            ensure!(Path::new(&wt).is_dir(), "Card B1 worktree missing: {wt}");
            (
                t.id.clone(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
                wt,
            )
        };
        world.hit("workflow.script_herdr_pty");
        oracle.check_step_invariants(world)?;

        // ---------------------------------------------------------------------
        // (f) Non-FastForward Merge Strategies (`Squash` and `Merge`) + `sync_upstream: true`
        //     on `remote-b`: exercises real `sync_target` + `squash`/`merge` + `push_target_branch`!
        // ---------------------------------------------------------------------
        // Create 2 commits in Card B1's worktree (to verify Squash collapses them into 1 commit)
        // AND advance upstream `origin/main` so `sync_upstream: true` fast-forwards local `main` first!
        let _b1_c1 = exec_b.commit_in_worktree(&wt_b1_path, "feat: squash commit 1")?;
        let _b1_c2 = exec_b.commit_in_worktree(&wt_b1_path, "feat: squash commit 2")?;
        let upstream_before_b1 = exec_b.advance_remote_ref(&dir_b, "origin", "main")?;
        ensure!(upstream_before_b1 != initial_b_head);

        world.server_call(
            "remote-b",
            "execution.report",
            json!({
                "terminal_id": b1_tid,
                "state": "completed",
                "message": "Card B1 ready for squash + sync_upstream delivery",
                "request_key": format!("ws-b1-impl-done-{seed}"),
            }),
        )?;
        world
            .herdr("remote-b")
            .unwrap()
            .finish_terminal(&b1_herdr_addr, 0, "exited");

        world.wait_until(12_000, |w| {
            let snap = w.server("remote-b").unwrap().snapshot()?;
            let merged = snap.cards.iter().any(|c| {
                c.id == raw_cid_b1
                    && c.resolution.as_ref().map(|r| r.kind.as_str()) == Some("merged")
            });
            let done = snap
                .workflow_runs
                .iter()
                .any(|r| r.id == wf_b1_id && r.state == "completed");
            Ok(merged && done)
        })?;

        let main_after_b1 = exec_b.head_sha(&dir_b).unwrap();
        ensure!(
            main_after_b1 != upstream_before_b1,
            "Expected main HEAD on remote-b to advance after Squash delivery"
        );
        ensure!(
            exec_b.commit_parents(&dir_b, &main_after_b1) == Some(vec![upstream_before_b1.clone()]),
            "Squash strategy + sync_upstream must produce 1 commit directly on top of synced upstream ({upstream_before_b1}), got parents={:?}",
            exec_b.commit_parents(&dir_b, &main_after_b1)
        );
        ensure!(
            exec_b.remote_ref_sha(&dir_b, "origin", "main").as_deref()
                == Some(main_after_b1.as_str()),
            "sync_upstream=true must advance remote origin/main via push_target_branch (expected {main_after_b1}, got {:?})",
            exec_b.remote_ref_sha(&dir_b, "origin", "main")
        );
        let b1_timeline = world.client_call(
            "timeline.list",
            json!({ "card_id": cid_b1, "tail": true, "limit": 50, "view": "full" }),
            Some(&pid_b),
        )?;
        ensure!(
            b1_timeline["events"].as_array().is_some_and(|evs| {
                evs.iter()
                    .any(|e| e["kind"] == "review.merged" && e["data"]["synced_upstream"] == true)
            }),
            "Expected review.merged timeline event with synced_upstream=true on Card B1: {b1_timeline}"
        );
        let b_cmds = exec_b.recorded_commands();
        ensure!(
            b_cmds
                .iter()
                .any(|c| c.args.iter().any(|a| a == "--squash"))
                && b_cmds.iter().any(|c| c.args.iter().any(|a| a == "fetch"))
                && b_cmds.iter().any(|c| c.args.iter().any(|a| a == "push")),
            "Expected recorded git merge --squash, git fetch, and git push on remote-b"
        );
        world.hit("delivery.squash_or_merge");
        world.hit("delivery.sync_upstream_pushed");

        // Also exercise `strategy: merge` (`--no-ff`, 2-parent merge commit) + `sync_upstream: true` on Card B2!
        let cb2 = world.client_call(
            "card.create",
            json!({ "project_id": pid_b, "title": format!("No-FF Merge + Upstream Feature ({seed})") }),
            Some(&pid_b),
        )?;
        let cid_b2 = cb2["id"].as_str().unwrap().to_owned();
        let raw_cid_b2 = strip_host(&cid_b2);
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "herdr-pty-squash-flow",
                "card_id": cid_b2,
                "entry": "start_merge",
                "request_key": format!("ws-b2-start-{seed}"),
            }),
            Some(&pid_b),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-b").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid_b2 && a.terminal.is_some() && a.state == "running"
                })
            }))
        })?;
        let (b2_tid, b2_herdr_addr, wt_b2_path) = {
            let snap = world.server("remote-b").unwrap().snapshot()?;
            let t = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid_b2))
                .unwrap();
            let card = snap.cards.iter().find(|c| c.id == raw_cid_b2).unwrap();
            (
                t.id.clone(),
                t.agent.as_ref().unwrap().terminal.clone().unwrap(),
                card.worktree.clone().expect("Card B2 worktree"),
            )
        };
        let b2_commit = exec_b.commit_in_worktree(&wt_b2_path, "feat: no-ff merge card B2")?;
        world.server_call(
            "remote-b",
            "execution.report",
            json!({
                "terminal_id": b2_tid,
                "state": "completed",
                "message": "Card B2 ready for merge + sync_upstream delivery",
                "request_key": format!("ws-b2-impl-done-{seed}"),
            }),
        )?;
        world
            .herdr("remote-b")
            .unwrap()
            .finish_terminal(&b2_herdr_addr, 0, "exited");
        world.wait_until(12_000, |w| {
            let snap = w.server("remote-b").unwrap().snapshot()?;
            Ok(snap.cards.iter().any(|c| {
                c.id == raw_cid_b2
                    && c.resolution.as_ref().map(|r| r.kind.as_str()) == Some("merged")
            }))
        })?;
        let main_after_b2 = exec_b.head_sha(&dir_b).unwrap();
        ensure!(
            exec_b.commit_parents(&dir_b, &main_after_b2)
                == Some(vec![main_after_b1.clone(), b2_commit.clone()]),
            "Merge strategy must produce a 2-parent merge commit [main_after_b1, b2_commit], got {:?}",
            exec_b.commit_parents(&dir_b, &main_after_b2)
        );
        ensure!(
            exec_b.remote_ref_sha(&dir_b, "origin", "main").as_deref()
                == Some(main_after_b2.as_str()),
            "sync_upstream=true must push Merge commit to origin/main"
        );

        // Final bounce across BOTH hosts + full quiescent invariants + ZERO blocked OS commands!
        world.bounce_server("remote-a")?;
        world.bounce_server("remote-b")?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        for host in ["local-server", "remote-a", "remote-b"] {
            if let Some(runner) = world.exec_runner(host) {
                ensure!(
                    runner.blocked_commands().is_empty(),
                    "workspaces_delivery leaked unexpected OS commands on {host}: {:?}",
                    runner.blocked_commands()
                );
            }
        }
        Ok(finalize_digest(world))
    })
}
