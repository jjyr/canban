use super::*;
use crate::hosts::HostRegistry;

#[test]
fn tcp_listener_probe_token_auth_and_diagnostics_work_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("remote-home");
    let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let token = network::ensure_server_token(&home).unwrap();
    let listen = ServerListenConfig {
        tailscale: false,
        port: 0,
        listen: Some("127.0.0.1:0".into()),
    };
    let running = app.start_with_listen(Some(listen)).unwrap();
    let bound = app.bound_tcp_addrs();
    assert_eq!(bound.len(), 1);
    let addr = bound[0].clone();

    // 1. Pre-auth `host.probe` succeeds without revealing private workspace entities.
    let probe = network::probe_tcp_host(&addr, Duration::from_secs(2)).unwrap();
    assert_eq!(probe["server_id"].as_str(), Some(app.server_id()));
    assert_eq!(probe["api_version"].as_u64(), Some(u64::from(API_VERSION)));
    assert_eq!(probe["auth_required"], true);

    // 2. Wrong token is rejected with explicit `auth_failed:`.
    let bad_err = match AppClient::connect_tcp(addr.clone(), "wrong-token".into()) {
        Ok(_) => panic!("Expected invalid token to fail"),
        Err(e) => e,
    };
    assert!(
        format!("{bad_err:#}").contains("auth_failed:"),
        "Expected auth_failed error, got: {bad_err:#}"
    );

    // 3. Valid pairing URI parses cleanly and authenticates TCP AppClient.
    let uri = format_pairing_uri(&addr, &token);
    let (parsed_addr, parsed_token) = parse_pairing_uri(&uri).unwrap();
    assert_eq!(parsed_addr, addr);
    assert_eq!(parsed_token, token);

    let client = AppClient::connect_tcp(parsed_addr, parsed_token).unwrap();
    assert_eq!(client.server_id(), Some(app.server_id()));
    let cfg = client.config();
    assert_eq!(
        cfg.relay_endpoint.as_deref(),
        Some(format!("tcp://{addr}").as_str())
    );
    assert_eq!(cfg.relay_token.as_deref(), Some(token.as_str()));

    // 4. Structured diagnostics status and log tail work over authenticated TCP.
    std::fs::write(home.join("server.log"), "line one\nline two\nline three\n").unwrap();
    let diag = client.call("diagnostics.status", json!({})).unwrap();
    assert_eq!(diag["server"]["server_id"].as_str(), Some(app.server_id()));
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
    let local_token = network::ensure_server_token(&local_home).unwrap();
    let local_running = local_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
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
    let remote_token = network::ensure_server_token(&remote_home).unwrap();
    let remote_running = remote_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
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
    let remote_token = network::ensure_server_token(&remote_home).unwrap();
    let remote_running = remote_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
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
