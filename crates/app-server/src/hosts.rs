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
        _pairing_code: Option<&'a str>,
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
                let client = Arc::new(canban_transport::ClientTransport::connect_unix(
                    tun.socket_path.clone(),
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

    fn build_sync_payload(&self) -> Result<(Value, Option<String>)> {
        let (records, fp) =
            canban_storage::env::list_global_env_for_sync_with_fingerprint_from_home(
                &self.local_home,
            )?;
        let wire: Vec<_> = records.into_iter().map(|r| r.to_wire()).collect();
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
#[derive(Clone, Debug)]
pub struct DesktopSshProvider {
    local_home: PathBuf,
}

impl DesktopSshProvider {
    pub fn new(local_home: PathBuf) -> Self {
        Self { local_home }
    }
}

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
            let kp = canban_transport::load_or_create_client_keypair(
                &self.local_home,
                true,
                canban_transport::system_noise_rng().as_ref(),
            );
            match kp.and_then(|keypair| {
                AppClient::connect_tcp(
                    direct_addr.clone(),
                    None,
                    canban_transport::TcpAuthMode::Tailscale,
                    Arc::new(keypair),
                )
            }) {
                Ok(client) => {
                    let sid = client.server_id().map(str::to_owned);
                    let pk = client.transport().remote_pubkey().map(str::to_owned);
                    drop(client);
                    Ok(json!({
                        "addr": direct_addr,
                        "server_pubkey": pk,
                        "tailscale_auth": true,
                        "server_id": sid,
                        "via": "tailscale_whois",
                    }))
                }
                Err(ts_err) => {
                    tailscale_auth_error = Some(format!("{ts_err:#}"));
                    network::pair_via_ssh_noninteractive(&self.local_home, ssh_target, true, port)
                }
            }
        } else {
            network::pair_via_ssh_noninteractive(&self.local_home, ssh_target, true, port)
        };
        canban_client::TailscalePeerConnectResult {
            tcp_reachable: probe.is_ok(),
            tailscale_auth_error,
            paired: pair_res.map_err(|e| format!("{e:#}")),
        }
    }

    fn pair_via_ssh(&self, ssh_target: &str, prefer_tailscale: bool, port: u16) -> Result<Value> {
        network::pair_via_ssh_noninteractive(&self.local_home, ssh_target, prefer_tailscale, port)
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

/// Builds the desktop [`canban_client::CompositeConnector`] (`Noise_XX` TCP + SSH tunnels).
pub fn desktop_connector(local_home: &Path) -> Result<Arc<dyn canban_client::HostConnector>> {
    let client_kp = Arc::new(canban_transport::load_or_create_client_keypair(
        local_home,
        true,
        canban_transport::system_noise_rng().as_ref(),
    )?);
    Ok(Arc::new(
        canban_client::CompositeConnector::with_tcp(client_kp)
            .push(Arc::new(DesktopSshConnector::new(local_home))),
    ))
}

/// Builds the unified desktop [`canban_client::MultiHostClient`] backed by `local` (`This Mac`),
/// `<home>/config/hosts.json`, Noise_XX TCP + SSH connectors, global-env replication, and
/// `API_VERSION` enforcement.
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
        connector: desktop_connector(&local_home)?,
        capabilities: canban_client::ClientCapabilities::desktop(),
        local_host: Some(Arc::new(DesktopLocalHost::new(local))),
        global_env_provider: Some(Arc::new(DesktopGlobalEnvProvider::new(local_home))),
        min_api_version: Some(API_VERSION),
        clock: canban_types::system_clock(),
        fs_durability: canban_context::FsDurability::Full,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_remote_daemon_update_clears_updating_flag_and_restores_accurate_status() {
        let dir = tempfile::tempdir().unwrap();
        let local_home = dir.path().join("local-home");
        let local_app = AppServer::open(local_home.clone(), PathBuf::from("/bin/echo")).unwrap();
        let _local_running = local_app.start_with_listen(None).unwrap();
        let local_client =
            AppClient::connect_home(local_home.clone(), Path::new("/bin/echo")).unwrap();

        let mock_home = dir.path().join("mock-remote-home");
        let mock_app = AppServer::open(mock_home, PathBuf::from("/bin/echo")).unwrap();
        let desktop_client_kp = canban_transport::load_or_create_client_keypair(
            &local_home,
            true,
            canban_transport::system_noise_rng().as_ref(),
        )
        .unwrap();
        mock_app
            .authorize_device_pubkey("desktop-test", &desktop_client_kp.public_b64())
            .unwrap();
        let remote_pk = mock_app.server_pubkey();
        let remote_sid = "srv-old-daemon".to_owned();
        let transport_cfg = mock_app.transport_config().clone();
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let (addr_tx, addr_rx) = std::sync::mpsc::channel();
        let mock_thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap().to_string();
                let _ = addr_tx.send(addr);
                let mut stop = stop_rx.clone();
                let mock_handler =
                    Arc::new(|accepted: canban_transport::AcceptedStream| async move {
                        let (r, mut w) = tokio::io::split(accepted.stream);
                        let mut reader = tokio::io::BufReader::new(r);
                        let mut first = accepted.initial_request;
                        loop {
                            let req: Value = if let Some(v) = first.take() {
                                v
                            } else {
                                match canban_transport::read_bounded_line(
                                    &mut reader,
                                    canban_transport::MAX_FRAME_BYTES,
                                )
                                .await
                                {
                                    Ok(Some(l)) => serde_json::from_str(&l).unwrap_or_default(),
                                    _ => break,
                                }
                            };
                            let reply = if req["method"] == "update.daemon" {
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": req["id"],
                                    "error": {"code": -32000, "message": "Release download timed out"}
                                })
                            } else if req["method"] == "app.ping" {
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": req["id"],
                                    "result": {
                                        "api_version": API_VERSION.saturating_sub(1),
                                        "server_id": "srv-old-daemon",
                                        "build": {"version": "20261114-1", "api": API_VERSION.saturating_sub(1)}
                                    }
                                })
                            } else {
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": req["id"],
                                    "error": {"code": -32601, "message": "Method not found"}
                                })
                            };
                            if canban_transport::write_json_line_async(&mut w, &reply)
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    });
                loop {
                    tokio::select! {
                        _ = stop.changed() => break,
                        res = listener.accept() => {
                            let Ok((stream, peer)) = res else { break; };
                            let cfg_c = transport_cfg.clone();
                            let stop_c = stop_rx.clone();
                            let h_c = Arc::clone(&mock_handler);
                            tokio::spawn(async move {
                                let _ = canban_transport::serve_authenticated_stream(
                                    stream,
                                    Some(peer),
                                    cfg_c,
                                    stop_c,
                                    false,
                                    h_c,
                                )
                                .await;
                            });
                        }
                    }
                }
            });
        });
        let remote_addr = addr_rx.recv().unwrap();

        let hosts_file = HostsFile {
            hosts: vec![SavedHost {
                id: "home-linux".into(),
                name: "Home Linux".into(),
                transport: HostTransport::Tcp {
                    addr: remote_addr,
                    server_pubkey: Some(remote_pk),
                    tailscale_auth: false,
                },
                ssh_target: None,
                server_id: Some(remote_sid),
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
        let _ = stop_tx.send(true);
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
