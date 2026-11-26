use super::*;
use crate::lifecycle::{COMPLETED_IDLE_RECLAIM_MS, idle_candidate};

fn observed(entry: &mut TerminalEntry, status: &str) {
    let run = entry.agent.as_mut().unwrap();
    run.observed = Some(json!({"state":"live","status":{"state":status}}));
    run.connection_state = "connected".into();
    run.lifecycle.observed_at = Some(now());
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
        let mut c:Card=db.get(&card.id)?;attention(db,&mut c,"Agent exited without an explicit outcome. Inspect the retained output before resuming.",app.now_ms())?;
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
            attention(db, &mut c, "Need a product decision", app.now_ms())?;
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
        host_id: String::new(),
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
