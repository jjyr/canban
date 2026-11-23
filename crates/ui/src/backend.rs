//! Transport-agnostic UI backend abstraction (`UiBackend` + `BackendEvent`).
//!
//! All RPC routing, merged snapshots, host summaries, host configs, event
//! subscriptions, and draft persistence route through [`UiBackend`] instead of
//! scattered `AppClient` / `HostRegistry` globals. The desktop [`LocalBackend`]
//! sits behind the `local-host` Cargo feature so pure UI builds
//! (`--no-default-features`) remain free of local server, SQLite, updater, and
//! subprocess crates.

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

#[cfg(feature = "local-host")]
pub struct LocalBackend {
    app: Arc<canban_app_server::AppClient>,
    hosts: Option<Arc<canban_app_server::hosts::HostRegistry>>,
}

#[cfg(feature = "local-host")]
impl LocalBackend {
    pub fn with_hosts(
        app: Arc<canban_app_server::AppClient>,
        hosts: Arc<canban_app_server::hosts::HostRegistry>,
    ) -> Self {
        Self {
            app,
            hosts: Some(hosts),
        }
    }

    pub fn from_client(app: Arc<canban_app_server::AppClient>) -> Self {
        Self { app, hosts: None }
    }
}

#[cfg(feature = "local-host")]
impl UiBackend for LocalBackend {
    fn call_routed(
        &self,
        method: &str,
        params: Value,
        active_project_id: Option<&str>,
    ) -> Result<Value> {
        if let Some(hosts) = &self.hosts {
            hosts.call_routed(method, params, active_project_id)
        } else {
            self.app.call(method, params)
        }
    }

    fn call_local(&self, method: &str, params: Value) -> Result<Value> {
        self.app.call(method, params)
    }

    fn call_all_hosts(&self, method: &str, params: Value) -> Vec<(String, Result<Value>)> {
        if let Some(hosts) = &self.hosts {
            hosts.call_all_hosts(method, params)
        } else {
            vec![("local".to_string(), self.app.call(method, params))]
        }
    }

    fn merged_snapshot(&self) -> Result<Snapshot> {
        if let Some(hosts) = &self.hosts {
            hosts.merged_snapshot()
        } else {
            self.app.snapshot()
        }
    }

    fn merged_runtime_state(&self) -> Result<Option<RuntimeState>> {
        if let Some(hosts) = &self.hosts {
            Ok(Some(hosts.merged_runtime_state()?))
        } else {
            Ok(Some(self.app.runtime_state()?))
        }
    }

    fn host_summaries(&self) -> Vec<HostSummary> {
        self.hosts
            .as_ref()
            .map(|h| h.summaries())
            .unwrap_or_default()
    }

    fn config_for_host(&self, host_id: &str) -> Config {
        if let Some(hosts) = &self.hosts {
            hosts.config_for(host_id)
        } else {
            self.app.config()
        }
    }

    fn resolve_target_host(
        &self,
        method: &str,
        params: &Value,
        active_project_id: Option<&str>,
    ) -> String {
        if let Some(hosts) = &self.hosts {
            hosts.resolve_target_host(method, params, active_project_id)
        } else {
            "local".into()
        }
    }

    fn server_notice(&self) -> Option<String> {
        self.app.server_notice().map(str::to_owned)
    }

    fn subscribe(&self) -> tokio::sync::mpsc::UnboundedReceiver<BackendEvent> {
        if let Some(hosts) = &self.hosts {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let mut upstream = hosts.subscribe_push();
            drop(hosts.runtime_handle().spawn(async move {
                loop {
                    let mapped = match upstream.recv().await {
                        Ok(canban_app_server::hosts::HostPushEvent::State(ev)) => {
                            BackendEvent::Host {
                                host_id: ev.host_id,
                                event: ev.event,
                            }
                        }
                        Ok(canban_app_server::hosts::HostPushEvent::UpdateChanged {
                            host_id,
                            state,
                        }) => BackendEvent::UpdateChanged { host_id, state },
                        Ok(canban_app_server::hosts::HostPushEvent::UtilitiesChanged {
                            host_id,
                            state,
                        }) => BackendEvent::UtilitiesChanged { host_id, state },
                        Ok(canban_app_server::hosts::HostPushEvent::UsageChanged {
                            host_id,
                            state,
                        }) => BackendEvent::UsageChanged { host_id, state },
                        Ok(canban_app_server::hosts::HostPushEvent::HerdrInstall {
                            host_id,
                            state,
                        }) => BackendEvent::HerdrInstall { host_id, state },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            BackendEvent::local(AppEvent::Changed {
                                topic: "resync",
                                card_id: None,
                            })
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    };
                    if tx.send(mapped).is_err() {
                        break;
                    }
                }
            }));
            rx
        } else {
            self.app.subscribe_mapped(
                &["snapshot", "update", "utilities", "usage", "herdr"],
                |ev| match ev {
                    canban_app_server::ClientPushEvent::State(app_ev) => {
                        BackendEvent::local(app_ev)
                    }
                    canban_app_server::ClientPushEvent::UpdateChanged(state) => {
                        BackendEvent::UpdateChanged {
                            host_id: "local".into(),
                            state,
                        }
                    }
                    canban_app_server::ClientPushEvent::UtilitiesChanged(state) => {
                        BackendEvent::UtilitiesChanged {
                            host_id: "local".into(),
                            state,
                        }
                    }
                    canban_app_server::ClientPushEvent::UsageChanged(state) => {
                        BackendEvent::UsageChanged {
                            host_id: "local".into(),
                            state,
                        }
                    }
                    canban_app_server::ClientPushEvent::HerdrInstall(state) => {
                        BackendEvent::HerdrInstall {
                            host_id: "local".into(),
                            state,
                        }
                    }
                },
            )
        }
    }

    fn home_for(&self, host_id: &str) -> String {
        if let Some(hosts) = &self.hosts {
            hosts.home_for(host_id).to_string_lossy().into_owned()
        } else {
            self.app.home.to_string_lossy().into_owned()
        }
    }

    fn local_home(&self) -> Option<PathBuf> {
        Some(self.app.home.clone())
    }

    fn has_host_registry(&self) -> bool {
        self.hosts.is_some()
    }

    fn saved_hosts(&self) -> Vec<SavedHost> {
        self.hosts
            .as_ref()
            .map(|h| h.saved_hosts())
            .unwrap_or_default()
    }

    fn add_host(&self, host: SavedHost) -> Result<HostSummary> {
        let hosts = self
            .hosts
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Host registry unavailable"))?;
        hosts.add_host(host)
    }

    fn remove_host(&self, host_id: &str) -> Result<()> {
        let hosts = self
            .hosts
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Host registry unavailable"))?;
        hosts.remove_host(host_id)
    }

    fn rename_host(&self, host_id: &str, new_name: &str) -> Result<()> {
        let hosts = self
            .hosts
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Host registry unavailable"))?;
        hosts.rename_host(host_id, new_name)
    }

    fn set_host_enabled(&self, host_id: &str, enabled: bool) -> Result<()> {
        let hosts = self
            .hosts
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Host registry unavailable"))?;
        hosts.set_host_enabled(host_id, enabled)
    }

    fn reconnect_host(&self, host_id: Option<&str>) {
        if let Some(hosts) = &self.hosts {
            hosts.reconnect(host_id);
        }
    }

    fn poll_remotes_once(&self) {
        if let Some(hosts) = &self.hosts {
            hosts.poll_remotes_once();
        }
    }

    fn update_remote_daemon(&self, host_id: &str) -> Result<Value> {
        let hosts = self
            .hosts
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Host registry unavailable"))?;
        hosts.update_remote_daemon(host_id)
    }

    fn sync_items(&self) -> Vec<SyncItemView> {
        self.hosts
            .as_ref()
            .map(|h| h.sync_items())
            .unwrap_or_default()
    }

    fn sync_action(&self, host_id: &str, mutation_id: &str, action: SyncAction) -> Result<()> {
        let hosts = self
            .hosts
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Host registry unavailable"))?;
        hosts.sync_action(host_id, mutation_id, action)
    }

    fn global_env_sync_summary(&self) -> canban_types::GlobalEnvSyncSummary {
        self.hosts
            .as_ref()
            .map(|h| h.global_env_sync_summary())
            .unwrap_or_default()
    }

    fn global_env_sync_status(&self) -> canban_types::GlobalEnvSyncStatus {
        self.hosts
            .as_ref()
            .map(|h| h.global_env_sync_status())
            .unwrap_or_default()
    }

    fn trigger_global_env_sync(&self) -> Result<()> {
        let hosts = self
            .hosts
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Host registry unavailable"))?;
        hosts.sync_global_env_now();
        Ok(())
    }
}

/// Pure-remote [`UiBackend`] backed by [`canban_client::RemoteClient`] (works without
/// `local-host`, SQLite, `AppServer`, or SSH subprocesses).
pub struct RemoteBackend {
    client: Arc<canban_client::RemoteClient>,
}

impl RemoteBackend {
    pub fn new(client: Arc<canban_client::RemoteClient>) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &Arc<canban_client::RemoteClient> {
        &self.client
    }
}

impl UiBackend for RemoteBackend {
    fn call_routed(
        &self,
        method: &str,
        params: Value,
        active_project_id: Option<&str>,
    ) -> Result<Value> {
        self.client.call_routed(method, params, active_project_id)
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
        self.client.subscribe_mapped(|ev| match ev {
            canban_client::ClientEvent::Host { host_id, event } => {
                BackendEvent::Host { host_id, event }
            }
            canban_client::ClientEvent::UpdateChanged { host_id, state } => {
                BackendEvent::UpdateChanged { host_id, state }
            }
            canban_client::ClientEvent::UtilitiesChanged { host_id, state } => {
                BackendEvent::UtilitiesChanged { host_id, state }
            }
            canban_client::ClientEvent::UsageChanged { host_id, state } => {
                BackendEvent::UsageChanged { host_id, state }
            }
            canban_client::ClientEvent::HerdrInstall { host_id, state } => {
                BackendEvent::HerdrInstall { host_id, state }
            }
        })
    }

    fn home_for(&self, host_id: &str) -> String {
        self.client.home_for(host_id)
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

    fn sync_items(&self) -> Vec<SyncItemView> {
        self.client.sync_items()
    }

    fn sync_action(&self, host_id: &str, mutation_id: &str, action: SyncAction) -> Result<()> {
        self.client.sync_action(host_id, mutation_id, action)
    }

    fn on_foreground(&self) {
        self.client.on_foreground();
    }

    fn remote_client(&self) -> Option<Arc<canban_client::RemoteClient>> {
        Some(self.client.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockBackend;

    impl UiBackend for MockBackend {
        fn call_routed(
            &self,
            method: &str,
            params: Value,
            active_project_id: Option<&str>,
        ) -> Result<Value> {
            Ok(json!({
                "method": method,
                "params": params,
                "active_project_id": active_project_id,
            }))
        }

        fn call_all_hosts(&self, method: &str, params: Value) -> Vec<(String, Result<Value>)> {
            vec![("remote-1".into(), self.call_routed(method, params, None))]
        }

        fn merged_snapshot(&self) -> Result<Snapshot> {
            Ok(Snapshot::default())
        }

        fn host_summaries(&self) -> Vec<HostSummary> {
            Vec::new()
        }

        fn config_for_host(&self, _host_id: &str) -> Config {
            Config::default()
        }

        fn resolve_target_host(
            &self,
            _method: &str,
            _params: &Value,
            _active_project_id: Option<&str>,
        ) -> String {
            "remote-1".into()
        }

        fn subscribe(&self) -> tokio::sync::mpsc::UnboundedReceiver<BackendEvent> {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = tx.send(BackendEvent::host(
                "remote-1",
                AppEvent::Changed {
                    topic: "resync",
                    card_id: None,
                },
            ));
            let _ = tx.send(BackendEvent::UsageChanged {
                host_id: "remote-1".into(),
                state: json!({"readings":[]}),
            });
            rx
        }
    }

    #[test]
    fn mock_ui_backend_trait_and_events_work_without_local_server() {
        let backend: Arc<dyn UiBackend> = Arc::new(MockBackend);
        let res = backend
            .call_routed("card.list", json!({"limit": 10}), Some("p1"))
            .unwrap();
        assert_eq!(res["method"], "card.list");
        assert_eq!(res["active_project_id"], "p1");
        assert_eq!(
            backend.resolve_target_host("card.list", &json!({}), Some("p1")),
            "remote-1"
        );
        assert!(!backend.has_host_registry());
        assert_eq!(backend.local_home(), None);
        let mut rx = backend.subscribe();
        let ev = rx.blocking_recv().unwrap();
        assert_eq!(ev.host_id(), "remote-1");
        assert!(matches!(
            ev.app_event(),
            Some(AppEvent::Changed {
                topic: "resync",
                card_id: None
            })
        ));
        let side = rx.blocking_recv().unwrap();
        assert_eq!(side.host_id(), "remote-1");
        assert!(side.app_event().is_none());
    }

    #[test]
    fn remote_backend_implements_ui_backend_without_local_host() {
        let dir =
            std::env::temp_dir().join(format!("canban-ui-remote-test-{}", canban_types::id()));
        let host_id = "remote-cached-1";
        let now_ms = canban_types::now();

        let mut card_snap = Snapshot::default();
        let project: canban_types::Project = serde_json::from_value(json!({
            "id": "p1",
            "name": "Cached Project",
            "directory": "/srv/cached",
            "workflow": canban_types::Workflow::default(),
            "created_at": now_ms,
        }))
        .unwrap();
        card_snap.projects.push(project);
        let card: canban_types::Card = serde_json::from_value(json!({
            "id": "c1",
            "project_id": "p1",
            "number": 1,
            "title": "Cached Card",
            "body": "Offline body",
            "stage": "todo",
            "revision": 3,
            "attention": null,
            "archived_at": null,
            "archive": null,
            "archive_error": null,
            "worktree": null,
            "created_at": now_ms,
        }))
        .unwrap();
        card_snap.cards.push(card);
        canban_client::store::save_cache(
            &dir,
            host_id,
            &canban_client::store::CachedHostData {
                saved_at: now_ms,
                snapshot: card_snap,
                config: Config::default(),
            },
        );
        let creds = Arc::new(canban_client::MemoryCredentialStore::new());
        canban_client::store::save_hosts(
            &dir,
            &[SavedHost {
                id: host_id.into(),
                name: "Offline Box".into(),
                transport: canban_types::HostTransport::Tcp {
                    addr: "127.0.0.1:1".into(),
                    token: "dev_0123456789abcdef0123456789abcdef".into(),
                },
                ssh_target: None,
                server_id: Some("srv-cached".into()),
                enabled: false,
                added_at: now_ms,
            }],
            creds.as_ref(),
        )
        .unwrap();

        let client = canban_client::RemoteClient::new(
            canban_client::RemoteClientConfig::new(dir.clone()),
            creds,
        )
        .unwrap();
        let backend: Arc<dyn UiBackend> = Arc::new(RemoteBackend::new(client));

        assert!(backend.has_host_registry());
        assert_eq!(backend.local_home(), None);

        let sums = backend.host_summaries();
        assert_eq!(sums.len(), 1);
        assert_eq!(sums[0].id, host_id);
        assert!(!sums[0].is_local);

        let merged = backend.merged_snapshot().unwrap();
        assert_eq!(merged.cards.len(), 1);
        let rt = backend
            .merged_runtime_state()
            .unwrap()
            .expect("RemoteBackend supports merged_runtime_state");
        assert!(rt.terminals.is_empty());
        assert_eq!(
            backend.resolve_target_host("card.update", &json!({"card_id": "c1"}), None),
            host_id
        );

        let mut rx = backend.subscribe();
        let created = backend
            .call_routed(
                "card.create",
                json!({"project_id": "p1", "title": "Queued offline", "body": "Text"}),
                Some("p1"),
            )
            .unwrap();
        assert_eq!(created["offline_queued"], true);
        let queued_ev = rx.blocking_recv().unwrap();
        assert_eq!(queued_ev.host_id(), host_id);

        let items = backend.sync_items();
        assert_eq!(items.len(), 1);
        backend
            .sync_action(host_id, &items[0].mutation_id, SyncAction::Discard)
            .unwrap();
        assert!(backend.sync_items().is_empty());

        drop(backend);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
