use super::*;

fn manual() -> (tempfile::TempDir, Arc<AppServer>, Card, TerminalEntry) {
    let (dir, app, project, card) = setup();
    std::fs::create_dir_all(&project.directory).unwrap();
    super::terminal_cases::core(&app, dir.path());
    let terminal = serde_json::from_value(
        app.call(
            "terminal.create",
            json!({
                "card_id":card.id,"agent":true,"profile_id":"codex","name":"My session"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let card = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.id == card.id)
        .unwrap();
    (dir, app, card, terminal)
}

fn adopt(card: &Card) -> Value {
    json!({"view":"full","card_id":card.id,"profile_id":"codex","cwd":card.worktree,
        "runtime_terminal":"t1","runtime_session":"default","request_key":"adopt-once"})
}

#[test]
fn adopt_takes_over_the_existing_pty_without_launching_and_replays_idempotently() {
    let (dir, app, card, source) = manual();
    app.call(
        "agent.bind",
        json!({"card_id":card.id,"profile_id":"codex","session_id":"exact-session"}),
    )
    .unwrap();
    // Tool shells may run in a subdirectory while the Agent's foreground cwd stays at root.
    let nested = dir.path().join("source/nested");
    std::fs::create_dir_all(&nested).unwrap();
    let mut params = adopt(&card);
    params["cwd"] = json!(nested);
    let first = app.call("card.adopt", params.clone()).unwrap();
    assert_eq!(app.call("card.adopt", params).unwrap(), first);
    let s = app.snapshot().unwrap();
    let entry = s.terminals.iter().find(|t| t.agent.is_some()).unwrap();
    let run = entry.agent.as_ref().unwrap();
    assert_eq!(entry.id, Conversation::key(&card.id, "codex"));
    assert_eq!(entry.name, source.name);
    assert_eq!(entry.number, source.number);
    assert_eq!(run.terminal, source.runtime.as_ref().unwrap().terminal);
    assert_eq!(run.core_id, source.runtime.as_ref().unwrap().core_id);
    assert_eq!(run.provider_session_id.as_deref(), Some("exact-session"));
    assert_eq!(run.state, "running");
    assert_eq!(run.trigger, ExecutionTrigger::Interactive);
    assert_eq!(
        run.workflow_run_id.as_deref(),
        first["workflow_run"]["id"].as_str()
    );
    assert_eq!(first["workflow_run"]["terminal_id"], entry.id);
    let detached = s.terminals.iter().find(|t| t.id == source.id).unwrap();
    assert_eq!(detached.runtime.as_ref().unwrap().state, "finished");
    assert!(detached.runtime.as_ref().unwrap().terminal.is_none());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("launches"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let entry_id = entry.id.clone();
    app.call("execution.report", json!({"card_id":card.id,"profile_id":"codex","state":"completed","request_key":"finished","message":"Implemented"})).unwrap();
    let after = app.snapshot().unwrap();
    let mut completed = after
        .terminals
        .iter()
        .find(|t| t.id == entry_id)
        .unwrap()
        .agent
        .clone()
        .unwrap();
    assert_eq!(completed.state, "completed");
    completed.observed = Some(json!({"state":"live","status":{"state":"done"}}));
    completed.lifecycle.observed_at = Some(now());
    completed.lifecycle.idle_since = Some(now() - 31_000);
    completed.lifecycle.last_interaction_at = Some(now() - 31_000);
    completed.completed_at = Some(now() - 31_000);
    assert!(crate::lifecycle::idle_candidate(
        &after,
        &completed,
        15,
        now(),
        false
    ));
    completed.lifecycle.last_interaction_at = Some(now());
    assert!(!crate::lifecycle::idle_candidate(
        &after,
        &completed,
        15,
        now(),
        false
    ));
    let home = app.home.clone();
    drop(app);
    let reopened = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    let resumed = reopened
        .snapshot()
        .unwrap()
        .terminals
        .into_iter()
        .find(|t| t.id == entry_id)
        .unwrap()
        .agent
        .unwrap();
    assert_eq!(resumed.terminal.as_deref(), Some("t1"));
    assert_eq!(
        resumed.provider_session_id.as_deref(),
        Some("exact-session")
    );
}

#[test]
fn adopt_can_bind_after_an_earlier_workflow_only_adoption() {
    let (dir, app, card, _) = manual();
    let first = app.call("card.adopt", json!({"view":"full","card_id":card.id,"profile_id":"codex","request_key":"without-terminal"})).unwrap();
    assert!(first["workflow_run"]["terminal_id"].is_null());
    let second = app.call("card.adopt", adopt(&card)).unwrap();
    assert_eq!(first["workflow_run"]["id"], second["workflow_run"]["id"]);
    assert_eq!(
        second["workflow_run"]["terminal_id"],
        Conversation::key(&card.id, "codex")
    );
    let mut again = adopt(&card);
    again["request_key"] = json!("again");
    app.call("card.adopt", again).unwrap();
    assert_eq!(
        app.snapshot()
            .unwrap()
            .terminals
            .iter()
            .filter(|t| t.agent.is_some())
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("launches"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn adopt_without_a_usable_terminal_keeps_the_workflow_and_manual_runtime() {
    for failure in [
        "missing",
        "unknown",
        "session",
        "cwd",
        "profile",
        "core",
        "ambiguous",
    ] {
        let (dir, app, card, source) = manual();
        let mut params = adopt(&card);
        match failure {
            "missing" => {
                params.as_object_mut().unwrap().remove("runtime_terminal");
            }
            "unknown" => params["runtime_terminal"] = json!("t404"),
            "session" => params["runtime_session"] = json!("another-core"),
            "cwd" => params["cwd"] = json!(dir.path()),
            "profile" => params["profile_id"] = json!("claude"),
            "core" => std::fs::write(dir.path().join("replace"), "").unwrap(),
            "ambiguous" => {
                let mut other = source.clone();
                other.id = "duplicate".into();
                other.number += 100;
                app.store
                    .lock()
                    .unwrap()
                    .write(|db| db.save(&other))
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let adopted = app.call("card.adopt", params).unwrap();
        assert_eq!(adopted["card"]["stage"], "develop", "{failure}");
        assert!(
            adopted["workflow_run"]["terminal_id"].is_null(),
            "{failure}"
        );
        let s = app.snapshot().unwrap();
        assert!(s.terminals.iter().all(|t| t.agent.is_none()), "{failure}");
        assert_eq!(
            s.terminals
                .iter()
                .find(|t| t.id == source.id)
                .unwrap()
                .runtime,
            source.runtime,
            "{failure}"
        );
        assert!(s.conversations.is_empty(), "{failure}");
    }
}

#[test]
fn adopt_binding_savepoint_preserves_an_existing_workflow_owner() {
    let (_dir, app, card, source) = manual();
    let result = app
        .call("card.adopt", json!({"view":"full","card_id":card.id}))
        .unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut workflow: WorkflowRun =
                db.get(result["workflow_run"]["id"].as_str().unwrap())?;
            workflow.terminal_id = Some("another-owner".into());
            db.save(&workflow)
        })
        .unwrap();
    let result = app.call("card.adopt", adopt(&card)).unwrap();
    assert_eq!(result["workflow_run"]["terminal_id"], "another-owner");
    let s = app.snapshot().unwrap();
    assert!(s.conversations.is_empty());
    assert_eq!(s.terminals.len(), 1);
    assert_eq!(s.terminals[0].runtime, source.runtime);
}
