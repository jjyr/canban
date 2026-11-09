use super::*;
fn setup() -> (tempfile::TempDir, Arc<AppServer>, Project, Card) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name":"Example","directory":dir.path().join("source")}),
        )
        .unwrap(),
    )
    .unwrap();
    let c: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id":p.id,"title":"Implement change","body":"Task context"}),
        )
        .unwrap(),
    )
    .unwrap();
    (dir, app, p, c)
}
fn admit(app: &AppServer, request: &Request) -> Attempt {
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut req: Request = db.get(&request.id)?;
            req.attempt += 1;
            req.state = "running".into();
            db.save(&req)?;
            let run = Attempt {
                runtime_home: None,
                location: None,
                owner_core_id: None,
                workspace: None,
                lifecycle: Lifecycle::default(),
                id: Attempt::key(&req.id, req.attempt),
                request_id: req.id.clone(),
                project_id: req.project_id.clone(),
                card_id: req.card_id.clone(),
                number: req.attempt,
                state: "running".into(),
                core_id: Some("core-test".into()),
                terminal: Some("t1".into()),
                cwd: "/tmp".into(),
                input_path: String::new(),
                output_path: None,
                observed: None,
                error: None,
                created_at: now(),
                conversation_id: None,
                provider_session_id: None,
                close_state: String::new(),
                turn_ended_at: None,
                response_path: None,
                completed_at: None,
                completion_turn: None,
                close_requested_at: None,
                connection_state: String::new(),
            };
            db.save(&run)?;
            Ok(run)
        })
        .unwrap()
}
#[test]
fn state_history_and_queue_are_atomic_and_retries_do_not_dispatch_twice() {
    let (_d, app, p, c) = setup();
    let move_p = json!({"card_id":c.id,"revision":1,"stage":"develop","request_key":"move-once"});
    app.call("card.move", move_p.clone()).unwrap();
    app.call("card.move", move_p).unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.requests.len(), 1);
    assert_eq!(snap.cards[0].stage, "develop");
    let run = admit(&app, &snap.requests[0]);
    let report = json!({"request_key":"finish-once","execution":{"request_id":run.request_id,"attempt_number":run.number},"state":"completed","message":"done","body":"# Full report\nVerified change"});
    app.call("execution.report", report.clone()).unwrap();
    app.call("execution.report", report).unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.cards[0].stage, "review");
    assert_eq!(snap.requests.len(), 2);
    assert_eq!(snap.reports.len(), 1);
    assert!(
        snap.events
            .windows(2)
            .all(|w| w[0].sequence < w[1].sequence)
    );
    let conflict = app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"backlog","request_key":"move-once"}),
    );
    assert!(conflict.is_err());
    drop(app);
    let reopened = AppServer::open(_d.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    assert_eq!(reopened.snapshot().unwrap().projects[0].id, p.id);
    assert_eq!(reopened.snapshot().unwrap().cards[0].stage, "review");
    let wal: String = reopened
        .store
        .lock()
        .unwrap()
        .db()
        .conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(wal, "wal");
}
#[test]
fn failed_revision_leaves_no_event_or_request() {
    let (_d, app, _p, c) = setup();
    assert!(
        app.call(
            "card.move",
            json!({"card_id":c.id,"revision":99,"stage":"develop"})
        )
        .is_err()
    );
    let s = app.snapshot().unwrap();
    assert_eq!(s.events.len(), 1);
    assert!(s.requests.is_empty());
    assert_eq!(s.cards[0].stage, "backlog");
}
#[test]
fn mentions_coalesce_in_the_card_profile_queue_without_stage_callback() {
    let (_d, app, _p, c) = setup();
    app.call(
        "comment.add",
        json!({"card_id":c.id,"body":"@codex explain this"}),
    )
    .unwrap();
    app.call(
        "comment.add",
        json!({"card_id":c.id,"body":"@codex another question"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.requests.len(), 1);
    assert_eq!(snap.questions.len(), 2);
    assert_eq!(snap.questions[0].request_id, snap.questions[1].request_id);
    assert!(snap.requests.iter().all(|r| r.on_completed.is_none()));
    let run = admit(&app, &snap.requests[0]);
    app.call("execution.report",json!({"execution":{"request_id":run.request_id,"attempt_number":1},"state":"completed","message":"answer","request_key":"answer1"})).unwrap();
    assert_eq!(app.snapshot().unwrap().cards[0].stage, "backlog");
}
#[test]
fn blocked_produces_attention_and_durable_notice() {
    let (_d, app, _p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let run = admit(&app, &app.snapshot().unwrap().requests[0]);
    app.call("execution.report",json!({"execution":{"request_id":run.request_id,"attempt_number":1},"state":"blocked","message":"Need clarification","request_key":"blocked1"})).unwrap();
    let s = app.snapshot().unwrap();
    assert!(s.cards[0].attention.is_some());
    assert_eq!(s.notices.len(), 1);
    assert_eq!(s.cards[0].stage, "develop");
}
#[test]
fn legacy_tags_are_ignored_and_unassigned_stages_use_the_global_default() {
    let (_d, app, mut project, card) = setup();
    project.workflow.stages[1].profile = None;
    let mut old_card = serde_json::to_value(&card).unwrap();
    old_card["tags"] = json!(["claude", "unknown-agent"]);
    let loaded: Card = serde_json::from_value(old_card).unwrap();
    assert!(serde_json::to_value(&loaded).unwrap().get("tags").is_none());
    let mut old_profile = serde_json::to_value(&app.config().profiles[0]).unwrap();
    old_profile["tags"] = json!(["unknown-agent"]);
    let profile: Profile = serde_json::from_value(old_profile).unwrap();
    assert!(
        serde_json::to_value(&profile)
            .unwrap()
            .get("tags")
            .is_none()
    );
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&project)?;
            db.save(&loaded)
        })
        .unwrap();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let snapshot = app.snapshot().unwrap();
    assert!(snapshot.cards[0].human_gate.is_none());
    assert_eq!(snapshot.requests.len(), 1);
    assert_eq!(snapshot.requests[0].profile.id, app.config().default_agent);
}
#[test]
fn archive_compresses_owned_files_and_remains_readable_after_restore() {
    let (_d, app, _p, c) = setup();
    let artifact: Artifact = serde_json::from_value(
        app.call(
            "attachment.add",
            json!({"card_id":c.id,"name":"context.txt","bytes":[104,101,108,108,111]}),
        )
        .unwrap(),
    )
    .unwrap();
    let archived = app
        .call(
            "card.archive",
            json!({"card_id":c.id,"revision":1,"request_key":"archive1"}),
        )
        .unwrap();
    assert!(archived["path"].as_str().unwrap().ends_with(".tar.zst"));
    assert!(!app.home.join(&artifact.path).exists());
    assert_eq!(
        app.call(
            "resource.read",
            json!({"card_id":c.id,"path":artifact.path})
        )
        .unwrap()["body"],
        "hello"
    );
    let c = app.snapshot().unwrap().cards[0].clone();
    app.call(
        "card.restore",
        json!({"card_id":c.id,"revision":c.revision}),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(app.home.join(artifact.path)).unwrap(),
        b"hello"
    );
    assert!(app.snapshot().unwrap().requests.is_empty());
}
#[test]
fn resource_cannot_escape_card_directory() {
    let (_d, app, _p, c) = setup();
    assert!(
        app.call(
            "resource.read",
            json!({"card_id":c.id,"path":"projects/../../config/agents.json"})
        )
        .is_err()
    );
}
#[test]
fn workflow_cycles_are_rejected() {
    let mut flow = Workflow::default();
    flow.stages[2].on_completed = Some("develop".into());
    assert!(canban_workflow_engine::validate(&flow).is_err());
}

#[test]
fn ipc_preserves_large_context_and_report_frames() {
    let (_dir, app, project, _) = setup();
    let body = "large context 中文\n".repeat(12000);
    let request = canban_scheduler::request(
        &project.id,
        &app.snapshot().unwrap().cards[0].id,
        "backlog",
        "comment:test",
        app.config().profiles[0].clone(),
        "Report only".into(),
        None,
    );
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&request))
        .unwrap();
    let mut attempt = admit(&app, &request);
    attempt.terminal = None;
    attempt.core_id = None;
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&attempt))
        .unwrap();
    let running = app.start().unwrap();
    let card = rpc(
        &app.endpoint(),
        "card.create",
        json!({"project_id":project.id,"title":"Large body","body":body}),
    )
    .unwrap();
    assert_eq!(card["body"].as_str().unwrap(), body);
    let snapshot = rpc(&app.endpoint(), "app.snapshot", json!({})).unwrap();
    assert!(
        snapshot["cards"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["body"].as_str() == Some(&body))
    );
    let accepted=rpc(&app.endpoint(),"execution.report",json!({"request_key":"large-report","execution":{"request_id":request.id,"attempt_number":1},"state":"completed","body":body})).unwrap();
    assert_eq!(accepted["accepted"], true);
    let reports = app.snapshot().unwrap().reports;
    assert_eq!(
        std::fs::read_to_string(app.home.join(reports[0].path.as_ref().unwrap())).unwrap(),
        body
    );
    drop(running);
}

#[test]
fn deferred_notes_preserve_legacy_data_without_injecting_it_into_new_executions() {
    let (_dir, app, project, card) = setup();
    let first: Note = serde_json::from_value(
        app.call(
            "note.save",
            json!({"project_id":project.id,"title":"Selected","body":"Original decision"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "note.save",
        json!({"project_id":project.id,"title":"Excluded","body":"Do not capture"}),
    )
    .unwrap();
    app.call(
        "card.context.update",
        json!({"card_id":card.id,"revision":1,"note_ids":[first.id]}),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":2,"stage":"develop"}),
    )
    .unwrap();
    let snapshot = app.snapshot().unwrap();
    let req = &snapshot.requests[0];
    let card = &snapshot.cards[0];
    let path = canban_context::capture(
        &app.home,
        &project,
        card,
        req,
        1,
        &snapshot,
        "/tmp/canban-test.sock",
        Path::new("/bin/echo"),
    )
    .unwrap();
    let manifest = app.home.join(&path).with_file_name("context.json");
    let original = std::fs::read(&manifest).unwrap();
    let value: Value = serde_json::from_slice(&original).unwrap();
    assert!(value["notes"].as_array().unwrap().is_empty());
    let prompt = std::fs::read_to_string(app.home.join(path)).unwrap();
    assert!(!prompt.contains("canban note"));
    assert!(!prompt.contains("Original decision"));
    assert_eq!(
        app.call("note.read", json!({"note_id":first.id})).unwrap()["body"],
        "Original decision"
    );
    app.call("note.save",json!({"project_id":project.id,"note_id":first.id,"revision":1,"title":"Selected","body":"Later revision"})).unwrap();
    assert_eq!(std::fs::read(manifest).unwrap(), original);
}

#[test]
fn legacy_capacity_fields_do_not_survive_config_or_request_loading() {
    let (_dir, app, _project, card) = setup();
    let mut legacy = serde_json::to_value(app.config()).unwrap();
    legacy["capacity"] = json!(1);
    for profile in legacy["profiles"].as_array_mut().unwrap() {
        profile["capacity"] = json!(1);
    }
    std::fs::write(
        app.home.join("config/agents.json"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    app.call("config.reload", json!({})).unwrap();
    let current = serde_json::to_value(app.config()).unwrap();
    assert!(current.get("capacity").is_none());
    assert!(
        current["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p.get("capacity").is_none())
    );
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@codex task"}),
    )
    .unwrap();
    let mut request = serde_json::to_value(&app.snapshot().unwrap().requests[0]).unwrap();
    request["profile"]["capacity"] = json!(1);
    let loaded: Request = serde_json::from_value(request).unwrap();
    assert!(
        serde_json::to_value(loaded).unwrap()["profile"]
            .get("capacity")
            .is_none()
    );
}

#[test]
fn template_profiles_and_mentions_select_independent_roles_and_callbacks() {
    let (_dir, app, _project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let developer = app.snapshot().unwrap().requests[0].clone();
    assert_eq!(developer.profile.id, "developer");
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@reviewer assess the approach"}),
    )
    .unwrap();
    let mention = app
        .snapshot()
        .unwrap()
        .requests
        .into_iter()
        .find(|r| r.trigger.starts_with("comment:"))
        .unwrap();
    assert_eq!(mention.profile.id, "reviewer");
    assert!(mention.on_completed.is_none());
    let run = admit(&app, &developer);
    app.call("execution.report",json!({"execution":{"request_id":run.request_id,"attempt_number":1},"state":"completed","request_key":"roles-completed"})).unwrap();
    let snapshot = app.snapshot().unwrap();
    let review = snapshot
        .requests
        .iter()
        .find(|r| r.trigger == "stage_entry" && r.stage == "review")
        .unwrap();
    assert_eq!(review.profile.id, "reviewer");
    assert_ne!(
        review.profile.role_instruction,
        developer.profile.role_instruction
    );
    assert!(
        serde_json::to_value(&snapshot.cards[0])
            .unwrap()
            .get("tags")
            .is_none()
    );
}

#[test]
fn unknown_stage_profiles_fail_before_project_creation() {
    let (dir, app, _project, _card) = setup();
    let mut workflow = Workflow::default();
    workflow.stages[2].profile = Some("missing-reviewer".into());
    assert!(
        app.call(
            "project.create",
            json!({"name":"Invalid","directory":dir.path().join("invalid"),"workflow":workflow})
        )
        .is_err()
    );
    assert_eq!(app.snapshot().unwrap().projects.len(), 1);
    assert!(!dir.path().join("invalid").exists());
}

#[test]
fn referenced_profile_removal_is_rejected_without_replacing_active_config() {
    let (_dir, app, _project, _card) = setup();
    let mut changed = app.config();
    changed.profiles.retain(|p| p.id != "reviewer");
    std::fs::write(
        app.home.join("config/agents.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    assert!(app.call("config.reload", json!({})).is_err());
    assert!(app.config().profiles.iter().any(|p| p.id == "reviewer"));
}

#[test]
fn queued_role_and_stage_task_are_immutable_after_config_reload() {
    let (_dir, app, project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let before = app.snapshot().unwrap().requests[0].clone();
    let mut config = app.config();
    config
        .profiles
        .iter_mut()
        .find(|p| p.id == "developer")
        .unwrap()
        .role_instruction = "New role".into();
    std::fs::write(
        app.home.join("config/agents.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    app.call("config.reload", json!({})).unwrap();
    assert_eq!(app.snapshot().unwrap().requests[0], before);
    let snapshot = app.snapshot().unwrap();
    let path = canban_context::capture(
        &app.home,
        &project,
        &snapshot.cards[0],
        &before,
        1,
        &snapshot,
        "/tmp/test.sock",
        Path::new("/bin/echo"),
    )
    .unwrap();
    let prompt = std::fs::read_to_string(app.home.join(path)).unwrap();
    assert!(prompt.contains(&before.profile.role_instruction));
    assert!(prompt.contains(&before.instruction));
    assert!(!prompt.contains("New role"));
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@developer explain"}),
    )
    .unwrap();
    // Input appended to the existing invocation retains its loaded profile.
    assert_eq!(app.snapshot().unwrap().requests.len(), 1);
    let run = admit(&app, &before);
    app.call("execution.report",json!({"execution":{"request_id":run.request_id,"attempt_number":1},"state":"completed","request_key":"finish-old-role"})).unwrap();
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@developer next assignment"}),
    )
    .unwrap();
    assert!(
        app.snapshot()
            .unwrap()
            .requests
            .iter()
            .any(|r| r.profile.role_instruction == "New role")
    );
}

#[test]
fn legacy_registry_and_workflow_are_preserved_without_starter_role_injection() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join("config")).unwrap();
    let mut legacy = serde_json::to_value(Config::default()).unwrap();
    legacy["default_agent"] = json!("codex");
    legacy["profiles"]
        .as_array_mut()
        .unwrap()
        .retain(|p| p["id"] == "codex");
    legacy["profiles"][0]
        .as_object_mut()
        .unwrap()
        .remove("role_instruction");
    let original = serde_json::to_vec(&legacy).unwrap();
    std::fs::write(home.join("config/agents.json"), &original).unwrap();
    let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    assert_eq!(
        std::fs::read(home.join("config/agents.json")).unwrap(),
        original
    );
    let flow: Workflow =
        serde_json::from_slice(&std::fs::read(home.join("config/workflows/default.json")).unwrap())
            .unwrap();
    assert!(flow.stages.iter().all(|s| s.profile.is_none()));
    assert_eq!(app.config().profiles.len(), 1);
}

#[test]
fn live_replies_preserve_task_callback_and_require_current_invocation() {
    let (_dir, app, _project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let run = admit(&app, &app.snapshot().unwrap().requests[0]);
    let binding =
        json!({"request_id":run.request_id,"attempt":run.number,"session_id":"exact-thread"});
    app.call("agent.bind", binding.clone()).unwrap();
    app.call("agent.turn_ended", json!({"request_id":run.request_id,"attempt":run.number,"session_id":"exact-thread","body":"Ready"})).unwrap();
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@developer explain progress"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.requests.len(), 1);
    let q = &snap.questions[0];
    let mut reply = json!({"request_key":"answer-once","request_id":run.request_id,"attempt":run.number,"question_id":q.id,"body":"Still working"});
    reply["attempt"] = json!(2);
    assert!(app.call("agent.reply", reply.clone()).is_err());
    reply["attempt"] = json!(run.number);
    app.call("agent.reply", reply.clone()).unwrap();
    app.call("agent.reply", reply).unwrap();
    let snap = app.snapshot().unwrap();
    assert!(snap.reports.is_empty());
    assert_eq!(snap.cards[0].stage, "develop");
    assert_eq!(
        snap.events
            .iter()
            .filter(|e| e.kind == "comment.replied")
            .count(),
        1
    );
    app.call("execution.report",json!({"execution":{"request_id":run.request_id,"attempt_number":run.number},"state":"completed","body":"Done","request_key":"finish-live"})).unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.cards[0].stage, "review");
    assert!(snap.attempts[0].close_state.is_empty());
    assert!(
        snap.attempts[0].turn_ended_at.is_none(),
        "Earlier ready turn must not close a later task"
    );
    let repeated = app
        .call(
            "agent.open",
            json!({"card_id":card.id,"profile_id":"developer"}),
        )
        .unwrap();
    assert_eq!(repeated["request_id"], run.request_id);
    assert_eq!(app.snapshot().unwrap().requests.len(), 2);
    let reviewer = admit(&app, &snap.requests[1]);
    assert!(app.call("agent.bind",json!({"request_id":reviewer.request_id,"attempt":reviewer.number,"session_id":"exact-thread"})).is_err());
    assert!(
        app.call(
            "agent.bind",
            json!({"request_id":run.request_id,"attempt":run.number,"session_id":"another-thread"})
        )
        .is_err()
    );
}

#[test]
fn restarting_does_not_blindly_redeliver_uncertain_question() {
    let (_dir, app, _project, card) = setup();
    app.call(
        "agent.open",
        json!({"card_id":card.id,"profile_id":"developer"}),
    )
    .unwrap();
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@developer question"}),
    )
    .unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut q = db.list::<Question>()?.remove(0);
            q.state = "delivering".into();
            db.save(&q)
        })
        .unwrap();
    // This restart regression must never contact the user's Herdr core.
    app.config.write().unwrap().hc = "/usr/bin/false".into();
    let _running = app.start().unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.questions[0].state, "delivery_unknown");
    assert!(snap.cards[0].attention.is_some());
}

#[test]
fn completion_retains_interactive_terminal_until_idle_timeout_or_explicit_close() {
    let (_dir, app, _project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let run = admit(&app, &app.snapshot().unwrap().requests[0]);
    app.call("execution.report",json!({"execution":{"request_id":run.request_id,"attempt_number":run.number},"state":"completed","message":"Done","request_key":"retained-completion"})).unwrap();
    for state in ["done", "idle", "blocked", "working"] {
        app.observe_lifecycle(
            &run,
            &json!({"state":"live","status":{"state":state},"turn":8,"status_ms":now()}),
        )
        .unwrap();
        let current = app.snapshot().unwrap().attempts[0].clone();
        assert!(current.close_state.is_empty());
        assert!(!current.finished());
        assert_eq!(current.terminal, run.terminal);
    }
    assert_eq!(app.snapshot().unwrap().cards[0].stage, "review");
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"Follow up", "mentions":["developer"]}),
    )
    .unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.questions[0].request_id, run.request_id);
    assert_eq!(
        s.requests
            .iter()
            .filter(|r| r.profile.id == "developer")
            .count(),
        1
    );
}

#[test]
fn legacy_config_migration_keeps_original_and_preserves_conversation_records() {
    let (dir, app, _, _) = setup();
    let mut old = serde_json::to_value(Config::default()).unwrap();
    for p in old["profiles"].as_array_mut().unwrap() {
        p["provider"] = json!("codex");
        p["argv"] = json!(["codex", "exec", "--sandbox", "workspace-write", "{prompt}"]);
        p["resume_argv"] = json!([]);
        p.as_object_mut().unwrap().remove("preset");
    }
    let bytes = serde_json::to_vec_pretty(&old).unwrap();
    let path = app.home.join("config/agents.json");
    std::fs::write(&path, &bytes).unwrap();
    drop(app);
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    assert!(
        !app.config().profiles[0]
            .command()
            .iter()
            .any(|a| a == "exec")
    );
    assert!(!app.config().profiles[0].resume_command().is_empty());
    let backups = std::fs::read_dir(app.home.join("config"))
        .unwrap()
        .filter_map(|e| {
            let p = e.ok()?.path();
            p.file_name()?
                .to_str()?
                .starts_with("agents.legacy-")
                .then_some(p)
        })
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read(&backups[0]).unwrap(), bytes);
    assert_eq!(app.snapshot().unwrap().cards.len(), 1);
}

#[test]
fn review_delivery_receipt_survives_retry_restart_and_archive_is_separate() {
    let (dir, app, mut project, mut card) = setup();
    let source = Path::new(&project.directory);
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(source)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    git(&["init", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Acceptance"),
        ("user.email", "acceptance@example.invalid"),
        ("core.hooksPath", "/dev/null"),
        ("commit.gpgsign", "false"),
    ] {
        git(&["config", k, v]);
    }
    std::fs::write(source.join("seed"), "seed").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "seed"]);
    project.target_branch = Some("main".into());
    project.workflow.workspace.sync_base = false;
    card.stage = "review".into();
    card.worktree = Some(
        canban_workspaces::prepare(&app.home, &project, &card)
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    );
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&project)?;
            db.save(&card)
        })
        .unwrap();
    std::fs::write(
        Path::new(card.worktree.as_ref().unwrap()).join("delivered"),
        "code",
    )
    .unwrap();
    let preview = app
        .call("review.preview", json!({"card_id":card.id}))
        .unwrap();
    let request = json!({"card_id":card.id,"revision":card.revision,"token":preview["token"],"request_key":"accept-once"});
    let accepted = app.call("review.merge", request.clone()).unwrap();
    assert_eq!(accepted["state"], "merged");
    assert_eq!(
        git(&["rev-parse", "HEAD"]),
        accepted["commit"].as_str().unwrap()
    );
    let snap = app.snapshot().unwrap();
    assert!(snap.cards[0].archived_at.is_none());
    assert!(snap.cards[0].delivery.is_some());
    assert_eq!(app.call("review.merge", request.clone()).unwrap(), accepted);
    assert_eq!(
        app.snapshot()
            .unwrap()
            .events
            .iter()
            .filter(|e| e.kind == "review.merged")
            .count(),
        1
    );
    // Simulate a crash after recording an intent: the same request must return
    // unknown without touching Git, even when files/revisions have since changed.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.conn.execute(
                "UPDATE operations SET result=?1 WHERE id='accept-once'",
                [r#"{"state":"unknown"}"#],
            )?;
            Ok(())
        })
        .unwrap();
    let head = git(&["rev-parse", "HEAD"]);
    drop(app);
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    assert_eq!(
        app.call("review.merge", request).unwrap()["state"],
        "unknown"
    );
    assert_eq!(git(&["rev-parse", "HEAD"]), head);
    let c = app.snapshot().unwrap().cards[0].clone();
    app.call(
        "card.archive",
        json!({"card_id":c.id,"revision":c.revision}),
    )
    .unwrap();
    assert_eq!(git(&["rev-parse", "HEAD"]), head);
    let archived = app.snapshot().unwrap().cards[0].clone();
    assert!(archived.archived_at.is_some());
    std::fs::write(source.join("later-project-change"), "later").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "Advance source after archive"]);
    let source_head = git(&["rev-parse", "HEAD"]);
    app.call(
        "card.restore",
        json!({"card_id":archived.id,"revision":archived.revision}),
    )
    .unwrap();
    let restored = app.snapshot().unwrap().cards[0].clone();
    let cwd = canban_workspaces::prepare(&app.home, &project, &restored).unwrap();
    assert!(cwd.join("delivered").exists());
    assert!(!cwd.join("later-project-change").exists());
    assert_eq!(git(&["rev-parse", "HEAD"]), source_head);
}

#[test]
fn core_loss_during_question_delivery_becomes_unknown_without_replay() {
    let (_dir, app, _project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let run = admit(&app, &app.snapshot().unwrap().requests[0]);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut a: Attempt = db.get(&run.id)?;
            a.connection_state = "connected".into();
            a.lifecycle.observed_at = Some(now());
            db.save(&a)
        })
        .unwrap();
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@developer explain progress"}),
    )
    .unwrap();
    let client = canban_herdr::Client {
        config: Config {
            hc: "/usr/bin/false".into(),
            ..Default::default()
        },
    };
    app.deliver_questions(&client).unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.questions[0].state, "delivery_unknown");
    assert!(snap.cards[0].attention.is_some());
    app.deliver_questions(&client).unwrap();
    assert_eq!(app.snapshot().unwrap().notices.len(), snap.notices.len());
    assert_eq!(
        app.snapshot().unwrap().questions[0].state,
        "delivery_unknown"
    );
}

#[test]
fn notes_keep_revisions_links_and_provenance_atomically() {
    let (_d, app, project, card) = setup();
    let create = |title: &str, body: &str| -> Note {
        serde_json::from_value(
            app.call(
                "note.save",
                json!({"project_id":project.id,"title":title,"body":body}),
            )
            .unwrap(),
        )
        .unwrap()
    };
    let b = create("Architecture.md", "Original");
    let a = create(
        "Idea.md",
        &format!("[Architecture](note:{})\n`[ignored](note:missing)`", b.id),
    );
    assert_eq!(a.links, vec![b.id.clone()]);
    let events: Vec<NoteEvent> =
        serde_json::from_value(app.call("note.history", json!({"note_id":b.id})).unwrap()).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.kind == "note.referenced" && e.data["source_note_id"] == a.id)
    );
    let renamed: Note = serde_json::from_value(
        app.call(
            "note.rename",
            json!({"note_id":b.id,"title":"Design.md","revision":b.revision}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(renamed.id, b.id);
    let unchanged: Note =
        serde_json::from_value(app.call("note.get", json!({"note_id":a.id})).unwrap()).unwrap();
    assert_eq!(unchanged.links, vec![b.id.clone()]);
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":card.revision,"stage":"develop"}),
    )
    .unwrap();
    let req = app.snapshot().unwrap().requests[0].clone();
    let run = admit(&app, &req);
    let payload = json!({"note_id":b.id,"revision":renamed.revision,"body":"Updated by Agent","execution":{"request_id":req.id,"attempt_number":run.number},"request_key":"note-agent-once"});
    let saved = app.call("note.write", payload.clone()).unwrap();
    assert_eq!(app.call("note.write", payload).unwrap(), saved);
    let history: Vec<NoteEvent> =
        serde_json::from_value(app.call("note.history", json!({"note_id":b.id})).unwrap()).unwrap();
    let event = history.last().unwrap();
    assert_eq!(event.actor, "@developer");
    assert_eq!(event.card_id, Some(card.id.clone()));
    assert_eq!(event.attempt, Some(run.number));
    assert_eq!(
        history.iter().filter(|e| e.kind == "note.updated").count(),
        1
    );
    let original = app
        .call("note.revision", json!({"note_id":b.id,"revision":1}))
        .unwrap();
    assert_eq!(original["body"], "Original");
    assert!(
        app.snapshot()
            .unwrap()
            .events
            .iter()
            .any(|e| e.card_id == card.id && e.data["note_id"] == b.id)
    );
    let before = app.snapshot().unwrap();
    assert!(
        app.call(
            "note.write",
            json!({"note_id":b.id,"revision":renamed.revision,"body":"Stale overwrite"})
        )
        .is_err()
    );
    assert!(app.call("note.write",json!({"note_id":b.id,"revision":saved["revision"],"body":"Bad invocation","execution":{"request_id":req.id,"attempt_number":run.number+1}})).is_err());
    assert_eq!(before, app.snapshot().unwrap());
}

#[test]
fn note_links_and_card_creation_roll_back_on_invalid_scope() {
    let (d, app, project, _card) = setup();
    let note: Note = serde_json::from_value(
        app.call(
            "note.save",
            json!({"project_id":project.id,"title":"Idea","body":"Plan"}),
        )
        .unwrap(),
    )
    .unwrap();
    let other: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name":"Other","directory":d.path().join("other")}),
        )
        .unwrap(),
    )
    .unwrap();
    let foreign: Note = serde_json::from_value(
        app.call(
            "note.save",
            json!({"project_id":other.id,"title":"Foreign","body":"Keep"}),
        )
        .unwrap(),
    )
    .unwrap();
    let before = app.snapshot().unwrap();
    assert!(app.call("note.write",json!({"note_id":note.id,"revision":note.revision,"body":format!("[x](note:{})",foreign.id)})).is_err());
    assert!(
        app.call(
            "card.create",
            json!({"project_id":other.id,"title":"Invalid","source_note_id":note.id})
        )
        .is_err()
    );
    assert_eq!(before, app.snapshot().unwrap());
    let card:Card=serde_json::from_value(app.call("card.create",json!({"project_id":project.id,"title":"Scoped work","body":"Implement scope","source_note_id":note.id})).unwrap()).unwrap();
    assert_eq!(card.stage, "backlog");
    assert_eq!(card.context_notes, Some(vec![note.id.clone()]));
    assert!(app.snapshot().unwrap().requests.is_empty());
    let saved: Note =
        serde_json::from_value(app.call("note.get", json!({"note_id":note.id})).unwrap()).unwrap();
    assert_eq!(saved.card_ids, vec![card.id.clone()]);
    app.call(
        "card.context.update",
        json!({"card_id":card.id,"revision":card.revision,"note_ids":[]}),
    )
    .unwrap();
    let saved: Note =
        serde_json::from_value(app.call("note.get", json!({"note_id":note.id})).unwrap()).unwrap();
    assert!(saved.card_ids.is_empty());
    let events: Vec<NoteEvent> = serde_json::from_value(
        app.call("note.history", json!({"note_id":note.id}))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(events.last().unwrap().kind, "note.card_unlinked");
}

#[test]
fn upgrading_existing_notes_retains_current_revision_without_inventing_history() {
    let (d, app, project, _) = setup();
    let n: Note = serde_json::from_value(
        app.call(
            "note.save",
            json!({"project_id":project.id,"title":"Existing","body":"Keep me"}),
        )
        .unwrap(),
    )
    .unwrap();
    let db = rusqlite::Connection::open(d.path().join("home/canban.sqlite")).unwrap();
    db.execute_batch("DROP TABLE note_events; DROP TABLE note_revisions; DROP TABLE terminals; DROP TABLE github_issues; DROP INDEX github_card_source; PRAGMA user_version=2;")
        .unwrap();
    drop(db);
    drop(app);
    let store = Store::open(&d.path().join("home")).unwrap();
    let saved: Note = store.db().get(&n.id).unwrap();
    assert_eq!(saved.body, "Keep me");
    let rev: NoteRevision = store.db().get(&format!("{}:1", n.id)).unwrap();
    assert_eq!(rev.body, saved.body);
    assert!(store.db().list::<NoteEvent>().unwrap().is_empty());
}

#[test]
fn global_slots_include_closing_and_unknown_agents_and_resume_after_exit() {
    let (_dir, app, project, card) = setup();
    assert_eq!(app.config().scheduler.max_agents, 8);
    for i in 0..9 {
        let c: Card = serde_json::from_value(
            app.call(
                "card.create",
                json!({"project_id":project.id,"title":format!("Task {i}")}),
            )
            .unwrap(),
        )
        .unwrap();
        app.call(
            "card.move",
            json!({"card_id":c.id,"revision":1,"stage":"develop"}),
        )
        .unwrap();
        if i < 8 {
            let req = app.snapshot().unwrap().requests.last().unwrap().clone();
            admit(&app, &req);
        }
    }
    let mut snapshot = app.snapshot().unwrap();
    assert_eq!(canban_scheduler::active_count(&snapshot), 8);
    assert!(canban_scheduler::next(&snapshot, 8).is_none());
    snapshot.attempts[0].state = "completed".into();
    snapshot.attempts[0].close_state = "closing".into();
    assert!(canban_scheduler::next(&snapshot, 8).is_none());
    snapshot.attempts[0].observed = Some(json!({"state":"finished"}));
    assert!(canban_scheduler::next(&snapshot, 8).is_some());
    snapshot.attempts[0].observed = None;
    snapshot.attempts[0].terminal = None;
    snapshot.attempts[0].state = "unknown".into();
    assert!(canban_scheduler::next(&snapshot, 8).is_none());
    let path = app.home.join("config/agents.json");
    let before: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    app.call("scheduler.configure", json!({"max_agents":2}))
        .unwrap();
    assert_eq!(app.config().scheduler.max_agents, 2);
    assert_eq!(app.snapshot().unwrap().attempts.len(), 8);
    assert!(
        app.call("scheduler.configure", json!({"max_agents":0}))
            .is_err()
    );
    let after: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(before["profiles"], after["profiles"]);
    drop(app);
    let reopened = AppServer::open(_dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    assert_eq!(reopened.config().scheduler.max_agents, 2);
    assert_eq!(reopened.snapshot().unwrap().cards[0].id, card.id);
}

#[test]
fn later_template_stages_win_across_boards_and_human_cards_are_skipped() {
    let (_dir, app, mut project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    project.id = "second-board".into();
    project.workflow.stages[2].id = "custom-final-check".into();
    project.workflow.stages[1].on_completed = Some("custom-final-check".into());
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&project))
        .unwrap();
    let second: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id":project.id,"title":"Later stage"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id":second.id,"revision":1,"stage":"custom-final-check"}),
    )
    .unwrap();
    let mut snapshot = app.snapshot().unwrap();
    assert_eq!(
        canban_scheduler::next(&snapshot, 8).unwrap().card_id,
        second.id
    );
    snapshot
        .cards
        .iter_mut()
        .find(|c| c.id == second.id)
        .unwrap()
        .human_gate = Some(HumanGate {
        reason: "Pause".into(),
        request_id: None,
        next_stage: None,
    });
    assert_eq!(
        canban_scheduler::next(&snapshot, 8).unwrap().card_id,
        card.id
    );
    let mut manual = snapshot.requests[1].clone();
    manual.id = "manual".into();
    manual.trigger = "interactive".into();
    snapshot.requests.push(manual);
    assert_eq!(canban_scheduler::next(&snapshot, 8).unwrap().id, "manual");
    snapshot.requests.pop();
    snapshot.requests[1].stage = "develop".into();
    snapshot
        .cards
        .iter_mut()
        .find(|c| c.id == second.id)
        .unwrap()
        .stage = "develop".into();
    snapshot
        .cards
        .iter_mut()
        .find(|c| c.id == second.id)
        .unwrap()
        .human_gate = None;
    snapshot.requests[0].created_at = 1;
    snapshot.requests[1].created_at = 1;
    assert_eq!(
        canban_scheduler::next(&snapshot, 8).unwrap().card_id,
        card.id
    );
}

#[test]
fn human_review_gate_survives_reports_acknowledgement_restart_and_requires_explicit_approval() {
    let (dir, app, mut project, card) = setup();
    project.workflow.stages[1].require_human_review = true;
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&project))
        .unwrap();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().requests[0].clone();
    let run = admit(&app, &request);
    let report = json!({"execution":{"request_id":request.id,"attempt_number":run.number},"state":"completed","request_key":"human-gate-complete"});
    app.call("execution.report", report.clone()).unwrap();
    app.call("execution.report", report).unwrap();
    let snapshot = app.snapshot().unwrap();
    assert_eq!(snapshot.cards[0].stage, "develop");
    assert_eq!(
        snapshot.cards[0]
            .human_gate
            .as_ref()
            .unwrap()
            .next_stage
            .as_deref(),
        Some("review")
    );
    assert_eq!(snapshot.requests.len(), 1);
    app.call("card.attention.clear", json!({"card_id":card.id}))
        .unwrap();
    assert!(app.snapshot().unwrap().cards[0].human_gate.is_some());
    // Explicit manual work can run, but its completion cannot approve this gate.
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@reviewer help me inspect"}),
    )
    .unwrap();
    let manual = app.snapshot().unwrap().requests.last().unwrap().clone();
    let manual_run = admit(&app, &manual);
    app.call("execution.report",json!({"execution":{"request_id":manual.id,"attempt_number":manual_run.number},"state":"completed","request_key":"human-gate-manual"})).unwrap();
    assert!(app.snapshot().unwrap().cards[0].human_gate.is_some());
    drop(app);
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    let current = app.snapshot().unwrap().cards[0].clone();
    assert!(current.human_gate.is_some());
    let approve =
        json!({"card_id":card.id,"revision":current.revision,"request_key":"approve-gate"});
    assert!(app.call("card.continue",json!({"card_id":card.id,"revision":current.revision,"execution":{"request_id":manual.id,"attempt_number":1}})).is_err());
    assert!(app.call("card.move",json!({"card_id":card.id,"revision":current.revision,"stage":"review","execution":{"request_id":manual.id,"attempt_number":1}})).is_err());
    app.call("card.continue", approve.clone()).unwrap();
    app.call("card.continue", approve).unwrap();
    let after = app.snapshot().unwrap();
    assert_eq!(after.cards[0].stage, "review");
    assert!(after.cards[0].human_gate.is_none());
    assert_eq!(
        after
            .requests
            .iter()
            .filter(|r| r.trigger == "stage_entry" && r.stage == "review")
            .count(),
        1
    );
}

#[test]
fn blocked_completion_does_not_advance_until_human_continues() {
    let (_dir, app, _project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().requests[0].clone();
    let run = admit(&app, &request);
    for state in ["blocked", "completed"] {
        app.call("execution.report",json!({"execution":{"request_id":request.id,"attempt_number":run.number},"state":state,"request_key":format!("blocked-{state}")})).unwrap();
    }
    let current = app.snapshot().unwrap().cards[0].clone();
    assert_eq!(current.stage, "develop");
    assert!(current.human_gate.is_some());
    app.call(
        "card.continue",
        json!({"card_id":card.id,"revision":current.revision}),
    )
    .unwrap();
    assert_eq!(app.snapshot().unwrap().cards[0].stage, "review");
}

#[test]
fn recovery_gate_retains_completion_callback_until_human_approval() {
    let (_dir, app, _project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().requests[0].clone();
    let run = admit(&app, &request);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut c: Card = db.get(&card.id)?;
            attention(db, &mut c, "Connection needs inspection")?;
            db.save(&c)
        })
        .unwrap();
    app.call("execution.report",json!({"execution":{"request_id":request.id,"attempt_number":run.number},"state":"completed","request_key":"recovery-completed"})).unwrap();
    let current = app.snapshot().unwrap().cards[0].clone();
    assert_eq!(current.stage, "develop");
    assert_eq!(
        current.human_gate.as_ref().unwrap().next_stage.as_deref(),
        Some("review")
    );
    app.call(
        "card.continue",
        json!({"card_id":card.id,"revision":current.revision}),
    )
    .unwrap();
    assert_eq!(app.snapshot().unwrap().cards[0].stage, "review");
}

#[test]
fn removed_routing_fields_are_rejected_without_mutating_cards() {
    let (_dir, app, project, card) = setup();
    for tags in [json!([]), json!(["claude"])] {
        let error = app
            .call(
                "card.create",
                json!({"project_id":project.id,"title":"Old client","tags":tags}),
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Routing tags are no longer supported")
        );
        let error = app
            .call(
                "card.update",
                json!({"card_id":card.id,"revision":1,"title":"Changed","tags":tags}),
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Routing tags are no longer supported")
        );
    }
    let snapshot = app.snapshot().unwrap();
    assert_eq!(snapshot.cards.len(), 1);
    assert_eq!(snapshot.cards[0], card);
    assert!(snapshot.requests.is_empty());
}

#[path = "tests/lifecycle.rs"]
mod lifecycle_cases;
#[path = "tests/terminals.rs"]
mod terminal_cases;

mod machines_cases;

#[test]
fn explicit_comment_recipients_override_prose_scanning() {
    let (_d, app, _p, c) = setup();
    app.call(
        "comment.add",
        json!({"card_id":c.id,"body":"The UI shows Ask @claude", "mentions":[]}),
    )
    .unwrap();
    assert!(app.snapshot().unwrap().requests.is_empty());
    app.call(
        "comment.add",
        json!({"card_id":c.id,"body":"Compare @claude and @codex", "mentions":["codex"]}),
    )
    .unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.requests.len(), 1);
    assert_eq!(s.requests[0].profile.id, "codex");
    assert_eq!(s.questions.len(), 1);
    assert_eq!(
        s.events
            .iter()
            .filter(|e| e.kind == "comment.created")
            .count(),
        2
    );
}

#[test]
fn local_card_removal_is_reversible_cancels_queue_and_retains_history() {
    let (_d, app, _p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":c.revision,"stage":"develop"}),
    )
    .unwrap();
    let current = app.snapshot().unwrap().cards[0].clone();
    app.call(
        "card.remove",
        json!({"card_id":c.id,"revision":current.revision}),
    )
    .unwrap();
    let s = app.snapshot().unwrap();
    assert!(s.cards.is_empty());
    assert!(s.requests.iter().all(|r| r.state == "cancelled"));
    assert!(s.events.iter().any(|e| e.kind == "card.removed"));
    let removed: Card =
        serde_json::from_value(app.call("card.get", json!({"card_id":c.id})).unwrap()).unwrap();
    assert!(removed.removed_at.is_some());
    let listed = app
        .call("card.list", json!({"include_removed":true}))
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    app.call(
        "card.restore",
        json!({"card_id":c.id,"revision":removed.revision}),
    )
    .unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.cards[0].id, c.id);
    assert_eq!(s.cards[0].number, c.number);
    assert!(s.cards[0].active());
    assert_eq!(s.cards[0].body, c.body);
    assert_eq!(s.requests.len(), 1);
    assert_eq!(s.requests[0].state, "cancelled");
}
#[test]
fn local_card_removal_cannot_orphan_a_live_agent() {
    let (_d, app, _p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":c.revision,"stage":"develop"}),
    )
    .unwrap();
    let run = admit(&app, &app.snapshot().unwrap().requests[0]);
    let current = app.snapshot().unwrap().cards[0].clone();
    let error = app
        .call(
            "card.remove",
            json!({"card_id":c.id,"revision":current.revision}),
        )
        .unwrap_err();
    assert!(error.to_string().contains("End active Card Agents"));
    let s = app.snapshot().unwrap();
    assert!(s.cards[0].active());
    assert_eq!(s.attempts[0].id, run.id);
    assert!(!s.events.iter().any(|e| e.kind == "card.removed"));
}

#[test]
fn continued_assignment_rejects_new_stale_reports_but_preserves_completion_receipts() {
    let (_d, app, _p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let run = admit(&app, &app.snapshot().unwrap().requests[0]);
    let report = json!({"request_key":"completed-before-continuation","execution":{"request_id":run.request_id,"attempt_number":run.number},"state":"completed","message":"Done"});
    app.call("execution.report", report.clone()).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut a: Attempt = db.get(&run.id)?;
            a.state = "continued".into();
            db.save(&a)
        })
        .unwrap();
    assert_eq!(
        app.call("execution.report", report).unwrap()["duplicate"],
        true
    );
    let before = app.snapshot().unwrap();
    let result=app.call("execution.report",json!({"request_key":"obsolete-progress","execution":{"request_id":run.request_id,"attempt_number":run.number},"state":"working","body":"Stale"}));
    assert!(result.is_err());
    let after = app.snapshot().unwrap();
    assert_eq!(before.reports, after.reports);
    assert_eq!(after.attempts[0].state, "continued");
}

#[path = "tests/board.rs"]
mod board_cases;
