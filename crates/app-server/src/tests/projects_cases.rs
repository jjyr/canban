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
fn app_shutdown_flushes_rpc_reply_before_wait_stopped_tears_down_runtime() {
    // 1. Direct in-process call wakes wait_stopped() immediately.
    let (_dir, app, _, _) = setup();
    let wait_app = Arc::clone(&app);
    let waiter = std::thread::spawn(move || wait_app.wait_stopped());
    assert_eq!(
        app.call("app.shutdown", json!({})).unwrap(),
        json!({"stopping": true})
    );
    assert!(app.stopped());
    waiter.join().unwrap();

    // 2. Socket RPC mimics `canban serve`: wait_stopped() immediately drops `Running` on return.
    for iter in 0..20 {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
        let endpoint = app.endpoint();
        let running = app.start().unwrap();
        let wait_app = Arc::clone(&app);
        let server_thread = std::thread::spawn(move || {
            wait_app.wait_stopped();
            drop(running);
        });

        if iter == 0 {
            let err = rpc(
                &endpoint,
                "app.shutdown",
                json!({"expected_pid": std::process::id() + 1}),
            )
            .unwrap_err();
            assert!(
                format!("{err:#}").contains("refusing shutdown"),
                "unexpected error on expected_pid mismatch: {err:#}"
            );
            assert!(!app.stopped());
        }

        let params = if iter % 2 == 0 {
            json!({"expected_pid": std::process::id()})
        } else {
            json!({})
        };
        let reply = if iter % 2 == 0 {
            canban_updater::handoff::rpc(&endpoint, "app.shutdown", params)
        } else {
            rpc(&endpoint, "app.shutdown", params)
        }
        .unwrap_or_else(|e| panic!("iteration {iter} app.shutdown RPC failed: {e:#}"));
        assert_eq!(reply, json!({"stopping": true}), "iteration {iter}");
        assert!(app.stopped(), "iteration {iter} should mark app stopped");
        server_thread.join().unwrap();
    }
}

#[test]
fn app_shutdown_flushes_rpc_reply_over_tcp_and_yamux_before_teardown() {
    let listen = ServerListenConfig {
        tailscale: false,
        port: 0,
        listen: Some("127.0.0.1:0".into()),
        allow_insecure_bind: false,
    };

    // Direct TCP RPC
    for iter in 0..5 {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        let running = app.start_with_listen(Some(listen.clone())).unwrap();
        let addr = app.bound_tcp_addrs().into_iter().next().unwrap();
        let (_, token) = app.issue_device_token("tcp-shutdown-test").unwrap();
        let wait_app = Arc::clone(&app);
        let server_thread = std::thread::spawn(move || {
            wait_app.wait_stopped();
            drop(running);
        });

        let reply = client::rpc_tcp(&addr, Some(&token), "app.shutdown", json!({}))
            .unwrap_or_else(|e| panic!("direct TCP iteration {iter} failed: {e:#}"));
        assert_eq!(reply, json!({"stopping": true}));
        assert!(app.stopped());
        server_thread.join().unwrap();
    }

    // yamux/1 multiplexed AppClient RPC
    for iter in 0..5 {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        let running = app.start_with_listen(Some(listen.clone())).unwrap();
        let addr = app.bound_tcp_addrs().into_iter().next().unwrap();
        let (_, token) = app.issue_device_token("yamux-shutdown-test").unwrap();
        let wait_app = Arc::clone(&app);
        let server_thread = std::thread::spawn(move || {
            wait_app.wait_stopped();
            drop(running);
        });

        let mux_client = client::AppClient::connect_tcp(addr, token).unwrap();
        let reply = mux_client
            .call("app.shutdown", json!({}))
            .unwrap_or_else(|e| panic!("yamux iteration {iter} failed: {e:#}"));
        assert_eq!(reply, json!({"stopping": true}));
        assert!(app.stopped());
        drop(mux_client);
        server_thread.join().unwrap();
    }
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

#[test]
fn project_create_detects_default_branch_and_project_update_validates_target_branch() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    let git = |cwd: &Path, args: &[&str]| {
        let out = canban_exec::raw_command("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };

    let remote_bare = dir.path().join("origin.git");
    let seed = dir.path().join("seed");
    let repo = dir.path().join("fiber");
    std::fs::create_dir_all(&seed).unwrap();
    git(
        &seed,
        &[
            "init",
            "--bare",
            "-b",
            "develop",
            remote_bare.to_str().unwrap(),
        ],
    );
    git(&seed, &["init", "-b", "develop"]);
    for (k, v) in [
        ("user.name", "Tester"),
        ("user.email", "tester@example.invalid"),
        ("core.hooksPath", "/dev/null"),
        ("commit.gpgsign", "false"),
    ] {
        git(&seed, &["config", k, v]);
    }
    std::fs::write(seed.join("README.md"), "# Fiber\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "init"]);
    git(
        &seed,
        &["remote", "add", "origin", remote_bare.to_str().unwrap()],
    );
    git(&seed, &["push", "origin", "develop"]);

    git(
        dir.path(),
        &[
            "clone",
            "-b",
            "develop",
            remote_bare.to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    git(&repo, &["remote", "set-head", "origin", "develop"]);
    git(&repo, &["switch", "-c", "lsp-support"]);

    let created: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "Fiber", "directory": repo, "github_auto_link": false}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(created.target_branch.as_deref(), Some("develop"));

    // Invalid target branch names are rejected before mutating.
    assert!(
        app.call(
            "project.update",
            json!({"project_id": created.id, "target_branch": "-bad"})
        )
        .is_err()
    );
    assert!(
        app.call(
            "project.update",
            json!({"project_id": created.id, "target_branch": "a..b"})
        )
        .is_err()
    );

    // Valid target branch update & empty string clear work.
    let updated: Project = serde_json::from_value(
        app.call(
            "project.update",
            json!({"project_id": created.id, "target_branch": "release/v1"}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(updated.target_branch.as_deref(), Some("release/v1"));

    let cleared: Project = serde_json::from_value(
        app.call(
            "project.update",
            json!({"project_id": created.id, "target_branch": ""}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(cleared.target_branch, None);
}
