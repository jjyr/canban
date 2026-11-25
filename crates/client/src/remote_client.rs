//! Multi-host client (`MultiHostClient`, aliased as `RemoteClient`) driven by pluggable
//! [`HostConnector`] adapters, Tokio async push streams, `canban-sync` outbox/read-cache replay,
//! AEAD TCP secret sealing, and remote PTY transport.
use crate::{
    capabilities::ClientCapabilities,
    connector::{ConnectionLease, HostConnector, TcpConnector},
    pairing::{PairingInvite, TransportSecurity},
    push::{PushFrame, change_topic, open_async_subscribe, state_marker},
    session::SingleHostSession,
    store::{
        CachedHostData, CredentialStore, load_cache, load_hosts, outbox_path, reads_dir,
        remove_host_files, save_cache, save_hosts, seal_params_for_tcp,
    },
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use canban_sync::{
    Outbox, QueueDecision, ReadCache,
    outbox::{Mutation, MutationKind, MutationState, UpdateBase},
    replay::{DrainReport, FinishOutcome, StepAction, finish_step, prepare_next_step},
};
use canban_terminal::{RemotePtyTransport, session::PtyTransport};
use canban_transport::ClientTransport;
use canban_types::{
    AppEvent, Config, DiscoveredCanbanHost, GlobalEnvSyncSummary, HostConnectionState, HostSummary,
    HostTransport, RuntimeState, SavedHost, Snapshot, SyncAction, SyncItemState, SyncItemView,
    TerminalEntry, is_device_token, is_pairing_code, normalize_daemon_addr, now,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, RwLock, Weak},
    time::{Duration, Instant},
};
use tokio::sync::{Notify, broadcast, mpsc, watch};

const DEFAULT_READ_CACHE_BUDGET: u64 = 20 * 1024 * 1024;
const PUSH_DEBOUNCE: Duration = Duration::from_millis(30);
const PUSH_MAX_DEBOUNCE: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(10);
const LEGACY_POLL_INTERVAL: Duration = Duration::from_secs(5);
const SUBSCRIBE_TOPICS: &[&str] = &["snapshot", "update", "utilities", "usage", "herdr"];

fn is_unsupported_subscribe_error(err: &anyhow::Error) -> bool {
    let msg = format!("{err:#}");
    msg.contains("Unknown operation") && msg.contains("app.subscribe")
}

fn count_active_agents_in(terminals: &[TerminalEntry]) -> usize {
    terminals
        .iter()
        .filter(|t| {
            t.agent
                .as_ref()
                .is_some_and(|a| matches!(a.state.as_str(), "starting" | "running" | "working"))
        })
        .count()
}

fn count_active_agents(s: &Snapshot) -> usize {
    count_active_agents_in(&s.terminals)
}

fn is_runtime_topic(topic: &str) -> bool {
    matches!(topic, "terminals" | "reclaim")
}

fn merge_push_topic(current: &'static str, incoming: &'static str) -> &'static str {
    if current == "resync" || incoming == "resync" {
        "resync"
    } else if current == "github" || incoming == "github" {
        "github"
    } else if !is_runtime_topic(current) && is_runtime_topic(incoming) {
        current
    } else {
        incoming
    }
}

/// Configuration for [`RemoteClient`].
#[derive(Clone, Debug)]
pub struct RemoteClientConfig {
    pub data_dir: PathBuf,
    pub device_name: String,
    pub read_cache_budget_bytes: u64,
}

impl RemoteClientConfig {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            device_name: "Canban Mobile".to_owned(),
            read_cache_budget_bytes: DEFAULT_READ_CACHE_BUDGET,
        }
    }
}

/// Real-time events emitted by [`RemoteClient`] across all connected hosts.
#[derive(Clone, Debug)]
pub enum ClientEvent {
    Host { host_id: String, event: AppEvent },
    UpdateChanged { host_id: String, state: Value },
    UtilitiesChanged { host_id: String, state: Value },
    UsageChanged { host_id: String, state: Value },
    HerdrInstall { host_id: String, state: Value },
}

struct RemoteHostSlot {
    saved: SavedHost,
    client: Option<Arc<ClientTransport>>,
    lease: ConnectionLease,
    terminal: Option<Arc<RemotePtyTransport>>,
    connection: HostConnectionState,
    cached: CachedHostData,
    home: String,
    notice: Option<String>,
    reconnect_count: u32,
    last_synced_at: Option<u64>,
    last_marker: Value,
    conn_gen: u64,
    supervisor_epoch: u64,
    supervisor_notify: Arc<Notify>,
    draining: Arc<tokio::sync::Mutex<()>>,
    outbox: Arc<Mutex<Outbox>>,
    reads: Arc<Mutex<ReadCache>>,
}

impl RemoteHostSlot {
    fn from_saved(saved: SavedHost, data_dir: &Path, read_cache_budget: u64) -> Self {
        let cached = load_cache(data_dir, &saved.id).unwrap_or(CachedHostData {
            saved_at: 0,
            snapshot: Snapshot::default(),
            config: Config::default(),
        });
        let cached_at = (cached.saved_at > 0).then_some(cached.saved_at);
        let initial_conn = if saved.enabled {
            HostConnectionState::Connecting
        } else {
            HostConnectionState::Offline {
                error: "Disabled".to_owned(),
                cached_at,
            }
        };
        let outbox = Outbox::load(outbox_path(data_dir, &saved.id));
        let reads = ReadCache::open(reads_dir(data_dir, &saved.id), read_cache_budget);
        Self {
            saved,
            client: None,
            lease: ConnectionLease::none(),
            terminal: None,
            connection: initial_conn,
            cached,
            home: String::new(),
            notice: None,
            reconnect_count: 0,
            last_synced_at: None,
            last_marker: Value::Null,
            conn_gen: 0,
            supervisor_epoch: 0,
            supervisor_notify: Arc::new(Notify::new()),
            draining: Arc::new(tokio::sync::Mutex::new(())),
            outbox: Arc::new(Mutex::new(outbox)),
            reads: Arc::new(Mutex::new(reads)),
        }
    }

    fn summary(&self) -> HostSummary {
        let outbox = self.outbox.lock().unwrap();
        let active_agents = count_active_agents(&self.cached.snapshot);
        HostSummary {
            id: self.saved.id.clone(),
            name: self.saved.name.clone(),
            is_local: matches!(self.saved.transport, HostTransport::Local)
                || self.saved.id == "local",
            transport_label: self.saved.transport.label().to_owned(),
            endpoint_label: self.saved.transport.endpoint_display(),
            ssh_target: self.saved.ssh_target.clone(),
            server_id: self.saved.server_id.clone(),
            connection: self.connection.clone(),
            active_agents,
            max_agents: self.cached.config.scheduler.max_agents,
            project_count: self.cached.snapshot.projects.len(),
            reconnect_count: self.reconnect_count,
            last_synced_at: self.last_synced_at,
            pending_mutations: outbox.pending_count(),
            sync_conflicts: outbox.conflict_count(),
        }
    }

    fn tcp_token(&self) -> Option<String> {
        if let Some(active) = self.client.as_ref().and_then(|c| c.active_token()) {
            return Some(active.to_owned());
        }
        match &self.saved.transport {
            HostTransport::Tcp { token, .. } if !token.trim().is_empty() => Some(token.clone()),
            _ => None,
        }
    }

    fn cached_at(&self) -> Option<u64> {
        (self.cached.saved_at > 0).then_some(self.cached.saved_at)
    }

    fn clear_connection(&mut self, next_state: HostConnectionState) {
        if let Some(client) = self.client.take() {
            client.close();
            self.reconnect_count = self.reconnect_count.saturating_add(1);
        }
        self.lease = ConnectionLease::none();
        self.terminal = None;
        self.connection = next_state;
        self.conn_gen = self.conn_gen.wrapping_add(1);
    }
}

/// Shared multi-host Canban client managing pluggable transport connections, Tokio push streams,
/// offline read cache, and FIFO mutation replay across saved hosts.
pub struct MultiHostClient {
    config: RemoteClientConfig,
    creds: Arc<dyn CredentialStore>,
    connector: Arc<dyn HostConnector>,
    capabilities: ClientCapabilities,
    hosts: RwLock<BTreeMap<String, RemoteHostSlot>>,
    events: broadcast::Sender<ClientEvent>,
    shutdown_tx: watch::Sender<bool>,
    runtime_handle: tokio::runtime::Handle,
    owned_runtime: Option<tokio::runtime::Runtime>,
    self_weak: OnceLock<Weak<Self>>,
}

/// Backward-compatible alias for pure-remote callers (`canban-ios`, `canban-android`).
pub type RemoteClient = MultiHostClient;

impl Drop for MultiHostClient {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        let (clients, leases): (Vec<Arc<ClientTransport>>, Vec<ConnectionLease>) = {
            let mut guard = self.hosts.write().unwrap();
            let mut out = Vec::new();
            let mut leases = Vec::new();
            for slot in guard.values_mut() {
                slot.terminal = None;
                leases.push(std::mem::replace(&mut slot.lease, ConnectionLease::none()));
                if let Some(c) = slot.client.take() {
                    out.push(c);
                }
            }
            (out, leases)
        };
        for client in clients {
            client.close();
        }
        drop(leases);
        if let Some(rt) = self.owned_runtime.take() {
            if tokio::runtime::Handle::try_current().is_ok() {
                std::thread::spawn(move || drop(rt));
            } else {
                drop(rt);
            }
        }
    }
}

fn is_connection_error(err: &anyhow::Error) -> bool {
    let msg = format!("{err:#}");
    let lower = msg.to_ascii_lowercase();
    lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("broken pipe")
        || lower.contains("unexpected eof")
        || lower.contains("timed out")
        || lower.contains("not connected")
        || lower.contains("closed")
        || lower.contains("failed to connect")
        || lower.contains("network is unreachable")
        || lower.contains("host is unreachable")
}

fn is_auth_error(err: &anyhow::Error) -> bool {
    let msg = format!("{err:#}").to_ascii_lowercase();
    msg.contains("auth_failed")
        || msg.contains("authentication failed")
        || msg.contains("unauthorized")
        || msg.contains("revoked")
        || msg.contains("invalid token")
        || msg.contains("invalid pairing")
        || msg.contains("-32001")
}

fn block_on_rt<F, T>(handle: &tokio::runtime::Handle, fut: F) -> T
where
    F: std::future::Future<Output = T> + Send,
    T: Send,
{
    if tokio::runtime::Handle::try_current().is_err() {
        return handle.block_on(fut);
    }
    std::thread::scope(|scope| {
        let h = handle.clone();
        scope
            .spawn(move || h.block_on(fut))
            .join()
            .expect("blocking Tokio bridge panicked")
    })
}

fn entity_id_candidates<'a>(method: &str, params: &'a Value) -> Vec<&'a str> {
    let keys: &[&str] = match method {
        m if m.starts_with("terminal.") => &["terminal_id", "card_id", "project_id"],
        m if m.starts_with("card.") || m.starts_with("timeline.") || m.starts_with("comment.") => {
            &["card_id", "project_id", "id"]
        }
        m if m.starts_with("project.") => &["project_id", "id"],
        m if m.starts_with("workflow.") => &["run_id", "source_id", "project_id", "card_id"],
        m if m.starts_with("schedule.") => &["schedule_id", "project_id"],
        m if m.starts_with("configuration.") => &["session_id", "project_id"],
        m if m.starts_with("merge_batch.") => &["batch_id", "project_id"],
        _ => &[
            "card_id",
            "terminal_id",
            "project_id",
            "run_id",
            "source_id",
            "schedule_id",
            "session_id",
            "batch_id",
            "id",
        ],
    };
    keys.iter()
        .filter_map(|k| {
            params
                .get(*k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .collect()
}

fn snapshot_contains_entity(snap: &Snapshot, id: &str) -> bool {
    snap.entity_hosts.contains_key(id)
        || snap.projects.iter().any(|p| p.id == id)
        || snap.cards.iter().any(|c| c.id == id)
        || snap.terminals.iter().any(|t| t.id == id)
        || snap.workflow_runs.iter().any(|r| r.id == id)
        || snap.workflow_sources.iter().any(|s| s.id == id)
        || snap.schedule_states.iter().any(|s| s.id == id)
        || snap.configuration_sessions.iter().any(|s| s.id == id)
        || snap.merge_batches.iter().any(|b| b.id == id)
}

fn extract_card_id_for_mutation(params: &Value) -> String {
    params["card_id"]
        .as_str()
        .or_else(|| params["id"].as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(canban_types::id)
}

fn build_update_base(params: &Value, snap: &Snapshot, card_id: &str) -> UpdateBase {
    let card = snap.cards.iter().find(|c| c.id == card_id);
    let title = if params.get("title").and_then(Value::as_str).is_some() {
        card.map(|c| c.title.clone())
    } else {
        None
    };
    let body = if params.get("body").and_then(Value::as_str).is_some() {
        card.map(|c| c.body.clone())
    } else {
        None
    };
    UpdateBase { title, body }
}

fn parse_refreshed_snapshot(state_val: &Value, host_id: &str) -> Option<Snapshot> {
    let snap_val = state_val.get("snapshot")?;
    let mut snap = serde_json::from_value::<Snapshot>(snap_val.clone()).ok()?;
    snap.register_host_entities(host_id);
    Some(snap)
}

fn apply_state_to_slot(
    slot: &mut RemoteHostSlot,
    data_dir: &Path,
    state_val: &Value,
    rtt_ms: Option<u64>,
    probe_val: Option<&Value>,
    preferred_topic: Option<&'static str>,
) -> Option<&'static str> {
    let changed = state_val["changed"] == true || state_val.get("snapshot").is_some();
    let prev_marker = slot.last_marker.clone();
    let next_marker = state_marker(state_val);
    let was_connected = matches!(slot.connection, HostConnectionState::Connected { .. });

    if let Some(sid) = state_val["server_id"]
        .as_str()
        .or_else(|| probe_val.and_then(|p| p["server_id"].as_str()))
        && !sid.is_empty()
    {
        slot.saved.server_id = Some(sid.to_owned());
    }
    if let Some(home) = state_val["home"].as_str() {
        slot.home = home.to_owned();
    }
    if let Some(notice) = state_val["notice"].as_str() {
        slot.notice = (!notice.trim().is_empty()).then(|| notice.to_owned());
    } else if state_val.get("notice").is_some_and(Value::is_null) {
        slot.notice = None;
    }

    if changed {
        if let Some(snap) = parse_refreshed_snapshot(state_val, &slot.saved.id) {
            slot.cached.snapshot = snap;
        }
        if let Ok(cfg) = serde_json::from_value::<Config>(state_val["config"].clone()) {
            slot.cached.config = cfg;
        }
        slot.cached.saved_at = now();
        save_cache(data_dir, &slot.saved.id, &slot.cached);
    }

    if let Some(probe) = probe_val {
        let api_version = probe["api_version"].as_u64().unwrap_or(0) as u32;
        let version = probe["build"]["version"]
            .as_str()
            .or_else(|| probe["version"].as_str())
            .unwrap_or("unknown")
            .to_owned();
        let platform = probe["build"]["platform"]
            .as_str()
            .or_else(|| probe["platform"].as_str())
            .unwrap_or("unknown")
            .to_owned();
        slot.connection = HostConnectionState::Connected {
            rtt_ms: rtt_ms.unwrap_or(0),
            api_version,
            version,
            platform,
        };
    } else if let HostConnectionState::Connected {
        rtt_ms: ref mut prev_rtt,
        ..
    } = slot.connection
        && let Some(rtt) = rtt_ms
    {
        *prev_rtt = rtt;
    }

    slot.last_synced_at = Some(now());
    if !next_marker["revision"].is_null() {
        slot.last_marker = next_marker.clone();
    }

    if !was_connected || preferred_topic == Some("resync") {
        Some("resync")
    } else if changed {
        Some(
            if prev_marker["github_revision"] != slot.last_marker["github_revision"] {
                "github"
            } else if let Some(topic) = preferred_topic {
                topic
            } else {
                change_topic(true, &prev_marker, &slot.last_marker)
            },
        )
    } else {
        None
    }
}

fn cache_and_augment_timeline(
    mut response: Value,
    method: &str,
    outbox: &Outbox,
    snapshot: &Snapshot,
    card_id: &str,
) -> Value {
    if method == "timeline.list" {
        let pending = canban_sync::pending_comment_events(&outbox.ops, card_id, snapshot);
        if !pending.is_empty()
            && let Some(arr) = response.get_mut("events").and_then(Value::as_array_mut)
        {
            for ev in pending {
                let msg = ev.message.clone();
                if let Ok(mut v) = serde_json::to_value(ev) {
                    if let Some(obj) = v.as_object_mut() {
                        obj.entry("body".to_owned()).or_insert_with(|| json!(msg));
                        let data = obj
                            .entry("data")
                            .or_insert_with(|| Value::Object(serde_json::Map::new()));
                        if let Some(data_obj) = data.as_object_mut() {
                            data_obj.insert("pending".into(), json!(true));
                        }
                    }
                    arr.push(v);
                }
            }
        }
    }
    response
}

impl MultiHostClient {
    /// Creates a new [`MultiHostClient`] owning a multi-thread Tokio runtime using the built-in
    /// [`TcpConnector`] and [`ClientCapabilities::mobile()`].
    pub fn new(config: RemoteClientConfig, creds: Arc<dyn CredentialStore>) -> Result<Arc<Self>> {
        Self::with_connector(
            config,
            creds,
            Arc::new(TcpConnector),
            ClientCapabilities::mobile(),
        )
    }

    /// Creates a new [`MultiHostClient`] with custom transport connector(s) and runtime capabilities.
    pub fn with_connector(
        config: RemoteClientConfig,
        creds: Arc<dyn CredentialStore>,
        connector: Arc<dyn HostConnector>,
        capabilities: ClientCapabilities,
    ) -> Result<Arc<Self>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("canban-client")
            .build()
            .context("Failed to build MultiHostClient Tokio runtime")?;
        let handle = runtime.handle().clone();
        Self::build(
            config,
            creds,
            connector,
            capabilities,
            handle,
            Some(runtime),
        )
    }

    /// Creates a new [`MultiHostClient`] bound to an existing caller Tokio runtime handle.
    pub fn with_runtime(
        config: RemoteClientConfig,
        creds: Arc<dyn CredentialStore>,
        runtime_handle: tokio::runtime::Handle,
    ) -> Result<Arc<Self>> {
        Self::with_runtime_and_connector(
            config,
            creds,
            Arc::new(TcpConnector),
            ClientCapabilities::mobile(),
            runtime_handle,
        )
    }

    /// Creates a new [`MultiHostClient`] bound to an existing runtime with custom connector(s) and capabilities.
    pub fn with_runtime_and_connector(
        config: RemoteClientConfig,
        creds: Arc<dyn CredentialStore>,
        connector: Arc<dyn HostConnector>,
        capabilities: ClientCapabilities,
        runtime_handle: tokio::runtime::Handle,
    ) -> Result<Arc<Self>> {
        Self::build(config, creds, connector, capabilities, runtime_handle, None)
    }

    fn build(
        config: RemoteClientConfig,
        creds: Arc<dyn CredentialStore>,
        connector: Arc<dyn HostConnector>,
        capabilities: ClientCapabilities,
        runtime_handle: tokio::runtime::Handle,
        owned_runtime: Option<tokio::runtime::Runtime>,
    ) -> Result<Arc<Self>> {
        let saved = load_hosts(&config.data_dir, creds.as_ref());
        let mut slots = BTreeMap::new();
        for host in saved {
            if !connector.supports(&host.transport) {
                continue;
            }
            slots.insert(
                host.id.clone(),
                RemoteHostSlot::from_saved(host, &config.data_dir, config.read_cache_budget_bytes),
            );
        }

        let (events, _) = broadcast::channel(512);
        let (shutdown_tx, _) = watch::channel(false);
        let client = Arc::new(Self {
            config,
            creds,
            connector,
            capabilities,
            hosts: RwLock::new(slots),
            events,
            shutdown_tx,
            runtime_handle,
            owned_runtime,
            self_weak: OnceLock::new(),
        });
        let _ = client.self_weak.set(Arc::downgrade(&client));

        let enabled_ids: Vec<String> = {
            let guard = client.hosts.read().unwrap();
            guard
                .values()
                .filter(|s| s.saved.enabled)
                .map(|s| s.saved.id.clone())
                .collect()
        };
        for host_id in enabled_ids {
            client.spawn_supervisor(&host_id);
        }

        Ok(client)
    }

    /// Runtime capabilities advertised by this client instance.
    pub fn capabilities(&self) -> ClientCapabilities {
        self.capabilities
    }

    /// Returns `true` when this client's configured [`HostConnector`] supports `transport`.
    pub fn supports_transport(&self, transport: &HostTransport) -> bool {
        self.connector.supports(transport)
    }

    fn upgrade_self(&self) -> Option<Arc<Self>> {
        self.self_weak.get().and_then(Weak::upgrade)
    }

    fn emit(&self, event: ClientEvent) {
        let _ = self.events.send(event);
    }

    fn emit_host(&self, host_id: &str, event: AppEvent) {
        self.emit(ClientEvent::Host {
            host_id: host_id.to_owned(),
            event,
        });
    }

    fn persist_locked(&self, hosts: &BTreeMap<String, RemoteHostSlot>) -> Result<()> {
        let saved: Vec<SavedHost> = hosts.values().map(|s| s.saved.clone()).collect();
        save_hosts(&self.config.data_dir, &saved, self.creds.as_ref())?;
        Ok(())
    }

    fn spawn_supervisor(&self, host_id: &str) {
        let Some(this) = self.upgrade_self() else {
            return;
        };
        let (epoch, old_notify, notify) = {
            let mut guard = self.hosts.write().unwrap();
            let Some(slot) = guard.get_mut(host_id) else {
                return;
            };
            slot.supervisor_epoch = slot.supervisor_epoch.wrapping_add(1);
            let next_notify = Arc::new(Notify::new());
            let prev_notify =
                std::mem::replace(&mut slot.supervisor_notify, Arc::clone(&next_notify));
            (slot.supervisor_epoch, prev_notify, next_notify)
        };
        old_notify.notify_waiters();
        let hid = host_id.to_owned();
        let shutdown_rx = self.shutdown_tx.subscribe();
        self.runtime_handle.spawn(async move {
            host_supervisor_task(Arc::downgrade(&this), hid, epoch, notify, shutdown_rx).await;
        });
    }

    /// Executes a future on this client's managed multi-thread Tokio runtime.
    pub fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime_handle.block_on(future)
    }

    /// Subscribes to [`ClientEvent`] notifications on an unbounded mpsc channel.
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<ClientEvent> {
        self.subscribe_mapped(|ev| ev)
    }

    /// Subscribes and transforms each [`ClientEvent`] inside `canban-client`'s own Tokio runtime.
    pub fn subscribe_mapped<T, F>(&self, map: F) -> mpsc::UnboundedReceiver<T>
    where
        T: Send + 'static,
        F: Fn(ClientEvent) -> T + Send + Sync + 'static,
    {
        let mut rx = self.events.subscribe();
        let (tx, out_rx) = mpsc::unbounded_channel();
        self.runtime_handle.spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        if tx.send(map(ev)).is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        out_rx
    }

    /// Notifies supervisors when the host OS brings the client app back to the foreground.
    pub fn on_foreground(&self) {
        self.reconnect(None);
    }

    /// Wakes reconnect/resync loops for `host_id` (or all enabled hosts when `None`).
    pub fn reconnect(&self, host_id: Option<&str>) {
        let notifies: Vec<Arc<Notify>> = {
            let mut guard = self.hosts.write().unwrap();
            let mut list = Vec::new();
            for (id, slot) in guard.iter_mut() {
                if host_id.is_some_and(|target| target != id) {
                    continue;
                }
                if !slot.saved.enabled {
                    continue;
                }
                if slot.client.is_none() {
                    slot.connection = HostConnectionState::Connecting;
                }
                list.push(Arc::clone(&slot.supervisor_notify));
            }
            list
        };
        for notify in notifies {
            notify.notify_one();
        }
    }

    /// Blocks until every enabled host completes one connect/refresh + outbox drain pass.
    pub fn poll_remotes_once(&self) {
        block_on_rt(&self.runtime_handle, self.poll_remotes_once_async());
    }

    /// Runs one immediate connect/refresh + outbox drain pass across all enabled hosts.
    pub async fn poll_remotes_once_async(&self) {
        let Some(this) = self.upgrade_self() else {
            return;
        };
        let host_ids: Vec<String> = {
            let guard = self.hosts.read().unwrap();
            guard
                .values()
                .filter(|s| s.saved.enabled)
                .map(|s| s.saved.id.clone())
                .collect()
        };
        let mut set = tokio::task::JoinSet::new();
        for host_id in host_ids {
            let c = Arc::clone(&this);
            set.spawn(async move {
                let _ = c.sync_one_host_once(&host_id).await;
            });
        }
        while set.join_next().await.is_some() {}
    }

    /// Pairs with the exact single address encoded in a QR/deep-link [`PairingInvite`].
    pub async fn pair_with_invite(
        &self,
        invite: &PairingInvite,
        display_name: Option<&str>,
    ) -> Result<HostSummary> {
        self.pair_internal(None, invite, display_name).await
    }

    /// Pairs directly with a same-account Tailscale Canban daemon without a pairing URI/code.
    pub async fn pair_tailscale_host(
        &self,
        addr: &str,
        display_name: Option<&str>,
        server_id: Option<&str>,
    ) -> Result<HostSummary> {
        let invite = PairingInvite::tailscale(addr, display_name, server_id).ok_or_else(|| {
            anyhow!(
                "Address `{addr}` is not a Tailscale endpoint; pair with a canban:// URI instead"
            )
        })?;
        self.pair_with_invite(&invite, display_name).await
    }

    /// Queries an already-connected host (`via_host_id` or the first connected host) for
    /// reachable Canban daemons on its saved host list and Tailnet, excluding already-paired hosts.
    pub async fn discover_canban_hosts(
        &self,
        via_host_id: Option<&str>,
    ) -> Result<Vec<DiscoveredCanbanHost>> {
        let via_client = {
            let guard = self.hosts.read().unwrap();
            if let Some(hid) = via_host_id {
                guard.get(hid).and_then(|s| s.client.clone())
            } else {
                guard
                    .values()
                    .find(|s| s.connection.is_connected() && s.client.is_some())
                    .or_else(|| guard.values().find(|s| s.client.is_some()))
                    .and_then(|s| s.client.clone())
            }
        }
        .context("Connect to at least one Canban host before discovering Tailnet machines")?;

        let raw = match via_client
            .call_async("host.discover_canban", &json!({}))
            .await
        {
            Ok(val) => val,
            Err(err) => {
                let msg = format!("{err:#}");
                if msg.contains("Unknown operation") && msg.contains("host.discover_canban") {
                    return Ok(Vec::new());
                }
                return Err(err);
            }
        };

        let items: Vec<DiscoveredCanbanHost> =
            serde_json::from_value(raw["hosts"].clone()).unwrap_or_default();
        let (known_sids, known_addrs) = {
            let guard = self.hosts.read().unwrap();
            let mut sids = BTreeSet::new();
            let mut addrs = BTreeSet::new();
            for slot in guard.values() {
                if let Some(sid) = slot.saved.server_id.as_deref() {
                    sids.insert(sid.to_owned());
                }
                if let HostTransport::Tcp { addr, .. } = &slot.saved.transport {
                    addrs.insert(normalize_daemon_addr(addr));
                }
            }
            (sids, addrs)
        };

        Ok(items
            .into_iter()
            .filter(|h| {
                let norm = normalize_daemon_addr(&h.addr);
                !known_addrs.contains(&norm)
                    && !h
                        .server_id
                        .as_deref()
                        .is_some_and(|sid| known_sids.contains(sid))
            })
            .collect())
    }

    async fn pair_internal(
        &self,
        preferred_id: Option<String>,
        invite: &PairingInvite,
        display_name: Option<&str>,
    ) -> Result<HostSummary> {
        let addr = normalize_daemon_addr(&invite.addr);
        ensure!(
            !addr.is_empty(),
            "Pairing invite does not contain an address"
        );

        let tcp_transport = HostTransport::Tcp {
            addr: addr.clone(),
            token: invite.secret.clone(),
        };
        let session = SingleHostSession::connect_async(
            self.connector.as_ref(),
            preferred_id.as_deref().unwrap_or("pairing"),
            &tcp_transport,
            &self.config.device_name,
            Value::Null,
        )
        .await?;
        let token = session
            .upgraded_token()
            .map(str::to_owned)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| invite.secret.clone());
        let (transport, lease, hs) = session.into_parts();
        let rtt_ms = hs.rtt_ms;
        let ping_val = hs.ping;
        let state_val = hs.state;

        let server_id = hs
            .server_id
            .or_else(|| invite.server_id.clone())
            .filter(|s| !s.trim().is_empty());

        let host_name = display_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| invite.host_name.as_deref().filter(|s| !s.trim().is_empty()))
            .or_else(|| hs.hostname.as_deref().filter(|s| !s.trim().is_empty()))
            .unwrap_or("Remote Host")
            .to_owned();

        let (host_id, summary, need_supervisor, notify) = {
            let mut guard = self.hosts.write().unwrap();
            let existing_id = server_id
                .as_ref()
                .and_then(|sid| {
                    guard
                        .values()
                        .find(|s| s.saved.server_id.as_deref() == Some(sid.as_str()))
                        .map(|s| s.saved.id.clone())
                })
                .or_else(|| {
                    preferred_id
                        .as_deref()
                        .filter(|id| guard.contains_key(*id))
                        .map(str::to_owned)
                });

            if let Some(hid) = existing_id {
                let slot = guard.get_mut(&hid).expect("existing host slot");
                let was_enabled = slot.saved.enabled;
                slot.clear_connection(HostConnectionState::Connecting);
                slot.saved.enabled = true;
                slot.saved.transport = HostTransport::Tcp { addr, token };
                if let Some(name) = display_name.map(str::trim).filter(|s| !s.is_empty()) {
                    slot.saved.name = name.to_owned();
                }
                slot.terminal = Some(Arc::new(RemotePtyTransport::new(
                    Arc::clone(&transport),
                    self.runtime_handle.clone(),
                )));
                slot.lease = lease;
                slot.client = Some(Arc::clone(&transport));
                apply_state_to_slot(
                    slot,
                    &self.config.data_dir,
                    &state_val,
                    Some(rtt_ms),
                    Some(&ping_val),
                    None,
                );
                let summary = slot.summary();
                let notify = was_enabled.then(|| Arc::clone(&slot.supervisor_notify));
                self.persist_locked(&guard)?;
                (hid, summary, !was_enabled, notify)
            } else {
                let hid = preferred_id
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(canban_types::id);
                let saved = SavedHost {
                    id: hid.clone(),
                    name: host_name,
                    transport: HostTransport::Tcp { addr, token },
                    ssh_target: None,
                    server_id,
                    enabled: true,
                    added_at: now(),
                };
                let mut slot = RemoteHostSlot::from_saved(
                    saved,
                    &self.config.data_dir,
                    self.config.read_cache_budget_bytes,
                );
                slot.terminal = Some(Arc::new(RemotePtyTransport::new(
                    Arc::clone(&transport),
                    self.runtime_handle.clone(),
                )));
                slot.lease = lease;
                slot.client = Some(Arc::clone(&transport));
                apply_state_to_slot(
                    &mut slot,
                    &self.config.data_dir,
                    &state_val,
                    Some(rtt_ms),
                    Some(&ping_val),
                    None,
                );
                let summary = slot.summary();
                guard.insert(hid.clone(), slot);
                self.persist_locked(&guard)?;
                (hid, summary, true, None)
            }
        };

        if need_supervisor {
            self.spawn_supervisor(&host_id);
        } else if let Some(n) = notify {
            n.notify_one();
        }
        self.emit_host(
            &host_id,
            AppEvent::Changed {
                topic: "resync",
                card_id: None,
            },
        );
        Ok(summary)
    }

    /// Adds or updates a host synchronously and connects immediately when `host.enabled`.
    pub fn add_host(&self, host: SavedHost) -> Result<HostSummary> {
        block_on_rt(&self.runtime_handle, self.add_host_async(host))
    }

    /// Adds or updates a host asynchronously and connects immediately when `host.enabled`.
    pub async fn add_host_async(&self, mut host: SavedHost) -> Result<HostSummary> {
        ensure!(
            self.connector.supports(&host.transport),
            "Transport {} is not supported by this client",
            host.transport.label()
        );
        if let HostTransport::Tcp { addr, token } = &mut host.transport {
            *addr = normalize_daemon_addr(addr);
            ensure!(!addr.is_empty(), "Host address cannot be empty");
            ensure!(
                !token.trim().is_empty(),
                "TCP host requires a device token or pairing code"
            );
        }
        if host.id.trim().is_empty() {
            host.id = canban_types::id();
        }
        if host.added_at == 0 {
            host.added_at = now();
        }

        if host.enabled
            && let HostTransport::Tcp { addr, token } = &host.transport
        {
            let invite = PairingInvite {
                addr: addr.clone(),
                security: TransportSecurity::classify_tcp_addr(addr),
                server_id: host.server_id.clone(),
                host_name: (!host.name.trim().is_empty()).then(|| host.name.clone()),
                secret: token.clone(),
            };
            let display = (!host.name.trim().is_empty()).then_some(host.name.as_str());
            return self.pair_internal(Some(host.id), &invite, display).await;
        }

        if host.enabled {
            let session = SingleHostSession::connect_async(
                self.connector.as_ref(),
                &host.id,
                &host.transport,
                &self.config.device_name,
                Value::Null,
            )
            .await?;
            let (transport, lease, hs) = session.into_parts();
            if host.server_id.is_none() {
                host.server_id = hs.server_id;
            }
            if host.name.trim().is_empty() {
                host.name = hs.hostname.unwrap_or_else(|| host.id.clone());
            }
            let hid = host.id.clone();
            let summary = {
                let mut guard = self.hosts.write().unwrap();
                let mut slot = RemoteHostSlot::from_saved(
                    host,
                    &self.config.data_dir,
                    self.config.read_cache_budget_bytes,
                );
                slot.terminal = Some(Arc::new(RemotePtyTransport::new(
                    Arc::clone(&transport),
                    self.runtime_handle.clone(),
                )));
                slot.lease = lease;
                slot.client = Some(Arc::clone(&transport));
                apply_state_to_slot(
                    &mut slot,
                    &self.config.data_dir,
                    &hs.state,
                    Some(hs.rtt_ms),
                    Some(&hs.ping),
                    None,
                );
                let sum = slot.summary();
                guard.insert(hid.clone(), slot);
                self.persist_locked(&guard)?;
                sum
            };
            self.spawn_supervisor(&hid);
            self.emit_host(
                &hid,
                AppEvent::Changed {
                    topic: "resync",
                    card_id: None,
                },
            );
            return Ok(summary);
        }

        let hid = host.id.clone();
        let summary = {
            let mut guard = self.hosts.write().unwrap();
            let slot = RemoteHostSlot::from_saved(
                host,
                &self.config.data_dir,
                self.config.read_cache_budget_bytes,
            );
            let sum = slot.summary();
            guard.insert(hid.clone(), slot);
            self.persist_locked(&guard)?;
            sum
        };
        self.emit_host(
            &hid,
            AppEvent::Changed {
                topic: "resync",
                card_id: None,
            },
        );
        Ok(summary)
    }

    /// Removes a saved host and deletes its persisted token, cache, outbox, and read cache.
    pub fn remove_host(&self, host_id: &str) -> Result<()> {
        let removed = {
            let mut guard = self.hosts.write().unwrap();
            let mut slot = guard
                .remove(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            slot.supervisor_epoch = slot.supervisor_epoch.wrapping_add(1);
            slot.clear_connection(HostConnectionState::Offline {
                error: "Removed".to_owned(),
                cached_at: None,
            });
            slot.supervisor_notify.notify_one();
            self.persist_locked(&guard)?;
            slot
        };
        drop(removed);
        remove_host_files(&self.config.data_dir, host_id, self.creds.as_ref());
        self.emit_host(
            host_id,
            AppEvent::Changed {
                topic: "resync",
                card_id: None,
            },
        );
        Ok(())
    }

    /// Renames a saved host and persists the updated host registry.
    pub fn rename_host(&self, host_id: &str, new_name: &str) -> Result<()> {
        let trimmed = new_name.trim();
        ensure!(!trimmed.is_empty(), "Host name cannot be empty");
        {
            let mut guard = self.hosts.write().unwrap();
            let slot = guard
                .get_mut(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            slot.saved.name = trimmed.to_owned();
            self.persist_locked(&guard)?;
        }
        self.emit_host(
            host_id,
            AppEvent::Changed {
                topic: "resync",
                card_id: None,
            },
        );
        Ok(())
    }

    /// Enables or disables a saved host, disconnecting cleanly or spawning/waking its supervisor.
    pub fn set_host_enabled(&self, host_id: &str, enabled: bool) -> Result<()> {
        let (spawn_needed, notify) = {
            let mut guard = self.hosts.write().unwrap();
            let slot = guard
                .get_mut(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            let was_enabled = slot.saved.enabled;
            slot.saved.enabled = enabled;
            let (spawn, n) = if enabled {
                if !was_enabled && slot.client.is_none() {
                    slot.connection = HostConnectionState::Connecting;
                }
                (!was_enabled, Some(Arc::clone(&slot.supervisor_notify)))
            } else {
                let cached_at = slot.cached_at();
                slot.supervisor_epoch = slot.supervisor_epoch.wrapping_add(1);
                slot.clear_connection(HostConnectionState::Offline {
                    error: "Disabled".to_owned(),
                    cached_at,
                });
                (false, Some(Arc::clone(&slot.supervisor_notify)))
            };
            self.persist_locked(&guard)?;
            (spawn, n)
        };
        if spawn_needed {
            self.spawn_supervisor(host_id);
        } else if let Some(n) = notify {
            n.notify_one();
        }
        self.emit_host(
            host_id,
            AppEvent::Changed {
                topic: "resync",
                card_id: None,
            },
        );
        Ok(())
    }

    pub fn saved_hosts(&self) -> Vec<SavedHost> {
        let guard = self.hosts.read().unwrap();
        guard.values().map(|s| s.saved.clone()).collect()
    }

    pub fn host_summaries(&self) -> Vec<HostSummary> {
        let guard = self.hosts.read().unwrap();
        guard.values().map(RemoteHostSlot::summary).collect()
    }

    pub fn config_for_host(&self, host_id: &str) -> Config {
        let guard = self.hosts.read().unwrap();
        guard
            .get(host_id)
            .map(|s| s.cached.config.clone())
            .unwrap_or_default()
    }

    pub fn home_for(&self, host_id: &str) -> String {
        let guard = self.hosts.read().unwrap();
        guard
            .get(host_id)
            .map(|s| s.home.clone())
            .unwrap_or_default()
    }

    pub fn server_notice(&self) -> Option<String> {
        let guard = self.hosts.read().unwrap();
        guard.values().find_map(|s| s.notice.clone())
    }

    pub fn terminal_transport(&self, host_id: &str) -> Option<Arc<dyn PtyTransport>> {
        let guard = self.hosts.read().unwrap();
        guard
            .get(host_id)
            .and_then(|s| s.terminal.as_ref())
            .map(|t| Arc::clone(t) as Arc<dyn PtyTransport>)
    }

    /// Merges all enabled (and offline-with-cache) hosts with optimistic `canban-sync` projections.
    pub fn merged_snapshot(&self) -> Result<Snapshot> {
        let guard = self.hosts.read().unwrap();
        let mut merged = Snapshot::default();
        for slot in guard.values() {
            if !slot.saved.enabled && slot.cached.saved_at == 0 {
                continue;
            }
            let summary = slot.summary();
            merged
                .host_summaries
                .insert(slot.saved.id.clone(), summary.clone());
            let outbox = slot.outbox.lock().unwrap();
            let mut host_snap = slot.cached.snapshot.clone();
            if !outbox.ops.is_empty() {
                canban_sync::apply(&mut host_snap, &slot.saved.id, &outbox.ops);
            }
            host_snap.register_host_entities(&slot.saved.id);
            if slot.saved.enabled || !host_snap.projects.is_empty() || !host_snap.cards.is_empty() {
                merged.merge_remote_snapshot(host_snap, summary);
            }
        }
        Ok(merged)
    }

    /// Merges the lightweight runtime state (`terminals`, `terminal_contexts`, `reclaim_status`,
    /// `merging_cards`) across enabled and offline-cached hosts with optimistic outbox projections.
    pub fn merged_runtime_state(&self) -> Result<RuntimeState> {
        use std::collections::HashSet;

        let guard = self.hosts.read().unwrap();
        let mut combined = RuntimeState::default();
        let mut seen_ids: HashSet<String> = HashSet::new();
        let mut entity_owners: BTreeMap<String, String> = BTreeMap::new();
        for slot in guard.values() {
            if !slot.saved.enabled && slot.cached.saved_at == 0 {
                continue;
            }
            let outbox = slot.outbox.lock().unwrap();
            let mut host_snap = slot.cached.snapshot.clone();
            if !outbox.ops.is_empty() {
                canban_sync::apply(&mut host_snap, &slot.saved.id, &outbox.ops);
            }
            for id in host_snap
                .projects
                .iter()
                .map(|p| &p.id)
                .chain(host_snap.cards.iter().map(|c| &c.id))
                .chain(host_snap.terminals.iter().map(|t| &t.id))
            {
                if !seen_ids.insert(id.clone()) {
                    let q = canban_types::qualify_host_id(&slot.saved.id, id);
                    entity_owners.insert(q, slot.saved.id.clone());
                } else {
                    entity_owners
                        .entry(id.clone())
                        .or_insert_with(|| slot.saved.id.clone());
                }
            }
            let active_agents = count_active_agents(&host_snap);
            let has_durable = !host_snap.projects.is_empty() || !host_snap.cards.is_empty();
            let rt = RuntimeState {
                terminals: host_snap.terminals,
                terminal_contexts: host_snap.terminal_contexts,
                reclaim_status: host_snap.reclaim_status,
                merging_cards: host_snap.merging_cards,
                terminal_hosts: BTreeMap::new(),
                host_active_agents: BTreeMap::from([(slot.saved.id.clone(), active_agents)]),
            };
            if slot.saved.enabled || has_durable || !rt.terminals.is_empty() {
                combined.merge_remote_with_owners(rt, &slot.saved.id, &entity_owners);
            }
        }
        Ok(combined)
    }

    /// Resolves which saved host owns the target entity or project for an RPC call.
    pub fn resolve_target_host(
        &self,
        method: &str,
        params: &Value,
        active_project_id: Option<&str>,
    ) -> String {
        if let Some(explicit) = params["host_id"].as_str().filter(|s| !s.is_empty()) {
            return explicit.to_owned();
        }
        let candidates = entity_id_candidates(method, params);
        for candidate in &candidates {
            if let Some((h, _)) = canban_types::split_qualified_host_id(candidate) {
                return h.to_owned();
            }
        }
        if let Some(pid) = active_project_id.filter(|s| !s.is_empty())
            && let Some((h, _)) = canban_types::split_qualified_host_id(pid)
        {
            return h.to_owned();
        }
        let guard = self.hosts.read().unwrap();
        for candidate in candidates {
            for slot in guard.values() {
                let outbox = slot.outbox.lock().unwrap();
                let in_outbox = outbox.ops.iter().any(|m| m.card_id == candidate);
                if in_outbox || snapshot_contains_entity(&slot.cached.snapshot, candidate) {
                    return slot.saved.id.clone();
                }
            }
        }
        if let Some(pid) = active_project_id.filter(|s| !s.is_empty()) {
            for slot in guard.values() {
                if slot.cached.snapshot.projects.iter().any(|p| p.id == pid)
                    || snapshot_contains_entity(&slot.cached.snapshot, pid)
                {
                    return slot.saved.id.clone();
                }
            }
        }
        guard
            .values()
            .find(|s| s.saved.enabled && s.client.is_some())
            .or_else(|| guard.values().find(|s| s.saved.enabled))
            .or_else(|| guard.values().next())
            .map(|s| s.saved.id.clone())
            .unwrap_or_default()
    }

    /// Executes an entity-routed RPC synchronously (with offline outbox/cache fallback).
    pub fn call_routed(
        &self,
        method: &str,
        params: Value,
        active_project_id: Option<&str>,
    ) -> Result<Value> {
        block_on_rt(
            &self.runtime_handle,
            self.call_routed_async(method, params, active_project_id),
        )
    }

    /// Executes an entity-routed RPC asynchronously (with offline outbox/cache fallback).
    pub async fn call_routed_async(
        &self,
        method: &str,
        mut params: Value,
        active_project_id: Option<&str>,
    ) -> Result<Value> {
        let host_id = self.resolve_target_host(method, &params, active_project_id);
        ensure!(!host_id.is_empty(), "No remote Canban host configured");
        if let Some(obj) = params.as_object_mut() {
            obj.remove("host_id");
        }
        canban_types::strip_qualified_host_ids(&mut params, &host_id);

        if method == "card.resolve.preview" {
            return self.handle_resolve_preview_async(&host_id, &params).await;
        }

        let (client_opt, client_gen, token_opt, card_blocked) = {
            let guard = self.hosts.read().unwrap();
            let slot = guard
                .get(&host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            let blocked = match canban_sync::classify_remote_mutation(method, &params) {
                QueueDecision::Queue(MutationKind::SyncGlobalEnv) => slot
                    .outbox
                    .lock()
                    .unwrap()
                    .blocked_cards()
                    .contains("__global_env__"),
                QueueDecision::Queue(kind) if kind != MutationKind::CreateCard => {
                    let cid = extract_card_id_for_mutation(&params);
                    slot.outbox.lock().unwrap().blocked_cards().contains(&cid)
                }
                _ => false,
            };
            (
                slot.client.clone(),
                slot.conn_gen,
                slot.tcp_token(),
                blocked,
            )
        };

        if let Some(client) = client_opt
            && !card_blocked
        {
            let raw_params_for_cache = params.clone();
            seal_params_for_tcp(method, &mut params, token_opt.as_deref())?;
            match client.call_async(method, &params).await {
                Ok(val) => {
                    self.record_online_read_cache(&host_id, method, &raw_params_for_cache, &val);
                    return Ok(val);
                }
                Err(err) if is_connection_error(&err) => {
                    self.mark_slot_disconnected(&host_id, Some(client_gen), err.to_string(), true);
                    return self.offline_fallback(&host_id, method, raw_params_for_cache, err);
                }
                Err(err) => return Err(err),
            }
        }

        self.offline_fallback(&host_id, method, params, anyhow!("Remote host is offline"))
    }

    async fn handle_resolve_preview_async(&self, host_id: &str, params: &Value) -> Result<Value> {
        let client_opt = {
            let guard = self.hosts.read().unwrap();
            guard
                .get(host_id)
                .and_then(|s| s.client.clone().map(|c| (c, s.conn_gen)))
        };
        if let Some((client, client_gen)) = client_opt {
            match client.call_async("card.resolve.preview", params).await {
                Ok(v) => return Ok(v),
                Err(err) if !is_connection_error(&err) => return Err(err),
                Err(err) => {
                    self.mark_slot_disconnected(host_id, Some(client_gen), err.to_string(), true);
                }
            }
        }
        let guard = self.hosts.read().unwrap();
        let slot = guard
            .get(host_id)
            .with_context(|| format!("Unknown host '{host_id}'"))?;
        let card_id = params["card_id"]
            .as_str()
            .context("card.resolve.preview requires card_id")?;
        canban_sync::offline_resolve_preview(&slot.cached.snapshot, card_id, &slot.saved.name)
    }

    fn record_online_read_cache(&self, host_id: &str, method: &str, params: &Value, val: &Value) {
        if !canban_sync::is_read_cacheable_method(method) {
            return;
        }
        let guard = self.hosts.read().unwrap();
        let Some(slot) = guard.get(host_id) else {
            return;
        };
        let card_id = params.get("card_id").and_then(Value::as_str);
        let card_rev = card_id.and_then(|cid| {
            slot.cached
                .snapshot
                .cards
                .iter()
                .find(|c| c.id == cid)
                .map(|c| c.revision)
        });
        let mut reads = slot.reads.lock().unwrap();
        let _ = reads.put(method, params, val, card_id, card_rev, now());
    }

    fn offline_fallback(
        &self,
        host_id: &str,
        method: &str,
        mut params: Value,
        conn_err: anyhow::Error,
    ) -> Result<Value> {
        if canban_sync::is_read_cacheable_method(method) {
            let guard = self.hosts.read().unwrap();
            let slot = guard
                .get(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            let outbox = slot.outbox.lock().unwrap();
            let mut reads = slot.reads.lock().unwrap();
            let now_ms = now();
            if let Some(cached) = reads.get(method, &params, now_ms) {
                let card_id = params["card_id"].as_str().unwrap_or("");
                return Ok(cache_and_augment_timeline(
                    cached,
                    method,
                    &outbox,
                    &slot.cached.snapshot,
                    card_id,
                ));
            }
            if method == "timeline.list" {
                let card_id = params["card_id"].as_str().unwrap_or("");
                let pending = canban_sync::pending_comment_events(
                    &outbox.ops,
                    card_id,
                    &slot.cached.snapshot,
                );
                if !pending.is_empty() {
                    return Ok(cache_and_augment_timeline(
                        json!({"events": []}),
                        method,
                        &outbox,
                        &slot.cached.snapshot,
                        card_id,
                    ));
                }
            }
            return Err(conn_err);
        }

        match canban_sync::classify_remote_mutation(method, &params) {
            QueueDecision::Queue(kind) => {
                let mutation_id = canban_types::id();
                let card_id = match kind {
                    MutationKind::SyncGlobalEnv => String::new(),
                    MutationKind::CreateCard => {
                        let cid = params["id"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .unwrap_or_else(canban_types::id);
                        if let Some(obj) = params.as_object_mut() {
                            obj.entry("id".to_owned()).or_insert_with(|| json!(cid));
                            obj.entry("request_key".to_owned())
                                .or_insert_with(|| json!(mutation_id));
                        }
                        cid
                    }
                    MutationKind::AddComment => {
                        let cid = extract_card_id_for_mutation(&params);
                        if let Some(obj) = params.as_object_mut() {
                            let ev_id = canban_types::id();
                            obj.entry("id".to_owned()).or_insert_with(|| json!(ev_id));
                            obj.entry("request_key".to_owned())
                                .or_insert_with(|| json!(mutation_id));
                        }
                        cid
                    }
                    _ => extract_card_id_for_mutation(&params),
                };

                let receipt = {
                    let guard = self.hosts.read().unwrap();
                    let slot = guard
                        .get(host_id)
                        .with_context(|| format!("Unknown host '{host_id}'"))?;
                    let base = if kind == MutationKind::UpdateCard {
                        build_update_base(&params, &slot.cached.snapshot, &card_id)
                    } else {
                        UpdateBase::default()
                    };
                    let mutation = Mutation {
                        id: mutation_id,
                        host_id: host_id.to_owned(),
                        card_id: card_id.clone(),
                        kind,
                        params,
                        base,
                        sent_params: Value::Null,
                        retry_same_params: false,
                        state: MutationState::Queued,
                        created_at: now(),
                        attempts: 0,
                    };
                    let mut outbox = slot.outbox.lock().unwrap();
                    let (stored, _) = outbox.enqueue(mutation)?;
                    outbox.save()?;
                    canban_sync::synthetic_receipt(
                        &stored,
                        &slot.cached.snapshot,
                        &outbox.ops,
                        host_id,
                    )
                    .context("Failed to compute offline synthetic receipt")?
                };
                self.emit_host(
                    host_id,
                    AppEvent::Changed {
                        topic: "application",
                        card_id: (!card_id.is_empty()).then_some(card_id),
                    },
                );
                Ok(receipt)
            }
            QueueDecision::RejectOffline(msg) => bail!("{msg}"),
            QueueDecision::NotQueueable => Err(conn_err),
        }
    }

    fn mark_slot_disconnected(
        &self,
        host_id: &str,
        expected_gen: Option<u64>,
        err_msg: String,
        wake_supervisor: bool,
    ) {
        let changed = {
            let mut guard = self.hosts.write().unwrap();
            let Some(slot) = guard.get_mut(host_id) else {
                return;
            };
            if expected_gen.is_some_and(|g| slot.conn_gen != g) {
                return;
            }
            let cached_at = slot.cached_at();
            let auth = is_auth_error(&anyhow!("{err_msg}"));
            let next_state = if !slot.saved.enabled {
                HostConnectionState::Offline {
                    error: "Disabled".to_owned(),
                    cached_at,
                }
            } else if auth {
                HostConnectionState::AuthFailed { error: err_msg }
            } else {
                HostConnectionState::Offline {
                    error: err_msg,
                    cached_at,
                }
            };
            let was_conn = slot.client.is_some();
            slot.clear_connection(next_state);
            if wake_supervisor && was_conn {
                slot.supervisor_notify.notify_one();
            }
            was_conn
        };
        if changed {
            self.emit_host(
                host_id,
                AppEvent::Changed {
                    topic: "resync",
                    card_id: None,
                },
            );
        }
    }

    /// Fans out `method` across all enabled hosts synchronously.
    pub fn call_all_hosts(&self, method: &str, params: Value) -> Vec<(String, Result<Value>)> {
        block_on_rt(
            &self.runtime_handle,
            self.call_all_hosts_async(method, params),
        )
    }

    /// Fans out `method` across all connected hosts concurrently (skipping offline/disabled hosts).
    pub async fn call_all_hosts_async(
        &self,
        method: &str,
        params: Value,
    ) -> Vec<(String, Result<Value>)> {
        let targets: Vec<(String, Arc<ClientTransport>, Option<String>)> = {
            let guard = self.hosts.read().unwrap();
            guard
                .values()
                .filter(|s| s.saved.enabled && s.connection.is_connected())
                .filter_map(|s| Some((s.saved.id.clone(), s.client.clone()?, s.tcp_token())))
                .collect()
        };
        let mut set = tokio::task::JoinSet::new();
        for (host_id, client, token_opt) in targets {
            let method = method.to_owned();
            let mut p = params.clone();
            set.spawn(async move {
                let res = match seal_params_for_tcp(&method, &mut p, token_opt.as_deref()) {
                    Err(e) => Err(e),
                    Ok(()) => client.call_async(&method, &p).await,
                };
                (host_id, res)
            });
        }
        let mut out = Vec::new();
        while let Some(joined) = set.join_next().await {
            if let Ok(item) = joined {
                out.push(item);
            }
        }
        out
    }

    /// Returns queued, in-flight, conflicted, and failed outbox items across all hosts.
    pub fn sync_items(&self) -> Vec<SyncItemView> {
        let guard = self.hosts.read().unwrap();
        let mut items = Vec::new();
        for slot in guard.values() {
            let outbox = slot.outbox.lock().unwrap();
            for op in &outbox.ops {
                let server_card = slot
                    .cached
                    .snapshot
                    .cards
                    .iter()
                    .find(|c| c.id == op.card_id);
                let (state, message, conflict_fields, mine, server) = match &op.state {
                    MutationState::Queued => (
                        SyncItemState::Queued,
                        String::new(),
                        Vec::new(),
                        BTreeMap::new(),
                        BTreeMap::new(),
                    ),
                    MutationState::InFlight => (
                        SyncItemState::InFlight,
                        String::new(),
                        Vec::new(),
                        BTreeMap::new(),
                        BTreeMap::new(),
                    ),
                    MutationState::Conflict { .. } => {
                        let (fields, mine, server) = canban_sync::conflict_details(op, server_card);
                        (
                            SyncItemState::Conflict,
                            "Concurrent edit on server".to_owned(),
                            fields,
                            mine,
                            server,
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
                let card_title = server_card
                    .map(|c| c.title.clone())
                    .or_else(|| {
                        op.params
                            .get("title")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| op.kind.label().to_owned());
                items.push(SyncItemView {
                    host_id: slot.saved.id.clone(),
                    host_name: slot.saved.name.clone(),
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

    /// Applies a user resolution or retry/discard action to an outbox mutation.
    pub fn sync_action(&self, host_id: &str, mutation_id: &str, action: SyncAction) -> Result<()> {
        let (card_id, wake_drain) = {
            let guard = self.hosts.read().unwrap();
            let slot = guard
                .get(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            let server_cards = slot.cached.snapshot.cards.clone();
            let mut outbox = slot.outbox.lock().unwrap();
            match action {
                SyncAction::Discard => {
                    let removed = outbox.remove_by_id(mutation_id)?;
                    outbox.save()?;
                    (removed.map(|m| m.card_id), false)
                }
                SyncAction::Retry => {
                    let op = outbox
                        .ops
                        .iter_mut()
                        .find(|m| m.id == mutation_id)
                        .with_context(|| format!("Unknown mutation '{mutation_id}'"))?;
                    op.state = MutationState::Queued;
                    op.retry_same_params = false;
                    let cid = op.card_id.clone();
                    outbox.save()?;
                    (Some(cid), slot.saved.enabled)
                }
                SyncAction::UseMine => {
                    let op = outbox
                        .ops
                        .iter_mut()
                        .find(|m| m.id == mutation_id)
                        .with_context(|| format!("Unknown mutation '{mutation_id}'"))?;
                    let srv = server_cards.iter().find(|c| c.id == op.card_id);
                    if let Some(c) = srv {
                        op.base.title = op.params.get("title").is_some().then(|| c.title.clone());
                        op.base.body = op.params.get("body").is_some().then(|| c.body.clone());
                        if let Some(obj) = op.params.as_object_mut()
                            && obj.contains_key("revision")
                        {
                            obj.insert("revision".into(), json!(c.revision));
                        }
                    }
                    op.sent_params = Value::Null;
                    op.retry_same_params = false;
                    op.state = MutationState::Queued;
                    let cid = op.card_id.clone();
                    outbox.save()?;
                    (Some(cid), slot.saved.enabled)
                }
                SyncAction::UseEdited { fields } => {
                    let op = outbox
                        .ops
                        .iter_mut()
                        .find(|m| m.id == mutation_id)
                        .with_context(|| format!("Unknown mutation '{mutation_id}'"))?;
                    let srv = server_cards.iter().find(|c| c.id == op.card_id);
                    if let Some(obj) = op.params.as_object_mut() {
                        for (k, v) in &fields {
                            obj.insert(k.clone(), json!(v));
                        }
                        if let Some(c) = srv
                            && obj.contains_key("revision")
                        {
                            obj.insert("revision".into(), json!(c.revision));
                        }
                    }
                    if let Some(c) = srv {
                        op.base.title = op.params.get("title").is_some().then(|| c.title.clone());
                        op.base.body = op.params.get("body").is_some().then(|| c.body.clone());
                    }
                    op.sent_params = Value::Null;
                    op.retry_same_params = false;
                    op.state = MutationState::Queued;
                    let cid = op.card_id.clone();
                    outbox.save()?;
                    (Some(cid), slot.saved.enabled)
                }
            }
        };

        self.emit_host(
            host_id,
            AppEvent::Changed {
                topic: "application",
                card_id,
            },
        );
        if wake_drain && let Some(this) = self.upgrade_self() {
            let hid = host_id.to_owned();
            self.runtime_handle.spawn(async move {
                let _ = this.drain_outbox_async(&hid).await;
            });
        }
        Ok(())
    }

    pub fn global_env_sync_summary(&self) -> GlobalEnvSyncSummary {
        let guard = self.hosts.read().unwrap();
        let enabled: Vec<&RemoteHostSlot> = guard.values().filter(|s| s.saved.enabled).collect();
        let total_remote_hosts = enabled.len();
        let mut pending_hosts = Vec::new();
        let mut synced_hosts = 0;
        for slot in enabled {
            let outbox = slot.outbox.lock().unwrap();
            let has_pending_env = outbox
                .ops
                .iter()
                .any(|m| m.kind == MutationKind::SyncGlobalEnv);
            if has_pending_env || slot.client.is_none() {
                pending_hosts.push(slot.saved.name.clone());
            } else {
                synced_hosts += 1;
            }
        }
        GlobalEnvSyncSummary {
            synced_hosts,
            pending_hosts,
            total_remote_hosts,
        }
    }

    async fn connect_slot_async(&self, host_id: &str) -> Result<(Arc<ClientTransport>, u64)> {
        let (transport_spec, server_id, cached_at, saved_at, last_marker) = {
            let guard = self.hosts.read().unwrap();
            let slot = guard
                .get(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            ensure!(slot.saved.enabled, "Host '{host_id}' is disabled");
            let spec = match &slot.saved.transport {
                HostTransport::Tcp { addr, token } => {
                    let norm = normalize_daemon_addr(addr.as_str());
                    ensure!(!norm.is_empty(), "No TCP address configured for host");
                    ensure!(
                        !token.trim().is_empty(),
                        "No device token found for host '{host_id}'"
                    );
                    HostTransport::Tcp {
                        addr: norm,
                        token: token.clone(),
                    }
                }
                other => other.clone(),
            };
            (
                spec,
                slot.saved.server_id.clone(),
                slot.cached_at(),
                slot.cached.saved_at,
                slot.last_marker.clone(),
            )
        };

        let after = if saved_at == 0 {
            Value::Null
        } else {
            last_marker["revision"].clone()
        };
        let session = match SingleHostSession::connect_async(
            self.connector.as_ref(),
            host_id,
            &transport_spec,
            &self.config.device_name,
            after,
        )
        .await
        {
            Ok(session) => session,
            Err(err) => {
                let err_str = err.to_string();
                let auth = is_auth_error(&err);
                {
                    let mut guard = self.hosts.write().unwrap();
                    if let Some(slot) = guard.get_mut(host_id) {
                        slot.reconnect_count = slot.reconnect_count.saturating_add(1);
                        let next_state = if auth {
                            HostConnectionState::AuthFailed { error: err_str }
                        } else {
                            HostConnectionState::Offline {
                                error: err_str,
                                cached_at,
                            }
                        };
                        slot.clear_connection(next_state);
                    }
                }
                self.emit_host(
                    host_id,
                    AppEvent::Changed {
                        topic: "resync",
                        card_id: None,
                    },
                );
                return Err(err);
            }
        };

        let upgraded = session
            .upgraded_token()
            .filter(|t| is_device_token(t))
            .map(str::to_owned);
        let (transport, lease, hs) = session.into_parts();

        if let Some(expected_sid) = server_id.as_deref()
            && let Some(actual_sid) = hs.server_id.as_deref()
            && actual_sid != expected_sid
        {
            transport.close();
            drop(lease);
            let err_msg =
                format!("Connected to wrong server (expected {expected_sid}, got {actual_sid})");
            let mut guard = self.hosts.write().unwrap();
            if let Some(slot) = guard.get_mut(host_id) {
                slot.clear_connection(HostConnectionState::AuthFailed {
                    error: err_msg.clone(),
                });
            }
            bail!("{err_msg}");
        }

        let conn_gen = {
            let mut guard = self.hosts.write().unwrap();
            let slot = guard
                .get_mut(host_id)
                .with_context(|| format!("Host '{host_id}' was removed"))?;
            slot.clear_connection(HostConnectionState::Connecting);
            let mut persist_needed = false;
            if let HostTransport::Tcp { token: cur, .. } = &mut slot.saved.transport
                && let Some(new_tok) = upgraded
                && *cur != new_tok
            {
                *cur = new_tok;
                persist_needed = true;
            }
            slot.terminal = Some(Arc::new(RemotePtyTransport::new(
                Arc::clone(&transport),
                self.runtime_handle.clone(),
            )));
            slot.lease = lease;
            slot.client = Some(Arc::clone(&transport));
            apply_state_to_slot(
                slot,
                &self.config.data_dir,
                &hs.state,
                Some(hs.rtt_ms),
                Some(&hs.ping),
                None,
            );
            let conn_gen = slot.conn_gen;
            if persist_needed {
                let _ = self.persist_locked(&guard);
            }
            conn_gen
        };

        self.emit_host(
            host_id,
            AppEvent::Changed {
                topic: "resync",
                card_id: None,
            },
        );
        Ok((transport, conn_gen))
    }

    async fn refresh_host_now_async(
        &self,
        host_id: &str,
        client: &Arc<ClientTransport>,
        preferred_topic: Option<&'static str>,
        card_id: Option<String>,
    ) -> Result<()> {
        let (after, saved_at) = {
            let guard = self.hosts.read().unwrap();
            let slot = guard
                .get(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            (slot.last_marker.clone(), slot.cached.saved_at)
        };
        let req_after = if saved_at == 0 {
            Value::Null
        } else {
            after["revision"].clone()
        };
        let start = Instant::now();
        let state_val = client
            .call_async("app.state", &json!({"after": req_after}))
            .await?;
        let rtt_ms = start.elapsed().as_millis() as u64;

        let emitted_topic = {
            let mut guard = self.hosts.write().unwrap();
            let Some(slot) = guard.get_mut(host_id) else {
                return Ok(());
            };
            apply_state_to_slot(
                slot,
                &self.config.data_dir,
                &state_val,
                Some(rtt_ms),
                None,
                preferred_topic,
            )
        };
        if let Some(topic) = emitted_topic {
            self.emit_host(host_id, AppEvent::Changed { topic, card_id });
        }
        Ok(())
    }

    async fn refresh_host_runtime_async(
        &self,
        host_id: &str,
        client: &Arc<ClientTransport>,
        topic: &'static str,
        card_id: Option<String>,
        latest_marker: &Value,
    ) -> Result<()> {
        let start = Instant::now();
        let mut val = match client.call_async("runtime.state", &json!({})).await {
            Ok(v) => v,
            Err(err) if !is_connection_error(&err) => {
                return self
                    .refresh_host_now_async(host_id, client, Some(topic), card_id)
                    .await;
            }
            Err(err) => return Err(err),
        };
        let rtt_ms = start.elapsed().as_millis() as u64;
        let Ok(mut rt) = serde_json::from_value::<RuntimeState>(val["runtime"].take()) else {
            return self
                .refresh_host_now_async(host_id, client, Some(topic), card_id)
                .await;
        };
        rt.terminal_hosts.clear();
        rt.host_active_agents.clear();
        rt.register_host(host_id);
        let active_agents = count_active_agents_in(&rt.terminals);
        rt.host_active_agents
            .insert(host_id.to_owned(), active_agents);

        {
            let mut guard = self.hosts.write().unwrap();
            let Some(slot) = guard.get_mut(host_id) else {
                return Ok(());
            };
            slot.cached.snapshot.apply_runtime_state(rt);
            slot.cached.snapshot.register_host_entities(host_id);
            slot.last_synced_at = Some(now());
            if let HostConnectionState::Connected {
                rtt_ms: ref mut prev_rtt,
                ..
            } = slot.connection
            {
                *prev_rtt = rtt_ms;
            }
            if !latest_marker["revision"].is_null() {
                slot.last_marker = latest_marker.clone();
            }
        }
        self.emit_host(host_id, AppEvent::Changed { topic, card_id });
        Ok(())
    }

    async fn drain_outbox_async(&self, host_id: &str) -> Result<DrainReport> {
        let draining_lock = {
            let guard = self.hosts.read().unwrap();
            let slot = guard
                .get(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            Arc::clone(&slot.draining)
        };
        let _drain_guard = draining_lock.lock().await;

        let mut report = DrainReport::default();
        let mut blocked = BTreeSet::new();
        let mut rebuilt_ids = BTreeSet::new();
        let mut active_client: Option<Arc<ClientTransport>> = None;

        loop {
            let step = {
                let guard = self.hosts.read().unwrap();
                let Some(slot) = guard.get(host_id) else {
                    break;
                };
                if !slot.saved.enabled {
                    break;
                }
                let Some(client) = slot.client.clone() else {
                    break;
                };
                let client_gen = slot.conn_gen;
                active_client = Some(Arc::clone(&client));
                let token = slot.tcp_token();
                let mut outbox = slot.outbox.lock().unwrap();
                let action = prepare_next_step(
                    &mut outbox,
                    &slot.cached.snapshot,
                    &mut blocked,
                    &mut report,
                );
                let _ = outbox.save();
                match action {
                    StepAction::Idle => None,
                    StepAction::Send {
                        mutation_id,
                        method,
                        sent_params,
                        was_retry_same_params,
                    } => Some((
                        client,
                        client_gen,
                        token,
                        mutation_id,
                        method,
                        sent_params,
                        was_retry_same_params,
                    )),
                }
            };

            let Some((
                client,
                client_gen,
                token,
                mutation_id,
                method,
                mut sent_params,
                was_retry_same_params,
            )) = step
            else {
                break;
            };

            let allow_rebuild = !rebuilt_ids.contains(&mutation_id);
            seal_params_for_tcp(method, &mut sent_params, token.as_deref())?;
            let result = client.call_async(method, &sent_params).await;
            let (is_conn_err, refreshed) = match &result {
                Ok(_) => (false, None),
                Err(err) => {
                    let conn = is_connection_error(err);
                    let msg = format!("{err:#}");
                    let need_refresh = !conn
                        && ((msg.contains("revision_conflict")
                            && !was_retry_same_params
                            && allow_rebuild)
                            || msg.contains("request_key_conflict"));
                    let snap = if need_refresh {
                        client
                            .call_async("app.state", &json!({}))
                            .await
                            .ok()
                            .and_then(|v| parse_refreshed_snapshot(&v, host_id))
                    } else {
                        None
                    };
                    (conn, snap)
                }
            };

            let outcome = {
                let mut guard = self.hosts.write().unwrap();
                let Some(slot) = guard.get_mut(host_id) else {
                    break;
                };
                let mut outbox = slot.outbox.lock().unwrap();
                let mut snap = slot.cached.snapshot.clone();
                let outcome = finish_step(
                    &mut outbox,
                    &mut snap,
                    &mutation_id,
                    result,
                    was_retry_same_params,
                    is_conn_err,
                    refreshed,
                    allow_rebuild,
                    &mut blocked,
                    &mut report,
                );
                let _ = outbox.save();
                snap.register_host_entities(host_id);
                slot.cached.snapshot = snap;
                slot.cached.saved_at = now();
                save_cache(&self.config.data_dir, host_id, &slot.cached);
                outcome
            };

            match outcome {
                FinishOutcome::Continue => {}
                FinishOutcome::RetryImmediate => {
                    rebuilt_ids.insert(mutation_id);
                }
                FinishOutcome::StopConnection => {
                    self.mark_slot_disconnected(
                        host_id,
                        Some(client_gen),
                        "Connection lost during outbox replay".to_owned(),
                        true,
                    );
                    break;
                }
            }
        }

        if report.applied > 0 || report.conflicts > 0 || report.failed > 0 {
            if let Some(client) = active_client {
                let _ = self
                    .refresh_host_now_async(host_id, &client, Some("application"), None)
                    .await;
            }
            self.emit_host(
                host_id,
                AppEvent::Changed {
                    topic: "application",
                    card_id: None,
                },
            );
        }
        Ok(report)
    }

    async fn sync_one_host_once(&self, host_id: &str) -> Result<()> {
        let existing = {
            let guard = self.hosts.read().unwrap();
            let Some(slot) = guard.get(host_id) else {
                return Ok(());
            };
            if !slot.saved.enabled {
                return Ok(());
            }
            slot.client.clone().map(|c| (c, slot.conn_gen))
        };
        match existing {
            Some((c, client_gen)) => {
                if let Err(err) = self.refresh_host_now_async(host_id, &c, None, None).await {
                    if is_connection_error(&err) {
                        self.mark_slot_disconnected(
                            host_id,
                            Some(client_gen),
                            err.to_string(),
                            true,
                        );
                        self.connect_slot_async(host_id).await?.0
                    } else {
                        return Err(err);
                    }
                } else {
                    c
                }
            }
            None => self.connect_slot_async(host_id).await?.0,
        };
        self.drain_outbox_async(host_id).await?;
        Ok(())
    }
}

struct PendingRefresh {
    topic: &'static str,
    card_id: Option<String>,
    latest_marker: Value,
    deadline: tokio::time::Instant,
    max_deadline: tokio::time::Instant,
}

async fn run_legacy_poll_loop(
    rc: &Arc<RemoteClient>,
    host_id: &str,
    epoch: u64,
    conn_gen: u64,
    transport: &Arc<ClientTransport>,
    notify: &Arc<Notify>,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> bool {
    let slot_active = || {
        let guard = rc.hosts.read().unwrap();
        guard.get(host_id).is_some_and(|s| {
            s.saved.enabled
                && s.supervisor_epoch == epoch
                && s.conn_gen == conn_gen
                && s.client.is_some()
        })
    };
    while slot_active() {
        tokio::select! {
            _ = tokio::time::sleep(LEGACY_POLL_INTERVAL) => {}
            _ = notify.notified() => {}
            _ = shutdown_rx.changed() => { return true; }
        }
        if !slot_active() {
            break;
        }
        match rc
            .refresh_host_now_async(host_id, transport, None, None)
            .await
        {
            Ok(()) => {
                let _ = rc.drain_outbox_async(host_id).await;
            }
            Err(err) if is_connection_error(&err) || is_auth_error(&err) => {
                rc.mark_slot_disconnected(host_id, Some(conn_gen), err.to_string(), false);
                break;
            }
            Err(_) => {}
        }
    }
    false
}

async fn host_supervisor_task(
    weak_client: Weak<RemoteClient>,
    host_id: String,
    epoch: u64,
    notify: Arc<Notify>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut backoff = Duration::from_millis(250);
    loop {
        if *shutdown_rx.borrow() {
            return;
        }
        let Some(rc) = weak_client.upgrade() else {
            return;
        };
        let (enabled, epoch_matches, token) = {
            let guard = rc.hosts.read().unwrap();
            let Some(slot) = guard.get(&host_id) else {
                return;
            };
            (
                slot.saved.enabled,
                slot.supervisor_epoch == epoch,
                slot.tcp_token(),
            )
        };
        if !enabled || !epoch_matches {
            return;
        }
        if token.as_deref().is_some_and(is_pairing_code) {
            tokio::select! {
                _ = notify.notified() => {}
                _ = shutdown_rx.changed() => { return; }
            }
            continue;
        }

        let (transport, conn_gen, last_marker) = {
            let existing = {
                let guard = rc.hosts.read().unwrap();
                guard.get(&host_id).and_then(|s| {
                    s.client
                        .clone()
                        .map(|c| (c, s.conn_gen, s.last_marker.clone()))
                })
            };
            if let Some(pair) = existing {
                pair
            } else {
                match rc.connect_slot_async(&host_id).await {
                    Ok((c, conn_gen)) => {
                        let marker = rc
                            .hosts
                            .read()
                            .unwrap()
                            .get(&host_id)
                            .map(|s| s.last_marker.clone())
                            .unwrap_or(Value::Null);
                        (c, conn_gen, marker)
                    }
                    Err(err) => {
                        if is_auth_error(&err) {
                            tokio::select! {
                                _ = notify.notified() => {}
                                _ = shutdown_rx.changed() => { return; }
                            }
                        } else {
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => {
                                    backoff = (backoff * 2).min(MAX_BACKOFF);
                                }
                                _ = notify.notified() => {
                                    backoff = Duration::from_millis(250);
                                }
                                _ = shutdown_rx.changed() => { return; }
                            }
                        }
                        continue;
                    }
                }
            }
        };

        let _ = rc.drain_outbox_async(&host_id).await;

        let sub_res = open_async_subscribe(
            &transport,
            &last_marker["revision"],
            SUBSCRIBE_TOPICS,
            Some(15_000),
        )
        .await;
        let (mut stream, handshake) = match sub_res {
            Ok(res) => {
                backoff = Duration::from_millis(250);
                res
            }
            Err(err) if is_unsupported_subscribe_error(&err) => {
                backoff = Duration::from_millis(250);
                if run_legacy_poll_loop(
                    &rc,
                    &host_id,
                    epoch,
                    conn_gen,
                    &transport,
                    &notify,
                    &mut shutdown_rx,
                )
                .await
                {
                    return;
                }
                continue;
            }
            Err(err) => {
                rc.mark_slot_disconnected(&host_id, Some(conn_gen), err.to_string(), false);
                if is_auth_error(&err) {
                    tokio::select! {
                        _ = notify.notified() => { backoff = Duration::from_millis(250); }
                        _ = shutdown_rx.changed() => { return; }
                    }
                } else {
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                        }
                        _ = notify.notified() => { backoff = Duration::from_millis(250); }
                        _ = shutdown_rx.changed() => { return; }
                    }
                }
                continue;
            }
        };

        if handshake.changed {
            let _ = rc
                .refresh_host_now_async(&host_id, &transport, None, None)
                .await;
            let _ = rc.drain_outbox_async(&host_id).await;
        }

        let mut pending: Option<PendingRefresh> = None;

        loop {
            {
                let guard = rc.hosts.read().unwrap();
                let still_valid = guard.get(&host_id).is_some_and(|s| {
                    s.saved.enabled
                        && s.supervisor_epoch == epoch
                        && s.conn_gen == conn_gen
                        && s.client.is_some()
                });
                if !still_valid {
                    break;
                }
            }

            tokio::select! {
                _ = shutdown_rx.changed() => {
                    return;
                }
                _ = notify.notified() => {
                    let _ = rc.refresh_host_now_async(&host_id, &transport, None, None).await;
                    let _ = rc.drain_outbox_async(&host_id).await;
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
                    let (still_active, already_fetched, can_use_runtime) = {
                        let guard = rc.hosts.read().unwrap();
                        let Some(slot) = guard.get(&host_id) else {
                            break;
                        };
                        let active = slot.saved.enabled
                            && slot.supervisor_epoch == epoch
                            && slot.conn_gen == conn_gen
                            && slot.client.is_some();
                        let rev = &p.latest_marker["revision"];
                        let up_to_date = p.topic != "resync"
                            && !rev.is_null()
                            && slot.last_marker["revision"] == *rev;
                        let can_runtime = is_runtime_topic(p.topic) && slot.cached.saved_at > 0;
                        (active, up_to_date, can_runtime)
                    };
                    if !still_active {
                        break;
                    }
                    if already_fetched {
                        continue;
                    }
                    let refresh_res = if can_use_runtime {
                        rc.refresh_host_runtime_async(
                            &host_id,
                            &transport,
                            p.topic,
                            p.card_id,
                            &p.latest_marker,
                        )
                        .await
                    } else {
                        rc.refresh_host_now_async(&host_id, &transport, Some(p.topic), p.card_id)
                            .await
                    };
                    if let Err(err) = refresh_res
                        && is_connection_error(&err)
                    {
                        rc.mark_slot_disconnected(&host_id, Some(conn_gen), err.to_string(), false);
                        break;
                    }
                }
                frame_res = stream.next_frame() => {
                    match frame_res {
                        Ok(Some(PushFrame::Heartbeat)) => {
                            let mut guard = rc.hosts.write().unwrap();
                            if let Some(slot) = guard.get_mut(&host_id) {
                                slot.last_synced_at = Some(now());
                            }
                        }
                        Ok(Some(PushFrame::StateChanged { topic, card_id, marker })) => {
                            let rev = &marker["revision"];
                            let same_marker = {
                                let guard = rc.hosts.read().unwrap();
                                guard.get(&host_id).is_some_and(|s| {
                                    topic != "resync"
                                        && !rev.is_null()
                                        && s.last_marker["revision"] == *rev
                                })
                            };
                            if same_marker {
                                continue;
                            }
                            let now_i = tokio::time::Instant::now();
                            match &mut pending {
                                Some(p) => {
                                    p.topic = merge_push_topic(p.topic, topic);
                                    if p.card_id != card_id {
                                        p.card_id = None;
                                    }
                                    if !rev.is_null() {
                                        p.latest_marker = marker;
                                    }
                                    p.deadline = (now_i + PUSH_DEBOUNCE).min(p.max_deadline);
                                }
                                None => {
                                    pending = Some(PendingRefresh {
                                        topic,
                                        card_id,
                                        latest_marker: marker,
                                        deadline: now_i + PUSH_DEBOUNCE,
                                        max_deadline: now_i + PUSH_MAX_DEBOUNCE,
                                    });
                                }
                            }
                        }
                        Ok(Some(PushFrame::UpdateChanged(state))) => {
                            rc.emit(ClientEvent::UpdateChanged { host_id: host_id.clone(), state });
                        }
                        Ok(Some(PushFrame::UtilitiesChanged(state))) => {
                            rc.emit(ClientEvent::UtilitiesChanged { host_id: host_id.clone(), state });
                        }
                        Ok(Some(PushFrame::UsageChanged(state))) => {
                            rc.emit(ClientEvent::UsageChanged { host_id: host_id.clone(), state });
                        }
                        Ok(Some(PushFrame::HerdrInstall(state))) => {
                            rc.emit(ClientEvent::HerdrInstall { host_id: host_id.clone(), state });
                        }
                        Ok(None) => {
                            rc.mark_slot_disconnected(
                                &host_id,
                                Some(conn_gen),
                                "Server closed push subscription".into(),
                                false,
                            );
                            break;
                        }
                        Err(err) => {
                            rc.mark_slot_disconnected(&host_id, Some(conn_gen), err.to_string(), false);
                            break;
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
    use crate::{pairing::format_pairing_uri, parse_pairing_invite, store::MemoryCredentialStore};
    use canban_app_server::AppServer;
    use canban_types::{Card, Project, ServerListenConfig, SyncMark};

    async fn start_tcp_server_with_verifier(
        home: PathBuf,
        port: u16,
        verifier: Option<Arc<dyn canban_transport::TailscalePeerVerifier>>,
    ) -> (Arc<AppServer>, canban_app_server::Running, String) {
        let server = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
        if let Some(v) = verifier {
            server.set_tailscale_verifier(Some(v));
        }
        let running = server
            .start_with_listen(Some(ServerListenConfig {
                tailscale: false,
                port,
                listen: Some(format!("127.0.0.1:{port}")),
                allow_insecure_bind: false,
                trust_tailscale_self: true,
            }))
            .unwrap();
        let addr = server
            .bound_tcp_addrs()
            .into_iter()
            .next()
            .expect("server bound TCP address");
        (server, running, addr)
    }

    async fn start_tcp_server(
        home: PathBuf,
        port: u16,
    ) -> (Arc<AppServer>, canban_app_server::Running, String) {
        start_tcp_server_with_verifier(home, port, None).await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pair_with_invite_connects_persists_device_token_receives_push_and_keeps_pty_transport()
    {
        let dir = tempfile::tempdir().unwrap();
        let server_home = dir.path().join("server-home");
        let client_dir = dir.path().join("client-data");
        let repo_dir = dir.path().join("repo");
        std::fs::create_dir_all(&repo_dir).unwrap();

        let (server, _running, server_addr) = start_tcp_server(server_home, 0).await;
        let project: Project = serde_json::from_value(
            server
                .call(
                    "project.create",
                    json!({"name": "Mobile Sync Demo", "directory": repo_dir}),
                )
                .unwrap(),
        )
        .unwrap();
        let (code, _) = server.create_pairing_code(300).unwrap();
        let ping = server.call("app.ping", json!({})).unwrap();
        let server_id = ping["server_id"].as_str().unwrap().to_owned();

        let uri = format_pairing_uri(&server_addr, &code, Some(&server_id), Some("Studio Mac"));
        let invite = parse_pairing_invite(&uri).expect("valid single-address invite");
        assert_eq!(invite.addr, server_addr);
        assert_eq!(invite.security, TransportSecurity::UnverifiedTcp);

        let creds = Arc::new(MemoryCredentialStore::new());
        let client = RemoteClient::with_runtime(
            RemoteClientConfig::new(client_dir.clone()),
            creds.clone(),
            tokio::runtime::Handle::current(),
        )
        .unwrap();

        let summary = client
            .pair_with_invite(&invite, Some("Paired Mac"))
            .await
            .expect("pairing succeeds");
        assert!(summary.is_connected());
        assert_eq!(summary.name, "Paired Mac");
        assert_eq!(summary.project_count, 1);

        let saved_tok = creds
            .get_token(&summary.id)
            .unwrap()
            .expect("device token stored");
        assert!(
            is_device_token(&saved_tok),
            "expected dev_ token, got {saved_tok}"
        );
        let raw_hosts = std::fs::read_to_string(client_dir.join("hosts.json")).unwrap();
        assert!(!raw_hosts.contains(&saved_tok));
        assert!(!raw_hosts.contains(&code));
        assert!(!client_dir.join("host_addrs.json").exists());

        let pty_before = client
            .terminal_transport(&summary.id)
            .expect("remote PTY transport ready");

        let mut events = client.subscribe();
        let pushed_card: Card = serde_json::from_value(
            server
                .call(
                    "card.create",
                    json!({
                        "view": "full",
                        "project_id": project.id,
                        "title": "Server Pushed Card",
                        "body": "Delivered over app.subscribe",
                    }),
                )
                .unwrap(),
        )
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snap = client.merged_snapshot().unwrap();
            if snap.cards.iter().any(|c| c.id == pushed_card.id) {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "timed out waiting for server push");
            let ev = tokio::time::timeout(remaining, events.recv())
                .await
                .expect("push event received in time")
                .expect("event channel open");
            if let ClientEvent::Host { host_id, .. } = ev {
                assert_eq!(host_id, summary.id);
            }
        }

        let pty_after = client
            .terminal_transport(&summary.id)
            .expect("remote PTY transport kept");
        assert!(
            Arc::ptr_eq(&pty_before, &pty_after),
            "PtyTransport instance must remain stable across state refreshes"
        );

        client.rename_host(&summary.id, "Renamed Mac").unwrap();
        assert_eq!(client.host_summaries()[0].name, "Renamed Mac");

        client.set_host_enabled(&summary.id, false).unwrap();
        assert!(client.terminal_transport(&summary.id).is_none());
        assert!(!client.host_summaries()[0].is_connected());

        client.set_host_enabled(&summary.id, true).unwrap();
        let reconnect_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if client.host_summaries()[0].is_connected() {
                break;
            }
            assert!(
                Instant::now() < reconnect_deadline,
                "host failed to reconnect using stored dev_ token"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        client.remove_host(&summary.id).unwrap();
        assert!(client.host_summaries().is_empty());
        assert_eq!(creds.get_token(&summary.id).unwrap(), None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn offline_mutations_read_cache_replay_and_three_way_conflict_use_mine() {
        let dir = tempfile::tempdir().unwrap();
        let server_home = dir.path().join("server-home");
        let client_dir = dir.path().join("client-data");
        let repo_dir = dir.path().join("repo");
        std::fs::create_dir_all(&repo_dir).unwrap();

        let (server1, running1, server_addr) = start_tcp_server(server_home.clone(), 0).await;
        let port: u16 = server_addr.rsplit(':').next().unwrap().parse().unwrap();
        let project: Project = serde_json::from_value(
            server1
                .call(
                    "project.create",
                    json!({"name": "Offline Demo", "directory": repo_dir}),
                )
                .unwrap(),
        )
        .unwrap();
        let card_a: Card = serde_json::from_value(
            server1
                .call(
                    "card.create",
                    json!({
                        "view": "full",
                        "project_id": project.id,
                        "title": "Original Title",
                        "body": "Original Body",
                    }),
                )
                .unwrap(),
        )
        .unwrap();
        server1
            .call(
                "comment.add",
                json!({"card_id": card_a.id, "body": "Initial timeline note"}),
            )
            .unwrap();
        let (code, _) = server1.create_pairing_code(300).unwrap();
        let server1_id = server1.call("app.ping", json!({})).unwrap()["server_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let creds = Arc::new(MemoryCredentialStore::new());
        let client = RemoteClient::with_runtime(
            RemoteClientConfig::new(client_dir),
            creds,
            tokio::runtime::Handle::current(),
        )
        .unwrap();

        let sum = client
            .add_host_async(SavedHost {
                id: "host-offline".into(),
                name: "Work Mac".into(),
                transport: HostTransport::Tcp {
                    addr: server_addr,
                    token: code,
                },
                ssh_target: None,
                server_id: Some(server1_id),
                enabled: true,
                added_at: now(),
            })
            .await
            .unwrap();
        assert_eq!(sum.id, "host-offline");
        let host_id = sum.id;

        let online_tl = client
            .call_routed_async(
                "timeline.list",
                json!({"view": "full", "card_id": card_a.id, "tail": true, "limit": 50}),
                Some(&project.id),
            )
            .await
            .unwrap();
        assert!(!online_tl["events"].as_array().unwrap().is_empty());

        drop(running1);
        drop(server1);
        client.poll_remotes_once_async().await;
        assert!(client.host_summaries()[0].is_offline());

        let offline_created = client
            .call_routed_async(
                "card.create",
                json!({
                    "project_id": project.id,
                    "title": "Offline Created Card",
                    "body": "Created on plane",
                }),
                Some(&project.id),
            )
            .await
            .unwrap();
        assert_eq!(offline_created["offline_queued"], true);
        let offline_created_id = offline_created["id"].as_str().unwrap().to_owned();

        let offline_comment = client
            .call_routed_async(
                "comment.add",
                json!({"card_id": card_a.id, "body": "Queued offline reply"}),
                Some(&project.id),
            )
            .await
            .unwrap();
        let offline_comment_id = offline_comment["id"].as_str().unwrap().to_owned();

        let offline_edit = client
            .call_routed_async(
                "card.update",
                json!({
                    "card_id": card_a.id,
                    "revision": card_a.revision,
                    "title": "Client Offline Title",
                }),
                Some(&project.id),
            )
            .await
            .unwrap();
        assert_eq!(offline_edit["offline_queued"], true);

        let resolve_preview = client
            .call_routed_async("card.resolve.preview", json!({"card_id": card_a.id}), None)
            .await
            .unwrap();
        assert!(resolve_preview.get("warnings").is_some());

        let offline_tl = client
            .call_routed_async(
                "timeline.list",
                json!({"view": "full", "card_id": card_a.id, "tail": true, "limit": 50}),
                Some(&project.id),
            )
            .await
            .unwrap();
        let evs = offline_tl["events"].as_array().unwrap();
        assert!(
            evs.iter().any(|e| {
                e["id"] == offline_comment_id
                    && e["data"]["pending"] == true
                    && e["body"] == "Queued offline reply"
            }),
            "cached timeline should include pending comment with body: {evs:?}"
        );
        assert!(
            evs.iter().any(|e| e["body"] == "Initial timeline note"),
            "cached timeline should preserve cached note: {evs:?}"
        );

        let proj_snap = client.merged_snapshot().unwrap();
        assert_eq!(proj_snap.cards.len(), 2);
        assert_eq!(
            proj_snap.pending_sync.get(&card_a.id),
            Some(&SyncMark::Pending)
        );
        assert_eq!(
            proj_snap.pending_sync.get(&offline_created_id),
            Some(&SyncMark::Pending)
        );

        let (server2, _running2, _) = start_tcp_server(server_home, port).await;
        server2
            .call(
                "card.update",
                json!({
                    "view": "full",
                    "card_id": card_a.id,
                    "revision": card_a.revision,
                    "title": "Server Concurrent Title",
                }),
            )
            .unwrap();

        client.reconnect(Some(&host_id));
        client.poll_remotes_once_async().await;

        let deadline = Instant::now() + Duration::from_secs(5);
        let conflict = loop {
            let items = client.sync_items();
            if let Some(c) = items.iter().find(|i| i.state == SyncItemState::Conflict)
                && items.len() == 1
            {
                break c.clone();
            }
            assert!(
                Instant::now() < deadline,
                "expected create+comment drained and card_a title conflict; items={items:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        };

        assert_eq!(conflict.card_id, card_a.id);
        assert_eq!(conflict.conflict_fields, vec!["title"]);
        assert_eq!(
            conflict.mine.get("title").map(String::as_str),
            Some("Client Offline Title")
        );
        assert_eq!(
            conflict.server.get("title").map(String::as_str),
            Some("Server Concurrent Title")
        );
        assert_eq!(
            client
                .merged_snapshot()
                .unwrap()
                .pending_sync
                .get(&card_a.id),
            Some(&SyncMark::Conflict)
        );

        client
            .sync_action(&host_id, &conflict.mutation_id, SyncAction::UseMine)
            .unwrap();
        let deadline2 = Instant::now() + Duration::from_secs(5);
        loop {
            if client.sync_items().is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline2,
                "expected conflict to drain after UseMine; items={:?}",
                client.sync_items()
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        let server_card_a: Card = serde_json::from_value(
            server2
                .call("card.get", json!({"view": "full", "card_id": card_a.id}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(server_card_a.title, "Client Offline Title");

        let server_created: Card = serde_json::from_value(
            server2
                .call(
                    "card.get",
                    json!({"view": "full", "card_id": offline_created_id}),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(server_created.title, "Offline Created Card");
    }

    #[test]
    fn merge_push_topic_preserves_durable_github_and_resync_precedence() {
        assert_eq!(merge_push_topic("terminals", "reclaim"), "reclaim");
        assert_eq!(merge_push_topic("reclaim", "terminals"), "terminals");
        assert_eq!(merge_push_topic("application", "terminals"), "application");
        assert_eq!(merge_push_topic("application", "reclaim"), "application");
        assert_eq!(merge_push_topic("terminals", "application"), "application");
        assert_eq!(merge_push_topic("github", "application"), "github");
        assert_eq!(merge_push_topic("terminals", "github"), "github");
        assert_eq!(merge_push_topic("github", "resync"), "resync");
        assert_eq!(merge_push_topic("resync", "terminals"), "resync");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_only_push_uses_runtime_state_and_updates_merged_runtime_state() {
        use canban_transport::{
            AcceptedStream, DeviceTokenStore, MAX_FRAME_BYTES, PairingCodeStore, RevocationBus,
            ServerTransportConfig, TailscaleSessionRegistry, read_bounded_line, spawn_tcp_listener,
            write_json_line_async,
        };
        use canban_types::{TerminalReclaimStatus, Workflow};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncBufReadExt, BufReader};

        let dir = tempfile::tempdir().unwrap();
        let server_home = dir.path().join("mock-server-home");
        let client_dir = dir.path().join("client-data");
        std::fs::create_dir_all(&server_home).unwrap();

        let store = Arc::new(DeviceTokenStore::open(&server_home));
        let (_dev, token) = store.issue_device("ios-runtime-test").unwrap();
        let pairing_codes = Arc::new(PairingCodeStore::open(&server_home));
        let revocations = RevocationBus::new();
        let active_tcp = Arc::new(AtomicUsize::new(0));
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let (push_tx, _) = broadcast::channel::<(String, Value)>(16);

        let app_state_calls = Arc::new(AtomicUsize::new(0));
        let runtime_state_calls = Arc::new(AtomicUsize::new(0));
        let subscribed_notify = Arc::new(Notify::new());

        let durable_project: Project = serde_json::from_value(json!({
            "id": "p-rt",
            "name": "Durable Project",
            "directory": "/srv/durable",
            "workflow": Workflow::default(),
            "created_at": 1000,
        }))
        .unwrap();
        let durable_card: Card = serde_json::from_value(json!({
            "id": "c-rt",
            "project_id": "p-rt",
            "number": 1,
            "title": "Durable Card",
            "body": "Must survive runtime refresh",
            "stage": "todo",
            "revision": 2,
            "attention": null,
            "archived_at": null,
            "archive": null,
            "archive_error": null,
            "worktree": null,
            "created_at": 1000,
        }))
        .unwrap();
        let mut initial_snap_typed = Snapshot::default();
        initial_snap_typed.projects.push(durable_project);
        initial_snap_typed.cards.push(durable_card);
        let initial_snap = serde_json::to_value(&initial_snap_typed).unwrap();

        let latest_runtime: Arc<Mutex<RuntimeState>> =
            Arc::new(Mutex::new(RuntimeState::default()));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let cfg = Arc::new(ServerTransportConfig {
            home: server_home,
            server_id: "srv-runtime-mock".into(),
            api_version: 85,
            device_store: store,
            pairing_codes,
            revocations,
            active_tcp_connections: active_tcp,
            tailscale_verifier: None,
            tailscale_devices: Arc::new(TailscaleSessionRegistry::new()),
        });

        let handle = tokio::runtime::Handle::current();
        let ptx = push_tx.clone();
        let app_calls = app_state_calls.clone();
        let rt_calls = runtime_state_calls.clone();
        let sub_notify = subscribed_notify.clone();
        let snap_ref = Arc::new(initial_snap);
        let rt_ref = latest_runtime.clone();
        spawn_tcp_listener(
            &handle,
            listener,
            cfg,
            stop_rx,
            move |accepted: AcceptedStream| {
                let mut prx = ptx.subscribe();
                let app_calls = app_calls.clone();
                let rt_calls = rt_calls.clone();
                let sub_notify = sub_notify.clone();
                let snap_ref = snap_ref.clone();
                let rt_ref = rt_ref.clone();
                async move {
                    let (read_half, mut writer) = tokio::io::split(accepted.stream);
                    let mut reader = BufReader::new(read_half);
                    let mut first_req = accepted.initial_request;
                    loop {
                        let req: Value = match first_req.take() {
                            Some(v) => v,
                            None => {
                                let Ok(Some(line)) =
                                    read_bounded_line(&mut reader, MAX_FRAME_BYTES).await
                                else {
                                    break;
                                };
                                serde_json::from_str(&line).unwrap()
                            }
                        };
                        let id = req["id"].clone();
                        match req["method"].as_str() {
                            Some("app.ping") => {
                                let _ = write_json_line_async(
                                    &mut writer,
                                    &json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "result": {
                                            "server_id": "srv-runtime-mock",
                                            "api_version": 85,
                                            "version": "test",
                                            "platform": "macos",
                                        }
                                    }),
                                )
                                .await;
                            }
                            Some("app.state") => {
                                app_calls.fetch_add(1, Ordering::SeqCst);
                                let _ = write_json_line_async(
                                    &mut writer,
                                    &json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "result": {
                                            "changed": true,
                                            "revision": ["srv-runtime-mock", "i1", 1, 1, 0],
                                            "github_revision": 0,
                                            "server_id": "srv-runtime-mock",
                                            "snapshot": *snap_ref,
                                            "config": Config::default(),
                                            "home": "/tmp/srv",
                                        }
                                    }),
                                )
                                .await;
                            }
                            Some("runtime.state") => {
                                rt_calls.fetch_add(1, Ordering::SeqCst);
                                let rt = rt_ref.lock().unwrap().clone();
                                let _ = write_json_line_async(
                                    &mut writer,
                                    &json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "result": {
                                            "server_id": "srv-runtime-mock",
                                            "runtime": rt,
                                        }
                                    }),
                                )
                                .await;
                            }
                            Some("app.subscribe") => {
                                let topics: Vec<String> = req["params"]["topics"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(|v| v.as_str().map(str::to_owned))
                                    .collect();
                                assert!(
                                    topics.iter().any(|t| t == "snapshot"),
                                    "remote subscriber must request 'snapshot' stream, got {topics:?}"
                                );
                                let _ = write_json_line_async(
                                    &mut writer,
                                    &json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "result": {
                                            "subscribed": true,
                                            "server_id": "srv-runtime-mock",
                                            "api_version": 85,
                                            "revision": ["srv-runtime-mock", "i1", 1, 1, 0],
                                            "changed": false,
                                            "github_revision": 0,
                                            "heartbeat_ms": 5000,
                                        }
                                    }),
                                )
                                .await;
                                sub_notify.notify_waiters();
                                let mut buf = String::new();
                                loop {
                                    tokio::select! {
                                        Ok((topic, rev)) = prx.recv() => {
                                            let frame = json!({
                                                "jsonrpc": "2.0",
                                                "method": "event",
                                                "params": {
                                                    "seq": 2,
                                                    "topic": topic,
                                                    "card_id": null,
                                                    "revision": rev,
                                                    "github_revision": 0,
                                                }
                                            });
                                            if write_json_line_async(&mut writer, &frame).await.is_err() {
                                                break;
                                            }
                                        }
                                        r = reader.read_line(&mut buf) => {
                                            if matches!(r, Ok(0) | Err(_)) {
                                                break;
                                            }
                                            buf.clear();
                                        }
                                    }
                                }
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            },
        );

        let creds = Arc::new(MemoryCredentialStore::new());
        let client = RemoteClient::with_runtime(
            RemoteClientConfig::new(client_dir),
            creds,
            tokio::runtime::Handle::current(),
        )
        .unwrap();
        let mut events = client.subscribe();

        let sum = client
            .add_host_async(SavedHost {
                id: "host-rt".into(),
                name: "Runtime Box".into(),
                transport: HostTransport::Tcp { addr, token },
                ssh_target: None,
                server_id: Some("srv-runtime-mock".into()),
                enabled: true,
                added_at: now(),
            })
            .await
            .unwrap();
        assert_eq!(sum.id, "host-rt");

        // Wait until live app.subscribe stream is active, then drain initial resync events.
        tokio::time::timeout(Duration::from_secs(3), subscribed_notify.notified())
            .await
            .expect("supervisor subscribed");
        while events.try_recv().is_ok() {}
        let baseline_app_calls = app_state_calls.load(Ordering::SeqCst);

        // Push a pure runtime "terminals" change (working agent + reclaim status).
        *latest_runtime.lock().unwrap() = RuntimeState {
            terminals: vec![
                serde_json::from_value::<TerminalEntry>(json!({
                    "id": "t-rt-1",
                    "conversation_id": null,
                    "number": 1,
                    "name": "Agent 1",
                    "origin_project_id": "p-rt",
                    "origin_card_id": "c-rt",
                    "agent_engine": null,
                    "runtime": null,
                    "agent": {
                        "project_id": "p-rt",
                        "card_id": "c-rt",
                        "number": 1,
                        "provider": "claude",
                        "state": "working",
                        "instruction": "Do work",
                        "cwd": "/srv/durable",
                        "terminal": "t-rt-1",
                        "started_at": 1750000000123_u64,
                    },
                    "created_at": 1001,
                }))
                .unwrap(),
            ],
            reclaim_status: vec![TerminalReclaimStatus {
                terminal_id: "t-rt-1".into(),
                paused: true,
                reclaiming_soon: false,
                auto_reclaiming: false,
                interactive_followup: false,
                deadline_at: None,
            }],
            terminal_hosts: BTreeMap::from([("t-rt-1".into(), "local".into())]),
            host_active_agents: BTreeMap::from([("local".into(), 1)]),
            ..Default::default()
        };
        let _ = push_tx.send((
            "terminals".into(),
            json!(["srv-runtime-mock", "i1", 2, 2, 0]),
        ));

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "timed out waiting for terminals push");
            if let Ok(Some(ClientEvent::Host {
                host_id,
                event: AppEvent::Changed { topic, .. },
            })) = tokio::time::timeout(remaining, events.recv()).await
            {
                assert_eq!(host_id, "host-rt");
                if topic == "terminals" {
                    break;
                }
            }
        }

        // Pure runtime refresh must NOT have called app.state again!
        assert_eq!(app_state_calls.load(Ordering::SeqCst), baseline_app_calls);
        assert!(runtime_state_calls.load(Ordering::SeqCst) >= 1);

        let rt = client.merged_runtime_state().unwrap();
        assert_eq!(rt.terminals.len(), 1);
        assert_eq!(rt.terminals[0].id, "t-rt-1");
        assert_eq!(
            rt.terminal_hosts.get("t-rt-1").map(String::as_str),
            Some("host-rt")
        );
        assert_eq!(rt.host_active_agents.get("host-rt").copied(), Some(1));
        assert_eq!(rt.reclaim_status.len(), 1);

        // Durable project and card must remain intact in merged_snapshot().
        let snap = client.merged_snapshot().unwrap();
        assert_eq!(snap.projects.len(), 1);
        assert_eq!(snap.cards.len(), 1);
        assert_eq!(snap.cards[0].id, "c-rt");

        let _ = stop_tx.send(true);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn supervisor_falls_back_to_polling_when_daemon_lacks_app_subscribe() {
        use canban_transport::{
            AcceptedStream, DeviceTokenStore, MAX_FRAME_BYTES, PairingCodeStore, RevocationBus,
            ServerTransportConfig, TailscaleSessionRegistry, read_bounded_line, spawn_tcp_listener,
            write_json_line_async,
        };
        use canban_types::Workflow;
        use std::sync::atomic::AtomicUsize;
        use tokio::io::BufReader;

        assert!(is_unsupported_subscribe_error(&anyhow!(
            "Unknown operation: app.subscribe"
        )));
        assert!(!is_unsupported_subscribe_error(&anyhow!(
            "auth_failed: bad token"
        )));

        let dir = tempfile::tempdir().unwrap();
        let server_home = dir.path().join("legacy-server");
        let client_dir = dir.path().join("legacy-client");
        std::fs::create_dir_all(&server_home).unwrap();

        let store = Arc::new(DeviceTokenStore::open(&server_home));
        let (_dev, token) = store.issue_device("ios-legacy-poll-test").unwrap();
        let pairing_codes = Arc::new(PairingCodeStore::open(&server_home));
        let revocations = RevocationBus::new();
        let active_tcp = Arc::new(AtomicUsize::new(0));
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let sub_rejected = Arc::new(Notify::new());

        let proj: Project = serde_json::from_value(json!({
            "id": "p-legacy",
            "name": "fiber",
            "directory": "/srv/fiber",
            "workflow": Workflow::default(),
            "created_at": 1000,
        }))
        .unwrap();
        let mut snap_typed = Snapshot::default();
        snap_typed.projects.push(proj);
        let snap_ref = Arc::new(serde_json::to_value(&snap_typed).unwrap());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let cfg = Arc::new(ServerTransportConfig {
            home: server_home,
            server_id: "srv-legacy-76".into(),
            api_version: 76,
            device_store: store,
            pairing_codes,
            revocations,
            active_tcp_connections: active_tcp,
            tailscale_verifier: None,
            tailscale_devices: Arc::new(TailscaleSessionRegistry::new()),
        });

        let handle = tokio::runtime::Handle::current();
        let rej_notify = sub_rejected.clone();
        spawn_tcp_listener(
            &handle,
            listener,
            cfg,
            stop_rx,
            move |accepted: AcceptedStream| {
                let rej_notify = rej_notify.clone();
                let snap_ref = snap_ref.clone();
                async move {
                    let (read_half, mut writer) = tokio::io::split(accepted.stream);
                    let mut reader = BufReader::new(read_half);
                    let mut first_req = accepted.initial_request;
                    loop {
                        let req: Value = match first_req.take() {
                            Some(v) => v,
                            None => {
                                let Ok(Some(line)) =
                                    read_bounded_line(&mut reader, MAX_FRAME_BYTES).await
                                else {
                                    break;
                                };
                                serde_json::from_str(&line).unwrap()
                            }
                        };
                        let id = req["id"].clone();
                        match req["method"].as_str() {
                            Some("app.ping") => {
                                let _ = write_json_line_async(
                                    &mut writer,
                                    &json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "result": {
                                            "server_id": "srv-legacy-76",
                                            "api_version": 76,
                                            "version": "20261122-1",
                                            "platform": "linux-x86_64",
                                        }
                                    }),
                                )
                                .await;
                            }
                            Some("app.state") => {
                                let _ = write_json_line_async(
                                    &mut writer,
                                    &json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "result": {
                                            "changed": true,
                                            "revision": 1,
                                            "github_revision": 0,
                                            "server_id": "srv-legacy-76",
                                            "snapshot": *snap_ref,
                                            "config": Config::default(),
                                            "home": "/srv/.canban",
                                        }
                                    }),
                                )
                                .await;
                            }
                            Some("app.subscribe") => {
                                let _ = write_json_line_async(
                                    &mut writer,
                                    &json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "error": {
                                            "code": -32000,
                                            "message": "Unknown operation: app.subscribe"
                                        }
                                    }),
                                )
                                .await;
                                rej_notify.notify_waiters();
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            },
        );

        let creds = Arc::new(MemoryCredentialStore::new());
        let client = RemoteClient::with_runtime(
            RemoteClientConfig::new(client_dir),
            creds,
            tokio::runtime::Handle::current(),
        )
        .unwrap();

        let sum = client
            .add_host_async(SavedHost {
                id: "host-legacy".into(),
                name: "Legacy Host".into(),
                transport: HostTransport::Tcp { addr, token },
                ssh_target: None,
                server_id: Some("srv-legacy-76".into()),
                enabled: true,
                added_at: now(),
            })
            .await
            .unwrap();
        assert_eq!(sum.id, "host-legacy");

        tokio::time::timeout(Duration::from_secs(3), sub_rejected.notified())
            .await
            .expect("supervisor attempted app.subscribe");
        tokio::time::sleep(Duration::from_millis(80)).await;

        let summaries = client.host_summaries();
        assert_eq!(summaries.len(), 1);
        assert!(
            matches!(
                summaries[0].connection,
                HostConnectionState::Connected {
                    api_version: 76,
                    ..
                }
            ),
            "expected host to remain Connected on legacy poll fallback, got {:?}",
            summaries[0].connection
        );
        let snap = client.merged_snapshot().unwrap();
        assert_eq!(snap.projects.len(), 1);
        assert_eq!(snap.projects[0].name, "fiber");

        let _ = stop_tx.send(true);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tailscale_whois_auto_auth_and_discover_canban_hosts_work_without_stored_device_token()
    {
        use canban_transport::{TailscalePeerIdentity, TailscalePeerVerifier};
        use canban_types::{EnvEntryMeta, EnvWriteEntry, PairedDevice, TAILSCALE_AUTH_SENTINEL};
        use std::collections::BTreeMap;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct ToggleTailscaleVerifier {
            allow: Arc<AtomicBool>,
        }

        impl TailscalePeerVerifier for ToggleTailscaleVerifier {
            fn verify_peer(
                &self,
                _peer_addr: std::net::SocketAddr,
            ) -> Option<TailscalePeerIdentity> {
                self.allow
                    .load(Ordering::SeqCst)
                    .then(|| TailscalePeerIdentity {
                        node_id: "nMobile01".into(),
                        device_name: "Pixel Test".into(),
                        login_name: "jjy@example.com".into(),
                    })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let home_a = dir.path().join("host-a");
        let home_b = dir.path().join("host-b");
        let client_dir = dir.path().join("mobile-client");
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();

        let allow = Arc::new(AtomicBool::new(true));
        let (server_a, _run_a, addr_a) = start_tcp_server_with_verifier(
            home_a.clone(),
            0,
            Some(Arc::new(ToggleTailscaleVerifier {
                allow: allow.clone(),
            })),
        )
        .await;
        let (server_b, _run_b, addr_b) = start_tcp_server_with_verifier(
            home_b.clone(),
            0,
            Some(Arc::new(ToggleTailscaleVerifier {
                allow: allow.clone(),
            })),
        )
        .await;

        // Register host B in host A's hosts.json (linked host discovery without requiring host Tailscale CLI).
        let b_ping = server_b.call("app.ping", json!({})).unwrap();
        let b_sid = b_ping["server_id"].as_str().unwrap().to_owned();
        let hosts_file = canban_types::HostsFile {
            hosts: vec![SavedHost {
                id: "host-b".into(),
                name: "Host B".into(),
                transport: HostTransport::Tcp {
                    addr: addr_b.clone(),
                    token: "dev_unused_on_a".into(),
                },
                ssh_target: None,
                server_id: Some(b_sid.clone()),
                enabled: true,
                added_at: 1,
            }],
        };
        std::fs::create_dir_all(home_a.join("config")).unwrap();
        std::fs::write(
            home_a.join("config/hosts.json"),
            serde_json::to_vec_pretty(&hosts_file).unwrap(),
        )
        .unwrap();

        let creds = Arc::new(MemoryCredentialStore::new());
        let client = RemoteClient::with_runtime(
            RemoteClientConfig::new(client_dir),
            creds.clone(),
            tokio::runtime::Handle::current(),
        )
        .unwrap();

        // Pair mobile directly with Host A using Tailscale sentinel (no pair_ code or dev_ token issued upfront).
        let invite_a = PairingInvite {
            addr: addr_a.clone(),
            security: TransportSecurity::Tailscale,
            server_id: None,
            host_name: Some("Host A".into()),
            secret: TAILSCALE_AUTH_SENTINEL.into(),
        };
        let sum_a = client
            .pair_with_invite(&invite_a, Some("Host A"))
            .await
            .expect("Tailscale WhoIs auto-auth must succeed for verified peer");
        assert!(sum_a.is_connected());
        assert_eq!(
            creds.get_token(&sum_a.id).unwrap().as_deref(),
            Some(TAILSCALE_AUTH_SENTINEL),
            "Tailscale connections must preserve ts_auto rather than persisting static device tokens"
        );

        // Ephemeral session token must seal TCP env.save secrets end-to-end.
        client
            .call_routed_async(
                "env.save",
                json!({
                    "host_id": sum_a.id,
                    "scope": "global",
                    "target_id": "",
                    "entries": BTreeMap::from([(
                        "TS_SECRET".to_owned(),
                        EnvWriteEntry::Secret {
                            value: Some("tailscale-sealed-secret".into()),
                            keep_existing: false,
                        },
                    )]),
                }),
                None,
            )
            .await
            .expect("env.save sealed with ephemeral ts_sess secret must succeed");
        let snap_a = server_a.snapshot().unwrap();
        assert!(matches!(
            snap_a.global_env.get("TS_SECRET"),
            Some(EnvEntryMeta::Secret {
                configured: true,
                ..
            })
        ));

        // device.list surfaces active Tailscale peer with auth_kind == "tailscale".
        let devs_val = server_a.call("device.list", json!({})).unwrap();
        let devs: Vec<PairedDevice> = serde_json::from_value(devs_val["devices"].clone()).unwrap();
        let ts_dev = devs
            .iter()
            .find(|d| d.id == "ts:nMobile01")
            .expect("Tailscale session listed in device.list");
        assert_eq!(ts_dev.auth_kind, "tailscale");

        // On-demand discovery via Host A finds Host B (and excludes Host A).
        let discovered = client
            .discover_canban_hosts(Some(&sum_a.id))
            .await
            .expect("discover_canban_hosts succeeds");
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].addr, addr_b);
        assert_eq!(discovered[0].name, "Host B");
        assert!(discovered[0].linked_on_host);

        // Mobile joins Host B directly using Tailscale sentinel too.
        let invite_b = PairingInvite {
            addr: discovered[0].addr.clone(),
            security: TransportSecurity::Tailscale,
            server_id: discovered[0].server_id.clone(),
            host_name: Some(discovered[0].name.clone()),
            secret: TAILSCALE_AUTH_SENTINEL.into(),
        };
        let sum_b = client
            .pair_with_invite(&invite_b, Some("Host B"))
            .await
            .unwrap();
        assert!(sum_b.is_connected());
        assert!(
            client
                .discover_canban_hosts(Some(&sum_a.id))
                .await
                .unwrap()
                .is_empty(),
            "already-paired Host B must be filtered out on subsequent discovery"
        );

        // Disallowing peer in Tailscale rejects new Tailscale auto-auth cleanly with auth_failed.
        allow.store(false, Ordering::SeqCst);
        let rej_err = client
            .pair_with_invite(
                &PairingInvite {
                    addr: addr_a,
                    security: TransportSecurity::Tailscale,
                    server_id: None,
                    host_name: Some("Host A Again".into()),
                    secret: TAILSCALE_AUTH_SENTINEL.into(),
                },
                None,
            )
            .await
            .unwrap_err();
        assert!(
            format!("{rej_err:#}").contains("auth_failed:"),
            "unverified Tailscale peer must fail with auth_failed, got {rej_err:#}"
        );
    }

    #[tokio::test]
    async fn multi_host_client_supports_custom_connector_and_drops_lease_on_disable() {
        use crate::connector::{
            CompositeConnector, ConnectFuture, ConnectedTransport, ConnectionLease, HostConnector,
        };
        use canban_transport::ClientEndpoint;
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let server_home = dir.path().join("local-server");
        let server =
            canban_app_server::AppServer::open(server_home.clone(), PathBuf::from("/bin/echo"))
                .unwrap();
        let _running = server.start().unwrap();
        let sock_path = server_home.join("app.sock");

        struct DropLease(Arc<AtomicBool>);
        impl Drop for DropLease {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        struct LocalSockConnector {
            sock: PathBuf,
            dropped: Arc<AtomicBool>,
        }
        impl HostConnector for LocalSockConnector {
            fn supports(&self, transport: &HostTransport) -> bool {
                matches!(transport, HostTransport::Local)
            }
            fn connect<'a>(
                &'a self,
                _host_id: &'a str,
                _transport: &'a HostTransport,
                _device_name: &'a str,
            ) -> ConnectFuture<'a> {
                let sock = self.sock.clone();
                let dropped = Arc::clone(&self.dropped);
                Box::pin(async move {
                    let tr = Arc::new(ClientTransport::connect_direct(ClientEndpoint::Unix(sock))?);
                    Ok(ConnectedTransport::with_lease(
                        tr,
                        ConnectionLease::from_guard(DropLease(dropped)),
                    ))
                })
            }
        }

        let lease_dropped = Arc::new(AtomicBool::new(false));
        let connector = Arc::new(CompositeConnector::with_default_tcp().push(Arc::new(
            LocalSockConnector {
                sock: sock_path,
                dropped: Arc::clone(&lease_dropped),
            },
        )));
        let client = MultiHostClient::with_runtime_and_connector(
            RemoteClientConfig::new(dir.path().join("client")),
            Arc::new(MemoryCredentialStore::new()),
            connector,
            ClientCapabilities::desktop(),
            tokio::runtime::Handle::current(),
        )
        .unwrap();
        assert_eq!(client.capabilities(), ClientCapabilities::desktop());
        assert!(client.supports_transport(&HostTransport::Local));

        let summary = client
            .add_host_async(SavedHost {
                id: "local".into(),
                name: "This Mac".into(),
                transport: HostTransport::Local,
                ssh_target: None,
                server_id: None,
                enabled: true,
                added_at: now(),
            })
            .await
            .unwrap();
        assert!(summary.is_local);
        assert!(summary.is_connected());
        assert!(!lease_dropped.load(Ordering::SeqCst));

        client.set_host_enabled("local", false).unwrap();
        assert!(
            lease_dropped.load(Ordering::SeqCst),
            "disabling host must drop its ConnectionLease"
        );
    }
}
