//! Project creation and work location settings: host-routed directory browser,
//! automatic project naming until explicitly edited.
pub(crate) mod selection;
use crate::presenter::*;
use canban_types::{
    DEFAULT_WORKFLOW_TEMPLATE_ID, DetectedGitHub, Project, Snapshot, WorkLocation,
    WorkflowTemplateFile,
};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

fn fixture_listing(host_id: &str, base_dir: &str) -> PathListing {
    let home = if host_id.is_empty() || host_id == "local" {
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
    let is_local = host_id.is_empty() || host_id == "local";
    let subdirs: &[&str] = match (is_local, clean.as_str()) {
        (true, "/") => &["Applications", "Users", "opt", "tmp"],
        (true, "/Users") => &["Shared", "demo"],
        (true, "/Users/demo") => &[".canban", ".config", "Desktop", "Documents", "Workspace"],
        (true, "/Users/demo/Workspace") => &[
            ".git",
            "Machines",
            "canban",
            "canban-site",
            "herdr",
            "sheprd",
        ],
        (false, "/") => &["home", "opt", "srv", "tmp", "var"],
        (false, "/home") => &["builder"],
        (false, "/home/builder") => &[".cache", ".ssh", "projects", "workspaces"],
        (false, "/home/builder/workspaces") => &["canban", "core-service", "infra"],
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

fn browse_directory(
    ui: Ui,
    host_id: Signal<String>,
    path: String,
    listing: Signal<PathListing>,
    epoch: Signal<u64>,
) {
    let target_host = host_id.get_untracked();
    if crate::APP.get().is_none() {
        listing.set(fixture_listing(&target_host, &path));
        return;
    }
    let generation = epoch.get_untracked() + 1;
    epoch.set(generation);
    listing.update(|l| {
        l.loading = true;
        l.error.clear();
    });
    let delay_ms = if target_host.is_empty() || target_host == "local" {
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
        let req_host = target_host.clone();
        let req_path = path.clone();
        let value = blocking(move || {
            crate::routed_call(
                "directory.list",
                json!({"host_id": req_host, "machine": "local", "path": req_path}),
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

/// Inline editable path combobox with live host-routed directory filtering,
/// shared by New Project and Work location settings.
pub fn directory_picker_for_host(
    ui: Ui,
    host_id: Signal<String>,
    directory: Signal<String>,
) -> impl IntoElement {
    let default_home = if host_id.get_untracked() == "local" || host_id.get_untracked().is_empty() {
        if crate::APP.get().is_some() {
            std::env::var("HOME").unwrap_or_default()
        } else {
            "/Users/demo".to_owned()
        }
    } else {
        String::new()
    };
    let listing = signal(PathListing {
        home: default_home,
        ..Default::default()
    });
    let epoch = signal(0u64);
    let prev_host = stored(host_id.get_untracked());
    effect(move || {
        let current_host = host_id.get();
        if prev_host.replace(current_host.clone()) != current_host {
            epoch.update(|n| *n += 1);
            let next_home = if current_host == "local" || current_host.is_empty() {
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
            browse_directory(ui, host_id, base_dir, listing, epoch);
        },
        |_| {},
    )
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
    let host_id = signal(ui.active_project_host());
    let directory = signal(initial.directory.clone());
    let expected = initial.clone();
    let id = p.map(|p| p.id).unwrap_or_default();
    view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_4()
        .child(
            scroll().flex_1().min_h(px(0.)).child(
                settings_section(
                    "Work location",
                    "Changes affect new executions on this host. Resume, Review and code delivery stay on the original directory.",
                )
                .trailing(badge(move || crate::hosts::host_name(ui, &host_id.get())))
                .max_w(px(760.))
                .child(settings_field(
                    "Directory",
                    "Filter and select subdirectories inline or type an absolute/`~/` path. New directories are created on save.",
                    directory_picker_for_host(ui, host_id, directory),
                ))
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
                                            "machine": "local",
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum WorkflowChoice {
    Default,
    Builtin,
    Project(String),
}

fn project_workflow_files(snapshot: &Snapshot, project_id: &str) -> Vec<WorkflowTemplateFile> {
    let mut active: Vec<_> = snapshot
        .workflow_sources
        .iter()
        .filter(|s| s.project_id == project_id)
        .collect();
    active.sort_by(|a, b| a.filename.cmp(&b.filename));
    active
        .into_iter()
        .filter_map(|item| {
            let raw = snapshot
                .workflow_revisions
                .iter()
                .find(|r| r.id == item.revision_id)
                .and_then(|r| r.source_yaml.clone())?;
            Some(WorkflowTemplateFile {
                filename: item.filename.clone(),
                source: raw,
            })
        })
        .collect()
}

fn summarize_project_workflow(snapshot: &Snapshot, project_id: &str) -> String {
    let files = project_workflow_files(snapshot, project_id);
    if !files.is_empty() {
        return canban_workflow_engine::source::summarize_collection(&files);
    }
    if let Some(proj) = snapshot.projects.iter().find(|p| p.id == project_id)
        && !proj.workflow.stages.is_empty()
    {
        return proj
            .workflow
            .stages
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(" → ");
    }
    "Backlog → Develop → Review".into()
}

fn workflow_picker(ui: Ui, choice: Signal<WorkflowChoice>) -> impl IntoElement {
    let trigger = press_target()
        .accessibility_label("Select initial workflow")
        .self_start()
        .px_2p5()
        .py_1p5()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .hover(|s| s.bg(INSET))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .child(icon(Icon::Trigger).w(px(14.)).h(px(14.)))
                .child(
                    label(move || {
                        let cur = choice.get();
                        ui.snapshot.with(|s| {
                            let custom_default = s
                                .workflow_templates
                                .iter()
                                .find(|t| t.id == DEFAULT_WORKFLOW_TEMPLATE_ID);
                            match cur {
                                WorkflowChoice::Default => {
                                    if let Some(tpl) = custom_default {
                                        if let Some(src) = &tpl.source_project_name {
                                            format!("Custom default ({src})")
                                        } else {
                                            "Custom default".into()
                                        }
                                    } else {
                                        "Default workflow".into()
                                    }
                                }
                                WorkflowChoice::Builtin => "Built-in default".into(),
                                WorkflowChoice::Project(pid) => s
                                    .projects
                                    .iter()
                                    .find(|p| p.id == pid)
                                    .map(|p| format!("Copy from {}", p.name))
                                    .unwrap_or_else(|| "Default workflow".into()),
                            }
                        })
                    })
                    .text_size(px(12.5)),
                )
                .child(icon(Icon::ChevronDown).w(px(13.)).h(px(13.))),
        );

    view()
        .gap_1p5()
        .child(
            search_popover(
                trigger,
                move || {
                    let cur = choice.get_untracked();
                    ui.snapshot.with_untracked(|s| {
                        let mut opts = Vec::new();
                        let custom_default = s
                            .workflow_templates
                            .iter()
                            .find(|t| t.id == DEFAULT_WORKFLOW_TEMPLATE_ID);
                        if let Some(tpl) = custom_default {
                            let label_text = match &tpl.source_project_name {
                                Some(src) => format!("Custom default ({src})"),
                                None => "Custom default".into(),
                            };
                            opts.push(
                                PickerOption::new("default", label_text)
                                    .detail(canban_workflow_engine::source::summarize_collection(
                                        &tpl.sources,
                                    ))
                                    .group("Presets")
                                    .icon(Icon::Trigger)
                                    .selected(cur == WorkflowChoice::Default)
                                    .keywords(["global", "template", "saved", "default"]),
                            );
                            opts.push(
                                PickerOption::new("builtin", "Built-in default")
                                    .detail("Backlog → Develop → Review")
                                    .group("Presets")
                                    .icon(Icon::Trigger)
                                    .selected(cur == WorkflowChoice::Builtin)
                                    .keywords(["native", "standard", "reset", "builtin"]),
                            );
                        } else {
                            opts.push(
                                PickerOption::new("default", "Default workflow")
                                    .detail("Backlog → Develop → Review")
                                    .group("Presets")
                                    .icon(Icon::Trigger)
                                    .selected(matches!(
                                        cur,
                                        WorkflowChoice::Default | WorkflowChoice::Builtin
                                    ))
                                    .keywords(["builtin", "standard", "default"]),
                            );
                        }
                        for p in &s.projects {
                            opts.push(
                                PickerOption::new(format!("project:{}", p.id), p.name.clone())
                                    .detail(summarize_project_workflow(s, &p.id))
                                    .group("Copy from existing Project")
                                    .icon(Icon::Projects)
                                    .selected(
                                        matches!(&cur, WorkflowChoice::Project(pid) if pid == &p.id),
                                    )
                                    .keywords(["copy", "clone", "project", p.directory.as_str()]),
                            );
                        }
                        opts
                    })
                },
                move |selected_id| {
                    let next = if selected_id == "builtin" {
                        WorkflowChoice::Builtin
                    } else if let Some(pid) = selected_id.strip_prefix("project:") {
                        WorkflowChoice::Project(pid.to_owned())
                    } else {
                        WorkflowChoice::Default
                    };
                    choice.set(next);
                },
            )
            .placeholder("Filter workflow presets or Projects…")
            .empty_text("No matching workflow sources")
            .width(380.0),
        )
        .child(
            muted(move || {
                let cur = choice.get();
                ui.snapshot.with(|s| {
                    let custom_default = s
                        .workflow_templates
                        .iter()
                        .find(|t| t.id == DEFAULT_WORKFLOW_TEMPLATE_ID);
                    match cur {
                        WorkflowChoice::Default => {
                            if let Some(tpl) = custom_default {
                                format!(
                                    "{} · Saved as global default in Project Workflows",
                                    canban_workflow_engine::source::summarize_collection(
                                        &tpl.sources
                                    )
                                )
                            } else {
                                "Backlog → Develop → Review · Tip: click Set as default in any Project's Workflows to reuse it automatically."
                                    .into()
                            }
                        }
                        WorkflowChoice::Builtin => {
                            "Backlog → Develop → Review · Built-in automated delivery".into()
                        }
                        WorkflowChoice::Project(pid) => format!(
                            "{} · Clones stages, steps, prompts, and workspace settings",
                            summarize_project_workflow(s, &pid)
                        ),
                    }
                })
            })
            .text_size(px(11.5)),
        )
}

pub fn new_project_form(
    ui: Ui,
    initial_directory: String,
    initial_detected: Option<DetectedGitHub>,
) -> impl IntoElement {
    let name = signal(String::new());
    let custom_name = signal(false);
    let directory = signal(initial_directory);
    let target_host = signal("local".to_owned());
    let workflow_expanded = signal(false);
    let workflow_choice = signal(WorkflowChoice::Default);
    let agents_expanded = signal(false);
    let link_github = signal(true);
    let detected_github = signal(initial_detected);
    let detect_epoch = signal(0u64);
    let agent_overrides = signal(std::collections::BTreeMap::<
        String,
        canban_types::ProjectAgentOverride,
    >::new());
    effect(move || {
        let path = directory.get();
        if !custom_name.get_untracked() {
            let trimmed = path.trim();
            let suggestion = if trimmed == "~" || trimmed == "/" {
                String::new()
            } else {
                std::path::Path::new(trimmed)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            name.set(suggestion);
        }
    });
    effect(move || {
        let canban_host = target_host.get();
        let path = directory.get();
        let trimmed = path.trim().to_owned();
        let generation = detect_epoch.get_untracked() + 1;
        detect_epoch.set(generation);
        if crate::APP.get().is_none() {
            return;
        }
        if trimmed.is_empty() || !std::path::Path::new(&trimmed).is_absolute() {
            detected_github.set(None);
            return;
        }
        ui.spawn(async move {
            let req_host = canban_host.clone();
            let target_path = trimmed.clone();
            let result = blocking(move || {
                crate::routed_call(
                    "project.detect_github",
                    json!({
                        "host_id": req_host,
                        "location": {
                            "machine": "local",
                            "directory": target_path,
                        }
                    }),
                    None,
                )
            })
            .await;
            if detect_epoch.try_get() != Some(generation) {
                return;
            }
            let parsed = result
                .ok()
                .and_then(Result::ok)
                .and_then(|v| {
                    serde_json::from_value::<Option<DetectedGitHub>>(v["detected"].clone()).ok()
                })
                .flatten();
            if detected_github.try_get().flatten() != parsed {
                link_github.try_set(true);
                detected_github.try_set(parsed);
            }
        });
    });
    view().flex_1().min_h(px(0.)).p_5().gap_4().child(
        scroll().flex_1().min_h(px(0.)).child(
            view()
                .w_full()
                .max_w(px(720.))
                .gap_4()
                .child(crate::hosts::new_project_host_picker(
                    ui,
                    target_host,
                    directory,
                ))
                .child(
                    view()
                        .gap_2()
                        .child(label("Directory"))
                        .child(directory_picker_for_host(ui, target_host, directory)),
                )
                .child(
                    view().gap_2().child(label("Name")).child(
                        bound_field(
                            move || name.get(),
                            move |value| {
                                custom_name.set(true);
                                name.set(value.to_owned());
                            },
                            "Project name",
                        )
                        .label("Project name"),
                    ),
                )
                .child(dynamic(move || {
                    let Some(det) = detected_github.get() else {
                        return view().into_element();
                    };
                    view()
                        .gap_2()
                        .child(label("GitHub Repository"))
                        .child(
                            panel()
                                .p_3()
                                .gap_1()
                                .border_1()
                                .border_color(BORDER)
                                .child(
                                    view()
                                        .flex_row()
                                        .flex_wrap()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .child(checkbox(
                                            format!("Link to {}", det.repository),
                                            link_github,
                                            move |checked| link_github.set(*checked),
                                        ))
                                        .child(muted(format!("(from git `{}`)", det.remote))),
                                )
                                .child(view().pl_7().child(muted(
                                    "Issues & PR search enabled · Automatic intake defaults to Off",
                                ))),
                        )
                        .into_element()
                }))
                .child(
                    nsh_base::collapsible()
                        .open(workflow_expanded)
                        .reveal(move || !canban_ui_kit::motion::reduced())
                        .child(
                            action(
                                move || {
                                    let arrow = if workflow_expanded.get() {
                                        "▾"
                                    } else {
                                        "▸"
                                    };
                                    let cur = workflow_choice.get();
                                    let summary = ui.snapshot.with(|s| {
                                        let custom_default = s
                                            .workflow_templates
                                            .iter()
                                            .find(|t| t.id == DEFAULT_WORKFLOW_TEMPLATE_ID);
                                        match cur {
                                            WorkflowChoice::Default => {
                                                if let Some(tpl) = custom_default {
                                                    if let Some(src) = &tpl.source_project_name {
                                                        format!("Custom default ({src})")
                                                    } else {
                                                        "Custom default".into()
                                                    }
                                                } else {
                                                    "Default".into()
                                                }
                                            }
                                            WorkflowChoice::Builtin => "Built-in default".into(),
                                            WorkflowChoice::Project(pid) => s
                                                .projects
                                                .iter()
                                                .find(|p| p.id == pid)
                                                .map(|p| format!("Copy from {}", p.name))
                                                .unwrap_or_else(|| "Default".into()),
                                        }
                                    });
                                    format!("{arrow} Workflow · {summary}")
                                },
                                Variant::Quiet,
                                move |_| workflow_expanded.update(|v| *v = !*v),
                            )
                            .self_start(),
                        )
                        .content(view().pt_2().child(workflow_picker(ui, workflow_choice))),
                )
                .child(
                    nsh_base::collapsible()
                        .open(agents_expanded)
                        .reveal(move || !canban_ui_kit::motion::reduced())
                        .child(
                            action(
                                move || {
                                    let arrow = if agents_expanded.get() { "▾" } else { "▸" };
                                    let count = agent_overrides
                                        .with(|m| m.values().filter(|ov| !ov.is_empty()).count());
                                    if count == 0 {
                                        format!("{arrow} Project Agents · Global defaults")
                                    } else {
                                        format!(
                                            "{arrow} Project Agents · {count} role override{}",
                                            if count == 1 { "" } else { "s" }
                                        )
                                    }
                                },
                                Variant::Quiet,
                                move |_| agents_expanded.update(|v| *v = !*v),
                            )
                            .self_start(),
                        )
                        .content(view().pt_2().child(
                            crate::agents_setup::project_overrides_editor(
                                ui,
                                agent_overrides,
                                target_host,
                            ),
                        )),
                )
                .child(
                    view()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .child(action("Cancel", Variant::Quiet, move |_| {
                            ui.navigate("board")
                        }))
                        .child(
                            action("Create Project", Variant::Primary, move |_| {
                                let auto_link = detected_github
                                    .get_untracked()
                                    .is_none_or(|_| link_github.get_untracked());
                                let chosen_host = target_host.get_untracked();
                                let mut payload = json!({
                                    "host_id": chosen_host,
                                    "name": name.get_untracked().trim(),
                                    "location": {
                                        "machine": "local",
                                        "directory": directory.get_untracked().trim(),
                                    },
                                    "github_auto_link": auto_link,
                                    "agent_overrides": agent_overrides.get_untracked(),
                                });
                                match workflow_choice.get_untracked() {
                                    WorkflowChoice::Default => {
                                        if chosen_host != "local" {
                                            let custom_sources = ui.snapshot.with_untracked(|s| {
                                                s.workflow_templates
                                                    .iter()
                                                    .find(|t| t.id == DEFAULT_WORKFLOW_TEMPLATE_ID)
                                                    .map(|t| t.sources.clone())
                                                    .filter(|v| !v.is_empty())
                                            });
                                            if let Some(files) = custom_sources {
                                                payload["workflow_sources"] = json!(files);
                                            }
                                        }
                                    }
                                    WorkflowChoice::Builtin => {
                                        payload["workflow_mode"] = json!("builtin");
                                    }
                                    WorkflowChoice::Project(pid) => {
                                        ui.snapshot.with_untracked(|s| {
                                            if s.projects.iter().any(|p| p.id == pid) {
                                                if s.host_of(&pid) == chosen_host {
                                                    payload["workflow_mode"] = json!("project");
                                                    payload["workflow_source_project_id"] =
                                                        json!(pid);
                                                } else {
                                                    let files = project_workflow_files(s, &pid);
                                                    if !files.is_empty() {
                                                        payload["workflow_sources"] = json!(files);
                                                    }
                                                }
                                            }
                                        });
                                    }
                                }
                                dispatch_then(ui, "project.create", payload, move |created| {
                                    if let Some(pid) = created["id"].as_str() {
                                        ui.project.set(pid.to_owned());
                                    }
                                    ui.navigate("board");
                                });
                            })
                            .disabled(move || {
                                ui.pending.get()
                                    || name.get().trim().is_empty()
                                    || directory.get().trim().is_empty()
                            }),
                        ),
                ),
        ),
    )
}

pub fn new_project(ui: Ui) -> impl IntoElement {
    new_project_form(ui, String::new(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_core::{ApproxTextEngine, Core, MountItem, geometry::size};

    #[test]
    fn new_project_form_shows_detected_github_repository_preview_and_safe_intake_caption() {
        #[component]
        fn NewProjectDetectedProbe() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            new_project_form(
                ui,
                "/Users/jjy/Workspace/canban".into(),
                Some(DetectedGitHub {
                    repository: "jjyr/canban".into(),
                    remote: "origin".into(),
                }),
            )
        }

        let mut core = Core::new(ApproxTextEngine, size(px(900.), px(700.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<NewProjectDetectedProbe>());
        let batch = core.commit().expect("initial mount batch");
        let texts: Vec<String> = batch
            .items
            .into_iter()
            .filter_map(|item| match item {
                MountItem::Text { text, .. } => Some(text.text.to_string()),
                _ => None,
            })
            .collect();

        assert!(
            texts.iter().any(|t| t == "GitHub Repository"),
            "expected GitHub Repository section, got {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == "Link to jjyr/canban"),
            "expected checkbox title `Link to jjyr/canban`, got {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == "(from git `origin`)"),
            "expected remote provenance `(from git `origin`)`, got {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t == "Issues & PR search enabled · Automatic intake defaults to Off"),
            "expected safe default intake caption, got {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == "▸ Workflow · Default"),
            "expected default Workflow collapsible summary, got {texts:?}"
        );
    }

    #[test]
    fn new_project_form_reflects_saved_custom_default_workflow() {
        #[component]
        fn NewProjectCustomDefaultProbe() -> impl IntoElement {
            canban_ui_kit::tokens::install_theme();
            let ui = Ui::new();
            let default_raw = canban_workflow_engine::source::encode(
                &canban_workflow_engine::source::default_document(
                    "delivery:proj-1",
                    canban_types::WorkspaceSettings::default(),
                ),
            )
            .unwrap();
            ui.snapshot.update(|s| {
                s.workflow_templates.push(canban_types::WorkflowTemplate {
                    id: DEFAULT_WORKFLOW_TEMPLATE_ID.into(),
                    name: "Default".into(),
                    source_project_name: Some("Canban".into()),
                    sources: vec![WorkflowTemplateFile {
                        filename: "development.yaml".into(),
                        source: default_raw,
                    }],
                    updated_at: 1,
                });
            });
            new_project_form(ui, "/Users/jjy/Workspace/demo".into(), None)
        }

        let mut core = Core::new(ApproxTextEngine, size(px(900.), px(700.)));
        core.set_components(nsh_base::registry());
        core.mount_root(&nsh_core::element::Root::of::<NewProjectCustomDefaultProbe>());
        let batch = core.commit().expect("initial mount batch");
        let texts: Vec<String> = batch
            .items
            .into_iter()
            .filter_map(|item| match item {
                MountItem::Text { text, .. } => Some(text.text.to_string()),
                _ => None,
            })
            .collect();
        assert!(
            texts
                .iter()
                .any(|t| t == "▸ Workflow · Custom default (Canban)"),
            "expected custom default Workflow collapsible summary, got {texts:?}"
        );
    }
}
