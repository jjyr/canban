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
pub(crate) mod index;
pub mod insights;
pub mod links;
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
            if let Some(id) = crate::projects::selection::restore(
                &boot.snapshot.projects,
                boot.saved_project.as_deref(),
            ) {
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

pub(crate) fn is_runtime_only_event_batch(events: &[backend::BackendEvent]) -> bool {
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

pub(crate) fn handle_backend_events(ui: Ui, events: Vec<backend::BackendEvent>) {
    if ui.snapshot_ready.try_get().is_none() {
        return;
    }
    let mut host_events = Vec::new();
    for ev in events {
        match ev {
            backend::BackendEvent::UpdateChanged { host_id, state } => {
                if host_id == "local" {
                    updates::apply_push(ui, state);
                }
            }
            backend::BackendEvent::UtilitiesChanged { host_id, state } => {
                if host_id == "local" {
                    utilities::apply_push(ui, state);
                }
            }
            backend::BackendEvent::UsageChanged { host_id, state } => {
                utilities::apply_usage_push(ui, &host_id, state);
            }
            backend::BackendEvent::HerdrInstall { .. } => {}
            ev @ backend::BackendEvent::Host { .. } => {
                host_events.push(ev);
            }
        }
    }
    if host_events.is_empty() {
        return;
    }
    if host_events.iter().any(|e| {
        matches!(
            e.app_event(),
            Some(canban_types::AppEvent::Changed {
                topic: "resync",
                ..
            })
        )
    }) {
        ui.notification_resync.replace(true);
    }
    if host_events.iter().any(|e| {
        e.host_id() == "local"
            && matches!(
                e.app_event(),
                Some(canban_types::AppEvent::Changed {
                    topic: "resync",
                    ..
                })
            )
    }) {
        updates::refresh(ui);
        utilities::refresh(ui);
    }
    if host_events.iter().any(|e| {
        matches!(
            e.app_event(),
            Some(canban_types::AppEvent::Changed {
                topic: "github" | "resync",
                ..
            })
        )
    }) {
        ui.github.epoch.update(|v| *v += 1);
    }
    review::handle_host_events(ui, &host_events);
    if ui.snapshot_ready.get_untracked() && is_runtime_only_event_batch(&host_events) {
        refresh_runtime(ui);
    } else {
        refresh(ui);
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

#[cfg(test)]
mod push_tests {
    use super::*;
    use serde_json::{Value, json};
    use std::sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };
    use std::time::{Duration, Instant};

    #[test]
    fn handle_backend_events_applies_side_channels_and_guards_pending_utilities() {
        static EVENTS_TX: OnceLock<tokio::sync::mpsc::UnboundedSender<backend::BackendEvent>> =
            OnceLock::new();
        static PENDING_ON: AtomicBool = AtomicBool::new(false);
        static OBSERVED_PHASE: Mutex<String> = Mutex::new(String::new());
        static OBSERVED_THEME: Mutex<String> = Mutex::new(String::new());
        static OBSERVED_USAGE: Mutex<Value> = Mutex::new(Value::Null);

        #[component]
        fn ProbeRoot() -> impl IntoElement {
            let ui = Ui::new();
            ui.snapshot_ready.set(true);
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = EVENTS_TX.set(tx);
            ui.spawn(async move {
                while let Some(first) = rx.recv().await {
                    let mut batch = vec![first];
                    while let Ok(more) = rx.try_recv() {
                        batch.push(more);
                    }
                    if PENDING_ON.load(Ordering::SeqCst) {
                        ui.pending_methods.update(|s| {
                            s.insert("utilities.configure");
                        });
                    } else {
                        ui.pending_methods.update(|s| {
                            s.remove("utilities.configure");
                        });
                    }
                    handle_backend_events(ui, batch);
                    *OBSERVED_PHASE.lock().unwrap() = ui
                        .update
                        .with_untracked(|v| v["state"]["phase"].as_str().unwrap_or("").to_owned());
                    *OBSERVED_THEME.lock().unwrap() = ui.utility.with_untracked(|v| {
                        v["preferences"]["theme"].as_str().unwrap_or("").to_owned()
                    });
                    *OBSERVED_USAGE.lock().unwrap() = ui.usage.get_untracked();
                }
            });
            view()
        }

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_time()
            .build()
            .unwrap();
        let mut core = nsh_core::Core::new(
            nsh_core::ApproxTextEngine,
            nsh_core::geometry::size(px(300.), px(200.)),
        );
        core.set_task_runtime(runtime.handle().clone());
        core.mount_root(&nsh_core::element::Root::of::<ProbeRoot>());
        core.run_component_tasks();
        core.commit();

        let tx = EVENTS_TX.get().unwrap();
        // Remote update.changed is ignored; local update.changed updates ui.update immediately.
        tx.send(backend::BackendEvent::UpdateChanged {
            host_id: "remote-1".into(),
            state: json!({"state": {"phase": "ignored"}}),
        })
        .unwrap();
        tx.send(backend::BackendEvent::UpdateChanged {
            host_id: "local".into(),
            state: json!({"state": {"phase": "downloading", "progress": 60}}),
        })
        .unwrap();
        tx.send(backend::BackendEvent::UtilitiesChanged {
            host_id: "local".into(),
            state: json!({"preferences": {"theme": "nord"}}),
        })
        .unwrap();
        // Mismatched usage host is ignored; matching local host applies.
        tx.send(backend::BackendEvent::UsageChanged {
            host_id: "other-host".into(),
            state: json!({"readings": [{"provider": "ignored"}]}),
        })
        .unwrap();
        tx.send(backend::BackendEvent::UsageChanged {
            host_id: "local".into(),
            state: json!({"readings": [{"provider": "codex", "used": 17}]}),
        })
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            core.run_component_tasks();
            core.commit();
            if OBSERVED_PHASE.lock().unwrap().as_str() == "downloading"
                && OBSERVED_THEME.lock().unwrap().as_str() == "nord"
                && OBSERVED_USAGE.lock().unwrap()["readings"][0]["used"] == 17
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(OBSERVED_PHASE.lock().unwrap().as_str(), "downloading");
        assert_eq!(OBSERVED_THEME.lock().unwrap().as_str(), "nord");
        assert_eq!(OBSERVED_USAGE.lock().unwrap()["host_id"], "local");

        // Optimistic `utilities.configure` in flight blocks overwriting `ui.utility`.
        PENDING_ON.store(true, Ordering::SeqCst);
        tx.send(backend::BackendEvent::UtilitiesChanged {
            host_id: "local".into(),
            state: json!({"preferences": {"theme": "stale-theme"}}),
        })
        .unwrap();
        for _ in 0..10 {
            core.run_component_tasks();
            core.commit();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            OBSERVED_THEME.lock().unwrap().as_str(),
            "nord",
            "pending utilities.configure must protect optimistic preference state"
        );

        drop(core);
        drop(runtime);
    }

    #[test]
    fn draft_error_watch_wakes_ui_spawn_consumer_without_polling() {
        static DRAFT_STATUS_VAL: AtomicU64 = AtomicU64::new(0);

        #[component]
        fn ProbeDrafts() -> impl IntoElement {
            let ui = Ui::new();
            let mut rx = drafts::persistence::subscribe_error_epoch();
            ui.spawn(async move {
                while rx.changed().await.is_ok() {
                    if let Some(cur) = ui.draft_status.try_get() {
                        let next = cur.wrapping_add(1);
                        let _ = ui.draft_status.try_set(next);
                        DRAFT_STATUS_VAL.store(next, Ordering::SeqCst);
                    }
                }
            });
            view()
        }

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_time()
            .build()
            .unwrap();
        let mut core = nsh_core::Core::new(
            nsh_core::ApproxTextEngine,
            nsh_core::geometry::size(px(300.), px(200.)),
        );
        core.set_task_runtime(runtime.handle().clone());
        core.mount_root(&nsh_core::element::Root::of::<ProbeDrafts>());
        core.run_component_tasks();
        core.commit();

        assert_eq!(DRAFT_STATUS_VAL.load(Ordering::SeqCst), 0);
        let worker = std::thread::spawn(|| {
            drafts::persistence::bump_error_epoch();
        });
        worker.join().unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && DRAFT_STATUS_VAL.load(Ordering::SeqCst) == 0 {
            core.run_component_tasks();
            core.commit();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(DRAFT_STATUS_VAL.load(Ordering::SeqCst), 1);

        drop(core);
        drop(runtime);
    }

    #[test]
    fn runtime_only_event_batch_classification() {
        let host_ev = |topic: &'static str| backend::BackendEvent::Host {
            host_id: "local".into(),
            event: canban_types::AppEvent::Changed {
                topic,
                card_id: None,
            },
        };

        assert!(!is_runtime_only_event_batch(&[]));
        assert!(is_runtime_only_event_batch(&[host_ev("terminals")]));
        assert!(is_runtime_only_event_batch(&[host_ev("reclaim")]));
        assert!(is_runtime_only_event_batch(&[
            host_ev("terminals"),
            host_ev("reclaim"),
        ]));
        assert!(!is_runtime_only_event_batch(&[
            host_ev("terminals"),
            host_ev("application"),
        ]));
        assert!(!is_runtime_only_event_batch(&[host_ev("application")]));
        assert!(!is_runtime_only_event_batch(&[host_ev("review")]));
        assert!(!is_runtime_only_event_batch(&[host_ev("github")]));
        assert!(!is_runtime_only_event_batch(&[host_ev("resync")]));
        assert!(!is_runtime_only_event_batch(&[host_ev("runtime")]));
        assert!(!is_runtime_only_event_batch(&[
            backend::BackendEvent::UtilitiesChanged {
                host_id: "local".into(),
                state: serde_json::json!({}),
            }
        ]));
    }
}
