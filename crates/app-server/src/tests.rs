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
fn unmatched_tags_never_fall_back_to_unrelated_agent() {
    let (_d, app, mut project, c) = setup();
    // Legacy stages without a profile reference retain tag/default routing.
    project.workflow.stages[1].profile = None;
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&project))
        .unwrap();
    app.call(
        "card.update",
        json!({"card_id":c.id,"revision":1,"tags":["unknown-agent"]}),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":2,"stage":"develop"}),
    )
    .unwrap();
    let s = app.snapshot().unwrap();
    assert!(s.requests.is_empty());
    assert!(s.cards[0].attention.is_some());
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
fn selected_note_context_is_captured_without_following_later_edits() {
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
    let manifest = app.home.join(path).with_file_name("context.json");
    let original = std::fs::read(&manifest).unwrap();
    let value: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(value["notes"].as_array().unwrap().len(), 1);
    assert_eq!(value["notes"][0]["body"], "Original decision");
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
        "card.update",
        json!({"card_id":card.id,"revision":1,"tags":["claude"]}),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":2,"stage":"develop"}),
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
    assert_eq!(snapshot.cards[0].tags, ["claude"]);
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
    assert_eq!(snap.attempts[0].close_state, "pending");
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
