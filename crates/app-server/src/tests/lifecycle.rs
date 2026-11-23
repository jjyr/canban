use super::*;
use crate::lifecycle::{COMPLETED_IDLE_RECLAIM_MS, idle_candidate, idle_reclaim_deadline};

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
        json!({"view":"full","card_id":c.id,"revision":1,"stage":"develop"}),
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
        json!({"view":"full","card_id":c.id,"revision":1,"stage":"develop"}),
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
        app.events()
            .unwrap()
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
    let events = app.events().unwrap();
    assert!(
        s.notices
            .iter()
            .find(|n| n.id
                == events
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
    assert!(
        !idle_candidate(&s, entry.agent.as_ref().unwrap(), 15, now(), true),
        "Active viewer presence lease must block idle reclamation"
    );
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
        json!({"view":"full","card_id":c.id,"revision":1,"stage":"develop"}),
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
        json!({"view":"full","card_id":c.id,"revision":1,"stage":"develop"}),
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
        json!({"view":"full","card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().terminals[0].clone();
    let mut entry = admit(&app, &request);
    let mut s = app.snapshot().unwrap();
    let orig_bin = entry.agent.as_ref().unwrap().profile.command()[0].clone();
    {
        let run = entry.agent.as_mut().unwrap();
        run.state = "completed".into();
        run.directory = Some(s.projects[0].directory.clone());
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
        json!({"view":"full","card_id":card.id,"revision":1,"stage":"develop"}),
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
        json!({"view":"full","card_id":card.id,"revision":1,"stage":"develop"}),
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
        json!({"view":"full","card_id":card.id,"revision":1,"stage":"develop"}),
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

#[test]
fn agy_screen_refinement_keeps_background_tasks_working_without_idle_polling() {
    let (dir, app, p, c) = setup();
    let hc_script = dir.path().join("refine-hc");
    std::fs::write(
        &hc_script,
        r#"#!/usr/bin/python3
import sys, json, os
base = os.path.dirname(__file__)
for line in sys.stdin:
    req = json.loads(line)
    m = req['method']
    params = req.get('params', {})
    if m == 'hello':
        res = {'generation': 1}
    elif m == 'server.status':
        res = {'core': {'id': 'core-refine'}}
    elif m == 'terminal.read':
        term = params.get('terminal', '')
        with open(base + '/reads.log', 'a') as f:
            f.write(json.dumps({'terminal': term, 'lines': params.get('lines')}) + '\n')
        screen_path = base + '/screen-' + term.replace('/', '_') + '.txt'
        txt = open(screen_path).read() if os.path.exists(screen_path) else ''
        res = {'complete': True, 'how': 'screen', 'text': txt}
    else:
        res = {'ok': True}
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': res}), flush=True)
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hc_script, std::fs::Permissions::from_mode(0o755)).unwrap();

    {
        let mut cfg = app.config();
        cfg.hc = hc_script.to_string_lossy().into_owned();
        cfg.profiles.push(Profile {
            id: "agy".into(),
            preset: Some("agy".into()),
            ..Default::default()
        });
        let resolved = canban_profiles::resolve(cfg, &app.home).unwrap();
        *app.config.write().unwrap() = resolved;
    }
    app.call(
        "project.agents",
        json!({"project_id": p.id, "agent_overrides": {"developer": "agy"}}),
    )
    .unwrap();
    app.observer_running.store(true, Ordering::SeqCst);

    // 1. Admit a Card Role Agent (`developer` -> `agy`) on `t1` and an independent shell on `t2`.
    app.call(
        "card.move",
        json!({"view":"full","card_id": c.id, "revision": 1, "stage": "develop"}),
    )
    .unwrap();
    let queued = app.snapshot().unwrap().terminals[0].clone();
    let mut card_entry = admit(&app, &queued);
    card_entry.agent.as_mut().unwrap().core_id = Some("core-refine".into());
    card_entry.agent.as_mut().unwrap().terminal = Some("t1".into());

    let shell_entry = TerminalEntry {
        id: "shell-agy".into(),
        conversation_id: None,
        number: 2,
        name: Some("Terminal 1".into()),
        origin_project_id: Some(p.id.clone()),
        origin_card_id: None,
        agent_engine: None,
        created_at: now().saturating_sub(5_000),
        runtime: Some(TerminalRuntime {
            core_id: Some("core-refine".into()),
            terminal: Some("t2".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
    };
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&card_entry)?;
            db.save(&shell_entry)
        })
        .unwrap();

    // 2. Both terminals emit raw `idle`/`done` after turn 1 while running background tasks
    //    in the bottom TUI tray.
    std::fs::write(
        dir.path().join("screen-t1.txt"),
        "\
● Running checks in the background.

────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
  ● [22:55:00] cargo nextest run --workspace running
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                        accept-edits · Gemini 4 Carbon · 1 task(s) · /tasks\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("screen-t2.txt"),
        "\
● Running acceptance suite in the background.

────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
  ● [22:55:10] python3 scripts/acceptance-events.py running
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                        accept-edits · Gemini 4 Carbon · 1 task(s) · /tasks\n",
    )
    .unwrap();

    {
        let mut inv = app.inventory.lock().unwrap();
        inv.core = "core-refine".into();
        inv.last_at = now();
        inv.records.insert(
            "t1".into(),
            json!({
                "address": "t1",
                "state": "live",
                "turn": 1,
                "status_ms": 1000,
                "status": {"state": "idle", "app": "agy"},
                "foreground": {"pid": 201, "name": "agy", "argv": ["/opt/homebrew/bin/agy"], "cwd": &p.directory}
            }),
        );
        inv.records.insert(
            "t2".into(),
            json!({
                "address": "t2",
                "state": "live",
                "turn": 1,
                "status_ms": 1000,
                "status": {"state": "done", "app": "agy"},
                "foreground": {"pid": 202, "name": "agy", "argv": ["/opt/homebrew/bin/agy"], "cwd": &p.directory}
            }),
        );
    }

    app.tick().unwrap();
    let s = app.snapshot().unwrap();
    let t1 = s.terminals.iter().find(|t| t.id == card_entry.id).unwrap();
    let a1 = t1.agent.as_ref().unwrap();
    assert_eq!(a1.runtime_status(), "working");
    assert!(!a1.agent_stopped());
    assert_eq!(t1.herdr_status(), HerdrStatus::Working);
    assert_eq!(a1.observed.as_ref().unwrap()["status"]["raw_state"], "idle");
    assert_eq!(
        a1.observed.as_ref().unwrap()["status"]["refined"],
        "agy.background_task"
    );
    assert!(a1.lifecycle.idle_since.is_none());
    assert_eq!(canban_scheduler::active_count(&s), 1);
    assert!(!idle_candidate(&s, a1, 15, u64::MAX, false));
    assert!(canban_types::agent_activity_warning(&c.id, &s.terminals).is_some());

    let t2 = s.terminals.iter().find(|t| t.id == "shell-agy").unwrap();
    assert_eq!(t2.herdr_status(), HerdrStatus::Working);
    assert_eq!(
        t2.runtime.as_ref().unwrap().agent_status.as_deref(),
        Some("working")
    );
    assert!(app.refine_deadline().is_some());

    // Immediate repeated tick within the re-check window reuses cached decision (still 2 probes total).
    let probes_after_first = app.refine_probes();
    assert_eq!(probes_after_first, 2);
    app.tick().unwrap();
    assert_eq!(app.refine_probes(), probes_after_first);

    // 3. Permission block (`status.state = "blocked"`) is NEVER masked by an active Swarm row!
    {
        let mut inv = app.inventory.lock().unwrap();
        inv.last_at = now();
        inv.records.get_mut("t1").unwrap()["status"] = json!({
            "state": "blocked",
            "app": "agy",
            "kind": "permission",
            "title": "Allow command?",
            "msg": "rm -rf /tmp/test"
        });
    }
    app.tick().unwrap();
    let s_blocked = app.snapshot().unwrap();
    let t1_blocked = s_blocked
        .terminals
        .iter()
        .find(|t| t.id == card_entry.id)
        .unwrap();
    assert_eq!(t1_blocked.herdr_status(), HerdrStatus::Blocked);
    assert!(
        t1_blocked
            .agent
            .as_ref()
            .unwrap()
            .lifecycle
            .waiting
            .is_some()
    );

    // 4. Background tasks exit (even if an undismissed Swarm row remains in the tray);
    //    when raw status returns to `done` at turn 2, a single probe marks both `Done` and
    //    clears `refine_deadline` (zero further polling!).
    std::fs::write(
        dir.path().join("screen-t1.txt"),
        "\
────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
  ● Swarm(Card #93 Verification)  verifier: running checks · 3 posts · alt+s board · alt+x dismiss
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                                 accept-edits · Gemini 4 Carbon · low\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("screen-t2.txt"),
        "\
────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                                 accept-edits · Gemini 4 Carbon · low\n",
    )
    .unwrap();
    {
        let mut inv = app.inventory.lock().unwrap();
        inv.last_at = now();
        inv.records.get_mut("t1").unwrap()["turn"] = json!(2);
        inv.records.get_mut("t1").unwrap()["status_ms"] = json!(2000);
        inv.records.get_mut("t1").unwrap()["status"] = json!({"state": "done", "app": "agy"});
        inv.records.get_mut("t2").unwrap()["turn"] = json!(2);
        inv.records.get_mut("t2").unwrap()["status_ms"] = json!(2000);
        inv.records.get_mut("t2").unwrap()["status"] = json!({"state": "done", "app": "agy"});
    }
    app.tick().unwrap();
    let s_done = app.snapshot().unwrap();
    let t1_done = s_done
        .terminals
        .iter()
        .find(|t| t.id == card_entry.id)
        .unwrap();
    assert_eq!(t1_done.herdr_status(), HerdrStatus::Done);
    assert!(t1_done.agent.as_ref().unwrap().agent_stopped());
    assert_eq!(canban_scheduler::active_count(&s_done), 0);

    let t2_done = s_done
        .terminals
        .iter()
        .find(|t| t.id == "shell-agy")
        .unwrap();
    assert_eq!(t2_done.herdr_status(), HerdrStatus::Done);
    assert_eq!(app.refine_deadline(), None);
    assert_eq!(app.refined_working_count(), 0);

    // Subsequent ticks while genuinely idle perform ZERO `terminal.read` probes!
    let probes_when_settled = app.refine_probes();
    app.tick().unwrap();
    app.tick().unwrap();
    assert_eq!(app.refine_probes(), probes_when_settled);
}

#[test]
fn completed_runs_reclaim_after_30s_and_repeatedly_skip_non_idle_transitions_without_limit() {
    let (dir, app, _p, c) = setup();
    let hc_script = dir.path().join("reclaim-hc");
    std::fs::write(
        &hc_script,
        r#"#!/usr/bin/python3
import sys, json, os
base = os.path.dirname(__file__)
for line in sys.stdin:
    req = json.loads(line)
    m = req['method']
    params = req.get('params', {})
    state = open(base + '/mode.txt').read().strip() if os.path.exists(base + '/mode.txt') else 'idle'
    if m == 'hello':
        res = {'generation': 1}
    elif m == 'server.status':
        res = {'core': {'id': 'core-reclaim'}}
    elif m == 'terminal.get':
        get_count = int(open(base + '/gets.txt').read()) if os.path.exists(base + '/gets.txt') else 0
        get_count += 1
        open(base + '/gets.txt', 'w').write(str(get_count))
        effective = state
        if state == 'flip_working_on_second_get' and get_count % 2 == 0:
            effective = 'working'
        elif state == 'idle_then_mismatch':
            effective = 'idle'
        res = {'address': params.get('terminal', 't1'), 'state': 'live', 'status': {'state': effective}}
    elif m == 'terminal.send':
        with open(base + '/sends.jsonl', 'a') as f:
            f.write(json.dumps(params) + '\n')
        if state == 'idle_then_mismatch':
            print(json.dumps({
                'jsonrpc': '2.0',
                'id': req['id'],
                'error': {
                    'code': -32000,
                    'message': 't1 is working, not idle',
                    'data': {'kind': 'status_mismatch', 'hint': 'nothing was typed'}
                }
            }), flush=True)
            continue
        res = {'started': True}
    elif m == 'terminal.kill':
        with open(base + '/kills.jsonl', 'a') as f:
            f.write(json.dumps(params) + '\n')
        res = {'sent': True}
    else:
        res = {'ok': True}
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': res}), flush=True)
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hc_script, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.config.write().unwrap().hc = hc_script.to_string_lossy().into_owned();
    let client = canban_herdr::Client {
        config: app.config(),
    };

    app.call(
        "agent.open",
        json!({"card_id": c.id, "profile_id": "codex"}),
    )
    .unwrap();
    let req = app.snapshot().unwrap().terminals[0].clone();
    let mut entry = admit(&app, &req);
    let base_now = now();
    observed(&mut entry, "done");
    {
        let run = entry.agent.as_mut().unwrap();
        run.core_id = Some("core-reclaim".into());
        run.terminal = Some("t1".into());
        run.profile.exit = Some(ExitAction::Input {
            text: "/exit".into(),
        });
        // Interactive running session still uses `idle_minutes` (15m), not 30s!
        run.state = "running".into();
        run.trigger = ExecutionTrigger::Interactive;
        run.lifecycle.idle_since = Some(base_now.saturating_sub(35_000));
    }
    let mut s = app.snapshot().unwrap();
    s.terminals = vec![entry.clone()];
    assert!(!idle_candidate(
        &s,
        entry.agent.as_ref().unwrap(),
        15,
        base_now,
        false
    ));
    assert_eq!(
        idle_reclaim_deadline(entry.agent.as_ref().unwrap(), 15, 0),
        Some(base_now.saturating_sub(35_000) + 15 * 60_000)
    );

    // Once completed (`completed` or `backlog`), unified 30s (`COMPLETED_IDLE_RECLAIM_MS`) applies,
    // anchored on `max(idle_since, completed_at, last_interaction_at)`.
    {
        let run = entry.agent.as_mut().unwrap();
        run.state = "completed".into();
        run.trigger = ExecutionTrigger::StageEntry;
        run.lifecycle.idle_since = Some(base_now.saturating_sub(60_000));
        run.completed_at = Some(base_now.saturating_sub(10_000));
    }
    s.terminals = vec![entry.clone()];
    assert!(
        !idle_candidate(&s, entry.agent.as_ref().unwrap(), 15, base_now, false),
        "Must wait full 30s after completed_at even if idle_since was earlier"
    );
    assert!(idle_candidate(
        &s,
        entry.agent.as_ref().unwrap(),
        15,
        base_now + 20_000,
        false
    ));
    assert!(!idle_candidate(
        &s,
        entry.agent.as_ref().unwrap(),
        0,
        base_now + 20_000,
        false
    ));
    assert_eq!(COMPLETED_IDLE_RECLAIM_MS, 30_000);

    // Queued `pending` follow-up assignment blocks premature idle reclamation.
    {
        let mut with_pending = entry.clone();
        with_pending.agent.as_mut().unwrap().completed_at = Some(base_now.saturating_sub(35_000));
        with_pending.agent.as_mut().unwrap().pending = Some(QueuedAssignment {
            stage: "review".into(),
            trigger: ExecutionTrigger::StageEntry,
            profile: with_pending.agent.as_ref().unwrap().profile.clone(),
            instruction: "next stage".into(),
            on_completed: None,
            require_human_review: false,
            workflow_run_id: None,
            workflow_step: None,
            queued_at: base_now,
        });
        s.terminals = vec![with_pending.clone()];
        assert!(!idle_candidate(
            &s,
            with_pending.agent.as_ref().unwrap(),
            15,
            base_now,
            false
        ));
    }

    // Persist a completed, 35s-idle candidate and exercise repeated non-idle races (> 5 cycles)
    // without ever tripping `close_state = "error"` or exhausting a retry cap.
    entry.agent.as_mut().unwrap().completed_at = Some(base_now.saturating_sub(35_000));
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&entry))
        .unwrap();

    for mode in [
        "flip_working_on_second_get",
        "idle_then_mismatch",
        "flip_working_on_second_get",
        "idle_then_mismatch",
        "flip_working_on_second_get",
        "idle_then_mismatch",
    ] {
        std::fs::write(dir.path().join("mode.txt"), mode).unwrap();
        let _ = std::fs::remove_file(dir.path().join("gets.txt"));
        app.close_idle(&client).unwrap();
        let after = app.snapshot().unwrap();
        let run = after.terminals[0].agent.as_ref().unwrap();
        assert_eq!(
            run.close_state, "",
            "Non-idle race ({mode}) must roll back close_state cleanly"
        );
        assert!(run.close_requested_at.is_none());
        assert_eq!(run.lifecycle.close_reason, "");
        assert!(run.error.is_none());
        assert!(
            !app.events()
                .unwrap()
                .iter()
                .any(|e| e.kind == "execution.idle_close_failed"),
            "Non-idle race ({mode}) must not emit idle_close_failed"
        );
    }

    // Herdr observing `working` -> `done` after completion is treated as a manual
    // follow-up: clears `idle_since`, sets `interactive_followup`, waits full `idle_minutes`
    // (15m) instead of 30s, then enters `closing` with `close_reason == "idle"`.
    app.observe_lifecycle(
        &app.snapshot().unwrap().terminals[0],
        &json!({"state": "live", "status": {"state": "working"}, "turn": 2}),
    )
    .unwrap();
    {
        let after_work = app.snapshot().unwrap();
        let a = after_work.terminals[0].agent.as_ref().unwrap();
        assert!(a.lifecycle.idle_since.is_none());
        assert!(a.lifecycle.interactive_followup);
    }
    app.observe_lifecycle(
        &app.snapshot().unwrap().terminals[0],
        &json!({"state": "live", "status": {"state": "done"}, "turn": 2}),
    )
    .unwrap();
    std::fs::write(dir.path().join("mode.txt"), "done").unwrap();
    // 31s in `done` is NOT reclaimed because interactive follow-up switched cooldown to 15m.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut cur: TerminalEntry = db.get(&entry.id)?;
            cur.agent.as_mut().unwrap().lifecycle.idle_since = Some(now().saturating_sub(31_000));
            db.save(&cur)
        })
        .unwrap();
    app.close_idle(&client).unwrap();
    assert_eq!(
        app.snapshot().unwrap().terminals[0]
            .agent
            .as_ref()
            .unwrap()
            .close_state,
        ""
    );

    // Once full `idle_minutes` (15m+) elapses in `done`, `close_idle` dispatches exit and enters `closing`.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut cur: TerminalEntry = db.get(&entry.id)?;
            let past = now().saturating_sub(16 * 60_000);
            let a = cur.agent.as_mut().unwrap();
            a.completed_at = Some(past);
            a.lifecycle.idle_since = Some(past);
            db.save(&cur)
        })
        .unwrap();
    app.close_idle(&client).unwrap();
    let final_snap = app.snapshot().unwrap();
    let final_run = final_snap.terminals[0].agent.as_ref().unwrap();
    assert_eq!(final_run.close_state, "closing");
    assert_eq!(final_run.lifecycle.close_reason, "idle");
    assert!(final_run.error.is_none());
}

#[test]
fn viewer_presence_protects_card_and_terminal_without_db_writes_and_drives_reclaim_warning() {
    let (_d, app, _p, c) = setup();
    app.call(
        "agent.open",
        json!({"card_id": c.id, "profile_id": "codex"}),
    )
    .unwrap();
    let req = app.snapshot().unwrap().terminals[0].clone();
    let mut entry = admit(&app, &req);
    observed(&mut entry, "done");
    let t0 = now();
    {
        let run = entry.agent.as_mut().unwrap();
        run.state = "completed".into();
        run.trigger = ExecutionTrigger::StageEntry;
        run.completed_at = Some(t0.saturating_sub(60_000));
        run.lifecycle.idle_since = Some(t0.saturating_sub(60_000));
        run.lifecycle.observed_at = Some(t0);
        run.core_id = Some("local-core".into());
        run.terminal = Some("pty-presence".into());
    }
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&entry))
        .unwrap();

    // Active Card presence (even with terminal pane collapsed: terminal_id = None)
    // protects the Card's managed terminal with zero SQLite writes and no state_revision bump
    // on steady heartbeats. Optional `terminal_size` updates in-memory launch sizing with zero SQLite writes.
    let db_changes_before = app.store.lock().unwrap().db().conn.total_changes();
    let rev_before = app.state_revision.load(std::sync::atomic::Ordering::SeqCst);
    app.call(
        "viewer.presence",
        json!({
            "client_id": "win-1",
            "card_id": c.id,
            "terminal_id": null,
            "terminal_size": {"cols": 108, "rows": 38}
        }),
    )
    .unwrap();
    assert_eq!(
        app.resolve_launch_terminal_size(None),
        canban_herdr::TerminalSize {
            cols: 108,
            rows: 38
        }
    );
    let rev_after_first = app.state_revision.load(std::sync::atomic::Ordering::SeqCst);
    app.call(
        "viewer.presence",
        json!({"client_id": "win-1", "card_id": c.id, "terminal_id": null}),
    )
    .unwrap();
    assert_eq!(
        app.store.lock().unwrap().db().conn.total_changes(),
        db_changes_before,
        "viewer.presence must remain strictly in-memory with zero SQLite writes"
    );
    assert_eq!(
        app.state_revision.load(std::sync::atomic::Ordering::SeqCst),
        rev_after_first,
        "Steady heartbeat must not bump state_revision or trigger sync broadcasts"
    );
    let _ = rev_before;

    let snap = app.snapshot().unwrap();
    let rs_paused = snap.reclaim_status_for(&entry.id).unwrap();
    assert!(rs_paused.paused);
    assert!(!rs_paused.reclaiming_soon);
    assert!(rs_paused.deadline_at.is_none());

    // Clearing presence removes active pause and arms a fresh 30s post-view grace window.
    app.call(
        "viewer.presence",
        json!({"client_id": "win-1", "card_id": null, "terminal_id": null}),
    )
    .unwrap();
    let client = canban_herdr::Client {
        config: app.config(),
    };
    app.close_idle(&client).unwrap();
    let after_clear = app.snapshot().unwrap();
    assert_eq!(
        after_clear.terminals[0].agent.as_ref().unwrap().close_state,
        "",
        "Leaving a viewed managed terminal must anchor a fresh 30s grace window instead of immediate reclaim"
    );
    let rs_grace = after_clear.reclaim_status_for(&entry.id).unwrap();
    assert!(!rs_grace.paused);
    assert!(!rs_grace.reclaiming_soon);
    assert!(rs_grace.deadline_at.is_some());

    // Deterministic lease TTL (25s), post-view grace (30s), warning window (15s), focus reset,
    // and overdue wakeup backoff (5s). Advance past `t0` so prior `win-1` view timestamps never shadow `base`.
    let base = t0 + 1_000_000;
    let mut det_entry = entry.clone();
    det_entry.agent.as_mut().unwrap().completed_at = Some(base - 120_000);
    det_entry.agent.as_mut().unwrap().lifecycle.idle_since = Some(base - 120_000);
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&det_entry))
        .unwrap();
    let det_snap = app.snapshot().unwrap();
    app.presence
        .update("det-win".into(), None, Some(entry.id.clone()), base);

    let at_24s = base + crate::presence::PRESENCE_LEASE_TTL_MS - 1;
    let s_24 = app.compute_reclaim_status(&det_snap, at_24s);
    assert!(s_24[0].paused);
    assert!(!s_24[0].reclaiming_soon);

    let expires_at = base + crate::presence::PRESENCE_LEASE_TTL_MS;
    let s_exp = app.compute_reclaim_status(&det_snap, expires_at);
    assert!(!s_exp[0].paused);
    assert!(!s_exp[0].reclaiming_soon);
    assert_eq!(
        s_exp[0].deadline_at,
        Some(expires_at + COMPLETED_IDLE_RECLAIM_MS)
    );

    // 14s after expiry: still 16s left (> 15s short warning window) -> not warning yet.
    let s_14 = app.compute_reclaim_status(&det_snap, expires_at + 14_000);
    assert!(!s_14[0].reclaiming_soon);

    // 15s after expiry: enters final 15s -> `reclaiming_soon == true`.
    let s_15 = app.compute_reclaim_status(&det_snap, expires_at + 15_000);
    assert!(s_15[0].reclaiming_soon);

    // Focusing / selecting the terminal again immediately clears `reclaiming_soon`.
    app.presence.update(
        "det-win".into(),
        None,
        Some(entry.id.clone()),
        expires_at + 16_000,
    );
    let s_refocus = app.compute_reclaim_status(&det_snap, expires_at + 16_000);
    assert!(s_refocus[0].paused);
    assert!(!s_refocus[0].reclaiming_soon);

    // Clear lease and inspect wake deadlines before warning, before reclaim, and when overdue.
    app.presence
        .update("det-win".into(), None, None, expires_at + 16_000);
    let deadline = expires_at + 16_000 + COMPLETED_IDLE_RECLAIM_MS;
    let warn_at = deadline - crate::lifecycle::reclaim_warning_window_ms(COMPLETED_IDLE_RECLAIM_MS);
    assert_eq!(
        app.reclaim_wake_deadline(&det_snap, expires_at + 16_000),
        Some(warn_at + 1)
    );
    assert_eq!(
        app.reclaim_wake_deadline(&det_snap, warn_at),
        Some(deadline + 1)
    );
    assert_eq!(
        app.reclaim_wake_deadline(&det_snap, deadline + 1_234),
        Some(deadline + 1_234 + 5_000),
        "Overdue runtime wakeup must back off instead of 1ms busy-spinning"
    );

    // Multi-minute (`15m`) follow-up session uses 30s warning window, not 15s.
    let mut long_entry = det_entry.clone();
    long_entry
        .agent
        .as_mut()
        .unwrap()
        .lifecycle
        .interactive_followup = true;
    long_entry.agent.as_mut().unwrap().lifecycle.idle_since = Some(base);
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&long_entry))
        .unwrap();
    let long_snap = app.snapshot().unwrap();
    let long_deadline =
        idle_reclaim_deadline(long_entry.agent.as_ref().unwrap(), 15, expires_at + 16_000).unwrap();
    assert!(!app.compute_reclaim_status(&long_snap, long_deadline - 31_000)[0].reclaiming_soon);
    assert!(app.compute_reclaim_status(&long_snap, long_deadline - 30_000)[0].reclaiming_soon);
}

#[test]
fn herdr_followup_detection_ignores_mere_pty_touch_and_preserves_card_stage() {
    let (_d, app, _p, c) = setup();
    app.call(
        "card.move",
        json!({"view":"full","card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let req = app.snapshot().unwrap().terminals[0].clone();
    let mut entry = admit(&app, &req);
    let t0 = now();
    {
        let run = entry.agent.as_mut().unwrap();
        run.state = "completed".into();
        run.completed_at = Some(t0.saturating_sub(5_000));
        run.completion_turn = Some(2);
        run.observed = Some(json!({"state":"live","turn":2,"status":{"state":"done"}}));
        run.connection_state = "connected".into();
        run.lifecycle.observed_at = Some(t0);
        run.lifecycle.idle_since = Some(t0.saturating_sub(5_000));
        // Mere PTY interaction timestamp without Herdr status/turn change must NOT switch cooldown.
        run.lifecycle.last_interaction_at = Some(t0);
    }
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&entry))
        .unwrap();
    assert!(!crate::lifecycle::is_interactive_followup(
        entry.agent.as_ref().unwrap()
    ));
    assert_eq!(
        crate::lifecycle::idle_reclaim_delay_ms(entry.agent.as_ref().unwrap(), 15),
        Some(COMPLETED_IDLE_RECLAIM_MS)
    );

    // Herdr reporting a new turn (`turn: 3 > completion_turn: 2`) marks `interactive_followup`
    // and keeps the completed Card in `develop` without re-triggering workflow transitions.
    app.observe_lifecycle(
        &app.snapshot().unwrap().terminals[0],
        &json!({"state":"live","turn":3,"status":{"state":"done"}}),
    )
    .unwrap();
    let after = app.snapshot().unwrap();
    let card_after = after.cards.iter().find(|card| card.id == c.id).unwrap();
    assert_eq!(card_after.stage, "develop");
    let run_after = after.terminals[0].agent.as_ref().unwrap();
    assert!(crate::lifecycle::is_interactive_followup(run_after));
    assert_eq!(
        crate::lifecycle::idle_reclaim_delay_ms(run_after, 15),
        Some(15 * 60_000)
    );
}

#[test]
fn cardless_scheduled_presence_grace_and_followup_cooldown_preserve_run_state() {
    let (dir, app, project, _c) = setup();
    let hc_script = dir.path().join("sched-reclaim-hc");
    std::fs::write(
        &hc_script,
        r#"#!/usr/bin/python3
import sys, json, os
base = os.path.dirname(__file__)
for line in sys.stdin:
    req = json.loads(line)
    m = req['method']
    params = req.get('params', {})
    if m == 'hello':
        res = {'generation': 1}
    elif m == 'server.status':
        res = {'core': {'id': 'core-sched'}}
    elif m == 'terminal.send':
        with open(base + '/sends.jsonl', 'a') as f:
            f.write(json.dumps(params) + '\n')
        res = {'started': True}
    elif m == 'terminal.kill':
        res = {'sent': True}
    else:
        res = {'ok': True}
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': res}), flush=True)
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hc_script, std::fs::Permissions::from_mode(0o755)).unwrap();
    {
        let mut cfg = app.config.write().unwrap();
        cfg.hc = hc_script.to_string_lossy().into_owned();
        if let Some(p) = cfg.profiles.iter_mut().find(|p| p.id == "codex") {
            p.exit = Some(ExitAction::Input {
                text: "/exit".into(),
            });
        }
    }

    let sched_id = format!("scheduled:{}:r99", project.id);
    let t0 = now();
    let run = ScheduledRun {
        id: sched_id.clone(),
        schedule_id: "sched-1".into(),
        project_id: project.id.clone(),
        workflow_id: "wf-sched".into(),
        workflow_name: "Nightly".into(),
        filename: "nightly.yaml".into(),
        rule_id: "schedule".into(),
        cron: "0 * * * *".into(),
        number: 99,
        trigger_kind: "manual".into(),
        creates_card: false,
        card_id: None,
        card_number: None,
        workflow_run_id: None,
        start_step: Some("audit".into()),
        current_step: Some("audit".into()),
        profile_id: Some("codex".into()),
        provider_session_id: None,
        terminal_id: Some(sched_id.clone()),
        cwd: project.directory.clone(),
        state: "completed".into(),
        summary: Some("Done".into()),
        report_path: None,
        output_path: Some("done.log".into()),
        error: None,
        steps: Vec::new(),
        scheduled_at: t0.saturating_sub(60_000),
        started_at: Some(t0.saturating_sub(60_000)),
        completed_at: Some(t0.saturating_sub(20_000)),
        close_requested_at: None,
        updated_at: t0,
    };
    let entry = TerminalEntry {
        id: sched_id.clone(),
        conversation_id: None,
        number: 1,
        name: Some("Nightly #99".into()),
        origin_project_id: Some(project.id),
        origin_card_id: None,
        agent_engine: Some("codex".into()),
        runtime: Some(TerminalRuntime {
            core_id: Some("core-sched".into()),
            terminal: Some("pty-sched".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("done".into()),
            agent_turn: Some(1),
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: t0.saturating_sub(60_000),
    };
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&run)?;
            db.save(&entry)
        })
        .unwrap();

    // 1. Viewing the cardless scheduled terminal protects it from auto-reclaim.
    app.call(
        "viewer.presence",
        json!({"client_id": "win-1", "card_id": null, "terminal_id": sched_id}),
    )
    .unwrap();
    app.pump_schedules().unwrap();
    let s1 = app.snapshot().unwrap();
    assert!(s1.scheduled_runs[0].close_requested_at.is_none());
    assert!(s1.reclaim_status_for(&sched_id).unwrap().paused);

    // 2. Manual follow-up observed by Herdr (`working`, turn 2) clears idle and marks
    //    `interactive_followup` without mutating `ScheduledRun.state == "completed"`.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut e: TerminalEntry = db.get(&sched_id)?;
            let rt = e.runtime.as_mut().unwrap();
            rt.agent_status = Some("working".into());
            rt.agent_turn = Some(2);
            db.save(&e)
        })
        .unwrap();
    app.pump_schedules().unwrap();
    let s2 = app.snapshot().unwrap();
    assert_eq!(s2.scheduled_runs[0].state, "completed");
    assert!(
        s2.reclaim_status_for(&sched_id)
            .unwrap()
            .interactive_followup
    );

    // 3. Settling back to `done` and leaving the terminal must NOT immediately reclaim
    //    (must honor interactive `idle_minutes = 15m`, and shows 15m deadline once unviewed).
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut e: TerminalEntry = db.get(&sched_id)?;
            e.runtime.as_mut().unwrap().agent_status = Some("done".into());
            db.save(&e)
        })
        .unwrap();
    app.call(
        "viewer.presence",
        json!({"client_id": "win-1", "card_id": null, "terminal_id": null}),
    )
    .unwrap();
    app.pump_schedules().unwrap();
    let s3 = app.snapshot().unwrap();
    assert!(s3.scheduled_runs[0].close_requested_at.is_none());
    assert_eq!(s3.scheduled_runs[0].state, "completed");
    let rs3 = s3.reclaim_status_for(&sched_id).unwrap();
    assert!(rs3.interactive_followup);
    assert!(rs3.deadline_at.is_some_and(|d| d > now() + 10 * 60_000));

    // 4. Direct completion without prior view/follow-up reclaims cleanly with `rt.state == "closing"`
    //    and `auto_reclaiming == true`.
    app.presence.remove_scheduled_track(&sched_id);
    // Use a fresh unviewed scheduled run to verify immediate unviewed reclaim path cleanly.
    let sched2 = format!("{}2", sched_id);
    let mut run2 = run.clone();
    run2.id = sched2.clone();
    run2.terminal_id = Some(sched2.clone());
    let mut entry2 = entry.clone();
    entry2.id = sched2.clone();
    entry2.number = 2;
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&run2)?;
            db.save(&entry2)
        })
        .unwrap();
    app.pump_schedules().unwrap();
    let s4 = app.snapshot().unwrap();
    let rt2 = s4
        .terminals
        .iter()
        .find(|t| t.id == sched2)
        .unwrap()
        .runtime
        .as_ref()
        .unwrap();
    assert_eq!(rt2.state, "closing");
    assert!(s4.reclaim_status_for(&sched2).unwrap().auto_reclaiming);
}
