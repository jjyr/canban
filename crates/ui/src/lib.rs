//! Production UI components and desktop host, shared with standalone examples.
use anyhow::Result;
use std::sync::Arc;
pub mod agents_setup;
pub mod archive;
pub mod board;
pub mod card;
mod card_actions;
pub mod commands;
pub mod components;
pub mod drafts;
pub mod editor;
pub mod frame;
pub mod github;
pub mod links;
pub mod machines;
pub mod navigation;
pub mod overview;
pub mod presenter;
pub mod profiles;
pub mod projects;
pub mod rail;
mod references;
pub mod review;
mod screens;
pub mod updates;
pub mod utilities;
pub use rail::sidebar;
pub use screens::screen;
pub mod quick_open;
pub mod reminders;
pub mod sessions;
pub mod settings;
pub mod startup;
pub mod terminals;
pub mod workflows;
pub mod workspace;
use nsh_core::prelude::*;
use presenter::*;
static APP: std::sync::OnceLock<Arc<canban_app_server::AppClient>> = std::sync::OnceLock::new();
#[native_module("canban/native")]
fn native(_: &mut Registry) {}
#[component]
pub fn Canban() -> impl IntoElement {
    startup::watch_quit();
    startup::ready();
    let ui = Ui::new();
    ui.backend_notice.set(
        APP.get()
            .and_then(|app| app.server_notice())
            .unwrap_or_default()
            .to_owned(),
    );
    updates::restore(ui);
    links::install(ui);
    set_interval(std::time::Duration::from_millis(250), move || {
        ui.draft_status.update(|n| *n += 1)
    });
    canban_ui_kit::tokens::install_theme();
    commands::bindings(ui);
    utilities::init(ui);
    updates::init(ui);
    github::init(ui);
    reminders::init(ui);
    projects::selection::remember(ui);
    terminals::bindings(ui);
    let window = nsh_core::element::desktop::current_window();
    effect(move || {
        if ui.page.get() != "terminals" {
            let mut attributes = nsh_core::desktop::WindowAttributes::default();
            attributes.title = Some("Canban — Human Context".into());
            window.set(attributes);
        }
    });
    frame::configure_window_chrome();
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
        if changed.iter().any(|e| {
            matches!(
                e,
                canban_types::AppEvent::Changed {
                    topic: "github" | "resync",
                    ..
                }
            )
        }) {
            ui.github.epoch.update(|v| *v += 1);
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
    nsh_platform_macos::set_components(canban_terminal::native::register);
    nsh_platform_macos::set_ui_sets(vec![nsh_base::registry(), registry()]);
    let mut session = None;
    let result = (|| {
        session = startup::prepare()?;
        run_application()
    })();
    if let Err(error) = &result {
        startup::show(error);
    }
    drop(session);
    result
}

fn run_application() -> Result<()> {
    let home = canban_app_server::default_home()?;
    let cli = std::env::current_exe()?.with_file_name("canban");
    anyhow::ensure!(
        cli.exists(),
        "Build the Agent CLI first: mise exec -- cargo build -p canban-cli"
    );
    let app_id = startup::app_id()?;
    let source = if canban_updater::handoff::journal(&home).map_or_else(
        |_| home.join("updates/install.json").exists(),
        |j| j.phase == "manual_recovery",
    ) {
        nsh_platform_macos::AppSource::Rust(
            nsh_platform_macos::Root::of::<updates::Recovery>().app_id(app_id.clone()),
        )
    } else {
        let app = canban_app_server::AppClient::connect_or_start(home, &cli)?;
        drafts::persistence::start(app.clone())?;
        APP.set(app).ok();
        nsh_platform_macos::AppSource::Rust(
            nsh_platform_macos::Root::of::<Canban>().app_id(app_id.clone()),
        )
    };
    let result = nsh_platform_macos::run_app(
        source,
        nsh_platform_macos::AppOptions {
            title: "Canban — Human Context".into(),
            app_id,
            size: (1360, 900),
            ..Default::default()
        },
    );
    drafts::persistence::flush();
    if result != std::process::ExitCode::SUCCESS {
        anyhow::bail!("Native application exited with an error");
    }
    Ok(())
}
