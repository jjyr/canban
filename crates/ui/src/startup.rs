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
    },
    time::Duration,
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
    let _ = nsh_platform_macos::run_app(
        nsh_platform_macos::AppSource::Rust(
            nsh_platform_macos::Root::of::<StartupError>().app_id(app_id.clone()),
        ),
        nsh_platform_macos::AppOptions {
            title: "Canban could not start".into(),
            app_id,
            size: (800, 520),
            ..Default::default()
        },
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Active,
    Ready,
    Attention,
    Error,
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
    Checking {
        home: String,
        step_app: StepState,
        app_detail: String,
        step_herdr: StepState,
        herdr_detail: String,
    },
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
            (
                Self::Checking {
                    home: h1,
                    step_app: a1,
                    app_detail: ad1,
                    step_herdr: r1,
                    herdr_detail: rd1,
                },
                Self::Checking {
                    home: h2,
                    step_app: a2,
                    app_detail: ad2,
                    step_herdr: r2,
                    herdr_detail: rd2,
                },
            ) => h1 == h2 && a1 == a2 && ad1 == ad2 && r1 == r2 && rd1 == rd2,
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
    Workspace,
    HerdrSetup(Arc<canban_app_server::AppClient>, HerdrSetupView),
    BackendOlder(canban_app_server::BackendOlder, String),
    AppServerError(String),
}

fn finalize_workspace(app: Arc<canban_app_server::AppClient>) -> Result<()> {
    let _ = app.snapshot();
    crate::drafts::persistence::start(app.clone())?;
    let _ = crate::APP.set(app);
    clear_fatal_exit();
    Ok(())
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
    let stage = signal(Stage::Checking {
        home: home_display.clone(),
        step_app: StepState::Active,
        app_detail: "Connecting to local service…".into(),
        step_herdr: StepState::Pending,
        herdr_detail: "Pending".into(),
    });
    let show_checking_panel = signal(false);
    let copy_status = signal(String::new());
    let poll_active = stored(false);
    let recheck_count = stored(0_u32);

    set_timeout(Duration::from_millis(140), move || {
        show_checking_panel.set(true);
    });

    let run_boot: Rc<dyn Fn()> = Rc::new({
        let home_path = home_path.clone();
        let home_display = home_display.clone();
        move || {
            clear_fatal_exit();
            stage.set(Stage::Checking {
                home: home_display.clone(),
                step_app: StepState::Active,
                app_detail: "Connecting to local service…".into(),
                step_herdr: StepState::Pending,
                herdr_detail: "Pending".into(),
            });
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
                        let app = match canban_app_server::AppClient::connect_or_start_ui(
                            hp.clone(),
                            &cli,
                            std::process::id(),
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
                        match app.call("herdr.check", json!({})) {
                            Ok(check) if check["ready"] == true => match finalize_workspace(app) {
                                Ok(()) => BootOutcome::Workspace,
                                Err(err) => BootOutcome::AppServerError(format!("{err:#}")),
                            },
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

                    match outcome {
                        Ok(BootOutcome::Workspace) => stage.set(Stage::Workspace),
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
            let show_checking =
                matches!(current, Stage::Checking { .. }) && show_checking_panel.get();
            untracked(|| match current {
                Stage::Workspace => crate::workspace_root().into_element(),
                Stage::Checking {
                    home,
                    step_app,
                    app_detail,
                    step_herdr,
                    herdr_detail,
                } => {
                    if !show_checking {
                        return startup_frame(view()).into_element();
                    }
                    checking_screen(home, step_app, app_detail, step_herdr, herdr_detail)
                        .into_element()
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
                    let text_to_copy = cmd_for_copy.clone();
                    root_owner.run(move || {
                        spawn(async move {
                            let copied = blocking(move || copy_to_clipboard(&text_to_copy)).await;
                            match copied {
                                Ok(Ok(())) => copy_status.set("Copied".into()),
                                _ => copy_status.set("Copy failed".into()),
                            }
                        });
                    });
                },
            )
            .into_element()
        }
            })
        }))
}

fn copy_to_clipboard(content: &str) -> Result<()> {
    use std::io::Write;
    let mut child = std::process::Command::new("/usr/bin/pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(content.as_bytes())?;
    }
    ensure!(child.wait()?.success(), "Clipboard copy failed");
    Ok(())
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

fn step_row(
    title: &'static str,
    detail: impl IntoReactive<SharedString>,
    state: StepState,
) -> View {
    let (tone, kind, live) = match state {
        StepState::Ready => (Tone::Success, Icon::Check, false),
        StepState::Active => (Tone::Info, Icon::Refresh, true),
        StepState::Attention => (Tone::Attention, Icon::TriangleAlert, false),
        StepState::Error => (Tone::Danger, Icon::Close, false),
        StepState::Pending => (Tone::Neutral, Icon::Terminal, false),
    };
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
        .child(tone_icon_badge(kind, tone, live))
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

pub fn checking_screen(
    home: String,
    app_state: StepState,
    app_detail: String,
    herdr_state: StepState,
    herdr_detail: String,
) -> impl IntoElement {
    startup_frame(
        panel()
            .w_full()
            .max_w(px(720.))
            .gap_4()
            .child(heading("Starting Canban…"))
            .child(muted("Verifying local services and terminal runtime."))
            .child(startup_checklist(
                app_state,
                app_detail,
                herdr_state,
                herdr_detail,
            ))
            .child(muted(format!("Home: {home}"))),
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
                            .font_family("Menlo")
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
                            .font_family("Menlo")
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
}
