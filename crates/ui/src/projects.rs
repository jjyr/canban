//! Project creation: directory first, automatic name until explicitly edited.
pub(crate) mod selection;
use crate::{machines, presenter::*};
use canban_types::{DEFAULT_WORKFLOW_TEMPLATE_ID, DetectedGitHub, Snapshot, WorkflowTemplateFile};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

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
    let machine = signal("local".to_owned());
    let expanded = signal(false);
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
        let host = if canban_host == "local" {
            machine.get()
        } else {
            "local".to_owned()
        };
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
            let exec_machine = host.clone();
            let target_path = trimmed.clone();
            let result = blocking(move || {
                crate::routed_call(
                    "project.detect_github",
                    json!({
                        "host_id": req_host,
                        "location": {
                            "machine": exec_machine,
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
                .child(view().gap_2().child(label("Directory")).child(
                    machines::directory_picker_for_host(ui, target_host, machine, directory),
                ))
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
                        .open(expanded)
                        .reveal(true)
                        .child(
                            action(
                                move || {
                                    let arrow = if expanded.get() { "▾" } else { "▸" };
                                    if machine.get() == "local" {
                                        format!("{arrow} Execution machine")
                                    } else {
                                        format!(
                                            "{arrow} Execution machine · {}",
                                            machines::name(ui, &machine.get())
                                        )
                                    }
                                },
                                Variant::Quiet,
                                move |_| expanded.update(|v| *v = !*v),
                            )
                            .self_start()
                            .accessibility_label(move || {
                                Some(
                                    format!(
                                        "{} execution machine{}",
                                        if expanded.get() { "Collapse" } else { "Expand" },
                                        if machine.get() == "local" {
                                            String::new()
                                        } else {
                                            format!(" · {}", machines::name(ui, &machine.get()))
                                        }
                                    )
                                    .into(),
                                )
                            }),
                        )
                        .content(
                            view()
                                .pt_2()
                                .child(machines::picker(ui, machine, directory)),
                        ),
                )
                .child(
                    nsh_base::collapsible()
                        .open(workflow_expanded)
                        .reveal(true)
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
                        .reveal(true)
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
                                machine,
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
                                let exec_machine = if chosen_host == "local" {
                                    machine.get_untracked()
                                } else {
                                    "local".to_owned()
                                };
                                let mut payload = json!({
                                    "host_id": chosen_host,
                                    "name": name.get_untracked().trim(),
                                    "location": {
                                        "machine": exec_machine,
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
