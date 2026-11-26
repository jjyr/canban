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
            .contains("default_agent: developer")
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

mod github;

mod workflows;

mod multi_host;

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
