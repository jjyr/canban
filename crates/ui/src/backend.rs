//! Transport-agnostic UI backend abstraction (`UiBackend` + `BackendEvent`).
//!
//! All RPC routing, merged snapshots, host summaries, host configs, event
//! subscriptions, and draft persistence route through [`UiBackend`] backed by
//! the single unified [`ClientBackend`] wrapping [`canban_client::MultiHostClient`]
//! for both Desktop (with local host driver + SSH/discovery providers) and Mobile
//! (pure-remote TCP + Tailscale `ts_auto`).

use anyhow::Result;
use canban_types::{
    AppEvent, Config, HostSummary, RuntimeState, SavedHost, Snapshot, SyncAction, SyncItemView,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

/// Host-scoped event or side-channel push payload delivered by a [`UiBackend`] subscription.
#[derive(Clone, Debug)]
pub enum BackendEvent {
    Host { host_id: String, event: AppEvent },
    UpdateChanged { host_id: String, state: Value },
    UtilitiesChanged { host_id: String, state: Value },
    UsageChanged { host_id: String, state: Value },
    HerdrInstall { host_id: String, state: Value },
}

impl BackendEvent {
    pub fn from_client_event(ev: canban_client::ClientEvent) -> Self {
        match ev {
            canban_client::ClientEvent::Host { host_id, event } => Self::Host { host_id, event },
            canban_client::ClientEvent::UpdateChanged { host_id, state } => {
                Self::UpdateChanged { host_id, state }
            }
            canban_client::ClientEvent::UtilitiesChanged { host_id, state } => {
                Self::UtilitiesChanged { host_id, state }
            }
            canban_client::ClientEvent::UsageChanged { host_id, state } => {
                Self::UsageChanged { host_id, state }
            }
            canban_client::ClientEvent::HerdrInstall { host_id, state } => {
                Self::HerdrInstall { host_id, state }
            }
        }
    }

    pub fn local(event: AppEvent) -> Self {
        Self::Host {
            host_id: "local".into(),
            event,
        }
    }

    pub fn host(host_id: impl Into<String>, event: AppEvent) -> Self {
        Self::Host {
            host_id: host_id.into(),
            event,
        }
    }

    pub fn host_id(&self) -> &str {
        match self {
            Self::Host { host_id, .. }
            | Self::UpdateChanged { host_id, .. }
            | Self::UtilitiesChanged { host_id, .. }
            | Self::UsageChanged { host_id, .. }
            | Self::HerdrInstall { host_id, .. } => host_id,
        }
    }

    pub fn app_event(&self) -> Option<&AppEvent> {
        match self {
            Self::Host { event, .. } => Some(event),
            _ => None,
        }
    }
}

/// Transport-agnostic backend interface consumed by `canban-ui` screens and presenters.
pub trait UiBackend: Send + Sync + 'static {
    /// Route an RPC call to the appropriate host based on `method`, `params`, and
    /// `active_project_id`.
    fn call_routed(
        &self,
        method: &str,
        params: Value,
        active_project_id: Option<&str>,
    ) -> Result<Value>;

    /// Send an RPC call directly to the primary/local host (used for local
    /// preferences, drafts, GitHub CLI, and updater queries).
    fn call_local(&self, method: &str, params: Value) -> Result<Value> {
        self.call_routed(method, params, None)
    }

    /// Broadcast an RPC call to every connected host.
    fn call_all_hosts(&self, method: &str, params: Value) -> Vec<(String, Result<Value>)>;

    /// Fetch the merged multi-host workspace snapshot.
    fn merged_snapshot(&self) -> Result<Snapshot>;

    /// Fetch the lightweight merged runtime slice (`terminals`, `terminal_contexts`,
    /// `reclaim_status`, `merging_cards`). Returns `Ok(None)` when unsupported.
    fn merged_runtime_state(&self) -> Result<Option<RuntimeState>> {
        Ok(None)
    }

    /// Return summaries for all known hosts.
    fn host_summaries(&self) -> Vec<HostSummary>;

    /// Return the effective configuration for `host_id`.
    fn config_for_host(&self, host_id: &str) -> Config;

    /// Optional explicit PTY transport override for `host_id` (used by pure-remote clients).
    fn terminal_transport(
        &self,
        _host_id: &str,
    ) -> Option<std::sync::Arc<dyn canban_terminal::session::PtyTransport>> {
        None
    }

    /// Resolve which host ID an RPC call will target.
    fn resolve_target_host(
        &self,
        method: &str,
        params: &Value,
        active_project_id: Option<&str>,
    ) -> String;

    /// Optional banner notice reported by the connected server.
    fn server_notice(&self) -> Option<String> {
        None
    }

    /// Subscribe to async host/application change events and side-channel push updates.
    fn subscribe(&self) -> tokio::sync::mpsc::UnboundedReceiver<BackendEvent>;

    /// Return the display home directory path for `host_id` (e.g. `~/.canban`).
    fn home_for(&self, host_id: &str) -> String {
        if host_id.is_empty() || host_id == "local" {
            "/preview/.canban".into()
        } else {
            "/home/builder/.canban".into()
        }
    }

    /// Local filesystem home directory, when backed by a local desktop daemon.
    fn local_home(&self) -> Option<PathBuf> {
        None
    }

    /// Whether this backend supports multi-host registry management operations.
    fn has_host_registry(&self) -> bool {
        false
    }

    fn saved_hosts(&self) -> Vec<SavedHost> {
        Vec::new()
    }

    fn add_host(&self, _host: SavedHost) -> Result<HostSummary> {
        anyhow::bail!("Host management is not supported by this backend")
    }

    fn add_host_with_pairing_code(
        &self,
        host: SavedHost,
        _pairing_code: Option<&str>,
    ) -> Result<HostSummary> {
        self.add_host(host)
    }

    fn remove_host(&self, _host_id: &str) -> Result<()> {
        anyhow::bail!("Host management is not supported by this backend")
    }

    fn rename_host(&self, _host_id: &str, _new_name: &str) -> Result<()> {
        anyhow::bail!("Host management is not supported by this backend")
    }

    fn set_host_enabled(&self, _host_id: &str, _enabled: bool) -> Result<()> {
        anyhow::bail!("Host management is not supported by this backend")
    }

    fn reconnect_host(&self, _host_id: Option<&str>) {}

    fn poll_remotes_once(&self) {}

    fn update_remote_daemon(&self, _host_id: &str) -> Result<Value> {
        anyhow::bail!("Remote daemon updates are not supported by this backend")
    }

    fn sync_items(&self) -> Vec<SyncItemView> {
        Vec::new()
    }

    fn sync_action(&self, _host_id: &str, _mutation_id: &str, _action: SyncAction) -> Result<()> {
        anyhow::bail!("Offline sync is not supported by this backend")
    }

    fn global_env_sync_summary(&self) -> canban_types::GlobalEnvSyncSummary {
        Default::default()
    }

    fn global_env_sync_status(&self) -> canban_types::GlobalEnvSyncStatus {
        let summary = self.global_env_sync_summary();
        canban_types::GlobalEnvSyncStatus {
            total_remote_hosts: summary.total_remote_hosts,
            synced_hosts: (0..summary.synced_hosts)
                .map(|i| format!("Host {}", i + 1))
                .collect(),
            syncing_hosts: Vec::new(),
            queued_hosts: summary.pending_hosts,
            unsynced_hosts: Vec::new(),
            failed_hosts: Vec::new(),
        }
    }

    fn trigger_global_env_sync(&self) -> Result<()> {
        anyhow::bail!("Global environment sync is not supported by this backend")
    }

    /// Returns runtime capabilities supported by this backend (local daemon, SSH, local discovery, updater).
    fn capabilities(&self) -> canban_client::ClientCapabilities {
        canban_client::ClientCapabilities::mobile()
    }

    /// Optional local discovery provider (`tailscale status` + `~/.ssh/config`) when supported.
    fn discovery_provider(&self) -> Option<Arc<dyn canban_client::LocalDiscoveryProvider>> {
        None
    }

    /// Optional SSH pairing/bootstrap capability provider when supported.
    fn ssh_provider(&self) -> Option<Arc<dyn canban_client::SshCapabilityProvider>> {
        None
    }

    /// Notifies a pure-remote client that the mobile/desktop app returned to foreground.
    fn on_foreground(&self) {}

    /// Returns the underlying [`canban_client::RemoteClient`] when running pure-remote.
    fn remote_client(&self) -> Option<Arc<canban_client::RemoteClient>> {
        None
    }
}

static BACKEND: OnceLock<Arc<dyn UiBackend>> = OnceLock::new();

/// Install the active [`UiBackend`] for this UI process.
pub fn set(backend: Arc<dyn UiBackend>) -> std::result::Result<(), Arc<dyn UiBackend>> {
    BACKEND.set(backend)
}

/// Return the currently registered [`UiBackend`], if any.
pub fn get() -> Option<Arc<dyn UiBackend>> {
    BACKEND.get().cloned()
}

/// Returns `true` when a live [`UiBackend`] is connected (as opposed to Gallery preview).
pub fn is_connected() -> bool {
    BACKEND.get().is_some()
}

pub fn call_routed(method: &str, params: Value, active_project_id: Option<&str>) -> Result<Value> {
    let params = crate::with_ui_projection(method, params);
    let backend = BACKEND
        .get()
        .ok_or_else(|| anyhow::anyhow!("Gallery preview: no AppServer connected"))?;
    backend.call_routed(method, params, active_project_id)
}

pub fn call_local(method: &str, params: Value) -> Result<Value> {
    let params = crate::with_ui_projection(method, params);
    let backend = BACKEND
        .get()
        .ok_or_else(|| anyhow::anyhow!("Gallery preview: no AppServer connected"))?;
    backend.call_local(method, params)
}

pub fn call_all_hosts(method: &str, params: Value) -> Vec<(String, Result<Value>)> {
    let params = crate::with_ui_projection(method, params);
    if let Some(backend) = BACKEND.get() {
        return backend.call_all_hosts(method, params);
    }
    vec![(
        "local".to_string(),
        Err(anyhow::anyhow!("Gallery preview: no AppServer connected")),
    )]
}

pub fn merged_snapshot() -> Result<Snapshot> {
    let backend = BACKEND
        .get()
        .ok_or_else(|| anyhow::anyhow!("Gallery preview: no AppServer connected"))?;
    backend.merged_snapshot()
}

pub fn merged_runtime_state() -> Result<Option<RuntimeState>> {
    let backend = BACKEND
        .get()
        .ok_or_else(|| anyhow::anyhow!("Gallery preview: no AppServer connected"))?;
    backend.merged_runtime_state()
}

pub fn host_summaries() -> Vec<HostSummary> {
    BACKEND
        .get()
        .map(|b| b.host_summaries())
        .unwrap_or_default()
}

pub fn config_for_host(host_id: &str) -> Config {
    BACKEND
        .get()
        .map(|b| b.config_for_host(host_id))
        .unwrap_or_default()
}

pub fn terminal_transport(
    host_id: &str,
) -> Option<std::sync::Arc<dyn canban_terminal::session::PtyTransport>> {
    BACKEND.get().and_then(|b| b.terminal_transport(host_id))
}

pub fn resolve_target_host(
    method: &str,
    params: &Value,
    active_project_id: Option<&str>,
) -> String {
    BACKEND
        .get()
        .map(|b| b.resolve_target_host(method, params, active_project_id))
        .unwrap_or_else(|| "local".into())
}

pub fn server_notice() -> Option<String> {
    BACKEND.get().and_then(|b| b.server_notice())
}

pub fn subscribe() -> Option<tokio::sync::mpsc::UnboundedReceiver<BackendEvent>> {
    BACKEND.get().map(|b| b.subscribe())
}

pub fn home_for(host_id: &str) -> String {
    if let Some(backend) = BACKEND.get() {
        return backend.home_for(host_id);
    }
    if host_id.is_empty() || host_id == "local" {
        "/preview/.canban".into()
    } else {
        "/home/builder/.canban".into()
    }
}

pub fn local_home() -> Option<PathBuf> {
    BACKEND.get().and_then(|b| b.local_home())
}

pub fn sync_items() -> Vec<SyncItemView> {
    BACKEND.get().map(|b| b.sync_items()).unwrap_or_default()
}

pub fn sync_action(host_id: &str, mutation_id: &str, action: SyncAction) -> Result<()> {
    let backend = BACKEND
        .get()
        .ok_or_else(|| anyhow::anyhow!("Gallery preview: no AppServer connected"))?;
    backend.sync_action(host_id, mutation_id, action)
}

pub fn global_env_sync_summary() -> canban_types::GlobalEnvSyncSummary {
    BACKEND
        .get()
        .map(|b| b.global_env_sync_summary())
        .unwrap_or_default()
}

pub fn global_env_sync_status() -> canban_types::GlobalEnvSyncStatus {
    BACKEND
        .get()
        .map(|b| b.global_env_sync_status())
        .unwrap_or_default()
}

pub fn trigger_global_env_sync() -> Result<()> {
    let backend = BACKEND
        .get()
        .ok_or_else(|| anyhow::anyhow!("Gallery preview: no AppServer connected"))?;
    backend.trigger_global_env_sync()
}

pub fn on_foreground() {
    if let Some(backend) = BACKEND.get() {
        backend.on_foreground();
    }
}

pub fn remote_client() -> Option<Arc<canban_client::RemoteClient>> {
    BACKEND.get().and_then(|b| b.remote_client())
}

/// Runtime capabilities of the active [`UiBackend`] (falls back to compile-time defaults in Gallery preview).
pub fn capabilities() -> canban_client::ClientCapabilities {
    if let Some(backend) = BACKEND.get() {
        return backend.capabilities();
    }
    #[cfg(feature = "local-host")]
    {
        canban_client::ClientCapabilities::desktop()
    }
    #[cfg(not(feature = "local-host"))]
    {
        canban_client::ClientCapabilities::mobile()
    }
}

/// Optional local host discovery provider (`tailscale status` + `~/.ssh/config`) from the active backend.
pub fn discovery_provider() -> Option<Arc<dyn canban_client::LocalDiscoveryProvider>> {
    BACKEND.get().and_then(|b| b.discovery_provider())
}

/// Optional SSH pairing & bootstrap provider from the active backend.
pub fn ssh_provider() -> Option<Arc<dyn canban_client::SshCapabilityProvider>> {
    BACKEND.get().and_then(|b| b.ssh_provider())
}

/// Desktop API version used for compatibility displays and fixture metadata.
pub(crate) fn desktop_api() -> u32 {
    #[cfg(feature = "local-host")]
    {
        canban_updater::DESKTOP_API
    }
    #[cfg(not(feature = "local-host"))]
    {
        0
    }
}

/// Current build metadata serialized as JSON for UI status/update panels.
pub(crate) fn build_info() -> Value {
    #[cfg(feature = "local-host")]
    {
        json!(canban_updater::Build::current())
    }
    #[cfg(not(feature = "local-host"))]
    {
        json!({
            "version": build_version(),
            "commit": build_commit(),
            "schema": 0,
            "api": desktop_api(),
            "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        })
    }
}

pub(crate) fn build_version() -> String {
    #[cfg(feature = "local-host")]
    {
        canban_updater::Build::current().version
    }
    #[cfg(not(feature = "local-host"))]
    {
        option_env!("CANBAN_RELEASE_VERSION")
            .unwrap_or("development")
            .into()
    }
}

pub(crate) fn build_commit() -> String {
    #[cfg(feature = "local-host")]
    {
        canban_updater::Build::current().commit
    }
    #[cfg(not(feature = "local-host"))]
    {
        option_env!("CANBAN_RELEASE_COMMIT")
            .unwrap_or("unknown")
            .into()
    }
}

/// Count currently active agent execution slots from a [`Snapshot`].
pub(crate) fn active_agent_count(snapshot: &Snapshot) -> usize {
    #[cfg(feature = "local-host")]
    {
        canban_scheduler::active_count(snapshot)
    }
    #[cfg(not(feature = "local-host"))]
    {
        let bound = snapshot
            .terminals
            .iter()
            .filter_map(|t| t.agent.as_ref())
            .filter(|a| {
                (a.terminal.is_some()
                    && !a.finished()
                    && (a.close_state == "closing"
                        || !matches!(
                            a.runtime_status(),
                            "closed" | "blocked" | "idle" | "done" | "error"
                        )))
                    || (a.terminal.is_none() && matches!(a.state.as_str(), "starting" | "unknown"))
            })
            .count();
        bound
            + snapshot
                .configuration_sessions
                .iter()
                .filter(|s| {
                    matches!(
                        s.state.as_str(),
                        "starting" | "unknown" | "running" | "working"
                    )
                })
                .count()
            + snapshot
                .merge_batches
                .iter()
                .filter(|b| {
                    matches!(
                        b.state.as_str(),
                        "starting" | "unknown" | "running" | "working"
                    )
                })
                .count()
            + snapshot
                .scheduled_runs
                .iter()
                .filter(|r| {
                    !r.creates_card
                        && r.terminal_id.is_some()
                        && matches!(
                            r.state.as_str(),
                            "starting" | "unknown" | "running" | "working"
                        )
                })
                .count()
    }
}

/// Unified [`UiBackend`] backed by [`canban_client::MultiHostClient`] for both Desktop
/// (with a built-in `"local"` host driver and optional SSH/discovery capability providers)
/// and Mobile (pure-remote TCP + Tailscale `ts_auto`, zero local server/SQLite/subprocess deps).
pub struct ClientBackend {
    client: Arc<canban_client::MultiHostClient>,
    discovery_provider: Option<Arc<dyn canban_client::LocalDiscoveryProvider>>,
    ssh_provider: Option<Arc<dyn canban_client::SshCapabilityProvider>>,
}

/// Backward-compatible alias for mobile callers (`canban-ios`, `canban-android`).
pub type RemoteBackend = ClientBackend;

impl ClientBackend {
    /// Constructs a pure-remote [`ClientBackend`] without desktop local-discovery or SSH providers.
    pub fn new(client: Arc<canban_client::MultiHostClient>) -> Self {
        Self {
            client,
            discovery_provider: None,
            ssh_provider: None,
        }
    }

    /// Constructs a [`ClientBackend`] with optional desktop platform capability providers.
    pub fn with_providers(
        client: Arc<canban_client::MultiHostClient>,
        discovery_provider: Option<Arc<dyn canban_client::LocalDiscoveryProvider>>,
        ssh_provider: Option<Arc<dyn canban_client::SshCapabilityProvider>>,
    ) -> Self {
        Self {
            client,
            discovery_provider,
            ssh_provider,
        }
    }

    pub fn client(&self) -> &Arc<canban_client::MultiHostClient> {
        &self.client
    }
}

impl UiBackend for ClientBackend {
    fn call_routed(
        &self,
        method: &str,
        params: Value,
        active_project_id: Option<&str>,
    ) -> Result<Value> {
        self.client.call_routed(method, params, active_project_id)
    }

    fn call_local(&self, method: &str, params: Value) -> Result<Value> {
        self.client.call_local(method, params)
    }

    fn call_all_hosts(&self, method: &str, params: Value) -> Vec<(String, Result<Value>)> {
        self.client.call_all_hosts(method, params)
    }

    fn merged_snapshot(&self) -> Result<Snapshot> {
        self.client.merged_snapshot()
    }

    fn merged_runtime_state(&self) -> Result<Option<RuntimeState>> {
        Ok(Some(self.client.merged_runtime_state()?))
    }

    fn host_summaries(&self) -> Vec<HostSummary> {
        self.client.host_summaries()
    }

    fn config_for_host(&self, host_id: &str) -> Config {
        self.client.config_for_host(host_id)
    }

    fn terminal_transport(
        &self,
        host_id: &str,
    ) -> Option<std::sync::Arc<dyn canban_terminal::session::PtyTransport>> {
        self.client.terminal_transport(host_id)
    }

    fn resolve_target_host(
        &self,
        method: &str,
        params: &Value,
        active_project_id: Option<&str>,
    ) -> String {
        self.client
            .resolve_target_host(method, params, active_project_id)
    }

    fn server_notice(&self) -> Option<String> {
        self.client.server_notice()
    }

    fn subscribe(&self) -> tokio::sync::mpsc::UnboundedReceiver<BackendEvent> {
        self.client
            .subscribe_mapped(BackendEvent::from_client_event)
    }

    fn home_for(&self, host_id: &str) -> String {
        self.client.home_for(host_id)
    }

    fn local_home(&self) -> Option<PathBuf> {
        self.client.local_home()
    }

    fn has_host_registry(&self) -> bool {
        true
    }

    fn saved_hosts(&self) -> Vec<SavedHost> {
        self.client.saved_hosts()
    }

    fn add_host(&self, host: SavedHost) -> Result<HostSummary> {
        self.client.add_host(host)
    }

    fn add_host_with_pairing_code(
        &self,
        host: SavedHost,
        pairing_code: Option<&str>,
    ) -> Result<HostSummary> {
        self.client
            .add_host_with_pairing_code(host, pairing_code.map(str::to_owned))
    }

    fn remove_host(&self, host_id: &str) -> Result<()> {
        self.client.remove_host(host_id)
    }

    fn rename_host(&self, host_id: &str, new_name: &str) -> Result<()> {
        self.client.rename_host(host_id, new_name)
    }

    fn set_host_enabled(&self, host_id: &str, enabled: bool) -> Result<()> {
        self.client.set_host_enabled(host_id, enabled)
    }

    fn reconnect_host(&self, host_id: Option<&str>) {
        self.client.reconnect(host_id);
    }

    fn poll_remotes_once(&self) {
        self.client.poll_remotes_once();
    }

    fn update_remote_daemon(&self, host_id: &str) -> Result<Value> {
        self.client.update_remote_daemon(host_id)
    }

    fn sync_items(&self) -> Vec<SyncItemView> {
        self.client.sync_items()
    }

    fn sync_action(&self, host_id: &str, mutation_id: &str, action: SyncAction) -> Result<()> {
        self.client.sync_action(host_id, mutation_id, action)
    }

    fn global_env_sync_summary(&self) -> canban_types::GlobalEnvSyncSummary {
        self.client.global_env_sync_summary()
    }

    fn global_env_sync_status(&self) -> canban_types::GlobalEnvSyncStatus {
        self.client.global_env_sync_status()
    }

    fn trigger_global_env_sync(&self) -> Result<()> {
        self.client.sync_global_env_now();
        Ok(())
    }

    fn capabilities(&self) -> canban_client::ClientCapabilities {
        self.client.capabilities()
    }

    fn discovery_provider(&self) -> Option<Arc<dyn canban_client::LocalDiscoveryProvider>> {
        self.discovery_provider.clone()
    }

    fn ssh_provider(&self) -> Option<Arc<dyn canban_client::SshCapabilityProvider>> {
        self.ssh_provider.clone()
    }

    fn on_foreground(&self) {
        self.client.on_foreground();
    }

    fn remote_client(&self) -> Option<Arc<canban_client::RemoteClient>> {
        (!self.client.has_local_host()).then(|| self.client.clone())
    }
}
