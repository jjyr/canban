//! Global bottom-rail update intent and upgrade-session persistence.
use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Serialize, Deserialize)]
struct Session {
    location: Location,
    drafts: std::collections::BTreeMap<String, crate::drafts::Draft>,
    #[serde(default)]
    uploads: std::collections::BTreeMap<String, Vec<crate::card::attachments::Upload>>,
    tabs: Vec<String>,
    archive_query: String,
}
fn capture(ui: Ui) -> Session {
    let drafts = ui.drafts.get();
    Session {
        location: ui.location(),
        uploads: drafts
            .iter()
            .filter(|(_, d)| !d.uploads.is_empty())
            .map(|(k, d)| (k.clone(), d.uploads.clone()))
            .collect(),
        drafts,
        tabs: ui.terminal_tabs.get_untracked(),
        archive_query: ui.archive_query.get_untracked(),
    }
}
fn decode_session(bytes: &[u8]) -> anyhow::Result<Session> {
    let value: Value = serde_json::from_slice(bytes)?;
    let mut saved: Session = serde_json::from_value(value.clone())?;
    // Earlier updater builds stored transient uploads inside Draft. Durable
    // editor drafts intentionally omit them from SQLite; retain upgrade bytes.
    if let Some(drafts) = value["drafts"].as_object() {
        for (key, draft) in drafts {
            if !saved.uploads.contains_key(key) && draft["uploads"].is_array() {
                saved.uploads.insert(
                    key.clone(),
                    serde_json::from_value(draft["uploads"].clone())?,
                );
            }
        }
    }
    Ok(saved)
}

#[cfg(test)]
mod session_tests {
    use super::*;
    #[test]
    fn legacy_and_durable_editor_sessions_keep_transient_uploads_out_of_sql_drafts() {
        let legacy = json!({
            "location":{"page":"card","project":"project","card":"card","request":"","terminal":"","standalone":false,"terminal_card":""},
            "drafts":{"card:card":{"comment":"unsent","uploads":[{"key":"upload-key","name":"paste.txt","file":["paste.txt","text/plain",[1,2,3]],"path":null,"comment":true,"state":"Uploading"}]}},
            "tabs":[],"archive_query":""
        });
        let saved = decode_session(&serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert!(saved.location.new_draft.is_empty());
        assert_eq!(saved.uploads["card:card"].len(), 1);
        assert!(saved.drafts["card:card"].uploads.is_empty());
        let encoded = serde_json::to_value(&saved).unwrap();
        assert!(encoded["drafts"]["card:card"].get("uploads").is_none());
        let restored = decode_session(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(&restored.uploads).unwrap(),
            serde_json::to_value(&saved.uploads).unwrap()
        );
        assert_eq!(restored.drafts["card:card"].comment, "unsent");
    }
}
pub fn restore(ui: Ui) {
    let Some(app) = crate::APP.get() else {
        return;
    };
    let path = app.home.join("updates/ui-session.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(mut saved) = decode_session(&bytes) else {
        return;
    };
    for (key, draft) in &mut saved.drafts {
        if let Some(uploads) = saved.uploads.remove(key) {
            draft.uploads = uploads;
        }
        crate::card::attachments::pause_after_upgrade(draft);
    }
    ui.drafts.replace(saved.drafts);
    ui.project.set(saved.location.project.clone());
    let location = stored(Some(saved.location));
    let tabs = stored(saved.tabs);
    let query = stored(saved.archive_query);
    effect(move || {
        if !ui.snapshot_ready.get() {
            return;
        }
        let Some(saved) = location.with(|v| v.take()) else {
            return;
        };
        batch(move || {
            ui.project.set(saved.project);
            ui.new_draft.set(saved.new_draft);
            ui.card.set(saved.card);
            ui.request.set(saved.request);
            ui.page.set(saved.page);
            ui.card_standalone.set(saved.standalone);
            ui.terminal_card_context.set(saved.terminal_card);
            ui.terminal_selection.set(saved.terminal);
            ui.terminal_tabs.set(tabs.get());
            ui.archive_query.set(query.get());
        });
    });
    if !canban_updater::handoff::active(&app.home) {
        let _ = std::fs::remove_file(path);
    }
}
pub fn init(ui: Ui) {
    let saved_token = stored(String::new());
    let ready = stored(false);
    refresh(ui);
    set_interval(std::time::Duration::from_secs(1), move || refresh(ui));
    effect(move || {
        let v = ui.update.get();
        let phase = v["install"]["phase"].as_str().unwrap_or("");
        if matches!(phase, "prepared" | "stopping") {
            let token = v["install"]["token"].as_str().unwrap_or("").to_owned();
            if !token.is_empty() && saved_token.get() != token {
                saved_token.replace(token.clone());
                let snapshot = capture(ui);
                let app = crate::APP.get().cloned();
                ui.spawn(async move {
                    if let Some(app) = app {
                        let r = blocking(move || -> anyhow::Result<()> {
                            canban_updater::atomic(
                                &app.home.join("updates/ui-session.json"),
                                &serde_json::to_vec(&snapshot)?,
                            )?;
                            canban_updater::atomic(
                                &app.home.join("updates").join(format!("ui-saved-{token}")),
                                b"ready",
                            )
                        })
                        .await;
                        match r {
                            Ok(Ok(())) => {}
                            other => ui
                                .operation_error
                                .set(format!("Could not preserve update session: {other:?}")),
                        }
                    }
                });
            }
        }
        let snapshot_ready = ui.snapshot_ready.get();
        let refreshing = ui.refreshing.get();
        let op_error = ui.operation_error.get();
        if std::env::var_os("CANBAN_UPGRADE_TOKEN").is_some()
            && snapshot_ready
            && !refreshing
            && op_error.is_empty()
            && !ready.get()
        {
            ready.replace(true);
            // Write readiness directly instead of gating on `request_animation_frame`,
            // which macOS suspends when a background-spawned candidate window is occluded
            // or on an inactive Space.
            if let (Some(app), Ok(token)) = (
                crate::APP.get().cloned(),
                std::env::var("CANBAN_UPGRADE_TOKEN"),
            ) {
                ui.spawn(async move {
                    let _ = blocking(move || {
                        let path = app.home.join("updates").join(format!("ui-ready-{token}"));
                        canban_updater::atomic(
                            &path,
                            &serde_json::to_vec(&canban_updater::Build::current())
                                .unwrap_or_default(),
                        )
                    })
                    .await;
                });
            }
        }
    });
}
fn refresh(ui: Ui) {
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    if ui.update_loading.get_untracked() {
        return;
    }
    ui.update_loading.set(true);
    ui.spawn(async move {
        let r = blocking(move || app.call("update.state", json!({}))).await;
        ui.update_loading.set(false);
        if let Ok(Ok(v)) = r {
            if !matches!(
                v["state"]["phase"].as_str(),
                Some("downloading" | "installing")
            ) {
                ui.update_busy.set(false);
            }
            if v != ui.update.get_untracked() {
                ui.update.set(v);
            }
        }
    });
}
pub fn check(ui: Ui) {
    if crate::APP.get().is_none() {
        ui.update
            .update(|v| v["state"]["phase"] = json!("checking"));
        return;
    }
    dispatch_complete(ui, "update.check", json!({}), move |_| refresh(ui));
}
pub fn install(ui: Ui) {
    if ui.update_busy.get_untracked() {
        return;
    }
    ui.update_busy.set(true);
    if crate::APP.get().is_none() {
        ui.update.update(|v| {
            v["state"]["phase"] = json!("downloading");
            v["state"]["progress"] = json!(42);
        });
        return;
    }
    dispatch_complete(
        ui,
        "update.install",
        json!({"version":ui.update.get_untracked()["state"]["available"]["manifest"]["version"],"ui_pid":std::process::id(),"app_id":std::env::var("CANBAN_APP_ID").unwrap_or_else(|_|"com.canban.desktop".into())}),
        move |r| {
            if r.is_err() {
                ui.update_busy.set(false);
            }
            refresh(ui);
        },
    );
}
fn busy(ui: Ui) -> bool {
    ui.update_busy.get()
        || matches!(
            ui.update.get()["state"]["phase"].as_str(),
            Some("downloading" | "installing" | "checking")
        )
}
fn hint(v: &Value) -> String {
    match v["state"]["phase"].as_str().unwrap_or("") {
        "downloading" => format!(
            "Downloading update · {}%",
            v["state"]["progress"].as_u64().unwrap_or(0)
        ),
        "installing" => "Installing update and restarting…".into(),
        "manual_recovery" => "Update requires manual recovery · Open Settings".into(),
        "error" => "Update failed · Click to retry".into(),
        _ => format!(
            "Update to {} · Install and restart",
            v["state"]["available"]["manifest"]["version"]
                .as_str()
                .unwrap_or("new version")
        ),
    }
}
pub fn rail(ui: Ui) -> impl IntoElement {
    when(
        move || {
            let value = ui.update.get();
            (value["disabled"] != true && !value["state"]["available"].is_null())
                || value["state"]["phase"] == "manual_recovery"
        },
        move || {
            view()
                .w(px(40.))
                .h(px(40.))
                .tooltip(hint_tooltip(move || hint(&ui.update.get())).placement(Placement::Right))
                .child(
                    canban_ui_kit::press_target()
                        .w_full()
                        .h_full()
                        .items_center()
                        .justify_center()
                        .rounded(px(RADIUS))
                        .bg(ACCENT_FILL)
                        .hover(|s| s.opacity(0.88))
                        .focus(|s| s.ring(px(2.)).ring_color(ACCENT))
                        .disabled(move || busy(ui))
                        .accessibility_label(move || Some(hint(&ui.update.get()).into()))
                        .child(dynamic(move || {
                            let v = ui.update.get();
                            if v["state"]["phase"] == "downloading" {
                                label(format!("{}%", v["state"]["progress"].as_u64().unwrap_or(0)))
                                    .text_size(px(10.))
                                    .text_color(ON_ACCENT)
                                    .into_element()
                            } else {
                                icon_tinted(Icon::Update, ON_ACCENT).into_element()
                            }
                        }))
                        .on_press(move |_: &()| {
                            if ui.update.get()["state"]["phase"] == "manual_recovery" {
                                ui.open_settings_section("updates");
                            } else {
                                install(ui);
                            }
                        }),
                )
        },
    )
}
pub fn settings(ui: Ui) -> impl IntoElement {
    settings_section("Updates", "Published desktop releases.")
        .trailing(dynamic(move || {
            let value = ui.update.get();
            if value["disabled"] == true {
                return status("Dev instance", Tone::Neutral).into_element();
            }
            if let Some(ver) = value["state"]["available"]["manifest"]["version"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                return status(format!("Update · {ver}"), Tone::Info).into_element();
            }
            match value["state"]["phase"].as_str() {
                Some("current") => status("Up to date", Tone::Success).into_element(),
                Some("manual_recovery") => {
                    status("Recovery needed", Tone::Attention).into_element()
                }
                Some("checking") => status("Checking…", Tone::Neutral).into_element(),
                _ => badge(
                    value["build"]["version"]
                        .as_str()
                        .unwrap_or("development")
                        .to_owned(),
                )
                .into_element(),
            }
        }))
        .child(when(
            move || ui.update.get()["disabled"] == true,
            || muted("Channel updates are disabled for this development instance. Rebuild with mise run dev."),
        ))
        .child(when(
            move || {
                matches!(
                    ui.update.get()["state"]["phase"].as_str(),
                    Some("checking" | "downloading" | "installing")
                )
            },
            move || {
                muted(move || {
                    let value = ui.update.get();
                    match value["state"]["phase"].as_str().unwrap_or("") {
                        "checking" => "Checking for updates…".into(),
                        _ => hint(&value),
                    }
                })
            },
        ))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_3()
                .child(settings_field(
                    "Current version",
                    "",
                    label(move || {
                        ui.update.get()["build"]["version"]
                            .as_str()
                            .unwrap_or("development")
                            .to_owned()
                    })
                    .font_family("Menlo")
                    .text_size(px(12.)),
                ))
                .child(
                    action("Check for updates", Variant::Secondary, move |_| check(ui))
                        .disabled(move || busy(ui) || ui.update.get()["disabled"] == true),
                ),
        )
        .child(when(
            move || ui.update.get()["disabled"] != true && !ui.update.get()["state"]["available"].is_null(),
            move || {
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded(px(RADIUS))
                    .bg(INSET)
                    .child(label(move || {
                        format!(
                            "Available · {}",
                            ui.update.get()["state"]["available"]["manifest"]["version"]
                                .as_str()
                                .unwrap_or("")
                        )
                    }))
                    .child(
                        action("Install and restart", Variant::Primary, move |_| {
                            install(ui)
                        })
                        .disabled(move || busy(ui)),
                    )
            },
        ))
        .child(when(
            move || ui.update.get()["state"]["error"].is_string(),
            move || {
                notice(
                    move || {
                        ui.update.get()["state"]["error"]
                            .as_str()
                            .unwrap_or("")
                            .to_owned()
                    },
                    Tone::Attention,
                )
            },
        ))
}

/// Standalone recovery UI reads only the updater journal, never application data.
#[component]
pub fn Recovery() -> impl IntoElement {
    crate::startup::watch_quit();
    canban_ui_kit::tokens::install_theme();
    let home = canban_app_server::default_home().unwrap_or_default();
    let journal = canban_updater::handoff::journal(&home).ok();
    // Automatically recover local backend handoffs on fresh user launches, while keeping
    // worker-spawned recovery windows (`CANBAN_UPGRADE_TOKEN` set) paused to prevent loops.
    let auto_start = std::env::var_os("CANBAN_UPGRADE_TOKEN").is_none()
        && canban_app_server::instance::bundled()
            .ok()
            .flatten()
            .is_none()
        && journal.as_ref().is_some_and(|j| j.local.is_some());
    let error = journal
        .and_then(|j| j.error)
        .unwrap_or_else(|| "Inspect updater.log before retrying recovery.".into());
    recovery_screen_auto(home.to_string_lossy().into_owned(), error, auto_start)
}

pub fn recovery_screen(home: String, error: String) -> impl IntoElement {
    recovery_screen_auto(home, error, false)
}

pub fn recovery_screen_auto(home: String, error: String, auto_start: bool) -> impl IntoElement {
    let directory = std::path::PathBuf::from(&home).join("updates");
    let home_path = std::path::PathBuf::from(&home);
    let retrying = signal(auto_start);
    let retry_error = signal(String::new());
    let run_recover = std::rc::Rc::new(move || {
        retrying.set(true);
        retry_error.set(String::new());
        let target_home = home_path.clone();
        let ui_pid = std::process::id();
        spawn(async move {
            let res = blocking(move || {
                let bundled = std::env::current_exe()
                    .ok()
                    .map(|p| p.with_file_name("canban-updater"));
                canban_updater::handoff::recover(&target_home, bundled.as_deref(), Some(ui_pid))
            })
            .await;
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    retry_error.set(format!("{e:#}"));
                    retrying.set(false);
                }
                Err(e) => {
                    retry_error.set(e.to_string());
                    retrying.set(false);
                }
            }
        });
    });
    if auto_start {
        let auto_recover = run_recover.clone();
        set_timeout(std::time::Duration::from_millis(10), move || {
            auto_recover();
        });
    }
    view()
        .flex_1()
        .bg(BACKGROUND)
        .p_5()
        .justify_center()
        .items_center()
        .child(
            panel()
                .w_full()
                .max_w(px(680.))
                .gap_4()
                .child(heading(move || {
                    if retrying.get() {
                        "Recovering local backend…"
                    } else {
                        "Update needs manual recovery"
                    }
                }))
                .child(muted(move || {
                    if retrying.get() {
                        "Restoring and activating the backend included in this App. Your workspace will open automatically."
                    } else {
                        "Automatic recovery stopped. The database and App copies have been preserved."
                    }
                }))
                .child(notice(error, Tone::Attention))
                .child(when(
                    move || !retry_error.get().is_empty(),
                    move || notice(move || retry_error.get(), Tone::Danger),
                ))
                .child(label(format!("Details: {home}/updates/updater.log")))
                .child(muted(
                    "Click Retry recovery to restore or finish activating this App, or inspect the update folder first.",
                ))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            action(
                                move || {
                                    if retrying.get() {
                                        "Recovering…"
                                    } else {
                                        "Retry recovery"
                                    }
                                },
                                Variant::Primary,
                                move |_| {
                                    if retrying.get_untracked() {
                                        return;
                                    }
                                    run_recover();
                                },
                            )
                            .disabled(move || retrying.get()),
                        )
                        .child(action("Open update folder", Variant::Secondary, move |_| {
                            let path = directory.clone();
                            spawn(async move {
                                let _ = blocking(move || {
                                    std::process::Command::new("/usr/bin/open")
                                        .arg(path)
                                        .status()
                                })
                                .await;
                            });
                        }))
                        .child(action("Quit", Variant::Quiet, move |_| {
                            nsh_core::element::desktop::quit()
                        })),
                ),
        )
}
