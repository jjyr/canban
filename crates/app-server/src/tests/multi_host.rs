use super::*;
use crate::hosts::desktop_multi_host_client;

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
    let desktop_client_kp = canban_transport::load_or_create_client_keypair(
        &local_home,
        true,
        canban_transport::system_noise_rng().as_ref(),
    )
    .unwrap();
    remote_app
        .authorize_device_pubkey("mux-workstation", &desktop_client_kp.public_b64())
        .unwrap();
    let remote_running = remote_app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            trust_tailscale_self: true,
        }))
        .unwrap();
    let remote_addr = remote_app.bound_tcp_addrs()[0].clone();
    let remote_pubkey = remote_app.server_pubkey();
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
                server_pubkey: Some(remote_pubkey),
                tailscale_auth: false,
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
    let client_kp = Arc::new(canban_transport::NoiseKeypair::generate().unwrap());
    app.authorize_device_pubkey("live-test", &client_kp.public_b64())
        .unwrap();
    let client = AppClient::connect_tcp(
        addrs[0].clone(),
        Some(app.server_pubkey()),
        canban_transport::TcpAuthMode::Key,
        client_kp,
    )
    .unwrap();
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
