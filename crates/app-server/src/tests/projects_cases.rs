use super::*;

#[test]
fn directory_edits_retarget_queued_work_but_not_existing_attempts_or_resumes() {
    let (dir, app, p, c) = setup();
    super::terminal_cases::core(&app, dir.path());
    apply_project_yaml(
        &app,
        &p.id,
        &develop_and_review_yaml(&p.id, false, false, false),
    );
    app.call(
        "card.move",
        json!({"view":"full","card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let next = dir.path().join("next");
    app.call(
        "project.update",
        json!({"project_id":p.id,"directory":next}),
    )
    .unwrap();
    let next_canon = next.canonicalize().unwrap().to_str().unwrap().to_owned();
    assert!(
        app.snapshot().unwrap().terminals[0]
            .agent
            .as_ref()
            .unwrap()
            .directory
            .is_none()
    );
    app.tick().unwrap();
    let s = app.snapshot().unwrap();
    let entry = s.terminals[0].clone();
    let run = entry.agent.as_ref().unwrap().clone();
    assert_eq!(run.directory.as_deref(), Some(next_canon.as_str()));
    assert_eq!(run.cwd, next_canon);
    app.call(
        "agent.bind",
        json!({"project_id":p.id,"card":c.number,"profile_id":"developer","session_id":"provider-1"}),
    )
    .unwrap();
    std::fs::write(dir.path().join("finished"), "").unwrap();
    app.tick().unwrap();
    let last = dir.path().join("future");
    assert!(
        app.call(
            "project.update",
            json!({"project_id":p.id,"expected_directory":"/stale","directory":last}),
        )
        .is_err()
    );
    app.call(
        "project.update",
        json!({"project_id":p.id,"expected_directory":next_canon,"directory":last,"name":"Renamed Project"}),
    )
    .unwrap();
    app.call(
        "agent.open",
        json!({"card_id":c.id,"profile_id":"developer","expected_terminal_id":entry.id}),
    )
    .unwrap();
    let s = app.snapshot().unwrap();
    let resume = s
        .terminals
        .iter()
        .find(|t| t.id == entry.id)
        .unwrap()
        .agent
        .as_ref()
        .unwrap();
    assert!(resume.resume_session);
    assert!(resume.directory.is_none());
    app.tick().unwrap();
    let s = app.snapshot().unwrap();
    let a = s
        .terminals
        .iter()
        .find(|t| t.id == entry.id)
        .unwrap()
        .agent
        .as_ref()
        .unwrap();
    assert_eq!(a.directory, run.directory);
    assert_eq!(a.cwd, run.cwd);
    assert_eq!(a.provider_session_id.as_deref(), Some("provider-1"));
    assert_eq!(s.cards[0].workspace.as_ref().unwrap().directory, next_canon);
    assert_eq!(s.projects[0].name, "Renamed Project");
    assert_eq!(
        s.projects[0].directory,
        last.canonicalize().unwrap().to_str().unwrap()
    );
}

#[test]
fn changed_environment_never_reviews_an_unrelated_checkout() {
    let (dir, app, p, c) = setup();
    super::terminal_cases::core(&app, dir.path());
    apply_project_yaml(
        &app,
        &p.id,
        &develop_and_review_yaml(&p.id, false, false, false),
    );
    app.call(
        "card.move",
        json!({"view":"full","card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    app.tick().unwrap();
    app.call(
        "project.update",
        json!({"project_id":p.id,"directory":dir.path().join("other")}),
    )
    .unwrap();
    app.call("execution.report",json!({"project_id":p.id,"card":c.number,"state":"completed","message":"done","request_key":"finish-once"})).unwrap();
    std::fs::write(dir.path().join("finished"), "").unwrap();
    app.tick().unwrap();
    let s = app.snapshot().unwrap();
    let c = &s.cards[0];
    assert!(c.attention.as_ref().unwrap().contains("undelivered"));
    assert_eq!(c.workspace.as_ref().unwrap().directory, p.directory);
    assert_eq!(s.terminals.iter().filter(|t| t.agent.is_some()).count(), 2); // 1 completed developer + 1 failed reviewer (never launched in unrelated directory)
    assert!(
        s.terminals
            .iter()
            .filter_map(|t| t.agent.as_ref())
            .all(|a| a.terminal.as_deref() != Some("t2"))
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("launches"))
            .unwrap()
            .lines()
            .count(),
        1
    ); // no Agent launched in the unrelated directory
}

#[test]
fn appserver_rejects_a_second_owner_and_supports_explicit_shutdown() {
    let (dir, app, _, _) = setup();
    let running = app.start().unwrap();
    // Ownership is rejected before opening/migrating the database.
    assert!(AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).is_err());
    assert_eq!(
        rpc(&app.endpoint(), "app.state", json!({})).unwrap()["home"],
        json!(app.home)
    );
    rpc(&app.endpoint(), "app.shutdown", json!({})).unwrap();
    assert!(app.stopped());
    drop(running);
}

#[test]
fn completion_keeps_terminal_without_an_exit_intent_or_clock_comparison() {
    let (_dir, app, _, c) = setup();
    app.call(
        "card.move",
        json!({"view":"full","card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let s = app.snapshot().unwrap();
    let entry = admit(&app, &s.terminals[0]);
    app.call("execution.report",json!({"project_id":c.project_id,"card":c.number,"state":"completed","message":"Done","request_key":"retain"})).unwrap();
    app.observe_lifecycle(
        &entry,
        &json!({"state":"live","status":{"state":"done"},"turn":6,"status_ms":100}),
    )
    .unwrap();
    let a = app.snapshot().unwrap().terminals[0]
        .agent
        .as_ref()
        .unwrap()
        .clone();
    assert!(a.close_state.is_empty());
    assert!(!a.finished());
}

#[test]
fn provider_session_ids_reject_duplicate_bindings_across_cards() {
    let (_dir, app, p, c) = setup();
    let second = app
        .call(
            "card.create",
            json!({"view":"full","project_id":p.id,"title":"Another Card"}),
        )
        .unwrap();
    for card in [json!(c.id), second["id"].clone()] {
        app.call("agent.open", json!({"card_id":card,"profile_id":"codex"}))
            .unwrap();
    }
    let s = app.snapshot().unwrap();
    let _first = admit(&app, &s.terminals[0]);
    let _other = admit(&app, &s.terminals[1]);
    app.call("agent.bind",json!({"project_id":p.id,"card":c.number,"profile_id":"codex","session_id":"short-provider-id"})).unwrap();
    assert!(app.call("agent.bind",json!({"project_id":p.id,"card":second["number"],"profile_id":"codex","session_id":"short-provider-id"})).is_err());
}

#[test]
fn directory_list_returns_sorted_subdirs_home_and_nonexistent_gracefully() {
    let (dir, app, _, _) = setup();
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(ws.join("zebra")).unwrap();
    std::fs::create_dir_all(ws.join("alpha")).unwrap();
    std::fs::create_dir_all(ws.join(".hidden")).unwrap();
    std::fs::write(ws.join("README.md"), b"not a directory").unwrap();

    let res = app.call("directory.list", json!({"path": ws})).unwrap();
    assert_eq!(res["exists"], json!(true));
    assert!(res["home"].as_str().is_some_and(|h| !h.is_empty()));
    let names: Vec<&str> = res["directories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec![".hidden", "alpha", "zebra"]);

    let missing = dir.path().join("workspace/not-created-yet");
    let res_missing = app
        .call("directory.list", json!({"path": missing}))
        .unwrap();
    assert_eq!(res_missing["exists"], json!(false));
    assert_eq!(res_missing["directories"], json!([]));
    assert!(res_missing["home"].as_str().is_some_and(|h| !h.is_empty()));

    let res_tilde = app.call("directory.list", json!({"path": "~"})).unwrap();
    assert_eq!(res_tilde["exists"], json!(true));
}
