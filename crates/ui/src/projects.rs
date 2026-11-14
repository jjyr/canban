//! Project creation: directory first, automatic name until explicitly edited.
pub(crate) mod selection;
use crate::{machines, presenter::*};
use canban_ui_kit::*;
use nsh_core::prelude::*;
use serde_json::json;

pub fn new_project(ui: Ui) -> impl IntoElement {
    let name = signal(String::new());
    let custom_name = signal(false);
    let directory = signal(String::new());
    let machine = signal("local".to_owned());
    let expanded = signal(false);
    let agents_expanded = signal(false);
    let agent_overrides = signal(std::collections::BTreeMap::<
        String,
        canban_types::ProjectAgentOverride,
    >::new());
    effect(move || {
        let path = directory.get();
        if !custom_name.get_untracked() {
            let suggestion = std::path::Path::new(path.trim())
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            name.set(suggestion);
        }
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
                                dispatch(
                                    ui,
                                    "project.create",
                                    json!({
                                        "name": name.get_untracked().trim(),
                                        "location": {
                                            "machine": machine.get_untracked(),
                                            "directory": directory.get_untracked().trim(),
                                        },
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
