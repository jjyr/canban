use super::*;
#[test]
fn remote_reports_wait_through_handoff_without_consuming_or_duplicating_completion() {
    let (_dir, app, p, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let _run = admit(&app, &app.snapshot().unwrap().terminals[0]);
    let report = json!({"request_key":"remote-upgrade-report","project_id":p.id,"card":c.number,"state":"completed","message":"done"});
    std::fs::write(app.home.join("updates/install.json"), b"interrupted").unwrap();
    assert!(
        app.remote_call("execution.report", report.clone())
            .unwrap()
            .is_none()
    );
    assert!(
        app.remote_call(
            "card.create",
            json!({"project_id":p.id,"title":"do not replay"})
        )
        .is_err()
    );
    assert!(app.snapshot().unwrap().reports.is_empty());
    std::fs::remove_file(app.home.join("updates/install.json")).unwrap();
    app.remote_call("execution.report", report.clone())
        .unwrap()
        .unwrap();
    app.remote_call("execution.report", report)
        .unwrap()
        .unwrap();
    let snapshot = app.snapshot().unwrap();
    assert_eq!(snapshot.reports.len(), 1);
    assert_eq!(snapshot.cards[0].stage, "review");
    assert!(app.remote_call("update.install", json!({})).is_err());
}
#[test]
fn location_edits_retarget_queued_work_but_not_existing_attempts_or_resumes() {
    let (dir, app, p, c) = setup();
    super::terminal_cases::core(&app, dir.path());
    apply_project_yaml(
        &app,
        &p.id,
        &develop_and_review_yaml(&p.id, false, false, false),
    );
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let next = dir.path().join("next");
    app.call(
        "project.location",
        json!({"project_id":p.id,"location":{"directory":next,"machine":"local"}}),
    )
    .unwrap();
    assert!(
        app.snapshot().unwrap().terminals[0]
            .agent
            .as_ref()
            .unwrap()
            .location
            .is_none()
    );
    app.tick().unwrap();
    let s = app.snapshot().unwrap();
    let entry = s.terminals[0].clone();
    let run = entry.agent.as_ref().unwrap().clone();
    assert_eq!(
        run.location.as_ref().unwrap().directory,
        next.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(run.cwd, next.canonicalize().unwrap().to_str().unwrap());
    app.call(
        "agent.bind",
        json!({"project_id":p.id,"card":c.number,"profile_id":"developer","session_id":"provider-1"}),
    )
    .unwrap();
    std::fs::write(dir.path().join("finished"), "").unwrap();
    app.tick().unwrap();
    let last = dir.path().join("future");
    app.call(
        "project.location",
        json!({"project_id":p.id,"location":{"directory":last,"machine":"local"}}),
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
    assert!(resume.location.is_none());
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
    assert_eq!(a.location, run.location);
    assert_eq!(a.cwd, run.cwd);
    assert_eq!(a.provider_session_id.as_deref(), Some("provider-1"));
    assert_eq!(
        s.cards[0].workspace.as_ref().unwrap().location.directory,
        next.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(
        s.projects[0].work_location().directory,
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
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    app.tick().unwrap();
    app.call(
        "project.location",
        json!({"project_id":p.id,"directory":dir.path().join("other")}),
    )
    .unwrap();
    app.call("execution.report",json!({"project_id":p.id,"card":c.number,"state":"completed","message":"done","request_key":"finish-once"})).unwrap();
    std::fs::write(dir.path().join("finished"), "").unwrap();
    app.tick().unwrap();
    let s = app.snapshot().unwrap();
    let c = &s.cards[0];
    assert!(c.attention.as_ref().unwrap().contains("undelivered"));
    assert_eq!(
        c.workspace.as_ref().unwrap().location.directory,
        p.directory
    );
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
fn remote_completion_keeps_terminal_without_an_exit_intent_or_clock_comparison() {
    let (_dir, app, _, c) = setup();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let s = app.snapshot().unwrap();
    let entry = admit(&app, &s.terminals[0]);
    app.call("execution.report",json!({"project_id":c.project_id,"card":c.number,"state":"completed","message":"Done","request_key":"remote-retain"})).unwrap();
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
fn offline_work_does_not_starve_local_boards_or_fall_back_for_terminals() {
    let (dir, app, p, c) = setup();
    super::terminal_cases::core(&app, dir.path());
    let path = dir.path().join("test-hc");
    let code=std::fs::read_to_string(&path).unwrap().replace("elif m=='server.status':","elif m=='machine.list':r={'machines':[{'id':'remote-owner','name':'Offline','state':'unreachable'}]}\n elif m=='server.status':");
    std::fs::write(path, code).unwrap();
    app.call("project.location",json!({"project_id":p.id,"location":{"machine":"remote-owner","directory":"/offline/source"}})).unwrap();
    app.call(
        "card.move",
        json!({"card_id":c.id,"revision":1,"stage":"develop"}),
    )
    .unwrap();
    let p2 = app
        .call(
            "project.create",
            json!({"name":"Local","directory":dir.path().join("local"),"workspace":{"create_worktree":false,"sync_base":false}}),
        )
        .unwrap();
    let c2 = app
        .call(
            "card.create",
            json!({"project_id":p2["id"],"title":"Runnable"}),
        )
        .unwrap();
    app.call(
        "card.move",
        json!({"card_id":c2["id"],"revision":1,"stage":"develop"}),
    )
    .unwrap();
    app.tick().unwrap();
    let s = app.snapshot().unwrap();
    let remote = s
        .terminals
        .iter()
        .filter_map(|t| t.agent.as_ref())
        .find(|r| r.card_id == c.id)
        .unwrap();
    assert_eq!(remote.state, "queued");
    assert!(remote.location.is_none());
    assert!(
        s.terminals
            .iter()
            .filter_map(|t| t.agent.as_ref())
            .any(|a| json!(a.card_id) == c2["id"]
                && a.location.as_ref().is_some_and(|l| l.machine == "local"))
    );
    assert!(
        app.call("terminal.create", json!({"machine":"remote-owner"}))
            .is_err()
    );
    app.call("machine.default", json!({"machine":"remote-owner"}))
        .unwrap();
    assert!(app.call("terminal.create", json!({})).is_err());
    assert!(
        app.snapshot()
            .unwrap()
            .terminals
            .iter()
            .all(|t| t.runtime.is_none())
    );
}
#[test]
fn identical_paths_on_another_machine_do_not_associate_a_local_shell() {
    let (_dir, app, mut p, _) = setup();
    let cwd = p.directory.clone();
    p.location = Some(WorkLocation {
        machine: "other-core".into(),
        directory: cwd.clone(),
    });
    let snapshot = Snapshot {
        projects: vec![p],
        ..Default::default()
    };
    assert!(
        canban_workspaces::context::resolve(&snapshot, &cwd)
            .0
            .is_none()
    );
    assert!(app.snapshot().is_ok());
}
#[test]
fn provider_session_ids_are_scoped_to_the_owning_machine() {
    let (_dir, app, p, c) = setup();
    let second = app
        .call(
            "card.create",
            json!({"project_id":p.id,"title":"Another Card"}),
        )
        .unwrap();
    for card in [json!(c.id), second["id"].clone()] {
        app.call("agent.open", json!({"card_id":card,"profile_id":"codex"}))
            .unwrap();
    }
    let s = app.snapshot().unwrap();
    let _first = admit(&app, &s.terminals[0]);
    let other = admit(&app, &s.terminals[1]);
    app.call("agent.bind",json!({"project_id":p.id,"card":c.number,"profile_id":"codex","session_id":"short-provider-id"})).unwrap();
    assert!(app.call("agent.bind",json!({"project_id":p.id,"card":second["number"],"profile_id":"codex","session_id":"short-provider-id"})).is_err());
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut t: TerminalEntry = db.get(&other.id)?;
            t.agent.as_mut().unwrap().location = Some(WorkLocation {
                machine: "remote-owner".into(),
                directory: "/repo".into(),
            });
            db.save(&t)
        })
        .unwrap();
    app.call("agent.bind",json!({"project_id":p.id,"card":second["number"],"profile_id":"codex","session_id":"short-provider-id"})).unwrap();
}

#[test]
fn directory_list_returns_sorted_subdirs_home_and_nonexistent_gracefully() {
    let (dir, app, _, _) = setup();
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(ws.join("zebra")).unwrap();
    std::fs::create_dir_all(ws.join("alpha")).unwrap();
    std::fs::create_dir_all(ws.join(".hidden")).unwrap();
    std::fs::write(ws.join("README.md"), b"not a directory").unwrap();

    let res = app
        .call("directory.list", json!({"machine": "local", "path": ws}))
        .unwrap();
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
        .call(
            "directory.list",
            json!({"machine": "local", "path": missing}),
        )
        .unwrap();
    assert_eq!(res_missing["exists"], json!(false));
    assert_eq!(res_missing["directories"], json!([]));
    assert!(res_missing["home"].as_str().is_some_and(|h| !h.is_empty()));

    let res_tilde = app
        .call("directory.list", json!({"machine": "local", "path": "~"}))
        .unwrap();
    assert_eq!(res_tilde["exists"], json!(true));
}
