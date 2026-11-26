//! Multi-host client (`MultiHostClient`, aliased as `RemoteClient`) driven by pluggable
//! [`HostConnector`] adapters, Tokio async push streams, `canban-sync` outbox/read-cache replay,
//! AEAD TCP secret sealing, and remote PTY transport.
use crate::{
    capabilities::{ClientCapabilities, GlobalEnvSyncProvider, LocalHostDriver},
    connector::{ConnectionLease, HostConnector, TcpConnector},
    pairing::{PairingInvite, TransportSecurity},
    push::{PushFrame, change_topic, open_async_subscribe, state_marker},
    session::{ApiMismatchError, SingleHostSession},
    store::{
        CachedHostData, CredentialHostStore, CredentialStore, HostStore, load_cache,
        load_global_env_sync_fingerprint, outbox_path, reads_dir, save_cache_with_durability,
        save_global_env_sync_fingerprint_with_durability, seal_params_for_tcp_with_id_gen,
    },
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use canban_context::FsDurability;
use canban_sync::{
    Outbox, QueueDecision, ReadCache,
    outbox::{Mutation, MutationKind, MutationState, UpdateBase},
    replay::{
        DrainReport, FinishOutcome, MAX_REVISION_REBUILDS_PER_DRAIN, StepAction, finish_step,
        prepare_next_step,
    },
};
use canban_terminal::{RemotePtyTransport, session::PtyTransport};
use canban_transport::ClientTransport;
use canban_types::{
    AppEvent, Clock, Config, DiscoveredCanbanHost, EnvEntryMeta, Failpoints,
    GlobalEnvSyncHostFailure, GlobalEnvSyncStatus, GlobalEnvSyncSummary, HostConnectionState,
    HostSummary, HostTransport, IdGen, RuntimeState, SavedHost, SchedulerSettings, Snapshot,
    SyncAction, SyncItemState, SyncItemView, is_device_token, is_pairing_code,
    normalize_daemon_addr, system_clock, system_id_gen,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, RwLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, broadcast, mpsc, watch};

const DEFAULT_READ_CACHE_BUDGET: u64 = 20 * 1024 * 1024;
const PUSH_DEBOUNCE: Duration = Duration::from_millis(30);
const PUSH_MAX_DEBOUNCE: Duration = Duration::from_millis(100);
const PUSH_REFRESH_RETRY_BASE_MS: u64 = 200;
const PUSH_REFRESH_RETRY_MAX_MS: u64 = 2_000;
const PUSH_REFRESH_MAX_NON_STORAGE_RETRIES: u32 = 8;
const MAX_BACKOFF: Duration = Duration::from_secs(10);
const API_MISMATCH_RETRY_DELAY: Duration = Duration::from_secs(30);
const LEGACY_POLL_INTERVAL: Duration = Duration::from_secs(5);
const SUBSCRIBE_TOPICS: &[&str] = &["snapshot", "update", "utilities", "usage", "herdr"];

struct AtomicFlagGuard(Arc<AtomicBool>);

impl Drop for AtomicFlagGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn is_unsupported_subscribe_error(err: &anyhow::Error) -> bool {
    let msg = format!("{err:#}");
    msg.contains("Unknown operation") && msg.contains("app.subscribe")
}

fn count_active_agents(s: &Snapshot, now_ms: u64) -> usize {
    canban_scheduler::active_count_at(s, now_ms)
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

/// Configuration options for constructing a [`MultiHostClient`] with custom storage, connectors,
/// capabilities, optional built-in `"local"` host, optional global-env replication provider, and
/// optional minimum remote API version.
pub struct MultiHostClientOptions {
    pub config: RemoteClientConfig,
    pub host_store: Arc<dyn HostStore>,
    pub connector: Arc<dyn HostConnector>,
    pub capabilities: ClientCapabilities,
    pub local_host: Option<Arc<dyn LocalHostDriver>>,
    pub global_env_provider: Option<Arc<dyn GlobalEnvSyncProvider>>,
    pub min_api_version: Option<u32>,
    pub clock: Arc<dyn Clock>,
    pub fs_durability: FsDurability,
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
    updating: bool,
    cached: CachedHostData,
    cache_gen: u64,
    local_cache_gen: u64,
    home: String,
    notice: Option<String>,
    reconnect_count: u32,
    last_synced_at: Option<u64>,
    last_marker: Value,
    last_runtime_marker: Value,
    next_retry_at: u64,
    conn_gen: u64,
    supervisor_epoch: u64,
    supervisor_notify: Arc<Notify>,
    draining: Arc<tokio::sync::Mutex<()>>,
    draining_active: Arc<AtomicBool>,
    prefetching: Arc<AtomicBool>,
    outbox: Arc<Mutex<Outbox>>,
    reads: Arc<Mutex<ReadCache>>,
    global_env_synced_fingerprint: Option<String>,
}

impl RemoteHostSlot {
    fn from_saved(
        saved: SavedHost,
        data_dir: &Path,
        read_cache_budget: u64,
        fs_durability: FsDurability,
        failpoints: Option<Arc<Failpoints>>,
    ) -> Self {
        let mut cached = load_cache(data_dir, &saved.id).unwrap_or(CachedHostData {
            saved_at: 0,
            snapshot: Snapshot::default(),
            config: Config::default(),
        });
        if cached.saved_at > 0 {
            cached.snapshot.register_host_entities(&saved.id);
        }
        let cached_at = (cached.saved_at > 0).then_some(cached.saved_at);
        let initial_conn = if saved.enabled {
            HostConnectionState::Connecting
        } else {
            HostConnectionState::Offline {
                error: "Disabled".to_owned(),
                cached_at,
            }
        };
        let outbox = Outbox::load_with_durability_and_failpoints(
            outbox_path(data_dir, &saved.id),
            fs_durability,
            failpoints,
        );
        let reads = ReadCache::open_with_durability(
            reads_dir(data_dir, &saved.id),
            read_cache_budget,
            fs_durability,
        );
        let global_env_synced_fingerprint = load_global_env_sync_fingerprint(data_dir, &saved.id);
        Self {
            saved,
            client: None,
            lease: ConnectionLease::none(),
            terminal: None,
            connection: initial_conn,
            updating: false,
            cached,
            cache_gen: 0,
            local_cache_gen: 0,
            home: String::new(),
            notice: None,
            reconnect_count: 0,
            last_synced_at: cached_at,
            last_marker: Value::Null,
            last_runtime_marker: Value::Null,
            next_retry_at: 0,
            conn_gen: 0,
            supervisor_epoch: 0,
            supervisor_notify: Arc::new(Notify::new()),
            draining: Arc::new(tokio::sync::Mutex::new(())),
            draining_active: Arc::new(AtomicBool::new(false)),
            prefetching: Arc::new(AtomicBool::new(false)),
            outbox: Arc::new(Mutex::new(outbox)),
            reads: Arc::new(Mutex::new(reads)),
            global_env_synced_fingerprint,
        }
    }

    fn summary(&self, now_ms: u64) -> HostSummary {
        let outbox = self.outbox.lock().unwrap();
        let active_agents = count_active_agents(&self.cached.snapshot, now_ms);
        HostSummary {
            id: self.saved.id.clone(),
            name: self.saved.name.clone(),
            is_local: matches!(self.saved.transport, HostTransport::Local)
                || self.saved.id == "local",
            transport_label: self.saved.transport.label().to_owned(),
            endpoint_label: self.saved.transport.endpoint_display(),
            ssh_target: self
                .saved
                .ssh_target
                .clone()
                .or_else(|| match &self.saved.transport {
                    HostTransport::Ssh { target, .. } => Some(target.clone()),
                    _ => None,
                }),
            server_id: self.saved.server_id.clone(),
            connection: self.connection.clone(),
            active_agents,
            max_agents: self.cached.config.scheduler.max_agents,
            project_count: self.cached.snapshot.projects.len(),
            reconnect_count: self.reconnect_count,
            last_synced_at: self.last_synced_at.or_else(|| self.cached_at()),
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

/// Shared multi-host Canban client managing pluggable transport connections, optional built-in
/// `"local"` primary host, Tokio push streams, offline read cache, and FIFO mutation replay across
/// saved hosts.
pub struct MultiHostClient {
    config: RemoteClientConfig,
    host_store: Arc<dyn HostStore>,
    connector: Arc<dyn HostConnector>,
    capabilities: ClientCapabilities,
    local_host: Option<Arc<dyn LocalHostDriver>>,
    local_summary_counts: Mutex<Option<(usize, usize, u64)>>,
    global_env_provider: Option<Arc<dyn GlobalEnvSyncProvider>>,
    min_api_version: Option<u32>,
    clock: Arc<dyn Clock>,
    id_gen: Arc<dyn IdGen>,
    fs_durability: FsDurability,
    failpoints: RwLock<Option<Arc<Failpoints>>>,
    hosts: RwLock<BTreeMap<String, RemoteHostSlot>>,
    entity_hosts: RwLock<BTreeMap<String, String>>,
    acknowledged_owners: Mutex<BTreeMap<String, String>>,
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

fn is_connection_error(err: &anyhow::Error) -> bool {
    if err.chain().any(|cause| {
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
    }) {
        return true;
    }
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

/// Identifies transient server-side SQLite lock contention or I/O faults that should
/// defer and retry queued outbox mutations rather than permanently failing the FIFO queue.
fn is_transient_storage_error(err: &anyhow::Error) -> bool {
    let lower = format!("{err:#}").to_ascii_lowercase();
    lower.contains("sqlite_busy")
        || lower.contains("sqlite_locked")
        || lower.contains("database is locked")
        || lower.contains("database file is locked")
        || lower.contains("database table is locked")
        || lower.contains("database schema is locked")
        || lower.contains("sqlite_ioerr")
        || lower.contains("disk i/o error")
}

/// Pure bounded exponential backoff (100ms..=2000ms) for deferred outbox retries after
/// transient server storage errors.
fn transient_storage_retry_delay_ms(attempts: u32) -> u64 {
    let shift = attempts.saturating_sub(1).min(5);
    100u64.saturating_mul(1u64 << shift).min(2_000)
}

/// Pure bounded exponential backoff (200ms..=2000ms) for push-supervisor refresh retries
/// after transient non-connection, non-auth errors.
fn push_refresh_retry_delay_ms(attempt: u32) -> u64 {
    let shift = attempt.saturating_sub(1).min(4);
    PUSH_REFRESH_RETRY_BASE_MS
        .saturating_mul(1u64 << shift)
        .min(PUSH_REFRESH_RETRY_MAX_MS)
}

fn auth_failed_error(host_name: &str, error: &str) -> anyhow::Error {
    let trimmed = error.trim();
    if trimmed.to_ascii_lowercase().contains("auth_failed") {
        anyhow!("Host '{host_name}': {trimmed}")
    } else if trimmed.is_empty() {
        anyhow!("auth_failed: Host '{host_name}' requires re-authentication")
    } else {
        anyhow!("auth_failed: Host '{host_name}': {trimmed}")
    }
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
        m if m.starts_with("terminal.") => &[
            "terminal_id",
            "inherit_terminal_id",
            "card_id",
            "project_id",
            "worktree_id",
        ],
        m if m.starts_with("card.") || m.starts_with("timeline.") || m.starts_with("comment.") => {
            &["card_id", "project_id", "comment_id", "id"]
        }
        m if m.starts_with("project.") => &["project_id", "id"],
        m if m.starts_with("workflow.") => &[
            "run_id",
            "source_id",
            "workflow_id",
            "revision_id",
            "project_id",
            "card_id",
        ],
        m if m.starts_with("schedule.") => &["schedule_id", "schedule_run_id", "project_id"],
        m if m.starts_with("configuration.") => &["session_id", "project_id"],
        m if m.starts_with("merge_batch.") => &["batch_id", "project_id"],
        _ => &[
            "card_id",
            "project_id",
            "schedule_id",
            "schedule_run_id",
            "run_id",
            "source_id",
            "workflow_id",
            "revision_id",
            "terminal_id",
            "worktree_id",
            "inherit_terminal_id",
            "session_id",
            "batch_id",
            "execution_id",
            "comment_id",
            "notification_id",
            "target_id",
            "id",
        ],
    };
    keys.iter()
        .filter_map(|k| {
            params
                .get(*k)
                .and_then(Value::as_str)
                .map(str::trim)
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

fn extract_card_id_for_mutation(params: &Value, id_gen: &dyn IdGen) -> String {
    params["card_id"]
        .as_str()
        .or_else(|| params["id"].as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| id_gen.next_id())
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StateCursor<'a> {
    identity: &'a str,
    state_instance: &'a str,
    changes: u64,
    state_revision: u64,
}

fn parse_state_cursor(revision: &Value) -> Option<StateCursor<'_>> {
    let arr = revision.as_array()?;
    if arr.len() < 4 {
        return None;
    }
    let identity = arr[0].as_str().filter(|s| !s.is_empty())?;
    let state_instance = arr[1].as_str().filter(|s| !s.is_empty())?;
    let changes = arr[2].as_u64()?;
    let state_revision = arr[3].as_u64()?;
    Some(StateCursor {
        identity,
        state_instance,
        changes,
        state_revision,
    })
}

fn cursor_changed_instance(current_revision: &Value, incoming_revision: &Value) -> bool {
    let (Some(cur), Some(inc)) = (
        parse_state_cursor(current_revision),
        parse_state_cursor(incoming_revision),
    ) else {
        return false;
    };
    cur.identity != inc.identity || cur.state_instance != inc.state_instance
}

/// Returns `true` when `incoming_revision` is strictly older than `current_revision` on the
/// same `(identity, state_instance)` (dominated in both `changes` and `state_revision`).
fn cursor_is_strictly_older(current_revision: &Value, incoming_revision: &Value) -> bool {
    let (Some(cur), Some(inc)) = (
        parse_state_cursor(current_revision),
        parse_state_cursor(incoming_revision),
    ) else {
        return false;
    };
    cur.identity == inc.identity
        && cur.state_instance == inc.state_instance
        && inc.changes <= cur.changes
        && inc.state_revision <= cur.state_revision
        && (inc.changes < cur.changes || inc.state_revision < cur.state_revision)
}

/// Returns `true` when `current_revision` already matches or strictly supersedes
/// `target_revision` on the same server instance.
fn cursor_already_covered_by(target_revision: &Value, current_revision: &Value) -> bool {
    !target_revision.is_null()
        && (*current_revision == *target_revision
            || cursor_is_strictly_older(current_revision, target_revision))
}

fn snapshot_regresses_card_revision(current: &Snapshot, incoming: &Snapshot) -> bool {
    if current.cards.is_empty() || incoming.cards.is_empty() {
        return false;
    }
    let current_revs: std::collections::HashMap<&str, u64> = current
        .cards
        .iter()
        .map(|c| (c.id.as_str(), c.revision))
        .collect();
    incoming.cards.iter().any(|inc| {
        current_revs
            .get(inc.id.as_str())
            .is_some_and(|&cur_rev| inc.revision < cur_rev)
    })
}

fn preserve_monotonic_card_revisions(current: &Snapshot, incoming: &mut Snapshot) {
    if current.cards.is_empty() || incoming.cards.is_empty() {
        return;
    }
    let current_cards: std::collections::HashMap<&str, &canban_types::Card> =
        current.cards.iter().map(|c| (c.id.as_str(), c)).collect();
    for card in &mut incoming.cards {
        if let Some(cur) = current_cards.get(card.id.as_str())
            && card.revision < cur.revision
        {
            *card = (*cur).clone();
        }
    }
}

/// Returns `true` when an incoming `app.state` response is stale relative to the slot's
/// current cursor and cached snapshot on the same `(identity, state_instance)`.
///
/// Always returns `false` when `identity` or `state_instance` differs (server restarted or
/// re-paired) or when either cursor is uninitialized/legacy so fresh post-restart snapshots
/// are never rejected.
fn is_stale_snapshot_cursor(
    current_revision: &Value,
    current_snapshot: &Snapshot,
    incoming_revision: &Value,
    incoming_snapshot: Option<&Snapshot>,
) -> bool {
    let (Some(cur), Some(inc)) = (
        parse_state_cursor(current_revision),
        parse_state_cursor(incoming_revision),
    ) else {
        return false;
    };
    if cur.identity != inc.identity || cur.state_instance != inc.state_instance {
        return false;
    }
    if inc.changes < cur.changes || inc.state_revision < cur.state_revision {
        return true;
    }
    if let Some(inc_snap) = incoming_snapshot
        && snapshot_regresses_card_revision(current_snapshot, inc_snap)
    {
        return true;
    }
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ApplyStateOutcome {
    emitted_topic: Option<&'static str>,
    stale_rejected: bool,
    needs_resync: bool,
}

#[allow(clippy::too_many_arguments)]
fn apply_state_to_slot(
    slot: &mut RemoteHostSlot,
    data_dir: &Path,
    state_val: &Value,
    rtt_ms: Option<u64>,
    probe_val: Option<&Value>,
    preferred_topic: Option<&'static str>,
    now_ms: u64,
    fs_durability: FsDurability,
) -> ApplyStateOutcome {
    let changed = state_val["changed"] == true || state_val.get("snapshot").is_some();
    let prev_marker = slot.last_marker.clone();
    let next_marker = state_marker(state_val);
    let was_connected = matches!(slot.connection, HostConnectionState::Connected { .. });
    let incoming_snap = if changed {
        parse_refreshed_snapshot(state_val, &slot.saved.id)
    } else {
        None
    };
    let stale = is_stale_snapshot_cursor(
        &slot.last_marker["revision"],
        &slot.cached.snapshot,
        &next_marker["revision"],
        incoming_snap.as_ref(),
    );

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
    if !stale {
        if let Some(notice) = state_val["notice"].as_str() {
            slot.notice = (!notice.trim().is_empty()).then(|| notice.to_owned());
        } else if state_val.get("notice").is_some_and(Value::is_null) {
            slot.notice = None;
        }

        if changed {
            if let Some(snap) = incoming_snap {
                slot.cached.snapshot = snap;
            }
            if let Ok(cfg) = serde_json::from_value::<Config>(state_val["config"].clone()) {
                slot.cached.config = cfg;
            }
            slot.cache_gen = slot.cache_gen.wrapping_add(1);
            slot.cached.saved_at = now_ms;
            save_cache_with_durability(data_dir, &slot.saved.id, &slot.cached, fs_durability);
        }
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

    slot.last_synced_at = Some(now_ms);
    if !stale && !next_marker["revision"].is_null() {
        slot.last_marker = next_marker.clone();
        slot.last_runtime_marker = next_marker.clone();
    }

    let emitted_topic = if !was_connected || preferred_topic == Some("resync") {
        Some("resync")
    } else if changed && !stale {
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
    };
    let needs_resync =
        stale && !cursor_is_strictly_older(&slot.last_marker["revision"], &next_marker["revision"]);
    ApplyStateOutcome {
        emitted_topic,
        stale_rejected: stale,
        needs_resync,
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
        let host_store = Arc::new(CredentialHostStore::new(config.data_dir.clone(), creds));
        Self::from_options(MultiHostClientOptions {
            config,
            host_store,
            connector,
            capabilities,
            local_host: None,
            global_env_provider: None,
            min_api_version: None,
            clock: system_clock(),
            fs_durability: FsDurability::Full,
        })
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
        let host_store = Arc::new(CredentialHostStore::new(config.data_dir.clone(), creds));
        Self::from_options_with_runtime(
            MultiHostClientOptions {
                config,
                host_store,
                connector,
                capabilities,
                local_host: None,
                global_env_provider: None,
                min_api_version: None,
                clock: system_clock(),
                fs_durability: FsDurability::Full,
            },
            runtime_handle,
        )
    }

    /// Creates a new [`MultiHostClient`] owning a multi-thread Tokio runtime from full options
    /// (used by Desktop to inject a local host driver, `config/hosts.json` store, SSH connector,
    /// global-env replication provider, and minimum API version).
    pub fn from_options(options: MultiHostClientOptions) -> Result<Arc<Self>> {
        Self::from_options_with_id_gen(options, system_id_gen())
    }

    /// Creates a new [`MultiHostClient`] owning a multi-thread Tokio runtime from full options
    /// with a custom [`IdGen`] for deterministic client-side IDs, request keys, and AEAD nonces.
    pub fn from_options_with_id_gen(
        options: MultiHostClientOptions,
        id_gen: Arc<dyn IdGen>,
    ) -> Result<Arc<Self>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("canban-client")
            .build()
            .context("Failed to build MultiHostClient Tokio runtime")?;
        let handle = runtime.handle().clone();
        Self::build(options, handle, Some(runtime), id_gen, None)
    }

    /// Creates a new [`MultiHostClient`] bound to an existing Tokio runtime from full options.
    pub fn from_options_with_runtime(
        options: MultiHostClientOptions,
        runtime_handle: tokio::runtime::Handle,
    ) -> Result<Arc<Self>> {
        Self::from_options_with_runtime_and_id_gen(options, runtime_handle, system_id_gen())
    }

    /// Creates a new [`MultiHostClient`] bound to an existing Tokio runtime from full options
    /// with a custom [`IdGen`] (used in DST to inject `DeterministicIdGen`).
    pub fn from_options_with_runtime_and_id_gen(
        options: MultiHostClientOptions,
        runtime_handle: tokio::runtime::Handle,
        id_gen: Arc<dyn IdGen>,
    ) -> Result<Arc<Self>> {
        Self::build(options, runtime_handle, None, id_gen, None)
    }

    /// Creates a new [`MultiHostClient`] bound to an existing Tokio runtime from full options
    /// with a custom [`IdGen`] and optional per-instance [`Failpoints`] registry (used in DST
    /// for outbox mid-write truncation crash windows).
    pub fn from_options_with_runtime_id_gen_and_failpoints(
        options: MultiHostClientOptions,
        runtime_handle: tokio::runtime::Handle,
        id_gen: Arc<dyn IdGen>,
        failpoints: Option<Arc<Failpoints>>,
    ) -> Result<Arc<Self>> {
        Self::build(options, runtime_handle, None, id_gen, failpoints)
    }

    /// Updates the optional per-instance [`Failpoints`] registry on this client and propagates
    /// it to every active remote host outbox.
    pub fn set_failpoints(&self, failpoints: Option<Arc<Failpoints>>) {
        *self.failpoints.write().unwrap() = failpoints.clone();
        let guard = self.hosts.read().unwrap();
        for slot in guard.values() {
            slot.outbox
                .lock()
                .unwrap()
                .set_failpoints(failpoints.clone());
        }
    }

    /// Returns the optional per-instance [`Failpoints`] registry attached to this client.
    pub fn failpoints(&self) -> Option<Arc<Failpoints>> {
        self.failpoints.read().unwrap().clone()
    }

    fn build(
        options: MultiHostClientOptions,
        runtime_handle: tokio::runtime::Handle,
        owned_runtime: Option<tokio::runtime::Runtime>,
        id_gen: Arc<dyn IdGen>,
        failpoints: Option<Arc<Failpoints>>,
    ) -> Result<Arc<Self>> {
        let _rt_guard = runtime_handle.enter();
        let MultiHostClientOptions {
            config,
            host_store,
            connector,
            capabilities,
            local_host,
            global_env_provider,
            min_api_version,
            clock,
            fs_durability,
        } = options;
        let saved = host_store.load_hosts();
        let mut slots = BTreeMap::new();
        let mut initial_entity_hosts = BTreeMap::new();
        for host in saved {
            if !connector.supports(&host.transport) {
                continue;
            }
            let slot = RemoteHostSlot::from_saved(
                host,
                &config.data_dir,
                config.read_cache_budget_bytes,
                fs_durability,
                failpoints.clone(),
            );
            if slot.cached.saved_at > 0 {
                for (k, v) in &slot.cached.snapshot.entity_hosts {
                    initial_entity_hosts.insert(k.clone(), v.clone());
                }
            }
            slots.insert(slot.saved.id.clone(), slot);
        }

        let (events, _) = broadcast::channel(512);
        let (shutdown_tx, _) = watch::channel(false);
        let client = Arc::new(Self {
            config,
            host_store,
            connector,
            capabilities,
            local_host,
            local_summary_counts: Mutex::new(None),
            global_env_provider,
            min_api_version,
            clock,
            id_gen,
            fs_durability,
            failpoints: RwLock::new(failpoints),
            hosts: RwLock::new(slots),
            entity_hosts: RwLock::new(initial_entity_hosts),
            acknowledged_owners: Mutex::new(BTreeMap::new()),
            events,
            shutdown_tx,
            runtime_handle,
            owned_runtime,
            self_weak: OnceLock::new(),
        });
        let _ = client.self_weak.set(Arc::downgrade(&client));

        if let Some(local) = client.local_host.clone() {
            let weak = Arc::downgrade(&client);
            let shutdown_rx = client.shutdown_tx.subscribe();
            client.runtime_handle.spawn(async move {
                local_push_supervisor(weak, local, shutdown_rx).await;
            });
        }

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

    fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    /// Returns `true` when this client's configured [`HostConnector`] supports `transport`.
    pub fn supports_transport(&self, transport: &HostTransport) -> bool {
        self.connector.supports(transport)
    }

    /// Returns the local daemon home directory when backed by a built-in `"local"` host.
    pub fn local_home(&self) -> Option<PathBuf> {
        self.local_host.as_ref().map(|l| l.home().to_path_buf())
    }

    /// Returns `true` when this client manages a built-in `"local"` primary host.
    pub fn has_local_host(&self) -> bool {
        self.local_host.is_some()
    }

    /// Sends an RPC directly to the built-in `"local"` primary host (or falls back to
    /// [`Self::call_routed`] when running pure-remote without a local daemon).
    pub fn call_local(&self, method: &str, params: Value) -> Result<Value> {
        if let Some(local) = &self.local_host {
            local.call(method, params)
        } else {
            self.call_routed(method, params, None)
        }
    }

    /// Returns the active underlying [`ClientTransport`] for `host_id` (including `"local"`).
    pub fn transport_for_host(&self, host_id: &str) -> Result<Arc<ClientTransport>> {
        if (host_id.is_empty() || host_id == "local")
            && let Some(local) = &self.local_host
        {
            return Ok(local.transport());
        }
        let guard = self.hosts.read().unwrap();
        let slot = guard
            .get(host_id)
            .with_context(|| format!("Unknown Canban host '{host_id}'"))?;
        if let Some(client) = &slot.client {
            return Ok(Arc::clone(client));
        }
        if let HostConnectionState::AuthFailed { error } = &slot.connection {
            return Err(auth_failed_error(&slot.saved.name, error));
        }
        bail!(
            "Host '{}' is currently unavailable ({})",
            slot.saved.name,
            slot.connection.short_status()
        )
    }

    fn slot_auth_failed_error(&self, host_id: &str) -> Option<anyhow::Error> {
        let guard = self.hosts.read().unwrap();
        guard.get(host_id).and_then(|slot| match &slot.connection {
            HostConnectionState::AuthFailed { error } if slot.client.is_none() => {
                Some(auth_failed_error(&slot.saved.name, error))
            }
            _ => None,
        })
    }

    /// Returns `true` when `host_id` is currently waiting for a scheduled retry deadline
    /// (for example, the 30-second backoff after an `ApiMismatch` probe).
    pub fn is_retry_backed_off(&self, host_id: &str) -> bool {
        let now_ms = self.now_ms();
        self.hosts
            .read()
            .unwrap()
            .get(host_id)
            .is_some_and(|s| s.next_retry_at > now_ms)
    }

    /// Clears any scheduled retry backoff for `host_id` so the next probe runs immediately.
    pub fn clear_retry_backoff(&self, host_id: &str) {
        if let Some(slot) = self.hosts.write().unwrap().get_mut(host_id) {
            slot.next_retry_at = 0;
        }
    }

    fn local_summary(&self, local: &dyn LocalHostDriver) -> HostSummary {
        let local_cfg = local.config();
        let (local_active, local_projects, last_synced_at) = self
            .local_summary_counts
            .lock()
            .unwrap()
            .map(|(active, projects, synced_at)| (active, projects, Some(synced_at)))
            .unwrap_or((0, 0, None));
        HostSummary {
            id: "local".into(),
            name: "This Mac".into(),
            is_local: true,
            transport_label: "Local socket".into(),
            endpoint_label: local.endpoint_label(),
            ssh_target: None,
            server_id: local.server_id(),
            connection: HostConnectionState::Connected {
                rtt_ms: 0,
                api_version: local.api_version(),
                version: local.version(),
                platform: local.platform(),
            },
            active_agents: local_active,
            max_agents: local_cfg.scheduler.max_agents,
            project_count: local_projects,
            reconnect_count: 0,
            last_synced_at,
            pending_mutations: 0,
            sync_conflicts: 0,
        }
    }

    fn replace_host_entity_owners(&self, host_id: &str, snap_entities: &BTreeMap<String, String>) {
        let mut eh = self.entity_hosts.write().unwrap();
        eh.retain(|k, v| v != host_id || snap_entities.contains_key(k));
        for (k, v) in snap_entities {
            match eh.get(k) {
                Some(existing) if existing != v => {
                    eh.insert(canban_types::qualify_host_id(host_id, k), v.clone());
                }
                _ => {
                    eh.insert(k.clone(), v.clone());
                }
            }
        }
    }

    fn qualify_remote_response(&self, host_id: &str, val: &mut Value) {
        let eh = self.entity_hosts.read().unwrap();
        canban_types::qualify_remote_value_ids(val, host_id, &eh);
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
        self.host_store.save_hosts(&saved)
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
        let weak = Arc::downgrade(&this);
        let hid = host_id.to_owned();
        let shutdown_rx = self.shutdown_tx.subscribe();
        self.runtime_handle.spawn(async move {
            host_supervisor_task(weak, hid, epoch, notify, shutdown_rx).await;
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
        let notifies: Vec<Arc<Notify>> = {
            let mut guard = self.hosts.write().unwrap();
            let mut list = Vec::new();
            for slot in guard.values_mut() {
                if !slot.saved.enabled || slot.updating {
                    continue;
                }
                if matches!(slot.connection, HostConnectionState::AuthFailed { .. }) {
                    let is_tailscale = matches!(
                        &slot.saved.transport,
                        HostTransport::Tcp { token, .. } if canban_types::is_tailscale_auth(token)
                    );
                    if !is_tailscale {
                        continue;
                    }
                }
                slot.next_retry_at = 0;
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

    /// Wakes reconnect/resync loops for `host_id` (or all enabled hosts when `None`).
    pub fn reconnect(&self, host_id: Option<&str>) {
        let (notifies, changed_ids): (Vec<Arc<Notify>>, Vec<String>) = {
            let mut guard = self.hosts.write().unwrap();
            let mut list = Vec::new();
            let mut changed = Vec::new();
            for (id, slot) in guard.iter_mut() {
                if host_id.is_some_and(|target| target != id) {
                    continue;
                }
                if !slot.saved.enabled || slot.updating {
                    continue;
                }
                slot.next_retry_at = 0;
                if slot.client.is_some() {
                    slot.clear_connection(HostConnectionState::Connecting);
                    changed.push(id.clone());
                } else if !matches!(slot.connection, HostConnectionState::Connecting) {
                    slot.connection = HostConnectionState::Connecting;
                    changed.push(id.clone());
                }
                list.push(Arc::clone(&slot.supervisor_notify));
            }
            (list, changed)
        };
        for id in changed_ids {
            self.emit_host(
                &id,
                AppEvent::Changed {
                    topic: "host",
                    card_id: None,
                },
            );
        }
        for notify in notifies {
            notify.notify_one();
        }
    }

    /// Wakes supervisor(s) without tearing down healthy connections or retrying `AuthFailed` hosts.
    pub fn wake_host(&self, host_id: &str) {
        if let Some(notify) = self
            .hosts
            .read()
            .unwrap()
            .get(host_id)
            .filter(|s| !matches!(s.connection, HostConnectionState::AuthFailed { .. }))
            .map(|s| Arc::clone(&s.supervisor_notify))
        {
            notify.notify_one();
        }
    }

    /// Wakes all remote host supervisors without tearing down healthy connections or retrying `AuthFailed` hosts.
    pub fn wake_all(&self) {
        let notifies: Vec<Arc<Notify>> = self
            .hosts
            .read()
            .unwrap()
            .values()
            .filter(|s| !matches!(s.connection, HostConnectionState::AuthFailed { .. }))
            .map(|s| Arc::clone(&s.supervisor_notify))
            .collect();
        for n in notifies {
            n.notify_one();
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
        let now_ms = self.now_ms();
        let host_ids: Vec<String> = {
            let guard = self.hosts.read().unwrap();
            guard
                .values()
                .filter(|s| {
                    s.saved.enabled
                        && !s.updating
                        && !matches!(s.connection, HostConnectionState::AuthFailed { .. })
                        && (s.client.is_some() || now_ms >= s.next_retry_at)
                })
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
        self.pair_internal(None, invite, display_name, None, false)
            .await
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
        ssh_target: Option<String>,
        reject_duplicate_server_id: bool,
    ) -> Result<HostSummary> {
        let addr = normalize_daemon_addr(&invite.addr);
        ensure!(
            !addr.is_empty(),
            "Pairing invite does not contain an address"
        );
        let check_id = preferred_id.as_deref().unwrap_or("");
        self.validate_remote_server_id(
            check_id,
            invite.server_id.as_deref(),
            reject_duplicate_server_id,
        )?;

        let tcp_transport = HostTransport::Tcp {
            addr: addr.clone(),
            token: invite.secret.clone(),
        };
        let session_res = SingleHostSession::connect_checked_async_with_clock(
            self.connector.as_ref(),
            preferred_id.as_deref().unwrap_or("pairing"),
            &tcp_transport,
            &self.config.device_name,
            Value::Null,
            self.min_api_version,
            self.clock.as_ref(),
        )
        .await;
        let session = match session_res {
            Ok(session) => session,
            Err(err) => {
                if let Some(mismatch) = err.downcast_ref::<ApiMismatchError>() {
                    let hid = preferred_id
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| self.id_gen.next_id());
                    let server_id = mismatch
                        .server_id
                        .clone()
                        .or_else(|| invite.server_id.clone());
                    self.validate_remote_server_id(
                        &hid,
                        server_id.as_deref(),
                        reject_duplicate_server_id,
                    )?;
                    let host_name = display_name
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .or_else(|| invite.host_name.as_deref().filter(|s| !s.trim().is_empty()))
                        .unwrap_or("Remote Host")
                        .to_owned();
                    let saved = SavedHost {
                        id: hid.clone(),
                        name: host_name,
                        transport: tcp_transport,
                        ssh_target,
                        server_id,
                        enabled: true,
                        added_at: self.now_ms(),
                    };
                    let mut slot = RemoteHostSlot::from_saved(
                        saved,
                        &self.config.data_dir,
                        self.config.read_cache_budget_bytes,
                        self.fs_durability,
                        self.failpoints(),
                    );
                    slot.connection = HostConnectionState::ApiMismatch {
                        server_api: mismatch.server_api,
                        app_api: mismatch.app_api,
                        version: mismatch.version.clone(),
                    };
                    let now_ms = self.now_ms();
                    slot.next_retry_at =
                        now_ms.saturating_add(API_MISMATCH_RETRY_DELAY.as_millis() as u64);
                    let summary = slot.summary(now_ms);
                    {
                        let mut guard = self.hosts.write().unwrap();
                        guard.insert(hid.clone(), slot);
                        self.persist_locked(&guard)?;
                    }
                    self.spawn_supervisor(&hid);
                    self.emit_host(
                        &hid,
                        AppEvent::Changed {
                            topic: "host",
                            card_id: None,
                        },
                    );
                    return Ok(summary);
                }
                return Err(err);
            }
        };
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

        let check_id = preferred_id.as_deref().unwrap_or("");
        if let Err(err) = self.validate_remote_server_id(
            check_id,
            server_id.as_deref(),
            reject_duplicate_server_id,
        ) {
            transport.close();
            drop(lease);
            return Err(err);
        }

        let host_name = display_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| invite.host_name.as_deref().filter(|s| !s.trim().is_empty()))
            .or_else(|| hs.hostname.as_deref().filter(|s| !s.trim().is_empty()))
            .unwrap_or("Remote Host")
            .to_owned();

        let (host_id, summary, updated_entities) = {
            let mut guard = self.hosts.write().unwrap();
            let existing_id = if reject_duplicate_server_id {
                preferred_id
                    .as_deref()
                    .filter(|id| guard.contains_key(*id))
                    .map(str::to_owned)
            } else {
                server_id
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
                    })
            };

            if let Some(hid) = existing_id {
                let slot = guard.get_mut(&hid).expect("existing host slot");
                // Invalidate any in-flight supervisor dial before replacing credentials
                // so a stale auth/network error cannot tear down this new session.
                slot.supervisor_epoch = slot.supervisor_epoch.wrapping_add(1);
                slot.clear_connection(HostConnectionState::Connecting);
                slot.next_retry_at = 0;
                slot.saved.enabled = true;
                slot.saved.transport = HostTransport::Tcp { addr, token };
                if let Some(target) = ssh_target {
                    slot.saved.ssh_target = Some(target);
                }
                if let Some(name) = display_name.map(str::trim).filter(|s| !s.is_empty()) {
                    slot.saved.name = name.to_owned();
                }
                slot.terminal = Some(Arc::new(RemotePtyTransport::new(
                    Arc::clone(&transport),
                    self.runtime_handle.clone(),
                )));
                slot.lease = lease;
                slot.client = Some(Arc::clone(&transport));
                let now_ms = self.now_ms();
                apply_state_to_slot(
                    slot,
                    &self.config.data_dir,
                    &state_val,
                    Some(rtt_ms),
                    Some(&ping_val),
                    None,
                    now_ms,
                    self.fs_durability,
                );
                let entities = slot.cached.snapshot.entity_hosts.clone();
                let summary = slot.summary(now_ms);
                self.persist_locked(&guard)?;
                (hid, summary, entities)
            } else {
                let hid = preferred_id
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| self.id_gen.next_id());
                let saved = SavedHost {
                    id: hid.clone(),
                    name: host_name,
                    transport: HostTransport::Tcp { addr, token },
                    ssh_target,
                    server_id,
                    enabled: true,
                    added_at: self.now_ms(),
                };
                let mut slot = RemoteHostSlot::from_saved(
                    saved,
                    &self.config.data_dir,
                    self.config.read_cache_budget_bytes,
                    self.fs_durability,
                    self.failpoints(),
                );
                slot.terminal = Some(Arc::new(RemotePtyTransport::new(
                    Arc::clone(&transport),
                    self.runtime_handle.clone(),
                )));
                slot.lease = lease;
                slot.client = Some(Arc::clone(&transport));
                let now_ms = self.now_ms();
                apply_state_to_slot(
                    &mut slot,
                    &self.config.data_dir,
                    &state_val,
                    Some(rtt_ms),
                    Some(&ping_val),
                    None,
                    now_ms,
                    self.fs_durability,
                );
                let entities = slot.cached.snapshot.entity_hosts.clone();
                let summary = slot.summary(now_ms);
                guard.insert(hid.clone(), slot);
                self.persist_locked(&guard)?;
                (hid, summary, entities)
            }
        };

        if !updated_entities.is_empty() {
            self.replace_host_entity_owners(&host_id, &updated_entities);
        }
        self.spawn_supervisor(&host_id);
        self.emit_host(
            &host_id,
            AppEvent::Changed {
                topic: "resync",
                card_id: None,
            },
        );
        Ok(summary)
    }

    fn validate_remote_server_id(
        &self,
        host_id: &str,
        server_id: Option<&str>,
        reject_duplicate: bool,
    ) -> Result<()> {
        let Some(sid) = server_id.filter(|s| !s.trim().is_empty()) else {
            return Ok(());
        };
        if let Some(local) = &self.local_host
            && local.server_id().as_deref() == Some(sid)
        {
            bail!(
                "That address points to This Mac's own Canban service ({sid}), which is already connected locally."
            );
        }
        if reject_duplicate {
            let guard = self.hosts.read().unwrap();
            if let Some(dup) = guard
                .values()
                .find(|s| s.saved.id != host_id && s.saved.server_id.as_deref() == Some(sid))
            {
                bail!(
                    "That Canban daemon is already connected under '{}'.",
                    dup.saved.name
                );
            }
        }
        Ok(())
    }

    /// Adds or updates a host synchronously and connects immediately when `host.enabled`.
    pub fn add_host(&self, host: SavedHost) -> Result<HostSummary> {
        block_on_rt(&self.runtime_handle, self.add_host_async(host))
    }

    /// Adds or updates a host asynchronously and connects immediately when `host.enabled`.
    pub async fn add_host_async(&self, mut host: SavedHost) -> Result<HostSummary> {
        if self.local_host.is_some() {
            ensure!(host.id != "local", "Invalid remote host ID");
        }
        ensure!(
            self.connector.supports(&host.transport),
            "Transport {} is not supported by this client",
            host.transport.label()
        );
        if let HostTransport::Tcp { addr, token } = &mut host.transport {
            *addr = normalize_daemon_addr(addr);
            *token = token.trim().to_owned();
            ensure!(!addr.is_empty(), "Host address cannot be empty");
            ensure!(
                !token.is_empty(),
                "TCP host requires a device token or pairing code"
            );
        }
        if host.id.trim().is_empty() {
            host.id = self.id_gen.next_id();
        }
        if host.added_at == 0 {
            host.added_at = self.now_ms();
        }

        let reject_dup = self.local_host.is_some();
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
            return self
                .pair_internal(Some(host.id), &invite, display, host.ssh_target, reject_dup)
                .await;
        }

        if host.enabled {
            let session = SingleHostSession::connect_checked_async_with_clock(
                self.connector.as_ref(),
                &host.id,
                &host.transport,
                &self.config.device_name,
                Value::Null,
                self.min_api_version,
                self.clock.as_ref(),
            )
            .await?;
            let (transport, lease, hs) = session.into_parts();
            if host.server_id.is_none() {
                host.server_id = hs.server_id;
            }
            if let Err(err) =
                self.validate_remote_server_id(&host.id, host.server_id.as_deref(), reject_dup)
            {
                transport.close();
                drop(lease);
                return Err(err);
            }
            if host.name.trim().is_empty() {
                host.name = hs.hostname.unwrap_or_else(|| host.id.clone());
            }
            let hid = host.id.clone();
            let (summary, entities) = {
                let mut guard = self.hosts.write().unwrap();
                let mut slot = RemoteHostSlot::from_saved(
                    host,
                    &self.config.data_dir,
                    self.config.read_cache_budget_bytes,
                    self.fs_durability,
                    self.failpoints(),
                );
                slot.terminal = Some(Arc::new(RemotePtyTransport::new(
                    Arc::clone(&transport),
                    self.runtime_handle.clone(),
                )));
                slot.lease = lease;
                slot.client = Some(Arc::clone(&transport));
                let now_ms = self.now_ms();
                apply_state_to_slot(
                    &mut slot,
                    &self.config.data_dir,
                    &hs.state,
                    Some(hs.rtt_ms),
                    Some(&hs.ping),
                    None,
                    now_ms,
                    self.fs_durability,
                );
                let entities = slot.cached.snapshot.entity_hosts.clone();
                let sum = slot.summary(now_ms);
                guard.insert(hid.clone(), slot);
                self.persist_locked(&guard)?;
                (sum, entities)
            };
            if !entities.is_empty() {
                self.replace_host_entity_owners(&hid, &entities);
            }
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
                self.fs_durability,
                self.failpoints(),
            );
            let sum = slot.summary(self.now_ms());
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
        if self.local_host.is_some() {
            ensure!(host_id != "local", "Cannot remove This Mac");
        }
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
        self.host_store.delete_credentials(host_id);
        let _ = std::fs::remove_file(crate::store::cache_path(&self.config.data_dir, host_id));
        let _ = std::fs::remove_file(outbox_path(&self.config.data_dir, host_id));
        let _ =
            std::fs::remove_dir_all(crate::store::host_cache_dir(&self.config.data_dir, host_id));
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
        if self.local_host.is_some() {
            ensure!(host_id != "local", "Cannot rename This Mac");
        }
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
        if self.local_host.is_some() {
            ensure!(host_id != "local", "Cannot disable This Mac");
        }
        let (spawn_needed, notify) = {
            let mut guard = self.hosts.write().unwrap();
            let slot = guard
                .get_mut(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            let was_enabled = slot.saved.enabled;
            slot.saved.enabled = enabled;
            let (spawn, n) = if enabled {
                if !slot.updating {
                    slot.next_retry_at = 0;
                    if !was_enabled && slot.client.is_none() {
                        slot.connection = HostConnectionState::Connecting;
                    }
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
        let mut out = Vec::new();
        if let Some(local) = &self.local_host {
            out.push(self.local_summary(local.as_ref()));
        }
        let now_ms = self.now_ms();
        let guard = self.hosts.read().unwrap();
        out.extend(guard.values().map(|s| s.summary(now_ms)));
        out
    }

    pub fn config_for_host(&self, host_id: &str) -> Config {
        if (host_id.is_empty() || host_id == "local")
            && let Some(local) = &self.local_host
        {
            return local.config();
        }
        let guard = self.hosts.read().unwrap();
        if let Some(slot) = guard.get(host_id) {
            let mut cfg = slot.cached.config.clone();
            if let Some(client) = &slot.client {
                let (relay_endpoint, relay_token) = client.relay_endpoint_and_token();
                cfg.relay_endpoint = Some(relay_endpoint);
                cfg.relay_token = relay_token;
            } else if self.local_host.is_some() {
                cfg.relay_endpoint = Some(format!("mux://offline:{host_id}"));
                cfg.relay_token = None;
            }
            return cfg;
        }
        if let Some(local) = &self.local_host {
            let mut cfg = local.config();
            cfg.relay_endpoint = Some(format!("mux://offline:{host_id}"));
            cfg.relay_token = None;
            return cfg;
        }
        Config::default()
    }

    pub fn home_for(&self, host_id: &str) -> String {
        if (host_id.is_empty() || host_id == "local")
            && let Some(local) = &self.local_host
        {
            return local.home().to_string_lossy().into_owned();
        }
        let guard = self.hosts.read().unwrap();
        if let Some(slot) = guard.get(host_id)
            && !slot.home.is_empty()
        {
            return slot.home.clone();
        }
        if self.local_host.is_some() {
            "~/.canban".into()
        } else {
            String::new()
        }
    }

    pub fn server_notice(&self) -> Option<String> {
        if let Some(notice) = self.local_host.as_ref().and_then(|l| l.server_notice()) {
            return Some(notice);
        }
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

    /// Merges the built-in `"local"` host (when present) and all enabled (plus mobile offline-cached)
    /// remote hosts with optimistic `canban-sync` projections, refreshing the O(1) routing table.
    pub fn merged_snapshot(&self) -> Result<Snapshot> {
        let now_ms = self.now_ms();
        let mut merged = if let Some(local) = &self.local_host {
            let mut combined = local.snapshot()?;
            let local_active = count_active_agents(&combined, now_ms);
            let local_projects = combined.projects.len();
            *self.local_summary_counts.lock().unwrap() =
                Some((local_active, local_projects, now_ms));
            combined.register_host_entities("local");
            combined
                .host_summaries
                .insert("local".into(), self.local_summary(local.as_ref()));
            combined
        } else {
            Snapshot::default()
        };

        let remote_snapshots: Vec<(Option<Snapshot>, HostSummary)> = {
            let guard = self.hosts.read().unwrap();
            guard
                .values()
                .filter(|slot| {
                    if self.local_host.is_some() {
                        slot.saved.enabled
                    } else {
                        slot.saved.enabled || slot.cached.saved_at > 0
                    }
                })
                .map(|slot| {
                    let ops = slot.outbox.lock().unwrap().ops.clone();
                    let snap = if slot.cached.saved_at > 0
                        || !slot.cached.snapshot.projects.is_empty()
                        || !slot.cached.snapshot.cards.is_empty()
                    {
                        let mut s = slot.cached.snapshot.clone();
                        if !ops.is_empty() {
                            canban_sync::apply(&mut s, &slot.saved.id, &ops);
                        }
                        canban_notifications::reconcile_runtime_reminders_at(&mut s, now_ms);
                        s.register_host_entities(&slot.saved.id);
                        Some(s)
                    } else if !ops.is_empty() {
                        let mut s = Snapshot::default();
                        canban_sync::apply(&mut s, &slot.saved.id, &ops);
                        s.register_host_entities(&slot.saved.id);
                        Some(s)
                    } else {
                        None
                    };
                    (snap, slot.summary(now_ms))
                })
                .collect()
        };

        for (snap_opt, summary) in remote_snapshots {
            if let Some(remote_snap) = snap_opt {
                merged.merge_remote_snapshot(remote_snap, summary);
            } else {
                merged.host_summaries.insert(summary.id.clone(), summary);
            }
        }

        self.acknowledged_owners.lock().unwrap().retain(|id, host| {
            if merged.entity_hosts.contains_key(id) || !merged.host_summaries.contains_key(host) {
                return false;
            }
            merged.entity_hosts.insert(id.clone(), host.clone());
            true
        });
        *self.entity_hosts.write().unwrap() = merged.entity_hosts.clone();
        Ok(merged)
    }

    /// Merges the lightweight runtime state (`terminals`, `terminal_contexts`, `reclaim_status`,
    /// `merging_cards`) across `"local"` (when present) and enabled remote hosts.
    pub fn merged_runtime_state(&self) -> Result<RuntimeState> {
        use std::collections::HashSet;

        let now_ms = self.now_ms();
        if let Some(local) = &self.local_host {
            let mut combined = local.runtime_state()?;
            let local_active = combined
                .host_active_agents
                .get("local")
                .copied()
                .unwrap_or(0);
            {
                let mut counts = self.local_summary_counts.lock().unwrap();
                let prev_projects = counts.map(|(_, p, _)| p).unwrap_or(0);
                *counts = Some((local_active, prev_projects, now_ms));
            }
            combined.register_host("local");

            let remote_states: Vec<(String, RuntimeState)> = {
                let guard = self.hosts.read().unwrap();
                guard
                    .values()
                    .filter(|slot| slot.saved.enabled)
                    .filter_map(|slot| {
                        let ops = slot.outbox.lock().unwrap().ops.clone();
                        let snap = if slot.cached.saved_at > 0
                            || !slot.cached.snapshot.projects.is_empty()
                            || !slot.cached.snapshot.cards.is_empty()
                        {
                            let mut s = slot.cached.snapshot.clone();
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
                        }?;
                        let active_agents = count_active_agents(&snap, now_ms);
                        let rt = RuntimeState {
                            terminals: snap.terminals,
                            terminal_contexts: snap.terminal_contexts,
                            reclaim_status: snap.reclaim_status,
                            merging_cards: snap.merging_cards,
                            terminal_hosts: BTreeMap::new(),
                            host_active_agents: BTreeMap::from([(
                                slot.saved.id.clone(),
                                active_agents,
                            )]),
                        };
                        Some((slot.saved.id.clone(), rt))
                    })
                    .collect()
            };

            let entity_owners = self.entity_hosts.read().unwrap().clone();
            for (host_id, remote_rt) in remote_states {
                combined.merge_remote_with_owners(remote_rt, &host_id, &entity_owners);
            }

            let acknowledged: Vec<(String, String)> = self
                .acknowledged_owners
                .lock()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let mut routing = self.entity_hosts.write().unwrap();
            for (tid, host) in &combined.terminal_hosts {
                routing.insert(tid.clone(), host.clone());
            }
            for (id, host) in acknowledged {
                routing.entry(id).or_insert(host);
            }
            return Ok(combined);
        }

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
            let active_agents = count_active_agents(&host_snap, now_ms);
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

    /// Resolves which host (`"local"` or a saved remote host ID) owns the target entity/project.
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
        if self.local_host.is_some() {
            if method == "env.save" {
                return match params["scope"].as_str().unwrap_or("") {
                    "global" => "local".into(),
                    "project" => {
                        let target_id = params["target_id"]
                            .as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty());
                        if let Some(tid) = target_id
                            && let Some((h, _)) = canban_types::split_qualified_host_id(tid)
                        {
                            return h.to_owned();
                        }
                        if let Some(pid) = active_project_id.filter(|s| !s.is_empty())
                            && let Some((h, _)) = canban_types::split_qualified_host_id(pid)
                            && target_id.is_none()
                        {
                            return h.to_owned();
                        }
                        let map = self.entity_hosts.read().unwrap();
                        target_id
                            .and_then(|id| map.get(id).cloned())
                            .or_else(|| {
                                active_project_id
                                    .filter(|s| !s.is_empty())
                                    .and_then(|pid| map.get(pid).cloned())
                            })
                            .unwrap_or_else(|| "local".into())
                    }
                    _ => "local".into(),
                };
            }
            if method.starts_with("utilities.")
                || method.starts_with("update.")
                || method.starts_with("workflow.default.")
                || method == "host.discover"
                || method == "host.discover_canban"
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
        {
            let map = self.entity_hosts.read().unwrap();
            for candidate in &candidates {
                if let Some(h) = map.get(*candidate) {
                    return h.clone();
                }
            }
            if let Some(pid) = active_project_id.filter(|s| !s.is_empty())
                && let Some(h) = map.get(pid)
            {
                return h.clone();
            }
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
        if self.local_host.is_some() {
            return "local".into();
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
        if let Some(local) = &self.local_host {
            if method == "env.save" && params["scope"].as_str() == Some("global") {
                if let Some(obj) = params.as_object_mut() {
                    obj.remove("host_id");
                }
                let res = local.call(method, params)?;
                self.emit_host(
                    "local",
                    AppEvent::Changed {
                        topic: "configuration",
                        card_id: None,
                    },
                );
                self.wake_all();
                return Ok(res);
            }
            if method == "notifications.read_all"
                && params["host_id"].is_null()
                && params["project_id"]
                    .as_str()
                    .is_none_or(|s| s.trim().is_empty())
            {
                let local_res = local.call(method, params.clone())?;
                let remote_clients: Vec<(String, Arc<ClientTransport>)> = {
                    let guard = self.hosts.read().unwrap();
                    guard
                        .iter()
                        .filter_map(|(id, slot)| Some((id.clone(), slot.client.clone()?)))
                        .collect()
                };
                for (hid, client) in remote_clients {
                    if client.call_async(method, &params).await.is_ok() {
                        let _ = self.refresh_host_now_async(&hid, &client, None, None).await;
                    }
                }
                return Ok(local_res);
            }
        }

        let host_id = self.resolve_target_host(method, &params, active_project_id);
        ensure!(!host_id.is_empty(), "No remote Canban host configured");
        if let Some(obj) = params.as_object_mut() {
            obj.remove("host_id");
        }
        if host_id == "local"
            && let Some(local) = &self.local_host
        {
            return local.call(method, params);
        }
        canban_types::strip_qualified_host_ids(&mut params, &host_id);
        if let Some(auth_err) = self.slot_auth_failed_error(&host_id) {
            return Err(auth_err);
        }

        if method == "card.resolve.preview" {
            return self.handle_resolve_preview_async(&host_id, &params).await;
        }

        if matches!(method, "card.get" | "card.show") {
            let requested_card_id = params["card_id"].as_str().unwrap_or("");
            if !requested_card_id.is_empty()
                && self.has_pending_create(&host_id, requested_card_id)
                && let Some(mut val) = self.projected_card_get(&host_id, &params)
            {
                self.qualify_remote_response(&host_id, &mut val);
                return Ok(val);
            }
            let (client_opt, client_gen) = {
                let guard = self.hosts.read().unwrap();
                let slot = guard
                    .get(&host_id)
                    .with_context(|| format!("Unknown host '{host_id}'"))?;
                (slot.client.clone(), slot.conn_gen)
            };
            if let Some(client) = client_opt {
                match client.call_async(method, &params).await {
                    Ok(mut val) => {
                        if params.get("view").and_then(Value::as_str) == Some("full")
                            && !requested_card_id.is_empty()
                            && self.has_pending_card_ops(&host_id, requested_card_id)
                            && let Some(projected) = self.projected_card_get(&host_id, &params)
                        {
                            val = projected;
                        }
                        self.qualify_remote_response(&host_id, &mut val);
                        return Ok(val);
                    }
                    Err(err) if is_auth_error(&err) => {
                        self.mark_slot_disconnected(
                            &host_id,
                            Some(client_gen),
                            err.to_string(),
                            false,
                        );
                        return Err(err);
                    }
                    Err(err) if is_connection_error(&err) => {
                        self.mark_slot_disconnected(
                            &host_id,
                            Some(client_gen),
                            err.to_string(),
                            true,
                        );
                        if let Some(mut val) = self.projected_card_get(&host_id, &params) {
                            self.qualify_remote_response(&host_id, &mut val);
                            return Ok(val);
                        }
                        return Err(err);
                    }
                    Err(err) => return Err(err),
                }
            }
            if let Some(mut val) = self.projected_card_get(&host_id, &params) {
                self.qualify_remote_response(&host_id, &mut val);
                return Ok(val);
            }
            return Err(anyhow!("Remote host is offline"));
        }

        if canban_sync::is_read_cacheable_method(method) {
            if method == "timeline.list"
                && let Some(cid) = params["card_id"].as_str().filter(|s| !s.is_empty())
                && self.has_pending_create(&host_id, cid)
            {
                let mut fallback = self.offline_fallback(
                    &host_id,
                    method,
                    params,
                    anyhow!("Card creation is still syncing"),
                )?;
                self.qualify_remote_response(&host_id, &mut fallback);
                return Ok(fallback);
            }
            let (client_opt, client_gen) = {
                let guard = self.hosts.read().unwrap();
                let slot = guard
                    .get(&host_id)
                    .with_context(|| format!("Unknown host '{host_id}'"))?;
                (slot.client.clone(), slot.conn_gen)
            };
            if let Some(client) = client_opt {
                match client.call_async(method, &params).await {
                    Ok(mut val) => {
                        self.record_online_read_cache(&host_id, method, &params, &val);
                        if method == "timeline.list"
                            && let Some(cid) = params["card_id"].as_str().filter(|s| !s.is_empty())
                        {
                            let guard = self.hosts.read().unwrap();
                            if let Some(slot) = guard.get(&host_id) {
                                let outbox = slot.outbox.lock().unwrap();
                                val = cache_and_augment_timeline(
                                    val,
                                    method,
                                    &outbox,
                                    &slot.cached.snapshot,
                                    cid,
                                );
                            }
                        }
                        self.qualify_remote_response(&host_id, &mut val);
                        return Ok(val);
                    }
                    Err(err) if is_auth_error(&err) => {
                        self.mark_slot_disconnected(
                            &host_id,
                            Some(client_gen),
                            err.to_string(),
                            false,
                        );
                        return Err(err);
                    }
                    Err(err) if is_connection_error(&err) => {
                        self.mark_slot_disconnected(
                            &host_id,
                            Some(client_gen),
                            err.to_string(),
                            true,
                        );
                        let mut fallback = self.offline_fallback(&host_id, method, params, err)?;
                        self.qualify_remote_response(&host_id, &mut fallback);
                        return Ok(fallback);
                    }
                    Err(err) => return Err(err),
                }
            }
            let mut fallback =
                self.offline_fallback(&host_id, method, params, anyhow!("Remote host is offline"))?;
            self.qualify_remote_response(&host_id, &mut fallback);
            return Ok(fallback);
        }

        let (host_name, status_str, client_opt, client_gen, token_opt, card_blocked) = {
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
                    let cid = extract_card_id_for_mutation(&params, self.id_gen.as_ref());
                    let ob = slot.outbox.lock().unwrap();
                    ob.blocked_cards().contains(&cid) || ob.ops.iter().any(|m| m.card_id == cid)
                }
                _ => false,
            };
            (
                slot.saved.name.clone(),
                slot.connection.short_status(),
                slot.client.clone(),
                slot.conn_gen,
                slot.tcp_token(),
                blocked,
            )
        };
        let unavailable_err =
            || anyhow!("Host '{host_name}' is currently unavailable ({status_str})");

        let mut res = match canban_sync::classify_remote_mutation(method, &params) {
            QueueDecision::RejectOffline(msg) => {
                if let Some(client) = client_opt.as_ref() {
                    seal_params_for_tcp_with_id_gen(
                        method,
                        &mut params,
                        token_opt.as_deref(),
                        self.id_gen.as_ref(),
                    )?;
                    match client.call_async(method, &params).await {
                        Ok(val) => val,
                        Err(err) if is_auth_error(&err) => {
                            self.mark_slot_disconnected(
                                &host_id,
                                Some(client_gen),
                                err.to_string(),
                                false,
                            );
                            return Err(err);
                        }
                        Err(err) if is_connection_error(&err) => {
                            self.mark_slot_disconnected(
                                &host_id,
                                Some(client_gen),
                                err.to_string(),
                                true,
                            );
                            bail!("{msg} ({host_name})");
                        }
                        Err(err) => return Err(err),
                    }
                } else {
                    bail!("{msg} ({host_name})");
                }
            }
            QueueDecision::NotQueueable => {
                let client = client_opt.as_ref().ok_or_else(unavailable_err)?;
                seal_params_for_tcp_with_id_gen(
                    method,
                    &mut params,
                    token_opt.as_deref(),
                    self.id_gen.as_ref(),
                )?;
                match client.call_async(method, &params).await {
                    Ok(val) => val,
                    Err(err) if is_auth_error(&err) => {
                        self.mark_slot_disconnected(
                            &host_id,
                            Some(client_gen),
                            err.to_string(),
                            false,
                        );
                        return Err(err);
                    }
                    Err(err) if is_connection_error(&err) => {
                        self.mark_slot_disconnected(
                            &host_id,
                            Some(client_gen),
                            err.to_string(),
                            true,
                        );
                        return Err(err);
                    }
                    Err(err) => return Err(err),
                }
            }
            QueueDecision::Queue(kind) => {
                let request_key = params["request_key"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| self.id_gen.next_uuid().to_string());
                if let Some(obj) = params.as_object_mut() {
                    obj.insert("request_key".into(), json!(request_key));
                    if matches!(kind, MutationKind::CreateCard | MutationKind::AddComment)
                        && obj
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .is_none_or(str::is_empty)
                    {
                        obj.insert("id".into(), json!(self.id_gen.next_id()));
                    }
                }
                {
                    let guard = self.hosts.read().unwrap();
                    if let Some(slot) = guard.get(&host_id) {
                        let ob = slot.outbox.lock().unwrap();
                        if let Some(existing) = ob.ops.iter().find(|m| m.id == request_key).cloned()
                        {
                            return canban_sync::synthetic_receipt(
                                &existing,
                                &slot.cached.snapshot,
                                &ob.ops,
                                &host_id,
                            );
                        }
                    }
                }

                let (live_reply, sent_before_drop) =
                    if let (Some(client), false) = (client_opt.as_ref(), card_blocked) {
                        let mut wire_params = params.clone();
                        seal_params_for_tcp_with_id_gen(
                            method,
                            &mut wire_params,
                            token_opt.as_deref(),
                            self.id_gen.as_ref(),
                        )?;
                        match client.call_async(method, &wire_params).await {
                            Ok(val) => (Some(val), false),
                            Err(err) if is_auth_error(&err) => {
                                self.mark_slot_disconnected(
                                    &host_id,
                                    Some(client_gen),
                                    err.to_string(),
                                    false,
                                );
                                return Err(err);
                            }
                            Err(err) if is_connection_error(&err) => {
                                self.mark_slot_disconnected(
                                    &host_id,
                                    Some(client_gen),
                                    err.to_string(),
                                    true,
                                );
                                (None, true)
                            }
                            Err(err) => return Err(err),
                        }
                    } else {
                        (None, false)
                    };

                if let Some(val) = live_reply {
                    val
                } else {
                    return self.enqueue_offline_mutation(
                        &host_id,
                        kind,
                        request_key,
                        params,
                        sent_before_drop,
                        client_opt.is_some(),
                    );
                }
            }
        };

        self.finalize_online_remote_call(&host_id, method, client_opt.as_ref(), &mut res)
            .await;
        Ok(res)
    }

    async fn finalize_online_remote_call(
        &self,
        host_id: &str,
        method: &str,
        client_opt: Option<&Arc<ClientTransport>>,
        res: &mut Value,
    ) {
        let is_read_only = matches!(
            method,
            "app.ping"
                | "app.state"
                | "runtime.state"
                | "app.snapshot"
                | "agent.catalog"
                | "diagnostics.status"
                | "diagnostics.logs"
                | "directory.list"
                | "project.detect_github"
                | "project.archived.list"
                | "project.export"
                | "git.compare"
                | "git.refs"
                | "git.log"
                | "git.commit_detail"
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
                | "env.preview"
                | "host.env.keys"
                | "usage.status"
                | "usage.refresh"
                | "viewer.presence"
        );
        if is_read_only {
            self.qualify_remote_response(host_id, res);
            return;
        }
        if matches!(
            method,
            "config.agents.save" | "agent.setup" | "scheduler.configure" | "env.save"
        ) {
            {
                let mut guard = self.hosts.write().unwrap();
                if let Some(slot) = guard.get_mut(host_id) {
                    slot.cache_gen = slot.cache_gen.wrapping_add(1);
                    slot.local_cache_gen = slot.local_cache_gen.wrapping_add(1);
                    match method {
                        "config.agents.save" | "agent.setup" => {
                            if let Ok(cfg) = serde_json::from_value::<Config>(res.clone()) {
                                slot.cached.config = cfg;
                            }
                        }
                        "scheduler.configure" => {
                            if let Ok(settings) =
                                serde_json::from_value::<SchedulerSettings>(res.clone())
                            {
                                slot.cached.config.scheduler = settings;
                            }
                        }
                        "env.save" => {
                            if let (Some(snap_key), Ok(env_meta)) = (
                                res["snapshot_key"].as_str(),
                                serde_json::from_value::<BTreeMap<String, EnvEntryMeta>>(
                                    res["env"].clone(),
                                ),
                            ) {
                                if env_meta.is_empty() {
                                    slot.cached.snapshot.scoped_env.remove(snap_key);
                                } else {
                                    slot.cached
                                        .snapshot
                                        .scoped_env
                                        .insert(snap_key.to_owned(), env_meta);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            self.wake_host(host_id);
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
            self.qualify_remote_response(host_id, res);
            if matches!(
                method,
                "card.create" | "github.issue.add" | "github.item.add" | "terminal.create"
            ) && let Some(id) = res["id"].as_str()
            {
                let existing_other = {
                    let eh = self.entity_hosts.read().unwrap();
                    eh.get(id).is_some_and(|owner| owner != host_id)
                };
                let tracked_id = if existing_other {
                    let q = canban_types::qualify_host_id(host_id, id);
                    res["id"] = json!(q);
                    q
                } else {
                    id.to_owned()
                };
                self.acknowledged_owners
                    .lock()
                    .unwrap()
                    .insert(tracked_id.clone(), host_id.to_owned());
                self.entity_hosts
                    .write()
                    .unwrap()
                    .insert(tracked_id, host_id.to_owned());
            }
            self.wake_host(host_id);
        } else {
            self.qualify_remote_response(host_id, res);
            if let Some(client) = client_opt {
                let _ = self
                    .refresh_host_now_async(host_id, client, None, None)
                    .await;
            }
        }
        self.emit_host(
            host_id,
            AppEvent::Changed {
                topic: "application",
                card_id: None,
            },
        );
    }

    fn enqueue_offline_mutation(
        &self,
        host_id: &str,
        kind: MutationKind,
        mutation_id: String,
        params: Value,
        sent_before_drop: bool,
        should_drain: bool,
    ) -> Result<Value> {
        let card_id = match kind {
            MutationKind::SyncGlobalEnv => String::new(),
            MutationKind::CreateCard => params["id"].as_str().unwrap_or_default().to_owned(),
            _ => extract_card_id_for_mutation(&params, self.id_gen.as_ref()),
        };
        let receipt = {
            let guard = self.hosts.read().unwrap();
            let slot = guard
                .get(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            let base = if kind == MutationKind::UpdateCard {
                let mut projected = slot.cached.snapshot.clone();
                let existing_ops = slot.outbox.lock().unwrap().ops.clone();
                canban_sync::apply(&mut projected, host_id, &existing_ops);
                build_update_base(&params, &projected, &card_id)
            } else {
                UpdateBase::default()
            };
            let mutation = Mutation {
                id: mutation_id,
                host_id: host_id.to_owned(),
                card_id: card_id.clone(),
                kind,
                params: params.clone(),
                base,
                sent_params: if sent_before_drop {
                    params
                } else {
                    Value::Null
                },
                retry_same_params: sent_before_drop,
                state: MutationState::Queued,
                created_at: self.now_ms(),
                attempts: u32::from(sent_before_drop),
            };
            let mut outbox = slot.outbox.lock().unwrap();
            let (stored, _) = outbox.enqueue(mutation)?;
            outbox.save()?;
            canban_sync::synthetic_receipt(&stored, &slot.cached.snapshot, &outbox.ops, host_id)
                .context("Failed to compute offline synthetic receipt")?
        };
        if kind == MutationKind::CreateCard && !card_id.is_empty() {
            self.acknowledged_owners
                .lock()
                .unwrap()
                .insert(card_id.clone(), host_id.to_owned());
            self.entity_hosts
                .write()
                .unwrap()
                .insert(card_id.clone(), host_id.to_owned());
        }
        self.emit_host(
            host_id,
            AppEvent::Changed {
                topic: "application",
                card_id: (!card_id.is_empty()).then_some(card_id),
            },
        );
        if should_drain && let Some(this) = self.upgrade_self() {
            let hid = host_id.to_owned();
            self.runtime_handle.spawn(async move {
                let _ = this.drain_outbox_async(&hid).await;
            });
        }
        Ok(receipt)
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
                Err(err) if is_auth_error(&err) => {
                    self.mark_slot_disconnected(host_id, Some(client_gen), err.to_string(), false);
                    return Err(err);
                }
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
        let ops = slot.outbox.lock().unwrap().ops.clone();
        let mut snap = slot.cached.snapshot.clone();
        if !ops.is_empty() {
            canban_sync::apply(&mut snap, host_id, &ops);
        }
        canban_sync::offline_resolve_preview(&snap, card_id, &slot.saved.name)
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
        let _ = reads.put(method, params, val, card_id, card_rev, self.now_ms());
    }

    fn has_pending_create(&self, host_id: &str, card_id: &str) -> bool {
        let guard = self.hosts.read().unwrap();
        let Some(slot) = guard.get(host_id) else {
            return false;
        };
        slot.outbox
            .lock()
            .unwrap()
            .ops
            .iter()
            .any(|m| m.card_id == card_id && m.kind == MutationKind::CreateCard)
    }

    fn has_pending_card_ops(&self, host_id: &str, card_id: &str) -> bool {
        let guard = self.hosts.read().unwrap();
        let Some(slot) = guard.get(host_id) else {
            return false;
        };
        slot.outbox.lock().unwrap().ops.iter().any(|m| {
            m.card_id == card_id
                && matches!(
                    m.kind,
                    MutationKind::CreateCard
                        | MutationKind::UpdateCard
                        | MutationKind::MoveCard
                        | MutationKind::ResolveCard
                )
        })
    }

    fn projected_card_get(&self, host_id: &str, params: &Value) -> Option<Value> {
        let guard = self.hosts.read().unwrap();
        let slot = guard.get(host_id)?;
        let outbox = slot.outbox.lock().unwrap();
        let mut projected = slot.cached.snapshot.clone();
        canban_sync::apply(&mut projected, host_id, &outbox.ops);
        let card = if let Some(cid) = params["card_id"].as_str().filter(|s| !s.is_empty()) {
            projected.cards.into_iter().find(|c| c.id == cid)
        } else if let Some(num) = params["card"].as_u64() {
            let pid = params["project_id"].as_str().unwrap_or("");
            projected
                .cards
                .into_iter()
                .find(|c| c.number == num && (pid.is_empty() || c.project_id == pid))
        } else {
            None
        }?;
        serde_json::to_value(card).ok()
    }

    fn offline_fallback(
        &self,
        host_id: &str,
        method: &str,
        params: Value,
        conn_err: anyhow::Error,
    ) -> Result<Value> {
        if canban_sync::is_read_cacheable_method(method) {
            let guard = self.hosts.read().unwrap();
            let slot = guard
                .get(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            let outbox = slot.outbox.lock().unwrap();
            let mut reads = slot.reads.lock().unwrap();
            let now_ms = self.now_ms();
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
                let is_created_offline = outbox
                    .ops
                    .iter()
                    .any(|m| m.card_id == card_id && m.kind == MutationKind::CreateCard);
                let in_cached_snapshot = slot.cached.snapshot.cards.iter().any(|c| c.id == card_id);
                let pending = canban_sync::pending_comment_events(
                    &outbox.ops,
                    card_id,
                    &slot.cached.snapshot,
                );
                if is_created_offline || !pending.is_empty() || in_cached_snapshot {
                    return Ok(cache_and_augment_timeline(
                        json!({
                            "events": [],
                            "next_cursor": Value::Null,
                            "last_sequence": 0,
                            "highwater": 0,
                            "has_more_before": false,
                            "oldest_sequence": Value::Null,
                            "offline_partial": !is_created_offline,
                        }),
                        method,
                        &outbox,
                        &slot.cached.snapshot,
                        card_id,
                    ));
                }
            }
            return Err(conn_err);
        }
        Err(conn_err)
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
            if slot.updating || expected_gen.is_some_and(|g| slot.conn_gen != g) {
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
                    topic: "host",
                    card_id: None,
                },
            );
        }
    }

    /// Fans out `method` across `"local"` (when present) and all connected remote hosts synchronously.
    pub fn call_all_hosts(&self, method: &str, params: Value) -> Vec<(String, Result<Value>)> {
        block_on_rt(
            &self.runtime_handle,
            self.call_all_hosts_async(method, params),
        )
    }

    /// Fans out `method` across `"local"` (when present) and all connected remote hosts concurrently
    /// (skipping offline/disabled hosts).
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
            let method_owned = method.to_owned();
            let mut p = params.clone();
            let seal_res = seal_params_for_tcp_with_id_gen(
                &method_owned,
                &mut p,
                token_opt.as_deref(),
                self.id_gen.as_ref(),
            );
            set.spawn(async move {
                let res = match seal_res {
                    Err(e) => Err(e),
                    Ok(()) => client.call_async(&method_owned, &p).await,
                };
                (host_id, res)
            });
        }
        let mut out = Vec::new();
        if let Some(local) = self.local_host.clone() {
            out.push(("local".to_owned(), local.call(method, params.clone())));
        }
        let mut remote_out = Vec::new();
        while let Some(joined) = set.join_next().await {
            if let Ok(item) = joined {
                remote_out.push(item);
            }
        }
        remote_out.sort_by(|a, b| a.0.cmp(&b.0));
        out.extend(remote_out);
        out
    }

    /// Returns queued, in-flight, conflicted, and failed outbox items across all hosts
    /// (excluding internal `SyncGlobalEnv` pointer entries).
    pub fn sync_items(&self) -> Vec<SyncItemView> {
        let guard = self.hosts.read().unwrap();
        let mut items = Vec::new();
        for slot in guard.values() {
            let outbox = slot.outbox.lock().unwrap();
            for op in &outbox.ops {
                if op.kind == MutationKind::SyncGlobalEnv {
                    continue;
                }
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
                        if let Some(c) = srv
                            && obj.contains_key("revision")
                        {
                            obj.insert("revision".into(), json!(c.revision));
                        }
                        obj.insert("force".into(), json!(true));
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
                SyncAction::UseEdited { fields } => {
                    let op = outbox
                        .ops
                        .iter_mut()
                        .find(|m| m.id == mutation_id)
                        .with_context(|| format!("Unknown mutation '{mutation_id}'"))?;
                    let srv = server_cards.iter().find(|c| c.id == op.card_id);
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
                        for (k, v) in &fields {
                            obj.insert(k.clone(), json!(v));
                        }
                        if let Some(c) = srv
                            && obj.contains_key("revision")
                        {
                            obj.insert("revision".into(), json!(c.revision));
                        }
                        obj.insert("force".into(), json!(true));
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

    /// Enqueues a pointer-only `SyncGlobalEnv` mutation into matching enabled remote hosts' outboxes
    /// and immediately triggers drain on connected hosts. Zero secrets are persisted to `.outbox.json`.
    pub fn enqueue_global_env_sync_for(&self, host_ids: Option<&[String]>) {
        let targets: Vec<(String, Arc<Mutex<Outbox>>, bool)> = {
            let guard = self.hosts.read().unwrap();
            guard
                .iter()
                .filter(|(id, slot)| {
                    slot.saved.enabled && host_ids.is_none_or(|list| list.iter().any(|x| x == *id))
                })
                .map(|(id, slot)| (id.clone(), slot.outbox.clone(), slot.client.is_some()))
                .collect()
        };
        if targets.is_empty() {
            return;
        }
        for (hid, outbox_arc, _) in &targets {
            let mutation = Mutation {
                id: self.id_gen.next_uuid().to_string(),
                host_id: hid.clone(),
                card_id: String::new(),
                kind: MutationKind::SyncGlobalEnv,
                params: json!({}),
                base: UpdateBase::default(),
                sent_params: Value::Null,
                retry_same_params: false,
                state: MutationState::Queued,
                created_at: self.now_ms(),
                attempts: 0,
            };
            if let Ok(mut ob) = outbox_arc.lock() {
                let _ = ob.enqueue(mutation);
                let _ = ob.save();
            }
        }
        self.emit_host(
            "local",
            AppEvent::Changed {
                topic: "global_env_sync",
                card_id: None,
            },
        );
        if let Some(this) = self.upgrade_self() {
            for (hid, _, connected) in targets {
                if connected {
                    let c = Arc::clone(&this);
                    self.runtime_handle.spawn(async move {
                        let _ = c.drain_outbox_async(&hid).await;
                    });
                }
            }
        }
    }

    /// Manually triggers background replication of Global Environment & Secrets to all enabled remote hosts.
    pub fn sync_global_env_now(&self) {
        self.enqueue_global_env_sync_for(None);
    }

    pub fn global_env_sync_status(&self) -> GlobalEnvSyncStatus {
        let local_fp = self
            .global_env_provider
            .as_ref()
            .and_then(|p| p.current_fingerprint());
        let guard = self.hosts.read().unwrap();
        let mut status = GlobalEnvSyncStatus::default();
        for slot in guard.values() {
            if !slot.saved.enabled {
                continue;
            }
            status.total_remote_hosts += 1;
            let mut failed_message: Option<String> = None;
            let mut has_in_flight = false;
            let mut has_queued = false;
            if let Ok(ob) = slot.outbox.lock() {
                for m in &ob.ops {
                    if m.kind != MutationKind::SyncGlobalEnv {
                        continue;
                    }
                    match &m.state {
                        MutationState::Failed { message } => {
                            if failed_message.is_none() {
                                failed_message = Some(message.clone());
                            }
                        }
                        MutationState::Conflict { .. } => {
                            if failed_message.is_none() {
                                failed_message = Some("Sync conflict".into());
                            }
                        }
                        MutationState::InFlight => has_in_flight = true,
                        MutationState::Queued => has_queued = true,
                    }
                }
            }
            let host_name = slot.saved.name.clone();
            if let Some(message) = failed_message {
                status.failed_hosts.push(GlobalEnvSyncHostFailure {
                    host_id: slot.saved.id.clone(),
                    host_name,
                    message,
                });
            } else if has_in_flight
                || (has_queued
                    && (slot.client.is_some() || slot.draining_active.load(Ordering::SeqCst)))
            {
                status.syncing_hosts.push(host_name);
            } else if has_queued {
                status.queued_hosts.push(host_name);
            } else if local_fp
                .as_deref()
                .is_some_and(|fp| slot.global_env_synced_fingerprint.as_deref() != Some(fp))
            {
                status.unsynced_hosts.push(host_name);
            } else {
                status.synced_hosts.push(host_name);
            }
        }
        status
    }

    pub fn global_env_sync_summary(&self) -> GlobalEnvSyncSummary {
        if self.global_env_provider.is_some() {
            let status = self.global_env_sync_status();
            let mut pending_hosts = Vec::with_capacity(
                status.syncing_hosts.len()
                    + status.queued_hosts.len()
                    + status.unsynced_hosts.len()
                    + status.failed_hosts.len(),
            );
            for name in status
                .syncing_hosts
                .iter()
                .chain(status.queued_hosts.iter())
                .chain(status.unsynced_hosts.iter())
                .chain(status.failed_hosts.iter().map(|f| &f.host_name))
            {
                if !pending_hosts.contains(name) {
                    pending_hosts.push(name.clone());
                }
            }
            return GlobalEnvSyncSummary {
                synced_hosts: status.synced_hosts.len(),
                pending_hosts,
                total_remote_hosts: status.total_remote_hosts,
            };
        }
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

    /// Triggers `update.daemon` on a remote host (even when held in `ApiMismatch`), transitions
    /// the host through `Updating`, and waits within a bounded window for the restarted daemon to
    /// recover to `Connected`.
    pub fn update_remote_daemon(&self, host_id: &str) -> Result<Value> {
        block_on_rt(
            &self.runtime_handle,
            self.update_remote_daemon_async(host_id),
        )
    }

    /// Async implementation of [`Self::update_remote_daemon`].
    pub async fn update_remote_daemon_async(&self, host_id: &str) -> Result<Value> {
        ensure!(
            host_id != "local",
            "Use Settings -> Updates to update This Mac"
        );
        let (saved, prior_server_id) = {
            let mut guard = self.hosts.write().unwrap();
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
            slot.clear_connection(HostConnectionState::Updating {
                version: String::new(),
            });
            (slot.saved.clone(), slot.saved.server_id.clone())
        };

        struct UpdatingGuard<'a> {
            client: &'a MultiHostClient,
            host_id: String,
            active: bool,
        }
        impl Drop for UpdatingGuard<'_> {
            fn drop(&mut self) {
                if !self.active {
                    return;
                }
                {
                    let mut guard = self.client.hosts.write().unwrap();
                    if let Some(slot) = guard.get_mut(&self.host_id) {
                        slot.updating = false;
                        slot.next_retry_at = 0;
                        if slot.connection.is_updating() {
                            slot.connection = if slot.saved.enabled {
                                HostConnectionState::Connecting
                            } else {
                                HostConnectionState::Offline {
                                    error: "Disabled".into(),
                                    cached_at: slot.cached_at(),
                                }
                            };
                        }
                    }
                }
                self.client.emit_host(
                    &self.host_id,
                    AppEvent::Changed {
                        topic: "host",
                        card_id: None,
                    },
                );
                self.client.wake_host(&self.host_id);
            }
        }

        let mut guard = UpdatingGuard {
            client: self,
            host_id: host_id.to_owned(),
            active: true,
        };
        self.emit_host(
            host_id,
            AppEvent::Changed {
                topic: "host",
                card_id: None,
            },
        );
        self.wake_host(host_id);

        let params = json!({ "request_key": self.id_gen.next_id() });
        let rpc_res = match self
            .connector
            .connect(
                &format!("{host_id}-update"),
                &saved.transport,
                &self.config.device_name,
            )
            .await
        {
            Ok(connected) => {
                let res = connected
                    .transport
                    .call_async("update.daemon", &params)
                    .await;
                connected.transport.close();
                drop(connected.lease);
                res
            }
            Err(err) => Err(err),
        };

        let res = match rpc_res {
            Ok(val) => val,
            Err(err) => {
                guard.active = false;
                self.clear_updating_and_reprobe(host_id).await;
                self.emit_host(
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
                    let mut hguard = self.hosts.write().unwrap();
                    if let Some(slot) = hguard.get_mut(host_id)
                        && slot.updating
                    {
                        slot.connection = HostConnectionState::Updating {
                            version: target_ver.clone(),
                        };
                    }
                }
                self.emit_host(
                    host_id,
                    AppEvent::Changed {
                        topic: "host",
                        card_id: None,
                    },
                );
            }

            let deadline_ms = self.now_ms().saturating_add(10_000);
            let mono_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            tokio::time::sleep(Duration::from_millis(350)).await;
            let mut connected = false;
            while tokio::time::Instant::now() < mono_deadline && self.now_ms() < deadline_ms {
                let still_enabled = self
                    .hosts
                    .read()
                    .unwrap()
                    .get(host_id)
                    .is_some_and(|s| s.saved.enabled);
                if !still_enabled {
                    break;
                }
                if let Ok(session) = SingleHostSession::connect_checked_async_with_clock(
                    self.connector.as_ref(),
                    host_id,
                    &saved.transport,
                    &self.config.device_name,
                    Value::Null,
                    self.min_api_version,
                    self.clock.as_ref(),
                )
                .await
                {
                    let version = session.handshake().ping["build"]["version"]
                        .as_str()
                        .or_else(|| session.handshake().ping["version"].as_str())
                        .unwrap_or("unknown");
                    let new_sid = session.handshake().server_id.as_deref();
                    let sid_changed = prior_server_id
                        .as_deref()
                        .zip(new_sid)
                        .is_some_and(|(old, cur)| old != cur);
                    if target_ver.is_empty() || version == target_ver || sid_changed {
                        let upgraded = session
                            .upgraded_token()
                            .filter(|t| is_device_token(t))
                            .map(str::to_owned);
                        let (transport, lease, hs) = session.into_parts();
                        guard.active = false;
                        let entities = {
                            let mut hguard = self.hosts.write().unwrap();
                            let Some(slot) = hguard.get_mut(host_id) else {
                                transport.close();
                                drop(lease);
                                break;
                            };
                            slot.updating = false;
                            slot.next_retry_at = 0;
                            slot.clear_connection(HostConnectionState::Connecting);
                            let mut persist_needed = false;
                            if let HostTransport::Tcp { token: cur, .. } = &mut slot.saved.transport
                                && let Some(new_tok) = upgraded
                                && *cur != new_tok
                            {
                                *cur = new_tok;
                                persist_needed = true;
                            }
                            if let Some(sid) = hs.server_id.clone() {
                                slot.saved.server_id = Some(sid);
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
                                self.now_ms(),
                                self.fs_durability,
                            );
                            let entities = slot.cached.snapshot.entity_hosts.clone();
                            if persist_needed {
                                let _ = self.persist_locked(&hguard);
                            }
                            entities
                        };
                        if !entities.is_empty() {
                            self.replace_host_entity_owners(host_id, &entities);
                        }
                        if let Some(this) = self.upgrade_self() {
                            let hid = host_id.to_owned();
                            self.runtime_handle.spawn(async move {
                                let _ = this.drain_outbox_async(&hid).await;
                            });
                        }
                        connected = true;
                        break;
                    } else {
                        let (transport, lease, _) = session.into_parts();
                        transport.close();
                        drop(lease);
                    }
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
            }

            if !connected {
                guard.active = false;
                self.clear_updating_and_reprobe(host_id).await;
            }

            self.emit_host(
                host_id,
                AppEvent::Changed {
                    topic: if connected { "resync" } else { "host" },
                    card_id: None,
                },
            );
            self.wake_host(host_id);
        } else {
            guard.active = false;
            self.clear_updating_and_reprobe(host_id).await;
            self.emit_host(
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

    async fn clear_updating_and_reprobe(&self, host_id: &str) {
        {
            let mut guard = self.hosts.write().unwrap();
            if let Some(slot) = guard.get_mut(host_id) {
                slot.updating = false;
                slot.next_retry_at = 0;
                slot.clear_connection(if slot.saved.enabled {
                    HostConnectionState::Connecting
                } else {
                    HostConnectionState::Offline {
                        error: "Disabled".into(),
                        cached_at: slot.cached_at(),
                    }
                });
            }
        }
        let _ = self.sync_one_host_once(host_id).await;
    }

    async fn connect_slot_async(&self, host_id: &str) -> Result<(Arc<ClientTransport>, u64)> {
        let (
            start_epoch,
            start_conn_gen,
            start_transport,
            transport_spec,
            server_id,
            cached_at,
            saved_at,
            last_marker,
        ) = {
            let guard = self.hosts.read().unwrap();
            let slot = guard
                .get(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            ensure!(slot.saved.enabled, "Host '{host_id}' is disabled");
            ensure!(!slot.updating, "Host '{host_id}' is updating");
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
                slot.supervisor_epoch,
                slot.conn_gen,
                slot.saved.transport.clone(),
                spec,
                slot.saved.server_id.clone(),
                slot.cached_at(),
                slot.cached.saved_at,
                slot.last_marker.clone(),
            )
        };
        let is_current = |slot: &RemoteHostSlot| {
            !slot.updating
                && slot.supervisor_epoch == start_epoch
                && slot.conn_gen == start_conn_gen
                && slot.saved.transport == start_transport
        };

        let after = if saved_at == 0 {
            Value::Null
        } else {
            last_marker["revision"].clone()
        };
        let session = match SingleHostSession::connect_checked_async_with_clock(
            self.connector.as_ref(),
            host_id,
            &transport_spec,
            &self.config.device_name,
            after,
            self.min_api_version,
            self.clock.as_ref(),
        )
        .await
        {
            Ok(session) => session,
            Err(err) => {
                if let Some(mismatch) = err.downcast_ref::<ApiMismatchError>() {
                    let next_state = HostConnectionState::ApiMismatch {
                        server_api: mismatch.server_api,
                        app_api: mismatch.app_api,
                        version: mismatch.version.clone(),
                    };
                    let changed = {
                        let mut guard = self.hosts.write().unwrap();
                        if let Some(slot) = guard.get_mut(host_id)
                            && is_current(slot)
                        {
                            let prev = slot.connection.clone();
                            if slot.saved.server_id.is_none()
                                && let Some(sid) = &mismatch.server_id
                            {
                                slot.saved.server_id = Some(sid.clone());
                            }
                            slot.next_retry_at = self
                                .now_ms()
                                .saturating_add(API_MISMATCH_RETRY_DELAY.as_millis() as u64);
                            if slot.client.is_some() {
                                slot.clear_connection(next_state.clone());
                            } else {
                                slot.connection = next_state.clone();
                            }
                            prev != slot.connection
                        } else {
                            false
                        }
                    };
                    if changed {
                        self.emit_host(
                            host_id,
                            AppEvent::Changed {
                                topic: "host",
                                card_id: None,
                            },
                        );
                    }
                    return Err(err);
                }
                let err_str = err.to_string();
                let auth = is_auth_error(&err);
                let changed = {
                    let mut guard = self.hosts.write().unwrap();
                    if let Some(slot) = guard.get_mut(host_id)
                        && is_current(slot)
                    {
                        let prev = slot.connection.clone();
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
                        prev != slot.connection
                    } else {
                        false
                    }
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
            if let Some(slot) = guard.get_mut(host_id)
                && is_current(slot)
            {
                slot.clear_connection(HostConnectionState::AuthFailed {
                    error: err_msg.clone(),
                });
            }
            bail!("{err_msg}");
        }

        let (conn_gen, entities) = {
            let mut guard = self.hosts.write().unwrap();
            let Some(slot) = guard.get_mut(host_id) else {
                transport.close();
                drop(lease);
                bail!("Host '{host_id}' was removed");
            };
            if !slot.saved.enabled || !is_current(slot) {
                transport.close();
                drop(lease);
                bail!("Host '{host_id}' state changed during connect");
            }
            slot.clear_connection(HostConnectionState::Connecting);
            slot.next_retry_at = 0;
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
                self.now_ms(),
                self.fs_durability,
            );
            let conn_gen = slot.conn_gen;
            let entities = slot.cached.snapshot.entity_hosts.clone();
            if persist_needed {
                let _ = self.persist_locked(&guard);
            }
            (conn_gen, entities)
        };

        if !entities.is_empty() {
            self.replace_host_entity_owners(host_id, &entities);
        }
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
        let (after, saved_at, before_gen, before_local_gen) = {
            let guard = self.hosts.read().unwrap();
            let slot = guard
                .get(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            (
                slot.last_marker.clone(),
                slot.cached.saved_at,
                slot.cache_gen,
                slot.local_cache_gen,
            )
        };
        let req_after = if saved_at == 0 {
            Value::Null
        } else {
            after["revision"].clone()
        };
        let started_ms = self.now_ms();
        let state_val = client
            .call_async("app.state", &json!({"after": req_after}))
            .await?;
        let finished_ms = self.now_ms();
        let rtt_ms = finished_ms.saturating_sub(started_ms);

        let (emitted_topic, updated_entities, stale_inflight) = {
            let mut guard = self.hosts.write().unwrap();
            let Some(slot) = guard.get_mut(host_id) else {
                return Ok(());
            };
            if slot.updating || !slot.saved.enabled {
                return Ok(());
            }
            let incoming_changed =
                state_val["changed"] == true || state_val.get("snapshot").is_some();
            // Reject an in-flight `app.state` if a local non-cursor write (`config.agents.save`,
            // `env.save`, or outbox write-receipt hydration) updated the cache while this RPC was
            // in flight, or if a legacy server without a tuple cursor raced another cache write.
            // When concurrent tuple-cursor refreshes race, defer to `apply_state_to_slot`'s
            // cursor monotonicity check so strictly newer snapshots still apply cleanly while
            // stale snapshots are rejected without redundant round-trips.
            let blocked_by_local_mutation = incoming_changed
                && (slot.local_cache_gen != before_local_gen
                    || (slot.cache_gen != before_gen
                        && parse_state_cursor(&state_val["revision"]).is_none()));
            if blocked_by_local_mutation {
                (None, None, true)
            } else {
                let outcome = apply_state_to_slot(
                    slot,
                    &self.config.data_dir,
                    &state_val,
                    Some(rtt_ms),
                    None,
                    preferred_topic,
                    finished_ms,
                    self.fs_durability,
                );
                let entities = (!outcome.stale_rejected && outcome.emitted_topic.is_some())
                    .then(|| slot.cached.snapshot.entity_hosts.clone());
                (outcome.emitted_topic, entities, outcome.needs_resync)
            }
        };
        if let Some(entities) = updated_entities {
            self.replace_host_entity_owners(host_id, &entities);
        }
        if let Some(topic) = emitted_topic {
            self.emit_host(host_id, AppEvent::Changed { topic, card_id });
        }
        if stale_inflight {
            self.wake_host(host_id);
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
        let started_ms = self.now_ms();
        let mut val = match client.call_async("runtime.state", &json!({})).await {
            Ok(v) => v,
            Err(err) if !is_connection_error(&err) => {
                return self
                    .refresh_host_now_async(host_id, client, Some(topic), card_id)
                    .await;
            }
            Err(err) => return Err(err),
        };
        let now_ms = self.now_ms();
        let rtt_ms = now_ms.saturating_sub(started_ms);
        let Ok(mut rt) = serde_json::from_value::<RuntimeState>(val["runtime"].take()) else {
            return self
                .refresh_host_now_async(host_id, client, Some(topic), card_id)
                .await;
        };
        rt.terminal_hosts.clear();
        rt.host_active_agents.clear();
        rt.register_host(host_id);

        {
            let mut guard = self.hosts.write().unwrap();
            let Some(slot) = guard.get_mut(host_id) else {
                return Ok(());
            };
            slot.cached.snapshot.apply_runtime_state(rt);
            let active_agents = count_active_agents(&slot.cached.snapshot, now_ms);
            if let Some(summary) = slot.cached.snapshot.host_summaries.get_mut(host_id) {
                summary.active_agents = active_agents;
            }
            canban_notifications::reconcile_runtime_reminders_at(&mut slot.cached.snapshot, now_ms);
            slot.cached.snapshot.register_host_entities(host_id);
            slot.last_synced_at = Some(now_ms);
            if let HostConnectionState::Connected {
                rtt_ms: ref mut prev_rtt,
                ..
            } = slot.connection
            {
                *prev_rtt = rtt_ms;
            }
            if !latest_marker["revision"].is_null()
                && !cursor_is_strictly_older(
                    &slot.last_runtime_marker["revision"],
                    &latest_marker["revision"],
                )
            {
                slot.last_runtime_marker = latest_marker.clone();
            }
        }
        self.emit_host(host_id, AppEvent::Changed { topic, card_id });
        Ok(())
    }

    pub async fn drain_outbox_async(&self, host_id: &str) -> Result<DrainReport> {
        let (draining_lock, draining_active) = {
            let guard = self.hosts.read().unwrap();
            let slot = guard
                .get(host_id)
                .with_context(|| format!("Unknown host '{host_id}'"))?;
            (
                Arc::clone(&slot.draining),
                Arc::clone(&slot.draining_active),
            )
        };
        let _drain_guard = draining_lock.lock().await;
        draining_active.store(true, Ordering::SeqCst);
        let _active_guard = AtomicFlagGuard(draining_active);

        let mut report = DrainReport::default();
        let mut blocked = BTreeSet::new();
        let mut rebuild_counts: BTreeMap<String, u32> = BTreeMap::new();
        let mut deferred_retry_ms: Option<u64> = None;
        let mut active_client: Option<Arc<ClientTransport>> = None;

        loop {
            let step = {
                let guard = self.hosts.read().unwrap();
                let Some(slot) = guard.get(host_id) else {
                    break;
                };
                if !slot.saved.enabled
                    || slot.updating
                    || matches!(slot.connection, HostConnectionState::AuthFailed { .. })
                {
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
                if !matches!(action, StepAction::Idle) {
                    let _ = outbox.save();
                }
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

            let allow_rebuild = rebuild_counts.get(&mutation_id).copied().unwrap_or(0)
                < MAX_REVISION_REBUILDS_PER_DRAIN;
            let is_global_env_sync = method == "env.global.sync";
            let (payload, sync_fingerprint) = if is_global_env_sync {
                if let Some(provider) = &self.global_env_provider {
                    match provider.build_sync_payload(token.as_deref()) {
                        Ok((wire_params, fp)) => (Ok(wire_params), fp),
                        Err(e) => (Err(e), None),
                    }
                } else {
                    (Ok(sent_params), None)
                }
            } else {
                match seal_params_for_tcp_with_id_gen(
                    method,
                    &mut sent_params,
                    token.as_deref(),
                    self.id_gen.as_ref(),
                ) {
                    Ok(()) => (Ok(sent_params), None),
                    Err(e) => (Err(e), None),
                }
            };
            let result = match payload {
                Ok(params_to_send) => client.call_async(method, &params_to_send).await,
                Err(e) => Err(e),
            };
            let sync_succeeded = result.is_ok();
            if is_global_env_sync && sync_succeeded {
                save_global_env_sync_fingerprint_with_durability(
                    &self.config.data_dir,
                    host_id,
                    sync_fingerprint.as_deref(),
                    self.fs_durability,
                );
            }
            let (is_conn_err, is_transient_storage, refreshed) = match &result {
                Ok(_) => (false, false, None),
                Err(err) => {
                    let mut conn = is_connection_error(err);
                    let mut transient_storage = !conn && is_transient_storage_error(err);
                    let msg = format!("{err:#}");
                    let need_refresh = !conn
                        && !transient_storage
                        && (msg.contains("revision_conflict")
                            || msg.contains("request_key_conflict"));
                    let snap = if need_refresh {
                        match client.call_async("app.state", &json!({})).await {
                            Ok(v) => {
                                parse_refreshed_snapshot(&v, host_id).map(|s| (state_marker(&v), s))
                            }
                            Err(refresh_err) => {
                                if is_connection_error(&refresh_err) {
                                    conn = true;
                                } else if is_transient_storage_error(&refresh_err) {
                                    transient_storage = true;
                                }
                                None
                            }
                        }
                    } else {
                        None
                    };
                    (conn, transient_storage, snap)
                }
            };

            enum DrainStepOutcome {
                Finish(FinishOutcome),
                DeferredTransient { delay_ms: u64 },
            }

            let outcome = {
                let mut guard = self.hosts.write().unwrap();
                let Some(slot) = guard.get_mut(host_id) else {
                    break;
                };
                let mut outbox = slot.outbox.lock().unwrap();
                if is_transient_storage && !is_conn_err {
                    let err_msg = result
                        .as_ref()
                        .err()
                        .map(|e| format!("{e:#}"))
                        .unwrap_or_default();
                    if let Some(pos) = outbox.ops.iter().position(|m| m.id == mutation_id) {
                        let block_key = if outbox.ops[pos].kind == MutationKind::SyncGlobalEnv {
                            "__global_env__".to_owned()
                        } else {
                            outbox.ops[pos].card_id.clone()
                        };
                        outbox.ops[pos].state = MutationState::Queued;
                        outbox.ops[pos].retry_same_params = !err_msg.contains("revision_conflict");
                        let attempts = outbox.ops[pos].attempts;
                        let _ = outbox.save();
                        blocked.insert(block_key);
                        DrainStepOutcome::DeferredTransient {
                            delay_ms: transient_storage_retry_delay_ms(attempts),
                        }
                    } else {
                        DrainStepOutcome::Finish(FinishOutcome::Continue)
                    }
                } else {
                    let mut snap = slot.cached.snapshot.clone();
                    let mut refreshed_marker_to_apply: Option<Value> = None;
                    let mut refreshed_new_instance = false;
                    let refreshed_for_step = match refreshed {
                        Some((refreshed_marker, refreshed_snap)) => {
                            let stale = is_stale_snapshot_cursor(
                                &slot.last_marker["revision"],
                                &slot.cached.snapshot,
                                &refreshed_marker["revision"],
                                Some(&refreshed_snap),
                            );
                            if stale {
                                Some(slot.cached.snapshot.clone())
                            } else {
                                refreshed_new_instance = cursor_changed_instance(
                                    &slot.last_marker["revision"],
                                    &refreshed_marker["revision"],
                                );
                                refreshed_marker_to_apply = Some(refreshed_marker);
                                Some(refreshed_snap)
                            }
                        }
                        None => None,
                    };
                    let outcome = finish_step(
                        &mut outbox,
                        &mut snap,
                        &mutation_id,
                        result,
                        was_retry_same_params,
                        is_conn_err,
                        refreshed_for_step,
                        allow_rebuild,
                        &mut blocked,
                        &mut report,
                    );
                    let _ = outbox.save();
                    if is_global_env_sync && sync_succeeded {
                        slot.global_env_synced_fingerprint = sync_fingerprint.clone();
                    }
                    if !refreshed_new_instance {
                        preserve_monotonic_card_revisions(&slot.cached.snapshot, &mut snap);
                    }
                    snap.register_host_entities(host_id);
                    slot.cached.snapshot = snap;
                    slot.cache_gen = slot.cache_gen.wrapping_add(1);
                    if let Some(marker) = refreshed_marker_to_apply {
                        if !marker["revision"].is_null() {
                            slot.last_marker = marker.clone();
                            slot.last_runtime_marker = marker;
                        }
                    } else if sync_succeeded {
                        slot.local_cache_gen = slot.local_cache_gen.wrapping_add(1);
                    }
                    slot.cached.saved_at = self.now_ms();
                    save_cache_with_durability(
                        &self.config.data_dir,
                        host_id,
                        &slot.cached,
                        self.fs_durability,
                    );
                    DrainStepOutcome::Finish(outcome)
                }
            };
            if is_global_env_sync {
                self.emit_host(
                    host_id,
                    AppEvent::Changed {
                        topic: "global_env_sync",
                        card_id: None,
                    },
                );
            }

            match outcome {
                DrainStepOutcome::Finish(FinishOutcome::Continue) => {}
                DrainStepOutcome::Finish(FinishOutcome::RetryImmediate) => {
                    *rebuild_counts.entry(mutation_id).or_insert(0) += 1;
                }
                DrainStepOutcome::Finish(FinishOutcome::Deferred) => {
                    deferred_retry_ms = Some(deferred_retry_ms.map_or(100, |prev| prev.max(100)));
                }
                DrainStepOutcome::DeferredTransient { delay_ms } => {
                    deferred_retry_ms =
                        Some(deferred_retry_ms.map_or(delay_ms, |prev| prev.max(delay_ms)));
                    break;
                }
                DrainStepOutcome::Finish(FinishOutcome::StopConnection) => {
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
            if !report.aborted_by_connection
                && let Some(client) = active_client
            {
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
        if let Some(delay_ms) = deferred_retry_ms
            && !report.aborted_by_connection
            && let Some(this) = self.upgrade_self()
        {
            let hid = host_id.to_owned();
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            self.runtime_handle.spawn(async move {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {}
                    _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {
                        this.wake_host(&hid);
                    }
                }
            });
        }
        self.spawn_prefetch_timelines(host_id);
        Ok(report)
    }

    fn spawn_prefetch_timelines(&self, host_id: &str) {
        let Some(this) = self.upgrade_self() else {
            return;
        };
        let (prefetching_flag, targets, reads_arc) = {
            let guard = self.hosts.read().unwrap();
            let Some(slot) = guard.get(host_id) else {
                return;
            };
            if slot.client.is_none() || slot.updating || !slot.saved.enabled {
                return;
            }
            let targets: Vec<(String, u64)> = slot
                .cached
                .snapshot
                .cards
                .iter()
                .filter(|c| c.active())
                .map(|c| (c.id.clone(), c.revision))
                .collect();
            (
                Arc::clone(&slot.prefetching),
                targets,
                Arc::clone(&slot.reads),
            )
        };
        if targets.is_empty()
            || prefetching_flag
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            return;
        }
        let weak = Arc::downgrade(&this);
        let hid = host_id.to_owned();
        self.runtime_handle.spawn(async move {
            let _guard = AtomicFlagGuard(prefetching_flag);
            for (card_id, rev) in targets {
                let client = {
                    let Some(this) = weak.upgrade() else {
                        break;
                    };
                    let guard = this.hosts.read().unwrap();
                    let Some(slot) = guard.get(&hid) else {
                        break;
                    };
                    if !slot.saved.enabled
                        || slot.updating
                        || slot.draining_active.load(Ordering::SeqCst)
                    {
                        break;
                    }
                    slot.client.clone()
                };
                let Some(client) = client else {
                    break;
                };
                if reads_arc.lock().unwrap().has_fresh_timeline(&card_id, rev) {
                    continue;
                }
                let params =
                    json!({"card_id": card_id, "tail": true, "limit": 200, "view": "full"});
                match client.call_async("timeline.list", &params).await {
                    Ok(val) => {
                        let Some(this) = weak.upgrade() else {
                            break;
                        };
                        let ts = this.now_ms();
                        let _ = reads_arc.lock().unwrap().put(
                            "timeline.list",
                            &params,
                            &val,
                            Some(&card_id),
                            Some(rev),
                            ts,
                        );
                    }
                    Err(err) if is_connection_error(&err) => break,
                    Err(_) => {}
                }
            }
        });
    }

    async fn sync_one_host_once(&self, host_id: &str) -> Result<()> {
        let now_ms = self.now_ms();
        let existing = {
            let guard = self.hosts.read().unwrap();
            let Some(slot) = guard.get(host_id) else {
                return Ok(());
            };
            if !slot.saved.enabled
                || slot.updating
                || matches!(slot.connection, HostConnectionState::AuthFailed { .. })
            {
                return Ok(());
            }
            if slot.client.is_none() && now_ms < slot.next_retry_at {
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

async fn local_push_supervisor(
    weak_client: Weak<MultiHostClient>,
    local_host: Arc<dyn LocalHostDriver>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut last = Value::Null;
    loop {
        if *shutdown_rx.borrow() || weak_client.strong_count() == 0 {
            return;
        }
        let transport_res = tokio::select! {
            biased;
            _ = shutdown_rx.changed() => return,
            res = local_host.connect_push_transport() => res,
        };
        let transport = match transport_res {
            Ok(t) => t,
            Err(_) => {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => return,
                    _ = tokio::time::sleep(Duration::from_millis(500)) => continue,
                }
            }
        };
        let open_res = tokio::select! {
            biased;
            _ = shutdown_rx.changed() => return,
            res = open_async_subscribe(&transport, &last["revision"], SUBSCRIBE_TOPICS, None) => res,
        };
        let (mut stream, handshake) = match open_res {
            Ok(v) => v,
            Err(_) => {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => return,
                    _ = tokio::time::sleep(Duration::from_millis(500)) => continue,
                }
            }
        };
        if handshake.changed {
            let topic = change_topic(!last.is_null(), &last, &handshake.marker);
            last = handshake.marker;
            if let Some(rc) = weak_client.upgrade() {
                rc.emit_host(
                    "local",
                    AppEvent::Changed {
                        topic,
                        card_id: None,
                    },
                );
            } else {
                return;
            }
        } else if last.is_null() && !handshake.marker["revision"].is_null() {
            last = handshake.marker;
        }

        loop {
            let next = tokio::select! {
                biased;
                _ = shutdown_rx.changed() => return,
                res = stream.next_frame() => res,
            };
            let Some(rc) = weak_client.upgrade() else {
                return;
            };
            match next {
                Ok(Some(PushFrame::Heartbeat)) => {}
                Ok(Some(PushFrame::StateChanged {
                    topic,
                    card_id,
                    marker,
                })) => {
                    if !marker["revision"].is_null() {
                        last = marker;
                    }
                    rc.emit_host("local", AppEvent::Changed { topic, card_id });
                }
                Ok(Some(PushFrame::UpdateChanged(state))) => {
                    rc.emit(ClientEvent::UpdateChanged {
                        host_id: "local".into(),
                        state,
                    });
                }
                Ok(Some(PushFrame::UtilitiesChanged(state))) => {
                    rc.emit(ClientEvent::UtilitiesChanged {
                        host_id: "local".into(),
                        state,
                    });
                }
                Ok(Some(PushFrame::UsageChanged(state))) => {
                    rc.emit(ClientEvent::UsageChanged {
                        host_id: "local".into(),
                        state,
                    });
                }
                Ok(Some(PushFrame::HerdrInstall(state))) => {
                    rc.emit(ClientEvent::HerdrInstall {
                        host_id: "local".into(),
                        state,
                    });
                }
                Ok(None) | Err(_) => break,
            }
        }
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => return,
            _ = tokio::time::sleep(Duration::from_millis(250)) => {}
        }
    }
}

#[derive(Clone, Debug)]
struct PendingRefresh {
    topic: &'static str,
    card_id: Option<String>,
    latest_marker: Value,
    deadline: tokio::time::Instant,
    max_deadline: tokio::time::Instant,
    retry_attempts: u32,
}

impl PendingRefresh {
    fn schedule_push(
        pending: &mut Option<Self>,
        topic: &'static str,
        card_id: Option<String>,
        marker: Value,
        now: tokio::time::Instant,
    ) {
        let rev_present = !marker["revision"].is_null();
        match pending {
            Some(p) => {
                p.topic = merge_push_topic(p.topic, topic);
                if p.card_id != card_id {
                    p.card_id = None;
                }
                if rev_present {
                    p.latest_marker = marker;
                }
                if p.retry_attempts == 0 {
                    p.deadline = (now + PUSH_DEBOUNCE).min(p.max_deadline);
                }
            }
            None => {
                *pending = Some(Self {
                    topic,
                    card_id,
                    latest_marker: marker,
                    deadline: now + PUSH_DEBOUNCE,
                    max_deadline: now + PUSH_MAX_DEBOUNCE,
                    retry_attempts: 0,
                });
            }
        }
    }

    fn seed_retry(
        topic: &'static str,
        card_id: Option<String>,
        latest_marker: Value,
        err: &anyhow::Error,
        now: tokio::time::Instant,
    ) -> Option<Self> {
        Self {
            topic,
            card_id,
            latest_marker,
            deadline: now,
            max_deadline: now,
            retry_attempts: 0,
        }
        .requeue_after_error(err, now)
    }

    fn requeue_after_error(
        mut self,
        err: &anyhow::Error,
        now: tokio::time::Instant,
    ) -> Option<Self> {
        let next_attempt = self.retry_attempts.saturating_add(1);
        if !is_transient_storage_error(err) && next_attempt > PUSH_REFRESH_MAX_NON_STORAGE_RETRIES {
            return None;
        }
        let delay = Duration::from_millis(push_refresh_retry_delay_ms(next_attempt));
        self.retry_attempts = next_attempt;
        self.deadline = now + delay;
        self.max_deadline = self.deadline;
        Some(self)
    }
}

async fn run_legacy_poll_loop(
    weak_client: &Weak<MultiHostClient>,
    host_id: &str,
    epoch: u64,
    conn_gen: u64,
    transport: &Arc<ClientTransport>,
    notify: &Arc<Notify>,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> bool {
    let slot_active = || {
        let Some(rc) = weak_client.upgrade() else {
            return false;
        };
        let guard = rc.hosts.read().unwrap();
        guard.get(host_id).is_some_and(|s| {
            s.saved.enabled
                && !s.updating
                && s.supervisor_epoch == epoch
                && s.conn_gen == conn_gen
                && s.client.is_some()
        })
    };
    while slot_active() {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => { return true; }
            _ = notify.notified() => {}
            _ = tokio::time::sleep(LEGACY_POLL_INTERVAL) => {}
        }
        if !slot_active() {
            break;
        }
        let Some(rc) = weak_client.upgrade() else {
            return true;
        };
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
    weak_client: Weak<MultiHostClient>,
    host_id: String,
    epoch: u64,
    notify: Arc<Notify>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut backoff = Duration::from_millis(250);
    loop {
        if *shutdown_rx.borrow() || weak_client.strong_count() == 0 {
            return;
        }
        let (enabled, epoch_matches, updating, auth_failed, next_retry_at, now_ms, token, existing) = {
            let Some(rc) = weak_client.upgrade() else {
                return;
            };
            let now_ms = rc.now_ms();
            let guard = rc.hosts.read().unwrap();
            let Some(slot) = guard.get(&host_id) else {
                return;
            };
            (
                slot.saved.enabled,
                slot.supervisor_epoch == epoch,
                slot.updating,
                matches!(slot.connection, HostConnectionState::AuthFailed { .. }),
                slot.next_retry_at,
                now_ms,
                slot.tcp_token(),
                slot.client
                    .clone()
                    .map(|c| (c, slot.conn_gen, slot.last_marker.clone())),
            )
        };
        if !enabled || !epoch_matches {
            return;
        }
        if updating || auth_failed || token.as_deref().is_some_and(is_pairing_code) {
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => { return; }
                _ = notify.notified() => {}
            }
            continue;
        }

        let (transport, conn_gen, last_marker) = if let Some(pair) = existing {
            pair
        } else {
            if next_retry_at > now_ms {
                let wait = Duration::from_millis(next_retry_at.saturating_sub(now_ms));
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => { return; }
                    _ = notify.notified() => {}
                    _ = tokio::time::sleep(wait) => {}
                }
                continue;
            }
            let connect_res = {
                let Some(rc) = weak_client.upgrade() else {
                    return;
                };
                match rc.connect_slot_async(&host_id).await {
                    Ok((c, conn_gen)) => {
                        let marker = rc
                            .hosts
                            .read()
                            .unwrap()
                            .get(&host_id)
                            .map(|s| s.last_marker.clone())
                            .unwrap_or(Value::Null);
                        Ok((c, conn_gen, marker))
                    }
                    Err(err) => Err(err),
                }
            };
            match connect_res {
                Ok(triple) => triple,
                Err(err) => {
                    let still_current = weak_client.upgrade().is_some_and(|rc| {
                        rc.hosts
                            .read()
                            .unwrap()
                            .get(&host_id)
                            .is_some_and(|s| s.saved.enabled && s.supervisor_epoch == epoch)
                    });
                    if !still_current {
                        return;
                    }
                    if err.downcast_ref::<ApiMismatchError>().is_some() {
                        tokio::select! {
                            biased;
                            _ = shutdown_rx.changed() => { return; }
                            _ = notify.notified() => { backoff = Duration::from_millis(250); }
                            _ = tokio::time::sleep(API_MISMATCH_RETRY_DELAY) => {}
                        }
                    } else if is_auth_error(&err) {
                        tokio::select! {
                            biased;
                            _ = shutdown_rx.changed() => { return; }
                            _ = notify.notified() => {}
                        }
                    } else {
                        tokio::select! {
                            biased;
                            _ = shutdown_rx.changed() => { return; }
                            _ = notify.notified() => {
                                backoff = Duration::from_millis(250);
                            }
                            _ = tokio::time::sleep(backoff) => {
                                backoff = (backoff * 2).min(MAX_BACKOFF);
                            }
                        }
                    }
                    continue;
                }
            }
        };

        if let Some(rc) = weak_client.upgrade() {
            let _ = rc.drain_outbox_async(&host_id).await;
        } else {
            return;
        }

        let sub_res = tokio::select! {
            biased;
            _ = shutdown_rx.changed() => return,
            res = open_async_subscribe(
                &transport,
                &last_marker["revision"],
                SUBSCRIBE_TOPICS,
                Some(15_000),
            ) => res,
        };
        let (mut stream, handshake) = match sub_res {
            Ok(res) => {
                backoff = Duration::from_millis(250);
                res
            }
            Err(err) if is_unsupported_subscribe_error(&err) => {
                backoff = Duration::from_millis(250);
                if run_legacy_poll_loop(
                    &weak_client,
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
                if let Some(rc) = weak_client.upgrade() {
                    rc.mark_slot_disconnected(&host_id, Some(conn_gen), err.to_string(), false);
                } else {
                    return;
                }
                if is_auth_error(&err) {
                    tokio::select! {
                        biased;
                        _ = shutdown_rx.changed() => { return; }
                        _ = notify.notified() => { backoff = Duration::from_millis(250); }
                    }
                } else {
                    tokio::select! {
                        biased;
                        _ = shutdown_rx.changed() => { return; }
                        _ = notify.notified() => { backoff = Duration::from_millis(250); }
                        _ = tokio::time::sleep(backoff) => {
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                        }
                    }
                }
                continue;
            }
        };

        let mut pending: Option<PendingRefresh> = None;

        if handshake.changed {
            if let Some(rc) = weak_client.upgrade() {
                if let Err(err) = rc
                    .refresh_host_now_async(&host_id, &transport, None, None)
                    .await
                {
                    if is_connection_error(&err) || is_auth_error(&err) {
                        rc.mark_slot_disconnected(&host_id, Some(conn_gen), err.to_string(), false);
                        continue;
                    }
                    pending = PendingRefresh::seed_retry(
                        "resync",
                        None,
                        handshake.marker.clone(),
                        &err,
                        tokio::time::Instant::now(),
                    );
                }
                let _ = rc.drain_outbox_async(&host_id).await;
            } else {
                return;
            }
        }

        loop {
            {
                let Some(rc) = weak_client.upgrade() else {
                    return;
                };
                let guard = rc.hosts.read().unwrap();
                let still_valid = guard.get(&host_id).is_some_and(|s| {
                    s.saved.enabled
                        && !s.updating
                        && s.supervisor_epoch == epoch
                        && s.conn_gen == conn_gen
                        && s.client.is_some()
                });
                if !still_valid {
                    break;
                }
            }

            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    return;
                }
                _ = notify.notified() => {
                    let Some(rc) = weak_client.upgrade() else {
                        return;
                    };
                    let still_valid = {
                        let guard = rc.hosts.read().unwrap();
                        guard.get(&host_id).is_some_and(|s| {
                            s.saved.enabled
                                && !s.updating
                                && s.supervisor_epoch == epoch
                                && s.conn_gen == conn_gen
                                && s.client.is_some()
                        })
                    };
                    if !still_valid {
                        break;
                    }
                    match rc
                        .refresh_host_now_async(&host_id, &transport, None, None)
                        .await
                    {
                        Ok(()) => {
                            if pending
                                .as_ref()
                                .is_some_and(|p| p.latest_marker["revision"].is_null())
                            {
                                pending = None;
                            }
                        }
                        Err(err) if is_connection_error(&err) || is_auth_error(&err) => {
                            rc.mark_slot_disconnected(
                                &host_id,
                                Some(conn_gen),
                                err.to_string(),
                                false,
                            );
                            break;
                        }
                        Err(err) => {
                            let now_i = tokio::time::Instant::now();
                            pending = match pending.take() {
                                Some(existing) => existing.requeue_after_error(&err, now_i),
                                None => PendingRefresh::seed_retry(
                                    "application",
                                    None,
                                    Value::Null,
                                    &err,
                                    now_i,
                                ),
                            };
                        }
                    }
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
                    let Some(rc) = weak_client.upgrade() else {
                        return;
                    };
                    let (still_active, already_fetched, can_use_runtime) = {
                        let guard = rc.hosts.read().unwrap();
                        let Some(slot) = guard.get(&host_id) else {
                            break;
                        };
                        let active = slot.saved.enabled
                            && !slot.updating
                            && slot.supervisor_epoch == epoch
                            && slot.conn_gen == conn_gen
                            && slot.client.is_some();
                        let rev = &p.latest_marker["revision"];
                        let up_to_date = p.topic != "resync"
                            && (cursor_already_covered_by(rev, &slot.last_marker["revision"])
                                || (is_runtime_topic(p.topic)
                                    && cursor_already_covered_by(
                                        rev,
                                        &slot.last_runtime_marker["revision"],
                                    )));
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
                            p.card_id.clone(),
                            &p.latest_marker,
                        )
                        .await
                    } else {
                        rc.refresh_host_now_async(
                            &host_id,
                            &transport,
                            Some(p.topic),
                            p.card_id.clone(),
                        )
                        .await
                    };
                    if let Err(err) = refresh_res {
                        if is_connection_error(&err) || is_auth_error(&err) {
                            rc.mark_slot_disconnected(
                                &host_id,
                                Some(conn_gen),
                                err.to_string(),
                                false,
                            );
                            break;
                        }
                        pending = p.requeue_after_error(&err, tokio::time::Instant::now());
                    }
                }
                frame_res = stream.next_frame() => {
                    let Some(rc) = weak_client.upgrade() else {
                        return;
                    };
                    match frame_res {
                        Ok(Some(PushFrame::Heartbeat)) => {
                            let mut guard = rc.hosts.write().unwrap();
                            if let Some(slot) = guard.get_mut(&host_id) {
                                slot.last_synced_at = Some(rc.now_ms());
                            }
                        }
                        Ok(Some(PushFrame::StateChanged { topic, card_id, marker })) => {
                            let rev = &marker["revision"];
                            let same_marker = {
                                let guard = rc.hosts.read().unwrap();
                                guard.get(&host_id).is_some_and(|s| {
                                    topic != "resync"
                                        && (cursor_already_covered_by(
                                            rev,
                                            &s.last_marker["revision"],
                                        ) || (is_runtime_topic(topic)
                                            && cursor_already_covered_by(
                                                rev,
                                                &s.last_runtime_marker["revision"],
                                            )))
                                })
                            };
                            if same_marker {
                                continue;
                            }
                            PendingRefresh::schedule_push(
                                &mut pending,
                                topic,
                                card_id,
                                marker,
                                tokio::time::Instant::now(),
                            );
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
    use crate::store::MemoryCredentialStore;
    use canban_types::{Card, Project, TerminalEntry, now};
    use tokio::time::Instant;

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
                        "connection_state": "connected",
                        "observed": { "state": "live", "status": { "state": "working" } },
                        "lifecycle": { "observed_at": now() },
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
        assert!(
            !snap.reminders.iter().any(|r| r.id.starts_with("recovery:")),
            "healthy connected runtime must not emit recovery reminder: {:?}",
            snap.reminders
        );

        // Regression test: pushing a runtime delta with `connection_state: "unavailable"`
        // must immediately reconcile `recovery:*` reminders into `merged_snapshot()` without
        // waiting on a full `app.state` refresh, and clear them once reconnected!
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
                        "state": "running",
                        "connection_state": "unavailable",
                        "instruction": "Do work",
                        "cwd": "/srv/durable",
                        "terminal": "t-rt-1",
                        "started_at": 1750000000123_u64,
                    },
                    "created_at": 1001,
                }))
                .unwrap(),
            ],
            terminal_hosts: BTreeMap::from([("t-rt-1".into(), "local".into())]),
            host_active_agents: BTreeMap::from([("local".into(), 1)]),
            ..Default::default()
        };
        let _ = push_tx.send((
            "terminals".into(),
            json!(["srv-runtime-mock", "i1", 3, 3, 0]),
        ));
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for recovery runtime push"
            );
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
        assert_eq!(app_state_calls.load(Ordering::SeqCst), baseline_app_calls);
        let snap_unavailable = client.merged_snapshot().unwrap();
        assert!(
            snap_unavailable
                .reminders
                .iter()
                .any(|r| r.id.starts_with("recovery:") && r.card_id == "c-rt"),
            "runtime.state delta must reconcile recovery reminder into merged_snapshot(); got {:?}",
            snap_unavailable.reminders
        );

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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_push_marker_does_not_suppress_same_revision_application_push() {
        use canban_transport::{
            AcceptedStream, DeviceTokenStore, MAX_FRAME_BYTES, PairingCodeStore, RevocationBus,
            ServerTransportConfig, TailscaleSessionRegistry, read_bounded_line, spawn_tcp_listener,
            write_json_line_async,
        };
        use canban_types::Workflow;
        use std::sync::atomic::AtomicUsize;
        use tokio::io::BufReader;

        let dir = tempfile::tempdir().unwrap();
        let server_home = dir.path().join("server");
        std::fs::create_dir_all(&server_home).unwrap();
        let store = Arc::new(DeviceTokenStore::open(&server_home).with_fs_sync(false));
        let (_dev, token) = store.issue_device("race-test").unwrap();
        let pairing_codes = Arc::new(PairingCodeStore::open(&server_home).with_fs_sync(false));
        let revocations = RevocationBus::new();
        let active_tcp = Arc::new(AtomicUsize::new(0));
        let (stop_tx, stop_rx) = watch::channel(false);
        let (push_tx, _) = broadcast::channel::<(String, Value)>(16);
        let subscribed_notify = Arc::new(Notify::new());
        let app_state_calls = Arc::new(AtomicUsize::new(0));

        let mut snap_v1 = Snapshot::default();
        snap_v1.projects.push(
            serde_json::from_value(json!({
                "id": "p-race",
                "name": "Race Project",
                "directory": "/srv/race",
                "workflow": Workflow::default(),
                "created_at": 1000,
            }))
            .unwrap(),
        );
        snap_v1.cards.push(
            serde_json::from_value(json!({
                "id": "c-race",
                "project_id": "p-race",
                "number": 1,
                "title": "Before Title",
                "body": "",
                "stage": "todo",
                "revision": 1,
                "created_at": 1000,
            }))
            .unwrap(),
        );
        let current_snap = Arc::new(Mutex::new((
            json!(["srv-race", "i1", 1, 1, 0]),
            serde_json::to_value(&snap_v1).unwrap(),
        )));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let cfg = Arc::new(ServerTransportConfig {
            home: server_home,
            server_id: "srv-race".into(),
            api_version: 90,
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
        let sub_notify = subscribed_notify.clone();
        let snap_ref = current_snap.clone();
        spawn_tcp_listener(
            &handle,
            listener,
            cfg,
            stop_rx,
            move |accepted: AcceptedStream| {
                let mut prx = ptx.subscribe();
                let app_calls = app_calls.clone();
                let sub_notify = sub_notify.clone();
                let snap_ref = snap_ref.clone();
                async move {
                    let (r, mut w) = tokio::io::split(accepted.stream);
                    let mut reader = BufReader::new(r);
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
                                    &mut w,
                                    &json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "result": {
                                            "server_id": "srv-race",
                                            "api_version": 90,
                                            "version": "test",
                                            "platform": "macos",
                                        }
                                    }),
                                )
                                .await;
                            }
                            Some("app.state") => {
                                app_calls.fetch_add(1, Ordering::SeqCst);
                                let (rev, snap_val) = snap_ref.lock().unwrap().clone();
                                let changed = req["params"]["after"] != rev;
                                let mut res = json!({
                                    "changed": changed,
                                    "revision": rev,
                                    "github_revision": 0,
                                    "server_id": "srv-race",
                                });
                                if changed {
                                    res["snapshot"] = snap_val;
                                    res["config"] =
                                        serde_json::to_value(Config::default()).unwrap();
                                    res["home"] = json!("/tmp/srv-race");
                                }
                                let _ = write_json_line_async(
                                    &mut w,
                                    &json!({"jsonrpc": "2.0", "id": id, "result": res}),
                                )
                                .await;
                            }
                            Some("runtime.state") => {
                                let _ = write_json_line_async(
                                    &mut w,
                                    &json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "result": {
                                            "server_id": "srv-race",
                                            "runtime": RuntimeState::default(),
                                        }
                                    }),
                                )
                                .await;
                            }
                            Some("app.subscribe") => {
                                let rev = snap_ref.lock().unwrap().0.clone();
                                let _ = write_json_line_async(
                                    &mut w,
                                    &json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "result": {
                                            "subscribed": true,
                                            "server_id": "srv-race",
                                            "api_version": 90,
                                            "revision": rev,
                                            "changed": false,
                                            "github_revision": 0,
                                            "heartbeat_ms": 5000,
                                        }
                                    }),
                                )
                                .await;
                                sub_notify.notify_one();
                                let mut seq = 1u64;
                                while let Ok((topic, push_rev)) = prx.recv().await {
                                    seq += 1;
                                    if write_json_line_async(
                                        &mut w,
                                        &json!({
                                            "jsonrpc": "2.0",
                                            "method": "event",
                                            "params": {
                                                "seq": seq,
                                                "revision": push_rev,
                                                "topic": topic,
                                                "card_id": Value::Null,
                                                "github_revision": 0,
                                            }
                                        }),
                                    )
                                    .await
                                    .is_err()
                                    {
                                        break;
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

        let client = MultiHostClient::with_runtime(
            RemoteClientConfig::new(dir.path().join("client")),
            Arc::new(MemoryCredentialStore::new()),
            tokio::runtime::Handle::current(),
        )
        .unwrap();
        let mut events = client.subscribe();
        client
            .add_host_async(SavedHost {
                id: "host-race".into(),
                name: "Race Host".into(),
                transport: HostTransport::Tcp { addr, token },
                ssh_target: None,
                server_id: Some("srv-race".into()),
                enabled: true,
                added_at: 1000,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), subscribed_notify.notified())
            .await
            .expect("subscribed");

        let calls_after_boot = app_state_calls.load(Ordering::SeqCst);
        let shared_rev = json!(["srv-race", "i1", 2, 2, 0]);
        {
            let mut snap_v2 = snap_v1;
            snap_v2.cards[0].title = "After Durable Commit".into();
            snap_v2.cards[0].revision = 2;
            *current_snap.lock().unwrap() =
                (shared_rev.clone(), serde_json::to_value(&snap_v2).unwrap());
        }

        // 1. Push `terminals` carrying the already-advanced cursor `shared_rev`.
        let _ = push_tx.send(("terminals".into(), shared_rev.clone()));
        let deadline1 = Instant::now() + Duration::from_secs(3);
        loop {
            let rem = deadline1.saturating_duration_since(Instant::now());
            assert!(!rem.is_zero(), "timed out waiting for terminals push");
            if let Ok(Some(ClientEvent::Host {
                host_id,
                event: AppEvent::Changed { topic, .. },
            })) = tokio::time::timeout(rem, events.recv()).await
                && host_id == "host-race"
                && topic == "terminals"
            {
                break;
            }
        }
        assert_eq!(
            app_state_calls.load(Ordering::SeqCst),
            calls_after_boot,
            "runtime-only push must not trigger full app.state"
        );

        // 2. Push `application` with the EXACT SAME cursor `shared_rev` (the production race when
        //    `on_commit` increments `db_changes` before `handle_subscribe` reads the earlier
        //    `terminals` push). Client MUST NOT drop it as `same_marker` and MUST fetch `app.state`.
        let _ = push_tx.send(("application".into(), shared_rev));
        let deadline2 = Instant::now() + Duration::from_secs(3);
        loop {
            let rem = deadline2.saturating_duration_since(Instant::now());
            assert!(
                !rem.is_zero(),
                "application push with same revision as prior runtime push must not be dropped"
            );
            if let Ok(Some(ClientEvent::Host {
                host_id,
                event: AppEvent::Changed { topic, .. },
            })) = tokio::time::timeout(rem, events.recv()).await
                && host_id == "host-race"
                && topic == "application"
            {
                break;
            }
        }
        assert!(
            app_state_calls.load(Ordering::SeqCst) > calls_after_boot,
            "application push must fetch full app.state"
        );
        let snap = client.merged_snapshot().unwrap();
        assert_eq!(snap.cards[0].title, "After Durable Commit");

        let _ = stop_tx.send(true);
    }

    #[test]
    fn remote_count_active_agents_matches_scheduler_semantics() {
        let now_ms = 1_750_000_050_000_u64;
        let make_term =
            |id: &str, state: &str, conn: &str, obs_state: Option<&str>, obs_at: Option<u64>| {
                let observed =
                    obs_state.map(|s| json!({ "state": "live", "status": { "state": s } }));
                serde_json::from_value::<TerminalEntry>(json!({
                    "id": id,
                    "conversation_id": null,
                    "number": 1,
                    "name": id,
                    "origin_project_id": "p1",
                    "origin_card_id": "c1",
                    "agent_engine": null,
                    "runtime": null,
                    "agent": {
                        "project_id": "p1",
                        "card_id": "c1",
                        "number": 1,
                        "provider": "claude",
                        "state": state,
                        "connection_state": conn,
                        "observed": observed,
                        "lifecycle": { "observed_at": obs_at },
                        "instruction": "Work",
                        "cwd": "/srv/p1",
                        "terminal": id,
                        "started_at": now_ms - 5_000,
                    },
                    "created_at": now_ms - 5_000,
                }))
                .unwrap()
            };

        let mut snap = Snapshot::default();
        // 1. Running terminal whose live observed status is `idle` -> NOT active (releases slot).
        snap.terminals.push(make_term(
            "t-idle",
            "running",
            "connected",
            Some("idle"),
            Some(now_ms - 1_000),
        ));
        // 2. Running terminal whose live observed status is `done` -> NOT active.
        snap.terminals.push(make_term(
            "t-done",
            "running",
            "connected",
            Some("done"),
            Some(now_ms - 1_000),
        ));
        // 3. Running terminal whose live observed status is `blocked` -> NOT active.
        snap.terminals.push(make_term(
            "t-blocked",
            "running",
            "connected",
            Some("blocked"),
            Some(now_ms - 1_000),
        ));
        assert_eq!(count_active_agents(&snap, now_ms), 0);
        assert_eq!(
            count_active_agents(&snap, now_ms),
            canban_scheduler::active_count_at(&snap, now_ms)
        );

        // 4. Actively working terminal -> active (+1).
        snap.terminals.push(make_term(
            "t-work",
            "working",
            "connected",
            Some("working"),
            Some(now_ms - 1_000),
        ));
        // 5. Running terminal with stale observation (>30s) -> conservative active (+1).
        snap.terminals.push(make_term(
            "t-stale",
            "running",
            "connected",
            Some("idle"),
            Some(now_ms - 31_000),
        ));
        // 6. Disconnected terminal -> conservative active (+1).
        snap.terminals
            .push(make_term("t-unavail", "running", "unavailable", None, None));
        assert_eq!(count_active_agents(&snap, now_ms), 3);
        assert_eq!(
            count_active_agents(&snap, now_ms),
            canban_scheduler::active_count_at(&snap, now_ms)
        );
    }

    #[test]
    fn transient_storage_error_classification_and_backoff_bounds() {
        for msg in [
            "injected SQLITE_BUSY fault: Error code 5: The database file is locked",
            "database is locked",
            "Error code 5: The database file is locked",
            "database table is locked",
            "SQLITE_LOCKED: database schema is locked",
            "injected SQLITE_IOERR fault: Error code 10: Some kind of disk I/O error occurred",
            "disk I/O error",
        ] {
            assert!(
                is_transient_storage_error(&anyhow!("{msg}")),
                "expected transient storage error for: {msg}"
            );
        }
        for msg in [
            "Card is archived or removed; restore it before syncing edits",
            "revision_conflict: expected revision 1, current revision 2",
            "request_key_conflict: payload differs",
            "Unknown card 'c-1'",
            "auth_failed: token revoked",
            "Card title is required",
        ] {
            assert!(
                !is_transient_storage_error(&anyhow!("{msg}")),
                "must NOT treat permanent error as transient storage error: {msg}"
            );
        }

        assert_eq!(transient_storage_retry_delay_ms(0), 100);
        assert_eq!(transient_storage_retry_delay_ms(1), 100);
        assert_eq!(transient_storage_retry_delay_ms(2), 200);
        assert_eq!(transient_storage_retry_delay_ms(3), 400);
        assert_eq!(transient_storage_retry_delay_ms(4), 800);
        assert_eq!(transient_storage_retry_delay_ms(5), 1_600);
        assert_eq!(transient_storage_retry_delay_ms(6), 2_000);
        assert_eq!(transient_storage_retry_delay_ms(u32::MAX), 2_000);
    }

    fn make_test_snapshot_with_card(card_id: &str, title: &str, revision: u64) -> Snapshot {
        use canban_types::Workflow;
        let mut snap = Snapshot::default();
        snap.projects.push(
            serde_json::from_value(json!({
                "id": "p-mono",
                "name": "Mono Project",
                "directory": "/srv/mono",
                "workflow": Workflow::default(),
                "created_at": 1000,
            }))
            .unwrap(),
        );
        snap.cards.push(
            serde_json::from_value(json!({
                "id": card_id,
                "project_id": "p-mono",
                "number": 1,
                "title": title,
                "body": "Body",
                "stage": "backlog",
                "revision": revision,
                "created_at": 1000,
            }))
            .unwrap(),
        );
        snap
    }

    #[test]
    fn snapshot_cursor_monotonicity_and_restart_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let saved = SavedHost {
            id: "remote-mono".into(),
            name: "Mono Host".into(),
            transport: HostTransport::Tcp {
                addr: "127.0.0.1:7680".into(),
                token: "dev_tok".into(),
            },
            ssh_target: None,
            server_id: Some("srv-mono".into()),
            enabled: true,
            added_at: 1000,
        };
        let mut slot = RemoteHostSlot::from_saved(
            saved,
            dir.path(),
            DEFAULT_READ_CACHE_BUDGET,
            FsDurability::Full,
            None,
        );
        assert_eq!(slot.cache_gen, 0);

        // Uninitialized / malformed cursors must never block initial snapshot application.
        let snap_rev1 = make_test_snapshot_with_card("c-1", "Rev 1", 1);
        assert!(!is_stale_snapshot_cursor(
            &Value::Null,
            &slot.cached.snapshot,
            &json!(["srv-mono", "inst-1", 1, 1, 0]),
            Some(&snap_rev1),
        ));
        assert!(!is_stale_snapshot_cursor(
            &json!(["", "inst-1", 99, 99, 0]),
            &snap_rev1,
            &json!(["srv-mono", "inst-1", 1, 1, 0]),
            Some(&snap_rev1),
        ));
        assert!(!is_stale_snapshot_cursor(
            &json!([0, "inst-1", 99, 99, 0]),
            &snap_rev1,
            &json!(["srv-mono", "inst-1", 1, 1, 0]),
            Some(&snap_rev1),
        ));

        // 1. Apply rev 3 (cursor changes=12, state_revision=3).
        let state_rev3 = json!({
            "changed": true,
            "revision": ["srv-mono", "inst-1", 12, 3, 100],
            "github_revision": 0,
            "server_id": "srv-mono",
            "snapshot": make_test_snapshot_with_card("c-1", "Rev 3", 3),
            "config": Config::default(),
            "home": "/srv/mono",
        });
        let probe = json!({
            "server_id": "srv-mono",
            "api_version": 90,
            "version": "test",
            "platform": "macos",
        });
        let out1 = apply_state_to_slot(
            &mut slot,
            dir.path(),
            &state_rev3,
            Some(5),
            Some(&probe),
            None,
            1001,
            FsDurability::Full,
        );
        assert!(!out1.stale_rejected);
        assert!(!out1.needs_resync);
        assert_eq!(slot.cache_gen, 1);
        assert_eq!(slot.cached.snapshot.cards[0].revision, 3);
        assert_eq!(slot.cached.snapshot.cards[0].title, "Rev 3");
        assert_eq!(
            slot.last_marker["revision"],
            json!(["srv-mono", "inst-1", 12, 3, 100])
        );

        // 2. Out-of-order older cursor on same `(identity, state_instance)` (rev 2, cursor 10/2)
        // must be rejected without mutating snapshot, marker, or cache_gen.
        let state_rev2_stale = json!({
            "changed": true,
            "revision": ["srv-mono", "inst-1", 10, 2, 90],
            "github_revision": 0,
            "server_id": "srv-mono",
            "snapshot": make_test_snapshot_with_card("c-1", "Stale Rev 2", 2),
            "config": Config::default(),
            "home": "/srv/mono",
        });
        let out2 = apply_state_to_slot(
            &mut slot,
            dir.path(),
            &state_rev2_stale,
            Some(4),
            None,
            None,
            1002,
            FsDurability::Full,
        );
        assert!(out2.stale_rejected);
        assert!(!out2.needs_resync);
        assert_eq!(out2.emitted_topic, None);
        assert_eq!(slot.cache_gen, 1);
        assert_eq!(slot.cached.snapshot.cards[0].revision, 3);
        assert_eq!(slot.cached.snapshot.cards[0].title, "Rev 3");
        assert_eq!(
            slot.last_marker["revision"],
            json!(["srv-mono", "inst-1", 12, 3, 100])
        );

        // 3. Tied cursor (or cursor ahead of stale last_marker) that regresses card revision
        // (e.g., after local write-receipt hydration bumped card to rev 4) must be rejected and
        // request a resync.
        slot.cached.snapshot.cards[0].revision = 4;
        slot.cached.snapshot.cards[0].title = "Hydrated Rev 4".into();
        let state_tied_regressing = json!({
            "changed": true,
            "revision": ["srv-mono", "inst-1", 12, 3, 100],
            "github_revision": 0,
            "server_id": "srv-mono",
            "snapshot": make_test_snapshot_with_card("c-1", "Regressing Rev 3", 3),
            "config": Config::default(),
            "home": "/srv/mono",
        });
        let out3 = apply_state_to_slot(
            &mut slot,
            dir.path(),
            &state_tied_regressing,
            Some(4),
            None,
            None,
            1003,
            FsDurability::Full,
        );
        assert!(out3.stale_rejected);
        assert!(out3.needs_resync);
        assert_eq!(slot.cached.snapshot.cards[0].revision, 4);
        assert_eq!(slot.cached.snapshot.cards[0].title, "Hydrated Rev 4");

        // 4. Tied cursor with non-regressing card revision (e.g., mock server with static cursor)
        // must still be accepted.
        let state_tied_advancing = json!({
            "changed": true,
            "revision": ["srv-mono", "inst-1", 12, 3, 100],
            "github_revision": 0,
            "server_id": "srv-mono",
            "snapshot": make_test_snapshot_with_card("c-1", "Rev 5", 5),
            "config": Config::default(),
            "home": "/srv/mono",
        });
        let out4 = apply_state_to_slot(
            &mut slot,
            dir.path(),
            &state_tied_advancing,
            Some(4),
            None,
            None,
            1004,
            FsDurability::Full,
        );
        assert!(!out4.stale_rejected);
        assert_eq!(slot.cache_gen, 2);
        assert_eq!(slot.cached.snapshot.cards[0].revision, 5);

        // 5. Server reboot (`state_instance` changed to `inst-2`, counters reset to 1/1) MUST
        // replace `slot.cached.snapshot` and `slot.last_marker`.
        let state_post_reboot = json!({
            "changed": true,
            "revision": ["srv-mono", "inst-2", 1, 1, 0],
            "github_revision": 0,
            "server_id": "srv-mono",
            "snapshot": make_test_snapshot_with_card("c-1", "Post Reboot", 1),
            "config": Config::default(),
            "home": "/srv/mono",
        });
        let out5 = apply_state_to_slot(
            &mut slot,
            dir.path(),
            &state_post_reboot,
            Some(3),
            None,
            None,
            1005,
            FsDurability::Full,
        );
        assert!(!out5.stale_rejected);
        assert_eq!(slot.cache_gen, 3);
        assert_eq!(slot.cached.snapshot.cards[0].revision, 1);
        assert_eq!(slot.cached.snapshot.cards[0].title, "Post Reboot");
        assert_eq!(
            slot.last_marker["revision"],
            json!(["srv-mono", "inst-2", 1, 1, 0])
        );

        // 6. `preserve_monotonic_card_revisions` blocks idempotent replay rev regression.
        let current_rev3 = make_test_snapshot_with_card("c-1", "Current Rev 3", 3);
        let mut replay_rev2 = make_test_snapshot_with_card("c-1", "Old Replay Rev 2", 2);
        preserve_monotonic_card_revisions(&current_rev3, &mut replay_rev2);
        assert_eq!(replay_rev2.cards[0].revision, 3);
        assert_eq!(replay_rev2.cards[0].title, "Current Rev 3");
    }

    #[test]
    fn push_refresh_retry_state_machine_and_backoff_bounds() {
        assert_eq!(push_refresh_retry_delay_ms(0), 200);
        assert_eq!(push_refresh_retry_delay_ms(1), 200);
        assert_eq!(push_refresh_retry_delay_ms(2), 400);
        assert_eq!(push_refresh_retry_delay_ms(3), 800);
        assert_eq!(push_refresh_retry_delay_ms(4), 1_600);
        assert_eq!(push_refresh_retry_delay_ms(5), 2_000);
        assert_eq!(push_refresh_retry_delay_ms(u32::MAX), 2_000);

        let t0 = tokio::time::Instant::now();
        let mut pending: Option<PendingRefresh> = None;

        // Initial push schedules debounce at +30ms (max +100ms).
        PendingRefresh::schedule_push(
            &mut pending,
            "application",
            Some("c-1".into()),
            json!({"revision": ["srv", "i1", 2, 2, 0], "github_revision": 0}),
            t0,
        );
        let p = pending.as_ref().unwrap();
        assert_eq!(p.deadline, t0 + PUSH_DEBOUNCE);
        assert_eq!(p.max_deadline, t0 + PUSH_MAX_DEBOUNCE);
        assert_eq!(p.retry_attempts, 0);

        // Transient SQLITE_BUSY re-queues with bounded exponential backoff and keeps retrying
        // past the non-storage retry cap while connected.
        let busy_err =
            anyhow!("injected SQLITE_BUSY fault: Error code 5: The database file is locked");
        let mut cur = pending.take().unwrap();
        let mut now = t0 + PUSH_DEBOUNCE;
        let expected_delays = [
            200, 400, 800, 1_600, 2_000, 2_000, 2_000, 2_000, 2_000, 2_000,
        ];
        for (idx, expected_ms) in expected_delays.iter().enumerate() {
            cur = cur
                .requeue_after_error(&busy_err, now)
                .expect("transient storage error must keep re-scheduling");
            assert_eq!(cur.retry_attempts, (idx + 1) as u32);
            assert_eq!(cur.deadline, now + Duration::from_millis(*expected_ms));
            now = cur.deadline;
        }

        // A newer push frame arriving during backoff merges topic/marker WITHOUT resetting the
        // backoff deadline back to 30ms (prevents busy-spinning during a fault storm).
        let backoff_deadline = cur.deadline;
        let mut backed_off = Some(cur);
        PendingRefresh::schedule_push(
            &mut backed_off,
            "github",
            Some("c-2".into()),
            json!({"revision": ["srv", "i1", 5, 5, 0], "github_revision": 1}),
            now - Duration::from_millis(500),
        );
        let merged = backed_off.unwrap();
        assert_eq!(merged.topic, "github");
        assert_eq!(merged.card_id, None);
        assert_eq!(
            merged.latest_marker["revision"],
            json!(["srv", "i1", 5, 5, 0])
        );
        assert_eq!(merged.deadline, backoff_deadline);

        // Non-storage RPC errors also retry with bounded backoff, then cap cleanly.
        let generic_err = anyhow!("temporary internal rpc error");
        let mut generic = PendingRefresh::seed_retry(
            "application",
            None,
            json!({"revision": ["srv", "i1", 6, 6, 0], "github_revision": 0}),
            &generic_err,
            t0,
        )
        .expect("first retry scheduled");
        assert_eq!(generic.retry_attempts, 1);
        for attempt in 2..=PUSH_REFRESH_MAX_NON_STORAGE_RETRIES {
            let next_now = generic.deadline;
            generic = generic
                .requeue_after_error(&generic_err, next_now)
                .expect("within non-storage retry cap");
            assert_eq!(generic.retry_attempts, attempt);
        }
        let final_now = generic.deadline;
        assert!(
            generic
                .requeue_after_error(&generic_err, final_now)
                .is_none(),
            "non-storage error must cap retries to avoid infinite loops"
        );
    }
}
