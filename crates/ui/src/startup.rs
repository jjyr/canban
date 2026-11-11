//! Native startup failures and graceful controller-requested desktop shutdown.
use anyhow::{Result, ensure};
use canban_app_server::instance::{self, Registration};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};

static QUIT: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static UI_RECORD: OnceLock<std::path::PathBuf> = OnceLock::new();
static ERROR: OnceLock<String> = OnceLock::new();

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
    set_interval(std::time::Duration::from_millis(100), || {
        if QUIT.get().is_some_and(|v| v.swap(false, Ordering::SeqCst)) {
            nsh_core::element::desktop::quit();
        }
    });
}

fn record(field: &str, value: serde_json::Value) {
    if let Some(path) = UI_RECORD.get()
        && let Ok(bytes) = std::fs::read(path)
        && let Ok(mut saved) = serde_json::from_slice::<serde_json::Value>(&bytes)
    {
        saved[field] = value;
        let _ = canban_context::atomic(path, &serde_json::to_vec(&saved).unwrap_or_default());
    }
}

pub fn ready() {
    request_animation_frame(|_| record("ready", true.into()));
}

pub fn show(error: &anyhow::Error) {
    let detail = format!("{error:#}");
    record("error", detail.clone().into());
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

#[component]
fn StartupError() -> impl IntoElement {
    canban_ui_kit::tokens::install_theme();
    watch_quit();
    let home = canban_app_server::default_home()
        .map(|v| v.display().to_string())
        .unwrap_or_else(|_| "Unavailable".into());
    error_screen(home, ERROR.get().cloned().unwrap_or_default())
}

/// Production startup surface, also rendered by the native Gallery.
pub fn error_screen(home: String, error: String) -> impl IntoElement {
    view()
        .flex_1()
        .bg(BACKGROUND)
        .p_5()
        .items_center()
        .justify_center()
        .child(
            panel()
                .w_full()
                .max_w(px(720.))
                .gap_4()
                .child(heading("Canban could not start"))
                .child(muted(
                    "Resolve the connection or instance configuration, then open Canban again.",
                ))
                .child(label(format!("Home: {home}")))
                .child(notice(error, Tone::Attention))
                .child(action("Quit", Variant::Primary, |_| {
                    nsh_core::element::desktop::quit()
                })),
        )
}
