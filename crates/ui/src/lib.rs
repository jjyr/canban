//! Production UI components and desktop host, shared with standalone examples.
use anyhow::Result;
pub mod activity;
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
pub mod index;
pub mod insights;
pub mod links;
pub mod mobile;
pub mod navigation;
pub mod overview;
pub mod platform;
pub mod presence;
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

pub fn with_ui_projection(method: &str, mut params: serde_json::Value) -> serde_json::Value {
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

pub(crate) fn merged_runtime_state() -> Result<Option<canban_types::RuntimeState>> {
    backend::merged_runtime_state()
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
            let restored = crate::projects::selection::restore(
                &boot.snapshot.projects,
                boot.saved_project.as_deref(),
            );
            ui.pending_remote_project.replace(
                crate::projects::selection::PendingRemoteRestore::from_bootstrap(
                    &boot.snapshot.projects,
                    boot.saved_project.as_deref(),
                    restored.as_deref(),
                ),
            );
            if let Some(id) = restored {
                ui.project.set(id);
            }
            ui.runtime_clock.set(canban_types::now());
            ui.accept_snapshot(boot.snapshot);
            ui.snapshot_ready.set(true);
        });
    }
    ui.backend_notice
        .set(backend::server_notice().unwrap_or_default());
    updates::restore(ui);
    links::install(ui);
    let mut draft_err_rx = drafts::persistence::subscribe_error_epoch();
    ui.spawn(async move {
        while draft_err_rx.changed().await.is_ok() {
            let Some(cur) = ui.draft_status.try_get() else {
                break;
            };
            let _ = ui.draft_status.try_set(cur.wrapping_add(1));
        }
    });
    commands::bindings(ui);
    utilities::init(ui);
    activity::init(ui);
    presence::init(ui);
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
    if let Some(mut events_rx) = backend::subscribe() {
        ui.spawn(async move {
            while let Some(first) = events_rx.recv().await {
                let mut events = vec![first];
                while let Ok(more) = events_rx.try_recv() {
                    events.push(more);
                }
                handle_backend_events(ui, events);
            }
        });
    }
    frame::screen(ui)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BackendRefreshAction {
    #[default]
    None,
    Runtime,
    Full,
}

#[derive(Clone, Debug, Default)]
pub struct BackendEventPlan {
    pub local_update_pushes: Vec<serde_json::Value>,
    pub local_utility_pushes: Vec<serde_json::Value>,
    pub usage_pushes: Vec<(String, serde_json::Value)>,
    pub host_events: Vec<backend::BackendEvent>,
    pub notification_resync: bool,
    pub refresh_local_side_channels: bool,
    pub bump_github_epoch: bool,
    pub bump_env_sync_epoch: bool,
    pub refresh_action: BackendRefreshAction,
}

pub fn is_runtime_only_event_batch(events: &[backend::BackendEvent]) -> bool {
    !events.is_empty()
        && events.iter().all(|ev| {
            matches!(
                ev.app_event(),
                Some(canban_types::AppEvent::Changed {
                    topic: "terminals" | "reclaim",
                    ..
                })
            )
        })
}

pub fn plan_backend_events(
    events: Vec<backend::BackendEvent>,
    snapshot_ready: bool,
) -> BackendEventPlan {
    let mut plan = BackendEventPlan::default();
    for ev in events {
        match ev {
            backend::BackendEvent::UpdateChanged { host_id, state } => {
                if host_id == "local" {
                    plan.local_update_pushes.push(state);
                }
            }
            backend::BackendEvent::UtilitiesChanged { host_id, state } => {
                if host_id == "local" {
                    plan.local_utility_pushes.push(state);
                }
            }
            backend::BackendEvent::UsageChanged { host_id, state } => {
                plan.usage_pushes.push((host_id, state));
            }
            backend::BackendEvent::HerdrInstall { .. } => {}
            ev @ backend::BackendEvent::Host { .. } => {
                plan.host_events.push(ev);
            }
        }
    }
    if plan.host_events.is_empty() {
        return plan;
    }
    plan.notification_resync = plan.host_events.iter().any(|e| {
        matches!(
            e.app_event(),
            Some(canban_types::AppEvent::Changed {
                topic: "resync",
                ..
            })
        )
    });
    plan.refresh_local_side_channels = plan.host_events.iter().any(|e| {
        e.host_id() == "local"
            && matches!(
                e.app_event(),
                Some(canban_types::AppEvent::Changed {
                    topic: "resync",
                    ..
                })
            )
    });
    plan.bump_github_epoch = plan.host_events.iter().any(|e| {
        matches!(
            e.app_event(),
            Some(canban_types::AppEvent::Changed {
                topic: "github" | "resync",
                ..
            })
        )
    });
    plan.bump_env_sync_epoch = plan.host_events.iter().any(|e| {
        matches!(
            e.app_event(),
            Some(canban_types::AppEvent::Changed {
                topic: "global_env_sync" | "configuration" | "host" | "resync",
                ..
            })
        )
    });
    let env_sync_only = plan.host_events.iter().all(|e| {
        matches!(
            e.app_event(),
            Some(canban_types::AppEvent::Changed {
                topic: "global_env_sync",
                ..
            })
        )
    });
    if !env_sync_only {
        plan.refresh_action = if snapshot_ready && is_runtime_only_event_batch(&plan.host_events) {
            BackendRefreshAction::Runtime
        } else {
            BackendRefreshAction::Full
        };
    }
    plan
}

pub(crate) fn handle_backend_events(ui: Ui, events: Vec<backend::BackendEvent>) {
    let Some(snapshot_ready) = ui.snapshot_ready.try_get() else {
        return;
    };
    let plan = plan_backend_events(events, snapshot_ready);
    for state in plan.local_update_pushes {
        updates::apply_push(ui, state);
    }
    for state in plan.local_utility_pushes {
        utilities::apply_push(ui, state);
    }
    for (host_id, state) in plan.usage_pushes {
        utilities::apply_usage_push(ui, &host_id, state);
    }
    if plan.host_events.is_empty() {
        return;
    }
    if plan.notification_resync {
        ui.notification_resync.replace(true);
    }
    if plan.refresh_local_side_channels {
        updates::refresh(ui);
        utilities::refresh(ui);
    }
    if plan.bump_github_epoch {
        ui.github.epoch.update(|v| *v += 1);
    }
    review::handle_host_events(ui, &plan.host_events);
    if plan.bump_env_sync_epoch {
        ui.env_sync_epoch.update(|v| *v += 1);
    }
    match plan.refresh_action {
        BackendRefreshAction::None => {}
        BackendRefreshAction::Runtime => refresh_runtime(ui),
        BackendRefreshAction::Full => refresh(ui),
    }
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
    presence::clear_blocking();
    if startup::fatal_exit() {
        anyhow::bail!("Canban startup failed");
    }
    if !ok {
        anyhow::bail!("Native application exited with an error");
    }
    Ok(())
}
