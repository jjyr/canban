//! Shared machine picker, server-side directory browser and Project location form.
use crate::presenter::*;
use canban_types::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;
pub fn refresh_catalog(ui: Ui) {
    if ui.machines_loading.get_untracked() {
        return;
    }
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    ui.machines_loading.set(true);
    ui.spawn(async move {
        let result = blocking(move || app.call("machine.list", json!({}))).await;
        if !ui.machines_loading.try_set(false) {
            return;
        }
        match result {
            Ok(Ok(value)) => {
                ui.machines.try_set(value);
            }
            Ok(Err(e)) => {
                ui.machines
                    .try_set(json!({"machines":[],"error":format!("{e:#}")}));
            }
            Err(e) => {
                ui.machines
                    .try_set(json!({"machines":[],"error":e.to_string()}));
            }
        }
    });
}
pub fn name(ui: Ui, id: &str) -> String {
    if id == "local" {
        return "AppServer host".into();
    }
    ui.machines
        .with(|v| {
            v["machines"]
                .as_array()
                .and_then(|ms| ms.iter().find(|m| m["id"] == id))
                .and_then(|m| m["name"].as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| id.chars().take(8).collect())
}
pub fn picker(ui: Ui, machine: Signal<String>, directory: Signal<String>) -> impl IntoElement {
    use nsh_core::element::desktop::{MenuItem, popup_menu};
    dynamic(move || {
        action(
            format!("{} ▾", name(ui, &machine.get())),
            Variant::Secondary,
            move |_| {
                ui.spawn(async move {
                    let mut rows = vec![("local".to_owned(), "AppServer host".to_owned())];
                    rows.extend(
                        ui.machines
                            .with_untracked(|v| {
                                v["machines"].as_array().cloned().unwrap_or_default()
                            })
                            .iter()
                            .filter_map(|m| {
                                Some((
                                    m["id"].as_str()?.into(),
                                    format!("{} · {}", m["name"].as_str()?, m["state"].as_str()?),
                                ))
                            }),
                    );
                    let mut items = Vec::new();
                    for (id, label) in rows {
                        items.push(MenuItem::new(label).on_select(move || {
                            if machine.try_get().is_some_and(|old| old != id) {
                                machine.try_set(id.clone());
                                directory.try_set(String::new());
                            }
                        }));
                    }
                    items.push(MenuItem::separator());
                    items.push(
                        MenuItem::new("Manage machines…")
                            .on_select(move || ui.navigate("machines")),
                    );
                    popup_menu(items).await;
                });
            },
        )
        .self_start()
    })
}
fn fixture_listing(machine: &str, base_dir: &str) -> PathListing {
    let home = if machine == "local" {
        "/Users/demo".to_owned()
    } else {
        "/home/builder".to_owned()
    };
    let resolved = if base_dir.is_empty() {
        home.clone()
    } else {
        expand_tilde(base_dir, &home)
    };
    let clean = if resolved.len() > 1 {
        resolved.trim_end_matches('/').to_owned()
    } else {
        resolved
    };
    let subdirs: &[&str] = match (machine, clean.as_str()) {
        ("local", "/") => &["Applications", "Users", "opt", "tmp"],
        ("local", "/Users") => &["Shared", "demo"],
        ("local", "/Users/demo") => &[".canban", ".config", "Desktop", "Documents", "Workspace"],
        ("local", "/Users/demo/Workspace") => &[
            ".git",
            "Machines",
            "canban",
            "canban-site",
            "herdr",
            "sheprd",
        ],
        (_, "/") => &["home", "opt", "srv", "tmp", "var"],
        (_, "/home") => &["builder"],
        (_, "/home/builder") => &[".cache", ".ssh", "projects", "workspaces"],
        (_, "/home/builder/workspaces") => &["canban", "core-service", "infra"],
        _ => &[],
    };
    let exists = !subdirs.is_empty()
        || matches!(
            clean.as_str(),
            "/Users/demo/Workspace/canban"
                | "/Users/demo/Workspace/canban-site"
                | "/Users/demo/Workspace/herdr"
                | "/Users/demo/Workspace/sheprd"
                | "/Users/demo/Workspace/Machines"
                | "/home/builder/workspaces/canban"
                | "/home/builder/workspaces/core-service"
                | "/home/builder/workspaces/infra"
        );
    let entries = subdirs
        .iter()
        .map(|name| {
            let full = if clean == "/" {
                format!("/{name}")
            } else {
                format!("{clean}/{name}")
            };
            PathEntry::new(*name, full)
        })
        .collect();
    PathListing {
        base_dir: clean,
        home,
        exists,
        entries,
        loading: false,
        error: String::new(),
    }
}

fn browse(
    ui: Ui,
    host_id: Signal<String>,
    machine: Signal<String>,
    path: String,
    listing: Signal<PathListing>,
    epoch: Signal<u64>,
) {
    let target_host = host_id.get_untracked();
    let host = if target_host == "local" {
        machine.get_untracked()
    } else {
        "local".to_owned()
    };
    if crate::APP.get().is_none() {
        listing.set(fixture_listing(&host, &path));
        return;
    }
    let generation = epoch.get_untracked() + 1;
    epoch.set(generation);
    listing.update(|l| {
        l.loading = true;
        l.error.clear();
    });
    let delay_ms = if target_host == "local" && host == "local" {
        0
    } else {
        120
    };
    ui.spawn(async move {
        if delay_ms > 0 {
            timer(std::time::Duration::from_millis(delay_ms)).await;
            if epoch.try_get() != Some(generation)
                || host_id.try_get().as_deref() != Some(target_host.as_str())
            {
                return;
            }
        }
        let target = host.clone();
        let req_host = target_host.clone();
        let req_path = path.clone();
        let value = blocking(move || {
            crate::routed_call(
                "directory.list",
                json!({"host_id": req_host, "machine": target, "path": req_path}),
                None,
            )
        })
        .await;
        if epoch.try_get() != Some(generation)
            || host_id.try_get().as_deref() != Some(target_host.as_str())
        {
            return;
        }
        match value {
            Ok(Ok(v)) => {
                let resolved_base = v["path"].as_str().unwrap_or(&path).to_owned();
                let home = v["home"].as_str().unwrap_or("").to_owned();
                let exists = v["exists"].as_bool().unwrap_or(true);
                let entries = v["directories"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|row| {
                                Some(PathEntry::new(row["name"].as_str()?, row["path"].as_str()?))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                listing.try_set(PathListing {
                    base_dir: resolved_base,
                    home,
                    exists,
                    entries,
                    loading: false,
                    error: String::new(),
                });
            }
            Ok(Err(e)) => {
                let msg = format!("{e:#}");
                listing.update(|l| {
                    l.loading = false;
                    l.error = msg;
                });
            }
            Err(e) => {
                let msg = e.to_string();
                listing.update(|l| {
                    l.loading = false;
                    l.error = msg;
                });
            }
        }
    });
}
/// Inline editable path combobox with live machine-aware directory filtering,
/// shared by New Project and Work location settings.
pub fn directory_picker(
    ui: Ui,
    machine: Signal<String>,
    directory: Signal<String>,
) -> impl IntoElement {
    directory_picker_for_host(ui, signal("local".to_owned()), machine, directory)
}

pub fn directory_picker_for_host(
    ui: Ui,
    host_id: Signal<String>,
    machine: Signal<String>,
    directory: Signal<String>,
) -> impl IntoElement {
    let default_home = if crate::APP.get().is_some() {
        std::env::var("HOME").unwrap_or_default()
    } else {
        "/Users/demo".to_owned()
    };
    let listing = signal(PathListing {
        home: default_home,
        ..Default::default()
    });
    let epoch = signal(0u64);
    let prev_key = stored((host_id.get_untracked(), machine.get_untracked()));
    effect(move || {
        let current_key = (host_id.get(), machine.get());
        if prev_key.replace(current_key.clone()) != current_key {
            epoch.update(|n| *n += 1);
            let next_home = if current_key.0 == "local" && current_key.1 == "local" {
                if crate::APP.get().is_some() {
                    std::env::var("HOME").unwrap_or_default()
                } else {
                    "/Users/demo".to_owned()
                }
            } else {
                String::new()
            };
            listing.set(PathListing {
                home: next_home,
                ..Default::default()
            });
        }
    });
    path_combobox(
        directory,
        move || listing.get(),
        move |base_dir| {
            browse(ui, host_id, machine, base_dir, listing, epoch);
        },
        |_| {},
    )
}
pub fn location_fields(
    ui: Ui,
    machine: Signal<String>,
    directory: Signal<String>,
) -> impl IntoElement {
    view()
        .min_w(px(0.))
        .gap_3p5()
        .child(settings_field(
            "Execution machine",
            "New work uses this machine. Existing Agents retain their original environment.",
            picker(ui, machine, directory),
        ))
        .child(settings_field(
            "Directory",
            "Filter and select subdirectories inline or type an absolute/`~/` path. New directories are created on save.",
            directory_picker(ui, machine, directory),
        ))
}

pub fn project_location(ui: Ui) -> impl IntoElement {
    let p = ui.current_project();
    let initial = p
        .as_ref()
        .map(Project::work_location)
        .unwrap_or(WorkLocation {
            machine: "local".into(),
            directory: String::new(),
        });
    let machine = signal(initial.machine.clone());
    let directory = signal(initial.directory.clone());
    let title = p
        .as_ref()
        .map(|p| format!("{} / Work location", p.name))
        .unwrap_or_else(|| "Work location".into());
    let expected = initial.clone();
    let id = p.map(|p| p.id).unwrap_or_default();
    let _ = title;
    view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_4()
        .child(
            scroll().flex_1().min_h(px(0.)).child(
                settings_section(
                    "Work location",
                    "Changes affect new executions. Resume, Review and code delivery stay on the original machine and directory.",
                )
                .max_w(px(760.))
                .child(location_fields(ui, machine, directory))
                .child(
                    view()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .child(action("Cancel", Variant::Quiet, move |_| {
                            ui.navigate("board")
                        }))
                        .child(
                            action("Save work location", Variant::Primary, move |_| {
                                let id = id.clone();
                                dispatch_then(
                                    ui,
                                    "project.location",
                                    json!({
                                        "project_id": id,
                                        "expected_location": expected,
                                        "location": {
                                            "machine": machine.get_untracked(),
                                            "directory": directory.get_untracked().trim()
                                        }
                                    }),
                                    move |_| ui.navigate("board"),
                                );
                            })
                            .disabled(move || {
                                ui.is_method_pending("project.location")
                                    || directory.get().trim().is_empty()
                            }),
                        ),
                ),
            ),
        )
}
pub fn settings(ui: Ui) -> impl IntoElement {
    let default_machine = signal(
        crate::APP
            .get()
            .map(|a| a.config().default_machine)
            .unwrap_or_else(|| "local".into()),
    );
    let unused = signal(String::new());
    let target = signal(String::new());
    let alias = signal(String::new());
    let session = signal(String::new());
    let remove = signal(String::new());
    let add_open = signal(ui.page.get_untracked() == "machines");

    settings_section(
        "Machines",
        "Connections use AppServer's SSH configuration. Remote programs and history remain when a connection is removed.",
    )
    .trailing(compact_action(
        "Refresh machines",
        Variant::Quiet,
        move |_| refresh_catalog(ui),
    ))
    .child(dynamic(move || {
        let v = ui.machines.get();
        let mut list = view().min_w(px(0.)).gap_1p5();
        if let Some(error) = v["error"].as_str() {
            list = list.child(notice(error.to_owned(), Tone::Attention));
        }
        let local_refs = ui.snapshot.with(|s| {
            s.projects
                .iter()
                .filter(|p| p.work_location().machine == "local")
                .count()
        });
        list = list.child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .py_2()
                .px_2p5()
                .rounded(px(RADIUS))
                .bg(INSET)
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .min_w(px(0.))
                        .child(label("AppServer host").font_weight(FontWeight::MEDIUM))
                        .child(status("Local", Tone::Success))
                        .child(muted(format!(
                            "Local execution · {local_refs} Project{}",
                            if local_refs == 1 { "" } else { "s" }
                        ))),
                ),
        );
        for row in v["machines"].as_array().cloned().unwrap_or_default() {
            let id = row["id"].as_str().unwrap_or("").to_owned();
            let confirmed = remove.get() == id;
            let machine_name = row["name"].as_str().unwrap_or("Machine").to_owned();
            let state_str = row["state"].as_str().unwrap_or("unknown").to_owned();
            let state_tone = match state_str.as_str() {
                "connected" => Tone::Success,
                "unreachable" | "error" => Tone::Attention,
                _ => Tone::Neutral,
            };
            let choose = id.clone();
            let refs = ui.snapshot.with(|s| {
                s.projects
                    .iter()
                    .filter(|p| p.work_location().machine == id)
                    .count()
            });
            list = list.child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .py_2()
                    .px_2p5()
                    .rounded(px(RADIUS))
                    .bg(INSET)
                    .child(
                        view()
                            .min_w(px(0.))
                            .flex_1()
                            .gap_0p5()
                            .child(
                                view()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .child(label(machine_name).font_weight(FontWeight::MEDIUM))
                                    .child(status(state_str, state_tone)),
                            )
                            .child(muted(format!(
                                "{} · {refs} Project reference{}",
                                row["target"].as_str().unwrap_or(""),
                                if refs == 1 { "" } else { "s" }
                            ))),
                    )
                    .child(compact_action(
                        if confirmed {
                            "Confirm disconnect · retain programs"
                        } else {
                            "Disconnect…"
                        },
                        if confirmed {
                            Variant::Danger
                        } else {
                            Variant::Quiet
                        },
                        move |_| {
                            if confirmed {
                                dispatch_then(
                                    ui,
                                    "machine.remove",
                                    json!({"machine":choose,"confirmed":true}),
                                    move |_| refresh_catalog(ui),
                                );
                                remove.set(String::new());
                            } else {
                                remove.set(choose.clone());
                            }
                        },
                    )),
            );
        }
        list
    }))
    .child(settings_field(
        "Default Terminal machine",
        "Used when no Terminal is selected. An unavailable target never falls back silently.",
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(picker(ui, default_machine, unused))
            .child(action(
                "Save default",
                Variant::Secondary,
                move |_| {
                    dispatch(
                        ui,
                        "machine.default",
                        json!({"machine":default_machine.get_untracked()}),
                    )
                },
            )),
    ))
    .child(dynamic(move || {
        let is_open = add_open.get();
        collapsible_section_group(
            "Add an SSH machine",
            None,
            is_open,
            move |_| add_open.update(|v| *v = !*v),
            view()
                .gap_2p5()
                .pt_1()
                .child(muted(
                    "Login, host-key and installation prompts open in a setup Terminal on the AppServer host.",
                ))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_2p5()
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(200.))
                                .gap_1()
                                .child(muted("Display name"))
                                .child(field(alias, "Optional alias").label("Machine name")),
                        )
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(220.))
                                .gap_1()
                                .child(muted("SSH target"))
                                .child(field(target, "user@host").label("SSH target")),
                        )
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(200.))
                                .gap_1()
                                .child(muted("Remote Core session"))
                                .child(
                                    field(session, "Optional session").label("Remote Core session"),
                                ),
                        ),
                )
                .child(
                    action("Connect machine…", Variant::Secondary, move |_| {
                        dispatch_then(
                            ui,
                            "machine.add",
                            json!({
                                "name": alias.get_untracked(),
                                "target": target.get_untracked().trim(),
                                "session": session.get_untracked()
                            }),
                            move |v| {
                                if let Some(id) = v["id"].as_str() {
                                    ui.open_terminal(id.into());
                                }
                            },
                        )
                    })
                    .self_start()
                    .disabled(move || {
                        ui.is_method_pending("machine.add") || target.get().trim().is_empty()
                    }),
                ),
        )
        .into_element()
    }))
}
