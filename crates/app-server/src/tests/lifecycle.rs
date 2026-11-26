use super::*;
use crate::lifecycle::idle_candidate;

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
