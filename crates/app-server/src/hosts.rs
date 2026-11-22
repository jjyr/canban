//! Multi-host connection registry (`This Mac` + remote Tailscale/TCP and SSH `canband`s),
//! offline snapshot caching, SSH Unix-socket tunneling, and host-aware RPC routing.
use super::*;
use canban_sync::{
    DEFAULT_READ_CACHE_BUDGET_BYTES, DrainReport, Mutation, MutationKind, MutationState, Outbox,
    QueueDecision, ReadCache, UpdateBase,
    replay::{FinishOutcome, StepAction, finish_step, prepare_next_step},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    process::{Child, Stdio},
    sync::{OnceLock, Weak},
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
    outbox: Arc<Mutex<Outbox>>,
    read_cache: Arc<Mutex<ReadCache>>,
    draining: Arc<AtomicBool>,
    prefetching: Arc<AtomicBool>,
    connect_gate: Arc<Mutex<()>>,
}

struct AtomicFlagGuard(Arc<AtomicBool>);

impl Drop for AtomicFlagGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn is_connection_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::WouldBlock
            )
        })
    })
}

fn enrich_timeline_with_pending(
    val: &mut Value,
    ops: &[Mutation],
    card_id: &str,
    snapshot: &Snapshot,
) {
    let pending = canban_sync::pending_comment_events(ops, card_id, snapshot);
    if pending.is_empty() {
        return;
    }
    let Some(arr) = val["events"].as_array_mut() else {
        return;
    };
    let existing: BTreeSet<String> = arr
        .iter()
        .filter_map(|v| v["id"].as_str().map(str::to_owned))
        .collect();
    for ev in pending {
        if existing.contains(&ev.id) {
            continue;
        }
        if let Ok(mut ev_val) = serde_json::to_value(ev) {
            if let Some(obj) = ev_val.as_object_mut()
                && let Some(msg) = obj.remove("message")
            {
                obj.insert("body".into(), msg);
            }
            arr.push(ev_val);
        }
    }
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

#[derive(Clone, Debug)]
pub enum HostPushEvent {
    State(HostEvent),
    UpdateChanged { host_id: String, state: Value },
    UtilitiesChanged { host_id: String, state: Value },
    UsageChanged { host_id: String, state: Value },
    HerdrInstall { host_id: String, state: Value },
}

struct HostSupervisorHandle {
    stop_tx: tokio::sync::watch::Sender<bool>,
    notify: Arc<tokio::sync::Notify>,
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
        self.registry.wake_host(&self.host_id);
    }
}

pub struct HostRegistry {
    local_home: PathBuf,
    local: Arc<AppClient>,
    self_weak: OnceLock<Weak<HostRegistry>>,
    local_summary_counts: Mutex<Option<(usize, usize, u64)>>,
    remotes: Mutex<BTreeMap<String, RemoteSlot>>,
    entity_hosts: RwLock<BTreeMap<String, String>>,
    acknowledged_owners: Mutex<BTreeMap<String, String>>,
    listeners: Mutex<Vec<mpsc::Sender<HostEvent>>>,
    runtime: Option<tokio::runtime::Runtime>,
    runtime_handle: tokio::runtime::Handle,
    stop_tx: tokio::sync::watch::Sender<bool>,
    push_tx: tokio::sync::broadcast::Sender<HostPushEvent>,
    supervisors: Mutex<BTreeMap<String, HostSupervisorHandle>>,
    stop: AtomicBool,
}

impl Drop for HostRegistry {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.stop_tx.send(true);
        let handles: Vec<HostSupervisorHandle> = {
            let mut guard = self.supervisors.lock().unwrap();
            let keys: Vec<String> = guard.keys().cloned().collect();
            keys.into_iter().filter_map(|k| guard.remove(&k)).collect()
        };
        for handle in handles {
            let _ = handle.stop_tx.send(true);
            handle.notify.notify_one();
        }
        self.remotes.lock().unwrap().clear();
        if let Some(rt) = self.runtime.take() {
            if tokio::runtime::Handle::try_current().is_ok() {
                let _ = std::thread::spawn(move || {
                    rt.shutdown_timeout(Duration::from_secs(2));
                })
                .join();
            } else {
                rt.shutdown_timeout(Duration::from_secs(2));
            }
        }
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
        let mut initial_entity_hosts = BTreeMap::new();
        for saved in hosts_file.hosts {
            let mut cached = Self::load_cache(&local_home, &saved.id);
            if let Some(c) = &mut cached {
                c.snapshot.register_host_entities(&saved.id);
                for (k, v) in &c.snapshot.entity_hosts {
                    initial_entity_hosts.insert(k.clone(), v.clone());
                }
            }
            let outbox = Arc::new(Mutex::new(Outbox::load(Self::outbox_path(
                &local_home,
                &saved.id,
            ))));
            let read_cache = Arc::new(Mutex::new(ReadCache::open(
                Self::reads_dir(&local_home, &saved.id),
                DEFAULT_READ_CACHE_BUDGET_BYTES,
            )));
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
                    outbox,
                    read_cache,
                    draining: Arc::new(AtomicBool::new(false)),
                    prefetching: Arc::new(AtomicBool::new(false)),
                    connect_gate: Arc::new(Mutex::new(())),
                },
            );
        }

        let initial_host_ids: Vec<String> = map.keys().cloned().collect();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("canban-hosts")
            .build()
            .expect("Could not start HostRegistry Tokio runtime");
        let runtime_handle = runtime.handle().clone();
        let (stop_tx, _) = tokio::sync::watch::channel(false);
        let (push_tx, _) = tokio::sync::broadcast::channel(256);

        let registry = Arc::new(Self {
            local_home,
            local: local.clone(),
            self_weak: OnceLock::new(),
            local_summary_counts: Mutex::new(None),
            remotes: Mutex::new(map),
            entity_hosts: RwLock::new(initial_entity_hosts),
            acknowledged_owners: Mutex::new(BTreeMap::new()),
            listeners: Mutex::new(Vec::new()),
            runtime: Some(runtime),
            runtime_handle,
            stop_tx,
            push_tx,
            supervisors: Mutex::new(BTreeMap::new()),
            stop: AtomicBool::new(false),
        });
        let _ = registry.self_weak.set(Arc::downgrade(&registry));

        registry.runtime_handle.spawn(local_push_supervisor(
            Arc::downgrade(&registry),
            local.transport().clone(),
            registry.stop_tx.subscribe(),
        ));
        for host_id in initial_host_ids {
            registry.ensure_host_supervisor(&host_id);
        }

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

    fn outbox_path(home: &Path, host_id: &str) -> PathBuf {
        home.join("cache/hosts")
            .join(format!("{host_id}.outbox.json"))
    }

    fn reads_dir(home: &Path, host_id: &str) -> PathBuf {
        home.join("cache/hosts").join(host_id).join("reads")
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

    pub fn subscribe_push(&self) -> tokio::sync::broadcast::Receiver<HostPushEvent> {
        self.push_tx.subscribe()
    }

    pub fn runtime_handle(&self) -> &tokio::runtime::Handle {
        &self.runtime_handle
    }

    fn emit(&self, host_id: &str, event: AppEvent) {
        let event = HostEvent {
            host_id: host_id.into(),
            event,
        };
        let _ = self.push_tx.send(HostPushEvent::State(event.clone()));
        self.listeners
            .lock()
            .unwrap()
            .retain(|tx| tx.send(event.clone()).is_ok());
    }

    fn emit_push(&self, event: HostPushEvent) {
        let _ = self.push_tx.send(event);
    }

    fn ensure_host_supervisor(&self, host_id: &str) {
        if self.stop.load(Ordering::SeqCst) {
            return;
        }
        let mut guard = self.supervisors.lock().unwrap();
        if let Some(handle) = guard.get(host_id) {
            handle.notify.notify_one();
            return;
        }
        let Some(weak) = self.self_weak.get().cloned() else {
            return;
        };
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let notify = Arc::new(tokio::sync::Notify::new());
        guard.insert(
            host_id.to_owned(),
            HostSupervisorHandle {
                stop_tx,
                notify: notify.clone(),
            },
        );
        self.runtime_handle.spawn(remote_host_supervisor(
            weak,
            host_id.to_owned(),
            stop_rx,
            self.stop_tx.subscribe(),
            notify,
        ));
    }

    fn stop_host_supervisor(&self, host_id: &str) {
        if let Some(handle) = self.supervisors.lock().unwrap().remove(host_id) {
            let _ = handle.stop_tx.send(true);
            handle.notify.notify_one();
        }
    }

    fn restart_host_supervisor(&self, host_id: &str) {
        self.stop_host_supervisor(host_id);
        self.ensure_host_supervisor(host_id);
    }

    pub fn wake_host(&self, host_id: &str) {
        if let Some(handle) = self.supervisors.lock().unwrap().get(host_id) {
            handle.notify.notify_one();
        }
    }

    pub fn wake(&self) {
        for handle in self.supervisors.lock().unwrap().values() {
            handle.notify.notify_one();
        }
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

        if let Some(new_tok) = client.as_ref().and_then(|c| c.upgraded_token())
            && let HostTransport::Tcp { token, .. } = &mut host.transport
        {
            *token = new_tok.to_owned();
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
            Some(Self::store_cached_snapshot(
                &self.local_home,
                &host.id,
                snap,
                cfg,
            ))
        } else {
            Self::load_cache(&self.local_home, &host.id).map(|mut c| {
                c.snapshot.register_host_entities(&host.id);
                c
            })
        };
        let initial_entities = cached
            .as_ref()
            .map(|c| c.snapshot.entity_hosts.clone())
            .unwrap_or_default();

        let outbox = Arc::new(Mutex::new(Outbox::load(Self::outbox_path(
            &self.local_home,
            &host.id,
        ))));
        let read_cache = Arc::new(Mutex::new(ReadCache::open(
            Self::reads_dir(&self.local_home, &host.id),
            DEFAULT_READ_CACHE_BUDGET_BYTES,
        )));
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
                outbox,
                read_cache,
                draining: Arc::new(AtomicBool::new(false)),
                prefetching: Arc::new(AtomicBool::new(false)),
                connect_gate: Arc::new(Mutex::new(())),
            };
            let sum = Self::slot_summary(&slot);
            guard.insert(slot.saved.id.clone(), slot);
            let all_saved: Vec<SavedHost> = guard.values().map(|s| s.saved.clone()).collect();
            drop(guard);
            self.save_hosts_file(&all_saved)?;
            sum
        };
        if !initial_entities.is_empty() {
            self.replace_host_entity_owners(&summary.id, &initial_entities);
        }

        if summary.connection.is_connected() {
            self.spawn_drain_and_prefetch(&summary.id);
        }
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
        self.restart_host_supervisor(&summary.id);
        Ok(summary)
    }

    pub fn remove_host(&self, host_id: &str) -> Result<()> {
        ensure!(host_id != "local", "Cannot remove This Mac");
        self.stop_host_supervisor(host_id);
        let all_saved = {
            let mut guard = self.remotes.lock().unwrap();
            guard.remove(host_id);
            guard.values().map(|s| s.saved.clone()).collect::<Vec<_>>()
        };
        let _ = std::fs::remove_file(Self::cache_path(&self.local_home, host_id));
        let _ = std::fs::remove_file(Self::outbox_path(&self.local_home, host_id));
        let _ = std::fs::remove_dir_all(self.local_home.join("cache/hosts").join(host_id));
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
        self.wake_host(host_id);
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
            self.wake_host(&id);
        }
    }

    fn store_cached_snapshot(
        local_home: &Path,
        host_id: &str,
        mut snap: Snapshot,
        config: Config,
    ) -> CachedHostData {
        snap.register_host_entities(host_id);
        let data = CachedHostData {
            saved_at: now(),
            snapshot: snap,
            config,
        };
        Self::save_cache(local_home, host_id, &data);
        data
    }

    fn replace_host_entity_owners(&self, host_id: &str, snap_entities: &BTreeMap<String, String>) {
        let mut eh = self.entity_hosts.write().unwrap();
        eh.retain(|k, v| v != host_id || snap_entities.contains_key(k));
        for (k, v) in snap_entities {
            eh.insert(k.clone(), v.clone());
        }
    }

    fn apply_established_slot(
        &self,
        host_id: &str,
        established: EstablishedConnection,
    ) -> (bool, bool) {
        let (client_opt, tunnel, conn_state, state_val, server_id) = established;
        let mut updated_entities = None;
        let mut upgraded_token_saved = false;
        let mut persist_hosts = None;
        let (changed, became_connected) = {
            let mut guard = self.remotes.lock().unwrap();
            let Some(slot) = guard.get_mut(host_id) else {
                return (false, false);
            };
            if slot.updating || !slot.saved.enabled {
                return (false, false);
            }
            let previous = slot.connection.clone();
            let data_changed = state_val
                .as_ref()
                .is_some_and(|v| super::state::state_marker(v) != slot.last_state_marker);
            if let Some(new_tok) = client_opt.as_ref().and_then(|c| c.upgraded_token())
                && let HostTransport::Tcp { token, .. } = &mut slot.saved.transport
                && *token != new_tok
            {
                *token = new_tok.to_owned();
                upgraded_token_saved = true;
            }
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
                    let data = Self::store_cached_snapshot(&self.local_home, host_id, snap, cfg);
                    updated_entities = Some(data.snapshot.entity_hosts.clone());
                    slot.cached = Some(data);
                }
                slot.last_state_marker = marker;
            }
            let became_connected = !previous.is_connected() && slot.connection.is_connected();
            let result = (
                previous != slot.connection || data_changed,
                became_connected,
            );
            if upgraded_token_saved {
                persist_hosts = Some(
                    guard
                        .values()
                        .map(|s| s.saved.clone())
                        .collect::<Vec<SavedHost>>(),
                );
            }
            result
        };
        if let Some(entities) = updated_entities {
            self.replace_host_entity_owners(host_id, &entities);
        }
        if let Some(all_saved) = persist_hosts {
            let _ = self.save_hosts_file(&all_saved);
        }
        (changed, became_connected)
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
        self.wake_host(host_id);

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
                self.wake_host(host_id);
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
                        let (_changed, became_connected) =
                            self.apply_established_slot(host_id, est);
                        if became_connected {
                            self.spawn_drain_and_prefetch(host_id);
                        }
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
            self.wake_host(host_id);
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
            self.wake_host(host_id);
        }
        Ok(res)
    }

    fn establish_connection(
        &self,
        host_id: &str,
        transport: &HostTransport,
    ) -> Result<EstablishedConnection> {
        Self::establish_connection_for(&self.local_home, host_id, transport)
    }

    fn establish_connection_for(
        local_home: &Path,
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
                    &Self::tunnels_dir(local_home),
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

    fn apply_remote_state_response(
        &self,
        host_id: &str,
        client: &Arc<AppClient>,
        mut state_val: Value,
        rtt_ms: u64,
        preferred_topic: Option<&'static str>,
    ) -> Option<&'static str> {
        let mut updated_entities = None;
        let mut emitted_topic = None;
        {
            let mut guard = self.remotes.lock().unwrap();
            let slot = guard.get_mut(host_id)?;
            if slot.updating
                || !slot.saved.enabled
                || !slot.client.as_ref().is_some_and(|c| Arc::ptr_eq(c, client))
            {
                return None;
            }
            if state_val["changed"] == true
                && let (Ok(snap), Ok(cfg)) = (
                    serde_json::from_value::<Snapshot>(state_val["snapshot"].take()),
                    serde_json::from_value::<Config>(state_val["config"].take()),
                )
            {
                client.set_cached_config(cfg.clone());
                let data = Self::store_cached_snapshot(&self.local_home, host_id, snap, cfg);
                updated_entities = Some(data.snapshot.entity_hosts.clone());
                slot.cached = Some(data);
                let topic = if preferred_topic == Some("resync") {
                    "resync"
                } else if slot.last_state_marker["github_revision"] != state_val["github_revision"]
                {
                    "github"
                } else if let Some(t) = preferred_topic {
                    t
                } else {
                    client::change_topic(true, &slot.last_state_marker, &state_val)
                };
                emitted_topic = Some(topic);
                slot.last_state_marker = super::state::state_marker(&state_val);
            }
            if let HostConnectionState::Connected {
                rtt_ms: prev_rtt, ..
            } = &mut slot.connection
            {
                *prev_rtt = rtt_ms;
            }
        }
        if let Some(entities) = updated_entities {
            self.replace_host_entity_owners(host_id, &entities);
        }
        emitted_topic
    }

    fn handle_remote_connected_error(
        &self,
        host_id: &str,
        client: &Arc<AppClient>,
        err: &anyhow::Error,
    ) -> bool {
        let msg = format!("{err:#}");
        let is_auth = msg.contains("auth_failed:");
        let changed = {
            let mut guard = self.remotes.lock().unwrap();
            let Some(slot) = guard.get_mut(host_id) else {
                return false;
            };
            if slot.updating
                || !slot.saved.enabled
                || !slot.client.as_ref().is_some_and(|c| Arc::ptr_eq(c, client))
            {
                return false;
            }
            slot.client = None;
            slot.tunnel = None;
            slot.reconnect_count = slot.reconnect_count.saturating_add(1);
            slot.consecutive_failures = 1;
            slot.next_retry_at =
                std::time::Instant::now() + Duration::from_secs(if is_auth { 4 } else { 1 });
            slot.connection = if is_auth {
                HostConnectionState::AuthFailed { error: msg }
            } else {
                HostConnectionState::Reconnecting {
                    attempt: 1,
                    error: msg,
                }
            };
            true
        };
        if changed {
            self.emit(
                host_id,
                AppEvent::Changed {
                    topic: "host",
                    card_id: None,
                },
            );
        }
        changed
    }

    fn handle_remote_connect_error(&self, host_id: &str, err: &anyhow::Error) {
        let msg = format!("{err:#}");
        let emit_host = {
            let mut guard = self.remotes.lock().unwrap();
            let Some(slot) = guard.get_mut(host_id) else {
                return;
            };
            if slot.updating || !slot.saved.enabled || slot.client.is_some() {
                return;
            }
            slot.consecutive_failures = slot.consecutive_failures.saturating_add(1);
            let backoff_secs = match slot.consecutive_failures {
                1 => 1,
                2 => 2,
                _ => 4,
            };
            slot.next_retry_at = std::time::Instant::now() + Duration::from_secs(backoff_secs);
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
            slot.connection != prev
        };
        if emit_host {
            self.emit(
                host_id,
                AppEvent::Changed {
                    topic: "host",
                    card_id: None,
                },
            );
        }
    }

    /// On-demand synchronous remote probe/sync (background monitoring runs via per-host push tasks).
    pub fn poll_remotes_once(&self) {
        let ids: Vec<String> = {
            let guard = self.remotes.lock().unwrap();
            guard.keys().cloned().collect()
        };

        for id in ids {
            let (saved, existing_client, should_retry, connect_gate) = {
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
                    slot.connect_gate.clone(),
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
                match client.call("app.state", json!({"after": after})) {
                    Ok(state_val) => {
                        let rtt_ms = start.elapsed().as_millis() as u64;
                        if let Some(topic) =
                            self.apply_remote_state_response(&id, &client, state_val, rtt_ms, None)
                        {
                            self.emit(
                                &id,
                                AppEvent::Changed {
                                    topic,
                                    card_id: None,
                                },
                            );
                        }
                    }
                    Err(err) => {
                        if self.handle_remote_connected_error(&id, &client, &err) {
                            self.wake_host(&id);
                        }
                    }
                }
                if self.has_drainable_outbox(&id) {
                    self.spawn_drain_and_prefetch(&id);
                }
                continue;
            }

            if !should_retry {
                continue;
            }

            let _gate = connect_gate.lock().unwrap();
            let still_due = {
                let guard = self.remotes.lock().unwrap();
                guard.get(&id).is_some_and(|slot| {
                    slot.saved.enabled
                        && !slot.updating
                        && slot.client.is_none()
                        && std::time::Instant::now() >= slot.next_retry_at
                })
            };
            if !still_due {
                continue;
            }

            match Self::establish_connection_for(&self.local_home, &id, &saved.transport) {
                Ok(established) => {
                    let (changed, connected) = self.apply_established_slot(&id, established);
                    if connected {
                        self.spawn_drain_and_prefetch(&id);
                        self.wake_host(&id);
                    }
                    if changed {
                        self.emit(
                            &id,
                            AppEvent::Changed {
                                topic: if connected { "resync" } else { "host" },
                                card_id: None,
                            },
                        );
                    }
                }
                Err(err) => {
                    self.handle_remote_connect_error(&id, &err);
                }
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
        let (pending_mutations, sync_conflicts) = {
            let ob = slot.outbox.lock().unwrap();
            (ob.pending_count(), ob.conflict_count())
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
            pending_mutations,
            sync_conflicts,
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
            pending_mutations: 0,
            sync_conflicts: 0,
        }];
        let guard = self.remotes.lock().unwrap();
        for slot in guard.values() {
            out.push(Self::slot_summary(slot));
        }
        out
    }

    /// Build the unified `Snapshot` across `This Mac` and all enabled remote hosts
    /// (live when connected, cached read-only when offline, with optimistic queued sync projection)
    /// and refresh the O(1) routing table.
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
                pending_mutations: 0,
                sync_conflicts: 0,
            },
        );

        let remote_snapshots: Vec<(Option<Snapshot>, HostSummary)> = {
            let guard = self.remotes.lock().unwrap();
            guard
                .values()
                .filter(|slot| slot.saved.enabled)
                .map(|slot| {
                    let ops = slot.outbox.lock().unwrap().ops.clone();
                    let snap = if let Some(c) = &slot.cached {
                        let mut s = c.snapshot.clone();
                        if !ops.is_empty() {
                            canban_sync::apply(&mut s, &slot.saved.id, &ops);
                        }
                        Some(s)
                    } else if !ops.is_empty() {
                        let mut s = Snapshot::default();
                        canban_sync::apply(&mut s, &slot.saved.id, &ops);
                        Some(s)
                    } else {
                        None
                    };
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
            let data = Self::store_cached_snapshot(&self.local_home, host_id, snap, cfg);
            self.replace_host_entity_owners(host_id, &data.snapshot.entity_hosts);
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
        if host_id == "local" {
            return self.local.call(method, params);
        }

        let (host_name, client_opt, outbox_arc, read_cache_arc, base_snap, status_str) = {
            let guard = self.remotes.lock().unwrap();
            let slot = guard
                .get(&host_id)
                .with_context(|| format!("Unknown Canban host '{host_id}'"))?;
            (
                slot.saved.name.clone(),
                slot.client.clone(),
                slot.outbox.clone(),
                slot.read_cache.clone(),
                slot.cached.as_ref().map(|c| c.snapshot.clone()),
                slot.connection.short_status(),
            )
        };
        let unavailable_err =
            || anyhow::anyhow!("Host '{host_name}' is currently unavailable ({status_str})");

        if method == "card.resolve.preview" {
            if let Some(client) = client_opt {
                match client.call(method, params.clone()) {
                    Ok(res) => return Ok(res),
                    Err(err) if is_connection_error(&err) => {
                        self.mark_host_reconnecting(&host_id, &err);
                    }
                    Err(err) => return Err(err),
                }
            }
            let card_id = strp(&params, "card_id")?;
            let ops = outbox_arc.lock().unwrap().ops.clone();
            let mut snap = base_snap.unwrap_or_default();
            canban_sync::apply(&mut snap, &host_id, &ops);
            return canban_sync::offline_resolve_preview(&snap, card_id, &host_name);
        }

        if canban_sync::is_read_cacheable_method(method) {
            let card_id_opt = params["card_id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let mut conn_err: Option<anyhow::Error> = None;
            if let Some(client) = client_opt {
                match client.call(method, params.clone()) {
                    Ok(mut val) => {
                        let card_rev = if method == "timeline.list"
                            && params["tail"] == true
                            && params.get("before_sequence").is_none()
                            && params.get("after_sequence").is_none()
                        {
                            card_id_opt.as_deref().and_then(|cid| {
                                base_snap
                                    .as_ref()
                                    .and_then(|s| s.cards.iter().find(|c| c.id == cid))
                                    .map(|c| c.revision)
                            })
                        } else {
                            None
                        };
                        let _ = read_cache_arc.lock().unwrap().put(
                            method,
                            &params,
                            &val,
                            card_id_opt.as_deref(),
                            card_rev,
                            now(),
                        );
                        if method == "timeline.list"
                            && let Some(cid) = card_id_opt.as_deref()
                        {
                            let ops = outbox_arc.lock().unwrap().ops.clone();
                            let snap = base_snap.unwrap_or_default();
                            enrich_timeline_with_pending(&mut val, &ops, cid, &snap);
                        }
                        return Ok(val);
                    }
                    Err(err) if is_connection_error(&err) => {
                        self.mark_host_reconnecting(&host_id, &err);
                        conn_err = Some(err);
                    }
                    Err(err) => return Err(err),
                }
            }
            let mut cached = read_cache_arc.lock().unwrap().get(method, &params, now());
            if method == "timeline.list" {
                let ops = outbox_arc.lock().unwrap().ops.clone();
                if cached.is_none()
                    && card_id_opt.as_deref().is_some_and(|cid| {
                        ops.iter()
                            .any(|m| m.card_id == cid && m.kind == MutationKind::CreateCard)
                    })
                {
                    cached = Some(json!({
                        "events": [],
                        "next_cursor": Value::Null,
                        "last_sequence": 0,
                        "highwater": 0,
                        "has_more_before": false,
                        "oldest_sequence": Value::Null,
                    }));
                }
                if let Some(mut val) = cached {
                    if let Some(cid) = card_id_opt.as_deref() {
                        let snap = base_snap.unwrap_or_default();
                        enrich_timeline_with_pending(&mut val, &ops, cid, &snap);
                    }
                    return Ok(val);
                }
            } else if let Some(val) = cached {
                return Ok(val);
            }
            return Err(conn_err.unwrap_or_else(unavailable_err));
        }

        let res = match canban_sync::classify_remote_mutation(method, &params) {
            QueueDecision::RejectOffline(msg) => {
                if let Some(client) = client_opt {
                    match client.call(method, params) {
                        Ok(res) => res,
                        Err(err) if is_connection_error(&err) => {
                            self.mark_host_reconnecting(&host_id, &err);
                            bail!("{msg} ({host_name})");
                        }
                        Err(err) => return Err(err),
                    }
                } else {
                    bail!("{msg} ({host_name})");
                }
            }
            QueueDecision::NotQueueable => {
                let client = client_opt.ok_or_else(unavailable_err)?;
                client.call(method, params)?
            }
            QueueDecision::Queue(kind) => {
                let request_key = params["request_key"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(canban_types::uuid_v4);
                params["request_key"] = json!(request_key);
                if matches!(kind, MutationKind::CreateCard | MutationKind::AddComment)
                    && params
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .is_none_or(str::is_empty)
                {
                    params["id"] = json!(canban_types::id());
                }
                let card_id = if kind == MutationKind::CreateCard {
                    params["id"].as_str().unwrap_or_default().to_owned()
                } else {
                    params["card_id"].as_str().unwrap_or_default().to_owned()
                };

                // Duplicate `mutation_id`: return synthetic receipt without duplicating.
                {
                    let ob = outbox_arc.lock().unwrap();
                    if let Some(existing) = ob.ops.iter().find(|m| m.id == request_key).cloned() {
                        let base = base_snap.unwrap_or_default();
                        return canban_sync::synthetic_receipt(&existing, &base, &ob.ops, &host_id);
                    }
                }

                let has_pending_for_card = outbox_arc
                    .lock()
                    .unwrap()
                    .ops
                    .iter()
                    .any(|m| m.card_id == card_id);

                let (live_reply, sent_before_drop) =
                    if let (Some(client), false) = (client_opt.as_ref(), has_pending_for_card) {
                        match client.call(method, params.clone()) {
                            Ok(res) => (Some(res), false),
                            Err(err) if is_connection_error(&err) => {
                                self.mark_host_reconnecting(&host_id, &err);
                                (None, true)
                            }
                            Err(err) => return Err(err),
                        }
                    } else {
                        (None, false)
                    };

                if let Some(res) = live_reply {
                    res
                } else {
                    let base = if kind == MutationKind::UpdateCard {
                        let mut projected = base_snap.clone().unwrap_or_default();
                        let existing_ops = outbox_arc.lock().unwrap().ops.clone();
                        canban_sync::apply(&mut projected, &host_id, &existing_ops);
                        projected
                            .cards
                            .iter()
                            .find(|c| c.id == card_id)
                            .map(|c| UpdateBase {
                                title: Some(c.title.clone()),
                                body: Some(c.body.clone()),
                            })
                            .unwrap_or_default()
                    } else {
                        UpdateBase::default()
                    };
                    let mutation = Mutation {
                        id: request_key,
                        host_id: host_id.clone(),
                        card_id: card_id.clone(),
                        kind,
                        params: params.clone(),
                        base,
                        sent_params: if sent_before_drop {
                            params.clone()
                        } else {
                            Value::Null
                        },
                        retry_same_params: sent_before_drop,
                        state: MutationState::Queued,
                        created_at: now(),
                        attempts: u32::from(sent_before_drop),
                    };
                    let (stored, ops_snapshot) = {
                        let mut ob = outbox_arc.lock().unwrap();
                        let (stored, _) = ob.enqueue(mutation)?;
                        (stored, ob.ops.clone())
                    };
                    if kind == MutationKind::CreateCard && !card_id.is_empty() {
                        self.acknowledged_owners
                            .lock()
                            .unwrap()
                            .insert(card_id.clone(), host_id.clone());
                        self.entity_hosts
                            .write()
                            .unwrap()
                            .insert(card_id.clone(), host_id.clone());
                    }
                    self.emit(
                        &host_id,
                        AppEvent::Changed {
                            topic: "application",
                            card_id: (!card_id.is_empty()).then_some(card_id),
                        },
                    );
                    if client_opt.is_some() {
                        self.spawn_drain_and_prefetch(&host_id);
                    }
                    let base = base_snap.unwrap_or_default();
                    return canban_sync::synthetic_receipt(&stored, &base, &ops_snapshot, &host_id);
                }
            }
        };
        if host_id != "local"
            && !matches!(
                method,
                "app.ping"
                    | "app.state"
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
                self.wake_host(&host_id);
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
                self.wake_host(&host_id);
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

    fn mark_host_reconnecting(&self, host_id: &str, err: &anyhow::Error) {
        let msg = format!("{err:#}");
        let mut changed = false;
        {
            let mut guard = self.remotes.lock().unwrap();
            if let Some(slot) = guard.get_mut(host_id)
                && !slot.updating
            {
                slot.client = None;
                slot.tunnel = None;
                slot.reconnect_count = slot.reconnect_count.saturating_add(1);
                slot.consecutive_failures = 1;
                slot.next_retry_at = std::time::Instant::now() + Duration::from_secs(1);
                slot.connection = HostConnectionState::Reconnecting {
                    attempt: 1,
                    error: msg,
                };
                changed = true;
            }
        }
        if changed {
            self.emit(
                host_id,
                AppEvent::Changed {
                    topic: "host",
                    card_id: None,
                },
            );
            self.wake_host(host_id);
        }
    }

    fn cached_snapshot_for(&self, host_id: &str) -> Option<Snapshot> {
        self.remotes
            .lock()
            .unwrap()
            .get(host_id)
            .and_then(|s| s.cached.as_ref().map(|c| c.snapshot.clone()))
    }

    fn has_drainable_outbox(&self, host_id: &str) -> bool {
        let Some(ob_arc) = self
            .remotes
            .lock()
            .unwrap()
            .get(host_id)
            .map(|s| s.outbox.clone())
        else {
            return false;
        };
        let ob = ob_arc.lock().unwrap();
        let blocked = ob.blocked_cards();
        ob.ops.iter().any(|m| {
            !blocked.contains(&m.card_id)
                && matches!(m.state, MutationState::Queued | MutationState::InFlight)
        })
    }

    fn spawn_drain_and_prefetch(&self, host_id: &str) {
        let Some(weak) = self.self_weak.get().cloned() else {
            return;
        };
        let hid = host_id.to_owned();
        drop(self.runtime_handle.spawn_blocking(move || {
            Self::run_drain_and_prefetch_weak(&weak, &hid);
        }));
    }

    fn run_drain_and_prefetch_weak(weak: &Weak<Self>, host_id: &str) {
        let Some(reg) = weak.upgrade() else {
            return;
        };
        if reg.stop.load(Ordering::SeqCst) {
            return;
        }
        let (outbox_arc, read_cache_arc, draining_flag, prefetching_flag) = {
            let guard = reg.remotes.lock().unwrap();
            let Some(slot) = guard.get(host_id) else {
                return;
            };
            if slot.client.is_none() {
                return;
            }
            (
                slot.outbox.clone(),
                slot.read_cache.clone(),
                slot.draining.clone(),
                slot.prefetching.clone(),
            )
        };
        drop(reg);

        loop {
            let Some(reg) = weak.upgrade() else {
                return;
            };
            if reg.stop.load(Ordering::SeqCst) {
                return;
            }
            if draining_flag
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                break;
            }
            let drain_guard = AtomicFlagGuard(draining_flag.clone());
            let mut stopped_on_conn_err = false;
            if reg.has_drainable_outbox(host_id) {
                reg.sync_host_now(host_id);
                let mut snapshot = reg.cached_snapshot_for(host_id).unwrap_or_default();
                let mut report = DrainReport::default();
                let mut blocked = BTreeSet::new();
                let mut rebuilt_ids = BTreeSet::new();
                drop(reg);

                loop {
                    let Some(reg) = weak.upgrade() else {
                        break;
                    };
                    if reg.stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let client = {
                        let guard = reg.remotes.lock().unwrap();
                        guard.get(host_id).and_then(|s| s.client.clone())
                    };
                    drop(reg);
                    let Some(client) = client else {
                        break;
                    };

                    let step = {
                        let mut ob = outbox_arc.lock().unwrap();
                        prepare_next_step(&mut ob, &snapshot, &mut blocked, &mut report)
                    };
                    let StepAction::Send {
                        mutation_id,
                        method,
                        sent_params,
                        was_retry_same_params,
                    } = step
                    else {
                        break;
                    };

                    let allow_rebuild = !rebuilt_ids.contains(&mutation_id);
                    let result = client.call(method, sent_params);

                    let Some(reg) = weak.upgrade() else {
                        break;
                    };
                    let (is_conn_err, refreshed) = match &result {
                        Ok(_) => (false, None),
                        Err(err) => {
                            let conn = is_connection_error(err);
                            if conn {
                                reg.mark_host_reconnecting(host_id, err);
                            }
                            let msg = format!("{err:#}");
                            let need_refresh = !conn
                                && ((msg.contains("revision_conflict")
                                    && !was_retry_same_params
                                    && allow_rebuild)
                                    || msg.contains("request_key_conflict"));
                            let snap = if need_refresh {
                                reg.sync_host_now(host_id);
                                reg.cached_snapshot_for(host_id)
                            } else {
                                None
                            };
                            (conn, snap)
                        }
                    };
                    let outcome = {
                        let mut guard = reg.remotes.lock().unwrap();
                        let mut ob = outbox_arc.lock().unwrap();
                        let outcome = finish_step(
                            &mut ob,
                            &mut snapshot,
                            &mutation_id,
                            result,
                            was_retry_same_params,
                            is_conn_err,
                            refreshed,
                            allow_rebuild,
                            &mut blocked,
                            &mut report,
                        );
                        if let Some(slot) = guard.get_mut(host_id)
                            && let Some(cached) = slot.cached.as_mut()
                        {
                            let mut next_snap = snapshot.clone();
                            next_snap.register_host_entities(host_id);
                            cached.snapshot = next_snap;
                        }
                        outcome
                    };
                    drop(reg);
                    match outcome {
                        FinishOutcome::Continue => {}
                        FinishOutcome::RetryImmediate => {
                            rebuilt_ids.insert(mutation_id);
                        }
                        FinishOutcome::StopConnection => {
                            stopped_on_conn_err = true;
                            break;
                        }
                    }
                }

                if (report.applied > 0 || report.conflicts > 0 || report.failed > 0)
                    && let Some(reg) = weak.upgrade()
                {
                    reg.sync_host_now(host_id);
                    reg.emit(
                        host_id,
                        AppEvent::Changed {
                            topic: "application",
                            card_id: None,
                        },
                    );
                }
            } else {
                drop(reg);
            }
            drop(drain_guard);
            let should_continue = !stopped_on_conn_err
                && weak.upgrade().is_some_and(|reg| {
                    !reg.stop.load(Ordering::SeqCst)
                        && reg
                            .remotes
                            .lock()
                            .unwrap()
                            .get(host_id)
                            .is_some_and(|s| s.client.is_some())
                        && reg.has_drainable_outbox(host_id)
                });
            if !should_continue {
                break;
            }
        }

        // Sequential timeline prefetch for active Cards when not draining.
        if prefetching_flag
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let _prefetch_guard = AtomicFlagGuard(prefetching_flag);
        let Some(snap) = weak
            .upgrade()
            .and_then(|reg| reg.cached_snapshot_for(host_id))
        else {
            return;
        };
        let targets: Vec<(String, u64)> = snap
            .cards
            .iter()
            .filter(|c| c.active())
            .map(|c| (c.id.clone(), c.revision))
            .collect();
        for (card_id, rev) in targets {
            let client = {
                let Some(reg) = weak.upgrade() else {
                    break;
                };
                if reg.stop.load(Ordering::SeqCst)
                    || draining_flag.load(Ordering::SeqCst)
                    || reg.has_drainable_outbox(host_id)
                {
                    break;
                }
                let guard = reg.remotes.lock().unwrap();
                guard.get(host_id).and_then(|s| s.client.clone())
            };
            let Some(client) = client else {
                break;
            };
            if read_cache_arc
                .lock()
                .unwrap()
                .has_fresh_timeline(&card_id, rev)
            {
                continue;
            }
            let params = json!({"card_id": card_id, "tail": true, "limit": 200, "view": "full"});
            match client.call("timeline.list", params.clone()) {
                Ok(val) => {
                    let _ = read_cache_arc.lock().unwrap().put(
                        "timeline.list",
                        &params,
                        &val,
                        Some(&card_id),
                        Some(rev),
                        now(),
                    );
                }
                Err(err) if is_connection_error(&err) => break,
                Err(_) => {}
            }
        }
    }

    pub fn sync_items(&self) -> Vec<SyncItemView> {
        let slots: Vec<(String, String, Option<Snapshot>, Vec<Mutation>)> = {
            let guard = self.remotes.lock().unwrap();
            guard
                .values()
                .map(|slot| {
                    (
                        slot.saved.id.clone(),
                        slot.saved.name.clone(),
                        slot.cached.as_ref().map(|c| c.snapshot.clone()),
                        slot.outbox.lock().unwrap().ops.clone(),
                    )
                })
                .collect()
        };
        let mut items = Vec::new();
        for (host_id, host_name, cached_snap, ops) in slots {
            let mut projected = cached_snap.clone().unwrap_or_default();
            canban_sync::apply(&mut projected, &host_id, &ops);
            for op in &ops {
                let server_card = cached_snap
                    .as_ref()
                    .and_then(|s| s.cards.iter().find(|c| c.id == op.card_id));
                let card_title = op
                    .params
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .or_else(|| {
                        projected
                            .cards
                            .iter()
                            .find(|c| c.id == op.card_id)
                            .map(|c| c.title.clone())
                    })
                    .unwrap_or_else(|| op.card_id.clone());
                let (state, message, conflict_fields, mine, server) = match &op.state {
                    MutationState::Queued => (
                        SyncItemState::Queued,
                        "Waiting to sync".to_owned(),
                        Vec::new(),
                        BTreeMap::new(),
                        BTreeMap::new(),
                    ),
                    MutationState::InFlight => (
                        SyncItemState::InFlight,
                        "Syncing…".to_owned(),
                        Vec::new(),
                        BTreeMap::new(),
                        BTreeMap::new(),
                    ),
                    MutationState::Conflict { fields } => {
                        let (cf, mine, srv) = canban_sync::conflict_details(op, server_card);
                        (
                            SyncItemState::Conflict,
                            format!("Conflict on {}", fields.join(", ")),
                            cf,
                            mine,
                            srv,
                        )
                    }
                    MutationState::Failed { message } => (
                        SyncItemState::Failed,
                        message.clone(),
                        Vec::new(),
                        BTreeMap::new(),
                        BTreeMap::new(),
                    ),
                };
                items.push(SyncItemView {
                    host_id: host_id.clone(),
                    host_name: host_name.clone(),
                    mutation_id: op.id.clone(),
                    card_id: op.card_id.clone(),
                    card_title,
                    kind: op.kind.label().to_owned(),
                    state,
                    message,
                    conflict_fields,
                    mine,
                    server,
                });
            }
        }
        items
    }

    pub fn sync_action(&self, host_id: &str, mutation_id: &str, action: SyncAction) -> Result<()> {
        let (outbox_arc, connected) = {
            let guard = self.remotes.lock().unwrap();
            let slot = guard
                .get(host_id)
                .with_context(|| format!("Unknown Canban host '{host_id}'"))?;
            (slot.outbox.clone(), slot.connection.is_connected())
        };
        {
            let mut ob = outbox_arc.lock().unwrap();
            match action {
                SyncAction::Discard => {
                    ob.remove_by_id(mutation_id)?
                        .with_context(|| format!("Unknown mutation '{mutation_id}'"))?;
                }
                SyncAction::Retry => {
                    let op = ob
                        .ops
                        .iter_mut()
                        .find(|m| m.id == mutation_id)
                        .with_context(|| format!("Unknown mutation '{mutation_id}'"))?;
                    if matches!(op.state, MutationState::Failed { .. }) {
                        op.retry_same_params = false;
                    }
                    op.state = MutationState::Queued;
                    ob.save()?;
                }
                SyncAction::UseMine => {
                    let op = ob
                        .ops
                        .iter_mut()
                        .find(|m| m.id == mutation_id)
                        .with_context(|| format!("Unknown mutation '{mutation_id}'"))?;
                    if let Some(obj) = op.params.as_object_mut() {
                        if let Some(base_t) = op.base.title.as_deref()
                            && obj
                                .get("title")
                                .and_then(Value::as_str)
                                .is_some_and(|t| t.trim() == base_t.trim())
                        {
                            obj.remove("title");
                        }
                        if let Some(base_b) = op.base.body.as_deref()
                            && obj.get("body").and_then(Value::as_str) == Some(base_b)
                        {
                            obj.remove("body");
                        }
                        obj.insert("force".into(), json!(true));
                    }
                    op.retry_same_params = false;
                    op.state = MutationState::Queued;
                    ob.save()?;
                }
                SyncAction::UseEdited { fields } => {
                    let op = ob
                        .ops
                        .iter_mut()
                        .find(|m| m.id == mutation_id)
                        .with_context(|| format!("Unknown mutation '{mutation_id}'"))?;
                    if let Some(obj) = op.params.as_object_mut() {
                        if !fields.contains_key("title")
                            && let Some(base_t) = op.base.title.as_deref()
                            && obj
                                .get("title")
                                .and_then(Value::as_str)
                                .is_some_and(|t| t.trim() == base_t.trim())
                        {
                            obj.remove("title");
                        }
                        if !fields.contains_key("body")
                            && let Some(base_b) = op.base.body.as_deref()
                            && obj.get("body").and_then(Value::as_str) == Some(base_b)
                        {
                            obj.remove("body");
                        }
                        for (k, v) in fields {
                            if matches!(k.as_str(), "title" | "body") {
                                obj.insert(k, Value::String(v));
                            }
                        }
                        obj.insert("force".into(), json!(true));
                    }
                    op.retry_same_params = false;
                    op.state = MutationState::Queued;
                    ob.save()?;
                }
            }
        }
        self.emit(
            host_id,
            AppEvent::Changed {
                topic: "application",
                card_id: None,
            },
        );
        if connected {
            self.spawn_drain_and_prefetch(host_id);
        }
        Ok(())
    }
}

const REMOTE_PUSH_DEBOUNCE: Duration = Duration::from_millis(30);
const REMOTE_PUSH_MAX_DEBOUNCE: Duration = Duration::from_millis(100);

fn merge_push_topic(current: &'static str, incoming: &'static str) -> &'static str {
    if current == "resync" || incoming == "resync" {
        "resync"
    } else if current == "github" || incoming == "github" {
        "github"
    } else {
        incoming
    }
}

enum RemoteSupervisorTarget {
    Idle,
    Connected {
        client: Arc<AppClient>,
        after_revision: Value,
        has_tunnel: bool,
    },
    Disconnected {
        transport: HostTransport,
        local_home: PathBuf,
        next_retry_at: std::time::Instant,
        connect_gate: Arc<Mutex<()>>,
    },
}

struct PendingRefresh {
    topic: &'static str,
    card_id: Option<String>,
    latest_revision: Value,
    deadline: tokio::time::Instant,
    max_deadline: tokio::time::Instant,
}

async fn local_push_supervisor(
    weak: Weak<HostRegistry>,
    transport: Arc<canban_transport::ClientTransport>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut last = Value::Null;

    loop {
        if *stop_rx.borrow() || weak.strong_count() == 0 {
            return;
        }
        let open_res = tokio::select! {
            biased;
            _ = stop_rx.changed() => return,
            res = client::open_async_subscribe(
                &transport,
                &last["revision"],
                &["snapshot", "update", "utilities", "usage", "herdr"],
                None,
            ) => res,
        };
        let (mut stream, handshake) = match open_res {
            Ok(pair) => pair,
            Err(_) => {
                tokio::select! {
                    biased;
                    _ = stop_rx.changed() => return,
                    _ = tokio::time::sleep(Duration::from_millis(500)) => continue,
                }
            }
        };

        if let Some(reg) = weak.upgrade() {
            let topic = if handshake.changed {
                client::change_topic(false, &last, &handshake.result)
            } else {
                "resync"
            };
            last = handshake.marker;
            reg.emit(
                "local",
                AppEvent::Changed {
                    topic,
                    card_id: None,
                },
            );
        } else {
            return;
        }

        loop {
            let frame_res = tokio::select! {
                biased;
                _ = stop_rx.changed() => return,
                res = stream.next_frame() => res,
            };
            match frame_res {
                Ok(Some(frame)) => {
                    let Some(reg) = weak.upgrade() else {
                        return;
                    };
                    match frame {
                        client::PushFrame::Heartbeat => {}
                        client::PushFrame::StateChanged {
                            topic,
                            card_id,
                            marker,
                        } => {
                            last = marker;
                            reg.emit("local", AppEvent::Changed { topic, card_id });
                        }
                        client::PushFrame::UpdateChanged(state) => {
                            reg.emit_push(HostPushEvent::UpdateChanged {
                                host_id: "local".into(),
                                state,
                            });
                        }
                        client::PushFrame::UtilitiesChanged(state) => {
                            reg.emit_push(HostPushEvent::UtilitiesChanged {
                                host_id: "local".into(),
                                state,
                            });
                        }
                        client::PushFrame::UsageChanged(state) => {
                            reg.emit_push(HostPushEvent::UsageChanged {
                                host_id: "local".into(),
                                state,
                            });
                        }
                        client::PushFrame::HerdrInstall(state) => {
                            reg.emit_push(HostPushEvent::HerdrInstall {
                                host_id: "local".into(),
                                state,
                            });
                        }
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }

        tokio::select! {
            biased;
            _ = stop_rx.changed() => return,
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
    }
}

async fn refresh_remote_host_async(
    weak: &Weak<HostRegistry>,
    host_id: &str,
    client: &Arc<AppClient>,
    preferred_topic: Option<&'static str>,
    card_id: Option<String>,
    host_stop_rx: &mut tokio::sync::watch::Receiver<bool>,
    global_stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    let after = {
        let Some(reg) = weak.upgrade() else {
            return false;
        };
        let guard = reg.remotes.lock().unwrap();
        let Some(slot) = guard.get(host_id) else {
            return false;
        };
        if !slot.saved.enabled
            || slot.updating
            || !slot.client.as_ref().is_some_and(|c| Arc::ptr_eq(c, client))
        {
            return false;
        }
        slot.last_state_marker["revision"].clone()
    };

    let rpc_client = client.clone();
    let join = tokio::task::spawn_blocking(move || {
        let start = std::time::Instant::now();
        let res = rpc_client.call("app.state", json!({"after": after}));
        (res, start.elapsed().as_millis() as u64)
    });
    let (rpc_res, rtt_ms) = tokio::select! {
        biased;
        _ = global_stop_rx.changed() => return false,
        _ = host_stop_rx.changed() => return false,
        res = join => match res {
            Ok(pair) => pair,
            Err(_) => return false,
        },
    };

    let Some(reg) = weak.upgrade() else {
        return false;
    };
    match rpc_res {
        Ok(state_val) => {
            if let Some(emitted_topic) =
                reg.apply_remote_state_response(host_id, client, state_val, rtt_ms, preferred_topic)
            {
                reg.emit(
                    host_id,
                    AppEvent::Changed {
                        topic: emitted_topic,
                        card_id,
                    },
                );
            }
            if reg.has_drainable_outbox(host_id) {
                reg.spawn_drain_and_prefetch(host_id);
            }
            true
        }
        Err(err) => {
            reg.handle_remote_connected_error(host_id, client, &err);
            false
        }
    }
}

async fn remote_host_supervisor(
    weak: Weak<HostRegistry>,
    host_id: String,
    mut host_stop_rx: tokio::sync::watch::Receiver<bool>,
    mut global_stop_rx: tokio::sync::watch::Receiver<bool>,
    notify: Arc<tokio::sync::Notify>,
) {
    loop {
        if *global_stop_rx.borrow() || *host_stop_rx.borrow() {
            return;
        }
        let target = {
            let Some(reg) = weak.upgrade() else {
                return;
            };
            let mut guard = reg.remotes.lock().unwrap();
            let Some(slot) = guard.get_mut(&host_id) else {
                return;
            };
            if !slot.saved.enabled || slot.updating {
                RemoteSupervisorTarget::Idle
            } else {
                if let Some(tun) = &mut slot.tunnel
                    && !tun.is_alive()
                {
                    slot.client = None;
                    slot.tunnel = None;
                }
                if let Some(client) = &slot.client {
                    RemoteSupervisorTarget::Connected {
                        client: client.clone(),
                        after_revision: slot.last_state_marker["revision"].clone(),
                        has_tunnel: slot.tunnel.is_some(),
                    }
                } else {
                    RemoteSupervisorTarget::Disconnected {
                        transport: slot.saved.transport.clone(),
                        local_home: reg.local_home.clone(),
                        next_retry_at: slot.next_retry_at,
                        connect_gate: slot.connect_gate.clone(),
                    }
                }
            }
        };

        match target {
            RemoteSupervisorTarget::Idle => {
                tokio::select! {
                    biased;
                    _ = global_stop_rx.changed() => return,
                    _ = host_stop_rx.changed() => return,
                    _ = notify.notified() => continue,
                }
            }
            RemoteSupervisorTarget::Disconnected {
                transport,
                local_home,
                next_retry_at,
                connect_gate,
            } => {
                if std::time::Instant::now() < next_retry_at {
                    tokio::select! {
                        biased;
                        _ = global_stop_rx.changed() => return,
                        _ = host_stop_rx.changed() => return,
                        _ = notify.notified() => continue,
                        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(next_retry_at)) => continue,
                    }
                }

                let hid = host_id.clone();
                let weak_blocking = weak.clone();
                let join = tokio::task::spawn_blocking(move || {
                    let _gate = connect_gate.lock().unwrap();
                    if let Some(reg) = weak_blocking.upgrade() {
                        let still_due = {
                            let guard = reg.remotes.lock().unwrap();
                            guard.get(&hid).is_some_and(|slot| {
                                slot.saved.enabled
                                    && !slot.updating
                                    && slot.client.is_none()
                                    && std::time::Instant::now() >= slot.next_retry_at
                            })
                        };
                        drop(reg);
                        if !still_due {
                            return Ok(None);
                        }
                    } else {
                        return Ok(None);
                    }
                    HostRegistry::establish_connection_for(&local_home, &hid, &transport).map(Some)
                });
                let est_res = tokio::select! {
                    biased;
                    _ = global_stop_rx.changed() => return,
                    _ = host_stop_rx.changed() => return,
                    res = join => match res {
                        Ok(r) => r,
                        Err(_) => return,
                    },
                };
                let Some(reg) = weak.upgrade() else {
                    return;
                };
                match est_res {
                    Ok(Some(established)) => {
                        let (changed, connected) =
                            reg.apply_established_slot(&host_id, established);
                        if connected {
                            reg.spawn_drain_and_prefetch(&host_id);
                        }
                        if changed {
                            reg.emit(
                                &host_id,
                                AppEvent::Changed {
                                    topic: if connected { "resync" } else { "host" },
                                    card_id: None,
                                },
                            );
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        reg.handle_remote_connect_error(&host_id, &err);
                    }
                }
            }
            RemoteSupervisorTarget::Connected {
                client,
                after_revision,
                has_tunnel,
            } => {
                let open_fut = client::open_async_subscribe(
                    client.transport(),
                    &after_revision,
                    &["snapshot", "usage"],
                    None,
                );
                tokio::pin!(open_fut);
                let subscribe_res = loop {
                    tokio::select! {
                        biased;
                        _ = global_stop_rx.changed() => return,
                        _ = host_stop_rx.changed() => return,
                        _ = notify.notified() => {
                            let keep_going = weak.upgrade().is_some_and(|reg| {
                                let guard = reg.remotes.lock().unwrap();
                                guard.get(&host_id).is_some_and(|s| {
                                    s.saved.enabled
                                        && !s.updating
                                        && s.client.as_ref().is_some_and(|c| Arc::ptr_eq(c, &client))
                                })
                            });
                            if !keep_going {
                                break None;
                            }
                        }
                        res = &mut open_fut => break Some(res),
                    }
                };
                let Some(subscribe_res) = subscribe_res else {
                    continue;
                };
                let (mut stream, handshake) = match subscribe_res {
                    Ok(pair) => pair,
                    Err(err) => {
                        if let Some(reg) = weak.upgrade() {
                            reg.handle_remote_connected_error(&host_id, &client, &err);
                        }
                        continue;
                    }
                };

                if handshake.changed
                    && !refresh_remote_host_async(
                        &weak,
                        &host_id,
                        &client,
                        None,
                        None,
                        &mut host_stop_rx,
                        &mut global_stop_rx,
                    )
                    .await
                {
                    continue;
                }
                if let Some(reg) = weak.upgrade()
                    && reg.has_drainable_outbox(&host_id)
                {
                    reg.spawn_drain_and_prefetch(&host_id);
                }

                let mut pending: Option<PendingRefresh> = None;
                loop {
                    tokio::select! {
                        biased;
                        _ = global_stop_rx.changed() => return,
                        _ = host_stop_rx.changed() => return,
                        _ = notify.notified() => {
                            let Some(reg) = weak.upgrade() else {
                                return;
                            };
                            let keep_going = {
                                let guard = reg.remotes.lock().unwrap();
                                guard.get(&host_id).is_some_and(|s| {
                                    s.saved.enabled
                                        && !s.updating
                                        && s.client.as_ref().is_some_and(|c| Arc::ptr_eq(c, &client))
                                })
                            };
                            if !keep_going {
                                break;
                            }
                            if reg.has_drainable_outbox(&host_id) {
                                reg.spawn_drain_and_prefetch(&host_id);
                            }
                        }
                        _ = async {
                            match pending.as_ref() {
                                Some(p) => tokio::time::sleep_until(p.deadline).await,
                                None => std::future::pending::<()>().await,
                            }
                        } => {
                            let Some(p) = pending.take() else {
                                continue;
                            };
                            let (still_active, already_fetched) = {
                                let Some(reg) = weak.upgrade() else {
                                    return;
                                };
                                let guard = reg.remotes.lock().unwrap();
                                let Some(slot) = guard.get(&host_id) else {
                                    break;
                                };
                                let active = slot.saved.enabled
                                    && !slot.updating
                                    && slot.client.as_ref().is_some_and(|c| Arc::ptr_eq(c, &client));
                                let up_to_date = p.topic != "resync"
                                    && !p.latest_revision.is_null()
                                    && slot.last_state_marker["revision"] == p.latest_revision;
                                (active, up_to_date)
                            };
                            if !still_active {
                                break;
                            }
                            if already_fetched {
                                continue;
                            }
                            if !refresh_remote_host_async(
                                &weak,
                                &host_id,
                                &client,
                                Some(p.topic),
                                p.card_id,
                                &mut host_stop_rx,
                                &mut global_stop_rx,
                            )
                            .await
                            {
                                break;
                            }
                        }
                        frame_res = stream.next_frame() => {
                            match frame_res {
                                Ok(Some(client::PushFrame::Heartbeat)) => {
                                    if has_tunnel {
                                        let Some(reg) = weak.upgrade() else {
                                            return;
                                        };
                                        let dead = {
                                            let mut guard = reg.remotes.lock().unwrap();
                                            guard
                                                .get_mut(&host_id)
                                                .and_then(|s| s.tunnel.as_mut())
                                                .is_some_and(|t| !t.is_alive())
                                        };
                                        if dead {
                                            reg.handle_remote_connected_error(
                                                &host_id,
                                                &client,
                                                &anyhow::anyhow!("SSH tunnel exited"),
                                            );
                                            break;
                                        }
                                    }
                                }
                                Ok(Some(client::PushFrame::StateChanged {
                                    topic,
                                    card_id,
                                    marker,
                                    ..
                                })) => {
                                    let rev = marker["revision"].clone();
                                    let already_fetched = weak.upgrade().is_some_and(|reg| {
                                        let guard = reg.remotes.lock().unwrap();
                                        guard.get(&host_id).is_some_and(|s| {
                                            topic != "resync"
                                                && !rev.is_null()
                                                && s.last_state_marker["revision"] == rev
                                        })
                                    });
                                    if already_fetched {
                                        continue;
                                    }
                                    let now_i = tokio::time::Instant::now();
                                    match &mut pending {
                                        Some(p) => {
                                            p.topic = merge_push_topic(p.topic, topic);
                                            if p.card_id != card_id {
                                                p.card_id = None;
                                            }
                                            p.latest_revision = rev;
                                            p.deadline =
                                                (now_i + REMOTE_PUSH_DEBOUNCE).min(p.max_deadline);
                                        }
                                        None => {
                                            pending = Some(PendingRefresh {
                                                topic,
                                                card_id,
                                                latest_revision: rev,
                                                deadline: now_i + REMOTE_PUSH_DEBOUNCE,
                                                max_deadline: now_i + REMOTE_PUSH_MAX_DEBOUNCE,
                                            });
                                        }
                                    }
                                }
                                Ok(Some(client::PushFrame::UpdateChanged(state))) => {
                                    if let Some(reg) = weak.upgrade() {
                                        reg.emit_push(HostPushEvent::UpdateChanged {
                                            host_id: host_id.clone(),
                                            state,
                                        });
                                    }
                                }
                                Ok(Some(client::PushFrame::UtilitiesChanged(state))) => {
                                    if let Some(reg) = weak.upgrade() {
                                        reg.emit_push(HostPushEvent::UtilitiesChanged {
                                            host_id: host_id.clone(),
                                            state,
                                        });
                                    }
                                }
                                Ok(Some(client::PushFrame::UsageChanged(state))) => {
                                    if let Some(reg) = weak.upgrade() {
                                        reg.emit_push(HostPushEvent::UsageChanged {
                                            host_id: host_id.clone(),
                                            state,
                                        });
                                    }
                                }
                                Ok(Some(client::PushFrame::HerdrInstall(state))) => {
                                    if let Some(reg) = weak.upgrade() {
                                        reg.emit_push(HostPushEvent::HerdrInstall {
                                            host_id: host_id.clone(),
                                            state,
                                        });
                                    }
                                }
                                Ok(None) => {
                                    if let Some(reg) = weak.upgrade() {
                                        reg.handle_remote_connected_error(
                                            &host_id,
                                            &client,
                                            &anyhow::anyhow!("Remote push stream closed"),
                                        );
                                    }
                                    break;
                                }
                                Err(err) => {
                                    if let Some(reg) = weak.upgrade() {
                                        reg.handle_remote_connected_error(&host_id, &client, &err);
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
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
                    allow_insecure_bind: false,
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
        // Legacy server.token in hosts.json was auto-upgraded to a revocable dev_... token.
        let persisted: HostsFile =
            serde_json::from_slice(&std::fs::read(HostRegistry::hosts_path(&local_home)).unwrap())
                .unwrap();
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
