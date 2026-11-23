//! Pure-remote multi-host client (`RemoteClient`) driven by Tokio async push streams,
//! `canban-sync` outbox/read-cache replay, AEAD TCP secret sealing, and remote PTY transport.
use crate::{
    pairing::{PairingInvite, TransportSecurity},
    push::{PushFrame, change_topic, open_async_subscribe, state_marker},
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
    AppEvent, Config, GlobalEnvSyncSummary, HostConnectionState, HostSummary, HostTransport,
    SavedHost, Snapshot, SyncAction, SyncItemState, SyncItemView, is_device_token, is_pairing_code,
    normalize_daemon_addr, now,
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
const MAX_BACKOFF: Duration = Duration::from_secs(10);
const SUBSCRIBE_TOPICS: &[&str] = &[
    "application",
    "github",
    "review",
    "configuration",
    "desktop",
    "schedules",
    "terminals",
    "worktrees",
    "execution",
    "host",
    "resync",
    "usage",
    "utilities",
    "update",
];

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
        let active_agents = self
            .cached
            .snapshot
            .terminals
            .iter()
            .filter(|t| t.has_active_agent())
            .count();
        HostSummary {
            id: self.saved.id.clone(),
            name: self.saved.name.clone(),
            is_local: false,
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

    fn tcp_addr_and_token(&self) -> Option<(String, String)> {
        match &self.saved.transport {
            HostTransport::Tcp { addr, token } => Some((addr.clone(), token.clone())),
            _ => None,
        }
    }

    fn tcp_token(&self) -> Option<String> {
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
        }
        self.terminal = None;
        self.connection = next_state;
        self.conn_gen = self.conn_gen.wrapping_add(1);
    }
}

/// Shared pure-remote Canban client managing TCP connections, Tokio push streams,
/// offline read cache, and FIFO mutation replay across saved hosts.
pub struct RemoteClient {
    config: RemoteClientConfig,
    creds: Arc<dyn CredentialStore>,
    hosts: RwLock<BTreeMap<String, RemoteHostSlot>>,
    events: broadcast::Sender<ClientEvent>,
    shutdown_tx: watch::Sender<bool>,
    runtime_handle: tokio::runtime::Handle,
    owned_runtime: Option<tokio::runtime::Runtime>,
    self_weak: OnceLock<Weak<Self>>,
}

impl Drop for RemoteClient {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        let clients: Vec<Arc<ClientTransport>> = {
            let mut guard = self.hosts.write().unwrap();
            let mut out = Vec::new();
            for slot in guard.values_mut() {
                slot.terminal = None;
                if let Some(c) = slot.client.take() {
                    out.push(c);
                }
            }
            out
        };
        for client in clients {
            client.close();
        }
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

    if !was_connected {
        Some("resync")
    } else if changed {
        Some(change_topic(true, &prev_marker, &slot.last_marker))
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

impl RemoteClient {
    /// Creates a new [`RemoteClient`] owning a multi-thread Tokio runtime, loads saved hosts
    /// and caches from `config.data_dir`, and starts push supervisors for enabled hosts.
    pub fn new(config: RemoteClientConfig, creds: Arc<dyn CredentialStore>) -> Result<Arc<Self>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("canban-remote-client")
            .build()
            .context("Failed to build RemoteClient Tokio runtime")?;
        let handle = runtime.handle().clone();
        Self::build(config, creds, handle, Some(runtime))
    }

    /// Creates a new [`RemoteClient`] bound to an existing caller Tokio runtime handle.
    pub fn with_runtime(
        config: RemoteClientConfig,
        creds: Arc<dyn CredentialStore>,
        runtime_handle: tokio::runtime::Handle,
    ) -> Result<Arc<Self>> {
        Self::build(config, creds, runtime_handle, None)
    }

    fn build(
        config: RemoteClientConfig,
        creds: Arc<dyn CredentialStore>,
        runtime_handle: tokio::runtime::Handle,
        owned_runtime: Option<tokio::runtime::Runtime>,
    ) -> Result<Arc<Self>> {
        let saved = load_hosts(&config.data_dir, creds.as_ref());
        let mut slots = BTreeMap::new();
        for host in saved {
            if !matches!(host.transport, HostTransport::Tcp { .. }) {
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
        let (epoch, notify) = {
            let mut guard = self.hosts.write().unwrap();
            let Some(slot) = guard.get_mut(host_id) else {
                return;
            };
            slot.supervisor_epoch = slot.supervisor_epoch.wrapping_add(1);
            (slot.supervisor_epoch, Arc::clone(&slot.supervisor_notify))
        };
        notify.notify_one();
        let hid = host_id.to_owned();
        let shutdown_rx = self.shutdown_tx.subscribe();
        self.runtime_handle.spawn(async move {
            host_supervisor_task(Arc::downgrade(&this), hid, epoch, notify, shutdown_rx).await;
        });
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

        let start = Instant::now();
        let transport = Arc::new(
            ClientTransport::connect_tcp_async(
                addr.clone(),
                invite.secret.clone(),
                self.config.device_name.clone(),
            )
            .await?,
        );
        let rtt_ms = start.elapsed().as_millis() as u64;

        let token = transport
            .upgraded_token()
            .map(str::to_owned)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| invite.secret.clone());

        let probe_res = transport.call_async("host.probe", &json!({})).await;
        let state_res = transport
            .call_async("app.state", &json!({"after": Value::Null}))
            .await;
        let (probe_val, state_val) = match (probe_res, state_res) {
            (Ok(p), Ok(s)) => (p, s),
            (Err(e), _) | (_, Err(e)) => {
                transport.close();
                return Err(e);
            }
        };

        let server_id = state_val["server_id"]
            .as_str()
            .or_else(|| probe_val["server_id"].as_str())
            .or(invite.server_id.as_deref())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned);

        let host_name = display_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| invite.host_name.as_deref().filter(|s| !s.trim().is_empty()))
            .or_else(|| {
                probe_val["hostname"]
                    .as_str()
                    .or_else(|| state_val["hostname"].as_str())
                    .filter(|s| !s.trim().is_empty())
            })
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
                slot.client = Some(Arc::clone(&transport));
                apply_state_to_slot(
                    slot,
                    &self.config.data_dir,
                    &state_val,
                    Some(rtt_ms),
                    Some(&probe_val),
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
                slot.client = Some(Arc::clone(&transport));
                apply_state_to_slot(
                    &mut slot,
                    &self.config.data_dir,
                    &state_val,
                    Some(rtt_ms),
                    Some(&probe_val),
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

    /// Adds or updates a TCP host synchronously and connects immediately when `host.enabled`.
    pub fn add_host(&self, host: SavedHost) -> Result<HostSummary> {
        block_on_rt(&self.runtime_handle, self.add_host_async(host))
    }

    /// Adds or updates a TCP host asynchronously and connects immediately when `host.enabled`.
    pub async fn add_host_async(&self, mut host: SavedHost) -> Result<HostSummary> {
        let HostTransport::Tcp { addr, token } = &mut host.transport else {
            bail!("RemoteClient only supports TCP hosts");
        };
        *addr = normalize_daemon_addr(addr);
        ensure!(!addr.is_empty(), "Host address cannot be empty");
        ensure!(
            !token.trim().is_empty(),
            "TCP host requires a device token or pairing code"
        );
        if host.id.trim().is_empty() {
            host.id = canban_types::id();
        }
        if host.added_at == 0 {
            host.added_at = now();
        }

        if host.enabled {
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
        let guard = self.hosts.read().unwrap();
        let candidates = entity_id_candidates(method, params);
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
                    self.mark_slot_disconnected(&host_id, Some(client_gen), err.to_string());
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
                    self.mark_slot_disconnected(host_id, Some(client_gen), err.to_string());
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

    fn mark_slot_disconnected(&self, host_id: &str, expected_gen: Option<u64>, err_msg: String) {
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
            slot.supervisor_notify.notify_one();
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

    /// Fans out `method` across all enabled hosts concurrently.
    pub async fn call_all_hosts_async(
        &self,
        method: &str,
        params: Value,
    ) -> Vec<(String, Result<Value>)> {
        let targets: Vec<(String, Option<Arc<ClientTransport>>, Option<String>)> = {
            let guard = self.hosts.read().unwrap();
            guard
                .values()
                .filter(|s| s.saved.enabled)
                .map(|s| (s.saved.id.clone(), s.client.clone(), s.tcp_token()))
                .collect()
        };
        let mut out = Vec::with_capacity(targets.len());
        for (host_id, client_opt, token_opt) in targets {
            let res = match client_opt {
                Some(client) => {
                    let mut p = params.clone();
                    if let Err(e) = seal_params_for_tcp(method, &mut p, token_opt.as_deref()) {
                        Err(e)
                    } else {
                        client.call_async(method, &p).await
                    }
                }
                None => Err(anyhow!("Remote host '{host_id}' is offline")),
            };
            out.push((host_id, res));
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
        let (addr, token, server_id, cached_at, saved_at, last_marker) = {
            let guard = self.hosts.read().unwrap();
            let slot = guard
                .get(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            ensure!(slot.saved.enabled, "Host '{host_id}' is disabled");
            let (raw_addr, token) = slot
                .tcp_addr_and_token()
                .with_context(|| format!("Host '{host_id}' is not TCP"))?;
            (
                normalize_daemon_addr(raw_addr.as_str()),
                token,
                slot.saved.server_id.clone(),
                slot.cached_at(),
                slot.cached.saved_at,
                slot.last_marker.clone(),
            )
        };
        ensure!(!addr.is_empty(), "No TCP address configured for host");
        ensure!(
            !token.trim().is_empty(),
            "No device token found for host '{host_id}'"
        );

        let start = Instant::now();
        let (transport, rtt_ms) =
            match ClientTransport::connect_tcp_async(addr, token, self.config.device_name.clone())
                .await
            {
                Ok(client) => (Arc::new(client), start.elapsed().as_millis() as u64),
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

        let probe_res = transport.call_async("host.probe", &json!({})).await;
        let after = if saved_at == 0 {
            Value::Null
        } else {
            last_marker
        };
        let state_res = transport
            .call_async("app.state", &json!({"after": after}))
            .await;
        let (probe_val, state_val) = match (probe_res, state_res) {
            (Ok(p), Ok(s)) => (p, s),
            (Err(e), _) | (_, Err(e)) => {
                transport.close();
                return Err(e);
            }
        };

        if let Some(expected_sid) = server_id.as_deref()
            && let Some(actual_sid) = state_val["server_id"]
                .as_str()
                .or_else(|| probe_val["server_id"].as_str())
            && actual_sid != expected_sid
        {
            transport.close();
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

        let upgraded = transport
            .upgraded_token()
            .filter(|t| is_device_token(t))
            .map(str::to_owned);

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
            slot.client = Some(Arc::clone(&transport));
            apply_state_to_slot(
                slot,
                &self.config.data_dir,
                &state_val,
                Some(rtt_ms),
                Some(&probe_val),
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
        card_id: Option<String>,
    ) -> Result<()> {
        let (after, saved_at) = {
            let guard = self.hosts.read().unwrap();
            let slot = guard
                .get(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            (slot.last_marker.clone(), slot.cached.saved_at)
        };
        let req_after = if saved_at == 0 { Value::Null } else { after };
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
            apply_state_to_slot(slot, &self.config.data_dir, &state_val, Some(rtt_ms), None)
        };
        if let Some(topic) = emitted_topic {
            self.emit_host(host_id, AppEvent::Changed { topic, card_id });
        }
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
                    );
                    break;
                }
            }
        }

        if report.applied > 0 || report.conflicts > 0 || report.failed > 0 {
            if let Some(client) = active_client {
                let _ = self.refresh_host_now_async(host_id, &client, None).await;
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
                if let Err(err) = self.refresh_host_now_async(host_id, &c, None).await {
                    if is_connection_error(&err) {
                        self.mark_slot_disconnected(host_id, Some(client_gen), err.to_string());
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
                        backoff = Duration::from_millis(250);
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

        let sub_res =
            open_async_subscribe(&transport, &last_marker, SUBSCRIBE_TOPICS, Some(15_000)).await;
        let (mut stream, handshake) = match sub_res {
            Ok(res) => res,
            Err(err) => {
                rc.mark_slot_disconnected(&host_id, Some(conn_gen), err.to_string());
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
            let _ = rc.refresh_host_now_async(&host_id, &transport, None).await;
            let _ = rc.drain_outbox_async(&host_id).await;
        }

        let mut pending_card_id: Option<String> = None;
        let debounce = tokio::time::sleep(Duration::from_secs(86_400));
        tokio::pin!(debounce);
        let mut debounce_active = false;

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
                    let _ = rc.refresh_host_now_async(&host_id, &transport, None).await;
                    let _ = rc.drain_outbox_async(&host_id).await;
                }
                _ = &mut debounce, if debounce_active => {
                    debounce_active = false;
                    let cid = pending_card_id.take();
                    if let Err(err) = rc.refresh_host_now_async(&host_id, &transport, cid).await
                        && is_connection_error(&err)
                    {
                        rc.mark_slot_disconnected(&host_id, Some(conn_gen), err.to_string());
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
                            let same_marker = {
                                let guard = rc.hosts.read().unwrap();
                                guard.get(&host_id).is_some_and(|s| s.last_marker == marker)
                            };
                            if topic == "resync" || !same_marker {
                                if card_id.is_some() {
                                    pending_card_id = card_id;
                                }
                                if !debounce_active {
                                    debounce.as_mut().reset(tokio::time::Instant::now() + PUSH_DEBOUNCE);
                                    debounce_active = true;
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
                            rc.mark_slot_disconnected(&host_id, Some(conn_gen), "Server closed push subscription".into());
                            break;
                        }
                        Err(err) => {
                            rc.mark_slot_disconnected(&host_id, Some(conn_gen), err.to_string());
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

    async fn start_tcp_server(
        home: PathBuf,
        port: u16,
    ) -> (Arc<AppServer>, canban_app_server::Running, String) {
        let server = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
        let running = server
            .start_with_listen(Some(ServerListenConfig {
                tailscale: false,
                port,
                listen: Some(format!("127.0.0.1:{port}")),
                allow_insecure_bind: false,
            }))
            .unwrap();
        let addr = server
            .bound_tcp_addrs()
            .into_iter()
            .next()
            .expect("server bound TCP address");
        (server, running, addr)
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
        let probe = server.call("host.probe", json!({})).unwrap();
        let server_id = probe["server_id"].as_str().unwrap().to_owned();

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
        let server1_id = server1.call("host.probe", json!({})).unwrap()["server_id"]
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
}
