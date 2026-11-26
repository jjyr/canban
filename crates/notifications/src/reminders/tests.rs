use super::*;
use serde_json::json;
fn fixture() -> (Snapshot, Vec<Event>) {
    let mut s = Snapshot::default();
    s.cards.push(serde_json::from_value(json!({"id":"c","project_id":"p","number":1,"title":"Task","body":"","stage":"review","revision":1,"created_at":1})).unwrap());
    (s, Vec::new())
}
fn project(s: &mut Snapshot, events: &[Event]) {
    s.reminders = reminders_from_events(s, events);
    s.sound_event_ids = sound_event_ids_from_events(s, events);
}
fn reply(events: &mut Vec<Event>, id: &str, sequence: u64) {
    events.push(Event {
        id: id.into(),
        project_id: "p".into(),
        card_id: "c".into(),
        sequence,
        kind: "comment.replied".into(),
        actor: "agent:developer".into(),
        message: id.into(),
        data: json!({"question_id":id}),
        created_at: sequence,
    });
}
fn counts_count(s: &Snapshot) -> ReminderCount {
    counts(&reminders(s)).get("c").copied().unwrap_or_default()
}
fn agent_terminal(id: &str, agent: serde_json::Value) -> TerminalEntry {
    TerminalEntry {
        id: id.into(),
        conversation_id: Some(id.into()),
        number: 1,
        name: None,
        origin_project_id: None,
        origin_card_id: None,
        agent_engine: None,
        runtime: None,
        agent: Some(serde_json::from_value(agent).unwrap()),
        created_at: 1,
    }
}

fn push_event(
    events: &mut Vec<Event>,
    id: &str,
    sequence: u64,
    kind: &str,
    actor: &str,
    terminal_id: Option<&str>,
    mut data: serde_json::Value,
) {
    if let Some(tid) = terminal_id {
        data["terminal_id"] = json!(tid);
    }
    events.push(Event {
        id: id.into(),
        project_id: "p".into(),
        card_id: "c".into(),
        sequence,
        kind: kind.into(),
        actor: actor.into(),
        message: id.into(),
        data,
        created_at: sequence,
    });
}

#[test]
fn sound_items_filter_automated_completed_reports_and_in_budget_conflicts_while_keeping_badges() {
    let (mut s, mut events) = fixture();
    // 1. Automated stage `completed` report without human_gate or @agent question:
    //    must remain a visual unread Reminder, but MUST NOT produce a sound item.
    push_event(
        &mut events,
        "ev-auto-done",
        1,
        "execution.reported",
        "agent:developer",
        Some("req-1"),
        json!({"report_id":"rep-auto","state":"completed"}),
    );
    project(&mut s, &events);
    assert_eq!(
        counts_count(&s),
        ReminderCount {
            total: 1,
            unread: 1,
            pending: 0
        }
    );
    assert!(sound_items(&s).is_empty());

    // 2. Automatic in-budget merge conflict routed back to Agent: silent.
    push_event(
        &mut events,
        "ev-conflict-rework",
        2,
        "review.merge_conflict",
        "system",
        None,
        json!({"attempt": 1}),
    );
    project(&mut s, &events);
    assert!(sound_items(&s).is_empty());

    // 3. Same `completed` report when echoed for a user's `@agent` question: audible!
    push_event(
        &mut events,
        "ev-question-echo",
        3,
        "comment.replied",
        "agent:developer",
        Some("req-1"),
        json!({"report_id":"rep-auto","question_id":"q-1"}),
    );
    project(&mut s, &events);
    assert_eq!(sound_items(&s), vec!["report:rep-auto"]);

    // Reading the card clears the unread reply/report from sound_items.
    s.card_reads.push(CardRead {
        id: "c".into(),
        project_id: "p".into(),
        card_id: "c".into(),
        sequence: 3,
    });
    assert!(sound_items(&s).is_empty());
}

#[test]
fn sound_items_cover_permissions_gates_backlog_replies_failures_and_merges() {
    let (mut s, mut events) = fixture();
    let mut delivery = DeliveryTracker::default();
    project(&mut s, &events);
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    // 1a. Runtime permission/input wait (`wait:*`)
    s.terminals.push(agent_terminal(
        "c:codex",
        json!({
            "project_id":"p","card_id":"c","number":1,"stage":"develop","trigger":"stage_entry",
            "profile":Config::default().profiles[0],"instruction":"Dev","state":"running",
            "terminal":"t1","cwd":"/tmp","input_path":"in","connection_state":"connected",
            "observed":{"state":"live","status":{"state":"blocked","kind":"permission","title":"Run cmd","msg":"ls"}},
            "lifecycle":{"observed_at":canban_types::now(),"waiting":{"id":"w1","kind":"permission","title":"Run cmd","message":"ls","since":1}},
            "updated_at":1
        }),
    ));
    project(&mut s, &events);
    assert_eq!(delivery.observe(&sound_items(&s)), vec!["wait:w1"]);
    // Resolving the wait clears it from active sound_items.
    s.terminals.clear();
    project(&mut s, &events);
    assert!(sound_items(&s).is_empty());

    // 1b. Agent returned to Backlog (`report --state backlog`)
    push_event(
        &mut events,
        "ev-backlog",
        1,
        "execution.reported",
        "agent:developer",
        None,
        json!({"report_id":"rep-backlog","state":"backlog"}),
    );
    project(&mut s, &events);
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["report:rep-backlog"]
    );

    // 1c. Direct `comment.replied` (`canban reply`)
    reply(&mut events, "q-direct", 2);
    project(&mut s, &events);
    assert_eq!(delivery.observe(&sound_items(&s)), vec!["reply:q-direct"]);

    // 1d. Explicit workflow approval / human gate on completed step
    s.terminals.push(agent_terminal(
        "c:reviewer",
        json!({
            "project_id":"p","card_id":"c","number":1,"stage":"review","trigger":"stage_entry",
            "profile":Config::default().profiles[0],"instruction":"Rev","state":"completed","updated_at":2
        }),
    ));
    push_event(
        &mut events,
        "ev-rev-done",
        3,
        "execution.reported",
        "agent:developer",
        Some("c:reviewer"),
        json!({"report_id":"rep-rev","state":"completed"}),
    );
    s.cards[0].human_gate = Some(HumanGate {
        reason: "Approve deployment".into(),
        terminal_id: Some("c:reviewer".into()),
        next_stage: None,
    });
    project(&mut s, &events);
    assert_eq!(delivery.observe(&sound_items(&s)), vec!["report:rep-rev"]);
    s.cards[0].human_gate = None;

    // 2. Agent failures (`report --state failed` and unreported exit `execution.failed`)
    push_event(
        &mut events,
        "ev-rep-fail",
        4,
        "execution.reported",
        "agent:developer",
        None,
        json!({"report_id":"rep-fail","state":"failed"}),
    );
    push_event(
        &mut events,
        "ev-proc-fail",
        5,
        "execution.failed",
        "system",
        None,
        json!({"attempt":1,"exit_code":1}),
    );
    project(&mut s, &events);
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["event:ev-proc-fail", "report:rep-fail"]
    );

    // 3. Merge failures (`review.merge_failed`, `review.upstream_sync_failed`, `github.pr_merge_attention`)
    push_event(
        &mut events,
        "ev-merge-fail",
        6,
        "review.merge_failed",
        "system",
        None,
        json!({"target_branch":"main"}),
    );
    push_event(
        &mut events,
        "ev-sync-fail",
        7,
        "review.upstream_sync_failed",
        "system",
        None,
        json!({"target_branch":"main"}),
    );
    project(&mut s, &events);
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["event:ev-merge-fail", "event:ev-sync-fail"]
    );

    // 4a. Manual user merge (`actor == "user"`) stays silent (UI shows immediate toast)
    push_event(
        &mut events,
        "ev-manual-merge",
        8,
        "review.merged",
        "user",
        None,
        json!({"target_branch":"main","request_key":"manual-1"}),
    );
    project(&mut s, &events);
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    // 4b. Automatic Queue merge (`actor == "system"`) and GitHub PR delivery (`actor == "github"`) chime once!
    push_event(
        &mut events,
        "ev-auto-merge",
        9,
        "review.merged",
        "system",
        None,
        json!({"target_branch":"main","request_key":"workflow-deliver:run1:1"}),
    );
    push_event(
        &mut events,
        "ev-gh-merge",
        10,
        "github.pr_delivered",
        "github",
        None,
        json!({"number":42,"commit":"abc"}),
    );
    project(&mut s, &events);
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["merged:c:pr:42", "merged:c:workflow-deliver:run1:1"]
    );
    // Repeated snapshots never replay already-delivered merge chimes.
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    // Archived cards clear sound items cleanly.
    s.cards[0].archived_at = Some(99);
    project(&mut s, &events);
    assert!(sound_items(&s).is_empty());
}

#[test]
fn unfinished_idle_turns_and_ephemeral_cli_agents_keep_visual_reminders_silent_while_blocked_chimes()
 {
    let (mut s, mut events) = fixture();
    let mut delivery = DeliveryTracker::default();
    project(&mut s, &events);
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    // Interactive Card Agent finishes Turn 1 without closing -> creates pending visual
    // reminder (`idle:c:codex:1`), but MUST NOT play a notification sound.
    s.terminals.push(agent_terminal(
        "c:codex",
        json!({
            "project_id":"p","card_id":"c","number":1,"stage":"develop","trigger":"interactive",
            "profile":Config::default().profiles[0],"instruction":"Chat","state":"running",
            "terminal":"t-int","cwd":"/tmp","input_path":"in","connection_state":"connected",
            "observed":{"state":"live","turn":1,"status":{"state":"idle"}},
            "lifecycle":{"observed_at":canban_types::now(),"idle_since":10},
            "updated_at":1
        }),
    ));
    project(&mut s, &events);
    assert_eq!(counts_count(&s).pending, 1);
    assert_eq!(reminders(&s)[0].id, "idle:c:codex:1");
    assert!(sound_items(&s).is_empty());

    // Automatic stage Agent transiently entering `idle` before reporting (< 1 hour)
    // produces neither a pending notification badge nor a sound (background task protection).
    s.terminals[0].agent.as_mut().unwrap().trigger = ExecutionTrigger::parse("stage_entry");
    s.terminals[0].agent.as_mut().unwrap().lifecycle.idle_since = Some(canban_types::now());
    project(&mut s, &events);
    assert_eq!(counts_count(&s).pending, 0);
    assert!(reminders(&s).is_empty());
    assert!(sound_items(&s).is_empty());

    // If the automatic stage Agent remains in the same `idle`/`done` state continuously for
    // >= 1 hour without reporting or user interaction, the fallback surfaces `stalled:*`
    // in `Needs you` AND plays an audible notification once.
    let stalled_anchor =
        canban_types::now().saturating_sub(crate::UNREPORTED_IDLE_ALERT_MS + 5_000);
    s.terminals[0].agent.as_mut().unwrap().lifecycle.idle_since = Some(stalled_anchor);
    project(&mut s, &events);
    assert_eq!(counts_count(&s).pending, 1);
    let expected_stalled_id = format!("stalled:c:codex:{stalled_anchor}");
    assert_eq!(reminders(&s)[0].id, expected_stalled_id);
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec![expected_stalled_id]
    );
    // Repeated snapshots while still stalled do not replay the chime.
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    // Restore interactive trigger for subsequent interactive turn tests.
    s.terminals[0].agent.as_mut().unwrap().trigger = ExecutionTrigger::parse("interactive");
    s.terminals[0].agent.as_mut().unwrap().lifecycle.idle_since = Some(10);

    // User replies -> Agent resumes working -> pending clears.
    s.terminals[0].agent.as_mut().unwrap().observed =
        Some(json!({"state":"live","turn":2,"status":{"state":"working"}}));
    project(&mut s, &events);
    assert!(reminders(&s).is_empty());
    assert!(sound_items(&s).is_empty());

    // Turn 2 finishes (`done`) -> visual pending reminder updates, still silent!
    s.terminals[0].agent.as_mut().unwrap().observed =
        Some(json!({"state":"live","turn":2,"status":{"state":"done"}}));
    project(&mut s, &events);
    assert_eq!(counts_count(&s).pending, 1);
    assert_eq!(reminders(&s)[0].id, "idle:c:codex:2");
    assert!(sound_items(&s).is_empty());

    // If the same idle Card also has an unread `@agent` reply, the reply still chimes!
    reply(&mut events, "q-while-idle", 1);
    project(&mut s, &events);
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["reply:q-while-idle"]
    );
    s.card_reads.push(CardRead {
        id: "c".into(),
        project_id: "p".into(),
        card_id: "c".into(),
        sequence: 1,
    });
    assert!(sound_items(&s).is_empty());

    // Once reported `completed`, the live idle terminal becomes quiet (no pending reminder or sound).
    s.terminals[0].agent.as_mut().unwrap().state = "completed".into();
    project(&mut s, &events);
    assert!(reminders(&s).is_empty());
    assert!(sound_items(&s).is_empty());

    // Ephemeral CLI Agent (`agent_engine = Some("agy")`):
    // Both Turn 0 launch and Turn 1 `idle`/`done` stay silent; `blocked` and `error` DO chime!
    s.terminals.push(TerminalEntry {
        id: "ephemeral-agy".into(),
        conversation_id: None,
        number: 2,
        name: Some("Antigravity 1".into()),
        origin_project_id: None,
        origin_card_id: Some("c".into()),
        agent_engine: Some("agy".into()),
        runtime: Some(TerminalRuntime {
            core_id: Some("core".into()),
            terminal: Some("pty-ephemeral".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("idle".into()),
            agent_turn: Some(0),
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: 2,
    });
    assert!(sound_items(&s).is_empty());

    s.terminals[1].runtime.as_mut().unwrap().agent_turn = Some(1);
    assert!(sound_items(&s).is_empty());

    s.terminals[1].runtime.as_mut().unwrap().agent_status = Some("done".into());
    assert!(sound_items(&s).is_empty());

    s.terminals[1].runtime.as_mut().unwrap().agent_status = Some("blocked".into());
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["terminal-attn:ephemeral-agy:blocked"]
    );

    s.terminals[1].runtime.as_mut().unwrap().agent_status = Some("error".into());
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["terminal-attn:ephemeral-agy:error"]
    );

    // Manual Shell (`agent_engine: None`) where user ran `claude` (`observed_agent: Some("claude")`)
    // chimes on `blocked` (keyed by RuntimeWait ID when present) and `error`, stays silent on `done`/`idle`!
    s.terminals.push(TerminalEntry {
        id: "shell-manual-claude".into(),
        conversation_id: None,
        number: 3,
        name: Some("Terminal 2".into()),
        origin_project_id: None,
        origin_card_id: None,
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            core_id: Some("core".into()),
            terminal: Some("pty-shell".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("working".into()),
            agent_turn: Some(1),
            observed_agent: Some("claude".into()),
            waiting: None,
        }),
        agent: None,
        created_at: 3,
    });
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    let manual_rt = s.terminals[2].runtime.as_mut().unwrap();
    manual_rt.agent_status = Some("blocked".into());
    manual_rt.waiting = Some(RuntimeWait {
        id: "w-bash-1".into(),
        kind: "permission".into(),
        title: "Allow bash?".into(),
        message: "rm -rf target".into(),
        since: 10,
    });
    assert_eq!(delivery.observe(&sound_items(&s)), vec!["wait:w-bash-1"]);

    // Exiting back to zsh clears wait/status -> silent.
    let manual_rt = s.terminals[2].runtime.as_mut().unwrap();
    manual_rt.observed_agent = None;
    manual_rt.agent_status = None;
    manual_rt.waiting = None;
    assert!(delivery.observe(&sound_items(&s)).is_empty());
}

#[test]
fn pr_ci_results_notify_and_failures_play_sound_while_passes_stay_silent() {
    let (mut s, mut events) = fixture();
    let mut delivery = DeliveryTracker::default();
    project(&mut s, &events);
    delivery.baseline(&sound_items(&s));

    push_event(
        &mut events,
        "ci-ok",
        1,
        "github.pr.ci_succeeded",
        "github",
        None,
        json!({"number": 148, "state": "success"}),
    );
    project(&mut s, &events);
    let rems = reminders(&s);
    assert_eq!(rems.len(), 1);
    assert_eq!(rems[0].id, "ci-pass:ci-ok");
    assert!(rems[0].unread);
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    push_event(
        &mut events,
        "ci-bad",
        2,
        "github.pr.ci_failed",
        "github",
        None,
        json!({"number": 148, "state": "failure"}),
    );
    project(&mut s, &events);
    let rems = reminders(&s);
    assert_eq!(rems.len(), 2);
    assert!(rems.iter().any(|r| r.id == "ci-fail:ci-bad" && r.unread));
    assert_eq!(delivery.observe(&sound_items(&s)), vec!["event:ci-bad"]);

    s.card_reads.push(CardRead {
        id: "c".into(),
        project_id: "p".into(),
        card_id: "c".into(),
        sequence: 2,
    });
    assert!(reminders(&s).is_empty());
}

#[test]
fn pr_updates_notify_skip_self_actions_and_chime_only_on_changes_requested_or_closed() {
    let (mut s, mut events) = fixture();
    let mut delivery = DeliveryTracker::default();
    project(&mut s, &events);
    delivery.baseline(&sound_items(&s));

    // Self-action comment & commit are silent and skipped from unread notifications.
    push_event(
        &mut events,
        "pr-upd-self",
        1,
        "github.pr.updated",
        "github",
        None,
        json!({"number": 149, "self_action": true}),
    );
    push_event(
        &mut events,
        "pr-cmt-self",
        2,
        "github.pr.commented",
        "github:jjy",
        None,
        json!({"number": 149, "self_action": true}),
    );
    project(&mut s, &events);
    assert!(reminders(&s).is_empty());
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    // External update, comment, and APPROVED review notify silently.
    push_event(
        &mut events,
        "pr-upd-ext",
        3,
        "github.pr.updated",
        "github",
        None,
        json!({"number": 149, "self_action": false}),
    );
    push_event(
        &mut events,
        "pr-cmt-ext",
        4,
        "github.pr.commented",
        "github:alice",
        None,
        json!({"number": 149, "self_action": false}),
    );
    push_event(
        &mut events,
        "pr-rev-ok",
        5,
        "github.pr.reviewed",
        "github:alice",
        None,
        json!({"number": 149, "state": "APPROVED", "self_action": false}),
    );
    project(&mut s, &events);
    let rems = reminders(&s);
    assert_eq!(rems.len(), 3);
    assert!(rems.iter().any(|r| r.id == "pr-update:pr-upd-ext"));
    assert!(rems.iter().any(|r| r.id == "pr-update:pr-cmt-ext"));
    assert!(rems.iter().any(|r| r.id == "pr-update:pr-rev-ok"));
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    // External CHANGES_REQUESTED review and unmerged close both chime.
    push_event(
        &mut events,
        "pr-rev-changes",
        6,
        "github.pr.reviewed",
        "github:bob",
        None,
        json!({"number": 149, "state": "CHANGES_REQUESTED", "self_action": false}),
    );
    project(&mut s, &events);
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["event:pr-rev-changes"]
    );

    push_event(
        &mut events,
        "pr-closed",
        7,
        "github.pr.state_changed",
        "github",
        None,
        json!({"number": 149, "state": "closed", "merged": false, "self_action": false}),
    );
    project(&mut s, &events);
    assert_eq!(delivery.observe(&sound_items(&s)), vec!["event:pr-closed"]);
}

#[test]
fn checking_connection_state_does_not_emit_transient_recovery_reminder() {
    let (mut s, events) = fixture();
    s.terminals.push(agent_terminal(
        "c:codex",
        json!({
            "project_id":"p","card_id":"c","number":1,"stage":"develop","trigger":"interactive",
            "profile":Config::default().profiles[0],"instruction":"Chat","state":"blocked",
            "terminal":"t-gate","cwd":"/tmp","input_path":"in","connection_state":"checking",
            "observed":{"state":"live","turn":1,"status":{"state":"done"}},
            "lifecycle":{"observed_at":null,"idle_since":null},
            "updated_at":1
        }),
    ));
    project(&mut s, &events);
    assert!(
        reminders(&s).is_empty(),
        "transient post-restart checking state should not emit recovery reminders: {:?}",
        reminders(&s)
    );

    // Once the probe reports an actual disconnect, recovery surfaces immediately.
    s.terminals[0].agent.as_mut().unwrap().connection_state = "unavailable".into();
    project(&mut s, &events);
    let rems = reminders(&s);
    assert_eq!(rems.len(), 1);
    assert_eq!(rems[0].id, "recovery:c:codex");
}

#[test]
fn reconcile_runtime_reminders_matches_full_projection_and_preserves_event_reminders() {
    let (mut s, mut events) = fixture();
    reply(&mut events, "q-keep", 1);
    s.terminals.push(agent_terminal(
        "c:codex",
        json!({
            "project_id":"p","card_id":"c","number":1,"stage":"develop","trigger":"interactive",
            "profile":Config::default().profiles[0],"instruction":"Chat","state":"running",
            "terminal":"t-1","cwd":"/tmp","input_path":"in","connection_state":"connected",
            "observed":{"state":"live","turn":1,"status":{"state":"working"}},
            "lifecycle":{"observed_at":u64::MAX,"idle_since":null},
            "updated_at":1
        }),
    ));
    project(&mut s, &events);
    assert_eq!(
        reminders(&s).into_iter().map(|r| r.id).collect::<Vec<_>>(),
        vec!["reply:q-keep"]
    );

    // Transition to blocked with RuntimeWait via partial RuntimeState.
    let mut blocked_snap = s.clone();
    {
        let a = blocked_snap.terminals[0].agent.as_mut().unwrap();
        a.observed = Some(json!({"state":"live","turn":1,"status":{"state":"blocked"}}));
        a.lifecycle.waiting = Some(RuntimeWait {
            id: "w-partial".into(),
            kind: "permission".into(),
            title: "Allow".into(),
            message: "Run cargo test".into(),
            since: 42,
        });
    }
    let blocked_rt = RuntimeState {
        terminals: blocked_snap.terminals.clone(),
        ..Default::default()
    };
    s.apply_runtime_state(blocked_rt);
    reconcile_runtime_reminders(&mut s);

    let mut full = s.clone();
    project(&mut full, &events);
    assert_eq!(reminders(&s), reminders(&full));
    assert_eq!(sound_items(&s), sound_items(&full));

    // Transition to idle turn 1 -> replaces wait:* with idle:* while keeping unread reply:*.
    let mut idle_rt_term = s.terminals[0].clone();
    {
        let a = idle_rt_term.agent.as_mut().unwrap();
        a.lifecycle.waiting = None;
        a.lifecycle.idle_since = Some(100);
        a.observed = Some(json!({"state":"live","turn":1,"status":{"state":"idle"}}));
    }
    s.apply_runtime_state(RuntimeState {
        terminals: vec![idle_rt_term],
        ..Default::default()
    });
    reconcile_runtime_reminders(&mut s);
    project(&mut full, &events);
    let mut full_reconcile_check = full.clone();
    // Apply exact same terminal state onto full for comparison
    full_reconcile_check.terminals = s.terminals.clone();
    project(&mut full_reconcile_check, &events);
    assert_eq!(reminders(&s), reminders(&full_reconcile_check));
    assert_eq!(sound_items(&s), sound_items(&full_reconcile_check));

    // Resume working -> clears idle:* while preserving unread reply:q-keep.
    let mut work_rt_term = s.terminals[0].clone();
    {
        let a = work_rt_term.agent.as_mut().unwrap();
        a.lifecycle.idle_since = None;
        a.observed = Some(json!({"state":"live","turn":2,"status":{"state":"working"}}));
    }
    s.apply_runtime_state(RuntimeState {
        terminals: vec![work_rt_term],
        ..Default::default()
    });
    reconcile_runtime_reminders(&mut s);
    assert_eq!(
        reminders(&s).into_iter().map(|r| r.id).collect::<Vec<_>>(),
        vec!["reply:q-keep"]
    );
}
