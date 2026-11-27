use super::*;

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
        trust_tailscale_self: true,
    };

    // Direct TCP RPC over Noise_XX
    for iter in 0..5 {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        let running = app.start_with_listen(Some(listen.clone())).unwrap();
        let addr = app.bound_tcp_addrs().into_iter().next().unwrap();
        let server_pk = app.server_pubkey();
        let client_kp = Arc::new(canban_transport::NoiseKeypair::generate().unwrap());
        app.authorize_device_pubkey("tcp-shutdown-test", &client_kp.public_b64())
            .unwrap();
        let wait_app = Arc::clone(&app);
        let server_thread = std::thread::spawn(move || {
            wait_app.wait_stopped();
            drop(running);
        });

        let reply = client::rpc_tcp(
            &addr,
            Some(&server_pk),
            canban_transport::TcpAuthMode::Key,
            client_kp,
            "app.shutdown",
            json!({}),
        )
        .unwrap_or_else(|e| panic!("direct TCP iteration {iter} failed: {e:#}"));
        assert_eq!(reply, json!({"stopping": true}));
        assert!(app.stopped());
        server_thread.join().unwrap();
    }

    // Noise_XX + yamux/1 multiplexed AppClient RPC
    for iter in 0..5 {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        let running = app.start_with_listen(Some(listen.clone())).unwrap();
        let addr = app.bound_tcp_addrs().into_iter().next().unwrap();
        let server_pk = app.server_pubkey();
        let client_kp = Arc::new(canban_transport::NoiseKeypair::generate().unwrap());
        app.authorize_device_pubkey("yamux-shutdown-test", &client_kp.public_b64())
            .unwrap();
        let wait_app = Arc::clone(&app);
        let server_thread = std::thread::spawn(move || {
            wait_app.wait_stopped();
            drop(running);
        });

        let mux_client = client::AppClient::connect_tcp(
            addr,
            Some(server_pk),
            canban_transport::TcpAuthMode::Key,
            client_kp,
        )
        .unwrap();
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
