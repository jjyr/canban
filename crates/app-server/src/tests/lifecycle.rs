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
fn idle_reclamation_covers_all_inactive_managed_agents_including_manual_exit_profiles() {
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
    assert!(idle_candidate(&s, &run, 15, now(), true));
    run.lifecycle.last_interaction_at = Some(now());
    assert!(!idle_candidate(&s, &run, 15, now(), false));
    run.lifecycle.last_interaction_at = None;
    // Unfinished automatic stage assignments stay alive for user inspection instead of silent reclamation.
    s.requests[0].trigger = "stage_entry".into();
    assert!(!idle_candidate(&s, &run, 15, now(), false));
    // Once the automatic assignment completes, it becomes inactive and eligible even without
    // `provider_session_id`, `resume_argv`, or `ExitAction::Input` (e.g. `agy`, `gemini`).
    s.requests[0].state = "completed".into();
    run.provider_session_id = None;
    s.requests[0].profile.resume_argv = None;
    s.requests[0].profile.exit = Some(ExitAction::Manual);
    assert!(idle_candidate(&s, &run, 15, now(), false));
    s.requests[0].state = "running".into();
    s.requests[0].trigger = "interactive".into();
    for status in ["working", "unknown", "blocked", "error"] {
        observed(&mut run, status);
        assert!(!idle_candidate(&s, &run, 15, now(), false));
    }
    observed(&mut run, "idle");
    assert!(idle_candidate(&s, &run, 15, now(), false));
}
#[test]
fn overview_is_global_deduplicates_conversations_and_surfaces_unfinished_idle_agents() {
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
    // Unfinished assignment whose Agent stopped at the prompt immediately alerts the user.
    let rows = canban_notifications::overview(&s);
    assert_eq!(rows[0].group, "Needs attention");
    assert!(rows[0].actionable());
    assert_eq!(rows[0].reasons[0].source, "runtime");
    // Once reported `completed` without a human gate, the live idle terminal is quiet (`Idle`).
    s.requests[0].state = "completed".into();
    let rows = canban_notifications::overview(&s);
    assert_eq!(rows[0].group, "Idle");
    assert!(!rows[0].actionable());
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

#[test]
fn a_closing_idle_terminal_cannot_receive_a_new_assignment_and_reserves_its_slot() {
    let (_d, app, _p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().requests[0].clone();
    let mut run = admit(&app, &request);
    let mut s = app.snapshot().unwrap();
    s.requests[0].state = "completed".into();
    run.state = "completed".into();
    run.location = Some(s.projects[0].work_location());
    observed(&mut run, "idle");
    s.attempts = vec![run];
    let mut next = request;
    next.id = "next-assignment".into();
    next.state = "queued".into();
    if !s.workflow_runs.is_empty() {
        s.workflow_runs[0].request_id = Some(next.id.clone());
        s.workflow_runs[0].state = "waiting_agent".into();
    }
    assert!(canban_scheduler::retained(&s, &next).is_some());
    // Evolving preset flags on the same executable (`codex` / `agy`) still reuses the idle session
    // instead of deadlocking `eligible()`, while changing the underlying CLI binary does not.
    next.profile.argv = Some(vec![
        s.requests[0].profile.command()[0].clone(),
        "--settings".into(),
        "{agy_trusted_settings}".into(),
        "{prompt}".into(),
    ]);
    assert!(canban_scheduler::retained(&s, &next).is_some());
    assert!(canban_scheduler::eligible(&s, &next));
    let mut switched_bin = next.clone();
    switched_bin.profile.argv = Some(vec!["different-agent-cli".into(), "{prompt}".into()]);
    assert!(canban_scheduler::retained(&s, &switched_bin).is_none());
    s.attempts[0].close_state = "closing".into();
    assert!(canban_scheduler::retained(&s, &next).is_none());
    assert!(!canban_scheduler::eligible(&s, &next));
    assert_eq!(canban_scheduler::active_count(&s), 1);
}

#[test]
fn exited_agent_keeps_task_recovery_but_reads_saved_output_without_a_core() {
    let (_dir, app, _project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().requests[0].clone();
    let mut run = admit(&app, &request);
    run.state = "needs_report".into();
    run.close_state = "closed".into();
    // The last runtime status may still say blocked; a confirmed exit wins.
    run.observed = Some(json!({"state":"finished","exit_code":1,"status":{"state":"blocked"}}));
    let output = format!(
        "{}\nSaved failure context: authentication required",
        "中".repeat(12_000)
    );
    let path = canban_context::run_dir(&app.home, &request, run.number).join("output.txt");
    canban_context::atomic(&path, output.as_bytes()).unwrap();
    run.output_path = Some(canban_context::relative(&app.home, &path).unwrap());
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut req: Request = db.get(&request.id)?;
            req.state = "needs_report".into();
            let mut card: Card = db.get(&card.id)?;
            card.attention = Some("Agent exited without an explicit outcome".into());
            card.human_gate = Some(HumanGate {
                reason: card.attention.clone().unwrap(),
                request_id: Some(req.id.clone()),
                next_stage: None,
            });
            db.save(&card)?;
            db.save(&req)?;
            db.save(&run)
        })
        .unwrap();
    app.config.write().unwrap().hc = "/nonexistent/canban-overview-test-core".into();
    let rows = canban_notifications::overview(&app.snapshot().unwrap());
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.group, "Needs attention");
    assert!(row.attempt.as_ref().unwrap().finished());
    assert_eq!(
        row.reasons
            .iter()
            .map(|r| r.source.as_str())
            .collect::<Vec<_>>(),
        vec!["task"]
    );
    let preview = app
        .call(
            "overview.preview",
            json!({"row_id":row.id,"attempt_id":run.id}),
        )
        .unwrap();
    assert!(preview["text"].as_str().unwrap().len() <= 32_768);
    assert!(
        preview["text"]
            .as_str()
            .unwrap()
            .ends_with("authentication required")
    );
    assert_eq!(preview["source"], "Saved execution output (read-only)");
    assert!(preview["observed_at"].is_null());
    assert!(
        app.call(
            "overview.preview",
            json!({"row_id":row.id,"attempt_id":"obsolete:1"})
        )
        .is_err()
    );
    assert_eq!(
        app.call("execution.output", json!({"attempt_id":run.id}))
            .unwrap()["body"],
        output
    );
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut card: Card = db.get(&card.id)?;
            card.attention = None;
            card.human_gate = None;
            db.save(&card)
        })
        .unwrap();
    let rows = canban_notifications::overview(&app.snapshot().unwrap());
    assert_eq!(rows[0].group, "Closed");
    assert!(!rows[0].actionable());
}

#[test]
fn migration_only_clears_the_automatic_exit_gate_and_keeps_explicit_task_gates() {
    let (_dir, app, _project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().requests[0].clone();
    let mut run = admit(&app, &request);
    run.state = "needs_report".into();
    run.observed = Some(json!({"state":"finished","exit_code":1}));
    app.store.lock().unwrap().write(|db|{
        db.save(&run)?;let mut r:Request=db.get(&request.id)?;r.state="needs_report".into();db.save(&r)?;
        let mut c:Card=db.get(&card.id)?;attention(db,&mut c,"Agent exited without an explicit outcome. Inspect the retained output before resuming.")?;
        c.human_gate.as_mut().unwrap().request_id=Some(r.id);db.save(&c)
    }).unwrap();
    app.migrate_exit_failures().unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.attempts[0].state, "failed");
    assert!(s.cards[0].human_gate.is_none());
    assert!(s.cards[0].attention.is_none());
    assert_eq!(canban_notifications::overview(&s)[0].group, "Closed");
    let mut run = s.attempts[0].clone();
    run.state = "needs_report".into();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&run)?;
            let mut c: Card = db.get(&card.id)?;
            attention(db, &mut c, "Need a product decision")?;
            c.human_gate.as_mut().unwrap().request_id = Some(request.id.clone());
            db.save(&c)
        })
        .unwrap();
    app.migrate_exit_failures().unwrap();
    assert_eq!(
        app.snapshot().unwrap().cards[0]
            .human_gate
            .as_ref()
            .unwrap()
            .reason,
        "Need a product decision"
    );
}

#[test]
fn authoritative_missing_resolves_terminal_input_but_keeps_task_decisions_and_known_exit_codes() {
    let (_dir, app, _project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().requests[0].clone();
    let run = admit(&app, &request);
    app.observe_lifecycle(
        &run,
        &json!({"state":"live","status":{"state":"blocked","kind":"permission","title":"Allow?"}}),
    )
    .unwrap();
    app.call("execution.report",json!({"execution":{"request_id":request.id,"attempt_number":run.number},"state":"blocked","message":"Choose the target environment","request_key":"explicit-task-gate"})).unwrap();
    let run = app.snapshot().unwrap().attempts[0].clone();
    app.runtime_missing(&run).unwrap();
    let s = app.snapshot().unwrap();
    assert!(s.attempts[0].finished());
    assert_eq!(s.attempts[0].connection_state, "missing");
    assert!(s.attempts[0].lifecycle.waiting.is_none());
    assert!(s.attempts[0].observed.as_ref().unwrap()["exit_code"].is_null());
    assert_eq!(
        s.cards[0].human_gate.as_ref().unwrap().reason,
        "Choose the target environment"
    );
    let reasons = &canban_notifications::overview(&s)[0].reasons;
    assert_eq!(reasons.len(), 1);
    assert_eq!(reasons[0].source, "task");
    let mut finished = s.attempts[0].clone();
    finished.observed = Some(json!({"state":"finished","exit_code":7}));
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&finished))
        .unwrap();
    app.runtime_missing(&finished).unwrap();
    assert_eq!(
        app.snapshot().unwrap().attempts[0]
            .observed
            .as_ref()
            .unwrap()["exit_code"],
        7
    );
}
