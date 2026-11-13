use super::*;
use serde_json::json;
fn fixture() -> Snapshot {
    let mut s = Snapshot::default();
    s.cards.push(serde_json::from_value(json!({"id":"c","project_id":"p","number":1,"title":"Task","body":"","stage":"review","revision":1,"created_at":1})).unwrap());
    s
}
fn reply(s: &mut Snapshot, id: &str, sequence: u64) {
    s.events.push(Event {
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
#[test]
fn card_counts_items_and_project_counts_cards_without_progress_or_own_comments() {
    let mut s = fixture();
    reply(&mut s, "one", 1);
    reply(&mut s, "two", 2);
    s.cards[0].attention = Some("Check configuration".into());
    let mut event = s.events[0].clone();
    event.id = "own".into();
    event.actor = "user".into();
    event.sequence = 3;
    s.events.push(event);
    let mut event = s.events[0].clone();
    event.id = "working".into();
    event.kind = "execution.reported".into();
    event.data = json!({"state":"working"});
    event.sequence = 4;
    s.events.push(event);
    let counts = counts(&reminders(&s));
    assert_eq!(counts.len(), 1);
    assert_eq!(
        counts["c"],
        ReminderCount {
            total: 3,
            unread: 2,
            pending: 1
        }
    );
    s.card_reads.push(CardRead {
        id: "c".into(),
        project_id: "p".into(),
        card_id: "c".into(),
        sequence: 4,
    });
    assert_eq!(
        counts_count(&s),
        ReminderCount {
            total: 1,
            unread: 0,
            pending: 1
        }
    );
    s.cards[0].archived_at = Some(5);
    assert!(reminders(&s).is_empty());
}
fn counts_count(s: &Snapshot) -> ReminderCount {
    counts(&reminders(s))["c"]
}
#[test]
fn report_reply_and_human_gate_share_one_item_even_without_a_resource_file() {
    let mut s = fixture();
    let a: Attempt = serde_json::from_value(json!({"id":"r","project_id":"p","card_id":"c","number":1,"stage":"review","trigger":"stage_entry","profile":Config::default().profiles[0],"instruction":"Review","state":"completed","created_at":1})).unwrap();
    s.attempts.push(a);
    s.cards[0].human_gate = Some(HumanGate {
        reason: "Review needed".into(),
        attempt_id: Some("r".into()),
        next_stage: None,
    });
    reply(&mut s, "report-event", 1);
    s.events[0].kind = "execution.reported".into();
    s.events[0].data =
        json!({"attempt_id":"r","report_id":"report","state":"completed","path":null});
    reply(&mut s, "echo", 2);
    s.events[1].data = json!({"attempt_id":"r","report_id":"report","question_id":"q","path":null});
    assert_eq!(
        counts_count(&s),
        ReminderCount {
            total: 1,
            unread: 1,
            pending: 1
        }
    );
    s.card_reads.push(CardRead {
        id: "c".into(),
        project_id: "p".into(),
        card_id: "c".into(),
        sequence: 2,
    });
    assert_eq!(
        counts_count(&s),
        ReminderCount {
            total: 1,
            unread: 0,
            pending: 1
        }
    );
    s.cards[0].human_gate = None;
    assert!(reminders(&s).is_empty());
}
#[test]
fn delivery_baselines_and_duplicate_snapshots_are_silent() {
    let mut s = fixture();
    reply(&mut s, "old", 1);
    let mut delivery = DeliveryTracker::default();
    assert!(delivery.observe(&reminders(&s)).is_empty());
    reply(&mut s, "new", 2);
    assert_eq!(delivery.observe(&reminders(&s)), vec!["reply:new"]);
    assert!(delivery.observe(&reminders(&s)).is_empty());
    reply(&mut s, "offline", 3);
    delivery.baseline(&reminders(&s));
    assert!(delivery.observe(&reminders(&s)).is_empty());
    reply(&mut s, "live", 4);
    assert_eq!(delivery.observe(&reminders(&s)), vec!["reply:live"]);
}

fn push_event(
    s: &mut Snapshot,
    id: &str,
    sequence: u64,
    kind: &str,
    actor: &str,
    attempt_id: Option<&str>,
    mut data: serde_json::Value,
) {
    if let Some(aid) = attempt_id {
        data["attempt_id"] = json!(aid);
    }
    s.events.push(Event {
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
    let mut s = fixture();
    // 1. Automated stage `completed` report without human_gate or @agent question:
    //    must remain a visual unread Reminder, but MUST NOT produce a sound item.
    push_event(
        &mut s,
        "ev-auto-done",
        1,
        "execution.reported",
        "agent:developer",
        Some("req-1"),
        json!({"report_id":"rep-auto","state":"completed"}),
    );
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
        &mut s,
        "ev-conflict-rework",
        2,
        "review.merge_conflict",
        "system",
        None,
        json!({"attempt": 1}),
    );
    assert!(sound_items(&s).is_empty());

    // 3. Same `completed` report when echoed for a user's `@agent` question: audible!
    push_event(
        &mut s,
        "ev-question-echo",
        3,
        "comment.replied",
        "agent:developer",
        Some("req-1"),
        json!({"report_id":"rep-auto","question_id":"q-1"}),
    );
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
    let mut s = fixture();
    let mut delivery = DeliveryTracker::default();
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    // 1a. Runtime permission/input wait (`wait:*`)
    let attempt: Attempt = serde_json::from_value(json!({
        "id":"r1:1","project_id":"p","card_id":"c","number":1,"stage":"develop","trigger":"stage_entry",
        "profile":Config::default().profiles[0],"instruction":"Dev","state":"running",
        "terminal":"t1","cwd":"/tmp","input_path":"in","connection_state":"connected",
        "observed":{"state":"live","status":{"state":"blocked","kind":"permission","title":"Run cmd","msg":"ls"}},
        "lifecycle":{"observed_at":canban_types::now(),"waiting":{"id":"w1","kind":"permission","title":"Run cmd","message":"ls","since":1}},
        "created_at":1
    }))
    .unwrap();
    s.attempts.push(attempt);
    assert_eq!(delivery.observe(&sound_items(&s)), vec!["wait:w1"]);
    // Resolving the wait clears it from active sound_items.
    s.attempts.clear();
    assert!(sound_items(&s).is_empty());

    // 1b. Agent returned to Backlog (`report --state backlog`)
    push_event(
        &mut s,
        "ev-backlog",
        1,
        "execution.reported",
        "agent:developer",
        None,
        json!({"report_id":"rep-backlog","state":"backlog"}),
    );
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["report:rep-backlog"]
    );

    // 1c. Direct `comment.replied` (`canban reply`)
    reply(&mut s, "q-direct", 2);
    assert_eq!(delivery.observe(&sound_items(&s)), vec!["reply:q-direct"]);

    // 1d. Explicit workflow approval / human gate on completed step
    let att_rev: Attempt = serde_json::from_value(json!({
        "id":"r2","project_id":"p","card_id":"c","number":1,"stage":"review","trigger":"stage_entry",
        "profile":Config::default().profiles[0],"instruction":"Rev","state":"completed","created_at":2
    }))
    .unwrap();
    s.attempts.push(att_rev);
    push_event(
        &mut s,
        "ev-rev-done",
        3,
        "execution.reported",
        "agent:developer",
        Some("r2"),
        json!({"report_id":"rep-rev","state":"completed"}),
    );
    s.cards[0].human_gate = Some(HumanGate {
        reason: "Approve deployment".into(),
        attempt_id: Some("r2".into()),
        next_stage: None,
    });
    assert_eq!(delivery.observe(&sound_items(&s)), vec!["report:rep-rev"]);
    s.cards[0].human_gate = None;

    // 2. Agent failures (`report --state failed` and unreported exit `execution.failed`)
    push_event(
        &mut s,
        "ev-rep-fail",
        4,
        "execution.reported",
        "agent:developer",
        None,
        json!({"report_id":"rep-fail","state":"failed"}),
    );
    push_event(
        &mut s,
        "ev-proc-fail",
        5,
        "execution.failed",
        "system",
        None,
        json!({"attempt":1,"exit_code":1}),
    );
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["event:ev-proc-fail", "report:rep-fail"]
    );

    // 3. Merge failures (`review.merge_failed`, `review.upstream_sync_failed`, `github.pr_merge_attention`)
    push_event(
        &mut s,
        "ev-merge-fail",
        6,
        "review.merge_failed",
        "system",
        None,
        json!({"target_branch":"main"}),
    );
    push_event(
        &mut s,
        "ev-sync-fail",
        7,
        "review.upstream_sync_failed",
        "system",
        None,
        json!({"target_branch":"main"}),
    );
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["event:ev-merge-fail", "event:ev-sync-fail"]
    );

    // 4a. Manual user merge (`actor == "user"`) stays silent (UI shows immediate toast)
    push_event(
        &mut s,
        "ev-manual-merge",
        8,
        "review.merged",
        "user",
        None,
        json!({"target_branch":"main","request_key":"manual-1"}),
    );
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    // 4b. Automatic Queue merge (`actor == "system"`) and GitHub PR delivery (`actor == "github"`) chime once!
    push_event(
        &mut s,
        "ev-auto-merge",
        9,
        "review.merged",
        "system",
        None,
        json!({"target_branch":"main","request_key":"workflow-deliver:run1:1"}),
    );
    push_event(
        &mut s,
        "ev-gh-merge",
        10,
        "github.pr_delivered",
        "github",
        None,
        json!({"number":42,"commit":"abc"}),
    );
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["merged:c:pr:42", "merged:c:workflow-deliver:run1:1"]
    );
    // Repeated snapshots never replay already-delivered merge chimes.
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    // Archived cards clear sound items cleanly.
    s.cards[0].archived_at = Some(99);
    assert!(sound_items(&s).is_empty());
}

#[test]
fn unfinished_idle_turns_and_ephemeral_cli_agents_notify_per_turn_without_false_launch_chime() {
    let mut s = fixture();
    let mut delivery = DeliveryTracker::default();
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    // Interactive Card Agent finishes Turn 1 without closing -> notifies with `idle:r-int:1:1`.
    let attempt: Attempt = serde_json::from_value(json!({
        "id":"r-int:1","project_id":"p","card_id":"c","number":1,"stage":"develop","trigger":"interactive",
        "profile":Config::default().profiles[0],"instruction":"Chat","state":"running",
        "terminal":"t-int","cwd":"/tmp","input_path":"in","connection_state":"connected",
        "observed":{"state":"live","turn":1,"status":{"state":"idle"}},
        "lifecycle":{"observed_at":canban_types::now(),"idle_since":10},
        "created_at":1
    }))
    .unwrap();
    s.attempts.push(attempt);
    assert_eq!(counts_count(&s).pending, 1);
    assert_eq!(delivery.observe(&sound_items(&s)), vec!["idle:r-int:1:1"]);
    // Repeated polls on the same turn do not re-chime.
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    // User replies -> Agent resumes working -> pending clears.
    s.attempts[0].observed = Some(json!({"state":"live","turn":2,"status":{"state":"working"}}));
    assert!(reminders(&s).is_empty());
    assert!(sound_items(&s).is_empty());

    // Turn 2 finishes -> new turn ID chimes again!
    s.attempts[0].observed = Some(json!({"state":"live","turn":2,"status":{"state":"done"}}));
    assert_eq!(delivery.observe(&sound_items(&s)), vec!["idle:r-int:1:2"]);

    // Once reported `completed`, the live idle terminal becomes quiet (no pending reminder or sound).
    s.attempts[0].state = "completed".into();
    assert!(reminders(&s).is_empty());
    assert!(sound_items(&s).is_empty());

    // Ephemeral CLI Agent (`agent_engine = Some("agy")`):
    // Turn 0 at fresh launch does NOT chime; Turn 1 completion and `blocked` DO chime!
    s.terminals.push(TerminalEntry {
        id: "ephemeral-agy".into(),
        conversation_id: None,
        number: 2,
        name: None,
        manual_number: Some(1),
        origin_card_id: Some("c".into()),
        agent_engine: Some("agy".into()),
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("core".into()),
            terminal: Some("pty-ephemeral".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("idle".into()),
            agent_turn: Some(0),
        }),
        created_at: 2,
    });
    assert!(delivery.observe(&sound_items(&s)).is_empty());

    s.terminals[0].runtime.as_mut().unwrap().agent_turn = Some(1);
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["terminal-idle:ephemeral-agy:1"]
    );

    s.terminals[0].runtime.as_mut().unwrap().agent_status = Some("blocked".into());
    assert_eq!(
        delivery.observe(&sound_items(&s)),
        vec!["terminal-attn:ephemeral-agy:blocked"]
    );
}
