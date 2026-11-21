//! Multi-host connection registry (`This Mac` + remote Tailscale/TCP and SSH `canband`s),
//! offline snapshot caching, SSH Unix-socket tunneling, and host-aware RPC routing.
use super::*;
use std::{
    collections::BTreeMap,
    process::{Child, Stdio},
};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct CachedHostData {
    saved_at: u64,
    snapshot: Snapshot,
    config: Config,
}

type EstablishedConnection = (
    Option<Arc<AppClient>>,
    Option<SshTunnel>,
    HostConnectionState,
    Option<Value>,
    Option<String>,
);

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

    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

struct RemoteSlot {
    saved: SavedHost,
    client: Option<Arc<AppClient>>,
    tunnel: Option<SshTunnel>,
    connection: HostConnectionState,
    updating: bool,
    cached: Option<CachedHostData>,
    last_state_marker: Value,
    reconnect_count: u32,
    consecutive_failures: u32,
    next_retry_at: std::time::Instant,
}

/// Window-side event provenance. This envelope never crosses the AppServer RPC
/// boundary: connection state and recovery belong to one host, not every host.
#[derive(Clone, Debug)]
pub struct HostEvent {
    pub host_id: String,
    pub event: AppEvent,
}

impl HostEvent {
    pub fn local(event: AppEvent) -> Self {
        Self {
            host_id: "local".into(),
            event,
        }
    }
}

struct UpdatingGuard<'a> {
    registry: &'a HostRegistry,
    host_id: String,
    active: bool,
}

impl Drop for UpdatingGuard<'_> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut guard = self.registry.remotes.lock().unwrap();
        if let Some(slot) = guard.get_mut(&self.host_id) {
            slot.updating = false;
            slot.next_retry_at = std::time::Instant::now();
            if slot.connection.is_updating() {
                slot.connection = if slot.saved.enabled {
                    HostConnectionState::Connecting
                } else {
                    HostConnectionState::Offline {
                        error: "Disabled".into(),
                        cached_at: slot.cached.as_ref().map(|c| c.saved_at),
                    }
                };
            }
        }
        drop(guard);
        self.registry.emit(
            &self.host_id,
            AppEvent::Changed {
                topic: "host",
                card_id: None,
            },
        );
        self.registry.wake();
    }
}

pub struct HostRegistry {
    local_home: PathBuf,
    local: Arc<AppClient>,
    local_summary_counts: Mutex<Option<(usize, usize, u64)>>,
    remotes: Mutex<BTreeMap<String, RemoteSlot>>,
    entity_hosts: RwLock<BTreeMap<String, String>>,
    acknowledged_owners: Mutex<BTreeMap<String, String>>,
    listeners: Mutex<Vec<mpsc::Sender<HostEvent>>>,
    wake_tx: mpsc::SyncSender<()>,
    stop: AtomicBool,
}

impl Drop for HostRegistry {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.wake_tx.try_send(());
    }
}

fn count_active_agents(s: &Snapshot) -> usize {
    s.terminals
        .iter()
        .filter(|t| {
            t.agent
                .as_ref()
                .is_some_and(|a| matches!(a.state.as_str(), "starting" | "running" | "working"))
        })
        .count()
}

impl HostRegistry {
    pub fn new(local: Arc<AppClient>) -> Arc<Self> {
        let local_home = local.home.clone();
        let hosts_file = Self::load_hosts_file(&local_home);
        let mut map = BTreeMap::new();
        for saved in hosts_file.hosts {
            let cached = Self::load_cache(&local_home, &saved.id);
            let initial_conn = if !saved.enabled {
                HostConnectionState::Offline {
                    error: "Disabled".into(),
                    cached_at: cached.as_ref().map(|c| c.saved_at),
                }
            } else {
                HostConnectionState::Connecting
            };
            map.insert(
                saved.id.clone(),
                RemoteSlot {
                    saved,
                    client: None,
                    tunnel: None,
                    connection: initial_conn,
                    updating: false,
                    cached,
                    last_state_marker: Value::Null,
                    reconnect_count: 0,
                    consecutive_failures: 0,
                    next_retry_at: std::time::Instant::now(),
                },
            );
        }

        let (wake_tx, wake_rx) = mpsc::sync_channel(4);
        let registry = Arc::new(Self {
            local_home,
            local: local.clone(),
            local_summary_counts: Mutex::new(None),
            remotes: Mutex::new(map),
            entity_hosts: RwLock::new(BTreeMap::new()),
            acknowledged_owners: Mutex::new(BTreeMap::new()),
            listeners: Mutex::new(Vec::new()),
            wake_tx,
            stop: AtomicBool::new(false),
        });

        // Forward local AppClient subscription events into unified listeners.
        let local_rx = local.subscribe();
        let weak_local = Arc::downgrade(&registry);
        std::thread::spawn(move || {
            while let Ok(ev) = local_rx.recv() {
                let Some(reg) = weak_local.upgrade() else {
                    break;
                };
                if reg.stop.load(Ordering::SeqCst) {
                    break;
                }
                reg.emit("local", ev);
            }
        });

        // Background remote host supervisor & state poller.
        let weak_sup = Arc::downgrade(&registry);
        std::thread::spawn(move || {
            while let Some(reg) = weak_sup.upgrade() {
                if reg.stop.load(Ordering::SeqCst) {
                    break;
                }
                reg.poll_remotes_once();
                drop(reg);
                let _ = wake_rx.recv_timeout(Duration::from_millis(850));
            }
        });

        registry
    }

    pub fn local(&self) -> &Arc<AppClient> {
        &self.local
    }

    pub fn hosts_path(home: &Path) -> PathBuf {
        home.join("config/hosts.json")
    }

    fn cache_path(home: &Path, host_id: &str) -> PathBuf {
        home.join("cache/hosts").join(format!("{host_id}.json"))
    }

    fn tunnels_dir(home: &Path) -> PathBuf {
        home.join("run/tunnels")
    }

    pub fn load_hosts_file(home: &Path) -> HostsFile {
        std::fs::read(Self::hosts_path(home))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_hosts_file(&self, hosts: &[SavedHost]) -> Result<()> {
        let file = HostsFile {
            hosts: hosts.to_vec(),
        };
        canban_context::atomic(
            &Self::hosts_path(&self.local_home),
            &serde_json::to_vec_pretty(&file)?,
        )
    }

    fn load_cache(home: &Path, host_id: &str) -> Option<CachedHostData> {
        std::fs::read(Self::cache_path(home, host_id))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    }

    fn save_cache(home: &Path, host_id: &str, data: &CachedHostData) {
        if let Ok(bytes) = serde_json::to_vec(data) {
            let _ = canban_context::atomic(&Self::cache_path(home, host_id), &bytes);
        }
    }

    pub fn subscribe(&self) -> mpsc::Receiver<HostEvent> {
        let (tx, rx) = mpsc::channel();
        self.listeners.lock().unwrap().push(tx);
        rx
    }

    fn emit(&self, host_id: &str, event: AppEvent) {
        let event = HostEvent {
            host_id: host_id.into(),
            event,
        };
        self.listeners
            .lock()
            .unwrap()
            .retain(|tx| tx.send(event.clone()).is_ok());
    }

    pub fn wake(&self) {
        let _ = self.wake_tx.try_send(());
    }

    pub fn saved_hosts(&self) -> Vec<SavedHost> {
        self.remotes
            .lock()
            .unwrap()
            .values()
            .map(|slot| slot.saved.clone())
            .collect()
    }

    /// Validate and connect/register a new or updated remote host.
    /// Rejects accidental loopback connections to `This Mac` or duplicate `server_id`s.
    pub fn add_host(&self, mut host: SavedHost) -> Result<HostSummary> {
        ensure!(
            host.id != "local" && !host.id.trim().is_empty(),
            "Invalid remote host ID"
        );
        host.name = host.name.trim().to_owned();
        ensure!(!host.name.is_empty(), "Host display name is required");
        if let HostTransport::Tcp { addr, token } = &mut host.transport {
            *addr = canban_types::normalize_daemon_addr(addr);
            *token = token.trim().to_owned();
            ensure!(!addr.is_empty(), "Remote host address is required");
            ensure!(!token.is_empty(), "Pairing token is required");
        }

        // Perform immediate connection/probe validation so errors surface cleanly in the wizard.
        let (client, tunnel, conn, mut state_val, server_id) =
            self.establish_connection(&host.id, &host.transport)?;

        if let Some(sid) = &server_id {
            if self.local.server_id() == Some(sid.as_str()) {
                bail!(
                    "That address points to This Mac's own Canban service ({sid}), which is already connected locally."
                );
            }
            let guard = self.remotes.lock().unwrap();
            if let Some(dup) = guard.values().find(|s| {
                s.saved.id != host.id && s.saved.server_id.as_deref() == Some(sid.as_str())
            }) {
                bail!(
                    "That Canban daemon is already connected under '{}'.",
                    dup.saved.name
                );
            }
        }

        host.server_id = server_id;
        let marker = state_val
            .as_ref()
            .map(super::state::state_marker)
            .unwrap_or(Value::Null);
        let cached = if let Some(val) = &mut state_val
            && let Ok(snap) = serde_json::from_value::<Snapshot>(val["snapshot"].take())
            && let Ok(cfg) = serde_json::from_value::<Config>(val["config"].take())
        {
            let data = CachedHostData {
                saved_at: now(),
                snapshot: snap,
                config: cfg,
            };
            Self::save_cache(&self.local_home, &host.id, &data);
            Some(data)
        } else {
            Self::load_cache(&self.local_home, &host.id)
        };

        let summary = {
            let mut guard = self.remotes.lock().unwrap();
            let slot = RemoteSlot {
                saved: host,
                client,
                tunnel,
                connection: conn,
                updating: false,
                cached,
                last_state_marker: marker,
                reconnect_count: 0,
                consecutive_failures: 0,
                next_retry_at: std::time::Instant::now(),
            };
            let sum = Self::slot_summary(&slot);
            guard.insert(slot.saved.id.clone(), slot);
            let all_saved: Vec<SavedHost> = guard.values().map(|s| s.saved.clone()).collect();
            drop(guard);
            self.save_hosts_file(&all_saved)?;
            sum
        };

        self.emit(
            &summary.id,
            AppEvent::Changed {
                topic: if summary.connection.is_connected() {
                    "resync"
                } else {
                    "host"
                },
                card_id: None,
            },
        );
        self.wake();
        Ok(summary)
    }

    pub fn remove_host(&self, host_id: &str) -> Result<()> {
        ensure!(host_id != "local", "Cannot remove This Mac");
        let all_saved = {
            let mut guard = self.remotes.lock().unwrap();
            guard.remove(host_id);
            guard.values().map(|s| s.saved.clone()).collect::<Vec<_>>()
        };
        let _ = std::fs::remove_file(Self::cache_path(&self.local_home, host_id));
        self.save_hosts_file(&all_saved)?;
        self.emit(
            host_id,
            AppEvent::Changed {
                topic: "host",
                card_id: None,
            },
        );
        Ok(())
    }

    pub fn rename_host(&self, host_id: &str, new_name: &str) -> Result<()> {
        ensure!(host_id != "local", "Cannot rename This Mac");
        let trimmed = new_name.trim();
        ensure!(!trimmed.is_empty(), "Host display name cannot be empty");
        let all_saved = {
            let mut guard = self.remotes.lock().unwrap();
            let slot = guard
                .get_mut(host_id)
                .with_context(|| format!("Unknown Canban host '{host_id}'"))?;
            slot.saved.name = trimmed.to_owned();
            guard.values().map(|s| s.saved.clone()).collect::<Vec<_>>()
        };
        self.save_hosts_file(&all_saved)?;
        self.emit(
            host_id,
            AppEvent::Changed {
                topic: "host",
                card_id: None,
            },
        );
        Ok(())
    }

    pub fn set_host_enabled(&self, host_id: &str, enabled: bool) -> Result<()> {
        ensure!(host_id != "local", "Cannot disable This Mac");
        let all_saved = {
            let mut guard = self.remotes.lock().unwrap();
            let slot = guard
                .get_mut(host_id)
                .with_context(|| format!("Unknown Canban host '{host_id}'"))?;
            slot.saved.enabled = enabled;
            if !enabled {
                slot.client = None;
                slot.tunnel = None;
                slot.connection = HostConnectionState::Offline {
                    error: "Disabled".into(),
                    cached_at: slot.cached.as_ref().map(|c| c.saved_at),
                };
            } else if !slot.updating {
                slot.consecutive_failures = 0;
                slot.next_retry_at = std::time::Instant::now();
                slot.connection = HostConnectionState::Connecting;
            }
            guard.values().map(|s| s.saved.clone()).collect::<Vec<_>>()
        };
        self.save_hosts_file(&all_saved)?;
        self.emit(
            host_id,
            AppEvent::Changed {
                topic: "host",
                card_id: None,
            },
        );
        self.wake();
        Ok(())
    }

    pub fn reconnect(&self, host_id: Option<&str>) {
        let mut changed = Vec::new();
        {
            let mut guard = self.remotes.lock().unwrap();
            for (id, slot) in guard.iter_mut() {
                if host_id.is_none_or(|target| target == id) && slot.saved.enabled && !slot.updating
                {
                    slot.client = None;
                    slot.tunnel = None;
                    slot.consecutive_failures = 0;
                    slot.next_retry_at = std::time::Instant::now();
                    slot.connection = HostConnectionState::Connecting;
                    changed.push(id.clone());
                }
            }
        }
        for id in changed {
            self.emit(
                &id,
                AppEvent::Changed {
                    topic: "host",
                    card_id: None,
                },
            );
        }
        self.wake();
    }

    fn apply_established_slot(
        &self,
        host_id: &str,
        established: EstablishedConnection,
    ) -> (bool, bool) {
        let (client_opt, tunnel, conn_state, state_val, server_id) = established;
        let mut guard = self.remotes.lock().unwrap();
        let Some(slot) = guard.get_mut(host_id) else {
            return (false, false);
        };
        let previous = slot.connection.clone();
        let data_changed = state_val
            .as_ref()
            .is_some_and(|v| super::state::state_marker(v) != slot.last_state_marker);
        slot.client = client_opt;
        slot.tunnel = tunnel;
        slot.connection = conn_state;
        slot.consecutive_failures = 0;
        // A reachable but incompatible daemon needs a slower probe, not an
        // immediate retry on every supervisor pass. Explicit reconnect bypasses it.
        slot.next_retry_at = std::time::Instant::now()
            + if slot.connection.is_connected() {
                Duration::ZERO
            } else {
                Duration::from_secs(30)
            };
        if let Some(sid) = server_id
            && slot.saved.server_id.is_none()
        {
            slot.saved.server_id = Some(sid);
        }
        if let Some(mut val) = state_val {
            let marker = super::state::state_marker(&val);
            if let (Ok(snap), Ok(cfg)) = (
                serde_json::from_value::<Snapshot>(val["snapshot"].take()),
                serde_json::from_value::<Config>(val["config"].take()),
            ) {
                let data = CachedHostData {
                    saved_at: now(),
                    snapshot: snap,
                    config: cfg,
                };
                Self::save_cache(&self.local_home, host_id, &data);
                slot.cached = Some(data);
            }
            slot.last_state_marker = marker;
        }
        let became_connected = !previous.is_connected() && slot.connection.is_connected();
        (
            previous != slot.connection || data_changed,
            became_connected,
        )
    }

    /// Upgrade a remote host's `canband` daemon over its authenticated transport (`update.daemon`),
    /// even when the host is currently held in `ApiMismatch` (`BackendOlder`) state.
    pub fn update_remote_daemon(&self, host_id: &str) -> Result<Value> {
        ensure!(
            host_id != "local",
            "Use Settings -> Updates to update This Mac"
        );
        let (saved, prior_server_id) = {
            let mut guard = self.remotes.lock().unwrap();
            let slot = guard
                .get_mut(host_id)
                .with_context(|| format!("Unknown Canban host '{host_id}'"))?;
            if slot.updating || slot.connection.is_updating() {
                bail!(
                    "Daemon update is already in progress on '{}'",
                    slot.saved.name
                );
            }
            if matches!(slot.saved.transport, HostTransport::Local) {
                bail!("Cannot remote-update Local transport");
            }
            slot.updating = true;
            slot.client = None;
            slot.tunnel = None;
            slot.connection = HostConnectionState::Updating {
                version: String::new(),
            };
            (slot.saved.clone(), slot.saved.server_id.clone())
        };
        let mut updating_guard = UpdatingGuard {
            registry: self,
            host_id: host_id.to_owned(),
            active: true,
        };
        self.emit(
            host_id,
            AppEvent::Changed {
                topic: "host",
                card_id: None,
            },
        );
        self.wake();

        let params = json!({
            "request_key": id(),
        });
        let rpc_res = match &saved.transport {
            HostTransport::Local => bail!("Cannot remote-update Local transport"),
            HostTransport::Tcp { addr, token } => client::rpc_endpoint(
                &ClientEndpoint::Tcp {
                    addr: addr.clone(),
                    token: token.clone(),
                },
                "update.daemon",
                params,
            ),
            HostTransport::Ssh {
                target,
                remote_home,
            } => SshTunnel::start(
                &Self::tunnels_dir(&self.local_home),
                &format!("{host_id}-update"),
                target,
                remote_home.as_deref(),
            )
            .and_then(|tun| {
                client::rpc_endpoint(
                    &ClientEndpoint::Unix(tun.socket_path.clone()),
                    "update.daemon",
                    params,
                )
            }),
        };

        let res = match rpc_res {
            Ok(val) => val,
            Err(err) => {
                updating_guard.active = false;
                {
                    let mut guard = self.remotes.lock().unwrap();
                    if let Some(slot) = guard.get_mut(host_id) {
                        slot.updating = false;
                        slot.consecutive_failures = 0;
                        slot.next_retry_at = std::time::Instant::now();
                        slot.connection = if slot.saved.enabled {
                            HostConnectionState::Connecting
                        } else {
                            HostConnectionState::Offline {
                                error: "Disabled".into(),
                                cached_at: slot.cached.as_ref().map(|c| c.saved_at),
                            }
                        };
                    }
                }
                self.poll_remotes_once();
                self.emit(
                    host_id,
                    AppEvent::Changed {
                        topic: "host",
                        card_id: None,
                    },
                );
                self.wake();
                return Err(err);
            }
        };

        if res["updated"] == true {
            let target_ver = res["version"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_owned();
            if !target_ver.is_empty() {
                {
                    let mut guard = self.remotes.lock().unwrap();
                    if let Some(slot) = guard.get_mut(host_id)
                        && slot.updating
                    {
                        slot.connection = HostConnectionState::Updating {
                            version: target_ver.clone(),
                        };
                    }
                }
                self.emit(
                    host_id,
                    AppEvent::Changed {
                        topic: "host",
                        card_id: None,
                    },
                );
            }

            // `update.daemon` schedules `systemctl --user restart canban.service` (`RestartSec=2`),
            // `launchctl kickstart -k`, or graceful `app.shutdown` + spawn ~120ms after flushing RPC.
            // Poll within a bounded window until the restarted daemon comes online with the updated build/API.
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            std::thread::sleep(Duration::from_millis(350));
            let mut connected = false;
            while std::time::Instant::now() < deadline {
                let still_enabled = {
                    let guard = self.remotes.lock().unwrap();
                    guard.get(host_id).is_some_and(|s| s.saved.enabled)
                };
                if !still_enabled {
                    break;
                }
                if let Ok(est) = self.establish_connection(host_id, &saved.transport) {
                    let matches_restarted = match &est.2 {
                        HostConnectionState::Connected { version, .. } => {
                            let new_sid = est.4.as_deref();
                            let sid_changed = prior_server_id
                                .as_deref()
                                .zip(new_sid)
                                .is_some_and(|(old, cur)| old != cur);
                            target_ver.is_empty() || version == &target_ver || sid_changed
                        }
                        _ => false,
                    };
                    if matches_restarted {
                        updating_guard.active = false;
                        {
                            let mut guard = self.remotes.lock().unwrap();
                            if let Some(slot) = guard.get_mut(host_id) {
                                slot.updating = false;
                                if let Some(sid) = &est.4 {
                                    slot.saved.server_id = Some(sid.clone());
                                }
                            }
                        }
                        let _ = self.apply_established_slot(host_id, est);
                        connected = true;
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(400));
            }

            if !connected {
                updating_guard.active = false;
                {
                    let mut guard = self.remotes.lock().unwrap();
                    if let Some(slot) = guard.get_mut(host_id) {
                        slot.updating = false;
                        slot.client = None;
                        slot.tunnel = None;
                        slot.consecutive_failures = 0;
                        slot.next_retry_at = std::time::Instant::now();
                        slot.connection = if slot.saved.enabled {
                            HostConnectionState::Connecting
                        } else {
                            HostConnectionState::Offline {
                                error: "Disabled".into(),
                                cached_at: slot.cached.as_ref().map(|c| c.saved_at),
                            }
                        };
                    }
                }
                self.poll_remotes_once();
            }

            self.emit(
                host_id,
                AppEvent::Changed {
                    topic: if connected { "resync" } else { "host" },
                    card_id: None,
                },
            );
            self.wake();
        } else {
            updating_guard.active = false;
            {
                let mut guard = self.remotes.lock().unwrap();
                if let Some(slot) = guard.get_mut(host_id) {
                    slot.updating = false;
                    slot.consecutive_failures = 0;
                    slot.next_retry_at = std::time::Instant::now();
                    slot.connection = if slot.saved.enabled {
                        HostConnectionState::Connecting
                    } else {
                        HostConnectionState::Offline {
                            error: "Disabled".into(),
                            cached_at: slot.cached.as_ref().map(|c| c.saved_at),
                        }
                    };
                }
            }
            self.poll_remotes_once();
            self.emit(
                host_id,
                AppEvent::Changed {
                    topic: "host",
                    card_id: None,
                },
            );
            self.wake();
        }
        Ok(res)
    }

    fn establish_connection(
        &self,
        host_id: &str,
        transport: &HostTransport,
    ) -> Result<EstablishedConnection> {
        let start = std::time::Instant::now();
        let (connect_res, tunnel) = match transport {
            HostTransport::Local => bail!("Local transport is reserved for This Mac"),
            HostTransport::Tcp { addr, token } => {
                (AppClient::connect_tcp(addr.clone(), token.clone()), None)
            }
            HostTransport::Ssh {
                target,
                remote_home,
            } => {
                let tun = SshTunnel::start(
                    &Self::tunnels_dir(&self.local_home),
                    host_id,
                    target,
                    remote_home.as_deref(),
                )?;
                let res = AppClient::connect_ssh_socket(tun.socket_path.clone());
                (res, Some(tun))
            }
        };

        match connect_res {
            Ok(client) => {
                let rtt_ms = start.elapsed().as_millis() as u64;
                let ping = client.call("app.ping", json!({})).unwrap_or_default();
                let state_val = client.call("app.state", json!({})).ok();
                let api_version = ping["api_version"]
                    .as_u64()
                    .unwrap_or(u64::from(API_VERSION)) as u32;
                let version = ping["build"]["version"]
                    .as_str()
                    .or_else(|| ping["version"].as_str())
                    .unwrap_or("unknown")
                    .to_owned();
                let platform = ping["build"]["platform"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_owned();
                let server_id = client
                    .server_id()
                    .map(str::to_owned)
                    .or_else(|| ping["server_id"].as_str().map(str::to_owned));
                Ok((
                    Some(client),
                    tunnel,
                    HostConnectionState::Connected {
                        rtt_ms,
                        api_version,
                        version,
                        platform,
                    },
                    state_val,
                    server_id,
                ))
            }
            Err(err) => {
                if let Some(older) = err.downcast_ref::<BackendOlder>() {
                    let server_api = older.ping["api_version"].as_u64().unwrap_or(0) as u32;
                    let version = older.ping["build"]["version"]
                        .as_str()
                        .or_else(|| older.ping["version"].as_str())
                        .unwrap_or("unknown")
                        .to_owned();
                    let sid = older.ping["server_id"].as_str().map(str::to_owned);
                    return Ok((
                        None,
                        None,
                        HostConnectionState::ApiMismatch {
                            server_api,
                            app_api: API_VERSION,
                            version,
                        },
                        None,
                        sid,
                    ));
                }
                let msg = format!("{err:#}");
                if msg.contains("auth_failed:") {
                    bail!("{msg}");
                }
                Err(err)
            }
        }
    }

    pub fn poll_remotes_once(&self) {
        let ids: Vec<String> = {
            let guard = self.remotes.lock().unwrap();
            guard.keys().cloned().collect()
        };

        for id in ids {
            let mut topic = None;
            let (saved, existing_client, should_retry) = {
                let mut guard = self.remotes.lock().unwrap();
                let Some(slot) = guard.get_mut(&id) else {
                    continue;
                };
                if !slot.saved.enabled || slot.updating {
                    continue;
                }
                if let Some(tun) = &mut slot.tunnel
                    && !tun.is_alive()
                {
                    slot.client = None;
                    slot.tunnel = None;
                }
                (
                    slot.saved.clone(),
                    slot.client.clone(),
                    std::time::Instant::now() >= slot.next_retry_at,
                )
            };

            if let Some(client) = existing_client {
                let start = std::time::Instant::now();
                let after = self
                    .remotes
                    .lock()
                    .unwrap()
                    .get(&id)
                    .map(|slot| slot.last_state_marker["revision"].clone())
                    .unwrap_or(Value::Null);
                match client.call("app.state", json!({"after":after})) {
                    Ok(mut state_val) => {
                        let rtt_ms = start.elapsed().as_millis() as u64;
                        let mut guard = self.remotes.lock().unwrap();
                        if let Some(slot) = guard.get_mut(&id)
                            && !slot.updating
                        {
                            if state_val["changed"] == true
                                && let (Ok(snap), Ok(cfg)) = (
                                    serde_json::from_value::<Snapshot>(
                                        state_val["snapshot"].take(),
                                    ),
                                    serde_json::from_value::<Config>(state_val["config"].take()),
                                )
                            {
                                client.set_cached_config(cfg.clone());
                                let data = CachedHostData {
                                    saved_at: now(),
                                    snapshot: snap,
                                    config: cfg,
                                };
                                Self::save_cache(&self.local_home, &id, &data);
                                slot.cached = Some(data);
                                topic = Some(client::change_topic(
                                    true,
                                    &slot.last_state_marker,
                                    &state_val,
                                ));
                                slot.last_state_marker = super::state::state_marker(&state_val);
                            }
                            if let HostConnectionState::Connected {
                                rtt_ms: prev_rtt, ..
                            } = &mut slot.connection
                            {
                                *prev_rtt = rtt_ms;
                            }
                        }
                    }
                    Err(err) => {
                        let msg = format!("{err:#}");
                        let mut guard = self.remotes.lock().unwrap();
                        if let Some(slot) = guard.get_mut(&id)
                            && !slot.updating
                        {
                            slot.client = None;
                            slot.tunnel = None;
                            slot.reconnect_count = slot.reconnect_count.saturating_add(1);
                            slot.consecutive_failures = 1;
                            slot.next_retry_at = std::time::Instant::now() + Duration::from_secs(1);
                            slot.connection = if msg.contains("auth_failed:") {
                                HostConnectionState::AuthFailed { error: msg }
                            } else {
                                HostConnectionState::Reconnecting {
                                    attempt: 1,
                                    error: msg,
                                }
                            };
                            topic = Some("host");
                        }
                    }
                }
                if let Some(topic) = topic {
                    self.emit(
                        &id,
                        AppEvent::Changed {
                            topic,
                            card_id: None,
                        },
                    );
                }
                continue;
            }

            if !should_retry {
                continue;
            }

            match self.establish_connection(&id, &saved.transport) {
                Ok(established) => {
                    let (changed, connected) = self.apply_established_slot(&id, established);
                    if changed {
                        topic = Some(if connected { "resync" } else { "host" });
                    }
                }
                Err(err) => {
                    let msg = format!("{err:#}");
                    let mut guard = self.remotes.lock().unwrap();
                    if let Some(slot) = guard.get_mut(&id)
                        && !slot.updating
                    {
                        slot.consecutive_failures = slot.consecutive_failures.saturating_add(1);
                        let backoff_secs = match slot.consecutive_failures {
                            1 => 1,
                            2 => 2,
                            _ => 4,
                        };
                        slot.next_retry_at =
                            std::time::Instant::now() + Duration::from_secs(backoff_secs);
                        let prev = slot.connection.clone();
                        slot.connection = if msg.contains("auth_failed:") {
                            HostConnectionState::AuthFailed { error: msg }
                        } else if slot.consecutive_failures <= 2 {
                            HostConnectionState::Reconnecting {
                                attempt: slot.consecutive_failures,
                                error: msg,
                            }
                        } else {
                            HostConnectionState::Offline {
                                error: msg,
                                cached_at: slot.cached.as_ref().map(|c| c.saved_at),
                            }
                        };
                        if slot.connection != prev {
                            topic = Some("host");
                        }
                    }
                }
            }
            if let Some(topic) = topic {
                self.emit(
                    &id,
                    AppEvent::Changed {
                        topic,
                        card_id: None,
                    },
                );
            }
        }
    }

    fn slot_summary(slot: &RemoteSlot) -> HostSummary {
        let (active_agents, max_agents, project_count, last_synced_at) = match &slot.cached {
            Some(c) => (
                count_active_agents(&c.snapshot),
                c.config.scheduler.max_agents,
                c.snapshot.projects.len(),
                Some(c.saved_at),
            ),
            None => (0, 8, 0, None),
        };
        HostSummary {
            id: slot.saved.id.clone(),
            name: slot.saved.name.clone(),
            is_local: false,
            transport_label: slot.saved.transport.label().into(),
            endpoint_label: slot.saved.transport.endpoint_display(),
            ssh_target: slot
                .saved
                .ssh_target
                .clone()
                .or_else(|| match &slot.saved.transport {
                    HostTransport::Ssh { target, .. } => Some(target.clone()),
                    _ => None,
                }),
            server_id: slot.saved.server_id.clone(),
            connection: slot.connection.clone(),
            active_agents,
            max_agents,
            project_count,
            reconnect_count: slot.reconnect_count,
            last_synced_at,
        }
    }

    /// Return lightweight host summaries from cached local counts and in-memory remote state
    /// without issuing a blocking `app.state` RPC.
    pub fn summaries(&self) -> Vec<HostSummary> {
        let local_cfg = self.local.config();
        let (local_active, local_projects, last_synced_at) = self
            .local_summary_counts
            .lock()
            .unwrap()
            .map(|(active, projects, synced_at)| (active, projects, Some(synced_at)))
            .unwrap_or((0, 0, None));
        let mut out = vec![HostSummary {
            id: "local".into(),
            name: "This Mac".into(),
            is_local: true,
            transport_label: "Local socket".into(),
            endpoint_label: self
                .local
                .home
                .join("app.sock")
                .to_string_lossy()
                .into_owned(),
            ssh_target: None,
            server_id: self.local.server_id().map(str::to_owned),
            connection: HostConnectionState::Connected {
                rtt_ms: 0,
                api_version: API_VERSION,
                version: env!("CARGO_PKG_VERSION").into(),
                platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            },
            active_agents: local_active,
            max_agents: local_cfg.scheduler.max_agents,
            project_count: local_projects,
            reconnect_count: 0,
            last_synced_at,
        }];
        let guard = self.remotes.lock().unwrap();
        for slot in guard.values() {
            out.push(Self::slot_summary(slot));
        }
        out
    }

    /// Build the unified `Snapshot` across `This Mac` and all enabled remote hosts
    /// (live when connected, cached read-only when offline) and refresh the O(1) routing table.
    pub fn merged_snapshot(&self) -> Result<Snapshot> {
        let mut combined = self.local.snapshot()?;
        let local_cfg = self.local.config();
        let local_active = count_active_agents(&combined);
        let local_projects = combined.projects.len();
        let synced_at = now();
        *self.local_summary_counts.lock().unwrap() =
            Some((local_active, local_projects, synced_at));
        combined.register_host_entities("local");
        combined.host_summaries.insert(
            "local".into(),
            HostSummary {
                id: "local".into(),
                name: "This Mac".into(),
                is_local: true,
                transport_label: "Local socket".into(),
                endpoint_label: self
                    .local
                    .home
                    .join("app.sock")
                    .to_string_lossy()
                    .into_owned(),
                ssh_target: None,
                server_id: self.local.server_id().map(str::to_owned),
                connection: HostConnectionState::Connected {
                    rtt_ms: 0,
                    api_version: API_VERSION,
                    version: env!("CARGO_PKG_VERSION").into(),
                    platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
                },
                active_agents: local_active,
                max_agents: local_cfg.scheduler.max_agents,
                project_count: local_projects,
                reconnect_count: 0,
                last_synced_at: Some(synced_at),
            },
        );

        let remote_snapshots: Vec<(Option<Snapshot>, HostSummary)> = {
            let guard = self.remotes.lock().unwrap();
            guard
                .values()
                .filter(|slot| slot.saved.enabled)
                .map(|slot| {
                    let snap = slot.cached.as_ref().map(|c| c.snapshot.clone());
                    (snap, Self::slot_summary(slot))
                })
                .collect()
        };

        for (snap_opt, summary) in remote_snapshots {
            if let Some(remote_snap) = snap_opt {
                combined.merge_remote_snapshot(remote_snap, summary);
            } else {
                combined.host_summaries.insert(summary.id.clone(), summary);
            }
        }

        // Newly created remote Cards/Terminals can be addressed before the supervisor's
        // authoritative snapshot catches up. Do not project replies into its
        // cache: the UI must distinguish a stale cache from read confirmation.
        self.acknowledged_owners.lock().unwrap().retain(|id, host| {
            if combined.entity_hosts.contains_key(id) || !combined.host_summaries.contains_key(host)
            {
                return false;
            }
            combined.entity_hosts.insert(id.clone(), host.clone());
            true
        });
        *self.entity_hosts.write().unwrap() = combined.entity_hosts.clone();
        Ok(combined)
    }

    pub fn client_for(&self, host_id: &str) -> Result<Arc<AppClient>> {
        if host_id.is_empty() || host_id == "local" {
            return Ok(self.local.clone());
        }
        let guard = self.remotes.lock().unwrap();
        let slot = guard
            .get(host_id)
            .with_context(|| format!("Unknown Canban host '{host_id}'"))?;
        if let Some(client) = &slot.client {
            return Ok(client.clone());
        }
        bail!(
            "Host '{}' is currently unavailable ({})",
            slot.saved.name,
            slot.connection.short_status()
        )
    }

    pub fn config_for(&self, host_id: &str) -> Config {
        if host_id.is_empty() || host_id == "local" {
            return self.local.config();
        }
        let guard = self.remotes.lock().unwrap();
        if let Some(slot) = guard.get(host_id) {
            if let Some(client) = &slot.client {
                return client.config();
            }
            if let Some(cached) = &slot.cached {
                return cached.config.clone();
            }
        }
        self.local.config()
    }

    pub fn home_for(&self, host_id: &str) -> PathBuf {
        if host_id.is_empty() || host_id == "local" {
            return self.local.home.clone();
        }
        let guard = self.remotes.lock().unwrap();
        if let Some(slot) = guard.get(host_id)
            && let Some(client) = &slot.client
        {
            return client.home.clone();
        }
        PathBuf::from("~/.canban")
    }

    pub fn resolve_target_host(
        &self,
        method: &str,
        params: &Value,
        active_project_id: Option<&str>,
    ) -> String {
        if let Some(explicit) = params["host_id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return explicit.to_owned();
        }
        // Global desktop / single-authority GitHub Workspace operations stay on `local`.
        if method.starts_with("utilities.")
            || method.starts_with("update.")
            || method.starts_with("workflow.default.")
            || method == "host.discover"
            || method == "activity.record"
            || method.starts_with("github.auth.")
            || matches!(
                method,
                "github.status"
                    | "github.search"
                    | "github.item"
                    | "review.github_pr"
                    | "review.github_pr_file"
            )
        {
            return "local".into();
        }
        let map = self.entity_hosts.read().unwrap();
        for key in [
            "card_id",
            "project_id",
            "schedule_id",
            "schedule_run_id",
            "terminal_id",
            "worktree_id",
            "inherit_terminal_id",
        ] {
            if let Some(id) = params[key]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                && let Some(h) = map.get(id)
            {
                return h.clone();
            }
        }
        if let Some(pid) = active_project_id.filter(|s| !s.is_empty())
            && let Some(h) = map.get(pid)
        {
            return h.clone();
        }
        "local".into()
    }

    pub fn sync_host_now(&self, host_id: &str) {
        if host_id.is_empty() || host_id == "local" {
            return;
        }
        let client = {
            let guard = self.remotes.lock().unwrap();
            guard.get(host_id).and_then(|s| s.client.clone())
        };
        let Some(client) = client else {
            return;
        };
        if let Ok(mut state_val) = client.call("app.state", json!({}))
            && let (Ok(snap), Ok(cfg)) = (
                serde_json::from_value::<Snapshot>(state_val["snapshot"].take()),
                serde_json::from_value::<Config>(state_val["config"].take()),
            )
        {
            client.set_cached_config(cfg.clone());
            let mut snap = snap;
            snap.register_host_entities(host_id);
            {
                let mut eh = self.entity_hosts.write().unwrap();
                eh.retain(|k, v| v != host_id || snap.entity_hosts.contains_key(k));
                for (k, v) in &snap.entity_hosts {
                    eh.insert(k.clone(), v.clone());
                }
            }
            let data = CachedHostData {
                saved_at: now(),
                snapshot: snap,
                config: cfg,
            };
            Self::save_cache(&self.local_home, host_id, &data);
            let mut guard = self.remotes.lock().unwrap();
            if let Some(slot) = guard.get_mut(host_id) {
                slot.cached = Some(data);
                slot.last_state_marker = super::state::state_marker(&state_val);
            }
        }
    }

    pub fn call_routed(
        &self,
        method: &str,
        mut params: Value,
        active_project_id: Option<&str>,
    ) -> Result<Value> {
        if method == "notifications.read_all"
            && params["host_id"].is_null()
            && params["project_id"]
                .as_str()
                .is_none_or(|s| s.trim().is_empty())
        {
            let local_res = self.local.call(method, params.clone())?;
            let remote_ids: Vec<(String, Arc<AppClient>)> = {
                let guard = self.remotes.lock().unwrap();
                guard
                    .iter()
                    .filter_map(|(id, slot)| Some((id.clone(), slot.client.clone()?)))
                    .collect()
            };
            for (hid, client) in remote_ids {
                if client.call(method, params.clone()).is_ok() {
                    self.sync_host_now(&hid);
                }
            }
            return Ok(local_res);
        }
        let host_id = self.resolve_target_host(method, &params, active_project_id);
        if let Some(obj) = params.as_object_mut() {
            obj.remove("host_id");
        }
        let client = self.client_for(&host_id)?;
        let res = client.call(method, params)?;
        if host_id != "local"
            && !matches!(
                method,
                "app.ping"
                    | "app.state"
                    | "app.watch"
                    | "app.snapshot"
                    | "agent.catalog"
                    | "diagnostics.status"
                    | "diagnostics.logs"
                    | "directory.list"
                    | "project.detect_github"
                    | "git.file_lines"
                    | "review.preview"
                    | "card.resolve.preview"
                    | "execution.output"
                    | "resource.read"
                    | "timeline.list"
                    | "activity.record"
                    | "insights.events"
                    | "insights.activity"
                    | "insights.activity_buckets"
                    | "usage.status"
                    | "usage.refresh"
            )
        {
            if matches!(
                method,
                "config.agents.save" | "agent.setup" | "scheduler.configure"
            ) {
                let mut guard = self.remotes.lock().unwrap();
                if let Some(slot) = guard.get_mut(&host_id)
                    && let Some(cached) = &mut slot.cached
                {
                    match method {
                        "config.agents.save" | "agent.setup" => {
                            if let Ok(cfg) = serde_json::from_value::<Config>(res.clone()) {
                                cached.config = cfg;
                            }
                        }
                        "scheduler.configure" => {
                            if let Ok(settings) =
                                serde_json::from_value::<SchedulerSettings>(res.clone())
                            {
                                cached.config.scheduler = settings;
                            }
                        }
                        _ => {}
                    }
                }
                drop(guard);
                self.wake();
            } else if matches!(
                method,
                "card.create"
                    | "card.move"
                    | "card.resolve"
                    | "card.reopen"
                    | "card.hold_auto_merge"
                    | "comment.add"
                    | "comment.update"
                    | "comment.delete"
                    | "notifications.read"
                    | "schedule.toggle"
                    | "github.issue.add"
                    | "github.item.add"
                    | "terminal.create"
            ) {
                // These replies contain the committed entity/receipt. Do not make
                // acknowledgement wait for a second full remote state transfer.
                if matches!(
                    method,
                    "card.create" | "github.issue.add" | "github.item.add" | "terminal.create"
                ) && let Some(id) = res["id"].as_str()
                {
                    self.acknowledged_owners
                        .lock()
                        .unwrap()
                        .insert(id.into(), host_id.clone());
                    self.entity_hosts
                        .write()
                        .unwrap()
                        .insert(id.into(), host_id.clone());
                }
                self.wake();
            } else {
                self.sync_host_now(&host_id);
            }
            self.emit(
                &host_id,
                AppEvent::Changed {
                    topic: "application",
                    card_id: None,
                },
            );
        }
        Ok(res)
    }

    pub fn call_all_hosts(&self, method: &str, params: Value) -> Vec<(String, Result<Value>)> {
        let mut results = Vec::new();
        results.push(("local".into(), self.local.call(method, params.clone())));
        let remote_clients: Vec<(String, Arc<AppClient>)> = {
            let guard = self.remotes.lock().unwrap();
            guard
                .iter()
                .filter_map(|(id, slot)| Some((id.clone(), slot.client.clone()?)))
                .collect()
        };
        for (hid, client) in remote_clients {
            results.push((hid, client.call(method, params.clone())));
        }
        results
    }
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

        // Mock an older `canband` TCP endpoint that blocks briefly inside `update.daemon`
        // so we can observe the live `Updating` state, test concurrent duplicate update rejection,
        // and verify `poll_remotes_once` / `reconnect` do not clobber `Updating` mid-upgrade!
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let remote_addr = format!("127.0.0.1:{port}");

        // Write saved host directly into hosts.json in an initial older/mismatch state.
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
            &HostRegistry::hosts_path(&local_home),
            &serde_json::to_vec_pretty(&hosts_file).unwrap(),
        )
        .unwrap();

        let registry = HostRegistry::new(local_client);
        let events_rx = registry.subscribe();

        let (entered_rpc_tx, entered_rpc_rx) = mpsc::channel();
        let (release_rpc_tx, release_rpc_rx) = mpsc::channel();
        let remote_home_for_restart = remote_home.clone();

        let mock_thread = std::thread::spawn(move || {
            // Accept connections until `update.daemon` arrives (background supervisor may probe first).
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
                        // Simulate older daemon API so supervisor sees ApiMismatch.
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
            // Drop old listener and simulate `RestartSec=2` systemd restart delay (~950ms > old 600ms sleep!)
            // before starting a real updated `AppServer` on the exact same port!
            drop(listener);
            std::thread::sleep(Duration::from_millis(950));
            let restarted_app =
                AppServer::open(remote_home_for_restart, PathBuf::from("/bin/echo")).unwrap();
            let restarted_running = restarted_app
                .start_with_listen(Some(ServerListenConfig {
                    tailscale: false,
                    port,
                    listen: Some(format!("127.0.0.1:{port}")),
                }))
                .unwrap();
            (restarted_app, restarted_running)
        });

        // Wait briefly for initial probe to establish `ApiMismatch`.
        for _ in 0..20 {
            registry.poll_remotes_once();
            if registry.summaries().iter().any(|h| {
                h.id == "home-linux"
                    && matches!(h.connection, HostConnectionState::ApiMismatch { .. })
            }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let initial_sum = registry
            .summaries()
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

        // Drain prior events, then launch `update_remote_daemon` on a worker thread.
        while events_rx.try_recv().is_ok() {}
        // Repeated incompatible probes are state observations, never recovery.
        // A normal supervisor pass must also respect the mismatch retry deadline.
        for _ in 0..3 {
            let retry = registry.remotes.lock().unwrap()["home-linux"].next_retry_at;
            assert!(retry > std::time::Instant::now());
            registry.poll_remotes_once();
            registry
                .remotes
                .lock()
                .unwrap()
                .get_mut("home-linux")
                .unwrap()
                .next_retry_at = std::time::Instant::now();
            registry.poll_remotes_once();
        }
        assert!(
            events_rx
                .try_iter()
                .all(|event| event.host_id != "home-linux"),
            "unchanged ApiMismatch emitted another event"
        );
        let reg_for_upd = registry.clone();
        let upd_handle = std::thread::spawn(move || reg_for_upd.update_remote_daemon("home-linux"));

        entered_rpc_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("update.daemon RPC should reach remote host");

        // Verify `Updating` was emitted BEFORE `update.daemon` finished!
        let mid_sum = registry
            .summaries()
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

        // Concurrent duplicate `update_remote_daemon` call must be rejected cleanly.
        let dup_err = registry
            .update_remote_daemon("home-linux")
            .expect_err("concurrent update_remote_daemon should fail");
        assert!(
            dup_err.to_string().contains("already in progress"),
            "unexpected duplicate update error: {dup_err:#}"
        );

        // Background `reconnect` and `poll_remotes_once` must NOT clobber `Updating`!
        registry.reconnect(None);
        registry.poll_remotes_once();
        let after_poll = registry
            .summaries()
            .into_iter()
            .find(|h| h.id == "home-linux")
            .unwrap();
        assert!(
            after_poll.is_updating(),
            "poll_remotes_once / reconnect clobbered Updating state: {:?}",
            after_poll.connection
        );

        // Let `update.daemon` reply and simulate delayed systemd restart (~950ms).
        let _ = release_rpc_tx.send(());
        let res = upd_handle.join().unwrap().expect("update_remote_daemon ok");
        assert_eq!(res["updated"], true);

        let (_restarted_app, _restarted_running) = mock_thread.join().unwrap();

        // By the time `update_remote_daemon` returns, the host must have seamlessly transitioned to `Connected`!
        let final_sum = registry
            .summaries()
            .into_iter()
            .find(|h| h.id == "home-linux")
            .unwrap();
        assert!(
            final_sum.is_connected(),
            "expected host to recover to Connected after delayed daemon restart, got {:?}",
            final_sum.connection
        );
        assert!(!final_sum.is_updating());
        let recovered = events_rx
            .try_iter()
            .filter(|event| {
                event.host_id == "home-linux"
                    && matches!(
                        event.event,
                        AppEvent::Changed {
                            topic: "resync",
                            ..
                        }
                    )
            })
            .count();
        assert_eq!(
            recovered, 1,
            "only actual remote recovery emits one host-scoped resync"
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
            &HostRegistry::hosts_path(&local_home),
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

        let registry = HostRegistry::new(local_client);
        let err = registry
            .update_remote_daemon("home-linux")
            .expect_err("expected RPC error from mock server");
        assert!(err.to_string().contains("Release download timed out"));

        let restored = registry
            .summaries()
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
        let registry = HostRegistry::new(local_client);

        // Before merged_snapshot(), summaries() returns immediately with zeroed local counts.
        let initial = registry.summaries();
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].id, "local");
        assert_eq!(initial[0].project_count, 0);
        assert!(initial[0].last_synced_at.is_none());

        // Create a project, run merged_snapshot() to populate cached counts, then stop the server.
        let proj_dir = dir.path().join("demo-proj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        let _ = registry.local().call(
            "project.create",
            json!({
                "name": "Demo",
                "directory": proj_dir.to_string_lossy(),
            }),
        );
        let snap = registry.merged_snapshot().unwrap();
        assert_eq!(snap.projects.len(), 1);
        drop(running);

        // Even with local AppServer shut down, summaries() succeeds without RPC and keeps counts.
        let cached = registry.summaries();
        assert_eq!(cached[0].project_count, 1);
        assert!(cached[0].last_synced_at.is_some());
    }
}
