//! Unified startup bootstrap gate, Herdr Core (`hc`) setup, backend upgrade
//! handoff, and graceful controller-requested desktop shutdown.
use anyhow::{Context, Result, ensure};
use canban_app_server::instance::{self, Registration};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::{
    rc::Rc,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const DISPLAY_INSTALL_COMMAND: &str = "curl -fsSL https://releases.herdr.dev/core/install.sh | sh";

static QUIT: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static UI_RECORD: OnceLock<std::path::PathBuf> = OnceLock::new();
static ERROR: OnceLock<String> = OnceLock::new();
static OLDER: OnceLock<canban_app_server::BackendOlder> = OnceLock::new();
static FATAL_EXIT: AtomicBool = AtomicBool::new(false);

pub fn prepare() -> Result<Option<Registration>> {
    let flag = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(signal, flag.clone())?;
    }
    let _ = QUIT.set(flag);
    let Some(instance) = instance::bundled()? else {
        return Ok(None);
    };
    instance.check_home(&canban_app_server::default_home()?)?;
    ensure!(
        std::env::var_os("CANBAN_ENDPOINT").is_none(),
        "A bound development app cannot attach to another endpoint; start its instance with mise run dev"
    );
    if let Ok(id) = std::env::var("CANBAN_APP_ID") {
        ensure!(
            id == instance.app_id,
            "Development app identity does not match its binding"
        );
    }
    let registration = instance::register(&instance, "ui")?;
    let _ = UI_RECORD.set(instance.directory.join("ui.json"));
    Ok(Some(registration))
}

pub fn app_id() -> Result<String> {
    Ok(instance::bundled()?.map(|v| v.app_id).unwrap_or_else(|| {
        std::env::var("CANBAN_APP_ID").unwrap_or_else(|_| "com.canban.desktop".into())
    }))
}

pub fn watch_quit() {
    set_interval(Duration::from_millis(100), || {
        if QUIT.get().is_some_and(|v| v.swap(false, Ordering::SeqCst)) {
            nsh_core::element::desktop::quit();
        }
    });
}

fn record(field: &str, value: Value) {
    if let Some(path) = UI_RECORD.get()
        && let Ok(bytes) = std::fs::read(path)
        && let Ok(mut saved) = serde_json::from_slice::<Value>(&bytes)
    {
        saved[field] = value;
        let _ = canban_context::atomic(path, &serde_json::to_vec(&saved).unwrap_or_default());
    }
}

pub(crate) fn record_error(detail: &str) {
    FATAL_EXIT.store(true, Ordering::SeqCst);
    record("error", detail.into());
}

pub(crate) fn clear_fatal_exit() {
    FATAL_EXIT.store(false, Ordering::SeqCst);
}

pub fn fatal_exit() -> bool {
    FATAL_EXIT.load(Ordering::SeqCst)
}

pub fn ready() {
    request_animation_frame(|_| record("ready", true.into()));
}

fn write_upgrade_ready(home: &std::path::Path) {
    if let Ok(token) = std::env::var("CANBAN_UPGRADE_TOKEN") {
        let path = home.join("updates").join(format!("ui-ready-{token}"));
        let _ = canban_updater::atomic(
            &path,
            &serde_json::to_vec(&canban_updater::Build::current()).unwrap_or_default(),
        );
    }
}

/// Pre-window fallback when `prepare()` or CLI discovery fails before the main
/// desktop root can mount.
pub fn show(error: &anyhow::Error) {
    if let Some(old) = error.downcast_ref::<canban_app_server::BackendOlder>()
        && !old.explicit_endpoint
        && instance::bundled().ok().flatten().is_none()
    {
        let _ = OLDER.set(old.clone());
    }
    let detail = format!("{error:#}");
    record_error(&detail);
    let _ = ERROR.set(detail);
    let app_id = app_id().unwrap_or_else(|_| "com.canban.startup-error".into());
    let _ = crate::platform::run_root::<StartupError>(app_id, "Canban could not start", (800, 520));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Active,
    Ready,
    Attention,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubStepState {
    Pending,
    Active,
    Ok,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootSubStep {
    pub id: String,
    pub label: String,
    pub state: SubStepState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootLogEntry {
    pub elapsed_ms: u64,
    pub tag: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootProgress {
    pub home: String,
    pub app_state: StepState,
    pub app_detail: String,
    pub app_badge_suffix: Option<String>,
    pub app_elapsed_ms: Option<u64>,
    pub app_started_ms: u64,
    pub app_substeps: Vec<BootSubStep>,
    pub herdr_state: StepState,
    pub herdr_detail: String,
    pub herdr_badge_suffix: Option<String>,
    pub herdr_elapsed_ms: Option<u64>,
    pub herdr_started_ms: Option<u64>,
    pub herdr_substeps: Vec<BootSubStep>,
    pub logs: Vec<BootLogEntry>,
    pub percent: u8,
    pub status_line: String,
    pub boot_complete: bool,
}

const MAX_BOOT_LOGS: usize = 24;

fn compact_user_path(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    compact_home(path, &home)
}

impl BootProgress {
    pub fn initial(home: String) -> Self {
        let compact = compact_user_path(&home);
        Self {
            home,
            app_state: StepState::Active,
            app_detail: "Connecting to local AppServer…".into(),
            app_badge_suffix: None,
            app_elapsed_ms: None,
            app_started_ms: 0,
            app_substeps: vec![
                BootSubStep {
                    id: "connect".into(),
                    label: "Connect to local AppServer (app.sock)".into(),
                    state: SubStepState::Active,
                },
                BootSubStep {
                    id: "storage".into(),
                    label: "Open SQLite WAL store & check schema".into(),
                    state: SubStepState::Pending,
                },
                BootSubStep {
                    id: "workflow".into(),
                    label: "Initialize workflows & verify durable snapshot".into(),
                    state: SubStepState::Pending,
                },
            ],
            herdr_state: StepState::Pending,
            herdr_detail: "Waiting for AppServer…".into(),
            herdr_badge_suffix: None,
            herdr_elapsed_ms: None,
            herdr_started_ms: None,
            herdr_substeps: Vec::new(),
            logs: vec![BootLogEntry {
                elapsed_ms: 0,
                tag: "HOME".into(),
                message: compact,
            }],
            percent: 10,
            status_line: "Connecting to local AppServer…".into(),
            boot_complete: false,
        }
    }

    fn push_log(&mut self, elapsed_ms: u64, tag: &str, message: impl Into<String>) {
        let message = message.into();
        if self
            .logs
            .last()
            .is_some_and(|last| last.tag == tag && last.message == message)
        {
            return;
        }
        self.logs.push(BootLogEntry {
            elapsed_ms,
            tag: tag.to_owned(),
            message,
        });
        if self.logs.len() > MAX_BOOT_LOGS {
            let drain = self.logs.len() - MAX_BOOT_LOGS;
            self.logs.drain(0..drain);
        }
    }

    fn set_app_substep(&mut self, id: &str, state: SubStepState, label: Option<&str>) {
        if let Some(step) = self.app_substeps.iter_mut().find(|s| s.id == id) {
            step.state = state;
            if let Some(text) = label {
                step.label = text.to_owned();
            }
        }
    }

    pub fn apply_connect_progress(
        &mut self,
        progress: &canban_app_server::ConnectProgress,
        elapsed_ms: u64,
    ) {
        self.push_log(elapsed_ms, &progress.tag, progress.detail.clone());
        match progress.phase.as_str() {
            "handoff" => {
                self.app_detail = progress.detail.clone();
                self.set_app_substep("connect", SubStepState::Active, Some(&progress.detail));
                self.percent = self.percent.max(20);
                self.status_line = "Resuming backend upgrade…".into();
            }
            "spawn" => {
                self.app_detail = "Starting local AppServer daemon…".into();
                self.set_app_substep(
                    "connect",
                    SubStepState::Active,
                    Some("Spawn local AppServer daemon (`canban serve`)"),
                );
                self.percent = self.percent.max(24);
                self.status_line = "Starting local AppServer daemon…".into();
            }
            "lock" => {
                self.set_app_substep(
                    "connect",
                    SubStepState::Ok,
                    Some("Acquire service lock & verify server identity"),
                );
                self.set_app_substep("storage", SubStepState::Active, None);
                self.percent = self.percent.max(32);
                self.status_line = "Acquiring service lock…".into();
            }
            "storage" => {
                self.set_app_substep(
                    "connect",
                    SubStepState::Ok,
                    Some("Acquire service lock & verify server identity"),
                );
                self.set_app_substep("storage", SubStepState::Active, Some(&progress.detail));
                self.percent = self.percent.max(40);
                self.status_line = "Opening SQLite database…".into();
            }
            "migrate" => {
                self.set_app_substep(
                    "connect",
                    SubStepState::Ok,
                    Some("Acquire service lock & verify server identity"),
                );
                self.set_app_substep("storage", SubStepState::Active, Some(&progress.detail));
                self.percent = self.percent.max(48);
                self.status_line = "Migrating database…".into();
            }
            "storage_ready" => {
                self.set_app_substep(
                    "connect",
                    SubStepState::Ok,
                    Some("Acquire service lock & verify server identity"),
                );
                self.set_app_substep("storage", SubStepState::Ok, Some(&progress.detail));
                self.set_app_substep("workflow", SubStepState::Active, None);
                self.percent = self.percent.max(56);
                self.status_line = "Loading configuration…".into();
            }
            "config" => {
                self.set_app_substep("connect", SubStepState::Ok, None);
                self.set_app_substep("storage", SubStepState::Ok, None);
                self.set_app_substep("workflow", SubStepState::Active, Some(&progress.detail));
                self.percent = self.percent.max(60);
                self.status_line = "Loading agent profiles…".into();
            }
            "workflow" => {
                self.set_app_substep("connect", SubStepState::Ok, None);
                self.set_app_substep("storage", SubStepState::Ok, None);
                self.set_app_substep("workflow", SubStepState::Active, Some(&progress.detail));
                self.percent = self.percent.max(66);
                self.status_line = "Validating project workflows…".into();
            }
            "snapshot" => {
                self.set_app_substep("connect", SubStepState::Ok, None);
                self.set_app_substep("storage", SubStepState::Ok, None);
                self.set_app_substep("workflow", SubStepState::Active, Some(&progress.detail));
                self.percent = self.percent.max(72);
                self.status_line = "Verifying durable snapshot…".into();
            }
            "ready" => {
                self.set_app_substep("connect", SubStepState::Ok, None);
                self.set_app_substep("storage", SubStepState::Ok, None);
                self.set_app_substep("workflow", SubStepState::Ok, Some(&progress.detail));
                self.percent = self.percent.max(74);
                self.status_line = "Connecting to AppServer RPC…".into();
            }
            "rpc" => {
                for step in &mut self.app_substeps {
                    step.state = SubStepState::Ok;
                }
                self.percent = self.percent.max(76);
                self.status_line = "AppServer online…".into();
            }
            _ => {
                self.app_detail = progress.detail.clone();
            }
        }
    }

    pub fn mark_app_ready(&mut self, api_version: u32, server_id: Option<&str>, elapsed_ms: u64) {
        self.app_state = StepState::Ready;
        self.app_elapsed_ms = Some(elapsed_ms.saturating_sub(self.app_started_ms));
        self.app_badge_suffix = Some(format!("API v{api_version}"));
        self.app_detail = match server_id {
            Some(id) if !id.is_empty() => {
                let short = &id[..id.len().min(8)];
                format!("Local daemon online (server {short})")
            }
            _ => "Local daemon online".into(),
        };
        self.app_substeps.clear();

        self.herdr_state = StepState::Active;
        self.herdr_started_ms = Some(elapsed_ms);
        self.herdr_detail = "Probing Herdr Core (`hc`) runtime…".into();
        self.herdr_substeps = vec![
            BootSubStep {
                id: "locate".into(),
                label: "Locate `hc` binary & resolve runtime".into(),
                state: SubStepState::Active,
            },
            BootSubStep {
                id: "handshake".into(),
                label: "Handshake with persistent PTY supervisor".into(),
                state: SubStepState::Pending,
            },
        ];
        self.push_log(elapsed_ms, "HERDR", "probing `hc` runtime daemon…");
        self.percent = self.percent.max(82);
        self.status_line = "Verifying Herdr Core…".into();
    }

    pub fn mark_herdr_ready(
        &mut self,
        version: Option<&str>,
        resolved_hc: Option<&str>,
        elapsed_ms: u64,
    ) {
        self.herdr_state = StepState::Ready;
        self.herdr_elapsed_ms =
            Some(elapsed_ms.saturating_sub(self.herdr_started_ms.unwrap_or(elapsed_ms)));
        self.herdr_badge_suffix = version.filter(|v| !v.is_empty()).map(|v| format!("hc {v}"));
        self.herdr_detail = match resolved_hc.filter(|p| !p.is_empty()) {
            Some(path) => format!(
                "Persistent PTY supervisor online ({})",
                compact_user_path(path)
            ),
            None => "Persistent PTY supervisor online".into(),
        };
        self.herdr_substeps.clear();
        let msg = match (
            version.filter(|v| !v.is_empty()),
            resolved_hc.filter(|p| !p.is_empty()),
        ) {
            (Some(v), Some(p)) => format!("hc {v} online ({})", compact_user_path(p)),
            (Some(v), None) => format!("hc {v} online"),
            (None, Some(p)) => format!("hc runtime online ({})", compact_user_path(p)),
            (None, None) => "hc runtime online".into(),
        };
        self.push_log(elapsed_ms, "HERDR", msg);
        self.percent = self.percent.max(90);
        self.status_line = "Loading workspace snapshot…".into();
    }

    pub fn mark_snapshot_loading(&mut self, elapsed_ms: u64) {
        self.push_log(
            elapsed_ms,
            "SNAPSHOT",
            "hydrating projects and workspace state…",
        );
        self.percent = self.percent.max(94);
        self.status_line = "Loading workspace snapshot…".into();
    }

    pub fn mark_boot_complete(
        &mut self,
        projects: Option<usize>,
        cards: Option<usize>,
        elapsed_ms: u64,
    ) {
        if let (Some(p), Some(c)) = (projects, cards) {
            self.push_log(
                elapsed_ms,
                "SNAPSHOT",
                format!("loaded {p} projects, {c} cards"),
            );
            if !self.app_detail.contains("projects") {
                self.app_detail = format!("{} · {p} projects", self.app_detail);
            }
        }
        self.percent = 100;
        self.boot_complete = true;
        self.status_line = "Boot complete. Mounting workspace…".into();
    }
}

enum BootEvent {
    Connect(canban_app_server::ConnectProgress, u64),
    AppReady {
        api_version: u32,
        server_id: Option<String>,
        elapsed_ms: u64,
    },
    HerdrReady {
        version: Option<String>,
        resolved_hc: Option<String>,
        elapsed_ms: u64,
    },
    SnapshotLoading {
        elapsed_ms: u64,
    },
    BootComplete {
        projects: Option<usize>,
        cards: Option<usize>,
        elapsed_ms: u64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct HerdrSetupView {
    pub home: String,
    pub api_version: u32,
    pub installed: bool,
    pub default_hc: bool,
    pub configured_hc: String,
    pub resolved_hc: Option<String>,
    pub install_command: String,
    pub error: Option<String>,
    pub installing: bool,
    pub checking: bool,
    pub logs: Vec<String>,
}

impl Default for HerdrSetupView {
    fn default() -> Self {
        Self {
            home: "~/.canban".into(),
            api_version: canban_app_server::API_VERSION,
            installed: false,
            default_hc: true,
            configured_hc: "hc".into(),
            resolved_hc: None,
            install_command: DISPLAY_INSTALL_COMMAND.into(),
            error: None,
            installing: false,
            checking: false,
            logs: Vec::new(),
        }
    }
}

impl HerdrSetupView {
    fn from_check(home: String, check: &Value) -> Self {
        let install_phase = check["install"]["phase"].as_str().unwrap_or("idle");
        let logs = check["install"]["logs"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let install_error = check["install"]["error"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let probe_error = check["error"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        Self {
            home,
            api_version: canban_app_server::API_VERSION,
            installed: check["installed"] == true,
            default_hc: check["default_hc"].as_bool().unwrap_or(true),
            configured_hc: check["configured_hc"].as_str().unwrap_or("hc").to_owned(),
            resolved_hc: check["resolved_hc"].as_str().map(str::to_owned),
            install_command: check["install_command"]
                .as_str()
                .unwrap_or(DISPLAY_INSTALL_COMMAND)
                .to_owned(),
            error: install_error.or(probe_error),
            installing: install_phase == "installing",
            checking: false,
            logs,
        }
    }
}

#[derive(Clone)]
enum Stage {
    Checking,
    BackendOlder {
        home: String,
        detail: String,
        old: canban_app_server::BackendOlder,
    },
    AppServerError {
        home: String,
        detail: String,
    },
    HerdrSetup {
        app: Arc<canban_app_server::AppClient>,
        state: HerdrSetupView,
    },
    Workspace,
}

impl PartialEq for Stage {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Workspace, Self::Workspace) => true,
            (Self::Checking, Self::Checking) => true,
            (
                Self::BackendOlder {
                    home: h1,
                    detail: d1,
                    old: o1,
                },
                Self::BackendOlder {
                    home: h2,
                    detail: d2,
                    old: o2,
                },
            ) => h1 == h2 && d1 == d2 && o1.home == o2.home && o1.ping == o2.ping,
            (
                Self::AppServerError {
                    home: h1,
                    detail: d1,
                },
                Self::AppServerError {
                    home: h2,
                    detail: d2,
                },
            ) => h1 == h2 && d1 == d2,
            (Self::HerdrSetup { app: a1, state: s1 }, Self::HerdrSetup { app: a2, state: s2 }) => {
                Arc::ptr_eq(a1, a2) && s1 == s2
            }
            _ => false,
        }
    }
}

enum BootOutcome {
    Workspace {
        projects: Option<usize>,
        cards: Option<usize>,
        elapsed_ms: u64,
    },
    HerdrSetup(Arc<canban_app_server::AppClient>, HerdrSetupView),
    BackendOlder(canban_app_server::BackendOlder, String),
    AppServerError(String),
}

pub(crate) struct InitialBootstrap {
    pub snapshot: canban_types::Snapshot,
    pub config: canban_types::Config,
    pub saved_project: Option<String>,
}

static INITIAL_BOOTSTRAP: std::sync::Mutex<Option<InitialBootstrap>> = std::sync::Mutex::new(None);

pub(crate) fn take_initial_bootstrap() -> Option<InitialBootstrap> {
    INITIAL_BOOTSTRAP
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
}

fn finalize_workspace(app: Arc<canban_app_server::AppClient>) -> Result<Option<(usize, usize)>> {
    let snapshot = app.snapshot().ok();
    let counts = snapshot.as_ref().map(|s| (s.projects.len(), s.cards.len()));
    let config = app.config();
    let saved_project = app
        .call("utilities.state", json!({}))
        .ok()
        .and_then(|v| v["preferences"]["last_project"].as_str().map(str::to_owned));
    if let Some(snapshot) = snapshot
        && let Ok(mut slot) = INITIAL_BOOTSTRAP.lock()
    {
        *slot = Some(InitialBootstrap {
            snapshot,
            config,
            saved_project,
        });
    }
    let registry = canban_app_server::hosts::HostRegistry::new(app.clone());
    let backend: Arc<dyn crate::backend::UiBackend> =
        Arc::new(crate::backend::LocalBackend::with_hosts(app, registry));
    crate::drafts::persistence::start(backend.clone())?;
    let _ = crate::backend::set(backend);
    clear_fatal_exit();
    Ok(counts)
}

fn apply_boot_event(progress: &mut BootProgress, event: BootEvent) {
    match event {
        BootEvent::Connect(p, elapsed_ms) => progress.apply_connect_progress(&p, elapsed_ms),
        BootEvent::AppReady {
            api_version,
            server_id,
            elapsed_ms,
        } => progress.mark_app_ready(api_version, server_id.as_deref(), elapsed_ms),
        BootEvent::HerdrReady {
            version,
            resolved_hc,
            elapsed_ms,
        } => progress.mark_herdr_ready(version.as_deref(), resolved_hc.as_deref(), elapsed_ms),
        BootEvent::SnapshotLoading { elapsed_ms } => progress.mark_snapshot_loading(elapsed_ms),
        BootEvent::BootComplete {
            projects,
            cards,
            elapsed_ms,
        } => progress.mark_boot_complete(projects, cards, elapsed_ms),
    }
}

/// Primary desktop bootstrap root: verifies `AppServer` and `herdr-core` (`hc`),
/// presents unified recovery / `Install` + `Check` actions when needed, and
/// mounts `<Canban />` seamlessly in the same window once ready.
#[component]
pub fn Bootstrap() -> impl IntoElement {
    canban_ui_kit::tokens::install_theme();
    watch_quit();
    crate::frame::configure_window_chrome();

    let home_path = canban_app_server::default_home().unwrap_or_default();
    let home_display = if home_path.as_os_str().is_empty() {
        "Unavailable".into()
    } else {
        home_path.display().to_string()
    };

    let root_owner = Owner::current();
    let stage = signal(Stage::Checking);
    let boot = signal(BootProgress::initial(home_display.clone()));
    let elapsed_ms = signal(0_u64);
    let boot_started = stored(Instant::now());
    let boot_rx = stored(None::<mpsc::Receiver<BootEvent>>);
    let show_checking_panel = signal(false);
    let copy_status = signal(String::new());
    let poll_active = stored(false);
    let recheck_count = stored(0_u32);

    set_timeout(Duration::from_millis(140), move || {
        show_checking_panel.set(true);
    });

    // Live POST clock + real-time progress event drain while checking.
    set_interval(Duration::from_millis(80), move || {
        if !matches!(stage.get_untracked(), Stage::Checking) {
            return;
        }
        let drained = boot_rx.with(|slot| {
            let mut events = Vec::new();
            if let Some(rx) = slot.as_ref() {
                while let Ok(ev) = rx.try_recv() {
                    events.push(ev);
                }
            }
            events
        });
        if !drained.is_empty() {
            boot.update(|b| {
                for ev in drained {
                    apply_boot_event(b, ev);
                }
            });
        }
        if !boot.get_untracked().boot_complete {
            elapsed_ms.set(boot_started.get().elapsed().as_millis() as u64);
        }
    });

    let run_boot: Rc<dyn Fn()> = Rc::new({
        let home_path = home_path.clone();
        let home_display = home_display.clone();
        move || {
            clear_fatal_exit();
            let started = Instant::now();
            boot_started.replace(started);
            elapsed_ms.set(0);
            boot.set(BootProgress::initial(home_display.clone()));
            let (tx, rx) = mpsc::channel::<BootEvent>();
            boot_rx.replace(Some(rx));
            stage.set(Stage::Checking);
            let hp = home_path.clone();
            let hd = home_display.clone();
            let hd_after = home_display.clone();
            root_owner.run(move || {
                spawn(async move {
                    let outcome = blocking(move || -> BootOutcome {
                        let cli = match std::env::current_exe() {
                            Ok(exe) => exe.with_file_name("canban"),
                            Err(e) => return BootOutcome::AppServerError(format!("{e:#}")),
                        };
                        let tx_connect = tx.clone();
                        let app =
                            match canban_app_server::AppClient::connect_or_start_ui_with_progress(
                                hp.clone(),
                                &cli,
                                std::process::id(),
                                move |progress| {
                                    let _ = tx_connect.send(BootEvent::Connect(
                                        progress,
                                        started.elapsed().as_millis() as u64,
                                    ));
                                },
                            ) {
                                Ok(app) => app,
                                Err(err) => {
                                    let detail = format!("{err:#}");
                                    if let Some(old) =
                                        err.downcast_ref::<canban_app_server::BackendOlder>()
                                        && !old.explicit_endpoint
                                        && instance::bundled().ok().flatten().is_none()
                                    {
                                        return BootOutcome::BackendOlder(old.clone(), detail);
                                    }
                                    return BootOutcome::AppServerError(detail);
                                }
                            };
                        let _ = tx.send(BootEvent::AppReady {
                            api_version: canban_app_server::API_VERSION,
                            server_id: app.server_id().map(str::to_owned),
                            elapsed_ms: started.elapsed().as_millis() as u64,
                        });
                        match app.call("herdr.check", json!({})) {
                            Ok(check) if check["ready"] == true => {
                                let _ = tx.send(BootEvent::HerdrReady {
                                    version: check["version"].as_str().map(str::to_owned),
                                    resolved_hc: check["resolved_hc"].as_str().map(str::to_owned),
                                    elapsed_ms: started.elapsed().as_millis() as u64,
                                });
                                let _ = tx.send(BootEvent::SnapshotLoading {
                                    elapsed_ms: started.elapsed().as_millis() as u64,
                                });
                                match finalize_workspace(app) {
                                    Ok(counts) => {
                                        let done_ms = started.elapsed().as_millis() as u64;
                                        let projects = counts.map(|(p, _)| p);
                                        let cards = counts.map(|(_, c)| c);
                                        let _ = tx.send(BootEvent::BootComplete {
                                            projects,
                                            cards,
                                            elapsed_ms: done_ms,
                                        });
                                        BootOutcome::Workspace {
                                            projects,
                                            cards,
                                            elapsed_ms: done_ms,
                                        }
                                    }
                                    Err(err) => BootOutcome::AppServerError(format!("{err:#}")),
                                }
                            }
                            Ok(check) => {
                                write_upgrade_ready(&hp);
                                let view = HerdrSetupView::from_check(hd, &check);
                                BootOutcome::HerdrSetup(app, view)
                            }
                            Err(err) => {
                                write_upgrade_ready(&hp);
                                BootOutcome::HerdrSetup(
                                    app,
                                    HerdrSetupView {
                                        home: hd,
                                        error: Some(format!(
                                            "Could not verify Herdr Core status: {err:#}"
                                        )),
                                        ..Default::default()
                                    },
                                )
                            }
                        }
                    })
                    .await;

                    // Drain any final progress events before transitioning.
                    let remaining = boot_rx.with(|slot| {
                        let mut events = Vec::new();
                        if let Some(rx) = slot.as_ref() {
                            while let Ok(ev) = rx.try_recv() {
                                events.push(ev);
                            }
                        }
                        events
                    });
                    boot_rx.replace(None);
                    if !remaining.is_empty() {
                        boot.update(|b| {
                            for ev in remaining {
                                apply_boot_event(b, ev);
                            }
                        });
                    }

                    match outcome {
                        Ok(BootOutcome::Workspace {
                            projects,
                            cards,
                            elapsed_ms: done_ms,
                        }) => {
                            elapsed_ms.set(done_ms);
                            boot.update(|b| {
                                if !b.boot_complete {
                                    b.mark_boot_complete(projects, cards, done_ms);
                                }
                            });
                            stage.set(Stage::Workspace);
                        }
                        Ok(BootOutcome::HerdrSetup(app, view)) => {
                            let resume_poll = view.installing;
                            ready();
                            stage.set(Stage::HerdrSetup { app, state: view });
                            if resume_poll {
                                poll_active.replace(true);
                            }
                        }
                        Ok(BootOutcome::BackendOlder(old, detail)) => {
                            record_error(&detail);
                            stage.set(Stage::BackendOlder {
                                home: hd_after.clone(),
                                detail,
                                old,
                            });
                        }
                        Ok(BootOutcome::AppServerError(detail)) => {
                            record_error(&detail);
                            stage.set(Stage::AppServerError {
                                home: hd_after.clone(),
                                detail,
                            });
                        }
                        Err(join_err) => {
                            let detail = format!("Startup task failed: {join_err}");
                            record_error(&detail);
                            stage.set(Stage::AppServerError {
                                home: hd_after,
                                detail,
                            });
                        }
                    }
                });
            });
        }
    });

    // Polling loop when `herdr.install` is running in the background on `AppServer`.
    let poll_in_flight = stored(false);
    set_interval(Duration::from_millis(250), move || {
        if !poll_active.get() || poll_in_flight.get() {
            return;
        }
        let current = stage.get_untracked();
        let Stage::HerdrSetup { app, state } = current else {
            poll_active.replace(false);
            return;
        };
        poll_in_flight.replace(true);
        let home_str = state.home.clone();
        root_owner.run(move || {
            spawn(async move {
                let app_for_poll = app.clone();
                let polled =
                    blocking(move || app_for_poll.call("herdr.check", json!({"probe": false})))
                        .await;
                match polled {
                    Ok(Ok(val)) => {
                        let phase = val["install"]["phase"].as_str().unwrap_or("idle");
                        let logs = val["install"]["logs"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_owned)
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let install_err = val["install"]["error"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned);
                        if phase == "installing" {
                            stage.update(|s| {
                                if let Stage::HerdrSetup { state, .. } = s {
                                    state.installing = true;
                                    state.logs = logs;
                                }
                            });
                            poll_in_flight.replace(false);
                        } else {
                            poll_active.replace(false);
                            let app_verify = app.clone();
                            let full = blocking(move || -> Result<(Value, bool)> {
                                let check = app_verify.call("herdr.check", json!({}))?;
                                if check["ready"] == true {
                                    finalize_workspace(app_verify)?;
                                    return Ok((check, true));
                                }
                                Ok((check, false))
                            })
                            .await;
                            poll_in_flight.replace(false);
                            match full {
                                Ok(Ok((_, true))) => {
                                    stage.set(Stage::Workspace);
                                }
                                Ok(Ok((check, false))) => {
                                    let mut next = HerdrSetupView::from_check(home_str, &check);
                                    next.installing = false;
                                    if !logs.is_empty() && next.logs.is_empty() {
                                        next.logs = logs;
                                    }
                                    if let Some(err) = install_err {
                                        next.error = Some(err);
                                    }
                                    stage.set(Stage::HerdrSetup { app, state: next });
                                }
                                Ok(Err(err)) => {
                                    stage.update(|s| {
                                        if let Stage::HerdrSetup { state, .. } = s {
                                            state.installing = false;
                                            state.error = Some(format!("{err:#}"));
                                        }
                                    });
                                }
                                Err(err) => {
                                    stage.update(|s| {
                                        if let Stage::HerdrSetup { state, .. } = s {
                                            state.installing = false;
                                            state.error = Some(err.to_string());
                                        }
                                    });
                                }
                            }
                        }
                    }
                    _ => {
                        poll_in_flight.replace(false);
                    }
                }
            });
        });
    });

    // Trigger initial boot once mounted.
    let boot_once = run_boot.clone();
    set_timeout(Duration::from_millis(1), move || {
        boot_once();
    });

    let retry_boot = run_boot.clone();
    view()
        .flex_1()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(dynamic(move || {
            let current = stage.get();
            let show_checking = matches!(current, Stage::Checking) && show_checking_panel.get();
            untracked(|| match current {
                Stage::Workspace => crate::workspace_root().into_element(),
                Stage::Checking => {
                    if !show_checking {
                        return startup_frame(view()).into_element();
                    }
                    checking_screen(boot, elapsed_ms).into_element()
                }
        Stage::BackendOlder { home, detail, old } => {
            let auto_start = std::env::var_os("CANBAN_UPGRADE_TOKEN").is_none();
            backend_update_screen_auto(home, detail, auto_start, move || {
                let old = old.clone();
                async move {
                    let cli = std::env::current_exe()?.with_file_name("canban");
                    let app_id = app_id()?;
                    let result = blocking(move || {
                        canban_updater::worker::local::prepare(
                            &old.home,
                            &cli,
                            &old.ping,
                            std::process::id(),
                            app_id,
                        )
                    })
                    .await;
                    result.context("Backend updater could not be started")?
                }
            })
            .into_element()
        }
        Stage::AppServerError { home, detail } => {
            let retry = retry_boot.clone();
            error_screen_with_retry(
                home,
                detail,
                Some(Rc::new(move || {
                    retry();
                })),
            )
            .into_element()
        }
        Stage::HerdrSetup { app, state } => {
            let app_install = app.clone();
            let app_check = app.clone();
            let home_for_check = state.home.clone();
            let cmd_for_copy = state.install_command.clone();
            herdr_setup_screen(
                state,
                copy_status,
                move || {
                    if poll_active.get() {
                        return;
                    }
                    copy_status.set(String::new());
                    stage.update(|s| {
                        if let Stage::HerdrSetup { state, .. } = s {
                            state.installing = true;
                            state.error = None;
                            if state.logs.is_empty() {
                                state.logs = vec!["Starting Herdr Core installer…".into()];
                            }
                        }
                    });
                    let app_call = app_install.clone();
                    root_owner.run(move || {
                        spawn(async move {
                            let res =
                                blocking(move || app_call.call("herdr.install", json!({}))).await;
                            match res {
                                Ok(Ok(_)) => {
                                    poll_active.replace(true);
                                }
                                Ok(Err(err)) => {
                                    stage.update(|s| {
                                        if let Stage::HerdrSetup { state, .. } = s {
                                            state.installing = false;
                                            state.error = Some(format!("{err:#}"));
                                        }
                                    });
                                }
                                Err(err) => {
                                    stage.update(|s| {
                                        if let Stage::HerdrSetup { state, .. } = s {
                                            state.installing = false;
                                            state.error = Some(err.to_string());
                                        }
                                    });
                                }
                            }
                        });
                    });
                },
                move || {
                    copy_status.set(String::new());
                    stage.update(|s| {
                        if let Stage::HerdrSetup { state, .. } = s {
                            state.checking = true;
                        }
                    });
                    let app_call = app_check.clone();
                    let home_label = home_for_check.clone();
                    root_owner.run(move || {
                        spawn(async move {
                            let res = blocking(move || -> Result<(Value, bool)> {
                                let check = app_call.call("herdr.check", json!({}))?;
                                if check["ready"] == true {
                                    finalize_workspace(app_call)?;
                                    return Ok((check, true));
                                }
                                Ok((check, false))
                            })
                            .await;
                            match res {
                                Ok(Ok((_, true))) => {
                                    stage.set(Stage::Workspace);
                                }
                                Ok(Ok((check, false))) => {
                                    let count = recheck_count.get() + 1;
                                    recheck_count.replace(count);
                                    let mut next = HerdrSetupView::from_check(home_label, &check);
                                    next.checking = false;
                                    let base_err = next.error.take().unwrap_or_else(|| {
                                        "Herdr Core (`hc`) is still not detected. Install it first or verify PATH, then click Check.".into()
                                    });
                                    next.error = Some(if count == 1 {
                                        format!("Checked just now — {base_err}")
                                    } else {
                                        format!("Checked again (#{count}) — {base_err}")
                                    });
                                    stage.update(|s| {
                                        if let Stage::HerdrSetup { state, .. } = s {
                                            let prev_logs = std::mem::take(&mut state.logs);
                                            *state = next;
                                            if state.logs.is_empty() {
                                                state.logs = prev_logs;
                                            }
                                        }
                                    });
                                }
                                Ok(Err(err)) => {
                                    stage.update(|s| {
                                        if let Stage::HerdrSetup { state, .. } = s {
                                            state.checking = false;
                                            state.error = Some(format!("{err:#}"));
                                        }
                                    });
                                }
                                Err(err) => {
                                    stage.update(|s| {
                                        if let Stage::HerdrSetup { state, .. } = s {
                                            state.checking = false;
                                            state.error = Some(err.to_string());
                                        }
                                    });
                                }
                            }
                        });
                    });
                },
                move || {
                    set_clipboard(&cmd_for_copy);
                    copy_status.set("Copied".into());
                },
            )
            .into_element()
        }
            })
        }))
}

fn startup_frame(content: impl IntoElement) -> View {
    view()
        .flex_1()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(view().h(px(42.)).w_full().flex_shrink_0().window_drag(true))
        .child(
            view()
                .flex_1()
                .min_h(px(0.))
                .p_5()
                .items_center()
                .justify_center()
                .child(content),
        )
}

fn step_icon_badge(state: StepState) -> View {
    match state {
        StepState::Active => loading_icon_badge(Tone::Info),
        StepState::Ready => tone_icon_badge(Icon::Check, Tone::Success, false),
        StepState::Attention => tone_icon_badge(Icon::TriangleAlert, Tone::Attention, false),
        StepState::Error => tone_icon_badge(Icon::Close, Tone::Danger, false),
        StepState::Pending => tone_icon_badge(Icon::Terminal, Tone::Neutral, false),
    }
}

fn step_row(
    title: &'static str,
    detail: impl IntoReactive<SharedString>,
    state: StepState,
) -> View {
    let status_badge = match state {
        StepState::Ready => status("Ready", Tone::Success).into_element(),
        StepState::Active => live_status("Checking", Tone::Info).into_element(),
        StepState::Attention => status("Action needed", Tone::Attention).into_element(),
        StepState::Error => status("Error", Tone::Danger).into_element(),
        StepState::Pending => status("Pending", Tone::Neutral).into_element(),
    };
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .px_3()
        .py_2p5()
        .child(step_icon_badge(state))
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .gap_0p5()
                .child(label(title).font_weight(FontWeight::SEMIBOLD))
                .child(muted(detail).truncate()),
        )
        .child(status_badge)
}

fn startup_checklist(
    app_state: StepState,
    app_detail: impl IntoReactive<SharedString>,
    herdr_state: StepState,
    herdr_detail: impl IntoReactive<SharedString>,
) -> View {
    view()
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .rounded(px(RADIUS))
        .child(step_row("AppServer", app_detail, app_state))
        .child(view().h(px(1.)).w_full().bg(BORDER))
        .child(step_row("Herdr Core", herdr_detail, herdr_state))
}

fn format_post_clock(ms: u64) -> String {
    format!("POST · {:04.1}s", ms as f64 / 1000.0)
}

fn format_log_ts(ms: u64) -> String {
    format!("[{:05.2}s]", ms as f64 / 1000.0)
}

fn format_step_secs(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

fn log_tag_color(tag: &str) -> ThemeColor {
    match tag {
        "MIGRATE" | "HANDOFF" => ATTENTION,
        "RPC" | "HERDR" | "DAEMON" | "REMOTE" => ACCENT,
        "SNAPSHOT" => SUCCESS,
        _ => MUTED,
    }
}

fn boot_substep_row(step: &BootSubStep, is_last: bool) -> View {
    let branch = if is_last { "└─" } else { "├─" };
    let status_cell = match step.state {
        SubStepState::Ok => text("[ OK ]")
            .font_family(MONO_FONT)
            .text_size(px(11.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(SUCCESS)
            .w(px(48.))
            .into_element(),
        SubStepState::Active => view()
            .flex_row()
            .items_center()
            .gap_1()
            .w(px(48.))
            .child(loading_spinner(11., ACCENT))
            .child(
                text("RUN")
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ACCENT),
            )
            .into_element(),
        SubStepState::Pending => text("[ .. ]")
            .font_family(MONO_FONT)
            .text_size(px(11.5))
            .text_color(MUTED)
            .w(px(48.))
            .into_element(),
    };
    let label_color = match step.state {
        SubStepState::Active => TEXT,
        SubStepState::Ok | SubStepState::Pending => MUTED,
    };
    view()
        .flex_row()
        .items_center()
        .gap_1p5()
        .min_w(px(0.))
        .child(
            text(branch)
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(MUTED),
        )
        .child(status_cell)
        .child(
            text(step.label.clone())
                .text_size(px(CAPTION_SIZE))
                .text_color(label_color)
                .flex_1()
                .min_w(px(0.))
                .truncate(),
        )
}

fn post_step_row(
    title: &'static str,
    boot: Signal<BootProgress>,
    elapsed_ms: Signal<u64>,
    is_app: bool,
) -> View {
    let get_state = move || {
        let b = boot.get();
        if is_app { b.app_state } else { b.herdr_state }
    };

    // Mount each icon variant via `when` so the Active spinner stays mounted and rotates
    // continuously without restarting while substeps or elapsed time update.
    let icon_slot = view()
        .flex_shrink_0()
        .mt(px(1.))
        .child(when(
            move || get_state() == StepState::Active,
            || loading_icon_badge(Tone::Info),
        ))
        .child(when(
            move || get_state() == StepState::Ready,
            || tone_icon_badge(Icon::Check, Tone::Success, false),
        ))
        .child(when(
            move || get_state() == StepState::Pending,
            || tone_icon_badge(Icon::Terminal, Tone::Neutral, false),
        ))
        .child(when(
            move || get_state() == StepState::Attention,
            || tone_icon_badge(Icon::TriangleAlert, Tone::Attention, false),
        ))
        .child(when(
            move || get_state() == StepState::Error,
            || tone_icon_badge(Icon::Close, Tone::Danger, false),
        ));

    let badge_slot = view()
        .flex_shrink_0()
        .child(when(
            move || get_state() == StepState::Active,
            move || {
                live_status(
                    move || -> SharedString {
                        let now_ms = elapsed_ms.get();
                        let b = boot.get();
                        let started = if is_app {
                            b.app_started_ms
                        } else {
                            b.herdr_started_ms.unwrap_or(now_ms)
                        };
                        let dur = now_ms.saturating_sub(started);
                        format!("Checking · {}", format_step_secs(dur)).into()
                    },
                    Tone::Info,
                )
            },
        ))
        .child(when(
            move || get_state() == StepState::Ready,
            move || {
                status(
                    move || -> SharedString {
                        let b = boot.get();
                        let (suffix, dur) = if is_app {
                            (&b.app_badge_suffix, b.app_elapsed_ms)
                        } else {
                            (&b.herdr_badge_suffix, b.herdr_elapsed_ms)
                        };
                        let mut parts = vec!["Ready".to_owned()];
                        if let Some(s) = suffix {
                            parts.push(s.clone());
                        }
                        if let Some(ms) = dur {
                            parts.push(format_step_secs(ms));
                        }
                        parts.join(" · ").into()
                    },
                    Tone::Success,
                )
            },
        ))
        .child(when(
            move || get_state() == StepState::Pending,
            || status("Pending", Tone::Neutral),
        ))
        .child(when(
            move || get_state() == StepState::Attention,
            || status("Action needed", Tone::Attention),
        ))
        .child(when(
            move || get_state() == StepState::Error,
            || status("Error", Tone::Danger),
        ));

    view()
        .flex_row()
        .items_start()
        .gap_3()
        .px_3()
        .py_2p5()
        .child(icon_slot)
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .gap_1()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(label(title).font_weight(FontWeight::SEMIBOLD))
                        .child(badge_slot),
                )
                .child(dynamic(move || {
                    let b = boot.get();
                    let (substeps, detail) = if is_app {
                        (b.app_substeps, b.app_detail)
                    } else {
                        (b.herdr_substeps, b.herdr_detail)
                    };
                    if substeps.is_empty() {
                        muted(detail).truncate().into_element()
                    } else {
                        let total = substeps.len();
                        let mut tree = view().gap_1().pt_0p5().min_w(px(0.));
                        for (idx, step) in substeps.iter().enumerate() {
                            tree = tree.child(boot_substep_row(step, idx + 1 == total));
                        }
                        tree.into_element()
                    }
                })),
        )
}

/// Production POST self-test startup screen (Option A: modern card + self-test tree + boot log),
/// also exercised in the native Gallery.
pub fn checking_screen(boot: Signal<BootProgress>, elapsed_ms: Signal<u64>) -> impl IntoElement {
    let header_status = view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_2p5()
        .py_1()
        .rounded(px(6.))
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .child(
            text(move || -> SharedString { format_post_clock(elapsed_ms.get()).into() })
                .font_family(MONO_FONT)
                .text_size(px(CAPTION_SIZE))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || {
                    if boot.get().boot_complete {
                        SUCCESS.resolve()
                    } else {
                        MUTED.resolve()
                    }
                }),
        )
        .child(when(
            move || !boot.get().boot_complete,
            || loading_spinner(13., ACCENT),
        ))
        .child(when(
            move || boot.get().boot_complete,
            || icon_tinted(Icon::Check, SUCCESS).w(px(13.)).h(px(13.)),
        ));

    let checklist = view()
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .rounded(px(RADIUS))
        .child(post_step_row("AppServer", boot, elapsed_ms, true))
        .child(view().h(px(1.)).w_full().bg(BORDER))
        .child(post_step_row("Herdr Core", boot, elapsed_ms, false));

    let boot_log_box = view()
        .gap_1p5()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    text("BOOT SEQUENCE")
                        .font_family(MONO_FONT)
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(MUTED),
                )
                .child(
                    text(move || -> SharedString {
                        format!("Home: {}", compact_user_path(&boot.get().home)).into()
                    })
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .text_color(MUTED)
                    .truncate(),
                ),
        )
        .child(
            view()
                .bg(BACKGROUND)
                .border_1()
                .border_color(BORDER)
                .rounded(px(RADIUS))
                .px_3()
                .py_2()
                .gap_1()
                .min_h(px(112.))
                .child(dynamic(move || {
                    let b = boot.get();
                    let complete = b.boot_complete;
                    let visible: Vec<_> = b
                        .logs
                        .iter()
                        .rev()
                        .take(6)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .cloned()
                        .collect();
                    let count = visible.len();
                    let mut list = view().gap_1().min_w(px(0.));
                    for (idx, entry) in visible.into_iter().enumerate() {
                        let is_last = idx + 1 == count;
                        let tag_color = log_tag_color(&entry.tag);
                        let msg_color = if is_last && !complete { TEXT } else { MUTED };
                        let message_slot = if is_last && !complete {
                            let msg = entry.message;
                            text(move || -> SharedString {
                                if canban_ui_kit::motion::reduced()
                                    || (elapsed_ms.get() / 450).is_multiple_of(2)
                                {
                                    format!("{msg} █").into()
                                } else {
                                    format!("{msg}  ").into()
                                }
                            })
                            .font_family(MONO_FONT)
                            .text_size(px(11.5))
                            .text_color(msg_color)
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .into_element()
                        } else {
                            text(entry.message)
                                .font_family(MONO_FONT)
                                .text_size(px(11.5))
                                .text_color(msg_color)
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .into_element()
                        };
                        let row = view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .min_w(px(0.))
                            .child(
                                text(format_log_ts(entry.elapsed_ms))
                                    .font_family(MONO_FONT)
                                    .text_size(px(11.5))
                                    .text_color(MUTED)
                                    .w(px(62.))
                                    .flex_shrink_0(),
                            )
                            .child(
                                text(entry.tag)
                                    .font_family(MONO_FONT)
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(tag_color)
                                    .w(px(72.))
                                    .flex_shrink_0(),
                            )
                            .child(message_slot);
                        list = list.child(row);
                    }
                    list.into_element()
                })),
        );

    let footer_progress = dynamic(move || {
        let b = boot.get();
        let tone = if b.boot_complete {
            Tone::Success
        } else {
            Tone::Info
        };
        let pct_color = if b.boot_complete { SUCCESS } else { TEXT };
        view()
            .flex_row()
            .items_center()
            .gap_3()
            .child(progress_bar(f32::from(b.percent) / 100.0, tone))
            .child(
                text(format!("{:>3}%", b.percent))
                    .font_family(MONO_FONT)
                    .text_size(px(CAPTION_SIZE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(pct_color)
                    .flex_shrink_0(),
            )
            .child(
                muted(b.status_line)
                    .flex_shrink_0()
                    .max_w(px(240.))
                    .truncate(),
            )
            .into_element()
    });

    startup_frame(
        panel()
            .w_full()
            .max_w(px(720.))
            .gap_4()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        view()
                            .gap_0p5()
                            .min_w(px(0.))
                            .flex_1()
                            .child(heading("Canban"))
                            .child(muted("System Self-Test & Workspace Bootstrap")),
                    )
                    .child(header_status),
            )
            .child(checklist)
            .child(boot_log_box)
            .child(footer_progress),
    )
}

/// Production Herdr Core startup setup surface, also exercised in the native Gallery.
pub fn herdr_setup_screen(
    state: HerdrSetupView,
    copy_status: Signal<String>,
    on_install: impl Fn() + 'static,
    on_check: impl Fn() + 'static,
    on_copy: impl Fn() + 'static,
) -> impl IntoElement {
    let busy = state.installing || state.checking;
    let has_failed_logs = !state.installing && !state.logs.is_empty() && state.error.is_some();
    let title = if state.installing {
        "Installing Herdr Core…"
    } else if state.installed {
        "Herdr Core Unavailable"
    } else {
        "Install Herdr Core"
    };
    let subtitle = if state.installing {
        "Downloading and installing the Herdr Core runtime (`hc`). Your workspace will open automatically once verified."
    } else if state.installed {
        "The Herdr Core binary was found on disk, but Canban could not connect to its daemon. Check its status or reinstall."
    } else {
        "Canban uses Herdr Core (`hc`) to supervise persistent Agent terminals and local shells across restarts."
    };

    let herdr_step_state = if state.installing || state.checking {
        StepState::Active
    } else if state.installed || has_failed_logs {
        StepState::Error
    } else {
        StepState::Attention
    };
    let herdr_step_detail = if state.installing {
        "Installing to ~/.local/bin/hc…".to_owned()
    } else if state.checking {
        "Checking hc runtime…".to_owned()
    } else if let Some(resolved) = &state.resolved_hc {
        format!("Unreachable ({resolved})")
    } else if state.default_hc {
        "Not installed (`hc` not found)".to_owned()
    } else {
        format!("Missing configured path (`{}`)", state.configured_hc)
    };

    let notice_tone = if state.installed || has_failed_logs {
        Tone::Danger
    } else {
        Tone::Attention
    };
    let notice_message = state.error.clone().unwrap_or_else(|| {
        "Herdr Core (`hc`) was not found on this machine. Click Install Herdr Core to install it automatically, or install it in your terminal and click Check.".into()
    });

    let install_label = if state.installing {
        "Installing…"
    } else if has_failed_logs {
        "Retry Install"
    } else if state.installed {
        "Reinstall Herdr Core"
    } else {
        "Install Herdr Core"
    };
    let check_label = if state.checking {
        "Checking…"
    } else {
        "Check"
    };

    let mut card = panel()
        .w_full()
        .max_w(px(720.))
        .gap_4()
        .child(heading(title))
        .child(muted(subtitle))
        .child(startup_checklist(
            StepState::Ready,
            format!("Running · API v{}", state.api_version),
            herdr_step_state,
            herdr_step_detail,
        ));

    if !state.installing {
        card = card.child(notice(notice_message, notice_tone));
    }

    if !state.default_hc && !state.installed {
        card = card.child(muted(format!(
            "Note: `config/agents.json` configures a custom `hc` path (`{}`). Installing via the official script places `hc` in `~/.local/bin/hc`; restore `\"hc\"` or place the binary at the configured path before clicking Check.",
            state.configured_hc
        )));
    }

    if !state.logs.is_empty() {
        let log_lines = state
            .logs
            .iter()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        card = card.child(
            view().gap_1p5().child(muted("Installer output")).child(
                scroll()
                    .max_h(px(140.))
                    .bg(INSET)
                    .border_1()
                    .border_color(BORDER)
                    .rounded(px(RADIUS))
                    .px_3()
                    .py_2()
                    .child(
                        read_text(log_lines)
                            .font_family(MONO_FONT)
                            .text_size(px(CAPTION_SIZE))
                            .text_color(MUTED),
                    ),
            ),
        );
    }

    let command_text = state.install_command.clone();
    card = card.child(
        view()
            .gap_1p5()
            .child(muted("Manual terminal install command"))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(INSET)
                    .border_1()
                    .border_color(BORDER)
                    .rounded(px(RADIUS))
                    .child(
                        read_text(command_text)
                            .flex_1()
                            .min_w(px(0.))
                            .font_family(MONO_FONT)
                            .text_size(px(CAPTION_SIZE))
                            .text_color(TEXT),
                    )
                    .child(compact_action(
                        move || -> SharedString {
                            let st = copy_status.get();
                            if st.is_empty() {
                                "Copy".into()
                            } else {
                                st.into()
                            }
                        },
                        Variant::Secondary,
                        move |_| on_copy(),
                    )),
            ),
    );

    card = card.child(muted(format!("Home: {}", state.home)));

    let mut actions = view().flex_row().flex_wrap().items_center().gap_2();
    if state.default_hc {
        actions = actions.child(
            action_with_icon(Icon::Update, install_label, Variant::Primary, move |_| {
                on_install()
            })
            .disabled(busy),
        );
        actions = actions.child(
            action_with_icon(Icon::Refresh, check_label, Variant::Secondary, move |_| {
                on_check()
            })
            .disabled(busy),
        );
    } else {
        actions = actions.child(
            action_with_icon(Icon::Refresh, check_label, Variant::Primary, move |_| {
                on_check()
            })
            .disabled(busy),
        );
        actions = actions.child(
            action_with_icon(
                Icon::Update,
                "Install Default Herdr Core",
                Variant::Secondary,
                move |_| on_install(),
            )
            .disabled(busy),
        );
    }
    actions = actions
        .child(view().flex_1())
        .child(action("Quit", Variant::Quiet, |_| {
            nsh_core::element::desktop::quit()
        }));

    startup_frame(card.child(actions))
}

#[component]
fn StartupError() -> impl IntoElement {
    canban_ui_kit::tokens::install_theme();
    watch_quit();
    let home = canban_app_server::default_home()
        .map(|v| v.display().to_string())
        .unwrap_or_else(|_| "Unavailable".into());
    if let Some(old) = OLDER.get() {
        let old = old.clone();
        let auto_start = std::env::var_os("CANBAN_UPGRADE_TOKEN").is_none();
        return backend_update_screen_auto(
            home,
            ERROR.get().cloned().unwrap_or_default(),
            auto_start,
            move || {
                let old = old.clone();
                async move {
                    let cli = std::env::current_exe()?.with_file_name("canban");
                    let app_id = app_id()?;
                    let result = blocking(move || {
                        canban_updater::worker::local::prepare(
                            &old.home,
                            &cli,
                            &old.ping,
                            std::process::id(),
                            app_id,
                        )
                    })
                    .await;
                    result.context("Backend updater could not be started")?
                }
            },
        )
        .into_element();
    }
    error_screen(home, ERROR.get().cloned().unwrap_or_default()).into_element()
}

pub fn backend_update_screen<F, Fut>(home: String, detail: String, update: F) -> impl IntoElement
where
    F: Fn() -> Fut + 'static,
    Fut: std::future::Future<Output = Result<()>> + 'static,
{
    backend_update_screen_auto(home, detail, false, update)
}

pub fn backend_update_screen_auto<F, Fut>(
    home: String,
    detail: String,
    auto_start: bool,
    update: F,
) -> impl IntoElement
where
    F: Fn() -> Fut + 'static,
    Fut: std::future::Future<Output = Result<()>> + 'static,
{
    let screen_owner = Owner::current();
    let pending = signal(auto_start);
    let error = signal(String::new());
    let update = Rc::new(update);
    if auto_start {
        let auto_update = update.clone();
        set_timeout(Duration::from_millis(10), move || {
            let future = auto_update();
            screen_owner.run(move || {
                spawn(async move {
                    if let Err(e) = future.await {
                        error.set(format!("{e:#}"));
                        pending.set(false);
                    }
                });
            });
        });
    }
    startup_frame(
        panel()
            .w_full()
            .max_w(px(720.))
            .gap_4()
            .child(heading(move || {
                if pending.get() {
                    "Updating local backend…"
                } else {
                    "Update backend"
                }
            }))
            .child(muted(move || {
                if pending.get() {
                    "Activating the backend included in this App. Your workspace will open automatically."
                } else {
                    "Use the backend included in this installed App. No update download is needed."
                }
            }))
            .child(dynamic(move || {
                let app_step = if pending.get() {
                    StepState::Active
                } else if !error.get().is_empty() {
                    StepState::Error
                } else {
                    StepState::Attention
                };
                let app_text = if pending.get() {
                    "Replacing older AppServer daemon…"
                } else {
                    "Older daemon requires local handoff"
                };
                startup_checklist(app_step, app_text, StepState::Pending, "Pending").into_element()
            }))
            .child(notice(detail, Tone::Attention))
            .child(when(
                move || !error.get().is_empty(),
                move || notice(move || error.get(), Tone::Danger),
            ))
            .child(muted(format!("Home: {home}")))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        action(
                            move || {
                                if pending.get() {
                                    "Updating…"
                                } else if !error.get().is_empty() {
                                    "Retry backend update"
                                } else {
                                    "Update backend"
                                }
                            },
                            Variant::Primary,
                            move |_| {
                                if pending.get_untracked() {
                                    return;
                                }
                                pending.set(true);
                                error.set(String::new());
                                let future = update();
                                screen_owner.run(move || {
                                    spawn(async move {
                                        if let Err(e) = future.await {
                                            error.set(format!("{e:#}"));
                                            pending.set(false);
                                        }
                                    });
                                });
                            },
                        )
                        .disabled(move || pending.get()),
                    )
                    .child(view().flex_1())
                    .child(action("Quit", Variant::Quiet, |_| {
                        nsh_core::element::desktop::quit()
                    })),
            ),
    )
}

/// Production startup failure surface, also rendered by the native Gallery.
pub fn error_screen(home: String, error: String) -> impl IntoElement {
    error_screen_with_retry(home, error, None)
}

pub fn error_screen_with_retry(
    home: String,
    error: String,
    retry: Option<Rc<dyn Fn()>>,
) -> impl IntoElement {
    let mut actions = view().flex_row().flex_wrap().items_center().gap_2();
    if let Some(retry_fn) = retry {
        actions = actions
            .child(action_with_icon(
                Icon::Refresh,
                "Retry",
                Variant::Primary,
                move |_| retry_fn(),
            ))
            .child(view().flex_1())
            .child(action("Quit", Variant::Quiet, |_| {
                nsh_core::element::desktop::quit()
            }));
    } else {
        actions = actions.child(action("Quit", Variant::Primary, |_| {
            nsh_core::element::desktop::quit()
        }));
    }
    startup_frame(
        panel()
            .w_full()
            .max_w(px(720.))
            .gap_4()
            .child(heading("Canban could not start"))
            .child(muted(
                "Resolve the connection or instance configuration, then retry or open Canban again.",
            ))
            .child(startup_checklist(
                StepState::Error,
                "Connection failed",
                StepState::Pending,
                "Pending",
            ))
            .child(notice(error, Tone::Attention))
            .child(muted(format!("Home: {home}")))
            .child(actions),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_core::{
        ApproxTextEngine, Core, EventKind, EventPayload, MountItem, ROOT_ID, element::Root,
        geometry::size,
    };
    use std::cell::Cell;

    #[test]
    fn untracked_workspace_stage_does_not_remount_on_inner_signal_or_theme_change() {
        thread_local! {
            static WORKSPACE_MOUNTS: Cell<usize> = const { Cell::new(0) };
        }
        WORKSPACE_MOUNTS.with(|m| m.set(0));

        #[component]
        fn ProbeBootstrap() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let stage = signal(0_u8);
            let show_checking_panel = signal(false);
            let inner = signal("initial".to_owned());

            view()
                .on_press(move |_: &()| {
                    if stage.get_untracked() == 0 {
                        stage.set(1);
                    } else {
                        inner.set("updated".into());
                        canban_ui_kit::tokens::set_theme("monokai");
                    }
                })
                .child(dynamic(move || {
                    let current = stage.get();
                    let show_checking = current == 0 && show_checking_panel.get();
                    untracked(|| match current {
                        0 => {
                            let _ = show_checking;
                            view().into_element()
                        }
                        _ => {
                            WORKSPACE_MOUNTS.with(|m| m.set(m.get() + 1));
                            let _ = inner.get();
                            view().bg(BACKGROUND).into_element()
                        }
                    })
                }))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(800.0), px(600.0)));
        core.mount_root(&Root::of::<ProbeBootstrap>());
        let batch = core.commit().expect("initial mount batch");
        assert_eq!(WORKSPACE_MOUNTS.with(|m| m.get()), 0);

        let host = batch
            .items
            .iter()
            .find_map(|item| match item {
                MountItem::Insert { parent, child, .. } if *parent == ROOT_ID => Some(*child),
                _ => None,
            })
            .expect("root host node");

        assert!(core.component_event(host, EventKind::Press, &EventPayload::None));
        let _ = core.commit();
        assert_eq!(WORKSPACE_MOUNTS.with(|m| m.get()), 1);

        assert!(core.component_event(host, EventKind::Press, &EventPayload::None));
        let _ = core.commit();
        assert_eq!(WORKSPACE_MOUNTS.with(|m| m.get()), 1);
    }

    #[test]
    fn boot_progress_advances_monotonically_through_post_sequence() {
        let mut p = BootProgress::initial("/tmp/.canban".into());
        assert_eq!(p.app_state, StepState::Active);
        assert_eq!(p.herdr_state, StepState::Pending);
        assert_eq!(p.percent, 10);
        assert_eq!(p.logs.len(), 1);
        assert_eq!(p.logs[0].tag, "HOME");

        p.apply_connect_progress(
            &canban_app_server::ConnectProgress {
                phase: "spawn".into(),
                tag: "DAEMON".into(),
                detail: "app.sock offline → spawning `canban serve`".into(),
            },
            80,
        );
        assert!(p.percent >= 24);

        p.apply_connect_progress(
            &canban_app_server::ConnectProgress {
                phase: "migrate".into(),
                tag: "MIGRATE".into(),
                detail: "Migrating SQLite schema (v18 → v20)…".into(),
            },
            420,
        );
        assert_eq!(p.app_substeps[0].state, SubStepState::Ok);
        assert_eq!(p.app_substeps[1].state, SubStepState::Active);
        assert_eq!(p.status_line, "Migrating database…");

        // Duplicate log messages should not be appended twice.
        let before_logs = p.logs.len();
        p.apply_connect_progress(
            &canban_app_server::ConnectProgress {
                phase: "migrate".into(),
                tag: "MIGRATE".into(),
                detail: "Migrating SQLite schema (v18 → v20)…".into(),
            },
            500,
        );
        assert_eq!(p.logs.len(), before_logs);

        p.mark_app_ready(8, Some("9f3a1c02abcd"), 1800);
        assert_eq!(p.app_state, StepState::Ready);
        assert_eq!(p.app_badge_suffix.as_deref(), Some("API v8"));
        assert!(p.app_substeps.is_empty());
        assert_eq!(p.herdr_state, StepState::Active);
        assert_eq!(p.herdr_substeps.len(), 2);

        p.mark_herdr_ready(Some("0.4.2"), Some("/Users/test/.local/bin/hc"), 2100);
        assert_eq!(p.herdr_state, StepState::Ready);
        assert_eq!(p.herdr_badge_suffix.as_deref(), Some("hc 0.4.2"));
        assert!(p.herdr_substeps.is_empty());

        p.mark_boot_complete(Some(14), Some(128), 2200);
        assert!(p.boot_complete);
        assert_eq!(p.percent, 100);
        assert!(p.app_detail.contains("14 projects"));
    }
}
