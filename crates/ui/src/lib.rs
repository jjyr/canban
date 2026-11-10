//! Production UI components and desktop host, shared with standalone examples.
use anyhow::Result;
use std::sync::Arc;
pub mod archive;
pub mod board;
pub mod card;
mod card_actions;
pub mod commands;
pub mod components;
pub mod drafts;
pub mod frame;
pub mod github;
pub mod machines;
pub mod navigation;
pub mod overview;
pub mod presenter;
pub mod profiles;
pub mod projects;
pub mod rail;
pub mod review;
mod screens;
pub mod utilities;
pub use rail::sidebar;
pub use screens::screen;
pub mod quick_open;
pub mod reminders;
mod sessions;
pub mod terminals;
pub mod workspace;
use nsh_core::prelude::*;
use presenter::*;
static APP: std::sync::OnceLock<Arc<canban_app_server::AppClient>> = std::sync::OnceLock::new();
#[native_module("canban/native")]
fn native(_: &mut Registry) {}
#[component]
pub fn Canban() -> impl IntoElement {
    let ui = Ui::new();
    canban_ui_kit::tokens::install_theme();
    commands::bindings(ui);
    utilities::init(ui);
    reminders::init(ui);
    projects::selection::remember(ui);
    terminals::bindings(ui);
    effect(move || {
        if ui.page.get() != "terminals" {
            nsh_core::element::desktop::set_title("Canban — Human Context");
        }
    });
    nsh_core::element::desktop::set_title_bar(
        nsh_core::desktop::TitleBarStyle::Hidden,
        Some(px(42.)),
    );
    refresh(ui);
    machines::refresh_catalog(ui);
    set_interval(std::time::Duration::from_secs(5), move || {
        machines::refresh_catalog(ui)
    });
    let events = stored(crate::APP.get().unwrap().subscribe());
    let dirty = signal(false);
    set_interval(std::time::Duration::from_millis(250), move || {
        let changed = events.with(|rx| rx.try_iter().collect::<Vec<_>>());
        if changed.iter().any(|e| {
            matches!(
                e,
                canban_types::AppEvent::Changed {
                    topic: "resync",
                    ..
                }
            )
        }) {
            ui.notification_resync.replace(true);
        }
        if !changed.is_empty() {
            dirty.set(true);
        }
        if dirty.get_untracked() && !ui.refreshing.get_untracked() {
            dirty.set(false);
            refresh(ui);
        }
    });
    frame::screen(ui)
}

pub fn run() -> Result<()> {
    let home = canban_app_server::default_home()?;
    let cli = std::env::current_exe()?.with_file_name("canban");
    anyhow::ensure!(
        cli.exists(),
        "Build the Agent CLI first: mise exec -- cargo build -p canban-cli"
    );
    let app = canban_app_server::AppClient::connect_or_start(home, &cli)?;
    APP.set(app).ok();
    nsh_platform_macos::set_components(canban_terminal::native::register);
    nsh_platform_macos::set_ui_sets(vec![nsh_base::registry(), registry()]);
    let app_id = std::env::var("CANBAN_APP_ID").unwrap_or_else(|_| "com.canban.desktop".into());
    let result = nsh_platform_macos::run_app(
        nsh_platform_macos::AppSource::Rust(
            nsh_platform_macos::Root::of::<Canban>().app_id(app_id.clone()),
        ),
        nsh_platform_macos::AppOptions {
            title: "Canban — Human Context".into(),
            app_id,
            size: (1360, 900),
            ..Default::default()
        },
    );
    if result != std::process::ExitCode::SUCCESS {
        anyhow::bail!("Native application exited with an error");
    }
    Ok(())
}
