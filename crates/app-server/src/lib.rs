use anyhow::{Context, Result, bail, ensure};
use canban_storage::{Db, Store};
use canban_types::*;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

/// Increment when desktop clients need operations unavailable in older services.
pub const API_VERSION: u32 = canban_updater::DESKTOP_API;
pub use canban_context::FsDurability;
pub use canban_exec::{CommandRunner, CommandSpec, ExecOutput, SystemRunner};
pub use canban_storage::{StorageFaultInjector, StorageFaultKind};
pub use canban_transport::auth;
pub mod instance;
mod updates;

/// Controls how Workflow `uses: script` steps execute inside [`AppServer`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScriptExecutionMode {
    /// Production default: Herdr PTY (`pump_scripts_herdr`) when observer is running with
    /// `SystemRunner`, otherwise synchronous in-process (`pump_scripts_inline`).
    #[default]
    Auto,
    /// Always run script steps inline via `exec_runner` (`pump_scripts_inline`).
    Inline,
    /// Always run script steps through Herdr PTY terminals (`pump_scripts_herdr`).
    Herdr,
}

/// Injectable runtime environment for [`AppServer`] (defaults to real wall clock,
/// real Herdr/GitHub backends, real OS subprocess/filesystem durability/watchers,
/// and `spawn_blocking` workers; customized in DST).
#[derive(Clone)]
pub struct ServerRuntimeEnv {
    pub clock: Arc<dyn canban_types::Clock>,
    pub id_gen: Arc<dyn canban_types::IdGen>,
    pub herdr_backend: Option<Arc<dyn canban_herdr::HerdrBackend>>,
    pub github_transport: Option<Arc<dyn canban_github::HttpTransport>>,
    pub inline_workers: bool,
    pub exec_runner: Arc<dyn canban_exec::CommandRunner>,
    pub fs_durability: canban_context::FsDurability,
    pub sqlite_synchronous_off: bool,
    pub disable_os_watchers: bool,
    pub tailscale_verifier: Option<Arc<dyn canban_transport::TailscalePeerVerifier>>,
    pub desktop_api_override: Option<u32>,
    pub script_execution_mode: ScriptExecutionMode,
    pub storage_faults: Option<Arc<canban_storage::StorageFaultInjector>>,
    pub failpoints: Option<Arc<canban_types::Failpoints>>,
}

impl Default for ServerRuntimeEnv {
    fn default() -> Self {
        Self {
            clock: canban_types::system_clock(),
            id_gen: canban_types::system_id_gen(),
            herdr_backend: None,
            github_transport: None,
            inline_workers: false,
            exec_runner: canban_exec::system_runner(),
            fs_durability: canban_context::FsDurability::Full,
            sqlite_synchronous_off: false,
            disable_os_watchers: false,
            tailscale_verifier: None,
            desktop_api_override: None,
            script_execution_mode: ScriptExecutionMode::Auto,
            storage_faults: None,
            failpoints: None,
        }
    }
}

/// Marker error wrapping any failure that occurs after Herdr has already spawned a PTY
/// (`terminal.run` succeeded) and before its launch receipt was committed to SQLite.
/// Callers must transition the affected session/run to `"unknown"` rather than `"failed"`
/// so no duplicate PTY is auto-launched and orphan cleanup remains reachable.
#[derive(Debug)]
pub(crate) struct LaunchReceiptUncertain(pub(crate) anyhow::Error);

impl std::fmt::Display for LaunchReceiptUncertain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

impl std::error::Error for LaunchReceiptUncertain {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

pub struct AppServer {
    _development: Option<instance::Registration>,
    updates_disabled: bool,
    _service_lock: std::fs::File,
    self_weak: OnceLock<std::sync::Weak<Self>>,
    clock: Arc<dyn canban_types::Clock>,
    id_gen: Arc<dyn canban_types::IdGen>,
    herdr_backend: Option<Arc<dyn canban_herdr::HerdrBackend>>,
    github_transport: Option<Arc<dyn canban_github::HttpTransport>>,
    inline_workers: bool,
    exec_runner: Arc<dyn canban_exec::CommandRunner>,
    fs_durability: canban_context::FsDurability,
    disable_os_watchers: bool,
    script_execution_mode: ScriptExecutionMode,
    failpoints: Option<Arc<canban_types::Failpoints>>,
    last_cardless_prune_at: AtomicU64,
    sim_abrupt_crash: Arc<AtomicBool>,
    utilities: utilities::Utilities,
    pub home: PathBuf,
    github: Arc<canban_github::auth::Service>,
    github_sync_due: AtomicBool,
    github_sync_notify: Arc<tokio::sync::Notify>,
    github_jobs: Mutex<github::worker::Jobs>,
    github_job_notify: Arc<tokio::sync::Notify>,
    github_revision: Arc<AtomicU64>,
    state_instance: String,
    db_changes: Arc<AtomicU64>,
    state_revision: Arc<AtomicU64>,
    observed_at: Arc<AtomicU64>,
    push_seq: Arc<AtomicU64>,
    push_tx: tokio::sync::broadcast::Sender<state::PushEvent>,
    github_cli: Arc<canban_github::cli::Service>,
    github_queries: Mutex<std::collections::BTreeMap<String, Value>>,
    branch_pr_probes: Mutex<std::collections::BTreeMap<(String, String), u64>>,
    identity: String,
    device_store: Arc<auth::DeviceTokenStore>,
    pairing_codes: Arc<auth::PairingCodeStore>,
    device_revocations: canban_transport::RevocationBus,
    tailscale_devices: Arc<canban_transport::TailscaleSessionRegistry>,
    tailscale_verifier_override: Mutex<Option<Arc<dyn canban_transport::TailscalePeerVerifier>>>,
    desktop_api_override: Mutex<Option<u32>>,
    started_at: u64,
    bound_tcp: Mutex<Vec<String>>,
    tcp_listener_stop: Mutex<Option<tokio::sync::watch::Sender<bool>>>,
    active_tcp_connections: Arc<AtomicUsize>,
    store: Mutex<Store>,
    config: RwLock<Config>,
    cli: PathBuf,
    listeners: Mutex<Vec<mpsc::Sender<AppEvent>>>,
    stop: AtomicBool,
    stop_tx: tokio::sync::watch::Sender<bool>,
    stop_cond: (Mutex<bool>, std::sync::Condvar),
    runtime_handle: Mutex<Option<tokio::runtime::Handle>>,
    checkout_gates: delivery_gates::KeyedGateTable<delivery_gates::CheckoutGateKey>,
    queue_gates: delivery_gates::KeyedGateTable<String>,
    card_delivery_gates: delivery_gates::KeyedGateTable<String>,
    github_action_gate: (Mutex<bool>, std::sync::Condvar),
    merging_cards: Mutex<std::collections::BTreeSet<String>>,
    terminal_gate: Mutex<()>,
    terminal_contexts: Mutex<Vec<TerminalContext>>,
    last_terminal_size: Mutex<Option<canban_herdr::TerminalSize>>,
    presence: presence::PresenceTracker,
    inventory: Mutex<observation::Inventory>,
    refinements: refine::RefinementTracker,
    observer_running: AtomicBool,
    herdr_wake: AtomicBool,
    herdr_notify: Arc<tokio::sync::Notify>,
    herdr_install: Arc<Mutex<herdr_setup::HerdrInstallState>>,
    check_slots: Mutex<bool>,
    check_signal: std::sync::Condvar,
    runtime_wake: Mutex<Option<Arc<tokio::sync::Notify>>>,
    delivery_wake: Mutex<Option<Arc<tokio::sync::Notify>>>,
    script_wake: Mutex<Option<Arc<tokio::sync::Notify>>>,
    contexts: Mutex<directory::ContextQueue>,
    context_notify: Arc<tokio::sync::Notify>,
    #[cfg(test)]
    pub(crate) runtime_worker_ticks: AtomicU64,
    #[cfg(test)]
    pub(crate) delivery_worker_passes: AtomicU64,
}
// A concurrently spawned child can briefly inherit the open file description
// before exec closes it. Closing just our descriptor would leave its flock held.
// Ownership ends with the last AppServer, independently of inherited descriptors.
impl Drop for AppServer {
    fn drop(&mut self) {
        let _ = self._service_lock.unlock();
    }
}
pub struct Running {
    server: Arc<AppServer>,
    runtime: Option<tokio::runtime::Runtime>,
    socket_path: PathBuf,
    _lock: std::fs::File,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.server.trigger_stop();
        *self.server.runtime_handle.lock().unwrap() = None;
        *self.server.runtime_wake.lock().unwrap() = None;
        *self.server.delivery_wake.lock().unwrap() = None;
        *self.server.script_wake.lock().unwrap() = None;
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
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = self.server.finish_agent_work();
        let _ = self.server._service_lock.unlock();
    }
}
/// Guard for an [`AppServer`] running inside an external Tokio runtime (used by DST).
pub struct SimRunning {
    server: Arc<AppServer>,
    _lock: std::fs::File,
    abrupt_crash: Arc<AtomicBool>,
}
impl SimRunning {
    /// Controls whether dropping this guard simulates an abrupt crash (skipping
    /// graceful `finish_agent_work()` teardown while still releasing runtime
    /// handles and the local service lock so the host can restart cleanly).
    pub fn set_abrupt_crash(&self, abrupt: bool) {
        self.abrupt_crash.store(abrupt, Ordering::SeqCst);
    }

    /// Returns a shared flag controlling abrupt-crash behavior on drop.
    pub fn abrupt_crash_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.abrupt_crash)
    }

    /// Marks this guard for abrupt crash and drops it immediately.
    pub fn crash(self) {
        self.set_abrupt_crash(true);
        drop(self);
    }
}
impl Drop for SimRunning {
    fn drop(&mut self) {
        self.server.trigger_stop();
        *self.server.runtime_handle.lock().unwrap() = None;
        *self.server.runtime_wake.lock().unwrap() = None;
        *self.server.delivery_wake.lock().unwrap() = None;
        *self.server.script_wake.lock().unwrap() = None;
        if !self.abrupt_crash.load(Ordering::SeqCst) {
            let _ = self.server.finish_agent_work();
        }
        let _ = self.server._service_lock.unlock();
    }
}
pub(crate) struct GithubActionGateGuard<'a> {
    gate: &'a (Mutex<bool>, std::sync::Condvar),
}
impl Drop for GithubActionGateGuard<'_> {
    fn drop(&mut self) {
        let mut busy = self.gate.0.lock().unwrap();
        *busy = false;
        self.gate.1.notify_one();
    }
}

pub(crate) struct MergingCard<'a> {
    server: &'a AppServer,
    card_id: String,
}
impl Drop for MergingCard<'_> {
    fn drop(&mut self) {
        let removed = self
            .server
            .merging_cards
            .lock()
            .unwrap()
            .remove(&self.card_id);
        if removed {
            self.server.emit("review", Some(self.card_id.clone()));
        }
    }
}
pub(crate) struct MergingCards<'a> {
    server: &'a AppServer,
    card_ids: Vec<String>,
}
impl Drop for MergingCards<'_> {
    fn drop(&mut self) {
        let mut guard = self.server.merging_cards.lock().unwrap();
        let mut changed = false;
        for id in &self.card_ids {
            changed |= guard.remove(id);
        }
        drop(guard);
        if changed {
            self.server.emit("review", None);
        }
    }
}
fn strp<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Missing string field: {key}"))
}
fn strings(p: &Value, key: &str) -> Vec<String> {
    p[key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
fn revise(c: &Card, p: &Value) -> Result<()> {
    canban_kanban::check_revision(
        c,
        p["revision"]
            .as_u64()
            .context("Expected card revision is required")?,
    )
}
#[allow(
    clippy::too_many_arguments,
    reason = "Transaction-scoped timeline append with explicit timestamp"
)]
fn log(
    db: Db<'_>,
    c: &Card,
    kind: &str,
    actor: &str,
    message: &str,
    terminal_id: Option<String>,
    mut data: Value,
    now_ms: u64,
) -> Result<()> {
    if let Some(tid) = terminal_id
        && let Some(obj) = data.as_object_mut()
    {
        obj.entry("terminal_id".to_owned())
            .or_insert_with(|| json!(tid));
    }
    db.append(canban_timeline::event_at(
        &c.project_id,
        &c.id,
        kind,
        actor,
        message,
        data,
        now_ms,
    ))?;
    Ok(())
}
fn attention(db: Db<'_>, card: &mut Card, message: &str, now_ms: u64) -> Result<()> {
    card.attention = Some(message.into());
    if card.human_gate.is_none() {
        card.human_gate = Some(HumanGate {
            reason: message.into(),
            terminal_id: None,
            next_stage: None,
        });
    }
    db.save(&canban_notifications::attention_at(
        &card.project_id,
        &card.id,
        message,
        now_ms,
    ))?;
    Ok(())
}
pub(crate) fn parse_card_number(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        return (n > 0).then_some(n);
    }
    let s = v.as_str()?.trim();
    let s = s.strip_prefix('#').unwrap_or(s).trim();
    let n = s.parse::<u64>().ok()?;
    (n > 0).then_some(n)
}
fn path_within(root: &str, target: &str) -> bool {
    if root.is_empty() || target.is_empty() {
        return false;
    }
    let r = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let t = std::fs::canonicalize(target).unwrap_or_else(|_| PathBuf::from(target));
    t.starts_with(&r)
}
pub(crate) fn resolve_workflow_scope(db: Db<'_>, raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("global") || is_global_project_id(trimmed) {
        return Ok(GLOBAL_PROJECT_ID.into());
    }
    resolve_project_id(db, trimmed)
}

pub(crate) fn resolve_project_id(db: Db<'_>, raw: &str) -> Result<String> {
    let raw = raw.trim();
    ensure!(!raw.is_empty(), "Missing project_id");
    if db.get::<Project>(raw).is_ok() {
        return Ok(raw.to_owned());
    }
    let migrated: Option<String> = db
        .conn
        .query_row(
            "SELECT new_id FROM id_migrations WHERE kind='project' AND old_id=?1",
            [raw],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None);
    if let Some(new_id) = migrated
        && db.get::<Project>(&new_id).is_ok()
    {
        return Ok(new_id);
    }
    let projects = db.list::<Project>()?;
    if raw.len() >= 4 && raw.chars().all(|c| c.is_ascii_hexdigit()) {
        let needle = raw.to_ascii_lowercase();
        let prefix_matches: Vec<_> = projects
            .iter()
            .filter(|p| p.id.to_ascii_lowercase().starts_with(&needle))
            .collect();
        if prefix_matches.len() == 1 {
            return Ok(prefix_matches[0].id.clone());
        }
        if prefix_matches.len() > 1 {
            bail!("Ambiguous project prefix: {raw}");
        }
    }
    let name_matches: Vec<_> = projects
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(raw))
        .collect();
    if name_matches.len() == 1 {
        return Ok(name_matches[0].id.clone());
    }
    if name_matches.len() > 1 {
        bail!("Ambiguous project name: {raw}");
    }
    bail!("Project not found: {raw}")
}

pub(crate) fn resolve_card(db: Db<'_>, p: &Value) -> Result<Card> {
    if let Some(raw_id) = p["card_id"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Ok(card) = db.get::<Card>(raw_id) {
            return Ok(card);
        }
        let migrated: Option<String> = db
            .conn
            .query_row(
                "SELECT new_id FROM id_migrations WHERE kind='card' AND old_id=?1",
                [raw_id],
                |r| r.get(0),
            )
            .optional()
            .unwrap_or(None);
        if let Some(new_id) = migrated
            && let Ok(card) = db.get::<Card>(&new_id)
        {
            return Ok(card);
        }
        if raw_id.len() >= 4 && raw_id.chars().all(|c| c.is_ascii_hexdigit()) {
            let needle = raw_id.to_ascii_lowercase();
            let prefix_matches: Vec<_> = db
                .list::<Card>()?
                .into_iter()
                .filter(|c| {
                    c.removed_at.is_none() && c.id.to_ascii_lowercase().starts_with(&needle)
                })
                .collect();
            if prefix_matches.len() == 1 {
                return Ok(prefix_matches.into_iter().next().unwrap());
            }
            if prefix_matches.len() > 1 {
                bail!("Ambiguous card prefix: {raw_id}");
            }
        }
        if parse_card_number(&json!(raw_id)).is_none() {
            bail!("Card not found: {raw_id}");
        }
    }
    let number = parse_card_number(&p["card"])
        .or_else(|| parse_card_number(&p["card_id"]))
        .context("Supply --card <number> (or CANBAN_CARD) or card_id")?;
    let cards = db.list::<Card>()?;
    if let Some(raw_pid) = p["project_id"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let pid = resolve_project_id(db, raw_pid)?;
        return cards
            .into_iter()
            .find(|c| c.project_id == pid && c.number == number && c.removed_at.is_none())
            .with_context(|| format!("Card #{number} not found in Project {pid}"));
    }
    if let Some(cwd) = p["cwd"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(c) = cards.iter().find(|c| {
            c.number == number
                && c.removed_at.is_none()
                && c.worktree.as_deref().is_some_and(|wt| path_within(wt, cwd))
        }) {
            return Ok(c.clone());
        }
        if let Some(wt) = db
            .list::<ManagedWorktree>()?
            .into_iter()
            .find(|w| path_within(&w.path, cwd))
            && let Some(c) = cards.iter().find(|c| {
                c.project_id == wt.project_id && c.number == number && c.removed_at.is_none()
            })
        {
            return Ok(c.clone());
        }
        let mut best_proj: Option<(usize, String)> = None;
        for proj in db.list::<Project>()? {
            let dir = proj.directory;
            if path_within(&dir, cwd) {
                let len = dir.len();
                if best_proj.as_ref().is_none_or(|(b, _)| len > *b) {
                    best_proj = Some((len, proj.id));
                }
            }
        }
        if let Some((_, pid)) = best_proj {
            return cards
                .into_iter()
                .find(|c| c.project_id == pid && c.number == number && c.removed_at.is_none())
                .with_context(|| {
                    format!("Card #{number} not found in current repository Project")
                });
        }
    }
    let matches: Vec<Card> = cards
        .into_iter()
        .filter(|c| c.number == number && c.removed_at.is_none())
        .collect();
    match matches.len() {
        1 => Ok(matches.into_iter().next().unwrap()),
        0 => bail!("Card #{number} not found"),
        _ => bail!(
            "Card #{number} exists in multiple Projects; pass --project <project_id> or run inside the Project directory"
        ),
    }
}
#[allow(
    clippy::too_many_arguments,
    reason = "Transaction-scoped composition of routing, stage and optional mention task"
)]
fn enqueue(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    project: &Project,
    stage: &Stage,
    trigger: ExecutionTrigger,
    explicit: Option<&str>,
    instruction: Option<String>,
    now_ms: u64,
) -> Result<Option<TerminalEntry>> {
    ensure!(
        card.removed_at.is_none(),
        "Restore the removed Card before scheduling"
    );
    if !stage.execute && explicit.is_none() {
        return Ok(None);
    }
    let profile = match canban_scheduler::select_for_stage_in_project(
        config,
        Some(project),
        stage.profile.as_deref(),
        explicit,
    ) {
        Ok(p) => p,
        Err(e) => {
            attention(db, card, &e.to_string(), now_ms)?;
            log(
                db,
                card,
                "scheduler.blocked",
                "system",
                &e.to_string(),
                None,
                json!({}),
                now_ms,
            )?;
            return Ok(None);
        }
    };
    let context = crate::sessions::conversation(db, card, &profile, now_ms)?;
    let mut entry: TerminalEntry = db.get(&context.id)?;
    let instruction_str = instruction.unwrap_or_else(|| stage.instruction.clone());
    let on_completed = if explicit.is_some() {
        None
    } else {
        stage.on_completed.clone()
    };
    let require_human_review = explicit.is_none() && stage.require_human_review;
    let has_live_pty = entry
        .agent
        .as_ref()
        .is_some_and(|a| a.terminal.is_some() && !a.finished());
    let profile_id = profile.id.clone();
    let run_number = if has_live_pty {
        let agent = entry.agent.as_mut().unwrap();
        agent.card_number = card.number;
        agent.pending = Some(QueuedAssignment {
            stage: stage.id.clone(),
            trigger: trigger.clone(),
            profile,
            instruction: instruction_str,
            on_completed,
            require_human_review,
            workflow_run_id: None,
            workflow_step: None,
            queued_at: now_ms,
        });
        agent.number.saturating_add(1)
    } else {
        let number = u32::try_from(db.counter(&format!("card-attempt:{}", card.id))?)?;
        let prior_agent = entry.agent.clone();
        let mut run = canban_scheduler::agent_run_at(
            &project.id,
            &card.id,
            number,
            &stage.id,
            trigger.clone(),
            profile,
            instruction_str,
            on_completed,
            now_ms,
        );
        run.card_number = card.number;
        run.require_human_review = require_human_review;
        if let Some(prev) = prior_agent
            && explicit.is_some()
            && context.provider_session_id.is_some()
            && prev.provider_session_id == context.provider_session_id
        {
            run.resume_session = true;
            run.workspace = prev.workspace.or_else(|| {
                prev.directory.map(|directory| WorkspaceBinding {
                    directory,
                    path: prev.cwd.clone(),
                    branch: None,
                    base_commit: None,
                    target_branch: None,
                })
            });
            run.cwd = prev.cwd;
            run.provider_session_id = prev.provider_session_id;
        }
        entry.agent = Some(run);
        number
    };
    db.save(&entry)?;
    log(
        db,
        card,
        "execution.queued",
        "system",
        &format!("Queued @{profile_id}"),
        Some(entry.id.clone()),
        json!({"trigger":trigger,"stage":stage.id,"run_number":run_number}),
        now_ms,
    )?;
    Ok(Some(entry))
}
impl AppServer {
    pub fn open(home: PathBuf, cli: PathBuf) -> Result<Arc<Self>> {
        Self::open_with_env(home, cli, ServerRuntimeEnv::default())
    }
    pub fn open_with_env(home: PathBuf, cli: PathBuf, env: ServerRuntimeEnv) -> Result<Arc<Self>> {
        let ServerRuntimeEnv {
            clock,
            id_gen,
            herdr_backend,
            github_transport,
            inline_workers,
            exec_runner,
            fs_durability,
            sqlite_synchronous_off,
            disable_os_watchers,
            tailscale_verifier,
            desktop_api_override,
            script_execution_mode,
            storage_faults,
            failpoints,
        } = env;
        let development = if exec_runner.is_system() {
            instance::bundled()?
        } else {
            None
        };
        if let Some(instance) = &development {
            instance.check_home(&home)?;
        }
        let updates_disabled = development.is_some();
        let _startup = canban_updater::handoff::startup_guard(&home)?;
        let startup_progress =
            startup_progress::StartupProgressWriter::new_with_durability_and_clock(
                &home,
                fs_durability,
                clock.clone(),
            );
        startup_progress.set("lock", "LOCK", "Acquiring local service lock…");
        let service_lock = canban_updater::service_lock(&home)?;
        let _ = canban_updater::recover_stale_state(&home);
        startup_progress.set(
            "lock",
            "LOCK",
            format!("service.lock acquired (pid {})", std::process::id()),
        );
        let store_options = canban_storage::StoreOptions {
            synchronous_off: sqlite_synchronous_off,
            fault_injector: storage_faults,
        };
        let mut store =
            Store::open_with_progress_and_options(&home, store_options, |phase, detail| {
                let tag = match phase {
                    "migrate" => "MIGRATE",
                    _ => "STORAGE",
                };
                startup_progress.set(phase, tag, detail);
            })?;
        let identity_path = home.join("server-id");
        let mut identity_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&identity_path)?;
        identity_file.lock()?;
        if identity_file.metadata()?.len() == 0 {
            identity_file.write_all(id_gen.next_token().as_bytes())?;
            if fs_durability == canban_context::FsDurability::Full {
                identity_file.sync_all()?;
            }
        }
        let identity = std::fs::read_to_string(&identity_path)?;
        drop(identity_file);
        ensure!(
            matches!(identity.len(), 8 | 32) && identity.chars().all(|c| c.is_ascii_hexdigit()),
            "Invalid AppServer identity"
        );
        // Existing attention is a real pause after upgrading, not merely a badge.
        store.write(|db| {
            let terminals = db.list::<TerminalEntry>()?;
            for mut card in db.list::<Card>()? {
                if let Some(reason) = card.attention.clone()
                    && card.human_gate.is_none()
                {
                    let terminal_id = terminals
                        .iter()
                        .rev()
                        .find(|t| {
                            t.agent.as_ref().is_some_and(|a| {
                                a.card_id == card.id
                                    && matches!(
                                        a.state.as_str(),
                                        "blocked" | "failed" | "needs_report" | "unknown"
                                    )
                            })
                        })
                        .map(|t| t.id.clone());
                    card.human_gate = Some(HumanGate {
                        reason,
                        terminal_id,
                        next_stage: None,
                    });
                    db.save(&card)?;
                }
            }
            Ok(())
        })?;
        let path = home.join("config/agents.json");
        if !path.exists() {
            canban_context::atomic_with_durability(
                &path,
                &serde_json::to_vec_pretty(&Config::default())?,
                fs_durability,
            )?;
        }
        startup_progress.set(
            "config",
            "CONFIG",
            "Loading agent profiles and configuration…",
        );
        canban_context::materialize_skill_with_durability(&home, fs_durability)?;
        let (config, migration) = Self::read_config_with_runner(&home, exec_runner.as_ref())?;
        startup_progress.set(
            "workflow",
            "WORKFLOW",
            "Validating project workflows and schedules…",
        );
        for mut project in store.db().list::<Project>()? {
            let mut cleared = false;
            for stage in &mut project.workflow.stages {
                if stage.execute
                    || stage.profile.is_some()
                    || !stage.instruction.is_empty()
                    || stage.on_completed.is_some()
                    || stage.require_human_review
                {
                    stage.execute = false;
                    stage.profile = None;
                    stage.instruction.clear();
                    stage.on_completed = None;
                    stage.require_human_review = false;
                    cleared = true;
                }
            }
            if cleared {
                store.write(|db| db.save(&project))?;
            }
            canban_workflow_engine::validate(&project.workflow)?;
        }
        Self::publish_migration_with_durability(&home, migration, fs_durability, id_gen.as_ref())?;
        let mut project_count = 0usize;
        let startup_now_ms = clock.now_ms();
        store.write(|db| {
            workflow::initialize(db, &config)?;
            let projects = db.list::<Project>()?;
            project_count = projects.len();
            for project in projects {
                authoring::initialize(&home, db, &config, &project, startup_now_ms)?;
            }
            authoring::initialize_global(&home, db, &config, startup_now_ms)?;
            schedules::reconcile_startup(db, startup_now_ms)?;
            Ok(())
        })?;
        startup_progress.set(
            "snapshot",
            "SNAPSHOT",
            format!("Decoding durable snapshot ({project_count} projects)…"),
        );
        // Verify that the full durable snapshot decodes cleanly before claiming the home socket,
        // so any incompatible SQL row fails fast at startup rather than after `app.ping`.
        let verified_snapshot = store
            .snapshot()
            .context("Failed to decode durable SQLite snapshot on startup")?;
        startup_progress.set(
            "ready",
            "SNAPSHOT",
            format!(
                "Verified {} projects, {} cards",
                verified_snapshot.projects.len(),
                verified_snapshot.cards.len()
            ),
        );
        let initial_changes = store.db().conn.total_changes();
        let db_changes = Arc::new(AtomicU64::new(initial_changes));
        let state_revision = Arc::new(AtomicU64::new(0));
        let github_revision = Arc::new(AtomicU64::new(0));
        let observed_at = Arc::new(AtomicU64::new(0));
        let push_seq = Arc::new(AtomicU64::new(0));
        let (push_tx, _) = tokio::sync::broadcast::channel(256);
        let (stop_tx, _) = tokio::sync::watch::channel(false);
        let fs_sync = fs_durability == canban_context::FsDurability::Full;
        let device_store = Arc::new(
            auth::DeviceTokenStore::open_with_clock(&home, clock.clone())
                .with_id_gen(id_gen.clone())
                .with_fs_sync(fs_sync),
        );
        let pairing_codes = Arc::new(
            auth::PairingCodeStore::open_with_clock(&home, clock.clone())
                .with_id_gen(id_gen.clone())
                .with_fs_sync(fs_sync),
        );
        let device_revocations = canban_transport::RevocationBus::new();
        let tailscale_devices = Arc::new(
            canban_transport::TailscaleSessionRegistry::new_with_clock(clock.clone()),
        );

        {
            let hook_changes = db_changes.clone();
            let hook_github_rev = github_revision.clone();
            let hook_push_seq = push_seq.clone();
            let hook_push_tx = push_tx.clone();
            store.set_on_commit(move |after, topic| {
                hook_changes.store(after, Ordering::SeqCst);
                let seq = hook_push_seq.fetch_add(1, Ordering::SeqCst) + 1;
                let _ = hook_push_tx.send(state::PushEvent::StateChanged {
                    seq,
                    topic,
                    card_id: None,
                    github_revision: hook_github_rev.load(Ordering::SeqCst),
                });
            });
        }

        let utilities =
            utilities::Utilities::new_with_options(&home, fs_durability, disable_os_watchers);
        let initial_terminal_size = utilities.default_terminal_size();
        {
            let usage_push_seq = push_seq.clone();
            let usage_push_tx = push_tx.clone();
            utilities.usage.set_on_update(move |state| {
                let seq = usage_push_seq.fetch_add(1, Ordering::SeqCst) + 1;
                let _ = usage_push_tx.send(state::PushEvent::UsageChanged { seq, state });
            });
        }

        let server = Arc::new(Self {
            _development: development
                .as_ref()
                .map(|v| instance::register(v, "server"))
                .transpose()?,
            updates_disabled,
            _service_lock: service_lock,
            self_weak: OnceLock::new(),
            started_at: clock.now_ms(),
            clock: clock.clone(),
            id_gen: id_gen.clone(),
            herdr_backend,
            github_transport,
            inline_workers,
            exec_runner,
            fs_durability,
            disable_os_watchers,
            script_execution_mode,
            failpoints,
            last_cardless_prune_at: AtomicU64::new(0),
            sim_abrupt_crash: Arc::new(AtomicBool::new(false)),
            utilities,
            github_sync_due: AtomicBool::new(true),
            github_sync_notify: Arc::new(tokio::sync::Notify::new()),
            github_jobs: Mutex::new(Default::default()),
            github_job_notify: Arc::new(tokio::sync::Notify::new()),
            github_revision,
            state_instance: id_gen.next_id(),
            db_changes,
            state_revision,
            observed_at,
            push_seq,
            push_tx,
            github_queries: Mutex::new(
                std::fs::read(home.join("cache/github/queries.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default(),
            ),
            branch_pr_probes: Mutex::new(std::collections::BTreeMap::new()),
            github_cli: Arc::new(canban_github::cli::Service::new_with_clock(
                home.join("cache/github/gh"),
                clock.clone(),
            )),
            github: Arc::new(canban_github::auth::Service::new_with_clock(
                home.clone(),
                clock,
            )),
            identity,
            device_store,
            pairing_codes,
            device_revocations,
            tailscale_devices,
            tailscale_verifier_override: Mutex::new(tailscale_verifier),
            desktop_api_override: Mutex::new(desktop_api_override),
            bound_tcp: Mutex::new(Vec::new()),
            tcp_listener_stop: Mutex::new(None),
            active_tcp_connections: Arc::new(AtomicUsize::new(0)),
            home,
            store: Mutex::new(store),
            config: RwLock::new(config),
            cli,
            listeners: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
            stop_tx,
            stop_cond: (Mutex::new(false), std::sync::Condvar::new()),
            runtime_handle: Mutex::new(None),
            checkout_gates: Default::default(),
            queue_gates: Default::default(),
            card_delivery_gates: Default::default(),
            github_action_gate: (Mutex::new(false), std::sync::Condvar::new()),
            merging_cards: Mutex::new(Default::default()),
            terminal_gate: Mutex::new(()),
            terminal_contexts: Mutex::new(Vec::new()),
            last_terminal_size: Mutex::new(initial_terminal_size),
            presence: Default::default(),
            inventory: Mutex::new(Default::default()),
            refinements: Default::default(),
            observer_running: AtomicBool::new(false),
            herdr_wake: AtomicBool::new(false),
            herdr_notify: Arc::new(tokio::sync::Notify::new()),
            herdr_install: Arc::new(Mutex::new(Default::default())),
            check_slots: Mutex::new(false),
            check_signal: std::sync::Condvar::new(),
            runtime_wake: Mutex::new(None),
            delivery_wake: Mutex::new(None),
            script_wake: Mutex::new(None),
            contexts: Mutex::new(Default::default()),
            context_notify: Arc::new(tokio::sync::Notify::new()),
            #[cfg(test)]
            runtime_worker_ticks: AtomicU64::new(0),
            #[cfg(test)]
            delivery_worker_passes: AtomicU64::new(0),
        });
        let _ = server.self_weak.set(Arc::downgrade(&server));
        Ok(server)
    }
    fn read_config_with_runner(
        home: &Path,
        runner: &dyn canban_exec::CommandRunner,
    ) -> Result<(Config, Option<Vec<u8>>)> {
        let bytes = std::fs::read(home.join("config/agents.json"))?;
        let mut value: Value = serde_json::from_slice(&bytes)?;
        let changed = canban_profiles::migrate(&mut value)?;
        let raw: Config = serde_json::from_value(value.clone())?;
        let config = canban_profiles::resolve_with_runner(raw, home, runner)?;
        Self::validate_config(&config)?;
        Ok((
            config,
            if changed {
                Some(serde_json::to_vec_pretty(&value)?)
            } else {
                None
            },
        ))
    }
    fn publish_migration_with_durability(
        home: &Path,
        bytes: Option<Vec<u8>>,
        durability: canban_context::FsDurability,
        id_gen: &dyn canban_types::IdGen,
    ) -> Result<()> {
        if let Some(bytes) = bytes {
            let path = home.join("config/agents.json");
            let backup = home
                .join("config")
                .join(format!("agents.legacy-{}.json", id_gen.next_id()));
            canban_context::atomic_with_durability(&backup, &std::fs::read(&path)?, durability)?;
            canban_context::atomic_with_durability(&path, &bytes, durability)?;
        }
        Ok(())
    }
    fn validate_config(c: &Config) -> Result<()> {
        ensure!(
            c.relay_endpoint.is_none() && c.relay_token.is_none(),
            "relay_endpoint / relay_token are client transport options; do not configure them on AppServer"
        );
        ensure!(
            c.scheduler.max_agents > 0,
            "max_agents must be greater than zero"
        );
        ensure!(
            c.scheduler.idle_minutes <= 525600,
            "idle_minutes must be at most one year"
        );
        let mut ids = std::collections::HashSet::new();
        for p in &c.profiles {
            ensure!(
                !p.id.is_empty() && ids.insert(&p.id),
                "Profile IDs must be nonempty and unique"
            );
            ensure!(!p.command().is_empty(), "Profile needs a launch command");
        }
        ensure!(
            ids.contains(&c.default_agent),
            "Default agent is not configured"
        );
        Ok(())
    }
    pub(crate) fn validate_references(&self, config: &Config) -> Result<()> {
        let snapshot = self.snapshot()?;
        for project in &snapshot.projects {
            canban_workflow_engine::validate(&project.workflow)
                .with_context(|| format!("Project {} workflow", project.name))?;
        }
        let valid_ids: Vec<String> = config.profiles.iter().map(|p| p.id.clone()).collect();
        for source in &snapshot.workflow_sources {
            let project = snapshot
                .projects
                .iter()
                .find(|p| p.id == source.project_id)
                .context("Workflow Project missing")?;
            let rev = snapshot
                .workflow_revisions
                .iter()
                .find(|r| r.id == source.revision_id)
                .context("Active Workflow revision missing")?;
            canban_workflow_engine::validate_definition(
                &rev.definition,
                &project
                    .workflow
                    .stages
                    .iter()
                    .map(|s| s.id.clone())
                    .collect::<Vec<_>>(),
                &valid_ids,
            )
            .with_context(|| format!("Project '{}' ({})", project.name, source.filename))?;
        }
        Ok(())
    }
    pub fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }
    pub fn id_gen(&self) -> &Arc<dyn canban_types::IdGen> {
        &self.id_gen
    }
    pub(crate) fn next_id(&self) -> String {
        self.id_gen.next_id()
    }
    pub(crate) fn next_uuid(&self) -> String {
        self.id_gen.next_uuid().hyphenated().to_string()
    }
    pub fn exec_runner(&self) -> &Arc<dyn canban_exec::CommandRunner> {
        &self.exec_runner
    }
    /// Returns the per-instance failpoints registry injected via [`ServerRuntimeEnv::failpoints`],
    /// or `None` in production.
    pub fn failpoints(&self) -> Option<&Arc<canban_types::Failpoints>> {
        self.failpoints.as_ref()
    }
    /// Evaluates failpoint `name` with zero lock overhead when `self.failpoints` is `None`.
    #[inline]
    pub(crate) fn check_failpoint(&self, name: &str) -> Result<()> {
        canban_types::Failpoints::check_opt(self.failpoints.as_ref(), name)
            .map_err(|msg| anyhow::anyhow!("failpoint[{name}]: {msg}"))
    }
    /// Returns the injected [`IdGen`] for deterministic vault key/nonce derivation under
    /// non-system runners (DST), and `None` in production so `OsRng` remains the default.
    pub(crate) fn vault_id_gen(&self) -> Option<&dyn canban_types::IdGen> {
        if self.exec_runner.is_system() {
            None
        } else {
            Some(self.id_gen.as_ref())
        }
    }
    /// Controls whether dropping the active [`SimRunning`] guard skips graceful
    /// `finish_agent_work()` teardown to simulate an abrupt server crash in DST.
    pub fn set_abrupt_crash(&self, abrupt: bool) {
        self.sim_abrupt_crash.store(abrupt, Ordering::SeqCst);
    }
    pub fn db_changes(&self) -> u64 {
        self.db_changes.load(Ordering::SeqCst)
    }
    pub(crate) fn herdr_client(&self) -> canban_herdr::Client {
        canban_herdr::Client::with_backend_and_runner(
            self.config(),
            self.herdr_backend.clone(),
            self.exec_runner.clone(),
        )
    }
    pub(crate) fn herdr_client_with_config(&self, config: Config) -> canban_herdr::Client {
        canban_herdr::Client::with_backend_and_runner(
            config,
            self.herdr_backend.clone(),
            self.exec_runner.clone(),
        )
    }
    pub(crate) fn herdr_activity(
        &self,
        core: &str,
        terminal: &str,
    ) -> canban_herdr::activity::Activity {
        if let Some(backend) = &self.herdr_backend {
            backend.activity(core, terminal)
        } else {
            canban_herdr::activity::get(core, terminal)
        }
    }
    pub(crate) async fn run_work<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        if self.inline_workers {
            let out = f();
            tokio::task::yield_now().await;
            Ok(out)
        } else {
            Ok(tokio::task::spawn_blocking(f).await?)
        }
    }
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
    pub(crate) fn spawn_bg<F>(&self, fut: F) -> bool
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = self.runtime_handle.lock().unwrap().clone();
        if let Some(handle) = handle {
            handle.spawn(fut);
            true
        } else {
            false
        }
    }
    pub(crate) fn spawn_blocking_bg<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let handle = self.runtime_handle.lock().unwrap().clone();
        if self.inline_workers {
            if let Some(handle) = handle {
                handle.spawn(async move {
                    task();
                    tokio::task::yield_now().await;
                });
            } else {
                task();
            }
            return;
        }
        if let Some(handle) = handle {
            handle.spawn_blocking(task);
        } else {
            std::thread::spawn(task);
        }
    }
    pub fn trigger_github_sync(&self) {
        self.github_sync_due.store(true, Ordering::SeqCst);
        self.github_sync_notify.notify_one();
    }
    pub(crate) fn wake_herdr(&self) {
        self.herdr_wake.store(true, Ordering::SeqCst);
        self.herdr_notify.notify_one();
    }
    pub(crate) async fn wait_or_stop(
        notify: &tokio::sync::Notify,
        stop: &mut tokio::sync::watch::Receiver<bool>,
        timeout: Option<Duration>,
    ) -> bool {
        if *stop.borrow() {
            return false;
        }
        match timeout {
            Some(d) => {
                tokio::select! {
                    biased;
                    res = stop.changed() => res.is_ok() && !*stop.borrow(),
                    _ = notify.notified() => !*stop.borrow(),
                    _ = tokio::time::sleep(d) => !*stop.borrow(),
                }
            }
            None => {
                tokio::select! {
                    biased;
                    res = stop.changed() => res.is_ok() && !*stop.borrow(),
                    _ = notify.notified() => !*stop.borrow(),
                }
            }
        }
    }
    /// Marks the server as logically stopping (`stopped() == true`) without waking
    /// `wait_stopped()` or cancelling transport tasks, so an in-flight `app.shutdown` RPC
    /// can flush its reply before runtime teardown begins.
    pub(crate) fn begin_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
    pub(crate) fn trigger_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.stop_tx.send(true);
        if let Some(tx) = self.tcp_listener_stop.lock().unwrap().take() {
            let _ = tx.send(true);
        }
        {
            let mut stopped = self.stop_cond.0.lock().unwrap();
            *stopped = true;
            self.stop_cond.1.notify_all();
        }
        self.wake_runtime();
        self.wake_delivery();
        self.wake_scripts();
        self.context_notify.notify_waiters();
        self.github_job_notify.notify_waiters();
        self.github_sync_notify.notify_waiters();
        self.herdr_notify.notify_waiters();
    }
    pub fn wait_stopped(&self) {
        let mut guard = self.stop_cond.0.lock().unwrap();
        while !*guard {
            guard = self.stop_cond.1.wait(guard).unwrap();
        }
    }
    pub fn stop_receiver(&self) -> tokio::sync::watch::Receiver<bool> {
        self.stop_tx.subscribe()
    }
    pub(crate) fn subscribe_push(&self) -> tokio::sync::broadcast::Receiver<state::PushEvent> {
        self.push_tx.subscribe()
    }
    #[cfg(test)]
    pub(crate) fn active_tcp_count(&self) -> usize {
        self.active_tcp_connections.load(Ordering::SeqCst)
    }
    pub fn endpoint(&self) -> PathBuf {
        self.home.join("app.sock")
    }
    pub fn server_id(&self) -> &str {
        &self.identity
    }
    pub fn issue_device_token(&self, name: &str) -> Result<(PairedDevice, String)> {
        self.device_store.issue_device(name)
    }
    pub fn create_pairing_code(&self, ttl_secs: u64) -> Result<(String, u64)> {
        self.pairing_codes.create_code(ttl_secs)
    }
    pub fn revoke_device(&self, device_id: &str) -> Result<bool> {
        let removed = self.device_store.revoke(device_id)?;
        self.device_revocations.revoke(device_id);
        Ok(removed)
    }
    pub fn paired_devices(&self) -> Vec<PairedDevice> {
        let mut items = self.device_store.list();
        items.extend(self.tailscale_devices.list());
        items.sort_by(|a, b| {
            let ka = a.last_seen_at.unwrap_or(a.created_at);
            let kb = b.last_seen_at.unwrap_or(b.created_at);
            kb.cmp(&ka).then_with(|| a.name.cmp(&b.name))
        });
        items
    }
    pub fn set_tailscale_verifier(
        &self,
        verifier: Option<Arc<dyn canban_transport::TailscalePeerVerifier>>,
    ) {
        *self.tailscale_verifier_override.lock().unwrap() = verifier;
    }
    pub fn api_version(&self) -> u32 {
        self.desktop_api_override
            .lock()
            .unwrap()
            .unwrap_or(API_VERSION)
    }
    pub fn set_desktop_api_override(&self, api: Option<u32>) {
        *self.desktop_api_override.lock().unwrap() = api;
    }
    pub(crate) fn effective_build(&self) -> canban_updater::Build {
        let mut build = canban_updater::Build::current();
        build.api = self.api_version();
        build
    }
    pub fn bound_tcp_addrs(&self) -> Vec<String> {
        self.bound_tcp.lock().unwrap().clone()
    }
    pub fn subscribe(&self) -> mpsc::Receiver<AppEvent> {
        let (tx, rx) = mpsc::channel();
        self.listeners.lock().unwrap().push(tx);
        rx
    }
    fn emit(&self, topic: &'static str, card: Option<String>) {
        if topic == "configuration" {
            self.herdr_notify.notify_one();
        }
        self.state_revision.fetch_add(1, Ordering::SeqCst);
        let gh_rev = if topic == "github" {
            self.github_revision.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            self.github_revision.load(Ordering::SeqCst)
        };
        let seq = self.push_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.push_tx.send(state::PushEvent::StateChanged {
            seq,
            topic,
            card_id: card.clone(),
            github_revision: gh_rev,
        });
        self.wake_runtime();
        let event = AppEvent::Changed {
            topic,
            card_id: card,
        };
        self.listeners
            .lock()
            .unwrap()
            .retain(|tx| tx.send(event.clone()).is_ok());
    }
    pub(crate) fn emit_update(&self) {
        let seq = self.push_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.push_tx.send(state::PushEvent::UpdateChanged {
            seq,
            state: self.update_state_payload(),
        });
    }
    pub(crate) fn emit_utilities(&self) {
        let seq = self.push_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.push_tx.send(state::PushEvent::UtilitiesChanged {
            seq,
            state: self.utilities.state(),
        });
    }
    pub(crate) fn emit_herdr_install(&self) {
        let seq = self.push_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.push_tx.send(state::PushEvent::HerdrInstall {
            seq,
            state: self.herdr_install_snapshot(),
        });
    }
    pub(crate) fn track_merging<'a>(&'a self, card_id: &str) -> MergingCard<'a> {
        let id = card_id.to_owned();
        if self.merging_cards.lock().unwrap().insert(id.clone()) {
            self.emit("review", Some(id.clone()));
        }
        MergingCard {
            server: self,
            card_id: id,
        }
    }
    pub(crate) fn track_merging_many<'a>(&'a self, card_ids: &[String]) -> MergingCards<'a> {
        let mut added = false;
        {
            let mut guard = self.merging_cards.lock().unwrap();
            for id in card_ids {
                if guard.insert(id.clone()) {
                    added = true;
                }
            }
        }
        if added {
            self.emit("review", None);
        }
        MergingCards {
            server: self,
            card_ids: card_ids.to_vec(),
        }
    }
    pub(crate) fn upsert_terminal_context(&self, context: TerminalContext) {
        let mut contexts = self.terminal_contexts.lock().unwrap();
        contexts.retain(|c| c.terminal_id != context.terminal_id);
        contexts.push(context);
        contexts.sort_by(|a, b| a.terminal_id.cmp(&b.terminal_id));
    }

    fn populate_terminal_contexts(&self, snapshot: &mut Snapshot) {
        snapshot.terminal_contexts = self
            .terminal_contexts
            .lock()
            .unwrap()
            .iter()
            .filter(|c| {
                snapshot.terminals.iter().any(|t| {
                    t.id == c.terminal_id
                        && Self::terminal_binding(snapshot, t).is_some_and(|(core, terminal)| {
                            core == c.core_id && terminal == c.terminal
                        })
                })
            })
            .cloned()
            .collect();
    }

    fn terminal_snapshot(&self) -> Result<Snapshot> {
        let mut snapshot = self.store.lock().unwrap().db().terminal_snapshot()?;
        self.populate_terminal_contexts(&mut snapshot);
        Ok(snapshot)
    }

    pub fn runtime_state(&self) -> Result<RuntimeState> {
        let mut snapshot = self.store.lock().unwrap().db().admission_snapshot()?;
        {
            let guard = self.merging_cards.lock().unwrap();
            if !guard.is_empty() {
                snapshot.merging_cards = guard
                    .iter()
                    .filter(|id| {
                        snapshot
                            .cards
                            .iter()
                            .any(|c| &c.id == *id && c.active() && !c.resolved())
                    })
                    .cloned()
                    .collect();
            }
        }
        self.populate_terminal_contexts(&mut snapshot);
        self.project_observation_freshness(&mut snapshot);
        self.populate_reclaim_status(&mut snapshot);
        let local_active = canban_scheduler::active_count_at(&snapshot, self.now_ms());
        let mut runtime = RuntimeState {
            terminals: snapshot.terminals,
            terminal_contexts: snapshot.terminal_contexts,
            reclaim_status: snapshot.reclaim_status,
            merging_cards: snapshot.merging_cards,
            terminal_hosts: std::collections::BTreeMap::new(),
            host_active_agents: std::collections::BTreeMap::from([(
                "local".to_owned(),
                local_active,
            )]),
        };
        runtime.register_host("local");
        Ok(runtime)
    }

    pub fn events(&self) -> Result<Vec<Event>> {
        self.store.lock().unwrap().db().list::<Event>()
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let (mut snapshot, mut card_events) = {
            let store = self.store.lock().unwrap();
            let snapshot = store.snapshot()?;
            let card_events = store.db().card_events()?;
            (snapshot, card_events)
        };
        snapshot.github_sync = self.github_jobs.lock().unwrap().status.clone();
        snapshot.github = serde_json::from_value(serde_json::to_value(self.github.status())?)?;
        self.project_github(&mut snapshot);
        snapshot.merging_cards = self
            .merging_cards
            .lock()
            .unwrap()
            .iter()
            .filter(|id| {
                snapshot
                    .cards
                    .iter()
                    .any(|c| &c.id == *id && c.active() && !c.resolved())
            })
            .cloned()
            .collect();
        self.populate_terminal_contexts(&mut snapshot);
        self.project_observation_freshness(&mut snapshot);
        self.populate_reclaim_status(&mut snapshot);

        let present_cards: std::collections::BTreeSet<&str> =
            snapshot.cards.iter().map(|c| c.id.as_str()).collect();
        card_events.retain(|e| present_cards.contains(e.card_id.as_str()));

        // Enrich only unread PR comment/review events so ReminderEventMeta.summary has body text.
        if let Ok(client) = self.github_cached_client() {
            let read_seqs: std::collections::BTreeMap<&str, u64> = snapshot
                .card_reads
                .iter()
                .map(|r| (r.card_id.as_str(), r.sequence))
                .collect();
            let mut unread_pr_indices = Vec::new();
            for (idx, ev) in card_events.iter().enumerate() {
                if matches!(
                    ev.kind.as_str(),
                    "github.pr.commented" | "github.pr.reviewed"
                ) {
                    let read_seq = read_seqs.get(ev.card_id.as_str()).copied().unwrap_or(0);
                    if ev.sequence > read_seq {
                        unread_pr_indices.push(idx);
                    }
                }
            }
            if !unread_pr_indices.is_empty() {
                let mut subset: Vec<Event> = unread_pr_indices
                    .iter()
                    .map(|&i| card_events[i].clone())
                    .collect();
                github::projection::enrich_pr_events(&client, &mut subset);
                for (i, ev) in unread_pr_indices.into_iter().zip(subset) {
                    card_events[i] = ev;
                }
            }
        }

        let mut by_card: std::collections::BTreeMap<&str, Vec<&Event>> =
            std::collections::BTreeMap::new();
        for ev in &card_events {
            by_card.entry(ev.card_id.as_str()).or_default().push(ev);
        }
        for (card_id, evs) in &by_card {
            let last_event_seq = evs.iter().map(|e| e.sequence).max().unwrap_or(0);
            let reply_count = evs.iter().filter(|e| is_narrative_event(e, evs)).count() as u32;
            snapshot.card_summaries.insert(
                (*card_id).to_owned(),
                CardEventSummary {
                    last_event_seq,
                    reply_count,
                },
            );
        }
        for t in &mut snapshot.terminals {
            if let Some(a) = &mut t.agent
                && !a.finished()
                && a.started_at.is_none_or(|ts| ts <= 1_000_000_000_000)
                && let Some(evs) = by_card.get(a.card_id.as_str())
                && let Some(ev) = evs.iter().rev().find(|e| {
                    matches!(e.kind.as_str(), "execution.started" | "execution.continued")
                        && e.data["run_number"].as_u64() == Some(u64::from(a.number))
                        && e.created_at > 1_000_000_000_000
                })
            {
                a.started_at = Some(ev.created_at);
            }
        }
        snapshot.reminders =
            canban_notifications::reminders_from_events_at(&snapshot, &card_events, self.now_ms());
        snapshot.sound_event_ids =
            canban_notifications::sound_event_ids_from_events(&snapshot, &card_events);
        Ok(snapshot)
    }
}

fn is_pr_narrative_event(event: &Event) -> bool {
    match event.kind.as_str() {
        "github.pr.commented" => true,
        "github.pr.reviewed" => {
            event.data["projected_body"]
                .as_str()
                .is_some_and(|b| !b.trim().is_empty())
                || event.data["has_body"] == true
        }
        _ => false,
    }
}

fn is_echoing_reply(event: &Event, other: &Event) -> bool {
    other.kind == "comment.replied"
        && other.data["terminal_id"] == event.data["terminal_id"]
        && event.data["terminal_id"].as_str().is_some()
        && ((event.data["report_id"].as_str().is_some()
            && other.data["report_id"] == event.data["report_id"])
            || (event.data["path"].as_str().is_some() && other.data["path"] == event.data["path"]))
}

fn is_narrative_event(event: &Event, events: &[&Event]) -> bool {
    let echoed_report = event.kind == "execution.reported"
        && (event.data["comment_replied"] == true
            || events.iter().any(|other| is_echoing_reply(event, other)));
    matches!(
        event.kind.as_str(),
        "comment.created" | "comment.replied" | "github.comment"
    ) || is_pr_narrative_event(event)
        || (event.kind == "execution.reported" && !echoed_report)
}

impl AppServer {
    fn project_observation_freshness(&self, snapshot: &mut Snapshot) {
        let inv = self.inventory.lock().unwrap();
        if inv.error.is_none() && inv.last_at > 0 {
            for t in &mut snapshot.terminals {
                if let Some(a) = &mut t.agent
                    && a.connection_state == "connected"
                    && a.core_id.as_deref() == Some(&inv.core)
                {
                    a.lifecycle.observed_at =
                        Some(a.lifecycle.observed_at.unwrap_or(0).max(inv.last_at));
                }
            }
        }
    }
    pub fn config(&self) -> Config {
        self.config.read().unwrap().clone()
    }
    fn lookup_cached_operation(db: Db<'_>, key: &str, fingerprint: &str) -> Result<Option<Value>> {
        let Some((old, result)) = db
            .conn
            .query_row(
                "SELECT fingerprint,result FROM operations WHERE id=?1",
                [key],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
        else {
            return Ok(None);
        };
        ensure!(
            old == fingerprint,
            "request_key_conflict: same key, different payload"
        );
        Ok(Some(serde_json::from_str(&result)?))
    }

    fn cached_operation(&self, method: &str, p: &Value) -> Result<Option<Value>> {
        let Some(key) = p["request_key"].as_str() else {
            return Ok(None);
        };
        let fingerprint = canban_context::digest(&serde_json::to_vec(&json!([method, p]))?);
        let store = self.store.lock().unwrap();
        Self::lookup_cached_operation(store.db(), key, &fingerprint)
    }

    fn mutate(
        &self,
        method: &str,
        p: &Value,
        f: impl FnOnce(Db<'_>) -> Result<Value>,
    ) -> Result<Value> {
        let fingerprint = canban_context::digest(&serde_json::to_vec(&json!([method, p]))?);
        let key = p["request_key"].as_str();
        let result = self.store.lock().unwrap().write(|db| {
            if let Some(key) = key
                && let Some(cached) = Self::lookup_cached_operation(db, key, &fingerprint)?
            {
                return Ok(cached);
            }
            events::pump_at_with_prune_state(
                db,
                &self.config(),
                self.now_ms(),
                Some(&self.last_cardless_prune_at),
            )?;
            let result = f(db)?;
            events::pump_at_with_prune_state(
                db,
                &self.config(),
                self.now_ms(),
                Some(&self.last_cardless_prune_at),
            )?;
            if let Some(key) = key {
                db.conn.execute(
                    "INSERT INTO operations(id,fingerprint,result) VALUES(?1,?2,?3)",
                    params![key, fingerprint, serde_json::to_string(&result)?],
                )?;
            }
            Ok(result)
        })?;
        let _ = self.pump_admissions();
        self.emit("application", p["card_id"].as_str().map(str::to_owned));
        self.request_deliveries();
        self.request_scripts();
        Ok(result)
    }
}

mod client;
mod commands;
mod delivery_gates;
mod diagnostics;
mod drafts;
mod env;
mod github;
pub mod hosts;
pub mod network;
pub mod representation;
pub mod startup_progress;
mod state;
pub use client::{AppClient, BackendOlder, ClientEndpoint, ClientPushEvent, ConnectProgress};
mod activity;
mod adoption;
mod agents;
mod authoring;
mod configuration;
mod events;
mod herdr_setup;
mod merge_batch;
mod project_bundle;
mod projects;
mod workflow;

mod directory;
mod lifecycle;
mod notes;
mod observation;
mod overview;
mod presence;
mod refine;
mod reminders;
mod reports;
mod resources;
mod runtime;
mod schedules;
mod scripts;
mod sessions;
mod terminals;
mod worktrees;

pub fn default_home() -> Result<PathBuf> {
    if let Some(v) = std::env::var_os("CANBAN_HOME") {
        return Ok(v.into());
    }
    if let Some(instance) = instance::bundled()? {
        return Ok(instance.home);
    }
    Ok(PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".canban"))
}
pub fn rpc(endpoint: &Path, method: &str, params: Value) -> Result<Value> {
    let _rpc = canban_profiler::Span::new("rpc.request");
    canban_client::rpc_once(endpoint, method, params)
}

#[cfg(test)]
mod tests;

mod git;
mod review;

mod utilities;
