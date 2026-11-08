//! Independent Terminal workspace: split window chrome, numbered tabs and one viewer.
pub mod entries;
mod labels;
pub mod model;
mod tabs;
mod tree;
mod viewer;
use crate::presenter::*;
use canban_notifications::OverviewRow;
use canban_ui_kit::{tokens::*, *};
pub use entries::{Entry, entries};
use nsh_core::prelude::*;
use serde_json::json;

pub fn selected(ui: Ui) -> Option<OverviewRow> {
    model::conversations(&ui.snapshot.get())
        .into_iter()
        .find(|r| r.id == ui.terminal_selection.get())
}
pub fn current(ui: Ui) -> Option<Entry> {
    entries(&ui.snapshot.get())
        .into_iter()
        .find(|r| r.saved.id == ui.terminal_selection.get())
}
pub fn restore_selection(ui: Ui) {
    if !ui.terminal_selection.get_untracked().is_empty() {
        return;
    }
    let rows = entries(&ui.snapshot.get_untracked());
    if let Some(entry) = ui
        .terminal_tabs
        .with_untracked(|ts| {
            ts.last()
                .and_then(|id| rows.iter().find(|e| &e.saved.id == id))
                .cloned()
        })
        .or_else(|| rows.into_iter().find(|e| !e.ended()))
    {
        ui.terminal_selection.set(entry.saved.id.clone());
        ui.terminal_tabs.update(|tabs| {
            if !tabs.contains(&entry.saved.id) {
                tabs.push(entry.saved.id.clone());
            }
        });
    }
}
pub fn reveal(ui: Ui, id: &str) {
    reveal_group(ui, id);
    if ui.narrow.get_untracked() {
        ui.terminal_sidebar.set(false);
    }
}
pub fn reveal_group(ui: Ui, id: &str) {
    if let Some(row) = entries(&ui.snapshot.get_untracked())
        .into_iter()
        .find(|r| r.saved.id == id)
    {
        if row.ended() {
            ui.terminal_closed.set(true);
        }
        ui.terminal_folds.update(|folds| {
            folds.remove(&format!("project:{}", row.project_id.unwrap_or_default()));
            if let Some(card) = row.card.map(|c| c.id) {
                folds.remove(&format!("card:{card}"));
            }
        });
    }
}
pub fn open_request(ui: Ui, request_id: &str) {
    let key = ui.snapshot.with_untracked(|s| {
        s.requests
            .iter()
            .find(|r| r.id == request_id)
            .map(|r| canban_types::Conversation::key(&r.card_id, &r.profile.id))
    });
    if let Some(key) = key {
        ui.open_terminal(key);
    } else {
        ui.operation_error
            .set("This Agent execution is no longer available".into());
    }
}
pub fn create(ui: Ui, project: String, card: Option<String>) {
    if card.as_ref().is_some_and(|id| {
        ui.snapshot.with_untracked(|s| {
            s.cards
                .iter()
                .any(|c| &c.id == id && c.archived_at.is_some())
        })
    }) {
        ui.operation_error
            .set("Restore this Card before opening a new terminal.".into());
        return;
    }
    launch(ui, json!({"project_id":project,"card_id":card}));
}
fn launch(ui: Ui, params: serde_json::Value) {
    let source = ui.location();
    let selection = ui.terminal_selection.get_untracked();
    if crate::APP.get().is_none() {
        let id = format!("gallery-shell-{}", canban_types::id());
        let saved = canban_types::TerminalEntry {
            id: id.clone(),
            conversation_id: None,
            number: ui
                .snapshot
                .with_untracked(|s| s.terminals.iter().map(|t| t.number).max().unwrap_or(8))
                + 1,
            name: None,
            manual_number: Some(
                ui.snapshot.with_untracked(|s| {
                    s.terminals
                        .iter()
                        .filter_map(|t| t.manual_number)
                        .max()
                        .unwrap_or(0)
                }) + 1,
            ),
            created_at: 0,
            runtime: Some(canban_types::TerminalRuntime {
                core_id: Some("gallery".into()),
                terminal: Some(id.clone()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
            }),
        };
        let mut location = current(ui).and_then(|e| e.location);
        ui.snapshot.update(|s| {
            s.terminals.push(saved.clone());
            if let Some(ref mut c) = location {
                c.terminal_id = id.clone();
                c.core_id = "gallery".into();
                c.terminal = id.clone();
                s.terminal_contexts.push(c.clone());
            }
        });
        ui.open_terminal(id);
        return;
    }
    dispatch_then(ui, "terminal.create", params, move |v| {
        if ui.location() == source
            && ui.terminal_selection.get_untracked() == selection
            && let Some(id) = v["id"].as_str()
        {
            ui.open_terminal(id.into());
        }
    });
}
pub fn new_tab(ui: Ui) {
    launch(
        ui,
        json!({"inherit_terminal_id":ui.terminal_selection.get_untracked()}),
    );
}
pub fn reopen(ui: Ui, id: String) {
    launch(ui, json!({"inherit_terminal_id":id}));
}
pub fn close_tab(ui: Ui, id: &str) {
    ui.terminal_focus.update(|v| *v += 1);
    let mut tabs = ui.terminal_tabs.get_untracked();
    let next = tabs.iter().position(|t| t == id).and_then(|i| {
        tabs.remove(i);
        tabs.get(i.min(tabs.len().saturating_sub(1))).cloned()
    });
    ui.terminal_tabs.set(tabs);
    if ui.terminal_selection.get_untracked() == id {
        ui.terminal_selection.set(next.unwrap_or_default());
        ui.terminal_details.set(false);
        if ui.narrow.get_untracked() && ui.terminal_selection.get_untracked().is_empty() {
            ui.terminal_sidebar.set(true);
        }
    }
}
pub(crate) const TAB_KEYS: [(&str, u8); 4] = [
    ("mod+t", 0),
    ("mod+w", 1),
    ("mod+shift+]", 2),
    ("mod+shift+[", 3),
];
pub fn bindings(ui: Ui) {
    use nsh_core::keymap::When;
    effect(move || {
        if ui.page.get() != "terminals" || ui.quick_open.get() || ui.terminal_rename.get().is_some()
        {
            return;
        }
        for (key, action) in TAB_KEYS {
            on_key_with(
                key,
                KeyOptions {
                    when: When::Always,
                    ..Default::default()
                },
                move |_| {
                    match action {
                        0 => new_tab(ui),
                        1 => close_tab(ui, &ui.terminal_selection.get_untracked()),
                        _ => {
                            let tabs = ui.terminal_tabs.get_untracked();
                            if !tabs.is_empty() {
                                let old = tabs
                                    .iter()
                                    .position(|t| *t == ui.terminal_selection.get_untracked())
                                    .unwrap_or(0);
                                let next = if action == 2 {
                                    (old + 1) % tabs.len()
                                } else {
                                    (old + tabs.len() - 1) % tabs.len()
                                };
                                ui.open_terminal(tabs[next].clone());
                            }
                        }
                    }
                    ui.terminal_focus.update(|v| *v += 1);
                },
            );
        }
    });
}
fn controls(ui: Ui) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_1()
        .child(chrome_button(
            Icon::Sidebar,
            "Show / hide terminal list",
            move |_| {
                ui.terminal_sidebar
                    .set(!ui.terminal_sidebar.get_untracked())
            },
        ))
        .child(when(
            move || ui.terminal_origin.get().is_some(),
            move || chrome_button(Icon::Back, "Back to source", move |_| ui.terminal_source()),
        ))
}
pub fn screen(ui: Ui) -> impl IntoElement {
    tabs::context_updates(ui);
    let ended = computed(move || current(ui).is_some_and(|e| e.ended()));
    effect(move || {
        if ended.get() {
            ui.terminal_closed.set(true);
        }
    });
    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .child(
            nsh_base::h_resizable()
                .flex_1()
                .min_h(px(0.))
                .handle_accessibility_label("Resize Terminal sidebar")
                .render_handle(|_: &nsh_base::ResizeHandleContext| {
                    view().w(px(1.)).h_full().bg(BORDER)
                })
                .on_resize(move |sizes: &Vec<Points>| {
                    if !ui.narrow.get_untracked()
                        && ui.terminal_sidebar.get_untracked()
                        && let Some(size) = sizes.first()
                    {
                        ui.terminal_width.set((size.0 - 56.).max(190.));
                        ui.terminal_focus.update(|v| *v += 1);
                    }
                })
                .child(
                    nsh_base::resizable_panel()
                        .size(move || {
                            if ui.narrow.get() && ui.terminal_sidebar.get() {
                                None
                            } else {
                                Some(px(if ui.terminal_sidebar.get() {
                                    56. + ui.terminal_width.get()
                                } else {
                                    56.
                                }))
                            }
                        })
                        .min_size(move || px(if ui.terminal_sidebar.get() { 246. } else { 56. }))
                        .max_size(move || {
                            px(if ui.terminal_sidebar.get() {
                                if ui.narrow.get() { 10000. } else { 600. }
                            } else {
                                56.
                            })
                        })
                        .fixed(move || !ui.narrow.get() || !ui.terminal_sidebar.get())
                        .child(
                            view()
                                .flex_1()
                                .min_h(px(0.))
                                .bg(SURFACE)
                                .child(
                                    view()
                                        .h(px(42.))
                                        .flex_shrink_0()
                                        .flex_row()
                                        .items_center()
                                        .window_drag(true)
                                        .pl(move || {
                                            px(nsh_core::element::desktop::title_bar()
                                                .left
                                                .0
                                                .max(78.))
                                        })
                                        .child(when(
                                            move || ui.terminal_sidebar.get(),
                                            move || controls(ui),
                                        )),
                                )
                                .child(
                                    view()
                                        .flex_1()
                                        .min_h(px(0.))
                                        .flex_row()
                                        .child(crate::rail::sidebar(ui))
                                        .child(when(
                                            move || ui.terminal_sidebar.get(),
                                            move || tree::sidebar(ui),
                                        )),
                                ),
                        ),
                )
                .child(
                    nsh_base::resizable_panel()
                        .min_size(px(300.))
                        .visible(move || !ui.narrow.get() || !ui.terminal_sidebar.get())
                        .child(
                            view()
                                .flex_1()
                                .min_h(px(0.))
                                .min_w(px(0.))
                                .child(tabs::header(ui))
                                .child(viewer::screen(ui)),
                        ),
                ),
        )
        .child(tabs::rename_dialog(ui))
}
