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
pub mod diagnostics;
pub mod drafts;
pub mod editor;
pub mod frame;
pub mod github;
pub mod hosts;
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
pub mod tags;
pub mod terminals;
pub mod workflows;
pub mod workspace;
use nsh_core::prelude::*;
use presenter::*;
static APP: std::sync::OnceLock<Arc<canban_app_server::AppClient>> = std::sync::OnceLock::new();
pub(crate) static HOSTS: std::sync::OnceLock<Arc<canban_app_server::hosts::HostRegistry>> =
    std::sync::OnceLock::new();

pub(crate) fn routed_call(
    method: &str,
    params: serde_json::Value,
    active_project_id: Option<&str>,
) -> Result<serde_json::Value> {
    if let Some(hosts) = HOSTS.get() {
        return hosts.call_routed(method, params, active_project_id);
    }
    let app = APP
        .get()
        .ok_or_else(|| anyhow::anyhow!("Gallery preview: no AppServer connected"))?;
    app.call(method, params)
}

pub(crate) fn merged_snapshot() -> Result<canban_types::Snapshot> {
    if let Some(hosts) = HOSTS.get() {
        return hosts.merged_snapshot();
    }
    let app = APP
        .get()
        .ok_or_else(|| anyhow::anyhow!("Gallery preview: no AppServer connected"))?;
    app.snapshot()
}

pub(crate) fn host_summaries() -> Vec<canban_types::HostSummary> {
    if let Some(hosts) = HOSTS.get() {
        return hosts.summaries();
    }
    Vec::new()
}

pub(crate) fn config_for_host(host_id: &str) -> canban_types::Config {
    if let Some(hosts) = HOSTS.get() {
        return hosts.config_for(host_id);
    }
    APP.get().map(|app| app.config()).unwrap_or_default()
}
#[native_module("canban/native")]
fn native(_: &mut Registry) {}
#[component]
pub fn Canban() -> impl IntoElement {
    canban_ui_kit::tokens::install_theme();
    workspace_root()
}

pub(crate) fn workspace_root() -> impl IntoElement {
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
        machines::refresh_catalog(ui);
        let has_active_runtime = ui.snapshot.with_untracked(|s| {
            s.workflow_runs.iter().any(|r| {
                r.active()
                    && matches!(
                        r.state.as_str(),
                        "waiting_agent" | "running" | "waiting_script"
                    )
            }) || s.terminals.iter().any(|t| {
                matches!(
                    t.herdr_status(),
                    canban_types::HerdrStatus::Working
                        | canban_types::HerdrStatus::Starting
                        | canban_types::HerdrStatus::Blocked
                )
            })
        });
        if has_active_runtime {
            ui.runtime_clock.set(canban_types::now());
        }
    });
    let last_cfg_host = signal(String::from("local"));
    effect(move || {
        let page = ui.page.get();
        let _ = ui.project.get();
        let _ = ui.settings_host.get();
        if crate::APP.get().is_some() {
            let target = ui.effective_config_host();
            ui.apply_config(&crate::config_for_host(&target));
            if last_cfg_host.get_untracked() != target {
                last_cfg_host.set(target);
                ui.agent_catalog.set(serde_json::Value::Null);
                if page == "settings" || page == "agent-setup" {
                    agents_setup::refresh_catalog(ui, "local");
                }
            }
        }
    });
    let events = stored(if let Some(hosts) = crate::HOSTS.get() {
        hosts.subscribe()
    } else {
        crate::APP.get().unwrap().subscribe()
    });
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
        if ui.page.get_untracked() == "review"
            && changed.iter().any(|e| {
                matches!(
                    e,
                    canban_types::AppEvent::Changed {
                        topic: "review" | "resync",
                        ..
                    }
                )
            })
        {
            review::load_refs(ui);
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
    let mut ran_window = false;
    let result = (|| {
        session = startup::prepare()?;
        run_application(&mut ran_window)
    })();
    if let Err(error) = &result
        && !ran_window
    {
        startup::show(error);
    }
    drop(session);
    result
}

fn run_application(ran_window: &mut bool) -> Result<()> {
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
        nsh_platform_macos::AppSource::Rust(
            nsh_platform_macos::Root::of::<startup::Bootstrap>().app_id(app_id.clone()),
        )
    };
    *ran_window = true;
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
    if startup::fatal_exit() {
        anyhow::bail!("Canban startup failed");
    }
    if result != std::process::ExitCode::SUCCESS {
        anyhow::bail!("Native application exited with an error");
    }
    Ok(())
}
