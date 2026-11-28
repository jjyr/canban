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

        // 5. Daily Inbox: cutoff preference, `inbox.triage` (defer -> bounce -> virtual-clock wake,
        //    dismiss -> undo/active), GitHub project stacking, and `terminal.preview` + `terminal.send`!
        let _ = world.client_call(
            "utilities.configure",
            json!({ "inbox_cutoff_minutes": 360 }),
            None,
        )?;
        let util_state = world.client_call("utilities.state", json!({}), None)?;
        ensure!(
            util_state["preferences"]["inbox_cutoff_minutes"].as_u64() == Some(360),
            "utilities.configure failed to save inbox_cutoff_minutes: {util_state:?}"
        );

        let defer_until = world.now_ms() + 30_000;
        let card_key = format!("card:{cid_a1}");
        let triage_res = world.client_call(
            "inbox.triage",
            json!({
                "items": [{
                    "id": card_key,
                    "state": "deferred",
                    "defer_until": defer_until,
                }]
            }),
            None,
        )?;
        ensure!(
            triage_res["accepted"] == true && triage_res["updated"].as_u64() == Some(1),
            "inbox.triage defer failed: {triage_res:?}"
        );

        // Bounce local-server to verify SQLite v25 `inbox_entries` durability and cutoff persistence.
        world.bounce_server("local-server")?;
        world.heal_and_quiesce(15_000)?;
        let util_after_bounce = world.client_call("utilities.state", json!({}), None)?;
        ensure!(
            util_after_bounce["preferences"]["inbox_cutoff_minutes"].as_u64() == Some(360),
            "inbox_cutoff_minutes lost across local bounce: {util_after_bounce:?}"
        );

        let merged_deferred = world.client_merged_snapshot()?;
        let now_deferred = world.now_ms();
        let inbox_deferred = canban_notifications::build_daily_inbox(
            &merged_deferred,
            &[
                canban_notifications::GitHubInboxNotification {
                    thread_id: "th-1".into(),
                    repository: "acme/app".into(),
                    kind: "pr".into(),
                    number: Some(42),
                    title: "Fix sync".into(),
                    reason: "review_requested".into(),
                    unread: true,
                    updated_at_ms: now_deferred,
                    url: "https://github.com/acme/app/pull/42".into(),
                    author: Some("alice".into()),
                    body: None,
                    linked_card_id: None,
                    linked_card_number: None,
                },
                canban_notifications::GitHubInboxNotification {
                    thread_id: "th-2".into(),
                    repository: "acme/app".into(),
                    kind: "issue".into(),
                    number: Some(43),
                    title: "Follow-up".into(),
                    reason: "mention".into(),
                    unread: true,
                    updated_at_ms: now_deferred.saturating_sub(1_000),
                    url: "https://github.com/acme/app/issues/43".into(),
                    author: Some("bob".into()),
                    body: None,
                    linked_card_id: None,
                    linked_card_number: None,
                },
                canban_notifications::GitHubInboxNotification {
                    thread_id: "th-3".into(),
                    repository: "acme/app".into(),
                    kind: "issue".into(),
                    number: Some(44),
                    title: "Subscribed noise".into(),
                    reason: "subscribed".into(),
                    unread: true,
                    updated_at_ms: now_deferred.saturating_sub(2_000),
                    url: "https://github.com/acme/app/issues/44".into(),
                    author: Some("carol".into()),
                    body: None,
                    linked_card_id: None,
                    linked_card_number: None,
                },
                // Historical unread notification (< day_start_ms) must route to `earlier_github`,
                // never polluting Today's bundle or actionable badge count.
                canban_notifications::GitHubInboxNotification {
                    thread_id: "th-old".into(),
                    repository: "acme/app".into(),
                    kind: "issue".into(),
                    number: Some(12),
                    title: "Two days ago".into(),
                    reason: "mention".into(),
                    unread: true,
                    updated_at_ms: now_deferred.saturating_sub(2 * 86_400_000),
                    url: "https://github.com/acme/app/issues/12".into(),
                    author: Some("dave".into()),
                    body: None,
                    linked_card_id: None,
                    linked_card_number: None,
                },
            ],
            now_deferred,
            0,
            360,
        );
        ensure!(
            inbox_deferred
                .deferred_items
                .iter()
                .any(|d| d.key == card_key),
            "Deferred card missing from inbox.deferred_items after bounce"
        );
        let gh_needs_count = inbox_deferred
            .needs_you
            .iter()
            .filter(|n| matches!(n.kind, canban_notifications::NeedsYouKind::GitHub { .. }))
            .count();
        ensure!(
            gh_needs_count == 2
                && inbox_deferred.github_watching.len() == 1
                && inbox_deferred.github_watching[0].items.len() == 1
                && inbox_deferred.earlier_github.len() == 1
                && inbox_deferred.earlier_github[0].number == Some(12),
            "Expected 2 unread direct GitHub blockers in Tier 1 needs_you, 1 passive subscribed in Tier 3 github_watching, and 1 historical in Tier 4 earlier_github; got gh_needs={gh_needs_count} watching={:?} earlier={:?}",
            inbox_deferred.github_watching,
            inbox_deferred.earlier_github
        );
        ensure!(
            inbox_deferred.actionable_count == inbox_deferred.needs_you.len(),
            "Strict 1:1 parity: actionable_count ({}) must equal needs_you.len() ({})",
            inbox_deferred.actionable_count,
            inbox_deferred.needs_you.len()
        );

        // Verify newly synced GitHub cards never flood `open_cards`:
        // - historical synced card -> `carryover_cards`
        // - today-updated synced card -> bundled in `github_stacks` (`is_synced_card == true`)
        {
            let mut synced_snap = merged_deferred.clone();
            let pid_first = synced_snap.projects[0].id.clone();
            let old_iso = canban_notifications::format_rfc3339_utc_ms(
                now_deferred.saturating_sub(3 * 86_400_000),
            );
            let today_iso = canban_notifications::format_rfc3339_utc_ms(now_deferred);
            let mk_gh_card = |id: &str, num: u64, issue_id: u64| canban_types::Card {
                id: id.into(),
                project_id: pid_first.clone(),
                number: num,
                board_order: None,
                title: format!("Synced #{issue_id}"),
                body: String::new(),
                stage: "backlog".into(),
                revision: 1,
                created_by: "github".into(),
                assignee: None,
                source_trust: Some("external".into()),
                hold_auto_merge: false,
                github_links: Vec::new(),
                content: canban_types::CardContent::GitHubIssue {
                    source: canban_types::GitHubIssueRef {
                        repository_id: 1,
                        repository: "acme/synced".into(),
                        issue_id,
                        number: issue_id,
                        url: format!("https://github.com/acme/synced/issues/{issue_id}"),
                    },
                },
                context_notes: None,
                attention: None,
                archived_at: None,
                archive: None,
                archive_error: None,
                removed_at: None,
                worktree: None,
                branch: None,
                base_commit: None,
                human_gate: None,
                pull_request: None,
                pull_request_history: Vec::new(),
                delivery_mode: None,
                delivery: None,
                resolution: None,
                workspace: None,
                workspace_history: Vec::new(),
                schedule_source: None,
                created_at: now_deferred, // Synced right now!
            };
            synced_snap.cards.push(mk_gh_card("gh-old-card", 901, 501));
            synced_snap
                .cards
                .push(mk_gh_card("gh-today-card", 902, 502));
            synced_snap
                .github_issues
                .push(canban_types::GitHubIssueSnapshot {
                    id: format!("{pid_first}:501"),
                    project_id: pid_first.clone(),
                    source: canban_types::GitHubIssueRef {
                        repository_id: 1,
                        repository: "acme/synced".into(),
                        issue_id: 501,
                        number: 501,
                        url: "https://github.com/acme/synced/issues/501".into(),
                    },
                    title: "Synced #501".into(),
                    body: String::new(),
                    state: "open".into(),
                    author_id: 1,
                    author_login: "ext".into(),
                    labels: Vec::new(),
                    assignees: Vec::new(),
                    updated_at: old_iso,
                    synced_at: now_deferred,
                    pr_head_sha: None,
                    pr_checks: None,
                });
            synced_snap
                .github_issues
                .push(canban_types::GitHubIssueSnapshot {
                    id: format!("{pid_first}:502"),
                    project_id: pid_first.clone(),
                    source: canban_types::GitHubIssueRef {
                        repository_id: 1,
                        repository: "acme/synced".into(),
                        issue_id: 502,
                        number: 502,
                        url: "https://github.com/acme/synced/issues/502".into(),
                    },
                    title: "Synced #502".into(),
                    body: String::new(),
                    state: "open".into(),
                    author_id: 1,
                    author_login: "ext".into(),
                    labels: Vec::new(),
                    assignees: Vec::new(),
                    updated_at: today_iso,
                    synced_at: now_deferred,
                    pr_head_sha: None,
                    pr_checks: None,
                });
            let synced_inbox =
                canban_notifications::build_daily_inbox(&synced_snap, &[], now_deferred, 0, 360);
            ensure!(
                !synced_inbox
                    .open_cards
                    .iter()
                    .any(|c| c.card_id == "gh-old-card" || c.card_id == "gh-today-card"),
                "Untouched synced GitHub cards leaked into open_cards: {:?}",
                synced_inbox.open_cards
            );
            ensure!(
                synced_inbox
                    .carryover_cards
                    .iter()
                    .any(|c| c.card_id == "gh-old-card"),
                "Historical synced GitHub card missing from carryover_cards"
            );
            ensure!(
                synced_inbox.github_watching.iter().any(|s| {
                    s.synced_card_count == 1
                        && s.items.iter().any(|i| {
                            i.card_id.as_deref() == Some("gh-today-card") && i.is_synced_card
                        })
                }),
                "Today-updated untouched synced GitHub card should route to Tier 3 github_watching: {:?}",
                synced_inbox.github_watching
            );

            // Verify ProjectPulseItem dynamically reads custom workflow column names (skipping
            // the non-executing initial queue column) rather than hardcoding `dev` / `rev`.
            let mut custom_snap = synced_snap.clone();
            if let Some(proj) = custom_snap.projects.first_mut() {
                let custom_pid = proj.id.clone();
                proj.workflow.stages = vec![
                    canban_types::Stage {
                        id: "triage".into(),
                        name: "Triage".into(),
                        execute: false,
                        profile: None,
                        instruction: String::new(),
                        on_completed: None,
                        require_human_review: false,
                    },
                    canban_types::Stage {
                        id: "spec".into(),
                        name: "Spec".into(),
                        execute: true,
                        profile: None,
                        instruction: String::new(),
                        on_completed: Some("qa".into()),
                        require_human_review: false,
                    },
                    canban_types::Stage {
                        id: "qa".into(),
                        name: "QA".into(),
                        execute: false,
                        profile: None,
                        instruction: String::new(),
                        on_completed: None,
                        require_human_review: true,
                    },
                ];
                custom_snap.cards.retain(|c| c.project_id != custom_pid);
                for (cid, num, stg, br) in [
                    ("custom-triage", 101, "triage", None),
                    ("custom-spec-1", 102, "spec", Some("feat/custom-spec")),
                    ("custom-qa-1", 103, "qa", Some("feat/custom-qa")),
                ] {
                    let mut c: canban_types::Card = serde_json::from_value(json!({
                        "id": cid,
                        "project_id": custom_pid,
                        "number": num,
                        "title": format!("Card {num}"),
                        "body": "",
                        "stage": stg,
                        "revision": 1,
                        "created_at": now_deferred.saturating_sub(60_000),
                        "branch": br,
                    }))?;
                    c.branch = br.map(str::to_owned);
                    custom_snap.cards.push(c);
                }
                let custom_inbox = canban_notifications::build_daily_inbox(
                    &custom_snap,
                    &[],
                    now_deferred,
                    0,
                    360,
                );
                let pulse = custom_inbox
                    .project_pulses
                    .iter()
                    .find(|p| p.project_id == custom_pid)
                    .ok_or_else(|| anyhow::anyhow!("Missing custom project pulse"))?;
                let pairs: Vec<(&str, usize)> = pulse
                    .stage_counts
                    .iter()
                    .map(|s| (s.stage_name.as_str(), s.count))
                    .collect();
                ensure!(
                    pairs == vec![("Spec", 1), ("QA", 1)],
                    "Expected dynamic column names [('Spec', 1), ('QA', 1)], got {pairs:?}"
                );
                let qa_work = custom_inbox
                    .open_cards
                    .iter()
                    .find(|c| c.card_id == "custom-qa-1");
                ensure!(
                    qa_work.is_some_and(
                        |c| c.stage == "QA" && c.branch.as_deref() == Some("feat/custom-qa")
                    ),
                    "Expected InboxCardWork to carry resolved column name and branch, got {qa_work:?}"
                );
            }
        }

        // Advance past defer_until -> item automatically resurfaces in Today!
        world.advance(Duration::from_secs(35))?;
        let inbox_woken = canban_notifications::build_daily_inbox(
            &world.client_merged_snapshot()?,
            &[],
            world.now_ms(),
            0,
            360,
        );
        ensure!(
            !inbox_woken.deferred_items.iter().any(|d| d.key == card_key),
            "Deferred card failed to resurface after virtual clock passed defer_until"
        );

        // Dismiss then undo (`active`) -> transitions done_today -> active cleanly.
        let _ = world.client_call(
            "inbox.triage",
            json!({
                "items": [{
                    "id": card_key,
                    "state": "dismissed",
                    "day_start_ms": inbox_woken.day_start_ms,
                }]
            }),
            None,
        )?;
        world.advance(Duration::from_millis(150))?;
        let inbox_dismissed = canban_notifications::build_daily_inbox(
            &world.client_merged_snapshot()?,
            &[],
            world.now_ms(),
            0,
            360,
        );
        ensure!(
            inbox_dismissed
                .done_today_items
                .iter()
                .any(|d| d.key == card_key),
            "Dismissed card missing from done_today_items"
        );
        let _ = world.client_call(
            "inbox.triage",
            json!({ "items": [{ "id": card_key, "state": "active" }] }),
            None,
        )?;
        world.advance(Duration::from_millis(150))?;
        world.hit("notifications.daily_inbox_triage");

        // Exercise `terminal.preview` (ANSI-stripped tail) & `terminal.send` (inline reply)
        // on a live remote terminal (`remote-a`).
        let created_term = world.client_call(
            "terminal.create",
            json!({ "project_id": pid_a, "host_id": "remote-a" }),
            Some(&pid_a),
        )?;
        let remote_tid = created_term["id"].as_str().unwrap().to_owned();
        let raw_remote_tid = strip_host(&remote_tid);
        world.wait_until(5_000, |w| {
            let snap = w.server("remote-a").unwrap().snapshot()?;
            Ok(snap.terminals.iter().any(|t| {
                t.id == raw_remote_tid
                    && t.runtime
                        .as_ref()
                        .is_some_and(|r| r.terminal.is_some() && r.state != "finished")
            }))
        })?;
        let remote_herdr_addr = world
            .server("remote-a")
            .unwrap()
            .snapshot()?
            .terminals
            .into_iter()
            .find(|t| t.id == raw_remote_tid)
            .and_then(|t| t.runtime?.terminal)
            .unwrap();
        world.herdr("remote-a").unwrap().append_output(
            &remote_herdr_addr,
            ["\x1b[32mBuild passed\x1b[0m", "Ready for input?"],
        );

        let preview = world.client_call(
            "terminal.preview",
            json!({ "terminal_id": remote_tid, "lines": 8 }),
            Some(&pid_a),
        )?;
        let preview_lines: Vec<&str> = preview["lines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .collect();
        ensure!(
            preview_lines.iter().any(|l| l.contains("Build passed"))
                && !preview["text"].as_str().unwrap_or("").contains("\x1b["),
            "terminal.preview failed to return ANSI-stripped tail lines: {preview:?}"
        );

        let sent = world.client_call(
            "terminal.send",
            json!({ "terminal_id": remote_tid, "text": "yes", "submit": true }),
            Some(&pid_a),
        )?;
        ensure!(
            sent["accepted"] == true,
            "terminal.send failed on remote terminal: {sent:?}"
        );
        let sent_inputs = world.herdr("remote-a").unwrap().sent_inputs();
        ensure!(
            sent_inputs
                .iter()
                .any(|s| s.terminal == remote_herdr_addr && s.text.contains("yes")),
            "SimHerdr did not observe terminal.send input: {sent_inputs:?}"
        );
        world.hit("notifications.daily_inbox_terminal_inline");
        oracle.check_quiescent_invariants(world)?;

        Ok(finalize_digest(world))
    })
}
