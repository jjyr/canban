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
    view()
        .flex_1()
        .min_h(px(0.))
        .p_5()
        .gap_4()
        .child(subpage_header("Back to Board", "New Project", move |_| {
            ui.navigate("board")
        }))
        .child(scroll().flex_1().min_h(px(0.)).child(
            view()
                .w_full()
                .max_w(px(720.))
                .gap_4()
                .child(view().gap_2().child(label("Directory"))
                    .child(machines::directory_picker(ui, machine, directory)))
                .child(view().gap_2().child(label("Name"))
                    .child(bound_field(move || name.get(), move |value| {
                        custom_name.set(true);
                        name.set(value.to_owned());
                    }, "Project name").label("Project name")))
                .child(nsh_base::collapsible()
                    .open(expanded)
                    .reveal(true)
                    .child(action(move || {
                        let arrow = if expanded.get() { "▾" } else { "▸" };
                        if machine.get() == "local" {
                            format!("{arrow} Execution machine")
                        } else {
                            format!("{arrow} Execution machine · {}", machines::name(ui, &machine.get()))
                        }
                    }, Variant::Quiet, move |_| expanded.update(|v| *v = !*v))
                        .self_start()
                        .accessibility_label(move || Some(format!("{} execution machine{}", if expanded.get() { "Collapse" } else { "Expand" }, if machine.get() == "local" { String::new() } else { format!(" · {}", machines::name(ui, &machine.get())) }).into())))
                    .content(view().pt_2().child(machines::picker(ui, machine, directory))))
                .child(view().flex_row().justify_end().gap_2()
                    .child(action("Cancel", Variant::Quiet, move |_| ui.navigate("board")))
                    .child(action("Create Project", Variant::Primary, move |_| {
                        dispatch(ui, "project.create", json!({"name":name.get_untracked().trim(),"location":{"machine":machine.get_untracked(),"directory":directory.get_untracked().trim()}}));
                    }).disabled(move || ui.pending.get() || name.get().trim().is_empty() || directory.get().trim().is_empty())))
        ))
}
