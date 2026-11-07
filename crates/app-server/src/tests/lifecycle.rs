use super::*;
use crate::lifecycle::idle_candidate;

fn observed(run: &mut Attempt, status: &str) {
    run.observed = Some(json!({"state":"live","status":{"state":status}}));
    run.connection_state = "connected".into();
    run.lifecycle.observed_at = Some(now());
}
#[test]
fn runtime_budget_releases_waits_but_reserves_unknown_and_keeps_live_conversation_exclusive() {
    let (_d, app, _p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().requests[0].clone();
    let mut run = admit(&app, &request);
    let mut snapshot = app.snapshot().unwrap();
    for status in ["blocked", "idle", "done", "error"] {
        observed(&mut run, status);
        snapshot.attempts = vec![run.clone()];
        assert_eq!(canban_scheduler::active_count(&snapshot), 0, "{status}");
        let mut followup = request.clone();
        followup.id = "followup".into();
        followup.state = "queued".into();
        assert!(
            !canban_scheduler::eligible(&snapshot, &followup),
            "Retained terminal must not allow duplicate conversation launches"
        );
    }
    for status in ["working", "unknown", ""] {
        observed(&mut run, status);
        snapshot.attempts = vec![run.clone()];
        assert_eq!(canban_scheduler::active_count(&snapshot), 1);
    }
    observed(&mut run, "blocked");
    run.connection_state = "unavailable".into();
    snapshot.attempts = vec![run.clone()];
    assert_eq!(canban_scheduler::active_count(&snapshot), 1);
    run.connection_state = "connected".into();
    run.lifecycle.observed_at = Some(now() - 31_000);
    snapshot.attempts = vec![run.clone()];
    assert_eq!(canban_scheduler::active_count(&snapshot), 1);
    observed(&mut run, "working");
    snapshot.attempts = vec![run.clone(), run.clone(), run];
    assert_eq!(canban_scheduler::active_count(&snapshot), 3);
    assert!(canban_scheduler::next(&snapshot, 2).is_none());
}
#[test]
fn runtime_wait_episodes_are_durable_deduplicated_and_never_approve_task_gates() {
    let (_d, app, _p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let req = app.snapshot().unwrap().requests[0].clone();
    let run = admit(&app, &req);
    let value = json!({"state":"live","status":{"state":"blocked","kind":"permission","title":"Run command?","msg":"Allow git status?"}});
    app.observe_lifecycle(&run, &value).unwrap();
    let s = app.snapshot().unwrap();
    assert!(s.cards[0].human_gate.is_none());
    assert!(s.cards[0].attention.is_none());
    assert_eq!(s.notices.len(), 1);
    assert!(s.attempts[0].lifecycle.waiting.is_some());
    app.observe_lifecycle(&s.attempts[0], &value).unwrap();
    assert_eq!(app.snapshot().unwrap().notices.len(), 1);
    let mut changed = value.clone();
    changed["status"]["msg"] = json!("Allow git diff?");
    app.observe_lifecycle(&run, &changed).unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.notices.len(), 1);
    assert!(s.notices[0].message.contains("diff"));
    assert_eq!(
        s.events
            .iter()
            .filter(|e| e.kind == "execution.input_required")
            .count(),
        2
    );
    app.call("execution.report",json!({"request_key":"task-block","execution":{"request_id":req.id,"attempt_number":run.number},"state":"blocked","message":"Choose a design"})).unwrap();
    let before = app.snapshot().unwrap().cards[0].human_gate.clone();
    app.observe_lifecycle(
        &s.attempts[0],
        &json!({"state":"live","status":{"state":"working"}}),
    )
    .unwrap();
    let s = app.snapshot().unwrap();
    assert!(s.attempts[0].lifecycle.waiting.is_none());
    assert_eq!(s.cards[0].human_gate, before);
    assert!(
        s.notices
            .iter()
            .find(|n| n.id
                == s.events
                    .iter()
                    .find(|e| e.kind == "execution.input_required")
                    .unwrap()
                    .data["wait"]["id"]
                    .as_str()
                    .unwrap())
            .unwrap()
            .read
    );
}
#[test]
fn idle_reclamation_requires_exact_resume_timeout_no_viewer_no_unfinished_work() {
    let (_d, app, _p, c) = setup();
    app.call("agent.open", json!({"card_id":c.id,"profile_id":"codex"}))
        .unwrap();
    let req = app.snapshot().unwrap().requests[0].clone();
    let mut run = admit(&app, &req);
    observed(&mut run, "idle");
    run.provider_session_id = Some("exact-session".into());
    run.lifecycle.idle_since = Some(now() - 16 * 60_000);
    let mut s = app.snapshot().unwrap();
    s.requests[0].profile.resume_argv = Some(vec!["resume".into(), "{session_id}".into()]);
    s.requests[0].profile.exit = Some(ExitAction::Input {
        text: "/exit".into(),
    });
    assert!(idle_candidate(&s, &run, 15, now(), false));
    assert!(!idle_candidate(&s, &run, 0, now(), false));
    assert!(!idle_candidate(&s, &run, 15, now(), true));
    run.lifecycle.last_interaction_at = Some(now());
    assert!(!idle_candidate(&s, &run, 15, now(), false));
    run.lifecycle.last_interaction_at = None;
    s.requests[0].trigger = "stage_entry".into();
    assert!(!idle_candidate(&s, &run, 15, now(), false));
    s.requests[0].trigger = "interactive".into();
    for status in ["working", "unknown", "blocked", "error"] {
        observed(&mut run, status);
        assert!(!idle_candidate(&s, &run, 15, now(), false));
    }
    observed(&mut run, "idle");
    run.provider_session_id = None;
    assert!(!idle_candidate(&s, &run, 15, now(), false));
    run.provider_session_id = Some("exact-session".into());
    s.requests[0].profile.resume_argv = None;
    assert!(!idle_candidate(&s, &run, 15, now(), false));
}
#[test]
fn overview_is_global_deduplicates_conversations_and_keeps_review_independent_of_runtime_done() {
    let (_d, app, p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let req = app.snapshot().unwrap().requests[0].clone();
    let mut run = admit(&app, &req);
    let mut s = app.snapshot().unwrap();
    observed(&mut run, "done");
    s.attempts = vec![run.clone()];
    let rows = canban_notifications::overview(&s);
    assert_eq!(rows[0].group, "Idle");
    assert!(!rows[0].actionable());
    s.requests[0].state = "completed".into();
    s.cards[0].human_gate = Some(HumanGate {
        reason: "Review report".into(),
        request_id: Some(req.id.clone()),
        next_stage: None,
    });
    observed(&mut run, "blocked");
    s.attempts = vec![run.clone()];
    let rows = canban_notifications::overview(&s);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].group, "Needs attention");
    assert_eq!(rows[0].reasons.len(), 2);
    observed(&mut run, "working");
    s.attempts = vec![run.clone()];
    let rows = canban_notifications::overview(&s);
    assert_eq!(rows[0].group, "Ready for review");
    let mut other_project = p.clone();
    other_project.id = "other-project".into();
    other_project.name = "Other Project".into();
    s.projects.push(other_project);
    let mut other_card = c.clone();
    other_card.id = "other-card".into();
    other_card.project_id = "other-project".into();
    other_card.attention = Some("No route".into());
    s.cards.push(other_card);
    let rows = canban_notifications::overview(&s);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.iter().filter(|r| r.actionable()).count(), 2);
    assert!(
        rows.iter()
            .any(|r| r.project_name == "Other Project" && r.request.is_none())
    );
    let mut newer = req.clone();
    newer.id = "later-request".into();
    newer.created_at += 100;
    newer.state = "queued".into();
    s.requests.push(newer);
    let rows = canban_notifications::overview(&s);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter()
            .find(|r| r.request.is_some())
            .unwrap()
            .request
            .as_ref()
            .unwrap()
            .id,
        req.id
    );
    s.attempts[0].connection_state = "unavailable".into();
    assert_eq!(
        canban_notifications::overview(&s)
            .iter()
            .find(|r| r.request.is_some())
            .unwrap()
            .group,
        "Needs attention"
    );
}
#[test]
fn scheduling_configuration_preserves_other_settings_and_defaults_idle_timeout() {
    let (_d, app, _p, _c) = setup();
    assert_eq!(app.config().scheduler.idle_minutes, 15);
    app.call("scheduler.configure", json!({"idle_minutes":5}))
        .unwrap();
    app.call("scheduler.configure", json!({"max_agents":4}))
        .unwrap();
    assert_eq!(app.config().scheduler.idle_minutes, 5);
    assert_eq!(app.config().scheduler.max_agents, 4);
    assert!(
        app.call("scheduler.configure", json!({"idle_minutes":-1}))
            .is_err()
    );
}

#[test]
fn terminal_exit_resolves_runtime_wait_even_when_root_status_is_unchanged() {
    let (_d, app, _p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let req = app.snapshot().unwrap().requests[0].clone();
    let run = admit(&app, &req);
    let blocked = json!({"state":"live","status":{"state":"blocked","kind":"permission"}});
    app.observe_lifecycle(&run, &blocked).unwrap();
    let run = app
        .store
        .lock()
        .unwrap()
        .write(|db| {
            let mut a: Attempt = db.get(&run.id)?;
            a.observed = Some(blocked.clone());
            a.connection_state = "connected".into();
            db.save(&a)?;
            Ok(a)
        })
        .unwrap();
    let mut finished = blocked;
    finished["state"] = json!("finished");
    app.observe_lifecycle(&run, &finished).unwrap();
    let s = app.snapshot().unwrap();
    assert!(s.attempts[0].lifecycle.waiting.is_none());
    assert!(s.notices[0].read);
    assert!(s.cards[0].human_gate.is_none());
}
