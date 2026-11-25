//! Scenario #8 — Notifications & unread reminders across local + remote hosts:
//! real blocked/approval/comment reminder generation on BOTH hosts, host-qualified vs raw ID
//! matching, `CardRead.sequence` advancement, `card.attention.clear` (`Notice.read`),
//! partition + `notifications.read_all` + server bounce non-resurrection, and archived suppression.
use crate::{
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorldConfig, run_with_world},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::time::Duration;

const NOTIFY_WORKFLOW_YAML: &str = r#"schema: 12
id: notify-flow
name: Notification Flow
workspace:
  create_worktree: false
  sync_base: false
on:
  start:
    event: manual
    start_at: plan
steps:
  - id: plan
    uses: agent
    with:
      profile: codex
      instruction: Work and report status
    next: approve
  - id: approve
    uses: approval
    with:
      title: Review notification gate
    next: finish
  - id: finish
    uses: complete
"#;

fn strip_host(id: &str) -> String {
    canban_types::split_qualified_host_id(id)
        .map(|(_, raw)| raw.to_owned())
        .unwrap_or_else(|| id.to_owned())
}

pub fn run(seed: u64) -> Result<u64> {
    let mut cfg = SimWorldConfig::desktop_default(seed, "notifications");
    cfg.remote_hosts = vec!["remote-a".into()];

    run_with_world(cfg, |world| {
        let mut oracle = InvariantOracle::default();

        // Create projects and install the workflow on BOTH `local` and `remote-a`.
        let dir_l = world.project_dir("notif-local")?;
        let dir_a = world.project_dir("notif-remote-a")?;
        let p_l = world.client_call(
            "project.create",
            json!({ "host_id": "local", "name": "Local Notifs", "directory": dir_l }),
            None,
        )?;
        let p_a = world.client_call(
            "project.create",
            json!({ "host_id": "remote-a", "name": "Remote Notifs", "directory": dir_a }),
            None,
        )?;
        let pid_l = p_l["id"].as_str().unwrap().to_owned();
        let pid_a = p_a["id"].as_str().unwrap().to_owned();

        for pid in [&pid_l, &pid_a] {
            let sources =
                world.client_call("workflow.sources", json!({ "project_id": pid }), Some(pid))?;
            let _ = world.client_call(
                "workflow.apply",
                json!({
                    "project_id": pid,
                    "revision": sources["revision"],
                    "sources": [{ "filename": "notify-flow.yaml", "source": NOTIFY_WORKFLOW_YAML }],
                }),
                Some(pid),
            )?;
        }

        // Create cards on BOTH hosts.
        let c_l = world.client_call(
            "card.create",
            json!({ "project_id": pid_l, "title": "Local Blocked + Approval Card" }),
            Some(&pid_l),
        )?;
        let c_a1 = world.client_call(
            "card.create",
            json!({ "project_id": pid_a, "title": "Remote Blocked Card" }),
            Some(&pid_a),
        )?;
        let c_a2 = world.client_call(
            "card.create",
            json!({ "project_id": pid_a, "title": "Remote Archive Card" }),
            Some(&pid_a),
        )?;
        let cid_l = c_l["id"].as_str().unwrap().to_owned();
        let cid_a1 = c_a1["id"].as_str().unwrap().to_owned();
        let cid_a2 = c_a2["id"].as_str().unwrap().to_owned();
        let raw_l = strip_host(&cid_l);
        let raw_a1 = strip_host(&cid_a1);
        let raw_a2 = strip_host(&cid_a2);

        // Start real agent workflows on BOTH `local` and `remote-a`.
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "notify-flow",
                "card_id": cid_l,
                "entry": "start",
                "request_key": format!("notif-l-{seed}"),
            }),
            Some(&pid_l),
        )?;
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "notify-flow",
                "card_id": cid_a1,
                "entry": "start",
                "request_key": format!("notif-a1-{seed}"),
            }),
            Some(&pid_a),
        )?;
        let _ = world.client_call(
            "workflow.start",
            json!({
                "workflow_id": "notify-flow",
                "card_id": cid_a2,
                "entry": "start",
                "request_key": format!("notif-a2-{seed}"),
            }),
            Some(&pid_a),
        )?;

        world.wait_until(10_000, |w| {
            let local_ready = w
                .server("local-server")
                .unwrap()
                .snapshot()?
                .terminals
                .iter()
                .any(|t| {
                    t.agent
                        .as_ref()
                        .is_some_and(|a| a.card_id == raw_l && a.terminal.is_some())
                });
            let remote_snap = w.server("remote-a").unwrap().snapshot()?;
            let a1_ready = remote_snap.terminals.iter().any(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == raw_a1 && a.terminal.is_some())
            });
            let a2_ready = remote_snap.terminals.iter().any(|t| {
                t.agent
                    .as_ref()
                    .is_some_and(|a| a.card_id == raw_a2 && a.terminal.is_some())
            });
            Ok(local_ready && a1_ready && a2_ready)
        })?;

        let tid_l = world
            .server("local-server")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_l))
            .unwrap()
            .id;
        let (tid_a1, herdr_a1, tid_a2) = {
            let snap = world.server("remote-a").unwrap().snapshot()?;
            let t1 = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_a1))
                .unwrap();
            let t2 = snap
                .terminals
                .iter()
                .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == raw_a2))
                .unwrap();
            (
                t1.id.clone(),
                t1.agent.as_ref().unwrap().terminal.clone().unwrap(),
                t2.id.clone(),
            )
        };

        // 1. Trigger real reminder-producing events on BOTH hosts:
        //    - Local: blocked report -> raises Notice + attention + unread reminder, then
        //      completed report -> transitions into `waiting_approval` human gate!
        //    - Remote-a card 1: Herdr `blocked` status + explicit `execution.report(blocked)` + comment!
        //    - Remote-a card 2: explicit `execution.report(blocked)` (will later be archived to
        //      verify stale-reminder suppression).
        world.server_call(
            "local-server",
            "execution.report",
            json!({
                "terminal_id": tid_l,
                "state": "blocked",
                "message": "Local agent blocked on schema choice",
                "request_key": format!("rep-l-blocked-{seed}"),
            }),
        )?;
        world.herdr("remote-a").unwrap().set_status_detailed(
            &herdr_a1,
            json!({ "state": "blocked", "reason": "approval", "detail": "Waiting for human" }),
            Some(1),
        );
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": tid_a1,
                "state": "blocked",
                "message": "Remote agent blocked on credentials",
                "request_key": format!("rep-a1-blocked-{seed}"),
            }),
        )?;
        world.server_call(
            "remote-a",
            "execution.report",
            json!({
                "terminal_id": tid_a2,
                "state": "blocked",
                "message": "Card to be archived later",
                "request_key": format!("rep-a2-blocked-{seed}"),
            }),
        )?;
        let _ = world.client_call(
            "comment.add",
            json!({ "card_id": cid_a1, "body": "Follow-up context on remote-a" }),
            Some(&pid_a),
        )?;

        // Wait for merged snapshot to observe unread reminders on BOTH hosts.
        world.wait_until(10_000, |w| {
            let merged = w.client_merged_snapshot()?;
            let rems = canban_notifications::reminders(&merged);
            let local_unread = rems
                .iter()
                .any(|r| strip_host(&r.card_id) == raw_l && r.unread);
            let remote_unread = rems
                .iter()
                .any(|r| strip_host(&r.card_id) == raw_a1 && r.unread);
            Ok(local_unread && remote_unread)
        })?;

        let snap_before = world.client_merged_snapshot()?;
        let rems_before = canban_notifications::reminders(&snap_before);
        let local_rems: Vec<_> = rems_before
            .iter()
            .filter(|r| strip_host(&r.card_id) == raw_l)
            .collect();
        let remote_a1_rems: Vec<_> = rems_before
            .iter()
            .filter(|r| strip_host(&r.card_id) == raw_a1)
            .collect();
        let remote_a2_rems: Vec<_> = rems_before
            .iter()
            .filter(|r| strip_host(&r.card_id) == raw_a2)
            .collect();
        ensure!(
            !local_rems.is_empty() && local_rems.iter().any(|r| r.unread),
            "Expected non-empty unread reminders on local before read; got {local_rems:?}"
        );
        ensure!(
            !remote_a1_rems.is_empty() && remote_a1_rems.iter().any(|r| r.unread),
            "Expected non-empty unread reminders on remote-a card 1 before read; got {remote_a1_rems:?}"
        );
        ensure!(
            !remote_a2_rems.is_empty(),
            "Expected non-empty reminders on remote-a card 2 before archive"
        );
        world.hit("notifications.non_empty_both_hosts");
        oracle.check_step_invariants(world)?;

        // 2. Advance local card's read position with `notifications.read` using its highest
        //    reminder sequence, verifying `CardRead.sequence` advances and ONLY local unread drops.
        let local_max_seq = local_rems.iter().map(|r| r.sequence).max().unwrap_or(0);
        let local_read_before = world
            .server("local-server")
            .unwrap()
            .snapshot()?
            .card_reads
            .iter()
            .find(|cr| cr.card_id == raw_l)
            .map(|cr| cr.sequence)
            .unwrap_or(0);
        let target_seq = local_max_seq.max(1);
        let _ = world.client_call(
            "notifications.read",
            json!({ "card_id": cid_l, "sequence": target_seq }),
            Some(&pid_l),
        )?;
        world.advance(Duration::from_millis(200))?;

        let local_snap_after_read = world.server("local-server").unwrap().snapshot()?;
        let local_read_after = local_snap_after_read
            .card_reads
            .iter()
            .find(|cr| cr.card_id == raw_l)
            .map(|cr| cr.sequence)
            .unwrap_or(0);
        ensure!(
            local_read_after >= target_seq && local_read_after > local_read_before,
            "notifications.read failed to advance CardRead.sequence on local (before={local_read_before}, after={local_read_after}, target={target_seq})"
        );
        let merged_after_local_read = world.client_merged_snapshot()?;
        let rems_after_local_read = canban_notifications::reminders(&merged_after_local_read);
        ensure!(
            rems_after_local_read
                .iter()
                .filter(|r| strip_host(&r.card_id) == raw_l)
                .all(|r| !r.unread),
            "Local reminders remained unread after notifications.read: {rems_after_local_read:?}"
        );
        ensure!(
            rems_after_local_read
                .iter()
                .any(|r| strip_host(&r.card_id) == raw_a1 && r.unread),
            "Reading local card must not clear unread reminders on remote-a"
        );
        world.hit("notifications.read_single");

        // Verify `card.attention.clear` marks legacy `Notice.read = true` (separate from CardRead).
        let _ = world.client_call(
            "card.attention.clear",
            json!({ "card_id": cid_l }),
            Some(&pid_l),
        )?;
        world.advance(Duration::from_millis(150))?;
        let local_notices = world.server("local-server").unwrap().snapshot()?.notices;
        ensure!(
            !local_notices.is_empty()
                && local_notices
                    .iter()
                    .filter(|n| n.card_id == raw_l)
                    .all(|n| n.read),
            "card.attention.clear must mark matching Notice.read = true; got {local_notices:?}"
        );
        for n in &local_notices {
            if n.read {
                oracle.record_notice_read("local-server", &n.id);
            }
        }
        world.hit("notifications.attention_clear");

        // Also advance local workflow into `waiting_approval` so a `human_gate` reminder is exercised!
        let local_herdr_addr = world
            .server("local-server")
            .unwrap()
            .snapshot()?
            .terminals
            .iter()
            .find(|t| t.id == tid_l)
            .and_then(|t| t.agent.as_ref()?.terminal.clone())
            .unwrap();
        world
            .herdr("local-server")
            .unwrap()
            .finish_terminal(&local_herdr_addr, 0, "exited");
        world.wait_until(5_000, |w| {
            let snap = w.server("local-server").unwrap().snapshot()?;
            Ok(snap
                .terminals
                .iter()
                .any(|t| t.id == tid_l && t.agent.as_ref().is_some_and(|a| a.finished())))
        })?;
        let local_rev = world
            .server("local-server")
            .unwrap()
            .snapshot()?
            .cards
            .iter()
            .find(|c| c.id == raw_l)
            .unwrap()
            .revision;
        let _ = world.client_call(
            "card.continue",
            json!({ "card_id": cid_l, "revision": local_rev, "message": "Proceed to gate" }),
            Some(&pid_l),
        )?;
        world.wait_until(10_000, |w| {
            let snap = w.server("local-server").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == tid_l
                    && t.agent
                        .as_ref()
                        .is_some_and(|a| a.state == "running" && a.terminal.is_some())
            }))
        })?;
        world.server_call(
            "local-server",
            "execution.report",
            json!({
                "terminal_id": tid_l,
                "state": "completed",
                "message": "Ready for human approval gate",
                "request_key": format!("rep-l-done-{seed}"),
            }),
        )?;
        world.wait_until(5_000, |w| {
            let snap = w.server("local-server").unwrap().snapshot()?;
            Ok(snap.workflow_runs.iter().any(|r| {
                r.card_id.as_deref() == Some(raw_l.as_str()) && r.state == "waiting_approval"
            }))
        })?;
        world.advance(Duration::from_millis(200))?;
        let gate_rems = canban_notifications::reminders(&world.client_merged_snapshot()?);
        ensure!(
            gate_rems
                .iter()
                .any(|r| strip_host(&r.card_id) == raw_l && r.unread),
            "human_gate waiting_approval must produce a fresh unread reminder on local"
        );
        world.hit("notifications.human_gate_reminder");
        oracle.check_step_invariants(world)?;

        // 3. Partition `ui-client` <-> `remote-a`, call `notifications.read_all`, crash & bounce
        //    `remote-a`, then heal & quiesce -> verify `CardRead.sequence` advanced on BOTH hosts
        //    and ZERO unread reminders resurrect across partition or bounce!
        let remote_read_before = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .card_reads
            .iter()
            .find(|cr| cr.card_id == raw_a1)
            .map(|cr| cr.sequence)
            .unwrap_or(0);

        world.partition("ui-client", "remote-a");
        world.advance(Duration::from_millis(120))?;

        let _ = world.client_call("notifications.read_all", json!({}), None)?;
        world.advance(Duration::from_millis(150))?;

        // Crash and bounce `remote-a` while partitioned, then heal & quiesce.
        world.crash_server("remote-a")?;
        world.advance(Duration::from_millis(200))?;
        world.bounce_server("remote-a")?;
        world.heal_and_quiesce(20_000)?;
        oracle.check_quiescent_invariants(world)?;

        // If `notifications.read_all` was dropped for the partitioned remote host (since
        // notifications.read_all is fan-out across currently connected hosts), also invoke
        // explicit `notifications.read_all` post-reconnect and re-bounce to prove persistence!
        let remote_a_snap_mid = world.server("remote-a").unwrap().snapshot()?;
        let remote_mid_unread = canban_notifications::reminders(&remote_a_snap_mid)
            .iter()
            .filter(|r| r.unread)
            .count();
        if remote_mid_unread > 0 {
            let _ = world.client_call("notifications.read_all", json!({}), None)?;
            world.advance(Duration::from_millis(250))?;
            world.crash_server("remote-a")?;
            world.advance(Duration::from_millis(150))?;
            world.bounce_server("remote-a")?;
            world.heal_and_quiesce(20_000)?;
            oracle.check_quiescent_invariants(world)?;
        }

        let remote_a_snap_after = world.server("remote-a").unwrap().snapshot()?;
        let remote_read_after = remote_a_snap_after
            .card_reads
            .iter()
            .find(|cr| cr.card_id == raw_a1)
            .map(|cr| cr.sequence)
            .unwrap_or(0);
        ensure!(
            remote_read_after > remote_read_before,
            "remote-a CardRead.sequence failed to advance across read_all + bounce (before={remote_read_before}, after={remote_read_after})"
        );
        let merged_after_bounce = world.client_merged_snapshot()?;
        let unread_after_bounce: Vec<_> = canban_notifications::reminders(&merged_after_bounce)
            .into_iter()
            .filter(|r| r.unread)
            .collect();
        ensure!(
            unread_after_bounce.is_empty(),
            "Unread reminders resurrected after read_all + server bounce: {unread_after_bounce:?}"
        );
        world.hit("notifications.read_all_partition_bounce");

        // 4. Archive `c_a2` (which had a blocked reminder) -> zero stale reminders for archived card!
        let a2_rev = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .cards
            .iter()
            .find(|c| c.id == raw_a2)
            .unwrap()
            .revision;
        let _ = world.client_call(
            "card.archive",
            json!({ "card_id": cid_a2, "revision": a2_rev }),
            Some(&pid_a),
        )?;
        world.heal_and_quiesce(15_000)?;
        oracle.check_quiescent_invariants(world)?;

        let final_merged = world.client_merged_snapshot()?;
        let final_rems = canban_notifications::reminders(&final_merged);
        ensure!(
            !final_rems.iter().any(|r| strip_host(&r.card_id) == raw_a2),
            "Archived card {raw_a2} retained stale reminders: {final_rems:?}"
        );
        world.hit("notifications.archived_suppression");

        Ok(finalize_digest(world))
    })
}
