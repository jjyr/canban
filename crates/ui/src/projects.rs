//! Project creation: directory first, automatic name until explicitly edited.
pub(crate) mod selection;
use crate::{machines, presenter::*};
use canban_types::DetectedGitHub;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

pub fn new_project_form(
    ui: Ui,
    initial_directory: String,
    initial_detected: Option<DetectedGitHub>,
) -> impl IntoElement {
    let name = signal(String::new());
    let custom_name = signal(false);
    let directory = signal(initial_directory);
    let machine = signal("local".to_owned());
    let expanded = signal(false);
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
        let host = machine.get();
        let path = directory.get();
        let trimmed = path.trim().to_owned();
        let generation = detect_epoch.get_untracked() + 1;
        detect_epoch.set(generation);
        let Some(app) = crate::APP.get().cloned() else {
            return;
        };
        if trimmed.is_empty() || !std::path::Path::new(&trimmed).is_absolute() {
            detected_github.set(None);
            return;
        }
        ui.spawn(async move {
            let target_host = host.clone();
            let target_path = trimmed.clone();
            let result = blocking(move || {
                app.call(
                    "project.detect_github",
                    json!({
                        "location": {
                            "machine": target_host,
                            "directory": target_path,
                        }
                    }),
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
                .child(
                    view()
                        .gap_2()
                        .child(label("Directory"))
                        .child(machines::directory_picker(ui, machine, directory)),
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
                                dispatch(
                                    ui,
                                    "project.create",
                                    json!({
                                        "name": name.get_untracked().trim(),
                                        "location": {
                                            "machine": machine.get_untracked(),
                                            "directory": directory.get_untracked().trim(),
                                        },
                                        "github_auto_link": auto_link,
                                        "agent_overrides": agent_overrides.get_untracked(),
                                    }),
                                );
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
    }
}
