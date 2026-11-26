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
            json!({"view":"full","project_id":p.id,"title":"Implement change","body":"Task context"}),
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
        json!({"view":"full","card_id":card.id,"revision":1,"stage":"develop"}),
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
        origin_project_id: None,
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
        json!({"view":"full","project_id":project.id,"title":"Large body","body":body}),
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
        json!({"view":"full","card_id":card.id,"revision":1,"note_ids":[first.id]}),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"view":"full","card_id":card.id,"revision":2,"stage":"develop"}),
    )
    .unwrap();
    let snapshot = app.snapshot().unwrap();
    let req = &snapshot.terminals[0];
    let card = &snapshot.cards[0];
    let card_events = app.events().unwrap();
    let path =
        canban_context::capture(&app.home, &project, card, req, &snapshot, &card_events).unwrap();
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
        json!({"view":"full","card_id":card.id,"body":"@codex task"}),
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
        json!({"view":"full","card_id":card.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let developer = app.snapshot().unwrap().terminals[0].clone();
    assert_eq!(developer.agent.as_ref().unwrap().profile.id, "developer");
    app.call(
        "comment.add",
        json!({"view":"full","card_id":card.id,"body":"@reviewer assess the approach"}),
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
        json!({"view":"full","card_id":card.id,"revision":1,"stage":"develop"}),
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
    let card_events = app.events().unwrap();
    let path = canban_context::capture(
        &app.home,
        &project,
        &snapshot.cards[0],
        &before,
        &snapshot,
        &card_events,
    )
    .unwrap();
    let prompt = std::fs::read_to_string(app.home.join(path)).unwrap();
    let before_agent = before.agent.as_ref().unwrap();
    assert!(prompt.contains(&before_agent.profile.role_instruction));
    assert!(prompt.contains(&before_agent.instruction));
    assert!(!prompt.contains("New role"));
    app.call(
        "comment.add",
        json!({"view":"full","card_id":card.id,"body":"@developer explain"}),
    )
    .unwrap();
    // Input appended to the existing invocation retains its loaded profile.
    assert_eq!(app.snapshot().unwrap().terminals.len(), 1);
    let _run = admit(&app, &before);
    app.call("execution.report",json!({"project_id":project.id,"card":card.number,"profile_id":"developer","state":"completed","request_key":"finish-old-role"})).unwrap();
    app.call(
        "comment.add",
        json!({"view":"full","card_id":card.id,"body":"@developer next assignment"}),
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
fn review_clears_warning_when_herdr_reports_agent_stopped_and_never_blocks_merge() {
    let (dir, app, mut project, mut card) = setup();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let git = |args: &[&str]| {
        let out = canban_exec::raw_command("git")
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
        json!({"view":"full","card_id":card.id,"body":"@codex review"}),
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
        json!({"view":"full","card_id":card.id,"revision":card.revision,"stage":"develop"}),
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
        app.events()
            .unwrap()
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
            json!({"view":"full","project_id":other.id,"title":"Invalid","source_note_id":note.id})
        )
        .is_err()
    );
    assert_eq!(before, app.snapshot().unwrap());
    let card:Card=serde_json::from_value(app.call("card.create",json!({"view":"full","project_id":project.id,"title":"Scoped work","body":"Implement scope","source_note_id":note.id})).unwrap()).unwrap();
    assert_eq!(card.stage, "backlog");
    assert_eq!(card.context_notes, Some(vec![note.id.clone()]));
    assert!(app.snapshot().unwrap().terminals.is_empty());
    let saved: Note =
        serde_json::from_value(app.call("note.get", json!({"note_id":note.id})).unwrap()).unwrap();
    assert_eq!(saved.card_ids, vec![card.id.clone()]);
    app.call(
        "card.context.update",
        json!({"view":"full","card_id":card.id,"revision":card.revision,"note_ids":[]}),
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
fn removed_routing_fields_are_rejected_without_mutating_cards() {
    let (_dir, app, project, card) = setup();
    for tags in [json!([]), json!(["claude"])] {
        let error = app
            .call(
                "card.create",
                json!({"view":"full","project_id":project.id,"title":"Old client","tags":tags}),
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
                json!({"view":"full","card_id":card.id,"revision":1,"title":"Changed","tags":tags}),
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

mod project_bundle_cases;
mod projects_cases;

#[test]
fn explicit_comment_recipients_override_prose_scanning() {
    let (_d, app, _p, c) = setup();
    app.call(
        "comment.add",
        json!({"view":"full","card_id":c.id,"body":"The UI shows Ask @claude", "mentions":[]}),
    )
    .unwrap();
    assert!(app.snapshot().unwrap().terminals.is_empty());
    app.call(
        "comment.add",
        json!({"view":"full","card_id":c.id,"body":"Compare @claude and @codex", "mentions":["codex"]}),
    )
    .unwrap();
    let s = app.snapshot().unwrap();
    assert_eq!(s.terminals.len(), 1);
    assert_eq!(s.terminals[0].agent.as_ref().unwrap().profile.id, "codex");
    assert_eq!(s.questions.len(), 1);
    assert_eq!(
        app.events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "comment.created")
            .count(),
        2
    );
}

#[test]
fn continued_assignment_rejects_new_stale_reports_but_preserves_completion_receipts() {
    let (_d, app, p, c) = setup();
    app.call(
        "card.move",
        json!({"view":"full","card_id":c.id,"revision":1,"stage":"develop"}),
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
    let before_events = app.events().unwrap();
    let home = app.home.clone();
    drop(app);
    let conn = rusqlite::Connection::open(home.join("canban.sqlite")).unwrap();
    conn.execute_batch("DROP TABLE card_reads; DROP TABLE editor_drafts; PRAGMA user_version=7;")
        .unwrap();
    drop(conn);
    let upgraded = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    let after = upgraded.snapshot().unwrap();
    assert_eq!(before.cards, after.cards);
    assert_eq!(before_events, upgraded.events().unwrap());
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
        let status = canban_exec::raw_command("git")
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

    let ctx_lines = app
        .call(
            "git.file_lines",
            json!({
                "project_id": p.id,
                "path": "lib.rs",
                "source": "rev",
                "rev": cmp["compare_sha"],
            }),
        )
        .unwrap();
    assert!(ctx_lines["total_lines"].as_u64().unwrap() >= 1);

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
    assert_eq!(sent["card_record"]["stage"], "develop");
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
    assert_eq!(created["card_record"]["id"], new_cid);
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

mod schedules_cases;

mod multi_host;

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
        json!({"view":"full",
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
            json!({"view":"full",
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

    // 5. Project-level identity override: override `@developer` -> `codex` (with project model/effort)
    // and `@reviewer` model/effort-only override ONLY for `project`, while global `@developer` remains `agy`,
    // and reject direct CLI agent keys in `project.agents`!
    assert!(
        app.call(
            "project.agents",
            json!({
                "project_id": project.id,
                "agent_overrides": {
                    "codex": {"model": "gpt-6.1-sol"}
                }
            }),
        )
        .is_err(),
        "Direct CLI Agents must be rejected in Project-level overrides"
    );
    app.call(
        "project.agents",
        json!({
            "project_id": project.id,
            "agent_overrides": {
                "developer": {
                    "agent": "codex",
                    "model": "gpt-5.4",
                    "effort": "high"
                },
                "reviewer": {
                    "model": "gemini-4-carbon-low",
                    "effort": "low"
                }
            }
        }),
    )
    .unwrap();
    app.call(
        "comment.add",
        json!({"view":"full",
            "card_id": card.id,
            "body": "@developer @reviewer check project override",
            "mentions": ["developer", "reviewer"],
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
    assert_eq!(req.profile.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(req.profile.effort.as_deref(), Some("high"));
    assert!(
        req.profile.role_instruction.contains("development Agent"),
        "Project override must preserve @developer's role instruction"
    );
    let rev_req = snap
        .terminals
        .iter()
        .filter_map(|r| r.agent.as_ref())
        .rfind(|r| r.card_id == card.id && r.profile.id == "reviewer")
        .expect("reviewer request queued");
    assert_eq!(
        rev_req.profile.command()[0],
        "agy",
        "Model/effort-only project override preserves global backing Agent"
    );
    assert_eq!(
        rev_req.profile.model.as_deref(),
        Some("gemini-4-carbon-low")
    );
    assert_eq!(rev_req.profile.effort.as_deref(), Some("low"));
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
            json!({"view":"full","project_id": second_proj.id, "title": "Drag to Develop"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"view":"full","card_id": second_card.id, "revision": second_card.revision, "stage": "develop"}),
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

    let cat_after_agy = app.call("agent.catalog", json!({})).unwrap();
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
    assert!(agy_entry["supports_orchestrator"].as_bool().unwrap());
    assert!(!agy_entry["models"].as_array().unwrap().is_empty());
    let codex_entry = cat_after_agy["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "codex")
        .unwrap();
    assert!(!codex_entry["supports_orchestrator"].as_bool().unwrap());

    // Verify per-model `supported_efforts` parsing for `agy models` and Codex `models_cache.json`
    let parsed_agy = crate::agents::parse_agy_models_output(
        "Fetching available models...\n\
         gemini-4-carbon-medium\tGemini 4 Carbon (Medium)\n\
         gemini-4-carbon-high\tGemini 4 Carbon (High)\n\
         gemini-4-carbon-low\tGemini 4 Carbon (Low)\n\
         gemini-4-hydrogen-high\tGemini 4 Hydrogen (High)\n\
         gemini-4-hydrogen-xhigh\tGemini 4 Hydrogen (XHigh)\n\
         gemini-4-hydrogen-medium\tGemini 4 Hydrogen (Medium)\n\
         gemini-4-hydrogen-low\tGemini 4 Hydrogen (Low)\n\
         gemini-pro-orchestrator\tOrchestrator (Carbon)\n\
         gemini-pro-orchestrator-argon\tOrchestrator (Argon)\n\
         gpt-oss-120b-medium\tGPT-OSS 120B (Medium)\n",
    );
    let find_agy = |id: &str| parsed_agy.iter().find(|m| m.id == id).unwrap();
    assert_eq!(find_agy("orchestrator").supported_efforts, Some(Vec::new()));
    assert!(!parsed_agy.iter().any(|m| m.id == "gemini-pro-orchestrator"));
    assert!(
        !parsed_agy
            .iter()
            .any(|m| m.id == "gemini-pro-orchestrator-argon")
    );
    assert_eq!(
        find_agy("gpt-oss-120b-medium").supported_efforts,
        Some(Vec::new())
    );
    assert_eq!(find_agy("gemini-4-carbon").label, "Gemini 4 Carbon");
    assert_eq!(
        find_agy("gemini-4-carbon").supported_efforts,
        Some(vec!["low".into(), "medium".into(), "high".into()])
    );
    assert_eq!(
        find_agy("gemini-4-carbon")
            .effort_labels
            .get("high")
            .map(String::as_str),
        Some("Gemini 4 Carbon (High)")
    );
    assert_eq!(find_agy("gemini-4-hydrogen").label, "Gemini 4 Hydrogen");
    assert_eq!(
        find_agy("gemini-4-hydrogen").supported_efforts,
        Some(vec![
            "low".into(),
            "medium".into(),
            "high".into(),
            "xhigh".into()
        ])
    );
    assert_eq!(
        find_agy("gemini-4-hydrogen")
            .effort_labels
            .get("xhigh")
            .map(String::as_str),
        Some("Gemini 4 Hydrogen (XHigh)")
    );
    assert!(
        !parsed_agy
            .iter()
            .any(|m| m.id == "gemini-4-carbon-high" || m.id == "gemini-4-hydrogen-high")
    );

    let codex_defaults = canban_profiles::builtin_catalog()
        .into_iter()
        .find(|e| e.id == "codex")
        .unwrap()
        .default_models;
    let parsed_codex = crate::agents::parse_codex_cached_models_json(
        &json!({
            "models": [
                {
                    "slug": "gpt-6.1-sol",
                    "display_name": "GPT-6.1-Sol",
                    "supported_reasoning_levels": [
                        {"effort": "low"},
                        {"effort": "medium"},
                        {"effort": "high"},
                        {"effort": "ultra"}
                    ]
                },
                {
                    "slug": "codex-fixed-fast",
                    "display_name": "Codex Fixed Fast",
                    "supported_reasoning_levels": []
                }
            ]
        }),
        &codex_defaults,
    );
    assert_eq!(
        parsed_codex
            .iter()
            .find(|m| m.id == "gpt-6.1-sol")
            .unwrap()
            .supported_efforts,
        Some(vec![
            "low".into(),
            "medium".into(),
            "high".into(),
            "ultra".into()
        ])
    );
    assert_eq!(
        parsed_codex
            .iter()
            .find(|m| m.id == "codex-fixed-fast")
            .unwrap()
            .supported_efforts,
        Some(Vec::new())
    );

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
    assert_eq!(dev_switched.orchestrator, None);

    // 7b. Dual-role Orchestrator mode on `agy` only (rejected on `codex` / `claude`):
    assert!(
        app.call(
            "config.agents.save",
            json!({
                "op": "set_profile_options",
                "id": "codex",
                "model": "orchestrator",
            }),
        )
        .is_err(),
        "Orchestrator mode must be rejected on non-agy agents"
    );
    assert!(
        app.call(
            "config.agents.save",
            json!({
                "op": "set_profile_options",
                "id": "developer",
                "orchestrator": { "planner_model": "gemini-4-neon" },
            }),
        )
        .is_err(),
        "Orchestrator config must be rejected on codex-backed identity"
    );
    assert!(
        app.call(
            "project.agents",
            json!({
                "project_id": project.id,
                "agent_overrides": {
                    "developer": {
                        "agent": "codex",
                        "model": "orchestrator"
                    }
                }
            }),
        )
        .is_err(),
        "Project orchestrator override must be rejected for codex"
    );

    // Configure `@agy` in Orchestrator mode and propagate to `@reviewer` (backed by `agy`),
    // while `@planner` overrides only `worker_effort` and Project override updates `developer` to agy orchestrator.
    app.call(
        "config.agents.save",
        json!({
            "op": "set_profile_options",
            "id": "agy",
            "model": "orchestrator",
            "orchestrator": {
                "planner_model": "claude-opus-5-5",
                "planner_effort": "high",
                "worker_model": "gemini-4-hydrogen",
                "worker_effort": "medium"
            }
        }),
    )
    .unwrap();
    app.call(
        "config.agents.save",
        json!({
            "op": "set_profile_options",
            "id": "reviewer",
            "model": null,
            "effort": null,
        }),
    )
    .unwrap();
    app.call(
        "config.agents.save",
        json!({
            "op": "upsert_identity",
            "id": "planner",
            "engine": "agy",
            "orchestrator": { "worker_effort": "low" },
            "enabled": true,
        }),
    )
    .unwrap();
    let cfg_orch = app.config();
    let rev_orch = cfg_orch
        .profiles
        .iter()
        .find(|p| p.id == "reviewer")
        .unwrap();
    assert_eq!(rev_orch.model.as_deref(), Some("orchestrator"));
    assert_eq!(rev_orch.effort, None);
    assert_eq!(
        rev_orch
            .orchestrator
            .as_ref()
            .and_then(|o| o.planner_model.as_deref()),
        Some("claude-opus-5-5")
    );
    let planner_orch = cfg_orch
        .profiles
        .iter()
        .find(|p| p.id == "planner")
        .unwrap();
    assert_eq!(
        planner_orch
            .orchestrator
            .as_ref()
            .and_then(|o| o.worker_effort.as_deref()),
        Some("low")
    );
    assert_eq!(
        planner_orch
            .orchestrator
            .as_ref()
            .and_then(|o| o.planner_model.as_deref()),
        Some("claude-opus-5-5")
    );
    // Project override with agy orchestrator succeeds
    app.call(
        "project.agents",
        json!({
            "project_id": project.id,
            "agent_overrides": {
                "developer": {
                    "agent": "agy",
                    "model": "orchestrator",
                    "orchestrator": {
                        "planner_model": "gemini-4-neon",
                        "planner_effort": "xhigh",
                        "worker_model": "claude-sonnet-5-5",
                        "worker_effort": "medium"
                    }
                }
            }
        }),
    )
    .unwrap();
    // Switching `@agy` back to a single model clears `orchestrator` cleanly
    app.call(
        "config.agents.save",
        json!({
            "op": "set_profile_options",
            "id": "agy",
            "model": "gemini-4-carbon",
            "effort": "medium",
        }),
    )
    .unwrap();
    let agy_single = app
        .config()
        .profiles
        .into_iter()
        .find(|p| p.id == "agy")
        .unwrap();
    assert_eq!(agy_single.model.as_deref(), Some("gemini-4-carbon"));
    assert_eq!(agy_single.orchestrator, None);
    // Switching `@planner` to `codex` clears its orchestrator override
    app.call(
        "config.agents.save",
        json!({
            "op": "upsert_identity",
            "id": "planner",
            "engine": "codex",
            "enabled": true,
        }),
    )
    .unwrap();
    let planner_codex = app
        .config()
        .profiles
        .into_iter()
        .find(|p| p.id == "planner")
        .unwrap();
    assert_eq!(planner_codex.orchestrator, None);

    // 8. Customizing an enabled Agent's Command + Argv (`set_profile_options` `argv`):
    //    - Saving custom `argv` on `@agy` overrides `argv[0]` (`/bin/agy-modified`) and args,
    //      synchronizes inherited `resume_argv[0]` to `/bin/agy-modified`, propagates to
    //      `@reviewer` (backed by `agy`) without rewriting `@reviewer.argv` in raw JSON,
    //      and survives re-running `agent.setup`.
    //    - Saving `argv` identical to preset defaults or `argv: null` clears raw `argv`
    //      and restores `agy` for both launch and resume.
    assert!(
        app.call(
            "config.agents.save",
            json!({
                "op": "set_profile_options",
                "id": "agy",
                "argv": [""],
            }),
        )
        .is_err(),
        "Empty executable in argv[0] must be rejected"
    );
    app.call(
        "config.agents.save",
        json!({
            "op": "set_profile_options",
            "id": "agy",
            "argv": [
                "/bin/agy-modified",
                "--custom-flag",
                "{cwd}",
                "--settings",
                "{agy_trusted_settings}",
                "-i",
                "{prompt}"
            ],
        }),
    )
    .unwrap();
    let cfg_custom_cmd = app.config();
    let agy_custom = cfg_custom_cmd
        .profiles
        .iter()
        .find(|p| p.id == "agy")
        .unwrap();
    assert_eq!(agy_custom.command()[0], "/bin/agy-modified");
    assert_eq!(
        agy_custom.resume_command()[0],
        "/bin/agy-modified",
        "Inherited resume_argv[0] must sync to custom Command"
    );
    let rev_custom = cfg_custom_cmd
        .profiles
        .iter()
        .find(|p| p.id == "reviewer")
        .unwrap();
    assert_eq!(
        rev_custom.command()[0],
        "/bin/agy-modified",
        "@reviewer backed by agy must inherit @agy's customized Command"
    );
    assert_eq!(rev_custom.resume_command()[0], "/bin/agy-modified");

    let cat_custom = app.call("agent.catalog", json!({})).unwrap();
    let raw_agy = cat_custom["raw_profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "agy")
        .unwrap();
    assert_eq!(raw_agy["argv"][0], "/bin/agy-modified");
    let raw_rev = cat_custom["raw_profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "reviewer")
        .unwrap();
    assert!(
        raw_rev.get("argv").is_none() || raw_rev["argv"].is_null(),
        "Identity raw profile must not duplicate peer Agent argv"
    );
    let agy_cat_row = cat_custom["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "agy")
        .unwrap();
    assert_eq!(agy_cat_row["binary"], "/bin/agy-modified");
    assert_eq!(agy_cat_row["default_argv"][0], "agy");

    // Re-running `agent.setup` preserves direct Agent's custom `argv`
    app.call(
        "agent.setup",
        json!({
            "enabled_agents": ["codex", "agy"],
            "default_engine": "agy",
        }),
    )
    .unwrap();
    let cfg_after_rerun = app.config();
    assert_eq!(
        cfg_after_rerun
            .profiles
            .iter()
            .find(|p| p.id == "agy")
            .unwrap()
            .command()[0],
        "/bin/agy-modified"
    );
    assert_eq!(
        cfg_after_rerun
            .profiles
            .iter()
            .find(|p| p.id == "developer")
            .unwrap()
            .command()[0],
        "/bin/agy-modified"
    );

    // Resetting `argv: null` clears raw `argv` and restores preset `agy`
    app.call(
        "config.agents.save",
        json!({
            "op": "set_profile_options",
            "id": "agy",
            "argv": serde_json::Value::Null,
        }),
    )
    .unwrap();
    let cfg_reset_cmd = app.config();
    let agy_reset = cfg_reset_cmd
        .profiles
        .iter()
        .find(|p| p.id == "agy")
        .unwrap();
    assert_eq!(agy_reset.command()[0], "agy");
    assert_eq!(agy_reset.resume_command()[0], "agy");
    let cat_reset = app.call("agent.catalog", json!({})).unwrap();
    let raw_agy_reset = cat_reset["raw_profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "agy")
        .unwrap();
    assert!(
        raw_agy_reset.get("argv").is_none() || raw_agy_reset["argv"].is_null(),
        "Resetting argv must clear raw argv override"
    );
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

    // 1. Comment-triggered attempt uses `ExecutionTrigger::Comment` and `reporting_mention.txt`
    // without rendering a separate `## Pending questions` / `reply --to` instruction.
    let comment_ev: Event = serde_json::from_value(
        app.call(
            "comment.add",
            json!({"view":"full","card_id": card.id, "body": "@developer What should we name the merger role?"}),
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
        let evs = app.events().unwrap();
        let rel = canban_context::capture(&app.home, &project, card, att, &s, &evs).unwrap();
        std::fs::read_to_string(app.home.join(rel)).unwrap()
    };

    let comment_attempt = admit(&app, &comment_req);
    let comment_prompt = capture_prompt(&card, &comment_attempt);
    assert!(
        comment_prompt.contains(
            "You were mentioned (@developer) on this Card. Read the Card context and respond to the user's message below."
        ) && comment_prompt.contains("## Behavior")
            && comment_prompt.contains("## Commands")
            && comment_prompt.contains("Finish this `@mention` in one report (no separate `reply`).")
            && comment_prompt.contains("\"$CANBAN_CLI\" card update")
            && comment_prompt.contains("\"$CANBAN_CLI\" card adopt")
            && comment_prompt.contains("\"$CANBAN_CLI\" report --state")
            && !comment_prompt.contains("CANBAN_ENDPOINT=")
            && !comment_prompt.contains("CANBAN_PROJECT=")
            && !comment_prompt.contains("CANBAN_CARD="),
        "comment prompt should use reporting_mention.txt via $CANBAN_CLI without inline env vars"
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
    let evs = app.events().unwrap();
    assert_eq!(
        evs.iter()
            .filter(|e| e.data["terminal_id"].as_str() == Some(&comment_req.id)
                && e.kind == "comment.replied")
            .count(),
        1
    );
    let reported_ev = evs
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
            json!({"view":"full","project_id": project.id, "title": "Interactive Card", "body": "Discuss"}),
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
        interactive_prompt.contains("## Behavior")
            && interactive_prompt.contains("## Commands")
            && interactive_prompt.contains(
                "Interactive discussion: reply in the terminal during discussion; report only when a requested task finishes."
            )
            && interactive_prompt.contains("\"$CANBAN_CLI\" card update")
            && interactive_prompt.contains("\"$CANBAN_CLI\" card adopt")
            && interactive_prompt.contains("\"$CANBAN_CLI\" report --state")
            && !interactive_prompt.contains("CANBAN_ENDPOINT=")
            && !interactive_prompt.contains("CANBAN_PROJECT=")
            && !interactive_prompt.contains("CANBAN_CARD="),
        "interactive prompt should use reporting_interactive.txt via $CANBAN_CLI without inline env vars"
    );

    // 3. Workflow/stage task with an injected pending question DOES include `## Pending questions`.
    let c3: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": project.id, "title": "Task Card", "body": "Build"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"view":"full","card_id": c3.id, "revision": c3.revision, "stage": "develop"}),
    )
    .unwrap();
    app.call(
        "comment.add",
        json!({"view":"full","card_id": c3.id, "body": "@developer Please also check Windows paths"}),
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
        task_prompt.contains("## Behavior")
            && task_prompt.contains("## Commands")
            && task_prompt.contains("- Report outcome:")
            && task_prompt.contains("\"$CANBAN_CLI\" report --state")
            && task_prompt.contains("## Pending questions")
            && task_prompt.contains("\"$CANBAN_CLI\" reply --to")
            && !task_prompt.contains("CANBAN_ENDPOINT=")
            && !task_prompt.contains("CANBAN_PROJECT=")
            && !task_prompt.contains("CANBAN_CARD="),
        "non-comment task with a concurrent question retains `## Pending questions` + `\"$CANBAN_CLI\" reply --to` without inline env vars"
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

#[test]
fn herdr_check_and_install_support_isolated_missing_failure_and_ready_progression() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let bin_dir = dir.path().join("isolated-bin");
    std::fs::create_dir_all(home.join("config")).unwrap();
    std::fs::create_dir_all(&bin_dir).unwrap();

    let fail_script = dir.path().join("install-fail.sh");
    std::fs::write(
        &fail_script,
        "#!/bin/sh\necho 'network unreachable' >&2\nexit 22\n",
    )
    .unwrap();

    let ok_script = dir.path().join("install-ok.sh");
    let mock_hc = r#"#!/bin/sh
if [ "$1" = "connect" ]; then
  while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
    [ -z "$id" ] && id=1
    printf '{"jsonrpc":"2.0","id":%s,"result":{"core":{"id":"isolated-core-54","version":"0.1.99"}}}\n' "$id"
  done
  exit 0
fi
exit 0
"#;
    std::fs::write(
        &ok_script,
        format!(
            "#!/bin/sh\nset -eu\necho 'detected macos/aarch64'\ncat > \"$HC_INSTALL_DIR/hc\" <<'EOF'\n{mock_hc}EOF\nchmod 755 \"$HC_INSTALL_DIR/hc\"\necho 'installed hc 0.1.99'\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ok_script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut cfg = Config::default();
    cfg.hc_env.insert(
        "CANBAN_HERDR_SEARCH_PATHS".into(),
        bin_dir.display().to_string(),
    );
    cfg.hc_env
        .insert("HC_INSTALL_DIR".into(), bin_dir.display().to_string());
    cfg.hc_env.insert(
        "CANBAN_HERDR_INSTALL_URL".into(),
        format!("file://{}", fail_script.display()),
    );
    canban_context::atomic(
        &home.join("config/agents.json"),
        &serde_json::to_vec_pretty(&cfg).unwrap(),
    )
    .unwrap();

    let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let _running = app.start().unwrap();

    // 1. Missing binary detection.
    let missing = app.call("herdr.check", json!({})).unwrap();
    assert_eq!(missing["installed"], false);
    assert_eq!(missing["ready"], false);
    assert_eq!(missing["default_hc"], true);
    assert!(missing["error"].as_str().unwrap().contains("not found"));

    // 2. Agent-bound invocations cannot trigger host installation.
    assert!(
        app.call("herdr.install", json!({"execution": {"attempt_id": "a1"}}))
            .is_err()
    );

    // 3. Failed installer surfaces logs + failure state and stays not-ready.
    let failed_check = app.call("herdr.install", json!({"wait": true})).unwrap();
    assert_eq!(failed_check["ready"], false);
    assert_eq!(failed_check["install"]["phase"], "failed");
    assert!(
        failed_check["install"]["error"]
            .as_str()
            .unwrap()
            .contains("network unreachable")
    );

    // 4. Point to working isolated installer and install -> automatically verifies & updates config.hc.
    cfg.hc_env.insert(
        "CANBAN_HERDR_INSTALL_URL".into(),
        format!("file://{}", ok_script.display()),
    );
    canban_context::atomic(
        &home.join("config/agents.json"),
        &serde_json::to_vec_pretty(&cfg).unwrap(),
    )
    .unwrap();

    let ready_check = app.call("herdr.install", json!({"wait": true})).unwrap();
    assert_eq!(ready_check["installed"], true);
    assert_eq!(ready_check["ready"], true);
    assert_eq!(ready_check["core_id"], "isolated-core-54");
    assert_eq!(ready_check["version"], "0.1.99");
    assert_eq!(ready_check["install"]["phase"], "succeeded");
    assert_eq!(app.config().hc, bin_dir.join("hc").display().to_string());

    // 5. External manual removal & re-install verified via `herdr.check`.
    std::fs::remove_file(bin_dir.join("hc")).unwrap();
    let recheck_missing = app.call("herdr.check", json!({})).unwrap();
    assert_eq!(recheck_missing["installed"], false);
    assert_eq!(recheck_missing["ready"], false);

    std::fs::write(bin_dir.join("hc"), mock_hc).unwrap();
    std::fs::set_permissions(bin_dir.join("hc"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let recheck_ready = app.call("herdr.check", json!({})).unwrap();
    assert_eq!(recheck_ready["installed"], true);
    assert_eq!(recheck_ready["ready"], true);
    assert_eq!(recheck_ready["core_id"], "isolated-core-54");
}

#[test]
fn comment_update_edits_own_local_comment_preserves_mentions_and_rejects_non_user_events() {
    let (_d, app, p, c) = setup();
    app.call(
        "comment.add",
        json!({"view":"full","card_id": c.id, "body": "Initial thought for @codex", "mentions": ["codex"]}),
    )
    .unwrap();
    let snap1 = app.snapshot().unwrap();
    let evs1 = app.events().unwrap();
    let created = evs1
        .iter()
        .find(|e| e.kind == "comment.created")
        .unwrap()
        .clone();
    assert_eq!(snap1.questions.len(), 1);
    assert_eq!(snap1.questions[0].profile_id, "codex");
    assert_eq!(snap1.questions[0].body, "Initial thought for @codex");

    // Edit own comment: preserves sequence, updates message + pending Question, adds new mention without duplicating existing Question.
    let updated: Event = serde_json::from_value(
        app.call(
            "comment.update",
            json!({"view":"full",
                "card_id": c.id,
                "comment_id": created.id,
                "body": "Refined thought for @codex and @reviewer — see https://github.com/a/b/issues/9",
                "mentions": ["codex", "reviewer"],
                "request_key": "edit-local-1",
            }),
        )
        .unwrap(),
    )
    .unwrap();
    // Idempotent replay with same request_key does not bump revision twice.
    app.call(
        "comment.update",
        json!({"view":"full",
            "card_id": c.id,
            "comment_id": created.id,
            "body": "Refined thought for @codex and @reviewer — see https://github.com/a/b/issues/9",
            "mentions": ["codex", "reviewer"],
            "request_key": "edit-local-1",
        }),
    )
    .unwrap();

    assert_eq!(updated.id, created.id);
    assert_eq!(updated.sequence, created.sequence);
    assert_eq!(updated.data["revision"], 2);
    assert!(updated.data["edited_at"].as_u64().is_some());
    assert!(updated.message.contains("Refined thought"));

    let snap2 = app.snapshot().unwrap();
    let evs2 = app.events().unwrap();
    let stored = evs2.iter().find(|e| e.id == created.id).unwrap();
    assert_eq!(stored.sequence, created.sequence);
    assert_eq!(stored.data["revision"], 2);
    assert!(evs2.iter().any(|e| e.kind == "comment.updated"));

    // Existing @codex Question was updated in-place (not duplicated); new @reviewer Question was added.
    let codex_qs: Vec<_> = snap2
        .questions
        .iter()
        .filter(|q| q.comment_id == created.id && q.profile_id == "codex")
        .collect();
    assert_eq!(codex_qs.len(), 1);
    assert!(codex_qs[0].body.contains("Refined thought"));
    let reviewer_qs: Vec<_> = snap2
        .questions
        .iter()
        .filter(|q| q.comment_id == created.id && q.profile_id == "reviewer")
        .collect();
    assert_eq!(reviewer_qs.len(), 1);

    // Reject empty body.
    assert!(
        app.call(
            "comment.update",
            json!({"view":"full","card_id": c.id, "comment_id": created.id, "body": "   "}),
        )
        .is_err()
    );

    // Reject editing an agent reply (`actor != "user"`).
    let _run = admit(&app, &snap2.terminals[0]);
    app.call(
        "execution.report",
        json!({
            "project_id": p.id,
            "card": c.number,
            "profile_id": "codex",
            "state": "completed",
            "message": "Agent reply",
            "body": "Detailed agent response",
            "request_key": "agent-rep-1",
        }),
    )
    .unwrap();
    let evs3 = app.events().unwrap();
    let agent_reply = evs3.iter().find(|e| e.kind == "comment.replied").unwrap();
    assert!(
        app.call(
            "comment.update",
            json!({"view":"full","card_id": c.id, "comment_id": agent_reply.id, "body": "Tamper"}),
        )
        .is_err()
    );

    // Reject editing on an archived Card.
    let (_d2, app2, _p2, c2) = setup();
    let added2: Event = serde_json::from_value(
        app2.call(
            "comment.add",
            json!({"view":"full","card_id": c2.id, "body": "Before archive"}),
        )
        .unwrap(),
    )
    .unwrap();
    let rev2 = app2.snapshot().unwrap().cards[0].revision;
    app2.call("card.archive", json!({"card_id": c2.id, "revision": rev2}))
        .unwrap();
    assert!(
        app2.call(
            "comment.update",
            json!({"view":"full","card_id": c2.id, "comment_id": added2.id, "body": "Edit while archived"}),
        )
        .is_err()
    );
}

#[test]
fn comment_delete_tombstones_own_comment_cancels_pending_questions_and_preserves_running_agents() {
    let (_d, app, p, c) = setup();
    let comment1: Event = serde_json::from_value(
        app.call(
            "comment.add",
            json!({"view":"full",
                "card_id": c.id,
                "body": "Queued question for @codex and https://github.com/a/b/issues/55",
                "mentions": ["codex"],
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let snap1 = app.snapshot().unwrap();
    assert_eq!(snap1.questions.len(), 1);
    assert_eq!(snap1.questions[0].state, "pending");
    assert_eq!(snap1.terminals[0].agent.as_ref().unwrap().state, "queued");

    // Delete own comment: tombstones event in place, cancels pending Question and its still-queued AgentRun.
    let del_res = app
        .call(
            "comment.delete",
            json!({
                "card_id": c.id,
                "comment_id": comment1.id,
                "request_key": "del-local-1",
            }),
        )
        .unwrap();
    assert_eq!(del_res["deleted"], true);
    assert_eq!(del_res["comment_id"], comment1.id);

    // Idempotent replay with same request_key returns cached result without failing.
    let replay = app
        .call(
            "comment.delete",
            json!({
                "card_id": c.id,
                "comment_id": comment1.id,
                "request_key": "del-local-1",
            }),
        )
        .unwrap();
    assert_eq!(replay, del_res);

    let snap2 = app.snapshot().unwrap();
    let evs2 = app.events().unwrap();
    let tombstoned = evs2
        .iter()
        .find(|e| e.id == comment1.id)
        .expect("tombstoned event keeps sequence slot");
    assert_eq!(tombstoned.sequence, comment1.sequence);
    assert_eq!(tombstoned.kind, "comment.deleted");
    assert!(!tombstoned.message.contains("issues/55"));
    assert_eq!(snap2.questions[0].state, "cancelled");
    assert_eq!(
        snap2.terminals[0].agent.as_ref().unwrap().state,
        "cancelled"
    );

    // Deleting an already-deleted comment with a new request_key fails cleanly.
    assert!(
        app.call(
            "comment.delete",
            json!({"card_id": c.id, "comment_id": comment1.id}),
        )
        .is_err()
    );

    // If an Agent is already running when a follow-up @mention comment is deleted,
    // the pending Question is cancelled while the running Agent and its later reply survive.
    let comment2: Event = serde_json::from_value(
        app.call(
            "comment.add",
            json!({"view":"full","card_id": c.id, "body": "Start @reviewer", "mentions": ["reviewer"]}),
        )
        .unwrap(),
    )
    .unwrap();
    let snap3 = app.snapshot().unwrap();
    let rev_entry = snap3
        .terminals
        .iter()
        .find(|t| t.agent.as_ref().is_some_and(|a| a.profile.id == "reviewer"))
        .unwrap();
    let _running = admit(&app, rev_entry);
    let comment3: Event = serde_json::from_value(
        app.call(
            "comment.add",
            json!({"view":"full","card_id": c.id, "body": "Accidental follow-up @reviewer", "mentions": ["reviewer"]}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "comment.delete",
        json!({"card_id": c.id, "comment_id": comment3.id}),
    )
    .unwrap();
    let snap4 = app.snapshot().unwrap();
    let active_rev = snap4
        .terminals
        .iter()
        .find(|t| t.id == rev_entry.id)
        .unwrap()
        .agent
        .as_ref()
        .unwrap();
    assert_eq!(active_rev.state, "running");
    assert_eq!(
        snap4
            .questions
            .iter()
            .find(|q| q.comment_id == comment3.id)
            .unwrap()
            .state,
        "cancelled"
    );
    assert_eq!(
        snap4
            .questions
            .iter()
            .find(|q| q.comment_id == comment2.id)
            .unwrap()
            .state,
        "pending"
    );

    // Agent replies (`comment.replied`) cannot be deleted by `comment.delete`.
    app.call(
        "execution.report",
        json!({
            "project_id": p.id,
            "card": c.number,
            "profile_id": "reviewer",
            "state": "completed",
            "message": "Done",
            "body": "Agent reply body",
            "request_key": "rev-rep-del-test",
        }),
    )
    .unwrap();
    let evs5 = app.events().unwrap();
    let reply = evs5.iter().find(|e| e.kind == "comment.replied").unwrap();
    assert!(
        app.call(
            "comment.delete",
            json!({"card_id": c.id, "comment_id": reply.id}),
        )
        .is_err()
    );

    // Archived Cards reject `comment.delete`.
    let (_d2, app2, _p2, c2) = setup();
    let comment_arch: Event = serde_json::from_value(
        app2.call(
            "comment.add",
            json!({"view":"full","card_id": c2.id, "body": "Before archive"}),
        )
        .unwrap(),
    )
    .unwrap();
    let rev2 = app2.snapshot().unwrap().cards[0].revision;
    app2.call("card.archive", json!({"card_id": c2.id, "revision": rev2}))
        .unwrap();
    assert!(
        app2.call(
            "comment.delete",
            json!({"card_id": c2.id, "comment_id": comment_arch.id}),
        )
        .is_err()
    );
}

#[test]
fn commenting_editing_deleting_and_clearing_notices_do_not_conflict_with_card_move_or_update() {
    let (_d, app, _p, card) = setup();
    let initial_rev = card.revision;

    // 1. Add a comment (including an @mention) and immediately move using the pre-comment revision.
    let added = app
        .call(
            "comment.add",
            json!({
                "view": "full",
                "card_id": card.id,
                "body": "Notes before starting @codex",
                "mentions": ["codex"],
            }),
        )
        .unwrap();
    assert_eq!(added["card_revision"], initial_rev);
    let comment_id = added["id"].as_str().unwrap().to_owned();

    let moved = app
        .call(
            "card.move",
            json!({
                "card_id": card.id,
                "revision": initial_rev,
                "stage": "develop",
            }),
        )
        .unwrap();
    let develop_rev = moved["revision"].as_u64().unwrap();
    assert_eq!(develop_rev, initial_rev + 1);

    // 2. Edit and delete comments, and clear notices without card-level attention,
    // then update Card title/body using `develop_rev` without revision_conflict.
    let updated_comment = app
        .call(
            "comment.update",
            json!({
                "card_id": card.id,
                "comment_id": comment_id,
                "body": "Refined notes for @codex",
                "mentions": ["codex"],
            }),
        )
        .unwrap();
    assert_eq!(updated_comment["card_revision"], develop_rev);

    let deleted_comment = app
        .call(
            "comment.delete",
            json!({
                "card_id": card.id,
                "comment_id": comment_id,
            }),
        )
        .unwrap();
    assert_eq!(deleted_comment["revision"], develop_rev);

    app.call("card.attention.clear", json!({"card_id": card.id}))
        .unwrap();

    let updated_card = app
        .call(
            "card.update",
            json!({
                "card_id": card.id,
                "revision": develop_rev,
                "title": "Updated without conflict",
                "body": "Updated body",
            }),
        )
        .unwrap();
    assert_eq!(updated_card["revision"], develop_rev + 1);
}

#[test]
fn git_client_rpcs_enforce_checkout_containment_user_guard_and_remote_sync() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let remote = dir.path().join("origin.git");
    let source = dir.path().join("source");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&outside).unwrap();

    let git = |cwd: &Path, args: &[&str]| {
        let st = canban_exec::raw_command("git")
            .current_dir(cwd)
            .args(args)
            .status()
            .unwrap();
        assert!(st.success());
    };
    git(&remote, &["init", "--bare", "-b", "main"]);
    git(&source, &["init", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Reviewer"),
        ("user.email", "reviewer@example.invalid"),
        ("core.hooksPath", "/dev/null"),
        ("commit.gpgsign", "false"),
    ] {
        git(&source, &["config", k, v]);
    }
    git(
        &source,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    std::fs::write(source.join("README.md"), "# Demo\n").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-m", "initial"]);
    git(&source, &["push", "-u", "origin", "main"]);

    let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Demo", "directory": source, "target_branch": "main"}),
        )
        .unwrap(),
    )
    .unwrap();

    // Outside directory must be rejected.
    let bad_cwd = app.call("git.refs", json!({"project_id": p.id, "cwd": outside}));
    assert!(bad_cwd.is_err());

    // Agent execution context must be rejected on mutating git.* RPCs.
    let exec_err = app.call(
        "git.stage",
        json!({"project_id": p.id, "all": true, "execution": {"request_id": "r1"}}),
    );
    assert!(exec_err.is_err());

    // Create standalone managed worktree and verify it appears in git.refs checkouts.
    let wt_res = app
        .call(
            "worktree.create",
            json!({"project_id": p.id, "branch": "feat/git-ui", "launch": "none"}),
        )
        .unwrap();
    let wt_path = wt_res["worktree"]["path"].as_str().unwrap().to_owned();

    let refs = app
        .call("git.refs", json!({"project_id": p.id, "cwd": wt_path}))
        .unwrap();
    assert_eq!(refs["current_branch"], "feat/git-ui");
    assert_eq!(refs["checkouts"].as_array().unwrap().len(), 2);

    // Externally created Git worktree (outside Canban's DB) must also be discovered
    // in `git.refs` checkouts and accepted by `git.refs` / `git.compare`.
    let ext_wt = Path::new(&source).join(".worktrees/ext-branch");
    git(
        Path::new(&source),
        &[
            "worktree",
            "add",
            "-b",
            "fix/external-wt",
            ext_wt.to_str().unwrap(),
        ],
    );
    let ext_wt_str = ext_wt.to_string_lossy().into_owned();
    std::fs::write(ext_wt.join("ext.rs"), "pub fn external() {}\n").unwrap();
    let refs_with_ext = app.call("git.refs", json!({"project_id": p.id})).unwrap();
    assert_eq!(refs_with_ext["checkouts"].as_array().unwrap().len(), 3);
    assert!(
        refs_with_ext["checkouts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["branch"] == "fix/external-wt"),
        "expected external worktree in git.refs checkouts: {refs_with_ext}"
    );
    let ext_refs = app
        .call("git.refs", json!({"project_id": p.id, "cwd": ext_wt_str}))
        .unwrap();
    assert_eq!(ext_refs["current_branch"], "fix/external-wt");
    assert_eq!(ext_refs["dirty"], true);
    // Unrelated outside directory must remain rejected even with external worktree fallback.
    assert!(
        app.call("git.refs", json!({"project_id": p.id, "cwd": outside}))
            .is_err()
    );

    // Stage, commit + push on managed worktree via RPCs.
    std::fs::write(Path::new(&wt_path).join("client.rs"), "pub fn sync() {}\n").unwrap();
    app.call(
        "git.stage",
        json!({"project_id": p.id, "cwd": wt_path, "paths": ["client.rs"], "request_key": "stg-1"}),
    )
    .unwrap();

    let commit_res = app
        .call(
            "git.commit",
            json!({
                "project_id": p.id,
                "cwd": wt_path,
                "message": "feat: add git client rpc\n\nverified in worktree",
                "push": true,
                "request_key": "cmt-1"
            }),
        )
        .unwrap();
    assert_eq!(commit_res["pushed"], true);
    let sha = commit_res["sha"].as_str().unwrap().to_owned();

    let log_res = app
        .call(
            "git.log",
            json!({"project_id": p.id, "cwd": wt_path, "rev": "feat/git-ui", "limit": 10}),
        )
        .unwrap();
    assert_eq!(log_res["commits"].as_array().unwrap().len(), 2);

    let detail = app
        .call(
            "git.commit_detail",
            json!({"project_id": p.id, "cwd": wt_path, "sha": sha}),
        )
        .unwrap();
    assert_eq!(
        detail["commit_detail"]["subject"],
        "feat: add git client rpc"
    );
    assert_eq!(detail["commit_detail"]["body"], "verified in worktree");
    assert_eq!(detail["files"][0]["filename"], "client.rs");

    // Fetch from main checkout and verify feat/git-ui was pushed to origin.
    let fetch_res = app
        .call(
            "git.fetch",
            json!({"project_id": p.id, "request_key": "fetch-1"}),
        )
        .unwrap();
    assert_eq!(fetch_res["remote"], "origin");
}

#[test]
fn git_sync_rpc_pulls_pushes_replays_idempotently_and_rejects_invalid_requests() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let remote = dir.path().join("origin.git");
    let source = dir.path().join("repo");
    let peer = dir.path().join("peer");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&outside).unwrap();

    let git = |cwd: &Path, args: &[&str]| {
        let st = canban_exec::raw_command("git")
            .current_dir(cwd)
            .args(args)
            .status()
            .unwrap();
        assert!(st.success());
    };
    let git_out = |cwd: &Path, args: &[&str]| -> String {
        let out = canban_exec::raw_command("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };

    git(&remote, &["init", "--bare", "-b", "main"]);
    git(&source, &["init", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Test"),
        ("user.email", "test@example.invalid"),
        ("core.hooksPath", "/dev/null"),
        ("commit.gpgsign", "false"),
    ] {
        git(&source, &["config", k, v]);
    }
    git(
        &source,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    std::fs::write(source.join("README.md"), "seed\n").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-m", "seed"]);
    git(&source, &["push", "-u", "origin", "main"]);

    git(
        dir.path(),
        &["clone", remote.to_str().unwrap(), peer.to_str().unwrap()],
    );
    for (k, v) in [
        ("user.name", "Peer"),
        ("user.email", "peer@example.invalid"),
        ("core.hooksPath", "/dev/null"),
        ("commit.gpgsign", "false"),
    ] {
        git(&peer, &["config", k, v]);
    }

    let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Sync RPC Demo", "directory": source, "target_branch": "main"}),
        )
        .unwrap(),
    )
    .unwrap();

    // Agent execution context and outside directories must be rejected.
    assert!(
        app.call(
            "git.sync",
            json!({"project_id": p.id, "execution": {"request_id": "r1"}}),
        )
        .is_err()
    );
    assert!(
        app.call("git.sync", json!({"project_id": p.id, "cwd": outside}))
            .is_err()
    );

    // Managed worktree commit -> git.sync publishes & pushes, and replays idempotently.
    let wt_res = app
        .call(
            "worktree.create",
            json!({"project_id": p.id, "branch": "feat/rpc-sync", "launch": "none"}),
        )
        .unwrap();
    let wt_path = wt_res["worktree"]["path"].as_str().unwrap().to_owned();
    for (k, v) in [
        ("user.name", "Test"),
        ("user.email", "test@example.invalid"),
        ("core.hooksPath", "/dev/null"),
        ("commit.gpgsign", "false"),
    ] {
        git(Path::new(&wt_path), &["config", k, v]);
    }
    std::fs::write(Path::new(&wt_path).join("sync_rpc.rs"), "pub fn ok() {}\n").unwrap();
    app.call(
        "git.stage",
        json!({"project_id": p.id, "cwd": wt_path, "paths": ["sync_rpc.rs"]}),
    )
    .unwrap();
    app.call(
        "git.commit",
        json!({"project_id": p.id, "cwd": wt_path, "message": "feat: rpc sync"}),
    )
    .unwrap();

    let sync_wt = app
        .call(
            "git.sync",
            json!({"project_id": p.id, "cwd": wt_path, "request_key": "sync-wt-1"}),
        )
        .unwrap();
    assert_eq!(sync_wt["pushed"], true);
    assert_eq!(sync_wt["set_upstream"], true);
    assert_eq!(
        git_out(Path::new(&wt_path), &["rev-parse", "HEAD"]),
        git_out(&remote, &["rev-parse", "refs/heads/feat/rpc-sync"])
    );

    // Same request_key replays the saved receipt; mismatched payload rejects.
    let replay = app
        .call(
            "git.sync",
            json!({"project_id": p.id, "cwd": wt_path, "request_key": "sync-wt-1"}),
        )
        .unwrap();
    assert_eq!(replay, sync_wt);
    assert!(
        app.call(
            "git.sync",
            json!({"project_id": p.id, "request_key": "sync-wt-1"}),
        )
        .is_err()
    );

    // Peer commit on main -> git.sync on Project root fast-forwards cleanly.
    std::fs::write(peer.join("peer_update.txt"), "from peer\n").unwrap();
    git(&peer, &["add", "."]);
    git(&peer, &["commit", "-m", "feat: peer main update"]);
    git(&peer, &["push", "origin", "main"]);

    let sync_root = app
        .call(
            "git.sync",
            json!({"project_id": p.id, "request_key": "sync-root-1"}),
        )
        .unwrap();
    assert_eq!(sync_root["fast_forwarded"], true);
    assert_eq!(sync_root["pulled_commits"], 1);
    assert_eq!(sync_root["pushed"], false);
    assert_eq!(
        std::fs::read_to_string(source.join("peer_update.txt")).unwrap(),
        "from peer\n"
    );
}

#[test]
fn short_ids_card_number_paths_and_legacy_alias_resolution() {
    let (_d, app, p, c) = setup();
    assert_eq!(p.id.len(), 8);
    assert!(p.id.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(c.id.len(), 8);
    assert!(c.id.bytes().all(|b| b.is_ascii_hexdigit()));

    // Move to develop, admit, add a comment to create a Question, then report & reply.
    app.call(
        "card.move",
        json!({"view":"full","card_id":c.id,"revision":c.revision,"stage":"develop"}),
    )
    .unwrap();
    let entry = app
        .snapshot()
        .unwrap()
        .terminals
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(entry.agent.as_ref().unwrap().card_number, c.number);
    let _running = admit(&app, &entry);
    app.call(
        "comment.add",
        json!({"view":"full","card_id":c.id,"body":"@developer quick question"}),
    )
    .unwrap();
    app.call(
        "execution.report",
        json!({"project_id":&p.id[..4],"card":c.number,"state":"completed","message":"done","body":"Full report","request_key":"rep-short"}),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let rep = &snap.reports[0];
    assert_eq!(rep.id.len(), 8);
    assert_eq!(
        rep.path.as_deref(),
        Some(
            format!(
                "projects/{}/cards/{}/agents/developer/reports/{}.md",
                p.id, c.number, rep.id
            )
            .as_str()
        )
    );

    // Prefix + name + legacy id_migrations lookup.
    let old_pid = format!("{}000000000000000000000000", p.id);
    let old_cid = format!("{}111111111111111111111111", c.id);
    let old_qid = "22223333444455556666777788889999";
    let real_qid = snap.questions[0].id.clone();
    app.store.lock().unwrap().write(|db| {
        db.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS id_migrations(kind TEXT NOT NULL, old_id TEXT NOT NULL, new_id TEXT NOT NULL, PRIMARY KEY(kind, old_id));",
        )?;
        db.conn.execute("INSERT OR REPLACE INTO id_migrations VALUES('project',?1,?2)", rusqlite::params![old_pid, p.id])?;
        db.conn.execute("INSERT OR REPLACE INTO id_migrations VALUES('card',?1,?2)", rusqlite::params![old_cid, c.id])?;
        db.conn.execute("INSERT OR REPLACE INTO id_migrations VALUES('question',?1,?2)", rusqlite::params![old_qid, real_qid])?;
        Ok(())
    }).unwrap();

    // Legacy 32-char project_id + prefix + project name resolve on card.get / card.list.
    let by_legacy = app
        .call(
            "card.get",
            json!({"view":"full","project_id":old_pid,"card":c.number}),
        )
        .unwrap();
    assert_eq!(by_legacy["id"], c.id);
    let by_name = app
        .call(
            "card.get",
            json!({"view":"full","project_id":"Example","card":c.number}),
        )
        .unwrap();
    assert_eq!(by_name["id"], c.id);
    let by_old_card = app
        .call("card.get", json!({"view":"full","card_id":old_cid}))
        .unwrap();
    assert_eq!(by_old_card["id"], c.id);
    let by_prefix = app
        .call("card.get", json!({"view":"full","card_id":&c.id[..4]}))
        .unwrap();
    assert_eq!(by_prefix["id"], c.id);
    let preview_err = app
        .call(
            "review.preview",
            json!({"project_id":&p.id,"card":c.number}),
        )
        .unwrap_err();
    assert!(preview_err.to_string().contains("no prepared worktree"));

    // Legacy 32-char question_id + project alias resolve on agent.reply.
    app.call(
        "agent.reply",
        json!({"project_id":old_pid,"card":c.number,"question_id":old_qid,"body":"Answered via old question id","request_key":"rep-legacy-q"}),
    )
    .unwrap();
    assert_eq!(app.snapshot().unwrap().questions[0].state, "replied");
}

#[cfg(unix)]
#[test]
fn git_related_card_requires_a_unique_canonical_worktree_match() {
    let (dir, app, p, mut card) = setup();
    let repo = Path::new(&p.directory);
    std::fs::create_dir_all(repo).unwrap();
    let git = |args: &[&str]| {
        assert!(
            canban_exec::raw_command("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init", "-b", "main"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--allow-empty",
        "-m",
        "initial",
    ]);
    let worktree = dir.path().join("card-worktree");
    git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        worktree.to_str().unwrap(),
    ]);
    let alias = dir.path().join("alias");
    std::os::unix::fs::symlink(&worktree, &alias).unwrap();
    card.worktree = Some(alias.to_string_lossy().into_owned());
    // Even an identical branch name must not associate the main checkout.
    card.branch = Some("main".into());
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&card))
        .unwrap();
    let main = app.call("git.refs", json!({"project_id":p.id})).unwrap();
    assert!(main["related_card_id"].is_null());
    let refs = app
        .call("git.refs", json!({"project_id":p.id,"cwd":worktree}))
        .unwrap();
    assert_eq!(refs["related_card_id"], card.id);
    let mut second: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id":p.id,"title":"Same path"}),
        )
        .unwrap(),
    )
    .unwrap();
    second.worktree = Some(worktree.to_string_lossy().into_owned());
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&second))
        .unwrap();
    let refs = app
        .call("git.refs", json!({"project_id":p.id,"cwd":worktree}))
        .unwrap();
    assert!(refs["related_card_id"].is_null());
    assert_eq!(
        refs["checkouts"].as_array().unwrap().len(),
        2,
        "symlink aliases must be deduplicated"
    );
}

mod memory;

mod representations;

mod clock_and_livelock;

#[test]
fn pr_card_worktree_tracks_pr_branch_and_review_drafts_roundtrip_with_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let remote = dir.path().join("origin.git");
    let source = dir.path().join("source");
    let peer = dir.path().join("peer");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&source).unwrap();
    let git = |cwd: &Path, args: &[&str]| {
        let out = canban_exec::raw_command("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&remote, &["init", "--bare", "-b", "main"]);
    git(&source, &["init", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Tester"),
        ("user.email", "tester@example.invalid"),
        ("core.hooksPath", "/dev/null"),
        ("commit.gpgsign", "false"),
    ] {
        git(&source, &["config", k, v]);
    }
    std::fs::write(source.join("README.md"), "base\n").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-m", "root"]);
    git(
        &source,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&source, &["push", "-u", "origin", "main"]);

    git(
        dir.path(),
        &["clone", remote.to_str().unwrap(), peer.to_str().unwrap()],
    );
    for (k, v) in [
        ("user.name", "Peer"),
        ("user.email", "peer@example.invalid"),
        ("core.hooksPath", "/dev/null"),
        ("commit.gpgsign", "false"),
    ] {
        git(&peer, &["config", k, v]);
    }
    git(&peer, &["switch", "-c", "feat/pr-work"]);
    std::fs::write(peer.join("pr.rs"), "pub fn pr_code() {}\n").unwrap();
    git(&peer, &["add", "."]);
    git(&peer, &["commit", "-m", "feat: pr work"]);
    git(&peer, &["push", "origin", "feat/pr-work"]);

    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join("credentials")).unwrap();
    std::fs::write(
        home.join("credentials/github.json"),
        serde_json::to_vec(&json!({
            "client_id": canban_github::auth::CLIENT_ID,
            "login": "fixture",
            "access_token": "fixture-not-a-real-token",
            "refresh_token": null,
            "expires_at": null
        }))
        .unwrap(),
    )
    .unwrap();
    let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name":"PRProj","directory":source}),
        )
        .unwrap(),
    )
    .unwrap();
    let c: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id":p.id,"title":"Review PR #256"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut card: Card = db.get(&c.id)?;
            card.content = CardContent::GitHubPr {
                source: GitHubIssueRef {
                    repository: "example/repo".into(),
                    repository_id: 1,
                    issue_id: 2560,
                    number: 256,
                    url: "https://github.com/example/repo/pull/256".into(),
                },
            };
            db.save(&card)
        })
        .unwrap();

    // Seed PR cache so `prepare_pr_card_source` sees same-repo `feat/pr-work` -> `main`.
    app.github.cached_client().unwrap().seed(
        "/repos/example/repo/pulls/256",
        json!({
            "id": 2560,
            "number": 256,
            "state": "open",
            "title": "Review PR #256",
            "head": {
                "ref": "feat/pr-work",
                "sha": "deadbeef",
                "repo": {"full_name": "example/repo"}
            },
            "base": {
                "ref": "main",
                "repo": {"full_name": "example/repo"}
            },
            "user": {"id": 1, "login": "alice"},
            "updated_at": "2026-11-18T00:00:00Z"
        }),
    );

    // Disable @reviewer to verify fallback to default_agent when running Agent Review.
    let mut cfg = app.config();
    for prof in &mut cfg.profiles {
        if prof.id == "reviewer" {
            prof.enabled = false;
        }
    }
    *app.config.write().unwrap() = cfg;

    let queued = app
        .call(
            "review.run_agent",
            json!({"card_id": c.id, "focus": "Check lock ordering"}),
        )
        .unwrap();
    assert_eq!(queued["state"], "queued");
    assert_eq!(queued["profile_id"], "developer");
    assert_eq!(queued["card_record"]["id"], c.id);
    assert_eq!(queued["terminal"]["id"], queued["terminal_id"]);

    let snap = app.snapshot().unwrap();
    let card_after = snap.cards.iter().find(|x| x.id == c.id).unwrap();
    let wt_path = PathBuf::from(card_after.worktree.as_deref().unwrap());
    assert!(wt_path.join("pr.rs").exists());
    let ws_info: WorkspaceInfo = serde_json::from_value(
        app.call("card.workspace", json!({"card_id": c.id}))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(ws_info.upstream.as_deref(), Some("origin/feat/pr-work"));
    assert_eq!(ws_info.target_branch.as_deref(), Some("main"));

    // Stage local diff review draft and verify Snapshot projection + draft.list isolation.
    let draft_res = app
        .call(
            "review.draft.set",
            json!({
                "card_id": c.id,
                "profile_id": "developer",
                "summary": "Check edge case in pr_code.",
                "verdict": "request_changes",
                "comments": [
                    {"path": "pr.rs", "line": 1, "side": "RIGHT", "body": "Add docstring."}
                ]
            }),
        )
        .unwrap();
    assert_eq!(draft_res["saved"], true);
    assert_eq!(draft_res["comment_count"], 1);
    assert_eq!(draft_res["draft"]["card_id"], c.id);
    assert_eq!(draft_res["draft"]["compare_sha"], "deadbeef");
    assert!(
        app.call("draft.list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.review_drafts.len(), 1);
    assert_eq!(
        snap.review_drafts[0].comments[0].author.as_deref(),
        Some("@developer")
    );

    // Clearing draft on PR publish removes it from Snapshot.
    app.clear_review_draft_for_pr("example/repo", 256, None)
        .unwrap();
    assert!(app.snapshot().unwrap().review_drafts.is_empty());
}

#[test]
fn card_create_mentions_launch_mention_reply_tasks_while_card_update_never_triggers() {
    let (_d, app, project, existing) = setup();

    // 1. Implicit prose mentions across title and body launch mention reply assignments even in backlog,
    // and reporting `completed` publishes a `comment.replied` message to the Card discussion without duplicating the body in `## Task`.
    let created: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({
                "view": "full",
                "project_id": project.id,
                "title": "@developer Investigate title mention",
                "body": "Please coordinate with @reviewer before coding."
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(created.stage, "backlog");
    let snap = app.snapshot().unwrap();
    let evs = app.events().unwrap();
    let created_ev = evs
        .iter()
        .find(|e| e.card_id == created.id && e.kind == "card.created")
        .expect("card.created event");
    let created_terminals: Vec<_> = snap
        .terminals
        .iter()
        .filter(|t| t.agent.as_ref().is_some_and(|a| a.card_id == created.id))
        .cloned()
        .collect();
    assert_eq!(created_terminals.len(), 2);
    for entry in &created_terminals {
        let run = entry.agent.as_ref().unwrap();
        assert_eq!(
            run.trigger,
            ExecutionTrigger::Comment {
                comment_id: created_ev.id.clone()
            }
        );
        assert_eq!(run.state, "queued");
        assert!(
            run.instruction.contains(&format!(
                "You were mentioned (@{}) on this Card. Read the Card context and respond to the user's message in the Card title and description.",
                run.profile.id
            ))
        );
        assert!(
            !run.instruction
                .contains("Please coordinate with @reviewer before coding."),
            "card mention instruction should reference Card title/description without duplicating body"
        );
    }
    assert!(
        created_terminals
            .iter()
            .any(|t| t.agent.as_ref().unwrap().profile.id == "developer")
    );
    assert!(
        created_terminals
            .iter()
            .any(|t| t.agent.as_ref().unwrap().profile.id == "reviewer")
    );
    assert_eq!(
        snap.questions
            .iter()
            .filter(|q| q.card_id == created.id && q.state == "pending")
            .count(),
        2
    );

    // Admitting and completing one of the Card-create mentions posts `comment.replied` directly to the Card discussion.
    let dev_req = created_terminals
        .iter()
        .find(|t| t.agent.as_ref().unwrap().profile.id == "developer")
        .unwrap();
    let dev_admitted = admit(&app, dev_req);
    let snap_for_prompt = app.snapshot().unwrap();
    let prompt_events = app.events().unwrap();
    let rel_prompt = canban_context::capture(
        &app.home,
        &project,
        &created,
        &dev_admitted,
        &snap_for_prompt,
        &prompt_events,
    )
    .unwrap();
    let rendered_prompt = std::fs::read_to_string(app.home.join(rel_prompt)).unwrap();
    assert!(
        rendered_prompt.contains("Finish this `@mention` in one report (no separate `reply`).")
            && !rendered_prompt.contains("## Pending questions"),
        "card mention prompt must use reporting_mention.txt and omit separate `reply --to` section"
    );
    app.call(
        "execution.report",
        json!({
            "request_key": "card-create-dev-reply",
            "project_id": project.id,
            "card": created.number,
            "terminal_id": dev_admitted.id,
            "profile_id": "developer",
            "state": "completed",
            "message": "Here is my analysis of the title mention."
        }),
    )
    .unwrap();
    let evs_after = app.events().unwrap();
    let reply_events: Vec<_> = evs_after
        .iter()
        .filter(|e| e.card_id == created.id && e.kind == "comment.replied")
        .collect();
    assert_eq!(reply_events.len(), 1);
    assert_eq!(reply_events[0].actor, "agent:developer");
    assert_eq!(
        reply_events[0].message,
        "Here is my analysis of the title mention."
    );
    assert_eq!(reply_events[0].data["reply_to"], json!(created_ev.id));

    // 2. Explicit `mentions` overrides prose scanning on `card.create`.
    let explicit_card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({
                "view": "full",
                "project_id": project.id,
                "title": "Ask @developer about @reviewer",
                "body": "Body mentions @developer",
                "mentions": ["reviewer"]
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    let explicit_terminals: Vec<_> = snap
        .terminals
        .iter()
        .filter_map(|t| t.agent.as_ref().filter(|a| a.card_id == explicit_card.id))
        .collect();
    assert_eq!(explicit_terminals.len(), 1);
    assert_eq!(explicit_terminals[0].profile.id, "reviewer");
    assert!(explicit_terminals[0].trigger.is_comment());

    // 3. Explicit empty `mentions: []` suppresses prose scanning on `card.create`.
    let suppressed_card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({
                "view": "full",
                "project_id": project.id,
                "title": "Discuss @developer behavior",
                "body": "Mention @reviewer only in docs",
                "mentions": []
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    assert!(snap.terminals.iter().all(|t| {
        t.agent
            .as_ref()
            .is_none_or(|a| a.card_id != suppressed_card.id)
    }));

    // 4. Editing a Card (`card.update`) never triggers or launches agents.
    app.call(
        "card.update",
        json!({
            "card_id": existing.id,
            "revision": existing.revision,
            "title": "@developer @reviewer Updated title",
            "body": "Updated body mentioning @developer"
        }),
    )
    .unwrap();
    let snap = app.snapshot().unwrap();
    assert!(
        snap.terminals
            .iter()
            .all(|t| t.agent.as_ref().is_none_or(|a| a.card_id != existing.id))
    );
}

#[test]
fn agent_catalog_and_diagnostics_detect_custom_prefix_pi_via_hc_env_path() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, app, _project, _card) = setup();
    let custom_dir = _dir.path().join("custom-npm-prefix/bin");
    std::fs::create_dir_all(&custom_dir).unwrap();
    let pi_bin = custom_dir.join("pi");
    std::fs::write(&pi_bin, "#!/bin/sh\necho pi\n").unwrap();
    std::fs::set_permissions(&pi_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Persist custom npm prefix in `hc_env["PATH"]` (simulating a user shell PATH not in GUI PATH).
    let cfg_path = app.home.join("config/agents.json");
    let mut cfg_val: Value = serde_json::from_slice(&std::fs::read(&cfg_path).unwrap()).unwrap();
    cfg_val["hc_env"]["PATH"] = json!(format!("{}:/usr/bin:/bin", custom_dir.display()));
    canban_context::atomic(&cfg_path, &serde_json::to_vec_pretty(&cfg_val).unwrap()).unwrap();
    app.call("config.reload", json!({})).unwrap();

    let catalog = app.call("agent.catalog", json!({})).unwrap();
    let pi_entry = catalog["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "pi")
        .unwrap();
    assert_eq!(pi_entry["installed"], true);
    assert_eq!(pi_entry["path"], pi_bin.display().to_string());

    // Enable `pi` and verify diagnostics also resolves it cleanly.
    app.call(
        "config.agents.save",
        json!({"op": "toggle_agent", "id": "pi", "enabled": true}),
    )
    .unwrap();
    let diag = app.call("diagnostics.status", json!({})).unwrap();
    let prof_check = diag["toolchain"]["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "pi")
        .unwrap();
    assert_eq!(prof_check["available"], true);
    assert_eq!(prof_check["resolved_path"], pi_bin.display().to_string());
}

#[test]
fn resolve_pr_remote_matches_upstream_and_falls_back_to_github_urls() {
    use canban_workspaces::compare::PrRemoteSource;

    let (_dir, app, project, _card) = setup();
    let repo_dir = PathBuf::from(&project.directory);
    let ops = app.ops();
    let run_git = |args: &[&str]| {
        let out = canban_exec::raw_command("git")
            .args(args)
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };

    // 1. Fork + upstream layout: `origin` is personal fork, `upstream` is target repo.
    run_git(&["init", "-b", "main"]);
    run_git(&["remote", "add", "origin", "git@github.com:jjyr/fiber.git"]);
    run_git(&[
        "remote",
        "add",
        "upstream",
        "git@github.com:nervosnetwork/fiber.git",
    ]);
    assert_eq!(
        app.resolve_pr_remote(&ops, &project, "nervosnetwork/fiber"),
        Some(PrRemoteSource::Named("upstream".into()))
    );
    // Preferred remote still wins when it matches the PR repository.
    assert_eq!(
        app.resolve_pr_remote(&ops, &project, "jjyr/fiber"),
        Some(PrRemoteSource::Named("origin".into()))
    );

    // 2. Fork without upstream: falls back to direct GitHub URLs (SSH first to match origin).
    run_git(&["remote", "remove", "upstream"]);
    assert_eq!(
        app.resolve_pr_remote(&ops, &project, "nervosnetwork/fiber"),
        Some(PrRemoteSource::Urls(vec![
            "git@github.com:nervosnetwork/fiber.git".into(),
            "https://github.com/nervosnetwork/fiber.git".into(),
        ]))
    );

    // 3. HTTPS fork without upstream: prefers HTTPS URL first.
    run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/jjyr/fiber.git",
    ]);
    assert_eq!(
        app.resolve_pr_remote(&ops, &project, "nervosnetwork/fiber"),
        Some(PrRemoteSource::Urls(vec![
            "https://github.com/nervosnetwork/fiber.git".into(),
            "git@github.com:nervosnetwork/fiber.git".into(),
        ]))
    );

    // 4. Non-GitHub local/custom remote: returns None so `seed_pr_for_card` uses configured remote.
    run_git(&["remote", "set-url", "origin", "/tmp/local-bare.git"]);
    assert_eq!(
        app.resolve_pr_remote(&ops, &project, "nervosnetwork/fiber"),
        None
    );
}

#[test]
fn project_delete_removes_canban_data_and_preserves_user_repo_and_worktrees() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let repo1 = dir.path().join("repo1");
    let repo2 = dir.path().join("repo2");
    let worktree1 = home.join("worktrees/repo1-1234/1");
    std::fs::create_dir_all(&repo1).unwrap();
    std::fs::create_dir_all(&repo2).unwrap();
    std::fs::create_dir_all(&worktree1).unwrap();
    std::fs::write(repo1.join("README.md"), "user repo").unwrap();
    std::fs::write(worktree1.join("WIP.txt"), "uncommitted worktree file").unwrap();

    let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let p1: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Alpha", "directory": repo1}),
        )
        .unwrap(),
    )
    .unwrap();
    let p2: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Beta", "directory": repo2}),
        )
        .unwrap(),
    )
    .unwrap();

    // Populate Cards, comments, drafts, desktop preferences, managed worktrees, and Canban metadata.
    let c1: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": p1.id, "title": "Alpha task"}),
        )
        .unwrap(),
    )
    .unwrap();
    let _c2: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": p2.id, "title": "Beta task"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "comment.add",
        json!({"card_id": c1.id, "revision": c1.revision, "body": "Notes on alpha"}),
    )
    .unwrap();
    app.call(
        "draft.save",
        json!({"id": format!("new-card:{}", p1.id), "revision": 0, "data": {"title": "Unsaved"}}),
    )
    .unwrap();
    app.call(
        "utilities.configure",
        json!({
            "last_project": p1.id,
            "board_filter": {
                "account": "local",
                "project_id": p1.id,
                "value": {"tags": ["alpha"]}
            }
        }),
    )
    .unwrap();
    let meta_dir = home.join("projects").join(&p1.id);
    std::fs::create_dir_all(&meta_dir).unwrap();
    std::fs::write(meta_dir.join("context.txt"), "canban metadata").unwrap();

    // Attach a managed worktree row and a live terminal to p1.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&ManagedWorktree {
                id: "wt-1".into(),
                project_id: p1.id.clone(),
                branch: "feat/keep-on-disk".into(),
                base_branch: "main".into(),
                base_commit: None,
                path: worktree1.to_string_lossy().into_owned(),
                directory: repo1.to_string_lossy().into_owned(),
                created_at: 1,
            })?;
            db.save(&TerminalEntry {
                id: "term-live".into(),
                conversation_id: None,
                number: 1,
                name: Some("Live shell".into()),
                origin_project_id: Some(p1.id.clone()),
                origin_card_id: None,
                agent_engine: None,
                runtime: Some(TerminalRuntime {
                    core_id: None,
                    terminal: Some("h-live".into()),
                    state: "running".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                    observed_agent: None,
                    waiting: None,
                    command: None,
                }),
                agent: None,
                created_at: 1,
            })?;
            Ok(())
        })
        .unwrap();

    // 1. Reject execution-scoped (Agent) calls.
    let err = app
        .call(
            "project.delete",
            json!({"project_id": p1.id, "execution": {"card_id": c1.id}}),
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("user action"),
        "expected user action guard, got: {err:#}"
    );

    // 2. Reject deletion while a live terminal belongs to the project.
    let err = app
        .call("project.delete", json!({"project_id": p1.id}))
        .unwrap_err();
    assert!(
        err.to_string().contains("1 live terminal"),
        "expected live terminal guard, got: {err:#}"
    );

    // Mark the terminal finished so deletion is allowed.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut t: TerminalEntry = db.get("term-live")?;
            t.runtime.as_mut().unwrap().state = "finished".into();
            db.save(&t)
        })
        .unwrap();

    // 3. Delete p1 successfully.
    let receipt = app
        .call("project.delete", json!({"project_id": p1.id}))
        .unwrap();
    assert_eq!(receipt["deleted"], true);
    assert_eq!(receipt["project_id"], p1.id);
    assert_eq!(receipt["name"], "Alpha");
    assert_eq!(receipt["deleted_cards"], 1);

    // 4. Verify Canban-side data is removed while p2 stays intact.
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.projects.len(), 1);
    assert_eq!(snap.projects[0].id, p2.id);
    assert_eq!(snap.cards.len(), 1);
    assert_eq!(snap.cards[0].project_id, p2.id);
    assert!(snap.terminals.is_empty());
    assert!(snap.worktrees.is_empty());
    assert!(snap.workflow_sources.iter().all(|s| s.project_id == p2.id));
    assert!(
        !meta_dir.exists(),
        "Canban project metadata dir should be removed"
    );

    let drafts = app.call("draft.list", json!({})).unwrap();
    assert!(
        drafts.as_array().unwrap().is_empty(),
        "project drafts should be removed"
    );

    let util = app.call("utilities.state", json!({})).unwrap();
    assert!(util["preferences"]["last_project"].is_null());
    assert!(util["preferences"].get("board_filters").is_none());

    // 5. Verify user repo and Git worktree on disk are completely untouched.
    assert!(repo1.join("README.md").exists());
    assert_eq!(
        std::fs::read_to_string(repo1.join("README.md")).unwrap(),
        "user repo"
    );
    assert!(worktree1.join("WIP.txt").exists());
    assert_eq!(
        std::fs::read_to_string(worktree1.join("WIP.txt")).unwrap(),
        "uncommitted worktree file"
    );
}
