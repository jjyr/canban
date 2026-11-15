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
                json!({"project_id": local_p.id, "title": "Local card", "body": "On Mac"}),
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
                json!({"project_id": remote_p.id, "title": "Remote card 1", "body": "On Linux"}),
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

    // 3. Routed mutation on remote Project lands on remote AppServer and updates merged_snapshot immediately!
    let remote_c2: Card = serde_json::from_value(
        registry
            .call_routed(
                "card.create",
                json!({
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
    let snap2 = registry.merged_snapshot().unwrap();
    assert_eq!(snap2.cards.len(), 3);
    assert_eq!(snap2.card_host(&remote_c2.id), "home-linux");

    // 4. Simulate laptop going offline / remote disconnecting: cached snapshot stays visible read-only!
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
        3,
        "Cached remote Cards must remain visible while offline"
    );

    // Local work continues uninterrupted while remote is offline!
    registry
        .call_routed(
            "card.create",
            json!({
                "project_id": local_p.id,
                "title": "Offline local work",
                "body": "Works on plane/train",
                "request_key": "local-offline-card"
            }),
            None,
        )
        .unwrap();
    assert_eq!(registry.merged_snapshot().unwrap().cards.len(), 4);

    // Remote mutations fail cleanly with host availability message while offline.
    let offline_mut = registry
        .call_routed(
            "card.create",
            json!({"project_id": remote_p.id, "title": "Should fail while offline"}),
            None,
        )
        .unwrap_err();
    assert!(offline_mut.to_string().contains("Home Linux"));

    // 5. Remote server kept working autonomously while disconnected, then comes back online on the same port!
    let remote_app2 = AppServer::open(remote_home.clone(), PathBuf::from("/bin/echo")).unwrap();
    remote_app2
        .call(
            "card.create",
            json!({
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

    let recovered = registry.merged_snapshot().unwrap();
    assert!(!recovered.is_host_offline("home-linux"));
    assert_eq!(recovered.cards.len(), 5);
    assert!(
        recovered
            .cards
            .iter()
            .any(|c| c.title == "Autonomous overnight remote card")
    );

    drop(registry);
    drop(remote_running2);
    drop(local_running);
}
