use super::*;
use crate::lifecycle::idle_candidate;

fn observed(entry: &mut TerminalEntry, status: &str) {
    let run = entry.agent.as_mut().unwrap();
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
    let request = app.snapshot().unwrap().terminals[0].clone();
    let mut entry = admit(&app, &request);
    let mut snapshot = app.snapshot().unwrap();
    for status in ["blocked", "idle", "done", "error"] {
        observed(&mut entry, status);
        snapshot.terminals = vec![entry.clone()];
        assert_eq!(canban_scheduler::active_count(&snapshot), 0, "{status}");
        let mut followup = entry.clone();
        followup.agent.as_mut().unwrap().pending = Some(QueuedAssignment {
            stage: "develop".into(),
            trigger: ExecutionTrigger::Interactive,
            profile: followup.agent.as_ref().unwrap().profile.clone(),
            instruction: "followup".into(),
            on_completed: None,
            require_human_review: false,
            workflow_run_id: None,
            workflow_step: None,
            queued_at: now(),
        });
        snapshot.terminals = vec![followup.clone()];
        assert!(
            !canban_scheduler::eligible(&snapshot, &followup),
            "Retained terminal with unfinished task must not allow duplicate conversation launches ({status})"
        );
    }
    for status in ["working", "unknown", ""] {
        observed(&mut entry, status);
        snapshot.terminals = vec![entry.clone()];
        assert_eq!(canban_scheduler::active_count(&snapshot), 1);
    }
    observed(&mut entry, "blocked");
    entry.agent.as_mut().unwrap().connection_state = "unavailable".into();
    snapshot.terminals = vec![entry.clone()];
    assert_eq!(canban_scheduler::active_count(&snapshot), 1);
    entry.agent.as_mut().unwrap().connection_state = "connected".into();
    entry.agent.as_mut().unwrap().lifecycle.observed_at = Some(now() - 31_000);
    snapshot.terminals = vec![entry.clone()];
    assert_eq!(canban_scheduler::active_count(&snapshot), 1);
    observed(&mut entry, "working");
    snapshot.terminals = vec![entry.clone(), entry.clone(), entry];
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
    let req = app.snapshot().unwrap().terminals[0].clone();
    let run = admit(&app, &req);
    let value = json!({"state":"live","status":{"state":"blocked","kind":"permission","title":"Run command?","msg":"Allow git status?"}});
    app.observe_lifecycle(&run, &value).unwrap();
    let s = app.snapshot().unwrap();
    assert!(s.cards[0].human_gate.is_none());
    assert!(s.cards[0].attention.is_none());
    assert_eq!(s.notices.len(), 1);
    assert!(
        s.terminals[0]
            .agent
            .as_ref()
            .unwrap()
            .lifecycle
            .waiting
            .is_some()
    );
    app.observe_lifecycle(&s.terminals[0], &value).unwrap();
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
    app.call("execution.report",json!({"request_key":"task-block","project_id":c.project_id,"card":c.number,"state":"blocked","message":"Choose a design"})).unwrap();
    let before = app.snapshot().unwrap().cards[0].human_gate.clone();
    app.observe_lifecycle(
        &s.terminals[0],
        &json!({"state":"live","status":{"state":"working"}}),
    )
    .unwrap();
    let s = app.snapshot().unwrap();
    assert!(
        s.terminals[0]
            .agent
            .as_ref()
            .unwrap()
            .lifecycle
            .waiting
            .is_none()
    );
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
    let req = app.snapshot().unwrap().terminals[0].clone();
    let mut entry = admit(&app, &req);
    observed(&mut entry, "idle");
    {
        let run = entry.agent.as_mut().unwrap();
        run.provider_session_id = Some("exact-session".into());
        run.lifecycle.idle_since = Some(now() - 16 * 60_000);
        run.profile.resume_argv = Some(vec!["resume".into(), "{session_id}".into()]);
        run.profile.exit = Some(ExitAction::Input {
            text: "/exit".into(),
        });
    }
    let mut s = app.snapshot().unwrap();
    s.terminals = vec![entry.clone()];
    assert!(idle_candidate(
        &s,
        entry.agent.as_ref().unwrap(),
        15,
        now(),
        false
    ));
    assert!(!idle_candidate(
        &s,
        entry.agent.as_ref().unwrap(),
        0,
        now(),
        false
    ));
    assert!(idle_candidate(
        &s,
        entry.agent.as_ref().unwrap(),
        15,
        now(),
        true
    ));
    entry.agent.as_mut().unwrap().lifecycle.last_interaction_at = Some(now());
    s.terminals = vec![entry.clone()];
    assert!(!idle_candidate(
        &s,
        entry.agent.as_ref().unwrap(),
        15,
        now(),
        false
    ));
    entry.agent.as_mut().unwrap().lifecycle.last_interaction_at = None;
    // Unfinished automatic stage assignments stay alive for user inspection instead of silent reclamation.
    entry.agent.as_mut().unwrap().trigger = "stage_entry".into();
    s.terminals = vec![entry.clone()];
    assert!(!idle_candidate(
        &s,
        entry.agent.as_ref().unwrap(),
        15,
        now(),
        false
    ));
    // Once the automatic assignment completes, it becomes inactive and eligible even without
    // `provider_session_id`, `resume_argv`, or `ExitAction::Input` (e.g. `agy`, `gemini`).
    {
        let run = entry.agent.as_mut().unwrap();
        run.state = "completed".into();
        run.provider_session_id = None;
        run.profile.resume_argv = None;
        run.profile.exit = Some(ExitAction::Manual);
    }
    s.terminals = vec![entry.clone()];
    assert!(idle_candidate(
        &s,
        entry.agent.as_ref().unwrap(),
        15,
        now(),
        false
    ));
    entry.agent.as_mut().unwrap().state = "running".into();
    entry.agent.as_mut().unwrap().trigger = "interactive".into();
    for status in ["working", "unknown", "blocked", "error"] {
        observed(&mut entry, status);
        s.terminals = vec![entry.clone()];
        assert!(!idle_candidate(
            &s,
            entry.agent.as_ref().unwrap(),
            15,
            now(),
            false
        ));
    }
    observed(&mut entry, "idle");
    s.terminals = vec![entry.clone()];
    assert!(idle_candidate(
        &s,
        entry.agent.as_ref().unwrap(),
        15,
        now(),
        false
    ));
}
#[test]
fn overview_is_global_deduplicates_conversations_and_surfaces_unfinished_idle_agents() {
    let (_d, app, p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let req = app.snapshot().unwrap().terminals[0].clone();
    let mut entry = admit(&app, &req);
    let mut s = app.snapshot().unwrap();
    observed(&mut entry, "done");
    entry.agent.as_mut().unwrap().lifecycle.idle_since = Some(now());
    s.terminals = vec![entry.clone()];
    // Automatic assignment pausing in `idle`/`done` while background tasks run stays quiet (<1h).
    let rows = canban_notifications::overview(&s);
    assert_eq!(rows[0].group, "Idle");
    assert!(!rows[0].actionable());
    assert!(rows[0].reasons.is_empty());
    // After 1 continuous hour in `done`/`idle` without reporting or interaction, it alerts the user.
    let stale_since = now().saturating_sub(canban_notifications::UNREPORTED_IDLE_ALERT_MS + 1_000);
    entry.agent.as_mut().unwrap().lifecycle.idle_since = Some(stale_since);
    s.terminals = vec![entry.clone()];
    let rows = canban_notifications::overview(&s);
    assert_eq!(rows[0].group, "Needs attention");
    assert!(rows[0].actionable());
    assert_eq!(rows[0].reasons[0].source, "runtime");
    // Interim `working` report or recent terminal interaction resets the 1-hour stall window.
    s.reports.push(Report {
        id: "rep-interim".into(),
        terminal_id: Some(entry.id.clone()),
        workflow_run_id: None,
        actor: "agent:developer".into(),
        project_id: p.id.clone(),
        card_id: c.id.clone(),
        state: "working".into(),
        message: "Running background verification".into(),
        path: None,
        request_key: "rk-interim".into(),
        fingerprint: "fp-interim".into(),
        created_at: now(),
    });
    let rows = canban_notifications::overview(&s);
    assert_eq!(rows[0].group, "Idle");
    assert!(!rows[0].actionable());
    s.reports.clear();
    // Once reported `completed` without a human gate, the live idle terminal is quiet (`Idle`).
    entry.agent.as_mut().unwrap().state = "completed".into();
    s.terminals = vec![entry.clone()];
    let rows = canban_notifications::overview(&s);
    assert_eq!(rows[0].group, "Idle");
    assert!(!rows[0].actionable());
    s.cards[0].human_gate = Some(HumanGate {
        reason: "Review report".into(),
        terminal_id: Some(entry.id.clone()),
        next_stage: None,
    });
    observed(&mut entry, "blocked");
    s.terminals = vec![entry.clone()];
    let rows = canban_notifications::overview(&s);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].group, "Needs attention");
    assert_eq!(rows[0].reasons.len(), 2);
    observed(&mut entry, "working");
    s.terminals = vec![entry.clone()];
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
            .any(|r| r.project_name == "Other Project" && r.terminal.is_none())
    );
    entry.agent.as_mut().unwrap().pending = Some(QueuedAssignment {
        stage: "develop".into(),
        trigger: ExecutionTrigger::Interactive,
        profile: entry.agent.as_ref().unwrap().profile.clone(),
        instruction: "later".into(),
        on_completed: None,
        require_human_review: false,
        workflow_run_id: None,
        workflow_step: None,
        queued_at: now() + 100,
    });
    s.terminals = vec![entry.clone()];
    let rows = canban_notifications::overview(&s);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter()
            .find(|r| r.terminal.is_some())
            .unwrap()
            .terminal
            .as_ref()
            .unwrap()
            .id,
        entry.id
    );
    s.terminals[0].agent.as_mut().unwrap().connection_state = "unavailable".into();
    assert_eq!(
        canban_notifications::overview(&s)
            .iter()
            .find(|r| r.terminal.is_some())
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
    let req = app.snapshot().unwrap().terminals[0].clone();
    let run = admit(&app, &req);
    let blocked = json!({"state":"live","status":{"state":"blocked","kind":"permission"}});
    app.observe_lifecycle(&run, &blocked).unwrap();
    let run = app
        .store
        .lock()
        .unwrap()
        .write(|db| {
            let mut t: TerminalEntry = db.get(&run.id)?;
            let a = t.agent.as_mut().unwrap();
            a.observed = Some(blocked.clone());
            a.connection_state = "connected".into();
            db.save(&t)?;
            Ok(t)
        })
        .unwrap();
    let mut finished = blocked;
    finished["state"] = json!("finished");
    app.observe_lifecycle(&run, &finished).unwrap();
    let s = app.snapshot().unwrap();
    assert!(
        s.terminals[0]
            .agent
            .as_ref()
            .unwrap()
            .lifecycle
            .waiting
            .is_none()
    );
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
    let request = app.snapshot().unwrap().terminals[0].clone();
    let mut entry = admit(&app, &request);
    let mut s = app.snapshot().unwrap();
    let orig_bin = entry.agent.as_ref().unwrap().profile.command()[0].clone();
    {
        let run = entry.agent.as_mut().unwrap();
        run.state = "completed".into();
        run.location = Some(s.projects[0].work_location());
        run.pending = Some(QueuedAssignment {
            stage: "develop".into(),
            trigger: ExecutionTrigger::StageEntry,
            profile: run.profile.clone(),
            instruction: "next".into(),
            on_completed: None,
            require_human_review: false,
            workflow_run_id: s.workflow_runs.first().map(|w| w.id.clone()),
            workflow_step: None,
            queued_at: now(),
        });
    }
    observed(&mut entry, "idle");
    if !s.workflow_runs.is_empty() {
        s.workflow_runs[0].terminal_id = Some(entry.id.clone());
        s.workflow_runs[0].state = "waiting_agent".into();
    }
    s.terminals = vec![entry.clone()];
    assert!(canban_scheduler::can_continue_in_place(&s, &entry));
    // Evolving preset flags on the same executable (`codex` / `agy`) still reuses the idle session
    // instead of deadlocking `eligible()`, while changing the underlying CLI binary does not.
    entry
        .agent
        .as_mut()
        .unwrap()
        .pending
        .as_mut()
        .unwrap()
        .profile
        .argv = Some(vec![
        orig_bin,
        "--settings".into(),
        "{agy_trusted_settings}".into(),
        "{prompt}".into(),
    ]);
    s.terminals = vec![entry.clone()];
    assert!(canban_scheduler::can_continue_in_place(&s, &entry));
    assert!(canban_scheduler::eligible(&s, &entry));
    let mut switched_bin = entry.clone();
    switched_bin
        .agent
        .as_mut()
        .unwrap()
        .pending
        .as_mut()
        .unwrap()
        .profile
        .argv = Some(vec!["different-agent-cli".into(), "{prompt}".into()]);
    assert!(!canban_scheduler::can_continue_in_place(&s, &switched_bin));
    entry.agent.as_mut().unwrap().close_state = "closing".into();
    s.terminals = vec![entry.clone()];
    assert!(!canban_scheduler::can_continue_in_place(&s, &entry));
    assert!(!canban_scheduler::eligible(&s, &entry));
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
    let request = app.snapshot().unwrap().terminals[0].clone();
    let mut entry = admit(&app, &request);
    let output = format!(
        "{}\nSaved failure context: authentication required",
        "中".repeat(12_000)
    );
    let path = canban_context::run_dir(&app.home, &entry).join("output.txt");
    canban_context::atomic(&path, output.as_bytes()).unwrap();
    {
        let run = entry.agent.as_mut().unwrap();
        run.state = "needs_report".into();
        run.close_state = "closed".into();
        // The last runtime status may still say blocked; a confirmed exit wins.
        run.observed = Some(json!({"state":"finished","exit_code":1,"status":{"state":"blocked"}}));
        run.output_path = Some(canban_context::relative(&app.home, &path).unwrap());
    }
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut card: Card = db.get(&card.id)?;
            card.attention = Some("Agent exited without an explicit outcome".into());
            card.human_gate = Some(HumanGate {
                reason: card.attention.clone().unwrap(),
                terminal_id: Some(entry.id.clone()),
                next_stage: None,
            });
            db.save(&card)?;
            db.save(&entry)
        })
        .unwrap();
    app.config.write().unwrap().hc = "/nonexistent/canban-overview-test-core".into();
    let rows = canban_notifications::overview(&app.snapshot().unwrap());
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.group, "Needs attention");
    assert!(row.agent().unwrap().finished());
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
            json!({"row_id":row.id,"terminal_id":entry.id}),
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
            json!({"row_id":row.id,"terminal_id":"obsolete:1"})
        )
        .is_err()
    );
    assert_eq!(
        app.call("execution.output", json!({"terminal_id":entry.id}))
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
    let request = app.snapshot().unwrap().terminals[0].clone();
    let mut entry = admit(&app, &request);
    {
        let run = entry.agent.as_mut().unwrap();
        run.state = "needs_report".into();
        run.observed = Some(json!({"state":"finished","exit_code":1}));
    }
    app.store.lock().unwrap().write(|db|{
        db.save(&entry)?;
        let mut c:Card=db.get(&card.id)?;attention(db,&mut c,"Agent exited without an explicit outcome. Inspect the retained output before resuming.")?;
        c.human_gate.as_mut().unwrap().terminal_id=Some(entry.id.clone());db.save(&c)
    }).unwrap();
    app.migrate_exit_failures().unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.terminals[0].agent.as_ref().unwrap().state, "failed");
    assert!(s.cards[0].human_gate.is_none());
    assert!(s.cards[0].attention.is_none());
    assert_eq!(canban_notifications::overview(&s)[0].group, "Closed");
    let mut entry = s.terminals[0].clone();
    entry.agent.as_mut().unwrap().state = "needs_report".into();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&entry)?;
            let mut c: Card = db.get(&card.id)?;
            attention(db, &mut c, "Need a product decision")?;
            c.human_gate.as_mut().unwrap().terminal_id = Some(entry.id.clone());
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
    let request = app.snapshot().unwrap().terminals[0].clone();
    let run = admit(&app, &request);
    app.observe_lifecycle(
        &run,
        &json!({"state":"live","status":{"state":"blocked","kind":"permission","title":"Allow?"}}),
    )
    .unwrap();
    app.call("execution.report",json!({"project_id":card.project_id,"card":card.number,"state":"blocked","message":"Choose the target environment","request_key":"explicit-task-gate"})).unwrap();
    let entry = app.snapshot().unwrap().terminals[0].clone();
    app.runtime_missing(&entry).unwrap();
    let s = app.snapshot().unwrap();
    let a = s.terminals[0].agent.as_ref().unwrap();
    assert!(a.finished());
    assert_eq!(a.connection_state, "missing");
    assert!(a.lifecycle.waiting.is_none());
    assert!(a.observed.as_ref().unwrap()["exit_code"].is_null());
    assert_eq!(
        s.cards[0].human_gate.as_ref().unwrap().reason,
        "Choose the target environment"
    );
    let reasons = &canban_notifications::overview(&s)[0].reasons;
    assert_eq!(reasons.len(), 1);
    assert_eq!(reasons[0].source, "task");
    let mut finished = s.terminals[0].clone();
    finished.agent.as_mut().unwrap().observed = Some(json!({"state":"finished","exit_code":7}));
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&finished))
        .unwrap();
    app.runtime_missing(&finished).unwrap();
    assert_eq!(
        app.snapshot().unwrap().terminals[0]
            .agent
            .as_ref()
            .unwrap()
            .observed
            .as_ref()
            .unwrap()["exit_code"],
        7
    );
}
