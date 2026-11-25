//! Scenario #10 — Workflow execution under abrupt faults:
//! mid-flight server crash & partition during active agent execution and approval gates,
//! `condition` branching (`source_trust == external_pr` vs trusted local), `notify` steps,
//! `workflow.decide(rework)` + `approve`, `workflow.stop`, and post-bounce recovery assertions.
use crate::{
    github::SimIssue,
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::time::Duration;

const FAULT_WORKFLOW_YAML: &str = r#"schema: 12
id: fault-flow
name: Fault-Tolerant Conditional Flow
workspace:
  create_worktree: false
  sync_base: false
on:
  start:
    event: manual
    start_at: check_trust
steps:
  - id: check_trust
    uses: condition
    with:
      pointer: /card/source_trust
      equals: external_pr
      then: gate_external
      otherwise: auto_notify
  - id: gate_external
    uses: approval
    with:
      title: Approve external PR before running agent
    next: work
    on_changes_requested: notify_rework
  - id: notify_rework
    uses: notify
    with:
      message: External PR rework requested by reviewer
    next: gate_external
  - id: auto_notify
    uses: notify
    with:
      message: Trusted card admitted automatically
    next: work
  - id: work
    uses: agent
    with:
      profile: developer
      instruction: Implement requested work
    next: approve_work
  - id: approve_work
    uses: approval
    with:
      title: Final implementation approval
    next: finish
    on_changes_requested: work
  - id: finish
    uses: complete
"#;

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "workflow_faults");
    cfg.remote_hosts = vec!["remote-a".into()];
    cfg.min_latency_ms = 5;
    cfg.max_latency_ms = 35;

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        let repo = "acme/wf-faults";
        world.github.add_repo(repo, 3003, "main");
        world.github.set_collaborators(repo, vec!["alice".into()]);
        world.github.upsert_issue(
            repo,
            SimIssue {
                number: 7,
                title: "External Contributor PR".into(),
                body: "Untrusted PR body".into(),
                state: "open".into(),
                author: "mallory".into(),
                labels: vec!["pr".into()],
                updated_at: "2026-06-01T00:00:00Z".into(),
                is_pr: true,
                draft: false,
                merged: false,
                head_sha: "sha-ext-7".into(),
                head_ref: "fork/fix".into(),
                base_ref: "main".into(),
                check_conclusion: Some("success".into()),
            },
        );

        let dir = world.project_dir("wf-faults")?;
        let proj = world.client_call(
            "project.create",
            json!({ "host_id": "remote-a", "name": "Workflow Faults", "directory": dir }),
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
                "sources": [{ "filename": "fault-flow.yaml", "source": FAULT_WORKFLOW_YAML }],
            }),
            Some(&pid),
        )?;

        // Bind GitHub with `pr_intake: "all"` so mallory's PR #7 is imported as `external_pr`.
        let _ = world.client_call(
            "project.github",
            json!({
                "project_id": pid,
                "repository": repo,
                "intake": "off",
                "pr_intake": "all",
                "include_drafts": true,
            }),
            Some(&pid),
        )?;
        let _ = world.client_call("github.sync", json!({ "project_id": pid }), Some(&pid))?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.cards.iter().any(|c| {
                c.project_id == raw_pid && c.source_trust.as_deref() == Some("external_pr")
            }))
        })?;

        // 1. Trusted local card: `condition(otherwise)` -> `auto_notify` (`uses: notify`) -> `work`!
        let local_card = world.client_call(
            "card.create",
            json!({ "project_id": pid, "title": "Trusted Local Feature" }),
            Some(&pid),
        )?;
        let cid1 = local_card["id"].as_str().unwrap().to_owned();
        let raw_cid1 = canban_types::split_qualified_host_id(&cid1)
            .map(|(_, r)| r.to_owned())
            .unwrap_or_else(|| cid1.clone());

        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "fault-flow",
                "card_id": cid1,
                "entry": "start",
                "request_key": format!("wff-start-1-{seed}"),
            }),
            Some(&pid),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == raw_cid1 && a.terminal.is_some() && a.state == "running"
                })
            }))
        })?;
        // Verify `auto_notify` emitted a durable notification event in the timeline!
        let tl1 = world.client_call(
            "timeline.list",
            json!({ "card_id": cid1, "tail": true, "limit": 50, "view": "full" }),
            Some(&pid),
        )?;
        ensure!(
            tl1.to_string()
                .contains("Trusted card admitted automatically"),
            "Expected notify step message in timeline for trusted card; got {tl1}"
        );
        world.hit("workflow_faults.condition_otherwise_notify");
        oracle.check_step_invariants(world)?;

        // 2. Mid-flight partition + abrupt `crash_server` while `work` agent is running!
        world.partition("ui-client", "remote-a");
        let _ = world.client_call(
            "comment.add",
            json!({ "card_id": cid1, "body": "Comment queued during server crash" }),
            Some(&pid),
        )?;
        world.crash_server("remote-a")?;
        world.advance(Duration::from_millis(250))?;
        world.bounce_server("remote-a")?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;
        world.hit("workflow_faults.crash_during_agent");

        // Complete `work` -> transitions to `approve_work` (`waiting_approval`).
        let work_tid = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_cid1))
            .unwrap()
            .id;
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": work_tid,
                "state": "completed",
                "message": "Initial implementation ready",
                "request_key": format!("wff-work1-{seed}"),
            }),
        )?;
        world.wait_until(5_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(raw_cid1.as_str())
                    && r.state == "waiting_approval"
                    && r.step.as_deref() == Some("approve_work")
            }))
        })?;

        // Crash & bounce `remote-a` WHILE paused in `waiting_approval` -> human gate must survive!
        world.crash_server("remote-a")?;
        world.advance(Duration::from_millis(200))?;
        world.bounce_server("remote-a")?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        let (wf1_id, wf1_visit, rev1) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let wf = snap
                .workflow_runs
                .iter()
                .find(|r| r.card_id.as_deref() == Some(raw_cid1.as_str()))
                .unwrap();
            ensure!(
                wf.state == "waiting_approval" && wf.step.as_deref() == Some("approve_work"),
                "Human gate did not survive server crash: state={} step={:?}",
                wf.state,
                wf.step
            );
            let card = snap.cards.iter().find(|c| c.id == raw_cid1).unwrap();
            (wf.id.clone(), wf.visit, card.revision)
        };
        world.hit("workflow_faults.crash_during_approval_gate");

        // Exercise `workflow.decide(rework)` -> re-enters `work` -> complete -> `approve` -> `completed`!
        let q_wf1 = canban_types::qualify_host_id("remote-a", &wf1_id);
        let _ = world.client_call(
            "workflow.decide",
            json!({
                "run_id": q_wf1,
                "visit": wf1_visit,
                "revision": rev1,
                "decision": "rework",
                "feedback": "Add unit tests",
                "request_key": format!("wff-rework-1-{seed}"),
            }),
            Some(&pid),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.id == wf1_id
                    && r.state == "waiting_agent"
                    && r.step.as_deref() == Some("work")
                    && r.visit > wf1_visit
            }))
        })?;
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": work_tid,
                "state": "completed",
                "message": "Added unit tests",
                "request_key": format!("wff-work2-{seed}"),
            }),
        )?;
        world.wait_until(5_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .workflow_runs
                .iter()
                .any(|r| r.id == wf1_id && r.state == "waiting_approval"))
        })?;
        let (wf1_visit2, rev1_2) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let wf = snap.workflow_runs.iter().find(|r| r.id == wf1_id).unwrap();
            let card = snap.cards.iter().find(|c| c.id == raw_cid1).unwrap();
            (wf.visit, card.revision)
        };
        let _ = world.client_call(
            "workflow.decide",
            json!({
                "run_id": q_wf1,
                "visit": wf1_visit2,
                "revision": rev1_2,
                "decision": "approve",
                "request_key": format!("wff-approve-1-{seed}"),
            }),
            Some(&pid),
        )?;
        world.wait_until(5_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap
                .workflow_runs
                .iter()
                .any(|r| r.id == wf1_id && r.state == "completed"))
        })?;
        world.hit("workflow_faults.rework_and_complete");

        // 3. Untrusted `external_pr` card: `condition(then)` MUST stop at `gate_external`
        //    WITHOUT launching any agent! Exercise `rework` -> `notify_rework` -> `gate_external`
        //    -> `workflow.stop`!
        let ext_raw_cid = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .into_iter()
            .find(|c| c.source_trust.as_deref() == Some("external_pr"))
            .unwrap()
            .id;
        let ext_cid = canban_types::qualify_host_id("remote-a", &ext_raw_cid);

        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "fault-flow",
                "card_id": ext_cid,
                "entry": "start",
                "request_key": format!("wff-ext-start-{seed}"),
            }),
            Some(&pid),
        )?;
        world.advance(Duration::from_millis(500))?;
        let (ext_wf_id, ext_visit, ext_rev) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let wf = snap
                .workflow_runs
                .iter()
                .find(|r| r.card_id.as_deref() == Some(ext_raw_cid.as_str()))
                .expect("Missing workflow run for external PR");
            ensure!(
                wf.state == "waiting_approval" && wf.step.as_deref() == Some("gate_external"),
                "external_pr condition must route to gate_external; got state={} step={:?}",
                wf.state,
                wf.step
            );
            ensure!(
                !snap
                    .terminals
                    .iter()
                    .any(|t| t.agent.as_ref().is_some_and(|a| a.card_id == ext_raw_cid)),
                "Untrusted external_pr launched an agent before approval!"
            );
            let card = snap.cards.iter().find(|c| c.id == ext_raw_cid).unwrap();
            (wf.id.clone(), wf.visit, card.revision)
        };
        world.hit("workflow_faults.condition_external_pr_gated");

        // Rework on `gate_external` runs `notify_rework` and loops back to `gate_external`!
        let q_ext_wf = canban_types::qualify_host_id("remote-a", &ext_wf_id);
        let _ = world.client_call(
            "workflow.decide",
            json!({
                "run_id": q_ext_wf,
                "visit": ext_visit,
                "revision": ext_rev,
                "decision": "rework",
                "feedback": "Wait for security review",
                "request_key": format!("wff-ext-rework-{seed}"),
            }),
            Some(&pid),
        )?;
        world.advance(Duration::from_millis(300))?;
        let tl_ext = world.client_call(
            "timeline.list",
            json!({ "card_id": ext_cid, "tail": true, "limit": 50, "view": "full" }),
            Some(&pid),
        )?;
        ensure!(
            tl_ext
                .to_string()
                .contains("External PR rework requested by reviewer"),
            "Expected notify_rework message in external PR timeline"
        );

        // Terminate external PR workflow with `workflow.stop` + verify clean quiescence!
        let _ = world.client_call("workflow.stop", json!({ "run_id": q_ext_wf }), Some(&pid))?;
        world.hit("workflow_faults.stop_at_gate");

        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        let final_snap = world.server("remote-a").unwrap().snapshot()?;
        let ext_wf = final_snap
            .workflow_runs
            .iter()
            .find(|r| r.id == ext_wf_id)
            .unwrap();
        ensure!(
            !ext_wf.active(),
            "workflow.stop must mark run non-active, got {}",
            ext_wf.state
        );

        Ok(finalize_digest(world))
    })
}
