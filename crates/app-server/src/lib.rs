use anyhow::{Context, Result, bail, ensure};
use canban_storage::{Db, Store};
use canban_types::*;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

/// Increment when desktop clients need operations unavailable in older services.
pub const API_VERSION: u32 = canban_updater::DESKTOP_API;
pub mod instance;
mod updates;

pub struct AppServer {
    _development: Option<instance::Registration>,
    updates_disabled: bool,
    _service_lock: std::fs::File,
    utilities: utilities::Utilities,
    pub home: PathBuf,
    github: Arc<canban_github::auth::Service>,
    github_sync_due: AtomicBool,
    github_revision: std::sync::atomic::AtomicU64,
    github_cli: Arc<canban_github::cli::Service>,
    github_queries: Mutex<std::collections::BTreeMap<String, Value>>,
    identity: String,
    store: Mutex<Store>,
    config: RwLock<Config>,
    cli: PathBuf,
    listeners: Mutex<Vec<mpsc::Sender<AppEvent>>>,
    stop: AtomicBool,
    review_gate: Mutex<()>,
    merging_cards: Mutex<std::collections::BTreeSet<String>>,
    terminal_gate: Mutex<()>,
    terminal_contexts: Mutex<Vec<TerminalContext>>,
    inventory: Mutex<observation::Inventory>,
    observer_running: AtomicBool,
    remote_health_due: AtomicBool,
    check_slots: Mutex<std::collections::BTreeSet<String>>,
    check_signal: std::sync::Condvar,
    runtime_wake: Mutex<Option<mpsc::SyncSender<()>>>,
    delivery_wake: Mutex<Option<mpsc::SyncSender<()>>>,
    contexts: Mutex<directory::ContextQueue>,
    context_signal: std::sync::Condvar,
    remote_workers: Mutex<std::collections::HashSet<String>>,
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
}
impl Drop for Running {
    fn drop(&mut self) {
        self.server.stop.store(true, Ordering::SeqCst);
        self.server.wake_runtime();
        self.server.wake_delivery();
        self.server.context_signal.notify_all();
    }
}
pub(crate) struct MergingCard<'a> {
    server: &'a AppServer,
    card_id: String,
}
impl Drop for MergingCard<'_> {
    fn drop(&mut self) {
        self.server
            .merging_cards
            .lock()
            .unwrap()
            .remove(&self.card_id);
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
fn log(
    db: Db<'_>,
    c: &Card,
    kind: &str,
    actor: &str,
    message: &str,
    terminal_id: Option<String>,
    mut data: Value,
) -> Result<()> {
    if let Some(tid) = terminal_id
        && let Some(obj) = data.as_object_mut()
    {
        obj.entry("terminal_id".to_owned())
            .or_insert_with(|| json!(tid));
    }
    db.append(canban_timeline::event(
        &c.project_id,
        &c.id,
        kind,
        actor,
        message,
        data,
    ))?;
    Ok(())
}
fn attention(db: Db<'_>, card: &mut Card, message: &str) -> Result<()> {
    card.attention = Some(message.into());
    if card.human_gate.is_none() {
        card.human_gate = Some(HumanGate {
            reason: message.into(),
            terminal_id: None,
            next_stage: None,
        });
    }
    db.save(&canban_notifications::attention(
        &card.project_id,
        &card.id,
        message,
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
pub(crate) fn resolve_card(db: Db<'_>, p: &Value) -> Result<Card> {
    if let Some(raw_id) = p["card_id"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Ok(card) = db.get::<Card>(raw_id) {
            return Ok(card);
        }
        if parse_card_number(&json!(raw_id)).is_none() {
            bail!("Card not found: {raw_id}");
        }
    }
    let number = parse_card_number(&p["card"])
        .or_else(|| parse_card_number(&p["card_id"]))
        .context("Supply --card <number> (or CANBAN_CARD) or card_id")?;
    let cards = db.list::<Card>()?;
    if let Some(pid) = p["project_id"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
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
            let dir = proj.work_location().directory;
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
            attention(db, card, &e.to_string())?;
            log(
                db,
                card,
                "scheduler.blocked",
                "system",
                &e.to_string(),
                None,
                json!({}),
            )?;
            return Ok(None);
        }
    };
    let context = crate::sessions::conversation(db, card, &profile)?;
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
        agent.pending = Some(QueuedAssignment {
            stage: stage.id.clone(),
            trigger: trigger.clone(),
            profile,
            instruction: instruction_str,
            on_completed,
            require_human_review,
            workflow_run_id: None,
            workflow_step: None,
            queued_at: now(),
        });
        agent.number.saturating_add(1)
    } else {
        let number = u32::try_from(db.counter(&format!("card-attempt:{}", card.id))?)?;
        let prior_agent = entry.agent.clone();
        let mut run = canban_scheduler::agent_run(
            &project.id,
            &card.id,
            number,
            &stage.id,
            trigger.clone(),
            profile,
            instruction_str,
            on_completed,
        );
        run.require_human_review = require_human_review;
        if let Some(prev) = prior_agent
            && explicit.is_some()
            && context.provider_session_id.is_some()
            && prev.provider_session_id == context.provider_session_id
        {
            run.resume_session = true;
            run.runtime_home = prev.runtime_home;
            run.workspace = prev.workspace.or_else(|| {
                prev.location.map(|location| WorkspaceBinding {
                    location,
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
    )?;
    Ok(Some(entry))
}
impl AppServer {
    pub fn open(home: PathBuf, cli: PathBuf) -> Result<Arc<Self>> {
        let development = instance::bundled()?;
        if let Some(instance) = &development {
            instance.check_home(&home)?;
        }
        let updates_disabled = development.is_some();
        let _startup = canban_updater::handoff::startup_guard(&home)?;
        let service_lock = canban_updater::service_lock(&home)?;
        let mut store = Store::open(&home)?;
        let identity_path = home.join("server-id");
        let mut identity_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&identity_path)?;
        identity_file.lock()?;
        if identity_file.metadata()?.len() == 0 {
            identity_file.write_all(id().as_bytes())?;
            identity_file.sync_all()?;
        }
        let identity = std::fs::read_to_string(&identity_path)?;
        drop(identity_file);
        ensure!(
            identity.len() == 32 && identity.chars().all(|c| c.is_ascii_hexdigit()),
            "Invalid AppServer identity"
        );
        // Existing attention is a real pause after upgrading, not merely a badge.
        store.write(|db| {
            for mut project in db.list::<Project>()? {
                if project.location.is_none() {
                    project.location = Some(project.work_location());
                    db.save(&project)?;
                }
            }
            for mut card in db.list::<Card>()? {
                if card.workspace.is_none()
                    && let Some(path) = &card.worktree
                {
                    let p: Project = db.get(&card.project_id)?;
                    card.workspace = Some(WorkspaceBinding {
                        location: p.work_location(),
                        path: path.clone(),
                        branch: card.branch.clone(),
                        base_commit: card.base_commit.clone(),
                        target_branch: p.target_branch.clone(),
                    });
                    db.save(&card)?;
                }
            }
            for mut t in db.list::<TerminalEntry>()? {
                if let Some(a) = &mut t.agent
                    && a.location.is_none()
                    && a.state != "queued"
                {
                    let p: Project = db.get(&a.project_id)?;
                    let location = WorkLocation {
                        machine: "local".into(),
                        directory: p.directory.clone(),
                    };
                    a.location = Some(location.clone());
                    a.owner_core_id = a.core_id.clone();
                    a.workspace = Some(WorkspaceBinding {
                        location,
                        path: a.cwd.clone(),
                        branch: None,
                        base_commit: None,
                        target_branch: p.target_branch.clone(),
                    });
                    db.save(&t)?;
                }
            }
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
            canban_context::atomic(&path, &serde_json::to_vec_pretty(&Config::default())?)?;
        }
        canban_context::materialize_skill(&home)?;
        let (config, migration) = Self::read_config(&home)?;
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
        Self::publish_migration(&home, migration)?;
        store.write(|db| {
            workflow::initialize(db, &config)?;
            for project in db.list::<Project>()? {
                authoring::initialize(&home, db, &config, &project)?;
            }
            Ok(())
        })?;
        Ok(Arc::new(Self {
            _development: development
                .as_ref()
                .map(|v| instance::register(v, "server"))
                .transpose()?,
            updates_disabled,
            _service_lock: service_lock,
            utilities: utilities::Utilities::new(&home),
            github_sync_due: AtomicBool::new(true),
            github_revision: std::sync::atomic::AtomicU64::new(0),
            github_queries: Mutex::new(
                std::fs::read(home.join("cache/github/queries.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default(),
            ),
            github_cli: Arc::new(canban_github::cli::Service::new(
                home.join("cache/github/gh"),
            )),
            github: Arc::new(canban_github::auth::Service::new(home.clone())),
            identity,
            home,
            store: Mutex::new(store),
            config: RwLock::new(config),
            cli,
            listeners: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
            review_gate: Mutex::new(()),
            merging_cards: Mutex::new(Default::default()),
            terminal_gate: Mutex::new(()),
            terminal_contexts: Mutex::new(Vec::new()),
            inventory: Mutex::new(Default::default()),
            observer_running: AtomicBool::new(false),
            remote_health_due: AtomicBool::new(false),
            check_slots: Mutex::new(Default::default()),
            check_signal: std::sync::Condvar::new(),
            runtime_wake: Mutex::new(None),
            delivery_wake: Mutex::new(None),
            contexts: Mutex::new(Default::default()),
            context_signal: std::sync::Condvar::new(),
            remote_workers: Mutex::new(Default::default()),
        }))
    }
    fn read_config(home: &Path) -> Result<(Config, Option<Vec<u8>>)> {
        let bytes = std::fs::read(home.join("config/agents.json"))?;
        let mut value: Value = serde_json::from_slice(&bytes)?;
        let changed = canban_profiles::migrate(&mut value)?;
        let raw: Config = serde_json::from_value(value.clone())?;
        let config = canban_profiles::resolve(raw, home)?;
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
    fn publish_migration(home: &Path, bytes: Option<Vec<u8>>) -> Result<()> {
        if let Some(bytes) = bytes {
            let path = home.join("config/agents.json");
            let backup = home
                .join("config")
                .join(format!("agents.legacy-{}.json", id()));
            canban_context::atomic(&backup, &std::fs::read(&path)?)?;
            canban_context::atomic(&path, &bytes)?;
        }
        Ok(())
    }
    fn validate_config(c: &Config) -> Result<()> {
        ensure!(
            c.relay_endpoint.is_none(),
            "relay_endpoint is a client transport option; do not configure it on AppServer"
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
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
    pub fn endpoint(&self) -> PathBuf {
        self.home.join("app.sock")
    }
    pub fn subscribe(&self) -> mpsc::Receiver<AppEvent> {
        let (tx, rx) = mpsc::channel();
        self.listeners.lock().unwrap().push(tx);
        rx
    }
    fn emit(&self, topic: &'static str, card: Option<String>) {
        if topic == "github" {
            self.github_revision.fetch_add(1, Ordering::SeqCst);
        }
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
    pub fn snapshot(&self) -> Result<Snapshot> {
        let mut snapshot = self.store.lock().unwrap().snapshot()?;
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
                    .any(|c| &c.id == *id && c.active() && !c.merged())
            })
            .cloned()
            .collect();
        snapshot.terminal_contexts = self
            .terminal_contexts
            .lock()
            .unwrap()
            .iter()
            .filter(|c| {
                snapshot.terminals.iter().any(|t| {
                    t.id == c.terminal_id
                        && Self::terminal_binding(&snapshot, t).is_some_and(|(core, terminal)| {
                            core == c.core_id && terminal == c.terminal
                        })
                })
            })
            .cloned()
            .collect();
        let inv = self.inventory.lock().unwrap();
        if inv.error.is_none() && inv.last_at > 0 {
            for t in &mut snapshot.terminals {
                if let Some(a) = &mut t.agent
                    && a.connection_state == "connected"
                    && a.core_id.as_deref() == Some(&inv.core)
                {
                    a.lifecycle.observed_at = Some(inv.last_at);
                }
            }
        }
        Ok(snapshot)
    }
    pub fn config(&self) -> Config {
        self.config.read().unwrap().clone()
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
                && let Some((old, result)) = db
                    .conn
                    .query_row(
                        "SELECT fingerprint,result FROM operations WHERE id=?1",
                        [key],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                    )
                    .optional()?
            {
                ensure!(
                    old == fingerprint,
                    "request_key_conflict: same key, different payload"
                );
                return Ok(serde_json::from_str(&result)?);
            }
            events::pump(db, &self.config())?;
            let result = f(db)?;
            events::pump(db, &self.config())?;
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
        Ok(result)
    }
}

mod client;
mod commands;
mod drafts;
mod github;
pub use client::{AppClient, BackendOlder};
mod agents;
mod authoring;
mod configuration;
mod events;
mod machines;
mod remote;
mod workflow;

mod directory;
mod lifecycle;
mod notes;
mod observation;
mod overview;
mod reminders;
mod reports;
mod resources;
mod runtime;
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
    let mut stream = UnixStream::connect(endpoint)
        .with_context(|| format!("Canban is not running at {}", endpoint.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(90)))?;
    writeln!(
        stream,
        "{}",
        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
    )?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let reply: Value = serde_json::from_str(&line)?;
    ensure!(
        reply.get("error").is_none(),
        "{}",
        reply["error"]["message"]
    );
    reply
        .get("result")
        .cloned()
        .context("Invalid Canban response")
}

#[cfg(test)]
mod tests;

mod review;

mod utilities;
