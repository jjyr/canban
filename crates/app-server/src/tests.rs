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
fn admit(app: &AppServer, queued: &TerminalEntry) -> TerminalEntry {
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&queued.id)?;
            let run = entry.agent.as_mut().unwrap();
            if run.pending.is_some() {
                run.promote_pending();
                run.number += 1;
            }
            if run.number == 0 {
                run.number = 1;
            }
            run.state = "running".into();
            run.core_id = Some("core-test".into());
            run.terminal = Some("t1".into());
            run.cwd = "/tmp".into();
            db.save(&entry)?;
            Ok(entry)
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
    assert_eq!(snap.terminals.len(), 1);
    assert_eq!(snap.cards[0].stage, "develop");
    let _run = admit(&app, &snap.terminals[0]);
    let report = json!({"request_key":"finish-once","project_id":p.id,"card":c.number,"state":"completed","message":"done","body":"# Full report\nVerified change"});
    app.call("execution.report", report.clone()).unwrap();
    app.call("execution.report", report).unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.cards[0].stage, "review");
    assert_eq!(snap.terminals.len(), 1);
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
    assert!(s.terminals.is_empty());
    assert_eq!(s.cards[0].stage, "backlog");
}
#[test]
fn mentions_coalesce_in_the_card_profile_queue_without_stage_callback() {
    let (_d, app, p, c) = setup();
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
    assert_eq!(snap.terminals.len(), 1);
    assert_eq!(snap.questions.len(), 2);
    assert_eq!(snap.questions[0].terminal_id, snap.questions[1].terminal_id);
    assert!(
        snap.terminals
            .iter()
            .all(|r| r.agent.as_ref().unwrap().on_completed.is_none())
    );
    let _run = admit(&app, &snap.terminals[0]);
    app.call("execution.report",json!({"project_id":p.id,"card":c.number,"profile_id":"codex","state":"completed","message":"answer","request_key":"answer1"})).unwrap();
    assert_eq!(app.snapshot().unwrap().cards[0].stage, "backlog");
}
#[test]
fn blocked_produces_attention_and_durable_notice() {
    let (_d, app, p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let _run = admit(&app, &app.snapshot().unwrap().terminals[0]);
    app.call("execution.report",json!({"project_id":p.id,"card":c.number,"state":"blocked","message":"Need clarification","request_key":"blocked1"})).unwrap();
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
    assert_eq!(snapshot.terminals.len(), 1);
    assert_eq!(
        snapshot.terminals[0].agent.as_ref().unwrap().profile.id,
        app.config().default_agent
    );
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
    assert!(app.snapshot().unwrap().terminals.is_empty());
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
    let (_dir, app, project, initial_card) = setup();
    let body = "large context 中文\n".repeat(12000);
    let profile = app.config().profiles[0].clone();
    let run = canban_scheduler::agent_run(
        &project.id,
        &initial_card.id,
        1,
        "backlog",
        "comment:test",
        profile.clone(),
        "Report only".into(),
        None,
    );
    let cid = Conversation::key(&initial_card.id, &profile.id);
    let queued = TerminalEntry {
        id: cid.clone(),
        conversation_id: Some(cid),
        number: 1,
        name: None,
        manual_number: None,
        origin_card_id: Some(initial_card.id.clone()),
        agent_engine: None,
        runtime: None,
        created_at: run.updated_at,
        agent: Some(run),
    };
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&queued))
        .unwrap();
    let mut admitted = admit(&app, &queued);
    if let Some(a) = admitted.agent.as_mut() {
        a.terminal = None;
        a.core_id = None;
    }
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&admitted))
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
    let accepted=rpc(&app.endpoint(),"execution.report",json!({"request_key":"large-report","project_id":project.id,"card":initial_card.number,"state":"completed","body":body})).unwrap();
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
    let req = &snapshot.terminals[0];
    let card = &snapshot.cards[0];
    let path = canban_context::capture(
        &app.home,
        &project,
        card,
        req,
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
    let mut request =
        serde_json::to_value(app.snapshot().unwrap().terminals[0].agent.as_ref().unwrap()).unwrap();
    request["profile"]["capacity"] = json!(1);
    let loaded: AgentRun = serde_json::from_value(request).unwrap();
    assert!(
        serde_json::to_value(loaded).unwrap()["profile"]
            .get("capacity")
            .is_none()
    );
}

#[test]
fn template_profiles_and_mentions_select_independent_roles_and_callbacks() {
    let (_dir, app, project, card) = setup();
    apply_project_yaml(
        &app,
        &project.id,
        &develop_and_review_yaml(&project.id, false, true, true),
    );
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let developer = app.snapshot().unwrap().terminals[0].clone();
    assert_eq!(developer.agent.as_ref().unwrap().profile.id, "developer");
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@reviewer assess the approach"}),
    )
    .unwrap();
    let mention = app
        .snapshot()
        .unwrap()
        .terminals
        .into_iter()
        .find(|r| r.agent.as_ref().is_some_and(|a| a.trigger.is_comment()))
        .unwrap();
    assert_eq!(mention.agent.as_ref().unwrap().profile.id, "reviewer");
    assert!(mention.agent.as_ref().unwrap().on_completed.is_none());
    let _run = admit(&app, &developer);
    app.call("execution.report",json!({"project_id":project.id,"card":card.number,"profile_id":"developer","state":"completed","request_key":"roles-completed"})).unwrap();
    let snapshot = app.snapshot().unwrap();
    let review_entry = snapshot
        .terminals
        .iter()
        .find(|r| r.agent.as_ref().is_some_and(|a| a.profile.id == "reviewer"))
        .unwrap();
    let review_agent = review_entry.agent.as_ref().unwrap();
    // Either `review_agent` itself (if promoted) or `review_agent.pending` holds the `review` StageEntry task!
    let (review_profile, review_stage, review_trigger) = if let Some(p) = &review_agent.pending {
        (&p.profile, p.stage.as_str(), &p.trigger)
    } else {
        (
            &review_agent.profile,
            review_agent.stage.as_str(),
            &review_agent.trigger,
        )
    };
    assert_eq!(*review_trigger, ExecutionTrigger::StageEntry);
    assert_eq!(review_stage, "review");
    assert_eq!(review_profile.id, "reviewer");
    assert_ne!(
        review_profile.role_instruction,
        developer.agent.as_ref().unwrap().profile.role_instruction
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
    let invalid_yaml = r#"schema: 1
id: delivery:invalid
name: Invalid
on:
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
steps:
  - id: agent:develop
    uses: agent
    with:
      profile: missing-reviewer
      instruction: Implement
    next: __end
  - id: __end
    uses: complete
"#;
    assert!(
        app.call(
            "project.create",
            json!({"name":"Invalid","directory":dir.path().join("invalid"),"workflow_yaml":invalid_yaml})
        )
        .is_err()
    );
    assert_eq!(app.snapshot().unwrap().projects.len(), 1);
    assert!(!dir.path().join("invalid").exists());
}

#[test]
fn referenced_profile_removal_is_rejected_without_replacing_active_config() {
    let (_dir, app, project, _card) = setup();
    apply_project_yaml(
        &app,
        &project.id,
        &develop_and_review_yaml(&project.id, false, true, true),
    );
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
    let before = app.snapshot().unwrap().terminals[0].clone();
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
    assert_eq!(app.snapshot().unwrap().terminals[0], before);
    let snapshot = app.snapshot().unwrap();
    let path = canban_context::capture(
        &app.home,
        &project,
        &snapshot.cards[0],
        &before,
        &snapshot,
        "/tmp/test.sock",
        Path::new("/bin/echo"),
    )
    .unwrap();
    let prompt = std::fs::read_to_string(app.home.join(path)).unwrap();
    let before_agent = before.agent.as_ref().unwrap();
    assert!(prompt.contains(&before_agent.profile.role_instruction));
    assert!(prompt.contains(&before_agent.instruction));
    assert!(!prompt.contains("New role"));
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@developer explain"}),
    )
    .unwrap();
    // Input appended to the existing invocation retains its loaded profile.
    assert_eq!(app.snapshot().unwrap().terminals.len(), 1);
    let _run = admit(&app, &before);
    app.call("execution.report",json!({"project_id":project.id,"card":card.number,"profile_id":"developer","state":"completed","request_key":"finish-old-role"})).unwrap();
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@developer next assignment"}),
    )
    .unwrap();
    assert!(
        app.snapshot()
            .unwrap()
            .terminals
            .iter()
            .any(|r| r.agent.as_ref().is_some_and(|a| {
                a.profile.role_instruction == "New role"
                    || a.pending
                        .as_ref()
                        .is_some_and(|p| p.profile.role_instruction == "New role")
            }))
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
    assert!(!home.join("config/workflows").exists());
    assert_eq!(app.config().profiles.len(), 1);
    let created: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name":"SingleProfile","directory":dir.path().join("single")}),
        )
        .unwrap(),
    )
    .unwrap();
    let sources = app
        .call("workflow.sources", json!({"project_id":created.id}))
        .unwrap();
    assert!(
        !sources["sources"][0]["source"]
            .as_str()
            .unwrap()
            .contains("profile: developer")
    );
}

#[test]
fn live_replies_preserve_task_callback_and_require_current_invocation() {
    let (_dir, app, project, card) = setup();
    apply_project_yaml(
        &app,
        &project.id,
        &develop_and_review_yaml(&project.id, false, true, true),
    );
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let run = admit(&app, &app.snapshot().unwrap().terminals[0]);
    let binding = json!({"project_id":project.id,"card":card.number,"profile_id":"developer","session_id":"exact-thread"});
    app.call("agent.bind", binding).unwrap();
    app.call("agent.turn_ended", json!({"project_id":project.id,"card":card.number,"profile_id":"developer","session_id":"exact-thread","body":"Ready"})).unwrap();
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@developer explain progress"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.terminals.len(), 1);
    let q = &snap.questions[0];
    let mut reply = json!({"request_key":"answer-once","project_id":project.id,"card":card.number,"profile_id":"developer","question_id":q.id,"body":"Still working"});
    reply["question_id"] = json!("missing-question");
    assert!(app.call("agent.reply", reply.clone()).is_err());
    reply["question_id"] = json!(q.id);
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
    app.call("execution.report",json!({"project_id":project.id,"card":card.number,"profile_id":"developer","state":"completed","body":"Done","request_key":"finish-live"})).unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.cards[0].stage, "review");
    let dev_agent = snap.terminals[0].agent.as_ref().unwrap();
    assert!(dev_agent.close_state.is_empty());
    assert!(
        dev_agent.turn_ended_at.is_none(),
        "Earlier ready turn must not close a later task"
    );
    let repeated = app
        .call(
            "agent.open",
            json!({"card_id":card.id,"profile_id":"developer"}),
        )
        .unwrap();
    assert_eq!(repeated["terminal_id"], run.id);
    assert_eq!(app.snapshot().unwrap().terminals.len(), 2);
    let reviewer_entry = snap
        .terminals
        .iter()
        .find(|t| t.agent.as_ref().is_some_and(|a| a.profile.id == "reviewer"))
        .unwrap();
    let _reviewer = admit(&app, reviewer_entry);
    assert!(app.call("agent.bind",json!({"project_id":project.id,"card":card.number,"profile_id":"reviewer","session_id":"exact-thread"})).is_err());
    assert!(
        app.call(
            "agent.bind",
            json!({"project_id":project.id,"card":card.number,"profile_id":"developer","session_id":"another-thread"})
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
    let (_dir, app, project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let run = admit(&app, &app.snapshot().unwrap().terminals[0]);
    app.call("execution.report",json!({"project_id":project.id,"card":card.number,"state":"completed","message":"Done","request_key":"retained-completion"})).unwrap();
    for state in ["done", "idle", "blocked", "working"] {
        app.observe_lifecycle(
            &run,
            &json!({"state":"live","status":{"state":state},"turn":8,"status_ms":now()}),
        )
        .unwrap();
        let current = app.snapshot().unwrap().terminals[0].clone();
        let current_agent = current.agent.as_ref().unwrap();
        assert!(current_agent.close_state.is_empty());
        assert!(!current_agent.finished());
        assert_eq!(current_agent.terminal, run.agent.as_ref().unwrap().terminal);
    }
    assert_eq!(app.snapshot().unwrap().cards[0].stage, "review");
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"Follow up", "mentions":["developer"]}),
    )
    .unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.questions[0].terminal_id.as_deref(), Some(run.id.as_str()));
    assert_eq!(
        s.terminals
            .iter()
            .filter(|r| r
                .agent
                .as_ref()
                .is_some_and(|a| a.profile.id == "developer"))
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
    // Mirror launch's persisted binding, including an allocated suffix.
    card.branch = canban_workspaces::Ops::local().current_branch(card.worktree.as_ref().unwrap());
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
    // A retained terminal can serve several assignments. Its old Develop
    // observation stays live after Review exits, but no longer owns runtime.
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@codex review"}),
    )
    .unwrap();
    let run = admit(&app, &app.snapshot().unwrap().terminals[0]);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut current = run.clone();
            let agent = current.agent.as_mut().unwrap();
            agent.number = 2;
            agent.state = "completed".into();
            agent.observed = Some(json!({"state":"finished","exit_code":0}));
            db.save(&current)
        })
        .unwrap();
    card = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.id == card.id)
        .unwrap();
    let preview = app
        .call("review.preview", json!({"card_id":card.id}))
        .unwrap();
    assert_eq!(preview["merge_allowed"], true, "{preview}");
    let request = json!({"card_id":card.id,"revision":card.revision,"token":preview["token"],"request_key":"accept-once"});
    let accepted = app.call("review.merge", request.clone()).unwrap();
    assert_eq!(accepted["state"], "merged");
    assert_eq!(
        git(&["rev-parse", "HEAD"]),
        accepted["commit"].as_str().unwrap()
    );
    let snap = app.snapshot().unwrap();
    // Merged Cards remain visible on the Board on the day of delivery and archive the next day.
    assert!(snap.cards[0].archived_at.is_none());
    assert!(snap.cards[0].archive.is_none());
    assert!(snap.cards[0].delivery.is_some());
    assert!(snap.cards[0].merged());
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
    // Same-day auto-archive stays deferred; next-day sweep archives cleanly.
    assert!(!app.try_auto_archive_merged(&card.id).unwrap());
    let next_day = now() + 36 * 60 * 60 * 1000;
    assert!(app.try_auto_archive_merged_at(&card.id, next_day).unwrap());
    let archived = app.snapshot().unwrap().cards[0].clone();
    assert!(archived.archived_at.is_some());
    assert!(archived.archive.is_some());
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
fn remote_pr_merge_defers_archive_while_agent_live_then_archives_and_handles_pr_source_cards() {
    let (_dir, app, project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let req = app.snapshot().unwrap().terminals[0].clone();
    let run = admit(&app, &req);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut c: Card = db.get(&card.id)?;
            c.stage = "review".into();
            c.pull_request = Some(GitHubPr {
                repository_id: 1,
                repository: "example/repo".into(),
                number: 7,
                url: "https://github.com/example/repo/pull/7".into(),
                head_branch: "canban/1".into(),
                base_branch: "main".into(),
                head: "abc1234".into(),
                state: "open".into(),
                merged: false,
                checks: "success".into(),
                review_summary: "approved".into(),
                workspace: WorkspaceBinding {
                    location: project.work_location(),
                    path: run.agent.as_ref().unwrap().cwd.clone(),
                    branch: Some("canban/1".into()),
                    base_commit: None,
                    target_branch: Some("main".into()),
                },
            });
            db.save(&c)?;
            let mut entry: TerminalEntry = db.get(&run.id)?;
            entry.agent.as_mut().unwrap().observed =
                Some(json!({"state":"live","status":{"state":"working"}}));
            db.save(&entry)
        })
        .unwrap();
    app.reconcile_remote_pr_merge(
        "example/repo",
        7,
        &json!({"merged":true,"merge_commit_sha":"deadbeef","base":{"ref":"main","repo":{"id":1}}}),
    )
    .unwrap();
    let after_remote_merge = app.snapshot().unwrap().cards[0].clone();
    assert!(after_remote_merge.delivery.is_some());
    assert!(after_remote_merge.pull_request.as_ref().unwrap().merged);
    let next_day = now() + 36 * 60 * 60 * 1000;
    // Active terminal prevents archive even on the next day so local work is not interrupted.
    assert!(!app.try_auto_archive_merged_at(&card.id, next_day).unwrap());
    assert!(after_remote_merge.archived_at.is_none());

    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&run.id)?;
            let agent = entry.agent.as_mut().unwrap();
            agent.state = "completed".into();
            agent.observed = Some(json!({"state":"finished","exit_code":0}));
            db.save(&entry)
        })
        .unwrap();
    // Same-day remains visible; next-day archives once the Agent has finished.
    assert!(!app.try_auto_archive_merged(&card.id).unwrap());
    assert!(app.try_auto_archive_merged_at(&card.id, next_day).unwrap());
    assert!(app.snapshot().unwrap().cards[0].archived_at.is_some());

    // A PR source card observed merged on GitHub records delivery today and archives the next day.
    let pr_card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id":project.id,"title":"PR source"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut c: Card = db.get(&pr_card.id)?;
            c.content = CardContent::GitHubPr {
                source: GitHubIssueRef {
                    repository: "example/repo".into(),
                    repository_id: 1,
                    issue_id: 99,
                    number: 12,
                    url: "https://github.com/example/repo/pull/12".into(),
                },
            };
            db.save(&c)
        })
        .unwrap();
    app.observe_github_response(
        "/repos/example/repo/pulls/12",
        &json!({"id":99,"number":12,"state":"closed","merged":true,"merge_commit_sha":"feedface","base":{"ref":"main","repo":{"id":1}},"updated_at":"2026-11-11T10:00:00Z","user":{"id":1}}),
    )
    .unwrap();
    let pr_updated = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.id == pr_card.id)
        .unwrap();
    assert_eq!(pr_updated.delivery.as_ref().unwrap().commit, "feedface");
    assert!(pr_updated.archived_at.is_none());
    assert!(
        app.try_auto_archive_merged_at(&pr_card.id, next_day)
            .unwrap()
    );
    let pr_archived = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.id == pr_card.id)
        .unwrap();
    assert!(pr_archived.archived_at.is_some());
}

#[test]
fn review_clears_warning_when_herdr_reports_agent_stopped_and_never_blocks_merge() {
    let (dir, app, mut project, mut card) = setup();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&source)
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
    card.branch = canban_workspaces::Ops::local().current_branch(card.worktree.as_ref().unwrap());
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

    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@codex review"}),
    )
    .unwrap();
    let run = admit(&app, &app.snapshot().unwrap().terminals[0]);
    let current_id = run.id.clone();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut current = run.clone();
            {
                let a = current.agent.as_mut().unwrap();
                a.number = 2;
                a.state = "completed".into();
                a.connection_state = "connected".into();
            }
            // Connected terminals whose Agent turn stopped (idle/done/blocked/error) emit no warning at all.
            for stopped_state in ["idle", "done", "blocked", "error"] {
                let obs = Some(json!({"state":"live","status":{"state":stopped_state}}));
                current.agent.as_mut().unwrap().observed = obs;
                db.save(&current)?;
                assert!(
                    super::review::activity_warning(db, &card)?.is_none(),
                    "expected no warning when Herdr reports {stopped_state}"
                );
            }
            // Disconnected or actively working terminals surface an advisory warning.
            for connection in ["unavailable", "replaced"] {
                let obs = Some(json!({"state":"live","status":{"state":"done"}}));
                let a = current.agent.as_mut().unwrap();
                a.connection_state = connection.into();
                a.observed = obs;
                db.save(&current)?;
                assert!(
                    super::review::activity_warning(db, &card)?
                        .unwrap()
                        .contains("status is unavailable")
                );
            }
            let obs = Some(json!({"state":"live","status":{"state":"working"}}));
            let a = current.agent.as_mut().unwrap();
            a.connection_state = "connected".into();
            a.observed = obs;
            db.save(&current)?;
            assert!(
                super::review::activity_warning(db, &card)?
                    .unwrap()
                    .contains("still running")
            );
            Ok(())
        })
        .unwrap();

    // Even when an Agent is actively working, preview keeps merge_allowed=true with a warning,
    // and review.merge is NOT blocked.
    let working_preview = app
        .call("review.preview", json!({"card_id":card.id}))
        .unwrap();
    assert_eq!(working_preview["merge_allowed"], true, "{working_preview}");
    assert!(working_preview["reason"].is_null(), "{working_preview}");
    assert!(
        working_preview["warning"]
            .as_str()
            .unwrap()
            .contains("still running"),
        "{working_preview}"
    );

    // When Herdr reports the Agent is idle at its TUI prompt, warning disappears completely.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut current: TerminalEntry = db.get(&current_id)?;
            let obs = Some(json!({"state":"live","status":{"state":"idle"}}));
            current.agent.as_mut().unwrap().observed = obs;
            db.save(&current)
        })
        .unwrap();
    let idle_preview = app
        .call("review.preview", json!({"card_id":card.id}))
        .unwrap();
    assert_eq!(idle_preview["merge_allowed"], true, "{idle_preview}");
    assert!(idle_preview["reason"].is_null(), "{idle_preview}");
    assert!(idle_preview["warning"].is_null(), "{idle_preview}");

    // Queued/starting/unknown runs without a terminal also only warn and never block human merge.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut pending = run.clone();
            pending.id = format!("{}:developer", card.id);
            pending.conversation_id = Some(pending.id.clone());
            pending.number = 99;
            {
                let a = pending.agent.as_mut().unwrap();
                a.terminal = None;
                a.core_id = None;
                a.observed = None;
            }
            for state in ["queued", "starting", "unknown"] {
                pending.agent.as_mut().unwrap().state = state.into();
                db.save(&pending)?;
                assert!(
                    super::review::activity_warning(db, &card)?
                        .unwrap()
                        .contains("pending Agent work")
                );
            }
            Ok(())
        })
        .unwrap();
    let queued_preview = app
        .call("review.preview", json!({"card_id":card.id}))
        .unwrap();
    assert_eq!(queued_preview["merge_allowed"], true, "{queued_preview}");
    assert!(
        queued_preview["warning"]
            .as_str()
            .unwrap()
            .contains("pending Agent work"),
        "{queued_preview}"
    );
    card = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.id == card.id)
        .unwrap();
    let merged = app
        .call(
            "review.merge",
            json!({
                "card_id": card.id,
                "revision": card.revision,
                "token": queued_preview["token"],
                "request_key": "merge-with-warning-allowed"
            }),
        )
        .unwrap();
    assert_eq!(merged["state"], "merged", "{merged}");
}

#[test]
fn core_loss_during_question_delivery_becomes_unknown_without_replay() {
    let (_dir, app, _project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let run = admit(&app, &app.snapshot().unwrap().terminals[0]);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&run.id)?;
            let a = entry.agent.as_mut().unwrap();
            a.connection_state = "connected".into();
            a.lifecycle.observed_at = Some(now());
            db.save(&entry)
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
    let req = app.snapshot().unwrap().terminals[0].clone();
    let run = admit(&app, &req);
    let payload = json!({"note_id":b.id,"revision":renamed.revision,"body":"Updated by Agent","execution":{"terminal_id":run.id},"request_key":"note-agent-once"});
    let saved = app.call("note.write", payload.clone()).unwrap();
    assert_eq!(app.call("note.write", payload).unwrap(), saved);
    let history: Vec<NoteEvent> =
        serde_json::from_value(app.call("note.history", json!({"note_id":b.id})).unwrap()).unwrap();
    let event = history.last().unwrap();
    assert_eq!(event.actor, "@developer");
    assert_eq!(event.card_id, Some(card.id.clone()));
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
    assert!(app.call("note.write",json!({"note_id":b.id,"revision":saved["revision"],"body":"Bad invocation","execution":{"terminal_id":"missing-terminal"}})).is_err());
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
    assert!(app.snapshot().unwrap().terminals.is_empty());
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
    db.execute_batch("DROP TABLE note_events; DROP TABLE note_revisions; DROP TABLE terminals; DROP TABLE card_reads; DROP TABLE github_issues; DROP INDEX github_card_source; DROP TABLE editor_drafts; PRAGMA user_version=2;")
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
            let req = app.snapshot().unwrap().terminals.last().unwrap().clone();
            admit(&app, &req);
        }
    }
    let mut snapshot = app.snapshot().unwrap();
    assert_eq!(canban_scheduler::active_count(&snapshot), 8);
    assert!(canban_scheduler::next(&snapshot, 8).is_none());
    {
        let a = snapshot.terminals[0].agent.as_mut().unwrap();
        a.state = "completed".into();
        a.close_state = "closing".into();
    }
    assert!(canban_scheduler::next(&snapshot, 8).is_none());
    snapshot.terminals[0].agent.as_mut().unwrap().observed = Some(json!({"state":"finished"}));
    assert!(canban_scheduler::next(&snapshot, 8).is_some());
    {
        let a = snapshot.terminals[0].agent.as_mut().unwrap();
        a.observed = None;
        a.terminal = None;
        a.state = "unknown".into();
    }
    assert!(canban_scheduler::next(&snapshot, 8).is_none());
    let path = app.home.join("config/agents.json");
    let before: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    app.call("scheduler.configure", json!({"max_agents":2}))
        .unwrap();
    assert_eq!(app.config().scheduler.max_agents, 2);
    assert_eq!(app.snapshot().unwrap().terminals.len(), 9);
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
    let (dir, app, _project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let custom_yaml = r#"schema: 1
id: delivery:second-board
name: Second Board
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: custom-final-check
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
  custom-final-check:
    event: card.column_entered
    start_at: agent:custom-final-check
    match:
      column: custom-final-check
steps:
  - id: agent:develop
    uses: agent
    with:
      profile: developer
      instruction: Implement
    next: move:develop
  - id: move:develop
    uses: card.move
    with:
      column: custom-final-check
    next: agent:custom-final-check
  - id: agent:custom-final-check
    uses: agent
    with:
      profile: reviewer
      instruction: Final check
    next: __end
  - id: __end
    uses: complete
"#;
    let second_project: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name":"Second","directory":dir.path().join("second-source"),"workflow_yaml":custom_yaml}),
        )
        .unwrap(),
    )
    .unwrap();
    let second: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id":second_project.id,"title":"Later stage"}),
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
        canban_scheduler::next(&snapshot, 8)
            .unwrap()
            .agent
            .unwrap()
            .card_id,
        second.id
    );
    snapshot
        .cards
        .iter_mut()
        .find(|c| c.id == second.id)
        .unwrap()
        .human_gate = Some(HumanGate {
        reason: "Pause".into(),
        terminal_id: None,
        next_stage: None,
    });
    assert_eq!(
        canban_scheduler::next(&snapshot, 8)
            .unwrap()
            .agent
            .unwrap()
            .card_id,
        card.id
    );
    let mut manual = snapshot.terminals[1].clone();
    manual.id = "manual".into();
    manual.agent.as_mut().unwrap().trigger = "interactive".into();
    snapshot.terminals.push(manual);
    assert_eq!(canban_scheduler::next(&snapshot, 8).unwrap().id, "manual");
    snapshot.terminals.pop();
    snapshot.terminals[1].agent.as_mut().unwrap().stage = "develop".into();
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
    snapshot.terminals[0].agent.as_mut().unwrap().updated_at = 1;
    snapshot.terminals[1].agent.as_mut().unwrap().updated_at = 1;
    assert_eq!(
        canban_scheduler::next(&snapshot, 8)
            .unwrap()
            .agent
            .unwrap()
            .card_id,
        card.id
    );
}

#[test]
fn human_review_gate_survives_reports_acknowledgement_restart_and_requires_explicit_approval() {
    let (dir, app, project, card) = setup();
    apply_project_yaml(
        &app,
        &project.id,
        &develop_and_review_yaml(&project.id, true, true, true),
    );
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().terminals[0].clone();
    let _run = admit(&app, &request);
    let report = json!({"project_id":project.id,"card":card.number,"profile_id":"developer","state":"completed","request_key":"human-gate-complete"});
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
    assert_eq!(snapshot.terminals.len(), 1);
    app.call("card.attention.clear", json!({"card_id":card.id}))
        .unwrap();
    assert!(app.snapshot().unwrap().cards[0].human_gate.is_some());
    // Explicit manual work can run, but its completion cannot approve this gate.
    app.call(
        "comment.add",
        json!({"card_id":card.id,"body":"@reviewer help me inspect"}),
    )
    .unwrap();
    let manual = app.snapshot().unwrap().terminals.last().unwrap().clone();
    let _manual_run = admit(&app, &manual);
    app.call("execution.report",json!({"project_id":project.id,"card":card.number,"profile_id":"reviewer","state":"completed","request_key":"human-gate-manual"})).unwrap();
    assert!(app.snapshot().unwrap().cards[0].human_gate.is_some());
    drop(app);
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    let current = app.snapshot().unwrap().cards[0].clone();
    assert!(current.human_gate.is_some());
    let approve =
        json!({"card_id":card.id,"revision":current.revision,"request_key":"approve-gate"});
    assert!(app.call("card.continue",json!({"card_id":card.id,"revision":current.revision,"execution":{"agent":true,"profile_id":"reviewer"}})).is_err());
    assert!(app.call("card.move",json!({"card_id":card.id,"revision":current.revision,"stage":"review","execution":{"agent":true,"profile_id":"reviewer"}})).is_err());
    app.call("card.continue", approve.clone()).unwrap();
    app.call("card.continue", approve).unwrap();
    let after = app.snapshot().unwrap();
    assert_eq!(after.cards[0].stage, "review");
    assert!(after.cards[0].human_gate.is_none());
    assert_eq!(
        after
            .terminals
            .iter()
            .filter(|r| r.agent.as_ref().is_some_and(|a| {
                (a.trigger == ExecutionTrigger::StageEntry && a.stage == "review")
                    || a.pending.as_ref().is_some_and(|p| {
                        p.trigger == ExecutionTrigger::StageEntry && p.stage == "review"
                    })
            }))
            .count(),
        1
    );
}

#[test]
fn blocked_completion_does_not_advance_until_human_continues() {
    let (_dir, app, project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().terminals[0].clone();
    let _run = admit(&app, &request);
    for state in ["blocked", "completed"] {
        app.call("execution.report",json!({"project_id":project.id,"card":card.number,"state":state,"request_key":format!("blocked-{state}")})).unwrap();
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
    let (_dir, app, project, card) = setup();
    app.call(
        "card.move",
        json!({"card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let request = app.snapshot().unwrap().terminals[0].clone();
    let _run = admit(&app, &request);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut c: Card = db.get(&card.id)?;
            attention(db, &mut c, "Connection needs inspection")?;
            db.save(&c)
        })
        .unwrap();
    app.call("execution.report",json!({"project_id":project.id,"card":card.number,"state":"completed","request_key":"recovery-completed"})).unwrap();
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
    assert!(snapshot.terminals.is_empty());
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
    assert!(app.snapshot().unwrap().terminals.is_empty());
    app.call(
        "comment.add",
        json!({"card_id":c.id,"body":"Compare @claude and @codex", "mentions":["codex"]}),
    )
    .unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.terminals.len(), 1);
    assert_eq!(s.terminals[0].agent.as_ref().unwrap().profile.id, "codex");
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
    assert!(
        s.terminals
            .iter()
            .all(|r| r.agent.as_ref().unwrap().state == "cancelled")
    );
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
    assert_eq!(s.terminals.len(), 1);
    assert_eq!(s.terminals[0].agent.as_ref().unwrap().state, "cancelled");
}
#[test]
fn local_card_removal_cannot_orphan_a_live_agent() {
    let (_d, app, _p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":c.revision,"stage":"develop"}),
    )
    .unwrap();
    let run = admit(&app, &app.snapshot().unwrap().terminals[0]);
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
    assert_eq!(s.terminals[0].id, run.id);
    assert!(!s.events.iter().any(|e| e.kind == "card.removed"));
}

#[test]
fn continued_assignment_rejects_new_stale_reports_but_preserves_completion_receipts() {
    let (_d, app, p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let run = admit(&app, &app.snapshot().unwrap().terminals[0]);
    let report = json!({"request_key":"completed-before-continuation","project_id":p.id,"card":c.number,"state":"completed","message":"Done"});
    app.call("execution.report", report.clone()).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&run.id)?;
            entry.agent.as_mut().unwrap().state = "continued".into();
            db.save(&entry)
        })
        .unwrap();
    assert_eq!(
        app.call("execution.report", report).unwrap()["duplicate"],
        true
    );
    let before = app.snapshot().unwrap();
    let result=app.call("execution.report",json!({"request_key":"obsolete-progress","terminal_id":run.id,"state":"working","body":"Stale"}));
    assert!(result.is_err());
    let after = app.snapshot().unwrap();
    assert_eq!(before.reports, after.reports);
    assert_eq!(
        after.terminals[0].agent.as_ref().unwrap().state,
        "continued"
    );
}

#[path = "tests/board.rs"]
mod board_cases;

#[test]
fn read_positions_are_monotonic_durable_and_never_approve_a_gate() {
    let (_dir, app, _, card) = setup();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut c: Card = db.get(&card.id)?;
            c.human_gate = Some(HumanGate {
                reason: "Review required".into(),
                terminal_id: None,
                next_stage: None,
            });
            db.save(&c)?;
            for n in 0..3 {
                db.append(canban_timeline::event(
                    &c.project_id,
                    &c.id,
                    "comment.replied",
                    "agent:developer",
                    &format!("Reply {n}"),
                    json!({}),
                ))?;
            }
            Ok(())
        })
        .unwrap();
    let before = app.snapshot().unwrap();
    let last = before.events.iter().map(|e| e.sequence).max().unwrap();
    app.call(
        "notifications.read",
        json!({"card_id":card.id,"sequence":last-1}),
    )
    .unwrap();
    app.call(
        "notifications.read",
        json!({"card_id":card.id,"sequence":1}),
    )
    .unwrap();
    assert_eq!(
        canban_notifications::read_sequence(&app.snapshot().unwrap(), &card.id),
        last - 1
    );
    app.call(
        "notifications.read",
        json!({"card_id":card.id,"sequence":u64::MAX}),
    )
    .unwrap();
    let after = app.snapshot().unwrap();
    assert_eq!(canban_notifications::read_sequence(&after, &card.id), last);
    assert_eq!(before.cards, after.cards);
    assert_eq!(before.events, after.events);
    assert_eq!(before.terminals, after.terminals);
    assert!(
        app.call(
            "notifications.read",
            json!({"card_id":card.id,"sequence":last,"execution":{}})
        )
        .is_err()
    );
    let home = app.home.clone();
    drop(app);
    let reopened = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    assert_eq!(after.card_reads, reopened.snapshot().unwrap().card_reads);
}

#[test]
fn reminder_upgrade_baselines_existing_history_without_changing_cards() {
    let (_dir, app, _, card) = setup();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.append(canban_timeline::event(
                &card.project_id,
                &card.id,
                "comment.replied",
                "agent:developer",
                "Historical reply",
                json!({}),
            ))?;
            Ok(())
        })
        .unwrap();
    let before = app.snapshot().unwrap();
    let home = app.home.clone();
    drop(app);
    let conn = rusqlite::Connection::open(home.join("canban.sqlite")).unwrap();
    conn.execute_batch("DROP TABLE card_reads; DROP TABLE editor_drafts; PRAGMA user_version=7;")
        .unwrap();
    drop(conn);
    let upgraded = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    let after = upgraded.snapshot().unwrap();
    assert_eq!(before.cards, after.cards);
    assert_eq!(before.events, after.events);
    assert!(canban_notifications::reminders(&after).is_empty());
    assert!(canban_notifications::read_sequence(&after, &card.id) > 0);
}

#[test]
fn agent_callbacks_cannot_install_desktop_updates() {
    let (_dir, app, _, _) = setup();
    let error = app
        .call(
            "update.install",
            json!({"execution":{"agent":true,"profile_id":"developer"}}),
        )
        .unwrap_err();
    assert!(error.to_string().contains("explicit user action"));
    assert!(!app.home.join("updates/install.json").exists());
}

#[test]
fn git_refs_compare_and_review_send_agent_work_with_and_without_cards() {
    let (dir, app, p, c) = setup();
    let repo = dir.path().join("source");
    std::fs::create_dir_all(&repo).unwrap();
    let run_git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    };
    run_git(&["init", "-b", "main"]);
    run_git(&["config", "user.name", "Tester"]);
    run_git(&["config", "user.email", "tester@example.invalid"]);
    run_git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("lib.rs"), "pub fn v1() -> i32 { 1 }\n").unwrap();
    run_git(&["add", "."]);
    run_git(&["commit", "-m", "initial"]);

    run_git(&["switch", "-c", "feat/compare"]);
    std::fs::write(
        repo.join("lib.rs"),
        "pub fn v1() -> i32 { 2 }\npub fn v2() {}\n",
    )
    .unwrap();
    run_git(&["add", "."]);
    run_git(&["commit", "-m", "add v2"]);

    let refs = app.call("git.refs", json!({"project_id": p.id})).unwrap();
    assert_eq!(refs["current_branch"], "feat/compare");
    assert!(
        refs["branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["name"] == "feat/compare")
    );

    let cmp = app
        .call(
            "git.compare",
            json!({
                "project_id": p.id,
                "base": "main",
                "compare": "feat/compare",
                "three_dot": true,
            }),
        )
        .unwrap();
    assert_eq!(cmp["files"].as_array().unwrap().len(), 1);
    assert_eq!(cmp["files"][0]["filename"], "lib.rs");

    // Dispatch structured inline comments to an existing Card's @developer locally
    let sent = app
        .call(
            "review.send_agent",
            json!({
                "mode": "card",
                "card_id": c.id,
                "profile_id": "developer",
                "move_to_develop": true,
                "context_label": "main ← feat/compare",
                "summary": "Please handle edge cases.",
                "comments": [
                    {
                        "path": "lib.rs",
                        "line": 2,
                        "side": "RIGHT",
                        "snippet": "pub fn v2() {}",
                        "body": "Return a Result here."
                    }
                ],
                "request_key": "review-send-1",
            }),
        )
        .unwrap();
    assert_eq!(sent["state"], "sent");
    let snap = app.snapshot().unwrap();
    let updated_card = snap.cards.iter().find(|x| x.id == c.id).unwrap();
    assert_eq!(updated_card.stage, "develop");
    assert!(
        snap.questions.iter().any(|q| q.card_id == c.id
            && q.profile_id == "developer"
            && q.body.contains("lib.rs:2"))
    );

    // Ad-hoc compare can also create a brand-new Card & assign @developer
    let created = app
        .call(
            "review.send_agent",
            json!({
                "mode": "new_card",
                "project_id": p.id,
                "profile_id": "developer",
                "title": "Address compare review",
                "context_label": "main ← feat/compare",
                "summary": "Follow up on branch diff.",
                "comments": [],
                "request_key": "review-new-card-1",
            }),
        )
        .unwrap();
    assert_eq!(created["state"], "created");
    let new_cid = created["card_id"].as_str().unwrap();
    let snap2 = app.snapshot().unwrap();
    assert!(
        snap2
            .cards
            .iter()
            .any(|x| x.id == new_cid && x.title == "Address compare review")
    );
}

mod github;

mod workflows;

#[test]
fn agent_onboarding_enabled_gating_custom_identities_and_project_overrides_work_end_to_end() {
    let (dir, app, project, card) = setup();
    let _ = dir;

    // 1. agent.catalog returns all 24 built-in Herdr agents + current config.
    let catalog = app.call("agent.catalog", json!({})).unwrap();
    let agents = catalog["agents"].as_array().unwrap();
    assert!(agents.len() >= 24);
    assert!(
        agents
            .iter()
            .any(|a| a["id"] == "agy" && a["binary"] == "agy")
    );
    assert!(
        agents
            .iter()
            .any(|a| a["id"] == "gemini" && a["binary"] == "gemini")
    );

    // 2. Run Onboarding (`agent.setup`): enable `codex` + `agy` (disabling `claude`),
    // set `agy` as the primary engine for `@planner`, `@developer`, `@reviewer`.
    let setup_res = app
        .call(
            "agent.setup",
            json!({
                "enabled_agents": ["codex", "agy"],
                "default_engine": "agy",
                "default_agent": "developer",
            }),
        )
        .unwrap();
    assert_eq!(setup_res["onboarded"], true);
    assert_eq!(setup_res["default_agent"], "developer");

    let cfg = app.config();
    assert!(cfg.onboarded);
    let claude = cfg.profiles.iter().find(|p| p.id == "claude").unwrap();
    assert!(!claude.enabled, "unchecked claude should be disabled");
    let agy = cfg.profiles.iter().find(|p| p.id == "agy").unwrap();
    assert!(agy.enabled, "checked agy should be created and enabled");
    let dev = cfg.profiles.iter().find(|p| p.id == "developer").unwrap();
    assert_eq!(dev.command()[0], "agy");
    let planner = cfg.profiles.iter().find(|p| p.id == "planner").unwrap();
    assert_eq!(planner.command()[0], "agy");
    let merger = cfg.profiles.iter().find(|p| p.id == "merger").unwrap();
    assert!(merger.is_identity() && merger.enabled);
    assert_eq!(merger.command()[0], "agy");
    assert!(
        app.call(
            "config.agents.save",
            json!({ "op": "delete_profile", "id": "merger" })
        )
        .is_err(),
        "built-in @merger cannot be deleted"
    );

    // 3. Disabled `@claude` is ignored in prose mentions and rejected when explicitly mentioned!
    app.call(
        "comment.add",
        json!({
            "card_id": card.id,
            "body": "Hey @claude should not trigger because claude is disabled",
        }),
    )
    .unwrap();
    assert!(
        !app.snapshot()
            .unwrap()
            .questions
            .iter()
            .any(|q| q.card_id == card.id && q.profile_id == "claude"),
        "disabled @claude must not queue from prose mentions"
    );
    let err = app
        .call(
            "comment.add",
            json!({
                "card_id": card.id,
                "body": "Explicit @claude",
                "mentions": ["claude"],
            }),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("disabled"));

    // 4. Create a custom identity `@qa` backed by `codex`, verify it works in comments,
    // and verify re-running `agent.setup` preserves `@qa` and its custom role_instruction!
    app.call(
        "config.agents.save",
        json!({
            "op": "upsert_identity",
            "id": "qa",
            "engine": "codex",
            "role_instruction": "Run edge-case QA checks thoroughly.",
        }),
    )
    .unwrap();
    app.call(
        "agent.setup",
        json!({
            "enabled_agents": ["codex", "agy"],
            "default_engine": "agy",
        }),
    )
    .unwrap();
    let cfg_after = app.config();
    let qa = cfg_after.profiles.iter().find(|p| p.id == "qa").unwrap();
    assert!(qa.is_identity() && qa.enabled);
    assert_eq!(qa.role_instruction, "Run edge-case QA checks thoroughly.");
    assert_eq!(qa.command()[0], "codex");

    // 5. Project-level identity override: override `@developer` -> `codex` ONLY for `project`,
    // while global `@developer` remains `agy`!
    app.call(
        "project.agents",
        json!({
            "project_id": project.id,
            "agent_overrides": {
                "developer": "codex"
            }
        }),
    )
    .unwrap();
    app.call(
        "comment.add",
        json!({
            "card_id": card.id,
            "body": "@developer check project override",
            "mentions": ["developer"],
        }),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let req = snap
        .terminals
        .iter()
        .filter_map(|r| r.agent.as_ref())
        .rfind(|r| r.card_id == card.id && r.profile.id == "developer")
        .expect("developer request queued");
    assert_eq!(
        req.profile.command()[0],
        "codex",
        "Project override must route @developer to codex without altering global agy"
    );
    assert!(
        req.profile.role_instruction.contains("development Agent"),
        "Project override must preserve @developer's role instruction"
    );
    assert_eq!(
        app.config()
            .profiles
            .iter()
            .find(|p| p.id == "developer")
            .unwrap()
            .command()[0],
        "agy",
        "Global @developer remains agy for other projects"
    );

    // 6. Workflow YAML steps that omit `profile` (or specify `profile: null`) strictly route
    // through `config.default_agent` (converged to `@developer` by `agent.setup`, without any
    // step-ID string inference)! Also verify re-running `agent.setup` without `default_agent`
    // migrates a legacy raw `default_agent: "codex"` to `"developer"`.
    app.call(
        "config.agents.save",
        json!({"op": "set_default", "default_agent": "codex"}),
    )
    .unwrap();
    assert_eq!(app.config().default_agent, "codex");
    app.call(
        "agent.setup",
        json!({
            "enabled_agents": ["codex", "agy"],
            "default_engine": "agy",
        }),
    )
    .unwrap();
    assert_eq!(
        app.config().default_agent,
        "developer",
        "Re-running Setup without explicit default_agent converges raw CLI default_agent to @developer"
    );

    let second_proj: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "NullProfileWorkflow",
                "directory": dir.path().join("null-prof"),
                "workspace": {"create_worktree": false, "sync_base": false},
                "workflow_yaml": "schema: 1\nid: delivery:null-prof\nname: Development\non:\n  develop:\n    event: card.column_entered\n    start_at: custom_step_name\n    match:\n      column: develop\nsteps:\n  - id: custom_step_name\n    uses: agent\n    with:\n      instruction: Run default agent.\n      profile: null\n    next: __end\n  - id: __end\n    uses: complete\nworkspace:\n  create_worktree: false\n  sync_base: false\n",
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let second_card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id": second_proj.id, "title": "Drag to Develop"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id": second_card.id, "revision": second_card.revision, "stage": "develop"}),
    )
    .unwrap();
    let null_prof_req = app
        .snapshot()
        .unwrap()
        .terminals
        .into_iter()
        .filter_map(|r| r.agent)
        .rfind(|r| r.card_id == second_card.id)
        .expect("workflow request queued on drag to develop");
    assert_eq!(null_prof_req.profile.id, "developer");
    assert_eq!(
        null_prof_req.profile.command()[0],
        "agy",
        "Omitted profile must use default_agent (@developer -> agy)"
    );

    // 7. Global `model` & `effort` options on CLI Agents and Identities (`set_profile_options`):
    //    - Setting `model` / `effort` on `@agy` propagates to `@developer` (which uses preset `agy`)
    //      without rewriting `@developer.model` in raw `config/agents.json`.
    //    - Setting an explicit `model` / `effort` on `@developer` overrides `@agy`'s defaults.
    //    - Switching `@developer`'s backing engine to `codex` clears incompatible `agy` model/effort.
    app.call(
        "config.agents.save",
        json!({
            "op": "set_profile_options",
            "id": "agy",
            "model": "gemini-4-carbon-medium",
            "effort": "medium",
        }),
    )
    .unwrap();
    let cfg_after_agy_opts = app.config();
    let dev_inherited = cfg_after_agy_opts
        .profiles
        .iter()
        .find(|p| p.id == "developer")
        .unwrap();
    assert_eq!(
        dev_inherited.model.as_deref(),
        Some("gemini-4-carbon-medium")
    );
    assert_eq!(dev_inherited.effort.as_deref(), Some("medium"));

    let cat_after_agy = app
        .call("agent.catalog", json!({"machine": "local"}))
        .unwrap();
    let raw_dev = cat_after_agy["raw_profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "developer")
        .unwrap();
    assert!(
        raw_dev.get("model").is_none() || raw_dev["model"].is_null(),
        "Inherited model must not be persisted as an explicit identity override"
    );
    let agy_entry = cat_after_agy["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "agy")
        .unwrap();
    assert!(agy_entry["supports_model"].as_bool().unwrap());
    assert!(agy_entry["supports_effort"].as_bool().unwrap());
    assert!(!agy_entry["models"].as_array().unwrap().is_empty());

    // Explicit identity override on `@developer`
    app.call(
        "config.agents.save",
        json!({
            "op": "set_profile_options",
            "id": "developer",
            "model": "claude-opus-5-5-high",
            "effort": "high",
        }),
    )
    .unwrap();
    let dev_explicit = app
        .config()
        .profiles
        .into_iter()
        .find(|p| p.id == "developer")
        .unwrap();
    assert_eq!(dev_explicit.model.as_deref(), Some("claude-opus-5-5-high"));
    assert_eq!(dev_explicit.effort.as_deref(), Some("high"));

    // Updating `@developer`'s prompt on the SAME engine preserves explicit model & effort
    app.call(
        "config.agents.save",
        json!({
            "op": "upsert_identity",
            "id": "developer",
            "engine": "agy",
            "role_instruction": "Custom developer prompt.",
            "enabled": true,
        }),
    )
    .unwrap();
    let dev_same_eng = app
        .config()
        .profiles
        .into_iter()
        .find(|p| p.id == "developer")
        .unwrap();
    assert_eq!(dev_same_eng.model.as_deref(), Some("claude-opus-5-5-high"));
    assert_eq!(dev_same_eng.effort.as_deref(), Some("high"));

    // Switching `@developer` to `codex` clears its incompatible `agy` model/effort override
    app.call(
        "config.agents.save",
        json!({
            "op": "upsert_identity",
            "id": "developer",
            "engine": "codex",
            "enabled": true,
        }),
    )
    .unwrap();
    let dev_switched = app
        .config()
        .profiles
        .into_iter()
        .find(|p| p.id == "developer")
        .unwrap();
    assert_eq!(dev_switched.model, None);
    assert_eq!(dev_switched.effort, None);
}

pub(super) fn apply_project_yaml(app: &AppServer, project_id: &str, yaml: &str) {
    let sources = app
        .call("workflow.sources", json!({"project_id": project_id}))
        .unwrap();
    app.call(
        "workflow.apply",
        json!({
            "project_id": project_id,
            "revision": sources["revision"],
            "sources": [{"filename": "development.yaml", "source": yaml}],
        }),
    )
    .unwrap();
}

pub(super) fn develop_and_review_yaml(
    project_id: &str,
    develop_human_review: bool,
    create_worktree: bool,
    sync_base: bool,
) -> String {
    let develop_next = if develop_human_review {
        "approval:develop"
    } else {
        "move:develop"
    };
    let approval_step = if develop_human_review {
        r#"  - id: approval:develop
    uses: approval
    with:
      title: Agent report ready; human review is required before continuing
    next: move:develop
"#
    } else {
        ""
    };
    format!(
        r#"schema: 1
id: delivery:{project_id}
name: Development
workspace:
  create_worktree: {create_worktree}
  sync_base: {sync_base}
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
  review:
    event: card.column_entered
    start_at: agent:review
    match:
      column: review
steps:
  - id: agent:develop
    uses: agent
    with:
      profile: developer
      instruction: Implement the card task in the supplied working directory. Explain your changes and validation in your report.
    next: {develop_next}
{approval_step}  - id: move:develop
    uses: card.move
    with:
      column: review
    next: agent:review
  - id: agent:review
    uses: agent
    with:
      profile: reviewer
      instruction: Review changes
    next: __end
  - id: __end
    uses: complete
"#
    )
}

#[test]
fn request_trigger_prompts_omit_duplicate_reply_for_comments_and_dedupe_completion_after_reply() {
    let (_dir, app, project, card) = setup();

    // 1. Comment-triggered attempt uses `ExecutionTrigger::Comment` and `reporting_comment.txt`
    // without rendering a separate `## Pending questions` / `reply --to` instruction.
    let comment_ev: Event = serde_json::from_value(
        app.call(
            "comment.add",
            json!({"card_id": card.id, "body": "@developer What should we name the merger role?"}),
        )
        .unwrap(),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let comment_req = snap
        .terminals
        .iter()
        .find(|r| {
            r.agent
                .as_ref()
                .is_some_and(|a| a.card_id == card.id && a.trigger.is_comment())
        })
        .expect("comment terminal queued")
        .clone();
    let comment_agent = comment_req.agent.as_ref().unwrap();
    assert_eq!(
        comment_agent.trigger,
        ExecutionTrigger::Comment {
            comment_id: comment_ev.id.clone()
        }
    );
    assert_eq!(
        serde_json::to_value(&comment_agent.trigger).unwrap(),
        json!(format!("comment:{}", comment_ev.id))
    );

    let capture_prompt = |card: &Card, att: &TerminalEntry| -> String {
        let s = app.snapshot().unwrap();
        let rel = canban_context::capture(
            &app.home,
            &project,
            card,
            att,
            &s,
            "/tmp/test.sock",
            Path::new("/bin/canban"),
        )
        .unwrap();
        std::fs::read_to_string(app.home.join(rel)).unwrap()
    };

    let comment_attempt = admit(&app, &comment_req);
    let comment_prompt = capture_prompt(&card, &comment_attempt);
    assert!(
        comment_prompt.contains(
            "Publish your answer and complete this comment assignment in a single CLI call"
        ),
        "comment prompt should use reporting_comment.txt"
    );
    assert!(
        !comment_prompt.contains("## Pending questions") && !comment_prompt.contains("reply --to"),
        "comment prompt must not instruct Agent to run both `reply` and `report`"
    );

    // Fallback safety: even if an Agent explicitly called `agent.reply` first and then `execution.report`,
    // `execution.reported` is marked `comment_replied: true` and does not create a second unread item.
    let q = snap
        .questions
        .iter()
        .find(|q| q.terminal_id.as_deref() == Some(&comment_req.id))
        .unwrap();
    app.call(
        "agent.reply",
        json!({
            "request_key": "explicit-reply-first",
            "project_id": project.id,
            "card": card.number,
            "profile_id": "developer",
            "question_id": q.id,
            "body": "Recommend `@merger`."
        }),
    )
    .unwrap();
    app.call(
        "execution.report",
        json!({
            "request_key": "followup-report",
            "project_id": project.id,
            "card": card.number,
            "profile_id": "developer",
            "state": "completed",
            "message": "Answered naming question"
        }),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(
        snap.events
            .iter()
            .filter(|e| e.data["terminal_id"].as_str() == Some(&comment_req.id)
                && e.kind == "comment.replied")
            .count(),
        1
    );
    let reported_ev = snap
        .events
        .iter()
        .find(|e| {
            e.data["terminal_id"].as_str() == Some(&comment_req.id)
                && e.kind == "execution.reported"
        })
        .unwrap();
    assert_eq!(reported_ev.data["comment_replied"], json!(true));
    let counts = canban_notifications::counts(&canban_notifications::reminders(&snap));
    assert_eq!(
        counts[&card.id].unread, 1,
        "explicit reply + completed report on a comment request must count as a single unread reply"
    );

    // 2. Interactive attempt uses `ExecutionTrigger::Interactive` and `reporting_interactive.txt`.
    let c2: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id": project.id, "title": "Interactive Card", "body": "Discuss"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "agent.open",
        json!({"card_id": c2.id, "profile_id": "developer"}),
    )
    .unwrap();
    let interactive_req = app
        .snapshot()
        .unwrap()
        .terminals
        .into_iter()
        .find(|r| r.agent.as_ref().is_some_and(|a| a.card_id == c2.id))
        .unwrap();
    assert_eq!(
        interactive_req.agent.as_ref().unwrap().trigger,
        ExecutionTrigger::Interactive
    );
    let interactive_attempt = admit(&app, &interactive_req);
    let interactive_prompt = capture_prompt(&c2, &interactive_attempt);
    assert!(
        interactive_prompt.contains("This is an interactive Card discussion session"),
        "interactive prompt should use reporting_interactive.txt"
    );

    // 3. Workflow/stage task with an injected pending question DOES include `## Pending questions`.
    let c3: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id": project.id, "title": "Task Card", "body": "Build"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id": c3.id, "revision": c3.revision, "stage": "develop"}),
    )
    .unwrap();
    app.call(
        "comment.add",
        json!({"card_id": c3.id, "body": "@developer Please also check Windows paths"}),
    )
    .unwrap();
    let task_req = app
        .snapshot()
        .unwrap()
        .terminals
        .into_iter()
        .find(|r| {
            r.agent
                .as_ref()
                .is_some_and(|a| a.card_id == c3.id && a.trigger.is_automatic())
        })
        .unwrap();
    let task_attempt = admit(&app, &task_req);
    let task_prompt = capture_prompt(&c3, &task_attempt);
    assert!(
        task_prompt.contains("Report your outcome via CLI")
            && task_prompt.contains("## Pending questions")
            && task_prompt.contains("reply --to"),
        "non-comment task with a concurrent question retains `## Pending questions` + `reply --to`"
    );
}

#[test]
fn project_tags_register_deduplicate_and_reject_invalid_or_numeric_names() {
    let (_dir, app, project, _card) = setup();
    assert!(project.tags.is_empty());

    let res = app
        .call(
            "tag.create",
            json!({"project_id": project.id, "tag": "#ui-kit"}),
        )
        .unwrap();
    assert_eq!(res["tag"], "ui-kit");
    assert_eq!(res["tags"], json!(["ui-kit"]));

    // Case-insensitive deduplication preserves the first canonical casing.
    let res_dup = app
        .call(
            "tag.create",
            json!({"project_id": project.id, "tag": "UI-KIT"}),
        )
        .unwrap();
    assert_eq!(res_dup["tag"], "ui-kit");
    assert_eq!(res_dup["tags"], json!(["ui-kit"]));

    // Unicode / CJK tags work cleanly.
    let res_cjk = app
        .call(
            "tag.create",
            json!({"project_id": project.id, "tag": "#前端"}),
        )
        .unwrap();
    assert_eq!(res_cjk["tags"], json!(["ui-kit", "前端"]));

    // Persisted in Snapshot.
    let snap = app.snapshot().unwrap();
    assert_eq!(
        snap.projects[0].tags,
        vec!["ui-kit".to_string(), "前端".to_string()]
    );

    // Pure numbers, empty strings, and punctuation/whitespace are rejected.
    for bad in ["", "#", "18", "#42", "bad space", "bad/slash"] {
        assert!(
            app.call("tag.create", json!({"project_id": project.id, "tag": bad}))
                .is_err(),
            "expected {bad:?} to be rejected as a tag"
        );
    }
}
