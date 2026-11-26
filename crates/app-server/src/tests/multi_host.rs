use super::*;
use crate::hosts::{desktop_multi_host_client, hosts_path};

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
            trust_tailscale_self: true,
        }))
        .unwrap();
    let remote_addr = remote_app.bound_tcp_addrs()[0].clone();

    let local_client = AppClient::connect_home(local_home, Path::new("/bin/echo")).unwrap();
    let registry = desktop_multi_host_client(local_client).unwrap();
    let mut events_rx = registry.subscribe();
    let mut push_rx = registry.subscribe();

    // Wait for local push supervisor initial handshake ("local" resync).
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(canban_client::ClientEvent::Host { host_id, .. }) = events_rx.try_recv()
            && host_id == "local"
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for local push supervisor handshake"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    // Side-channel push from local AppServer flows through local_push_supervisor -> ClientEvent.
    local_app.emit_utilities();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match push_rx.try_recv() {
            Ok(canban_client::ClientEvent::UtilitiesChanged { host_id, state }) => {
                assert_eq!(host_id, "local");
                assert!(state.is_object());
                break;
            }
            Ok(_) | Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "Expected ClientEvent::UtilitiesChanged from local push stream"
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
    let registry = desktop_multi_host_client(local_client).unwrap();
    let mut push_rx = registry.subscribe();

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
            Ok(canban_client::ClientEvent::UsageChanged { host_id, state })
                if host_id == "remote-usage" =>
            {
                assert_eq!(state["readings"][0]["provider"], "codex");
                assert_eq!(state["readings"][0]["used"], 42);
                break;
            }
            Ok(_) | Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "Expected ClientEvent::UsageChanged for remote-usage"
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
            trust_tailscale_self: true,
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
    let registry = desktop_multi_host_client(local_client).unwrap();
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

    let remote_transport = registry.transport_for_host("home-linux").unwrap();
    let relay_cfg = registry.config_for_host("home-linux");
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
        let rc = remote_transport.clone();
        let pid = remote_proj.id.clone();
        rpc_handles.push(std::thread::spawn(move || {
            let ping = rc.call("app.ping", &json!({})).unwrap();
            assert!(!ping["server_id"].as_str().unwrap().is_empty());
            rc.call(
                "card.create",
                &json!({
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

    drop(remote_transport);
    registry.set_host_enabled("home-linux", false).unwrap();
    let dl2 = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < dl2 && remote_app.active_tcp_count() > 0 {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(remote_app.active_tcp_count(), 0);
    drop(registry);
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
            trust_tailscale_self: true,
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
            trust_tailscale_self: true,
        }))
        .unwrap();
    let invite_addr = invite_app.bound_tcp_addrs()[0].clone();
    let (pairing_code, _) = invite_app.create_pairing_code(300).unwrap();

    let local_client = AppClient::connect_home(local_home.clone(), Path::new("/bin/echo")).unwrap();
    let registry = desktop_multi_host_client(local_client).unwrap();

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
        serde_json::from_slice(&std::fs::read(hosts_path(&local_home)).unwrap()).unwrap();
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
    for s in registry.host_summaries() {
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
fn tailscale_verifier_and_discovery_rpc_work_on_app_server() {
    let dir = tempfile::tempdir().unwrap();
    let home_a = dir.path().join("home-a");
    let home_b = dir.path().join("home-b");

    let app_b = AppServer::open(home_b, PathBuf::from("/bin/echo")).unwrap();
    let _run_b = app_b
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
            trust_tailscale_self: true,
        }))
        .unwrap();
    let addr_b = app_b.bound_tcp_addrs()[0].clone();

    let dead_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_addr = dead_listener.local_addr().unwrap().to_string();
    drop(dead_listener);

    let hosts_file = HostsFile {
        hosts: vec![
            SavedHost {
                id: "b-live".into(),
                name: "Live Server B".into(),
                transport: HostTransport::Tcp {
                    addr: addr_b.clone(),
                    token: "dev_unused".into(),
                },
                ssh_target: None,
                server_id: Some(app_b.server_id().to_owned()),
                enabled: true,
                added_at: 1,
            },
            SavedHost {
                id: "c-dead".into(),
                name: "Unreachable Host".into(),
                transport: HostTransport::Tcp {
                    addr: dead_addr,
                    token: "dev_unused".into(),
                },
                ssh_target: None,
                server_id: None,
                enabled: true,
                added_at: 2,
            },
        ],
    };
    std::fs::create_dir_all(home_a.join("config")).unwrap();
    std::fs::write(
        home_a.join("config/hosts.json"),
        serde_json::to_vec_pretty(&hosts_file).unwrap(),
    )
    .unwrap();

    let app_a = AppServer::open(home_a, PathBuf::from("/bin/echo")).unwrap();
    struct AllowAll;
    impl canban_transport::TailscalePeerVerifier for AllowAll {
        fn verify_peer(
            &self,
            _peer_addr: std::net::SocketAddr,
        ) -> Option<canban_transport::TailscalePeerIdentity> {
            Some(canban_transport::TailscalePeerIdentity {
                node_id: "nDesktop01".into(),
                device_name: "Studio Desktop".into(),
                login_name: "jjy@example.com".into(),
            })
        }
    }
    app_a.set_tailscale_verifier(Some(Arc::new(AllowAll)));
    let _run_a = app_a
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
            trust_tailscale_self: true,
        }))
        .unwrap();
    let addr_a = app_a.bound_tcp_addrs()[0].clone();

    let client = AppClient::connect_tcp(addr_a, TAILSCALE_AUTH_SENTINEL.into()).unwrap();
    assert_eq!(client.upgraded_token(), None);
    let discovered = client.call("host.discover_canban", json!({})).unwrap();
    let items: Vec<DiscoveredCanbanHost> =
        serde_json::from_value(discovered["hosts"].clone()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].addr, addr_b);
    assert_eq!(items[0].name, "Live Server B");
    assert!(items[0].linked_on_host);
}

#[test]
fn device_remote_access_set_toggles_tcp_listener_live_and_persists_config() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let _running = app.start().unwrap();

    let initial = app.call("device.list", json!({})).unwrap();
    assert_eq!(initial["listening"], false);
    assert_eq!(initial["tcp_enabled"], false);
    assert!(app.bound_tcp_addrs().is_empty());

    // Enable TCP listening live on loopback without restarting AppServer.
    let enabled = app
        .call(
            "device.remote_access.set",
            json!({
                "tailscale": false,
                "listen": "127.0.0.1:0",
                "trust_tailscale_self": true,
            }),
        )
        .unwrap();
    assert_eq!(enabled["listening"], true);
    assert_eq!(enabled["tcp_enabled"], true);
    let addrs = app.bound_tcp_addrs();
    assert_eq!(addrs.len(), 1);
    let (_, token) = app.issue_device_token("live-test").unwrap();
    let client = AppClient::connect_tcp(addrs[0].clone(), token).unwrap();
    assert_eq!(client.server_id(), Some(app.server_id()));
    drop(client);

    // Disable remote access live and confirm listener and persisted config clear cleanly.
    let disabled = app
        .call(
            "device.remote_access.set",
            json!({
                "tailscale": false,
                "clear_listen": true,
            }),
        )
        .unwrap();
    assert_eq!(disabled["listening"], false);
    assert_eq!(disabled["tcp_enabled"], false);
    assert!(app.bound_tcp_addrs().is_empty());
    let persisted = network::load_listen_config(&home);
    assert!(!persisted.tcp_enabled());
}
