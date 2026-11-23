use super::*;
use crate::hosts::HostRegistry;

#[test]
fn tcp_listener_probe_token_auth_and_diagnostics_work_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("remote-home");
    let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let listen = ServerListenConfig {
        tailscale: false,
        port: 0,
        listen: Some("127.0.0.1:0".into()),
        allow_insecure_bind: false,
    };
    let running = app.start_with_listen(Some(listen)).unwrap();
    let bound = app.bound_tcp_addrs();
    assert_eq!(bound.len(), 1);
    let addr = bound[0].clone();
    assert!(
        network::read_server_token(&home).is_none(),
        "new daemons must not auto-create legacy server.token"
    );

    // 1. Port reachability probe succeeds without revealing private workspace entities, while
    //    unauthenticated `host.probe` over TCP is rejected.
    let _rtt_ms = network::probe_tcp_reachable(&addr, Duration::from_secs(2)).unwrap();
    let unauth_probe = client::rpc_tcp(&addr, None, "host.probe", json!({})).unwrap_err();
    assert!(
        format!("{unauth_probe:#}").contains("auth_failed:"),
        "Expected auth_failed on unauthenticated host.probe, got: {unauth_probe:#}"
    );

    // 2. Wrong token is rejected with explicit `auth_failed:`.
    let bad_err = match AppClient::connect_tcp(addr.clone(), "wrong-token".into()) {
        Ok(_) => panic!("Expected invalid token to fail"),
        Err(e) => e,
    };
    assert!(
        format!("{bad_err:#}").contains("auth_failed:"),
        "Expected auth_failed error, got: {bad_err:#}"
    );

    // 3. Valid single-use pairing URI parses, exchanges into a revocable device token over
    //    `transport.mux`, and cannot be replayed.
    let (invite_code, _) = app.create_pairing_code(300).unwrap();
    let uri = format_pairing_uri(&addr, &invite_code);
    let (parsed_addr, parsed_token) = parse_pairing_uri(&uri).unwrap();
    assert_eq!(parsed_addr, addr);
    assert_eq!(parsed_token, invite_code);

    let client = AppClient::connect_tcp(parsed_addr.clone(), parsed_token.clone()).unwrap();
    assert_eq!(client.server_id(), Some(app.server_id()));
    let dev_tok = client
        .upgraded_token()
        .expect("single-use pairing code must exchange for a device token")
        .to_owned();
    assert!(is_device_token(&dev_tok));

    let replay_err = match AppClient::connect_tcp(parsed_addr, parsed_token) {
        Ok(_) => panic!("Replaying a consumed single-use pairing code must fail"),
        Err(e) => e,
    };
    assert!(
        format!("{replay_err:#}").contains("auth_failed:"),
        "Expected auth_failed on pairing code replay, got: {replay_err:#}"
    );

    let cfg = client.config();
    assert!(
        cfg.relay_endpoint
            .as_deref()
            .is_some_and(|e| e.starts_with("mux://")),
        "Expected mux:// relay endpoint, got {:?}",
        cfg.relay_endpoint
    );
    assert_eq!(cfg.relay_token, None);

    // 4. Structured diagnostics status and log tail work over multiplexed TCP.
    std::fs::write(home.join("server.log"), "line one\nline two\nline three\n").unwrap();
    let diag = client.call("diagnostics.status", json!({})).unwrap();
    assert_eq!(diag["server"]["server_id"].as_str(), Some(app.server_id()));
    assert_eq!(diag["server"]["paired_device_count"], 1);
    assert_eq!(diag["server"]["legacy_token_configured"], false);
    assert_eq!(diag["storage"]["healthy"], true);
    assert_eq!(diag["storage"]["journal_mode"], "wal");
    assert_eq!(diag["toolchain"]["git"]["available"], true);

    let logs = client
        .call("diagnostics.logs", json!({"source": "server", "lines": 2}))
        .unwrap();
    assert_eq!(logs["exists"], true);
    assert_eq!(logs["text"].as_str(), Some("line two\nline three"));

    drop(client);
    drop(running);
}

#[test]
fn host_registry_merges_multi_host_routes_mutations_caches_offline_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let local_home = dir.path().join("local-home");
    let remote_home = dir.path().join("remote-home");

    // Start `This Mac` (`local`) with both Unix socket and an ephemeral TCP port (to test loopback guard).
    let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let (_, local_token) = local_app.issue_device_token("local-test").unwrap();
    let local_running = local_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let local_tcp_addr = local_app.bound_tcp_addrs()[0].clone();

    let local_p: Project = serde_json::from_value(
        local_app
            .call(
                "project.create",
                json!({"name": "Local Mac Repo", "directory": dir.path().join("local-repo")}),
            )
            .unwrap(),
    )
    .unwrap();
    let local_c: Card = serde_json::from_value(
        local_app
            .call(
                "card.create",
                json!({"view":"full","project_id": local_p.id, "title": "Local card", "body": "On Mac"}),
            )
            .unwrap(),
    )
    .unwrap();

    // Start remote Linux-like `canband` over TCP.
    let remote_app = AppServer::open(remote_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let (_, remote_token) = remote_app.issue_device_token("remote-test").unwrap();
    let remote_running = remote_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let remote_addr = remote_app.bound_tcp_addrs()[0].clone();
    let remote_port: u16 = remote_addr.rsplit(':').next().unwrap().parse().unwrap();

    let remote_p: Project = serde_json::from_value(
        remote_app
            .call(
                "project.create",
                json!({"name": "Home Linux Repo", "directory": dir.path().join("remote-repo")}),
            )
            .unwrap(),
    )
    .unwrap();
    let remote_c1: Card = serde_json::from_value(
        remote_app
            .call(
                "card.create",
                json!({"view":"full","project_id": remote_p.id, "title": "Remote card 1", "body": "On Linux"}),
            )
            .unwrap(),
    )
    .unwrap();

    // Connect HostRegistry to local Unix socket.
    let local_client = AppClient::connect_home(local_home.clone(), Path::new("/bin/echo")).unwrap();
    let registry = HostRegistry::new(local_client);

    // 1. Loopback guard rejects adding This Mac's own TCP listener as a remote host.
    let loopback_err = registry
        .add_host(SavedHost {
            id: "self-loop".into(),
            name: "Oops Self".into(),
            transport: HostTransport::Tcp {
                addr: local_tcp_addr,
                token: local_token,
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap_err();
    assert!(
        loopback_err
            .to_string()
            .contains("points to This Mac's own Canban service"),
        "Unexpected loopback error: {loopback_err}"
    );

    // 2. Add real remote host and verify unified snapshot + duplicate server_id guard.
    let summary = registry
        .add_host(SavedHost {
            id: "home-linux".into(),
            name: "Home Linux".into(),
            transport: HostTransport::Tcp {
                addr: remote_addr.clone(),
                token: remote_token.clone(),
            },
            ssh_target: Some("jjy@home-linux".into()),
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap();
    assert!(summary.connection.is_connected());

    let dup_err = registry
        .add_host(SavedHost {
            id: "home-linux-dup".into(),
            name: "Home Linux Duplicate".into(),
            transport: HostTransport::Tcp {
                addr: remote_addr.clone(),
                token: remote_token.clone(),
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap_err();
    assert!(dup_err.to_string().contains("already connected"));

    let snap = registry.merged_snapshot().unwrap();
    assert_eq!(snap.projects.len(), 2);
    assert_eq!(snap.cards.len(), 2);
    assert_eq!(snap.project_host(&local_p.id), "local");
    assert_eq!(snap.project_host(&remote_p.id), "home-linux");
    assert_eq!(snap.card_host(&local_c.id), "local");
    assert_eq!(snap.card_host(&remote_c1.id), "home-linux");
    assert_eq!(snap.host_display_name("home-linux"), "Home Linux");
    assert!(!snap.is_host_offline("home-linux"));

    // 3. Routed creation acknowledges the write and ownership immediately; the
    // supervisor refreshes the full remote snapshot separately.
    let remote_c2: Card = serde_json::from_value(
        registry
            .call_routed(
                "card.create",
                json!({"view":"full",
                    "project_id": remote_p.id,
                    "title": "Created via routed client",
                    "body": "Transparently routed to home-linux",
                    "request_key": "routed-card-2"
                }),
                None,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(local_app.snapshot().unwrap().cards.len(), 1);
    assert_eq!(remote_app.snapshot().unwrap().cards.len(), 2);
    let pending_snap = registry.merged_snapshot().unwrap();
    assert_eq!(pending_snap.card_host(&remote_c2.id), "home-linux");
    assert_eq!(
        registry.resolve_target_host("comment.add", &json!({"card_id":remote_c2.id}), None),
        "home-linux"
    );
    registry.poll_remotes_once();
    let snap2 = registry.merged_snapshot().unwrap();
    assert_eq!(snap2.cards.len(), 3);
    assert_eq!(snap2.card_host(&remote_c2.id), "home-linux");

    // Online read-cacheable calls populate the bounded offline cache.
    let initial_tl = registry
        .call_routed(
            "timeline.list",
            json!({"view":"full","card_id": remote_c1.id, "tail": true, "limit": 200}),
            None,
        )
        .unwrap();
    assert!(!initial_tl["events"].as_array().unwrap().is_empty());

    // Add a second remote card to move while offline, plus set worktree on remote_c2 to exercise
    // offline `card.resolve.preview` + `card.resolve`.
    let remote_c_move: Card = serde_json::from_value(
        remote_app
            .call(
                "card.create",
                json!({"view":"full","project_id": remote_p.id, "title": "Move me offline", "body": "Stage move"}),
            )
            .unwrap(),
    )
    .unwrap();
    remote_app
        .store
        .lock()
        .unwrap()
        .write(|db| {
            let mut c: Card = db.get(&remote_c2.id)?;
            c.worktree = Some(dir.path().join("remote-wt").to_string_lossy().into_owned());
            db.save(&c)
        })
        .unwrap();
    registry.poll_remotes_once();

    // 4. Simulate laptop going offline / remote disconnecting: cached snapshot stays visible & editable!
    drop(remote_running);
    drop(remote_app);
    registry.poll_remotes_once();

    let offline_snap = registry.merged_snapshot().unwrap();
    assert!(offline_snap.is_host_offline("home-linux"));
    assert_eq!(
        offline_snap.projects.len(),
        2,
        "Cached remote Project must remain visible while offline"
    );
    assert_eq!(
        offline_snap.cards.len(),
        4,
        "Cached remote Cards must remain visible while offline"
    );

    // Local work continues uninterrupted while remote is offline!
    registry
        .call_routed(
            "card.create",
            json!({"view":"full",
                "project_id": local_p.id,
                "title": "Offline local work",
                "body": "Works on plane/train",
                "request_key": "local-offline-card"
            }),
            None,
        )
        .unwrap();
    assert_eq!(registry.merged_snapshot().unwrap().cards.len(), 5);

    // Non-queueable remote mutations still fail cleanly with host availability message while offline.
    let offline_non_queueable = registry
        .call_routed(
            "terminal.create",
            json!({"project_id": remote_p.id, "directory": "/tmp"}),
            None,
        )
        .unwrap_err();
    assert!(offline_non_queueable.to_string().contains("Home Linux"));

    // Queueable remote mutations succeed optimistically offline!
    let offline_created: Card = serde_json::from_value(
        registry
            .call_routed(
                "card.create",
                json!({"view":"full","project_id": remote_p.id, "title": "Draft offline", "body": "Initial offline body"}),
                None,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(offline_created.number, 0);
    assert_eq!(offline_created.revision, 0);

    // Updating a pending-created card coalesces into the queued create.
    let coalesced: Card = serde_json::from_value(
        registry
            .call_routed(
                "card.update",
                json!({"view":"full","card_id": offline_created.id, "revision": 0, "title": "Created while offline"}),
                None,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(coalesced.title, "Created while offline");

    // Edit title of an existing remote card, add a comment, move another card, and resolve a worktree card.
    let edited_c1 = registry
        .call_routed(
            "card.update",
            json!({"view":"full","card_id": remote_c1.id, "revision": remote_c1.revision, "title": "Remote card 1 edited offline"}),
            None,
        )
        .unwrap();
    assert_eq!(edited_c1["offline_queued"], true);
    assert_eq!(edited_c1["revision"], remote_c1.revision);

    let offline_comment = registry
        .call_routed(
            "comment.add",
            json!({"view":"full","card_id": remote_c1.id, "body": "Queued offline reply"}),
            None,
        )
        .unwrap();
    let offline_comment_id = offline_comment["id"].as_str().unwrap().to_owned();
    assert_eq!(offline_comment["offline_queued"], true);

    // Offline timeline read serves cached page plus the synthetic pending comment with matching client ID.
    let offline_tl = registry
        .call_routed(
            "timeline.list",
            json!({"view":"full","card_id": remote_c1.id, "tail": true, "limit": 200}),
            None,
        )
        .unwrap();
    let offline_events = offline_tl["events"].as_array().unwrap();
    assert!(
        offline_events
            .iter()
            .any(|ev| ev["id"] == offline_comment_id && ev["data"]["pending"] == true),
        "Expected pending comment {offline_comment_id} in cached timeline, got {offline_events:?}"
    );

    registry
        .call_routed(
            "card.move",
            json!({"view":"full","card_id": remote_c_move.id, "revision": remote_c_move.revision, "stage": "develop"}),
            None,
        )
        .unwrap();

    let preview = registry
        .call_routed(
            "card.resolve.preview",
            json!({"card_id": remote_c2.id}),
            None,
        )
        .unwrap();
    assert!(
        preview["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("Home Linux"))
    );
    registry
        .call_routed(
            "card.resolve",
            json!({"view":"full","card_id": remote_c2.id, "revision": remote_c2.revision}),
            None,
        )
        .unwrap();

    // Projected snapshot shows optimistic changes and bounded pending_sync marks.
    let projected = registry.merged_snapshot().unwrap();
    assert_eq!(projected.cards.len(), 6);
    assert_eq!(
        projected.pending_sync.get(&offline_created.id),
        Some(&SyncMark::Pending)
    );
    assert_eq!(
        projected.pending_sync.get(&remote_c1.id),
        Some(&SyncMark::Pending)
    );
    assert_eq!(
        projected
            .cards
            .iter()
            .find(|c| c.id == remote_c_move.id)
            .unwrap()
            .stage,
        "develop"
    );
    assert!(
        projected
            .cards
            .iter()
            .find(|c| c.id == remote_c2.id)
            .unwrap()
            .resolved()
    );
    let hl_sum = registry
        .summaries()
        .into_iter()
        .find(|s| s.id == "home-linux")
        .unwrap();
    assert_eq!(hl_sum.pending_mutations, 5);
    assert_eq!(hl_sum.sync_conflicts, 0);

    // 5. Remote server kept working autonomously while disconnected, then comes back online on the same port!
    let remote_app2 = AppServer::open(remote_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    remote_app2
        .call(
            "card.create",
            json!({"view":"full",
                "project_id": remote_p.id,
                "title": "Autonomous overnight remote card",
                "body": "Created while MacBook slept"
            }),
        )
        .unwrap();
    let remote_running2 = remote_app2
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: remote_port,
            listen: Some(format!("127.0.0.1:{remote_port}")),
            allow_insecure_bind: false,
        }))
        .unwrap();

    registry.reconnect(Some("home-linux"));
    registry.poll_remotes_once();

    // Wait for background drain to replay all 5 queued mutations cleanly.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let sum = registry
            .summaries()
            .into_iter()
            .find(|s| s.id == "home-linux")
            .unwrap();
        if sum.pending_mutations == 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Outbox failed to drain in time; remaining items: {:?}",
            registry.sync_items()
        );
        std::thread::sleep(Duration::from_millis(25));
    }

    let recovered = registry.merged_snapshot().unwrap();
    assert!(!recovered.is_host_offline("home-linux"));
    assert!(recovered.pending_sync.is_empty());
    assert_eq!(recovered.cards.len(), 7);
    assert!(
        recovered
            .cards
            .iter()
            .any(|c| c.title == "Autonomous overnight remote card")
    );
    let synced_created = recovered
        .cards
        .iter()
        .find(|c| c.id == offline_created.id)
        .unwrap();
    assert_eq!(synced_created.title, "Created while offline");
    assert!(synced_created.number > 0);
    assert_eq!(
        recovered
            .cards
            .iter()
            .find(|c| c.id == remote_c1.id)
            .unwrap()
            .title,
        "Remote card 1 edited offline"
    );
    assert_eq!(
        recovered
            .cards
            .iter()
            .find(|c| c.id == remote_c_move.id)
            .unwrap()
            .stage,
        "develop"
    );
    assert!(
        recovered
            .cards
            .iter()
            .find(|c| c.id == remote_c2.id)
            .unwrap()
            .resolved()
    );
    let live_tl = remote_app2
        .call(
            "timeline.list",
            json!({"view":"full","card_id": remote_c1.id, "tail": true, "limit": 50}),
        )
        .unwrap();
    assert!(
        live_tl["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|ev| ev["id"] == offline_comment_id && ev["body"] == "Queued offline reply")
    );

    drop(registry);
    drop(remote_running2);
    drop(local_running);
}

#[test]
fn host_registry_detects_same_field_conflict_and_resolves_via_use_mine() {
    let dir = tempfile::tempdir().unwrap();
    let local_home = dir.path().join("local-home");
    let remote_home = dir.path().join("remote-home");

    let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let _local_running = local_app.start_with_listen(None).unwrap();

    let remote_app = AppServer::open(remote_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let (_, remote_token) = remote_app.issue_device_token("remote-test").unwrap();
    let remote_running = remote_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let remote_addr = remote_app.bound_tcp_addrs()[0].clone();
    let remote_port: u16 = remote_addr.rsplit(':').next().unwrap().parse().unwrap();

    let remote_p: Project = serde_json::from_value(
        remote_app
            .call(
                "project.create",
                json!({"name": "Conflict Repo", "directory": dir.path().join("repo")}),
            )
            .unwrap(),
    )
    .unwrap();
    let card: Card = serde_json::from_value(
        remote_app
            .call(
                "card.create",
                json!({"view":"full","project_id": remote_p.id, "title": "Original", "body": "Body"}),
            )
            .unwrap(),
    )
    .unwrap();

    let local_client = AppClient::connect_home(local_home, Path::new("/bin/echo")).unwrap();
    let registry = HostRegistry::new(local_client);
    registry
        .add_host(SavedHost {
            id: "home-linux".into(),
            name: "Home Linux".into(),
            transport: HostTransport::Tcp {
                addr: remote_addr,
                token: remote_token,
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap();

    // Stop remote, edit title offline.
    drop(remote_running);
    drop(remote_app);
    registry.poll_remotes_once();
    registry
        .call_routed(
            "card.update",
            json!({"view":"full","card_id": card.id, "revision": card.revision, "title": "My offline title", "body": "Body"}),
            None,
        )
        .unwrap();

    // Restart remote and concurrently edit title on the server.
    let remote_app2 = AppServer::open(remote_home, PathBuf::from("/bin/echo")).unwrap();
    remote_app2
        .call(
            "card.update",
            json!({"view":"full","card_id": card.id, "revision": card.revision, "title": "Server concurrent title"}),
        )
        .unwrap();
    let _remote_running2 = remote_app2
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: remote_port,
            listen: Some(format!("127.0.0.1:{remote_port}")),
            allow_insecure_bind: false,
        }))
        .unwrap();

    registry.reconnect(Some("home-linux"));
    registry.poll_remotes_once();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let item = loop {
        let items = registry.sync_items();
        if items.iter().any(|i| i.state == SyncItemState::Conflict) {
            break items
                .into_iter()
                .find(|i| i.state == SyncItemState::Conflict)
                .unwrap();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Expected conflict item"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(item.conflict_fields, vec!["title"]);
    assert_eq!(
        item.mine.get("title").map(String::as_str),
        Some("My offline title")
    );
    assert_eq!(
        item.server.get("title").map(String::as_str),
        Some("Server concurrent title")
    );
    assert_eq!(
        registry
            .merged_snapshot()
            .unwrap()
            .pending_sync
            .get(&card.id),
        Some(&SyncMark::Conflict)
    );

    // Resolve with `UseMine`.
    registry
        .sync_action("home-linux", &item.mutation_id, SyncAction::UseMine)
        .unwrap();
    let deadline2 = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if registry.sync_items().is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline2,
            "Expected conflict to drain after UseMine: {:?}",
            registry.sync_items()
        );
        std::thread::sleep(Duration::from_millis(25));
    }

    let final_card: Card = serde_json::from_value(
        remote_app2
            .call("card.get", json!({"view":"full","card_id": card.id}))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(final_card.title, "My offline title");
}

#[test]
fn remote_host_push_updates_merged_snapshot_without_polling_and_detects_disconnect_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let local_home = dir.path().join("local-home");
    let remote_home = dir.path().join("remote-home");

    let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let local_running = local_app.start_with_listen(None).unwrap();

    let remote_app = AppServer::open(remote_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let (_, remote_token) = remote_app.issue_device_token("remote-test").unwrap();
    let remote_running = remote_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let remote_addr = remote_app.bound_tcp_addrs()[0].clone();

    let remote_p: Project = serde_json::from_value(
        remote_app
            .call(
                "project.create",
                json!({"name": "Push Remote", "directory": dir.path().join("remote-repo")}),
            )
            .unwrap(),
    )
    .unwrap();

    let local_client = AppClient::connect_home(local_home, Path::new("/bin/echo")).unwrap();
    let registry = HostRegistry::new(local_client);
    let events_rx = registry.subscribe();

    registry
        .add_host(SavedHost {
            id: "home-linux".into(),
            name: "Home Linux".into(),
            transport: HostTransport::Tcp {
                addr: remote_addr,
                token: remote_token,
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap();

    // Wait for initial subscribe handshake settle, then drain prior events.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while remote_app.active_tcp_count() == 0 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    while events_rx.try_recv().is_ok() {}

    // Create card directly on remote AppServer WITHOUT calling poll_remotes_once().
    let pushed_card: Card = serde_json::from_value(
        remote_app
            .call(
                "card.create",
                json!({
                    "view": "full",
                    "project_id": remote_p.id,
                    "title": "Pushed without poll",
                    "body": "Instant remote push",
                }),
            )
            .unwrap(),
    )
    .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut saw_remote_event = false;
    loop {
        if let Ok(ev) = events_rx.recv_timeout(Duration::from_millis(50))
            && ev.host_id == "home-linux"
        {
            saw_remote_event = true;
            if registry
                .merged_snapshot()
                .unwrap()
                .cards
                .iter()
                .any(|c| c.id == pushed_card.id && c.title == "Pushed without poll")
            {
                break;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Expected remote push update without polling (saw_remote_event={saw_remote_event})"
        );
    }

    // Drop remote server without calling poll_remotes_once() -> EOF disconnect detected immediately.
    while events_rx.try_recv().is_ok() {}
    drop(remote_running);
    drop(remote_app);

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(ev) = events_rx.recv_timeout(Duration::from_millis(50))
            && ev.host_id == "home-linux"
            && matches!(ev.event, AppEvent::Changed { topic: "host", .. })
            && registry
                .merged_snapshot()
                .unwrap()
                .is_host_offline("home-linux")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Expected immediate offline detection via app.subscribe EOF"
        );
    }

    drop(registry);
    drop(local_running);
}

#[test]
fn stalled_remote_host_does_not_block_healthy_remote_host_push_updates() {
    let dir = tempfile::tempdir().unwrap();
    let local_home = dir.path().join("local-home");
    let remote_home = dir.path().join("healthy-home");

    let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let local_running = local_app.start_with_listen(None).unwrap();

    // Mock hung TCP endpoint that accepts sockets and holds them open without replying.
    let stalled_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    stalled_listener.set_nonblocking(true).unwrap();
    let stalled_addr = stalled_listener.local_addr().unwrap().to_string();
    let stop_stalled = Arc::new(AtomicBool::new(false));
    let stop_stalled_clone = stop_stalled.clone();
    let held_sockets: Arc<Mutex<Vec<std::net::TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
    let held_clone = held_sockets.clone();
    let stalled_thread = std::thread::spawn(move || {
        while !stop_stalled_clone.load(Ordering::SeqCst) {
            match stalled_listener.accept() {
                Ok((s, _)) => held_clone.lock().unwrap().push(s),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(15));
                }
                Err(_) => break,
            }
        }
    });

    // Seed stalled host before creating HostRegistry so its supervisor blocks in connect/handshake.
    let hosts_file = HostsFile {
        hosts: vec![SavedHost {
            id: "stalled-host".into(),
            name: "Stalled Remote".into(),
            transport: HostTransport::Tcp {
                addr: stalled_addr,
                token: "deadbeef".into(),
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        }],
    };
    canban_context::atomic(
        &HostRegistry::hosts_path(&local_home),
        &serde_json::to_vec_pretty(&hosts_file).unwrap(),
    )
    .unwrap();

    let remote_app = AppServer::open(remote_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let (_, remote_token) = remote_app.issue_device_token("remote-test").unwrap();
    let remote_running = remote_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let remote_addr = remote_app.bound_tcp_addrs()[0].clone();
    let remote_p: Project = serde_json::from_value(
        remote_app
            .call(
                "project.create",
                json!({"name": "Healthy Repo", "directory": dir.path().join("healthy-repo")}),
            )
            .unwrap(),
    )
    .unwrap();

    let local_client = AppClient::connect_home(local_home, Path::new("/bin/echo")).unwrap();
    let registry = HostRegistry::new(local_client);

    // Wait briefly until stalled-host has attempted at least one hung TCP connection.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while held_sockets.lock().unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }

    registry
        .add_host(SavedHost {
            id: "healthy-remote".into(),
            name: "Healthy Remote".into(),
            transport: HostTransport::Tcp {
                addr: remote_addr,
                token: remote_token,
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while remote_app.active_tcp_count() == 0 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }

    let card: Card = serde_json::from_value(
        remote_app
            .call(
                "card.create",
                json!({
                    "view": "full",
                    "project_id": remote_p.id,
                    "title": "Healthy host isolated",
                    "body": "Stalled host never blocks me",
                }),
            )
            .unwrap(),
    )
    .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if registry
            .merged_snapshot()
            .unwrap()
            .cards
            .iter()
            .any(|c| c.id == card.id)
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Healthy remote push update should arrive while stalled-host is hung"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // Release hung sockets before dropping HostRegistry so shutdown finishes immediately.
    stop_stalled.store(true, Ordering::SeqCst);
    held_sockets.lock().unwrap().clear();
    let _ = stalled_thread.join();

    drop(registry);
    drop(remote_running);
    drop(local_running);
}

#[test]
fn host_registry_lifecycle_closes_remote_push_connections_and_forwards_local_side_channels() {
    let dir = tempfile::tempdir().unwrap();
    let local_home = dir.path().join("local-home");
    let remote_home = dir.path().join("remote-home");

    let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let local_running = local_app.start_with_listen(None).unwrap();

    let remote_app = AppServer::open(remote_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let (_, remote_token) = remote_app.issue_device_token("remote-test").unwrap();
    let remote_running = remote_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let remote_addr = remote_app.bound_tcp_addrs()[0].clone();

    let local_client = AppClient::connect_home(local_home, Path::new("/bin/echo")).unwrap();
    let registry = HostRegistry::new(local_client);
    let events_rx = registry.subscribe();
    let mut push_rx = registry.subscribe_push();

    // Wait for local push supervisor initial handshake ("local" resync).
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(ev) = events_rx.recv_timeout(Duration::from_millis(50))
            && ev.host_id == "local"
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for local push supervisor handshake"
        );
    }

    // Side-channel push from local AppServer flows through local_push_supervisor -> HostPushEvent.
    local_app.emit_utilities();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match push_rx.try_recv() {
            Ok(crate::hosts::HostPushEvent::UtilitiesChanged { host_id, state }) => {
                assert_eq!(host_id, "local");
                assert!(state.is_object());
                break;
            }
            Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "Expected HostPushEvent::UtilitiesChanged from local push stream"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("Unexpected push_rx error: {e:?}"),
        }
    }

    let host_cfg = SavedHost {
        id: "home-linux".into(),
        name: "Home Linux".into(),
        transport: HostTransport::Tcp {
            addr: remote_addr,
            token: remote_token,
        },
        ssh_target: None,
        server_id: None,
        enabled: true,
        added_at: now(),
    };
    registry.add_host(host_cfg.clone()).unwrap();

    let wait_active = |min_count: usize| {
        let dl = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let cur = remote_app.active_tcp_count();
            if (min_count == 0 && cur == 0) || (min_count > 0 && cur >= min_count) {
                break;
            }
            assert!(
                std::time::Instant::now() < dl,
                "Timed out waiting for active_tcp_count condition {min_count} (got {cur})"
            );
            std::thread::sleep(Duration::from_millis(15));
        }
    };

    wait_active(1);

    registry.set_host_enabled("home-linux", false).unwrap();
    wait_active(0);

    registry.set_host_enabled("home-linux", true).unwrap();
    wait_active(1);

    registry.remove_host("home-linux").unwrap();
    wait_active(0);

    registry.add_host(host_cfg).unwrap();
    wait_active(1);

    drop(registry);
    wait_active(0);

    drop(remote_running);
    drop(local_running);
}

#[test]
fn idle_remote_host_receives_heartbeats_without_polling_app_state() {
    use std::sync::atomic::AtomicUsize;

    let dir = tempfile::tempdir().unwrap();
    let local_home = dir.path().join("local-home");
    let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let local_running = local_app.start_with_listen(None).unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let token = "test-remote-token";

    let state_calls = Arc::new(AtomicUsize::new(0));
    let state_calls_srv = state_calls.clone();
    let stop_srv = Arc::new(AtomicBool::new(false));
    let stop_srv_clone = stop_srv.clone();
    let (sub_open_tx, sub_open_rx) = mpsc::channel::<()>();
    let (hb_tx, hb_rx) = mpsc::channel::<()>();
    let (state_call_tx, state_call_rx) = mpsc::channel::<usize>();
    let (push_trigger_tx, push_trigger_rx) = mpsc::channel::<u64>();
    let push_trigger_rx = Arc::new(Mutex::new(push_trigger_rx));
    let cur_change = Arc::new(AtomicU64::new(1));
    let cur_change_srv = cur_change.clone();

    let server_thread = std::thread::spawn(move || {
        let mut workers = Vec::new();
        while !stop_srv_clone.load(Ordering::SeqCst) {
            let Ok((stream, _)) = listener.accept() else {
                break;
            };
            if stop_srv_clone.load(Ordering::SeqCst) {
                break;
            }
            let state_calls_w = state_calls_srv.clone();
            let stop_w = stop_srv_clone.clone();
            let sub_open_tx_w = sub_open_tx.clone();
            let hb_tx_w = hb_tx.clone();
            let state_call_tx_w = state_call_tx.clone();
            let push_trigger_rx_w = push_trigger_rx.clone();
            let cur_change_w = cur_change_srv.clone();
            workers.push(std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut writer = stream;
                let snap_val = serde_json::to_value(Snapshot::default()).unwrap();
                let cfg_val = serde_json::to_value(Config::default()).unwrap();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let req: Value = serde_json::from_str(&line).unwrap_or_default();
                    let id = req["id"].clone();
                    match req["method"].as_str().unwrap_or("") {
                        "app.ping" => {
                            let _ = writeln!(
                                writer,
                                "{}",
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "result": {
                                        "api_version": API_VERSION,
                                        "server_id": "srv-mock-push",
                                        "build": {
                                            "version": env!("CARGO_PKG_VERSION"),
                                            "api": API_VERSION,
                                            "platform": "linux-x86_64"
                                        }
                                    }
                                })
                            );
                            let _ = writer.flush();
                        }
                        "app.state" => {
                            let count = state_calls_w.fetch_add(1, Ordering::SeqCst) + 1;
                            let _ = state_call_tx_w.send(count);
                            let rev_num = cur_change_w.load(Ordering::SeqCst);
                            let rev = json!(["srv-mock-push", "inst-1", rev_num, 0, 0]);
                            let changed = req["params"]["after"] != rev;
                            let mut res = json!({
                                "revision": rev,
                                "changed": changed,
                                "server_id": "srv-mock-push",
                                "github_revision": 0,
                            });
                            if changed {
                                res["snapshot"] = snap_val.clone();
                                res["config"] = cfg_val.clone();
                                res["home"] = json!("/tmp/mock-remote");
                                res["hostname"] = json!("mock-remote");
                            }
                            let _ = writeln!(
                                writer,
                                "{}",
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "result": res,
                                })
                            );
                            let _ = writer.flush();
                        }
                        "app.subscribe" => {
                            let rev_num = cur_change_w.load(Ordering::SeqCst);
                            let rev = json!(["srv-mock-push", "inst-1", rev_num, 0, 0]);
                            let changed = req["params"]["after"] != rev;
                            let _ = writeln!(
                                writer,
                                "{}",
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "result": {
                                        "subscribed": true,
                                        "server_id": "srv-mock-push",
                                        "api_version": API_VERSION,
                                        "revision": rev,
                                        "changed": changed,
                                        "github_revision": 0,
                                        "heartbeat_ms": 80,
                                    }
                                })
                            );
                            let _ = writer.flush();
                            let _ = sub_open_tx_w.send(());
                            while !stop_w.load(Ordering::SeqCst) {
                                let sent_heartbeat = if let Ok(new_rev) = push_trigger_rx_w
                                    .lock()
                                    .unwrap()
                                    .recv_timeout(Duration::from_millis(80))
                                {
                                    let _ = writeln!(
                                        writer,
                                        "{}",
                                        json!({
                                            "jsonrpc": "2.0",
                                            "method": "event",
                                            "params": {
                                                "seq": 1,
                                                "revision": ["srv-mock-push", "inst-1", new_rev, 0, 0],
                                                "topic": "application",
                                                "card_id": Value::Null,
                                                "github_revision": 0,
                                            }
                                        })
                                    );
                                    false
                                } else {
                                    let _ = writeln!(
                                        writer,
                                        "{}",
                                        json!({
                                            "jsonrpc": "2.0",
                                            "method": "heartbeat",
                                            "params": { "ts": now(), "server_id": "srv-mock-push" }
                                        })
                                    );
                                    true
                                };
                                if writer.flush().is_err() {
                                    break;
                                }
                                if sent_heartbeat {
                                    let _ = hb_tx_w.send(());
                                }
                            }
                            break;
                        }
                        _ => break,
                    }
                }
            }));
        }
        for w in workers {
            let _ = w.join();
        }
    });

    let local_client = AppClient::connect_home(local_home, Path::new("/bin/echo")).unwrap();
    let registry = HostRegistry::new(local_client);

    registry
        .add_host(SavedHost {
            id: "mock-remote".into(),
            name: "Mock Remote".into(),
            transport: HostTransport::Tcp {
                addr: addr.clone(),
                token: token.into(),
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap();

    sub_open_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("app.subscribe should connect");
    let baseline = state_calls.load(Ordering::SeqCst);

    // Stay idle across 4 explicit heartbeats: ZERO `app.state` calls may occur while idle.
    while hb_rx.try_recv().is_ok() {}
    for _ in 0..4 {
        hb_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("expected idle heartbeat");
    }
    assert_eq!(
        state_calls.load(Ordering::SeqCst),
        baseline,
        "Idle remote host should not poll app.state"
    );

    // Push a state invalidation event and verify exactly one conditional `app.state` follows.
    while state_call_rx.try_recv().is_ok() {}
    cur_change.store(2, Ordering::SeqCst);
    push_trigger_tx.send(2).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for conditional app.state"
        );
        if state_call_rx.recv_timeout(remaining).unwrap() > baseline {
            break;
        }
    }
    while hb_rx.try_recv().is_ok() {}
    for _ in 0..2 {
        hb_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("expected post-push heartbeat");
    }
    assert_eq!(state_calls.load(Ordering::SeqCst), baseline + 1);

    stop_srv.store(true, Ordering::SeqCst);
    drop(registry);
    let _ = std::net::TcpStream::connect(&addr);
    let _ = server_thread.join();
    drop(local_running);
}

#[test]
fn remote_host_supervisor_forwards_usage_changed_side_channel() {
    let dir = tempfile::tempdir().unwrap();
    let local_home = dir.path().join("local-home");
    let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let local_running = local_app.start_with_listen(None).unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let stop_srv = Arc::new(AtomicBool::new(false));
    let stop_srv_clone = stop_srv.clone();
    let (sub_topics_tx, sub_topics_rx) = mpsc::channel::<Vec<String>>();

    let server_thread = std::thread::spawn(move || {
        let mut workers = Vec::new();
        while !stop_srv_clone.load(Ordering::SeqCst) {
            let Ok((stream, _)) = listener.accept() else {
                break;
            };
            if stop_srv_clone.load(Ordering::SeqCst) {
                break;
            }
            let stop_w = stop_srv_clone.clone();
            let sub_tx_w = sub_topics_tx.clone();
            workers.push(std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut writer = stream;
                let snap_val = serde_json::to_value(Snapshot::default()).unwrap();
                let cfg_val = serde_json::to_value(Config::default()).unwrap();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let req: Value = serde_json::from_str(&line).unwrap_or_default();
                    let id = req["id"].clone();
                    match req["method"].as_str().unwrap_or("") {
                        "app.ping" => {
                            let _ = writeln!(
                                writer,
                                "{}",
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "result": {
                                        "api_version": API_VERSION,
                                        "server_id": "srv-mock-usage",
                                        "build": {
                                            "version": env!("CARGO_PKG_VERSION"),
                                            "api": API_VERSION,
                                            "platform": "linux-x86_64"
                                        }
                                    }
                                })
                            );
                            let _ = writer.flush();
                        }
                        "app.state" => {
                            let rev = json!(["srv-mock-usage", "inst-1", 1, 0, 0]);
                            let changed = req["params"]["after"] != rev;
                            let mut res = json!({
                                "revision": rev,
                                "changed": changed,
                                "server_id": "srv-mock-usage",
                                "github_revision": 0,
                            });
                            if changed {
                                res["snapshot"] = snap_val.clone();
                                res["config"] = cfg_val.clone();
                                res["home"] = json!("/tmp/mock-usage");
                                res["hostname"] = json!("mock-usage");
                            }
                            let _ = writeln!(
                                writer,
                                "{}",
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "result": res,
                                })
                            );
                            let _ = writer.flush();
                        }
                        "app.subscribe" => {
                            let topics: Vec<String> = req["params"]["topics"]
                                .as_array()
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(Value::as_str)
                                        .map(str::to_owned)
                                        .collect()
                                })
                                .unwrap_or_default();
                            let _ = sub_tx_w.send(topics);
                            let rev = json!(["srv-mock-usage", "inst-1", 1, 0, 0]);
                            let _ = writeln!(
                                writer,
                                "{}",
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "result": {
                                        "subscribed": true,
                                        "server_id": "srv-mock-usage",
                                        "api_version": API_VERSION,
                                        "revision": rev,
                                        "changed": false,
                                        "github_revision": 0,
                                        "heartbeat_ms": 100,
                                    }
                                })
                            );
                            let _ = writeln!(
                                writer,
                                "{}",
                                json!({
                                    "jsonrpc": "2.0",
                                    "method": "usage.changed",
                                    "params": {
                                        "seq": 1,
                                        "state": {
                                            "readings": [{"provider": "codex", "used": 42}],
                                            "busy": false,
                                        }
                                    }
                                })
                            );
                            let _ = writer.flush();
                            while !stop_w.load(Ordering::SeqCst) {
                                std::thread::sleep(Duration::from_millis(25));
                            }
                            break;
                        }
                        _ => break,
                    }
                }
            }));
        }
        for w in workers {
            let _ = w.join();
        }
    });

    let local_client = AppClient::connect_home(local_home, Path::new("/bin/echo")).unwrap();
    let registry = HostRegistry::new(local_client);
    let mut push_rx = registry.subscribe_push();

    registry
        .add_host(SavedHost {
            id: "remote-usage".into(),
            name: "Remote Usage".into(),
            transport: HostTransport::Tcp {
                addr: addr.clone(),
                token: "test-token".into(),
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap();

    let subscribed_topics = sub_topics_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(subscribed_topics.iter().any(|t| t == "snapshot"));
    assert!(subscribed_topics.iter().any(|t| t == "usage"));

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match push_rx.try_recv() {
            Ok(crate::hosts::HostPushEvent::UsageChanged { host_id, state })
                if host_id == "remote-usage" =>
            {
                assert_eq!(state["readings"][0]["provider"], "codex");
                assert_eq!(state["readings"][0]["used"], 42);
                break;
            }
            Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "Expected HostPushEvent::UsageChanged for remote-usage"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("Unexpected push_rx error: {e:?}"),
        }
    }

    stop_srv.store(true, Ordering::SeqCst);
    drop(registry);
    let _ = std::net::TcpStream::connect(&addr);
    let _ = server_thread.join();
    drop(local_running);
}

#[cfg(unix)]
fn write_echo_mock_hc(path: &Path, core_id: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(
        path,
        format!(
            r#"#!/usr/bin/python3
import json, sys
first = sys.stdin.buffer.readline()
if not first:
    sys.exit(0)
try:
    msg = json.loads(first.decode())
except Exception:
    msg = None
if isinstance(msg, dict) and msg.get("method") == "hello":
    def handle(req):
        m = req.get("method")
        if m == "hello":
            res = {{}}
        elif m == "server.status":
            res = {{"core": {{"id": "{core_id}"}}}}
        elif m == "events.subscribe":
            res = {{"seq": 1, "terminals": []}}
        else:
            res = {{}}
        sys.stdout.write(json.dumps({{"jsonrpc": "2.0", "id": req["id"], "result": res}}) + "\n")
        sys.stdout.flush()
    handle(msg)
    for raw in sys.stdin.buffer:
        line = raw.decode().strip()
        if line:
            handle(json.loads(line))
else:
    sys.stdout.buffer.write(first)
    sys.stdout.buffer.flush()
    while True:
        chunk = sys.stdin.buffer.read1(4096)
        if not chunk:
            break
        sys.stdout.buffer.write(chunk)
        sys.stdout.buffer.flush()
"#
        ),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn single_tcp_yamux_multiplexes_rpc_subscribe_and_multiple_herdr_relays() {
    let dir = tempfile::tempdir().unwrap();
    let local_home = dir.path().join("local-home");
    let remote_home = dir.path().join("remote-home");
    let core_id = format!("core-mux-{}", id());
    let mock_hc = dir.path().join("mock-hc");
    write_echo_mock_hc(&mock_hc, &core_id);

    let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let _local_running = local_app.start_with_listen(None).unwrap();

    let remote_app = AppServer::open(remote_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    remote_app.config.write().unwrap().hc = mock_hc.to_string_lossy().into_owned();
    let (_, remote_token) = remote_app.issue_device_token("mux-workstation").unwrap();
    let remote_running = remote_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let remote_addr = remote_app.bound_tcp_addrs()[0].clone();
    let remote_proj: Project = serde_json::from_value(
        remote_app
            .call(
                "project.create",
                json!({"name": "Mux Repo", "directory": dir.path().join("mux-repo")}),
            )
            .unwrap(),
    )
    .unwrap();

    let local_client = AppClient::connect_home(local_home, Path::new("/bin/echo")).unwrap();
    let registry = HostRegistry::new(local_client);
    registry
        .add_host(SavedHost {
            id: "home-linux".into(),
            name: "Home Linux".into(),
            transport: HostTransport::Tcp {
                addr: remote_addr,
                token: remote_token,
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while remote_app.active_tcp_count() == 0 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(remote_app.active_tcp_count(), 1);

    let remote_client = registry.client_for("home-linux").unwrap();
    let relay_cfg = remote_client.config();
    assert!(
        relay_cfg
            .relay_endpoint
            .as_deref()
            .is_some_and(|e| e.starts_with("mux://")),
        "expected mux:// relay endpoint, got {:?}",
        relay_cfg.relay_endpoint
    );

    // Open TWO concurrent herdr.connect terminal relays over the single TCP yamux session.
    let term_1 = format!("term-mux-1-{}", id());
    let term_2 = format!("term-mux-2-{}", id());
    let mut tr1 =
        canban_herdr::transport::connect_view_fenced(&relay_cfg, Some(&term_1), None).unwrap();
    let mut tr2 =
        canban_herdr::transport::connect_view_fenced(&relay_cfg, Some(&term_2), None).unwrap();
    assert_eq!(canban_herdr::activity::get(&core_id, &term_1).viewers, 1);
    assert_eq!(canban_herdr::activity::get(&core_id, &term_2).viewers, 1);
    assert_eq!(
        remote_app.active_tcp_count(),
        1,
        "two relays + subscribe + RPC must share exactly 1 TCP connection"
    );

    // Perform concurrent RPCs while both terminal streams and app.subscribe are active.
    let mut rpc_handles = Vec::new();
    for i in 0..4 {
        let rc = remote_client.clone();
        let pid = remote_proj.id.clone();
        rpc_handles.push(std::thread::spawn(move || {
            let ping = rc.call("app.ping", json!({})).unwrap();
            assert!(!ping["server_id"].as_str().unwrap().is_empty());
            rc.call(
                "card.create",
                json!({
                    "project_id": pid,
                    "title": format!("Mux card {i}"),
                    "body": "Over yamux"
                }),
            )
            .unwrap()
        }));
    }

    let mut w1 = tr1.writer.take().unwrap();
    let mut r1 = tr1.reader.take().unwrap();
    let mut w2 = tr2.writer.take().unwrap();
    let mut r2 = tr2.reader.take().unwrap();
    let p1 = b"stream-one-payload\n";
    let p2 = b"stream-two-payload\n";
    w1.write_all(p1).unwrap();
    w1.flush().unwrap();
    w2.write_all(p2).unwrap();
    w2.flush().unwrap();

    let mut buf1 = vec![0u8; p1.len()];
    let mut buf2 = vec![0u8; p2.len()];
    r1.read_exact(&mut buf1).unwrap();
    r2.read_exact(&mut buf2).unwrap();
    assert_eq!(&buf1, p1);
    assert_eq!(&buf2, p2);

    for h in rpc_handles {
        h.join().unwrap();
    }
    assert_eq!(remote_app.active_tcp_count(), 1);

    // Close both terminal relays: viewer counts return to 0 while the shared TCP link stays open.
    drop(w1);
    drop(r1);
    drop(tr1);
    drop(w2);
    drop(r2);
    drop(tr2);
    let dl = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < dl {
        if canban_herdr::activity::get(&core_id, &term_1).viewers == 0
            && canban_herdr::activity::get(&core_id, &term_2).viewers == 0
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(canban_herdr::activity::get(&core_id, &term_1).viewers, 0);
    assert_eq!(canban_herdr::activity::get(&core_id, &term_2).viewers, 0);
    assert_eq!(remote_app.active_tcp_count(), 1);

    drop(remote_client);
    registry.set_host_enabled("home-linux", false).unwrap();
    let dl2 = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < dl2 && remote_app.active_tcp_count() > 0 {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(remote_app.active_tcp_count(), 0);
    drop(registry);
    drop(remote_running);
}

#[cfg(unix)]
#[test]
fn revoking_paired_device_immediately_disconnects_mux_session_and_relays() {
    let dir = tempfile::tempdir().unwrap();
    let remote_home = dir.path().join("remote-home");
    let core_id = format!("core-rev-{}", id());
    let mock_hc = dir.path().join("mock-hc");
    write_echo_mock_hc(&mock_hc, &core_id);

    let remote_app = AppServer::open(remote_home, PathBuf::from("/bin/echo")).unwrap();
    remote_app.config.write().unwrap().hc = mock_hc.to_string_lossy().into_owned();
    let (dev, dev_token) = remote_app.issue_device_token("Revokable Mac").unwrap();
    let remote_running = remote_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let addr = remote_app.bound_tcp_addrs()[0].clone();

    let client = AppClient::connect_tcp(addr.clone(), dev_token.clone()).unwrap();
    assert_eq!(remote_app.active_tcp_count(), 1);
    let term_id = format!("term-rev-{}", id());
    let cfg = client.config();
    let _tr = canban_herdr::transport::connect_view_fenced(&cfg, Some(&term_id), None).unwrap();
    assert_eq!(canban_herdr::activity::get(&core_id, &term_id).viewers, 1);

    // Revoke the active device: TCP mux session + child herdr relay must terminate immediately.
    let revoked = remote_app
        .call("device.revoke", json!({"device_id": dev.id}))
        .unwrap();
    assert_eq!(revoked["revoked"], true);

    let dl = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < dl {
        if remote_app.active_tcp_count() == 0
            && canban_herdr::activity::get(&core_id, &term_id).viewers == 0
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(remote_app.active_tcp_count(), 0);
    assert_eq!(canban_herdr::activity::get(&core_id, &term_id).viewers, 0);

    // Reconnecting with the revoked token is rejected.
    let err = match AppClient::connect_tcp(addr, dev_token) {
        Ok(_) => panic!("Expected revoked device token to be rejected"),
        Err(e) => e,
    };
    assert!(format!("{err:#}").contains("auth_failed:"));

    drop(client);
    drop(remote_running);
}

#[test]
fn pairing_code_and_legacy_server_token_upgrade_persist_in_hosts_json_and_remove_legacy_works() {
    let dir = tempfile::tempdir().unwrap();
    let local_home = dir.path().join("local-home");
    let legacy_remote_home = dir.path().join("legacy-remote");
    let invite_remote_home = dir.path().join("invite-remote");

    let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let _local_running = local_app.start_with_listen(None).unwrap();

    // Legacy remote daemon with deprecated `server.token`.
    let legacy_app =
        AppServer::open(legacy_remote_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let legacy_token = network::ensure_server_token(&legacy_remote_home).unwrap();
    let legacy_running = legacy_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let legacy_addr = legacy_app.bound_tcp_addrs()[0].clone();

    // New remote daemon using single-use pairing code.
    let invite_app =
        AppServer::open(invite_remote_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let invite_running = invite_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let invite_addr = invite_app.bound_tcp_addrs()[0].clone();
    let (pairing_code, _) = invite_app.create_pairing_code(300).unwrap();

    let local_client = AppClient::connect_home(local_home.clone(), Path::new("/bin/echo")).unwrap();
    let registry = HostRegistry::new(local_client);

    registry
        .add_host(SavedHost {
            id: "legacy-host".into(),
            name: "Legacy Host".into(),
            transport: HostTransport::Tcp {
                addr: legacy_addr,
                token: legacy_token,
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap();

    registry
        .add_host(SavedHost {
            id: "invite-host".into(),
            name: "Invite Host".into(),
            transport: HostTransport::Tcp {
                addr: invite_addr,
                token: pairing_code,
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap();

    // Verify hosts.json stores revocable dev_... tokens (never legacy token or raw pair_ code).
    let persisted: HostsFile =
        serde_json::from_slice(&std::fs::read(HostRegistry::hosts_path(&local_home)).unwrap())
            .unwrap();
    for h in &persisted.hosts {
        let HostTransport::Tcp { token, .. } = &h.transport else {
            panic!("expected TCP transport for {}", h.id);
        };
        assert!(
            is_device_token(token),
            "expected dev_... token persisted for {}, got {token}",
            h.id
        );
    }

    // Remove deprecated `server.token` on legacy remote and verify reconnect continues using dev_...
    let rm_res = legacy_app
        .call("device.remove_legacy_token", json!({}))
        .unwrap();
    assert_eq!(rm_res["removed"], true);
    assert!(network::read_server_token(&legacy_remote_home).is_none());

    registry.reconnect(None);
    registry.poll_remotes_once();
    for s in registry.summaries() {
        assert!(
            s.is_connected(),
            "expected {} to reconnect using upgraded dev_... token, got {:?}",
            s.id,
            s.connection
        );
    }

    drop(registry);
    drop(legacy_running);
    drop(invite_running);
}

#[test]
fn global_env_replicates_via_outbox_with_coalescing_tombstones_lww_and_zero_plaintext_leaks() {
    use canban_types::{EnvEntryMeta, EnvWriteEntry};
    use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};

    let dir = tempfile::tempdir().unwrap();
    let local_home = dir.path().join("local-home");
    let remote_home = dir.path().join("remote-home");

    let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let _local_running = local_app.start_with_listen(None).unwrap();

    let remote_app = AppServer::open(remote_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let (_, remote_token) = remote_app.issue_device_token("remote-test").unwrap();
    assert!(
        network::read_server_token(&remote_home).is_none(),
        "new daemons must not auto-create legacy server.token"
    );
    let remote_running = remote_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let remote_addr = remote_app.bound_tcp_addrs()[0].clone();
    let remote_port: u16 = remote_addr.rsplit(':').next().unwrap().parse().unwrap();

    let local_client = AppClient::connect_home(local_home.clone(), Path::new("/bin/echo")).unwrap();
    let registry = HostRegistry::new(local_client);
    registry
        .add_host(SavedHost {
            id: "home-linux".into(),
            name: "Home Linux".into(),
            transport: HostTransport::Tcp {
                addr: remote_addr.clone(),
                token: remote_token.clone(),
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap();

    // 1. Connected save replicates Global Plain + Secret over TCP and re-encrypts under remote vault.key.
    let sec_v1 = "multi_global_secret_v1_alpha";
    registry
        .call_routed(
            "env.save",
            json!({
                "scope": "global",
                "entries": BTreeMap::<String, EnvWriteEntry>::from([
                    ("G_PLAIN".into(), EnvWriteEntry::Plain { value: "initial".into() }),
                    ("TEMP_DEL".into(), EnvWriteEntry::Plain { value: "temporary".into() }),
                    (
                        "G_SECRET".into(),
                        EnvWriteEntry::Secret {
                            value: Some(sec_v1.into()),
                            keep_existing: false,
                        },
                    ),
                ]),
            }),
            None,
        )
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if registry.global_env_sync_summary().pending_hosts.is_empty()
            && remote_app
                .snapshot()
                .unwrap()
                .global_env
                .contains_key("G_SECRET")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for initial global env sync"
        );
        std::thread::sleep(Duration::from_millis(25));
    }

    // Distinct vault.key files with 0600 permissions on each host.
    let local_key = std::fs::read(local_home.join("vault.key")).unwrap();
    let remote_key = std::fs::read(remote_home.join("vault.key")).unwrap();
    assert_ne!(local_key, remote_key);
    assert_eq!(
        std::fs::metadata(remote_home.join("vault.key"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let remote_resolved1 = remote_app
        .resolve_runtime_env(None, None, &BTreeMap::new(), None, Some("local"))
        .unwrap();
    assert_eq!(
        remote_resolved1.env.get("G_SECRET").map(String::as_str),
        Some(sec_v1)
    );

    // 2. Take remote offline and make 3 rapid global edits: coalesce to 1 pointer-only outbox op.
    drop(remote_running);
    drop(remote_app);
    registry.poll_remotes_once();

    let sec_v2 = "multi_global_secret_v2_beta";
    let sec_v3 = "multi_global_secret_v3_gamma";
    registry
        .call_routed(
            "env.save",
            json!({
                "scope": "global",
                "entries": BTreeMap::<String, EnvWriteEntry>::from([
                    ("G_PLAIN".into(), EnvWriteEntry::Plain { value: "offline_1".into() }),
                    ("TEMP_DEL".into(), EnvWriteEntry::Plain { value: "temporary".into() }),
                    ("LWW_KEY".into(), EnvWriteEntry::Plain { value: "local_older".into() }),
                    (
                        "G_SECRET".into(),
                        EnvWriteEntry::Secret {
                            value: Some(sec_v2.into()),
                            keep_existing: false,
                        },
                    ),
                ]),
            }),
            None,
        )
        .unwrap();
    registry
        .call_routed(
            "env.save",
            json!({
                "scope": "global",
                "entries": BTreeMap::<String, EnvWriteEntry>::from([
                    ("G_PLAIN".into(), EnvWriteEntry::Plain { value: "offline_2".into() }),
                    ("LWW_KEY".into(), EnvWriteEntry::Plain { value: "local_older".into() }),
                    (
                        "G_SECRET".into(),
                        EnvWriteEntry::Secret {
                            value: Some(sec_v3.into()),
                            keep_existing: false,
                        },
                    ),
                ]),
            }),
            None,
        )
        .unwrap();

    let summary_offline = registry.global_env_sync_summary();
    assert_eq!(summary_offline.pending_hosts, vec!["Home Linux"]);
    assert!(
        registry.sync_items().is_empty(),
        "Global sync must stay out of card pending counts"
    );

    // Outbox file must be coalesced to 1 SyncGlobalEnv entry and contain zero plaintext secrets.
    let outbox_path = local_home.join("cache/hosts/home-linux.outbox.json");
    let outbox_bytes = std::fs::read_to_string(&outbox_path).unwrap();
    let parsed_ob = canban_sync::Outbox::load(outbox_path);
    assert_eq!(
        parsed_ob
            .ops
            .iter()
            .filter(|m| m.kind == canban_sync::MutationKind::SyncGlobalEnv)
            .count(),
        1
    );
    let cache_bytes =
        std::fs::read_to_string(local_home.join("cache/hosts/home-linux.json")).unwrap_or_default();
    for s in [sec_v1, sec_v2, sec_v3] {
        assert!(
            !outbox_bytes.contains(s),
            "Secret sentinel leaked in .outbox.json"
        );
        assert!(
            !cache_bytes.contains(s),
            "Secret sentinel leaked in cached host snapshot"
        );
    }

    // 3. Restart remote with a newer LWW_KEY write on remote DB, then reconnect & drain.
    let remote_app2 = AppServer::open(remote_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    remote_app2
        .call(
            "env.save",
            json!({
                "scope": "global",
                "entries": BTreeMap::<String, EnvWriteEntry>::from([
                    ("G_PLAIN".into(), EnvWriteEntry::Plain { value: "initial".into() }),
                    ("TEMP_DEL".into(), EnvWriteEntry::Plain { value: "temporary".into() }),
                    ("LWW_KEY".into(), EnvWriteEntry::Plain { value: "remote_newer_wins".into() }),
                    (
                        "G_SECRET".into(),
                        EnvWriteEntry::Secret {
                            value: Some(sec_v1.into()),
                            keep_existing: false,
                        },
                    ),
                ]),
            }),
        )
        .unwrap();
    remote_app2
        .store
        .lock()
        .unwrap()
        .write(|db| {
            db.conn.execute(
                "UPDATE env_entries SET updated_at = 1 WHERE scope = 'global' AND key != 'LWW_KEY'",
                [],
            )?;
            db.conn.execute(
                "UPDATE env_entries SET updated_at = updated_at + 60000 WHERE scope = 'global' AND key = 'LWW_KEY'",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let _remote_running2 = remote_app2
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: remote_port,
            listen: Some(format!("127.0.0.1:{remote_port}")),
            allow_insecure_bind: false,
        }))
        .unwrap();
    registry.reconnect(Some("home-linux"));
    registry.poll_remotes_once();

    let deadline2 = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if registry.global_env_sync_summary().pending_hosts.is_empty()
            && remote_app2.snapshot().unwrap().global_env.get("G_PLAIN")
                == Some(&EnvEntryMeta::Plain {
                    value: "offline_2".into(),
                })
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline2,
            "Timed out waiting for offline global env sync drain"
        );
        std::thread::sleep(Duration::from_millis(25));
    }

    let remote_snap = remote_app2.snapshot().unwrap();
    assert!(
        !remote_snap.global_env.contains_key("TEMP_DEL"),
        "Tombstone must delete TEMP_DEL"
    );
    assert_eq!(
        remote_snap.global_env.get("LWW_KEY"),
        Some(&EnvEntryMeta::Plain {
            value: "remote_newer_wins".into(),
        })
    );
    let remote_resolved2 = remote_app2
        .resolve_runtime_env(None, None, &BTreeMap::new(), None, Some("local"))
        .unwrap();
    assert_eq!(
        remote_resolved2.env.get("G_SECRET").map(String::as_str),
        Some(sec_v3)
    );
    assert_eq!(
        remote_resolved2.env.get("G_PLAIN").map(String::as_str),
        Some("offline_2")
    );
}

#[test]
fn remote_agent_and_project_env_save_seal_over_tcp_and_isolate_across_hosts() {
    use canban_types::{EnvEntryMeta, EnvWriteEntry};
    use std::collections::BTreeMap;

    let dir = tempfile::tempdir().unwrap();
    let local_home = dir.path().join("local-home");
    let remote_home = dir.path().join("remote-home");
    let legacy_home = dir.path().join("legacy-home");

    let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let _local_running = local_app.start_with_listen(None).unwrap();

    let remote_app = AppServer::open(remote_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let (_, remote_token) = remote_app.issue_device_token("remote-test").unwrap();
    assert!(
        network::read_server_token(&remote_home).is_none(),
        "new daemons must not auto-create legacy server.token"
    );
    let _remote_running = remote_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let remote_addr = remote_app.bound_tcp_addrs()[0].clone();

    let legacy_app = AppServer::open(legacy_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let legacy_token = network::ensure_server_token(&legacy_home).unwrap();
    let _legacy_running = legacy_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let legacy_addr = legacy_app.bound_tcp_addrs()[0].clone();

    let remote_proj: Project = serde_json::from_value(
        remote_app
            .call(
                "project.create",
                json!({ "name": "Remote Env Repo", "directory": dir.path().join("rem-repo") }),
            )
            .unwrap(),
    )
    .unwrap();

    let local_client = AppClient::connect_home(local_home, Path::new("/bin/echo")).unwrap();
    let registry = HostRegistry::new(local_client);
    registry
        .add_host(SavedHost {
            id: "home-linux".into(),
            name: "Home Linux".into(),
            transport: HostTransport::Tcp {
                addr: remote_addr,
                token: remote_token.clone(),
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap();
    registry
        .add_host(SavedHost {
            id: "legacy-linux".into(),
            name: "Legacy Linux".into(),
            transport: HostTransport::Tcp {
                addr: legacy_addr,
                token: legacy_token,
            },
            ssh_target: None,
            server_id: None,
            enabled: true,
            added_at: now(),
        })
        .unwrap();

    // Direct TCP sealed call check + HostRegistry routed agent/project saves.
    let sealed_agent =
        canban_storage::vault::seal_transport(&remote_token, "REM_AGENT_SEC", "rem_agent_sec_555")
            .unwrap();
    assert!(sealed_agent.starts_with(canban_storage::vault::TRANSPORT_SEAL_PREFIX));

    registry
        .call_routed(
            "env.save",
            json!({
                "host_id": "home-linux",
                "scope": "agent",
                "target_id": "codex",
                "entries": BTreeMap::<String, EnvWriteEntry>::from([
                    ("REM_AGENT_PLAIN".into(), EnvWriteEntry::Plain { value: "linux-codex".into() }),
                    (
                        "REM_AGENT_SEC".into(),
                        EnvWriteEntry::Secret {
                            value: Some("rem_agent_sec_555".into()),
                            keep_existing: false,
                        },
                    ),
                ]),
            }),
            None,
        )
        .unwrap();

    registry
        .call_routed(
            "env.save",
            json!({
                "host_id": "home-linux",
                "scope": "project",
                "target_id": remote_proj.id,
                "entries": BTreeMap::<String, EnvWriteEntry>::from([
                    ("REM_PROJ_PLAIN".into(), EnvWriteEntry::Plain { value: "linux-proj".into() }),
                    (
                        "REM_PROJ_SEC".into(),
                        EnvWriteEntry::Secret {
                            value: Some("rem_proj_sec_666".into()),
                            keep_existing: false,
                        },
                    ),
                ]),
            }),
            Some(&remote_proj.id),
        )
        .unwrap();

    // Legacy server.token host auto-upgrades to dev_... on transport.mux and unseals TCP secrets
    // using its active session token even after legacy server.token is deleted.
    legacy_app
        .call("device.remove_legacy_token", json!({}))
        .unwrap();
    registry
        .call_routed(
            "env.save",
            json!({
                "host_id": "legacy-linux",
                "scope": "agent",
                "target_id": "codex",
                "entries": BTreeMap::<String, EnvWriteEntry>::from([(
                    "UPGRADED_SEC".into(),
                    EnvWriteEntry::Secret {
                        value: Some("upgraded_dev_secret_777".into()),
                        keep_existing: false,
                    },
                )]),
            }),
            None,
        )
        .unwrap();

    let merged = registry.merged_snapshot().unwrap();
    assert!(matches!(
        merged.agent_env("home-linux", "codex").get("REM_AGENT_SEC"),
        Some(EnvEntryMeta::Secret {
            configured: true,
            ..
        })
    ));
    assert!(matches!(
        merged.project_env(&remote_proj.id).get("REM_PROJ_SEC"),
        Some(EnvEntryMeta::Secret {
            configured: true,
            ..
        })
    ));
    assert!(
        !local_app
            .snapshot()
            .unwrap()
            .agent_env("local", "codex")
            .contains_key("REM_AGENT_SEC"),
        "Remote Agent secrets must remain isolated from local host"
    );

    let codex_prof = remote_app
        .config()
        .profiles
        .into_iter()
        .find(|p| p.id == "codex")
        .unwrap();
    let rem_resolved = remote_app
        .resolve_runtime_env(
            Some(&codex_prof),
            Some(&remote_proj),
            &BTreeMap::new(),
            None,
            Some("local"),
        )
        .unwrap();
    assert_eq!(
        rem_resolved.env.get("REM_AGENT_SEC").map(String::as_str),
        Some("rem_agent_sec_555")
    );
    assert_eq!(
        rem_resolved.env.get("REM_PROJ_SEC").map(String::as_str),
        Some("rem_proj_sec_666")
    );
    assert_eq!(
        rem_resolved.env.get("REM_AGENT_PLAIN").map(String::as_str),
        Some("linux-codex")
    );
    assert_eq!(
        rem_resolved.env.get("REM_PROJ_PLAIN").map(String::as_str),
        Some("linux-proj")
    );

    let leg_resolved = legacy_app
        .resolve_runtime_env(
            Some(&codex_prof),
            None,
            &BTreeMap::new(),
            None,
            Some("local"),
        )
        .unwrap();
    assert_eq!(
        leg_resolved.env.get("UPGRADED_SEC").map(String::as_str),
        Some("upgraded_dev_secret_777")
    );
}
