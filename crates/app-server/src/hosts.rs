//! Desktop platform adapters plugging into the unified [`canban_client::MultiHostClient`]:
//! - [`DesktopLocalHost`] (`"local"` primary host driver over [`AppClient`])
//! - [`DesktopSshConnector`] & [`desktop_connector`] (TCP + SSH Unix-socket tunnels with RAII [`canban_client::ConnectionLease`])
//! - [`DesktopGlobalEnvProvider`] (Global Environment & Secrets replication payload builder)
//! - [`DesktopDiscoveryProvider`] & [`DesktopSshProvider`] (local Tailscale / `~/.ssh/config` discovery and SSH auto-pairing/bootstrap)
use super::*;
use std::process::{Child, Stdio};

/// Canonical desktop `hosts.json` path (`<home>/config/hosts.json`).
pub fn hosts_path(home: &Path) -> PathBuf {
    home.join("config/hosts.json")
}

/// Directory storing desktop SSH-forwarded Unix sockets (`<home>/run/tunnels`).
pub fn tunnels_dir(home: &Path) -> PathBuf {
    home.join("run/tunnels")
}

/// Loads desktop saved hosts from `<home>/config/hosts.json`.
pub fn load_hosts_file(home: &Path) -> HostsFile {
    std::fs::read(hosts_path(home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

struct SshTunnel {
    socket_path: PathBuf,
    child: Child,
}

impl SshTunnel {
    fn start(
        tunnels_dir: &Path,
        host_id: &str,
        target: &str,
        remote_home: Option<&str>,
    ) -> Result<Self> {
        std::fs::create_dir_all(tunnels_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(tunnels_dir, std::fs::Permissions::from_mode(0o700));
        }
        let socket_path = tunnels_dir.join(format!("{host_id}.sock"));
        let _ = std::fs::remove_file(&socket_path);

        // Resolve remote home path cleanly if `~` or unset so ssh StreamLocalForward gets an absolute path.
        let remote_sock = match remote_home.map(str::trim).filter(|s| !s.is_empty()) {
            Some(h) if Path::new(h).is_absolute() => {
                format!("{}/app.sock", h.trim_end_matches('/'))
            }
            _ => {
                let probe = canban_exec::command("ssh")
                    .args([
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "ConnectTimeout=8",
                        target.trim(),
                        "printf '%s' \"$HOME/.canban/app.sock\"",
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output()
                    .context("Failed to invoke ssh for tunnel setup")?;
                ensure!(
                    probe.status.success(),
                    "SSH connection to {} failed: {}",
                    target.trim(),
                    String::from_utf8_lossy(&probe.stderr).trim()
                );
                let resolved = String::from_utf8_lossy(&probe.stdout).trim().to_owned();
                ensure!(
                    resolved.starts_with('/'),
                    "Could not resolve remote ~/.canban/app.sock path on {}",
                    target.trim()
                );
                resolved
            }
        };

        let forward_spec = format!("{}:{remote_sock}", socket_path.display());
        let mut child = canban_exec::command("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "StreamLocalBindUnlink=yes",
                "-o",
                "ConnectTimeout=8",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-N",
                "-L",
                &forward_spec,
                target.trim(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to start SSH socket forwarder")?;

        for _ in 0..80 {
            if socket_path.exists() && UnixStream::connect(&socket_path).is_ok() {
                return Ok(Self { socket_path, child });
            }
            if let Ok(Some(status)) = child.try_wait() {
                let mut err_msg = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = stderr.read_to_string(&mut err_msg);
                }
                bail!(
                    "SSH tunnel to {} exited ({status}): {}",
                    target.trim(),
                    err_msg.trim()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&socket_path);
        bail!(
            "Timed out waiting for SSH socket tunnel to {}",
            target.trim()
        )
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Desktop [`canban_client::HostConnector`] supporting SSH-forwarded Unix sockets with RAII
/// [`canban_client::ConnectionLease`] subprocess cleanup.
pub struct DesktopSshConnector {
    tunnels_dir: PathBuf,
}

impl DesktopSshConnector {
    pub fn new(local_home: &Path) -> Self {
        Self {
            tunnels_dir: tunnels_dir(local_home),
        }
    }
}

impl canban_client::HostConnector for DesktopSshConnector {
    fn supports(&self, transport: &HostTransport) -> bool {
        matches!(transport, HostTransport::Ssh { .. })
    }

    fn connect<'a>(
        &'a self,
        host_id: &'a str,
        transport: &'a HostTransport,
        _device_name: &'a str,
    ) -> canban_client::ConnectFuture<'a> {
        let tunnels_dir = self.tunnels_dir.clone();
        let host_id = host_id.to_owned();
        let transport = transport.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let HostTransport::Ssh {
                    target,
                    remote_home,
                } = transport
                else {
                    bail!("DesktopSshConnector only supports SSH hosts");
                };
                let tun =
                    SshTunnel::start(&tunnels_dir, &host_id, &target, remote_home.as_deref())?;
                let client = Arc::new(canban_transport::ClientTransport::connect_direct(
                    ClientEndpoint::Unix(tun.socket_path.clone()),
                )?);
                Ok(canban_client::ConnectedTransport::with_lease(
                    client,
                    canban_client::ConnectionLease::from_guard(Mutex::new(tun)),
                ))
            })
            .await?
        })
    }
}

/// Desktop [`canban_client::LocalHostDriver`] backed by [`AppClient`].
pub struct DesktopLocalHost {
    local: Arc<AppClient>,
}

impl DesktopLocalHost {
    pub fn new(local: Arc<AppClient>) -> Self {
        Self { local }
    }
}

impl canban_client::LocalHostDriver for DesktopLocalHost {
    fn home(&self) -> &Path {
        &self.local.home
    }

    fn endpoint_label(&self) -> String {
        self.local
            .home
            .join("app.sock")
            .to_string_lossy()
            .into_owned()
    }

    fn server_id(&self) -> Option<String> {
        self.local.server_id().map(str::to_owned)
    }

    fn server_notice(&self) -> Option<String> {
        self.local.server_notice().map(str::to_owned)
    }

    fn api_version(&self) -> u32 {
        API_VERSION
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").into()
    }

    fn platform(&self) -> String {
        format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
    }

    fn transport(&self) -> Arc<canban_transport::ClientTransport> {
        self.local.transport().clone()
    }

    fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.local.call(method, params)
    }

    fn snapshot(&self) -> Result<Snapshot> {
        self.local.snapshot()
    }

    fn runtime_state(&self) -> Result<RuntimeState> {
        self.local.runtime_state()
    }

    fn config(&self) -> Config {
        self.local.config()
    }
}

/// Desktop [`canban_client::GlobalEnvSyncProvider`] backed by `canban_storage::env`.
pub struct DesktopGlobalEnvProvider {
    local_home: PathBuf,
}

impl DesktopGlobalEnvProvider {
    pub fn new(local_home: PathBuf) -> Self {
        Self { local_home }
    }
}

impl canban_client::GlobalEnvSyncProvider for DesktopGlobalEnvProvider {
    fn current_fingerprint(&self) -> Option<String> {
        canban_storage::env::global_env_state_fingerprint_from_home(&self.local_home)
            .ok()
            .flatten()
    }

    fn build_sync_payload(&self, tcp_token: Option<&str>) -> Result<(Value, Option<String>)> {
        let (records, fp) =
            canban_storage::env::list_global_env_for_sync_with_fingerprint_from_home(
                &self.local_home,
            )?;
        let mut wire = Vec::with_capacity(records.len());
        for record in records {
            wire.push(record.to_wire(tcp_token)?);
        }
        Ok((json!({ "records": wire }), fp))
    }
}

/// Desktop [`canban_client::LocalDiscoveryProvider`] delegating to local Tailscale & `~/.ssh/config` probing.
#[derive(Clone, Copy, Debug, Default)]
pub struct DesktopDiscoveryProvider;

impl canban_client::LocalDiscoveryProvider for DesktopDiscoveryProvider {
    fn discover_local(&self) -> canban_types::DiscoveryCatalog {
        network::discover_all()
    }
}

/// Desktop [`canban_client::SshCapabilityProvider`] implementing Tailscale WhoIs + SSH auto-pairing and bootstrap.
#[derive(Clone, Copy, Debug, Default)]
pub struct DesktopSshProvider;

impl canban_client::SshCapabilityProvider for DesktopSshProvider {
    fn connect_tailscale_peer_or_ssh(
        &self,
        peer_ip: &str,
        ssh_target: &str,
        port: u16,
    ) -> canban_client::TailscalePeerConnectResult {
        let direct_addr = format!("{peer_ip}:{port}");
        let probe = network::probe_tcp_reachable(&direct_addr, Duration::from_secs(2));
        let mut tailscale_auth_error: Option<String> = None;
        let pair_res = if probe.is_ok() {
            match AppClient::connect_tcp(
                direct_addr.clone(),
                canban_types::TAILSCALE_AUTH_SENTINEL.into(),
            ) {
                Ok(client) => {
                    let sid = client.server_id().map(str::to_owned);
                    drop(client);
                    Ok(json!({
                        "addr": direct_addr,
                        "token": canban_types::TAILSCALE_AUTH_SENTINEL,
                        "server_id": sid,
                        "via": "tailscale_whois",
                    }))
                }
                Err(ts_err) => {
                    tailscale_auth_error = Some(format!("{ts_err:#}"));
                    network::pair_via_ssh_noninteractive(ssh_target, true, port)
                }
            }
        } else {
            network::pair_via_ssh_noninteractive(ssh_target, true, port)
        };
        canban_client::TailscalePeerConnectResult {
            tcp_reachable: probe.is_ok(),
            tailscale_auth_error,
            paired: pair_res.map_err(|e| format!("{e:#}")),
        }
    }

    fn pair_via_ssh(&self, ssh_target: &str, prefer_tailscale: bool, port: u16) -> Result<Value> {
        network::pair_via_ssh_noninteractive(ssh_target, prefer_tailscale, port)
    }

    fn build_ssh_bootstrap_command(
        &self,
        ssh_target: &str,
        repo: &str,
        prefer_tailscale: bool,
        port: u16,
    ) -> String {
        network::build_ssh_bootstrap_command(ssh_target, repo, prefer_tailscale, port)
    }
}

/// Builds the desktop [`canban_client::CompositeConnector`] (TCP + SSH tunnels).
pub fn desktop_connector(local_home: &Path) -> Arc<dyn canban_client::HostConnector> {
    Arc::new(
        canban_client::CompositeConnector::with_default_tcp()
            .push(Arc::new(DesktopSshConnector::new(local_home))),
    )
}

/// Builds the unified desktop [`canban_client::MultiHostClient`] backed by `local` (`This Mac`),
/// `<home>/config/hosts.json`, TCP + SSH connectors, global-env replication, and `API_VERSION`
/// enforcement.
pub fn desktop_multi_host_client(
    local: Arc<AppClient>,
) -> Result<Arc<canban_client::MultiHostClient>> {
    client::init_mux_relay_opener();
    let local_home = local.home.clone();
    let mut config = canban_client::RemoteClientConfig::new(local_home.clone());
    config.device_name = network::hostname_string();
    canban_client::MultiHostClient::from_options(canban_client::MultiHostClientOptions {
        config,
        host_store: Arc::new(canban_client::InlineHostsFileStore::new(hosts_path(
            &local_home,
        ))),
        connector: desktop_connector(&local_home),
        capabilities: canban_client::ClientCapabilities::desktop(),
        local_host: Some(Arc::new(DesktopLocalHost::new(local))),
        global_env_provider: Some(Arc::new(DesktopGlobalEnvProvider::new(local_home))),
        min_api_version: Some(API_VERSION),
        clock: canban_types::system_clock(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn update_remote_daemon_transitions_updating_guards_concurrency_and_recovers_after_delayed_restart()
     {
        let dir = tempfile::tempdir().unwrap();
        let local_home = dir.path().join("local-home");
        let remote_home = dir.path().join("remote-home");

        let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
        let _local_running = local_app.start_with_listen(None).unwrap();
        let local_client =
            AppClient::connect_home(local_home.clone(), Path::new("/bin/echo")).unwrap();

        let remote_token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        canban_updater::atomic(
            &network::token_path(&remote_home),
            format!("{remote_token}\n").as_bytes(),
        )
        .unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let remote_addr = format!("127.0.0.1:{port}");

        let hosts_file = HostsFile {
            hosts: vec![SavedHost {
                id: "home-linux".into(),
                name: "Home Linux".into(),
                transport: HostTransport::Tcp {
                    addr: remote_addr.clone(),
                    token: remote_token.into(),
                },
                ssh_target: Some("jjy@127.0.0.1".into()),
                server_id: Some("srv-old-daemon".into()),
                enabled: true,
                added_at: now(),
            }],
        };
        canban_context::atomic(
            &hosts_path(&local_home),
            &serde_json::to_vec_pretty(&hosts_file).unwrap(),
        )
        .unwrap();

        let client = desktop_multi_host_client(local_client).unwrap();
        let mut events_rx = client.subscribe();

        let (entered_rpc_tx, entered_rpc_rx) = mpsc::channel();
        let (release_rpc_tx, release_rpc_rx) = mpsc::channel();
        let remote_home_for_restart = remote_home.clone();

        let mock_thread = std::thread::spawn(move || {
            loop {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line.is_empty() {
                    continue;
                }
                let req: Value = serde_json::from_str(&line).unwrap_or_default();
                assert_eq!(req["auth"].as_str(), Some(remote_token));
                match req["method"].as_str().unwrap_or("") {
                    "app.ping" => {
                        let _ = writeln!(
                            stream,
                            "{}",
                            json!({
                                "jsonrpc": "2.0",
                                "id": req["id"],
                                "result": {
                                    "api_version": API_VERSION.saturating_sub(2),
                                    "server_id": "srv-old-daemon",
                                    "build": {
                                        "version": "20261114-1",
                                        "api": API_VERSION.saturating_sub(2),
                                        "platform": "linux-x86_64"
                                    }
                                }
                            })
                        );
                        let _ = stream.flush();
                    }
                    "update.daemon" => {
                        let _ = entered_rpc_tx.send(());
                        let _ = release_rpc_rx.recv_timeout(Duration::from_secs(5));
                        let _ = writeln!(
                            stream,
                            "{}",
                            json!({
                                "jsonrpc": "2.0",
                                "id": req["id"],
                                "result": {
                                    "updated": true,
                                    "version": env!("CARGO_PKG_VERSION"),
                                    "api_version": API_VERSION,
                                    "restarting": true
                                }
                            })
                        );
                        let _ = stream.flush();
                        drop(stream);
                        break;
                    }
                    _ => {}
                }
            }
            drop(listener);
            std::thread::sleep(Duration::from_millis(950));
            let restarted_app =
                AppServer::open(remote_home_for_restart, PathBuf::from("/bin/echo")).unwrap();
            let restarted_running = restarted_app
                .start_with_listen(Some(ServerListenConfig {
                    tailscale: false,
                    port,
                    listen: Some(format!("127.0.0.1:{port}")),
                    allow_insecure_bind: false,
                    trust_tailscale_self: true,
                }))
                .unwrap();
            (restarted_app, restarted_running)
        });

        for _ in 0..20 {
            client.poll_remotes_once();
            if client.host_summaries().iter().any(|h| {
                h.id == "home-linux"
                    && matches!(h.connection, HostConnectionState::ApiMismatch { .. })
            }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let initial_sum = client
            .host_summaries()
            .into_iter()
            .find(|h| h.id == "home-linux")
            .unwrap();
        assert!(
            matches!(
                initial_sum.connection,
                HostConnectionState::ApiMismatch { .. }
            ),
            "expected initial ApiMismatch, got {:?}",
            initial_sum.connection
        );

        while events_rx.try_recv().is_ok() {}
        for _ in 0..3 {
            assert!(client.is_retry_backed_off("home-linux"));
            client.poll_remotes_once();
            client.clear_retry_backoff("home-linux");
            client.poll_remotes_once();
        }
        while let Ok(ev) = events_rx.try_recv() {
            if let canban_client::ClientEvent::Host { host_id, .. } = ev {
                assert_ne!(
                    host_id, "home-linux",
                    "unchanged ApiMismatch emitted another event"
                );
            }
        }
        let client_for_upd = client.clone();
        let upd_handle =
            std::thread::spawn(move || client_for_upd.update_remote_daemon("home-linux"));

        entered_rpc_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("update.daemon RPC should reach remote host");

        let mid_sum = client
            .host_summaries()
            .into_iter()
            .find(|h| h.id == "home-linux")
            .unwrap();
        assert!(
            mid_sum.is_updating(),
            "expected host to be in Updating state during in-flight RPC, got {:?}",
            mid_sum.connection
        );
        assert_eq!(mid_sum.connection.short_status(), "Updating daemon…");
        assert!(
            events_rx.try_recv().is_ok(),
            "expected AppEvent::Changed when entering Updating state"
        );

        let dup_err = client
            .update_remote_daemon("home-linux")
            .expect_err("concurrent update_remote_daemon should fail");
        assert!(
            dup_err.to_string().contains("already in progress"),
            "unexpected duplicate update error: {dup_err:#}"
        );

        client.reconnect(None);
        client.poll_remotes_once();
        let after_poll = client
            .host_summaries()
            .into_iter()
            .find(|h| h.id == "home-linux")
            .unwrap();
        assert!(
            after_poll.is_updating(),
            "poll_remotes_once / reconnect clobbered Updating state: {:?}",
            after_poll.connection
        );

        let _ = release_rpc_tx.send(());
        let res = upd_handle.join().unwrap().expect("update_remote_daemon ok");
        assert_eq!(res["updated"], true);

        let (_restarted_app, _restarted_running) = mock_thread.join().unwrap();

        let final_sum = client
            .host_summaries()
            .into_iter()
            .find(|h| h.id == "home-linux")
            .unwrap();
        assert!(
            final_sum.is_connected(),
            "expected host to recover to Connected after delayed daemon restart, got {:?}",
            final_sum.connection
        );
        assert!(!final_sum.is_updating());
        let mut recovered = 0;
        while let Ok(ev) = events_rx.try_recv() {
            if let canban_client::ClientEvent::Host {
                host_id,
                event: AppEvent::Changed {
                    topic: "resync", ..
                },
            } = ev
                && host_id == "home-linux"
            {
                recovered += 1;
            }
        }
        assert_eq!(
            recovered, 1,
            "only actual remote recovery emits one host-scoped resync"
        );
        let persisted: HostsFile =
            serde_json::from_slice(&std::fs::read(hosts_path(&local_home)).unwrap()).unwrap();
        let HostTransport::Tcp { token, .. } = &persisted.hosts[0].transport else {
            panic!("expected tcp host transport");
        };
        assert!(
            is_device_token(token),
            "expected legacy token auto-upgrade to dev_..., got {token}"
        );
    }

    #[test]
    fn failed_remote_daemon_update_clears_updating_flag_and_restores_accurate_status() {
        let dir = tempfile::tempdir().unwrap();
        let local_home = dir.path().join("local-home");
        let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
        let _local_running = local_app.start_with_listen(None).unwrap();
        let local_client =
            AppClient::connect_home(local_home.clone(), Path::new("/bin/echo")).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let remote_addr = format!("127.0.0.1:{port}");
        let remote_token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        let hosts_file = HostsFile {
            hosts: vec![SavedHost {
                id: "home-linux".into(),
                name: "Home Linux".into(),
                transport: HostTransport::Tcp {
                    addr: remote_addr,
                    token: remote_token.into(),
                },
                ssh_target: None,
                server_id: Some("srv-old-daemon".into()),
                enabled: true,
                added_at: now(),
            }],
        };
        canban_context::atomic(
            &hosts_path(&local_home),
            &serde_json::to_vec_pretty(&hosts_file).unwrap(),
        )
        .unwrap();

        let stop_mock = Arc::new(AtomicBool::new(false));
        let stop_clone = stop_mock.clone();
        let mock_thread = std::thread::spawn(move || {
            let _ = listener.set_nonblocking(false);
            while !stop_clone.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                if stop_clone.load(Ordering::SeqCst) {
                    break;
                }
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line.is_empty() {
                    continue;
                }
                let req: Value = serde_json::from_str(&line).unwrap_or_default();
                if req["method"] == "update.daemon" {
                    let _ = writeln!(
                        stream,
                        "{}",
                        json!({
                            "jsonrpc": "2.0",
                            "id": req["id"],
                            "error": {"code": -32000, "message": "Release download timed out"}
                        })
                    );
                    let _ = stream.flush();
                } else if req["method"] == "app.ping" {
                    let _ = writeln!(
                        stream,
                        "{}",
                        json!({
                            "jsonrpc": "2.0",
                            "id": req["id"],
                            "result": {
                                "api_version": API_VERSION.saturating_sub(1),
                                "server_id": "srv-old-daemon",
                                "build": {"version": "20261114-1", "api": API_VERSION.saturating_sub(1)}
                            }
                        })
                    );
                    let _ = stream.flush();
                }
            }
        });

        let client = desktop_multi_host_client(local_client).unwrap();
        let err = client
            .update_remote_daemon("home-linux")
            .expect_err("expected RPC error from mock server");
        assert!(err.to_string().contains("Release download timed out"));

        let restored = client
            .host_summaries()
            .into_iter()
            .find(|h| h.id == "home-linux")
            .unwrap();
        assert!(
            !restored.is_updating(),
            "updating flag must be cleared on failure, got {:?}",
            restored.connection
        );
        assert!(
            matches!(restored.connection, HostConnectionState::ApiMismatch { .. }),
            "expected host to return to ApiMismatch after failed update, got {:?}",
            restored.connection
        );

        stop_mock.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(format!("127.0.0.1:{port}"));
        let _ = mock_thread.join();
    }

    #[test]
    fn summaries_reads_cached_local_counts_without_issuing_app_state_rpc() {
        let dir = tempfile::tempdir().unwrap();
        let local_home = dir.path().join("local-home");
        let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
        let running = local_app.start_with_listen(None).unwrap();
        let local_client =
            AppClient::connect_home(local_home.clone(), Path::new("/bin/echo")).unwrap();
        let client = desktop_multi_host_client(local_client).unwrap();

        let initial = client.host_summaries();
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].id, "local");
        assert_eq!(initial[0].project_count, 0);
        assert!(initial[0].last_synced_at.is_none());

        let proj_dir = dir.path().join("demo-proj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        let _ = client.call_local(
            "project.create",
            json!({
                "name": "Demo",
                "directory": proj_dir.to_string_lossy(),
            }),
        );
        let snap = client.merged_snapshot().unwrap();
        assert_eq!(snap.projects.len(), 1);
        drop(running);

        let cached = client.host_summaries();
        assert_eq!(cached[0].project_count, 1);
        assert!(cached[0].last_synced_at.is_some());
    }
}
