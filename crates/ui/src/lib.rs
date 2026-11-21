//! Production UI components and desktop host, shared with standalone examples.
use anyhow::Result;
pub mod agents_setup;
pub mod archive;
pub mod backend;
pub mod board;
pub(crate) mod cache;
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
pub(crate) mod index;
pub mod insights;
pub mod links;
pub mod navigation;
pub mod overview;
pub mod platform;
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
pub mod schedules;
pub mod sessions;
pub mod settings;
#[cfg(feature = "local-host")]
pub mod startup;
pub mod tags;
pub mod terminals;
pub mod workflows;
pub mod workspace;
use nsh_core::prelude::*;
use presenter::*;

pub(crate) fn with_ui_projection(method: &str, mut params: serde_json::Value) -> serde_json::Value {
    if params.get("fields").is_none()
        && matches!(
            method,
            "card.create"
                | "card.update"
                | "card.move"
                | "card.resolve"
                | "card.reopen"
                | "card.hold_auto_merge"
                | "card.restore"
                | "card.context.update"
                | "comment.add"
                | "comment.update"
                | "timeline.list"
        )
    {
        params["view"] = serde_json::json!("full");
    }
    params
}

pub(crate) fn routed_call(
    method: &str,
    params: serde_json::Value,
    active_project_id: Option<&str>,
) -> Result<serde_json::Value> {
    backend::call_routed(method, params, active_project_id)
}

pub(crate) fn call_all_hosts(
    method: &str,
    params: serde_json::Value,
) -> Vec<(String, Result<serde_json::Value>)> {
    backend::call_all_hosts(method, params)
}

pub(crate) fn merged_snapshot() -> Result<canban_types::Snapshot> {
    backend::merged_snapshot()
}

pub(crate) fn host_summaries() -> Vec<canban_types::HostSummary> {
    backend::host_summaries()
}

pub(crate) fn config_for_host(host_id: &str) -> canban_types::Config {
    backend::config_for_host(host_id)
}
#[native_module("canban/native")]
fn native(_: &mut Registry) {}
#[component]
pub fn Canban() -> impl IntoElement {
    canban_ui_kit::tokens::install_theme();
    workspace_root()
}

pub(crate) fn workspace_root() -> impl IntoElement {
    #[cfg(feature = "local-host")]
    startup::ready();
    let ui = Ui::new();
    #[cfg(feature = "local-host")]
    if let Some(boot) = startup::take_initial_bootstrap() {
        batch(move || {
            ui.apply_config(&boot.config);
            if let Some(id) = crate::projects::selection::restore(
                &boot.snapshot.projects,
                boot.saved_project.as_deref(),
            ) {
                ui.project.set(id);
            }
            ui.runtime_clock.set(canban_types::now());
            ui.snapshot.set(boot.snapshot);
            ui.snapshot_ready.set(true);
        });
    }
    ui.backend_notice
        .set(backend::server_notice().unwrap_or_default());
    updates::restore(ui);
    links::install(ui);
    let last_draft_err_epoch = stored(drafts::persistence::error_epoch());
    set_interval(std::time::Duration::from_millis(250), move || {
        let epoch = drafts::persistence::error_epoch();
        if last_draft_err_epoch.get() != epoch {
            last_draft_err_epoch.replace(epoch);
            ui.draft_status.update(|n| *n += 1);
        }
    });
    commands::bindings(ui);
    utilities::init(ui);
    updates::init(ui);
    github::init(ui);
    github::filters::init(ui);
    reminders::init(ui);
    projects::selection::remember(ui);
    terminals::bindings(ui);
    let window = nsh_core::element::desktop::current_window();
    let in_terminals = computed(move || ui.page.get() == "terminals");
    effect(move || {
        if !in_terminals.get() {
            let mut attributes = nsh_core::desktop::WindowAttributes::default();
            attributes.title = Some("Canban — Human Context".into());
            window.set(attributes);
        }
    });
    frame::configure_window_chrome();
    refresh(ui);
    set_interval(std::time::Duration::from_secs(5), move || {
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
        if backend::is_connected() {
            let target = ui.effective_config_host();
            ui.apply_config(&crate::config_for_host(&target));
            if last_cfg_host.get_untracked() != target {
                last_cfg_host.set(target);
                ui.agent_catalog.set(serde_json::Value::Null);
                if page == "settings" || page == "agent-setup" {
                    agents_setup::refresh_catalog(ui);
                }
            }
        }
    });
    review::init(ui);
    let events = stored(backend::subscribe());
    let dirty = signal(false);
    set_interval(std::time::Duration::from_millis(250), move || {
        let changed = events.with(|slot| {
            slot.as_ref()
                .map(|rx| rx.try_iter().collect::<Vec<_>>())
                .unwrap_or_default()
        });
        if changed.iter().any(|e| {
            matches!(
                e.app_event(),
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
                e.app_event(),
                canban_types::AppEvent::Changed {
                    topic: "github" | "resync",
                    ..
                }
            )
        }) {
            ui.github.epoch.update(|v| *v += 1);
        }
        review::handle_host_events(ui, &changed);
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

#[cfg(feature = "local-host")]
pub fn run() -> Result<()> {
    platform::init_host(vec![nsh_base::registry(), registry()]);
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

#[cfg(feature = "local-host")]
fn run_application(ran_window: &mut bool) -> Result<()> {
    let home = canban_app_server::default_home()?;
    let cli = std::env::current_exe()?.with_file_name("canban");
    anyhow::ensure!(
        cli.exists(),
        "Build the Agent CLI first: mise exec -- cargo build -p canban-cli"
    );
    let app_id = startup::app_id()?;
    let manual_recovery = canban_updater::handoff::journal(&home).map_or_else(
        |_| home.join("updates/install.json").exists(),
        |j| j.phase == "manual_recovery",
    );
    *ran_window = true;
    let ok = if manual_recovery {
        platform::run_root::<updates::Recovery>(app_id, "Canban — Human Context", (1360, 900))
    } else {
        platform::run_root::<startup::Bootstrap>(app_id, "Canban — Human Context", (1360, 900))
    };
    github::filters::flush();
    drafts::persistence::flush();
    if startup::fatal_exit() {
        anyhow::bail!("Canban startup failed");
    }
    if !ok {
        anyhow::bail!("Native application exited with an error");
    }
    Ok(())
}
