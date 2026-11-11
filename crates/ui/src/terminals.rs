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

/// Finished/archived entries retain Card history but never leave stale tabs in Terminals.
pub fn watch(ui: Ui) {
    effect(move || {
        let terminal = ui.terminal_selection.get();
        let page = ui.page.get();
        if page != "terminals" || terminal.is_empty() {
            return;
        }
        if let Some(app) = crate::APP.get().cloned() {
            ui.spawn(async move {
                let _ =
                    blocking(move || app.call("terminal.context", json!({"terminal_id":terminal})))
                        .await;
            });
        }
    });
    let listed = computed(move || {
        entries(&ui.snapshot.get())
            .into_iter()
            .filter(Entry::listed)
            .map(|e| e.saved.id)
            .collect::<Vec<_>>()
    });
    effect(move || {
        if crate::APP.get().is_some() && !ui.snapshot_ready.get() {
            return;
        }
        let ids = listed.get();
        let tabs = ui.terminal_tabs.get();
        let kept: Vec<_> = tabs.iter().filter(|id| ids.contains(id)).cloned().collect();
        if tabs != kept {
            batch(move || {
                let selected = ui.terminal_selection.get_untracked();
                ui.terminal_tabs.set(kept.clone());
                if !selected.is_empty() && !ids.contains(&selected) {
                    ui.terminal_selection
                        .set(kept.last().cloned().unwrap_or_default());
                    ui.terminal_card_context.set(String::new());
                    ui.terminal_details.set(false);
                    crate::navigation::replace(ui);
                }
            });
        }
    });
}
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
        .filter(Entry::listed)
        .or_else(|| rows.into_iter().find(Entry::listed))
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
        ui.terminal_folds.update(|folds| {
            folds.remove(&format!("project:{}", row.project_id.unwrap_or_default()));
            if let Some(card) = row.card.map(|c| c.id) {
                folds.remove(&format!("card:{card}"));
            }
        });
    }
}
/// Navigation never creates/resumes an invocation; that requires an explicit Agent action.
pub fn open_card(ui: Ui, card_id: &str) {
    let existing = entries(&ui.snapshot.get_untracked())
        .into_iter()
        .filter(|e| e.agent.is_some() && e.card.as_ref().is_some_and(|c| c.id == card_id))
        .min_by_key(|e| e.ended());
    if let Some(entry) = existing {
        ui.open_terminal(entry.saved.id);
    } else {
        ui.navigate("terminals");
        ui.terminal_selection.set(String::new());
        ui.terminal_card_context.set(card_id.into());
    }
}
pub fn card_context(ui: Ui) -> Option<canban_types::Card> {
    if let Some(entry) = current(ui) {
        return entry.card;
    }
    let card = ui.terminal_card_context.get();
    ui.snapshot
        .with(|s| s.cards.iter().find(|c| c.id == card).cloned())
}
pub fn choose_card_agent(ui: Ui) {
    use nsh_core::element::desktop::{MenuItem, popup_menu};
    let Some(card) = card_context(ui).filter(|c| c.archived_at.is_none()) else {
        return;
    };
    ui.spawn(async move {
        let items = ui
            .profiles
            .get_untracked()
            .into_iter()
            .map(|p| {
                let card = card.id.clone();
                MenuItem::new(format!("Open @{}", p.id))
                    .on_select(move || crate::sessions::open(ui, card.clone(), p.id.clone()))
            })
            .collect();
        popup_menu(items).await;
    });
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
                machine: "local".into(),
                owner_core_id: None,
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
    let selected = ui.terminal_selection.get_untracked();
    let params = if selected.is_empty() {
        json!({})
    } else {
        json!({"inherit_terminal_id":selected})
    };
    launch(ui, params);
}
pub fn machine_menu(ui: Ui) {
    use nsh_core::element::desktop::{MenuItem, popup_menu};
    ui.spawn(async move {
        let mut items =
            vec![MenuItem::new("Local").on_select(move || launch(ui, json!({"machine":"local"})))];
        for m in ui
            .machines
            .with_untracked(|v| v["machines"].as_array().cloned().unwrap_or_default())
        {
            let id = m["id"].as_str().unwrap_or("").to_owned();
            let title = format!(
                "{} · {}",
                m["name"].as_str().unwrap_or("Machine"),
                m["state"].as_str().unwrap_or("unknown")
            );
            items.push(MenuItem::new(title).on_select(move || launch(ui, json!({"machine":id}))));
        }
        items.push(MenuItem::separator());
        items.push(MenuItem::new("Add machine…").on_select(move || ui.navigate("machines")));
        popup_menu(items).await;
    });
}
pub fn reopen(ui: Ui, id: String) {
    if let Some(project) = ui.snapshot.with_untracked(|s| {
        s.configuration_sessions
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.project_id.clone())
    }) {
        ui.project.set(project);
        ui.navigate("workflows");
        return;
    }

    launch(ui, json!({"inherit_terminal_id":id}));
}
pub fn close_tab(ui: Ui, id: &str) {
    if id.is_empty()
        || !ui
            .terminal_tabs
            .with_untracked(|tabs| tabs.iter().any(|t| t == id))
    {
        return;
    }
    if crate::APP.get().is_none() {
        // A static Gallery has no runtime; simulate the same closed outcome.
        let entry = entries(&ui.snapshot.get_untracked())
            .into_iter()
            .find(|e| e.saved.id == id);
        ui.snapshot.update(|s| {
            if let Some(t) = s.terminals.iter_mut().find(|t| t.id == id)
                && let Some(runtime) = &mut t.runtime
            {
                runtime.state = "finished".into();
            }
            if let Some(attempt) = entry.and_then(|e| e.agent).and_then(|r| r.attempt)
                && let Some(run) = s.attempts.iter_mut().find(|a| a.id == attempt.id)
            {
                run.observed = Some(json!({"state":"finished"}));
            }
        });
        remove_tab(ui, id);
        return;
    }
    let id = id.to_owned();
    dispatch_complete(
        ui,
        "terminal.close",
        json!({"terminal_id":id}),
        move |result| {
            if result.is_ok_and(|value| value["state"] == "closed") {
                remove_tab(ui, &id);
            }
        },
    );
}

fn remove_tab(ui: Ui, id: &str) {
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
pub fn cycle(ui: Ui, forward: bool) {
    let tabs = ui.terminal_tabs.get_untracked();
    if tabs.is_empty() {
        return;
    }
    let old = tabs
        .iter()
        .position(|t| *t == ui.terminal_selection.get_untracked())
        .unwrap_or(0);
    let next = if forward {
        (old + 1) % tabs.len()
    } else {
        (old + tabs.len() - 1) % tabs.len()
    };
    ui.open_terminal(tabs[next].clone());
}
pub(crate) const TAB_KEYS: [(&str, u8); 4] = [
    (crate::commands::CATALOG[3].key, 0),
    (crate::commands::CATALOG[4].key, 1),
    (crate::commands::CATALOG[5].key, 2),
    (crate::commands::CATALOG[6].key, 3),
];
pub fn bindings(ui: Ui) {
    use nsh_core::keymap::When;
    effect(move || {
        if ui.page.get() != "terminals"
            || ui.quick_open.get()
            || ui.terminal_rename.get().is_some()
            || !ui.utility_popup.get().is_empty()
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
        .child(when(|| true, move || crate::navigation::controls(ui)))
}
pub fn screen(ui: Ui) -> impl IntoElement {
    tabs::context_updates(ui);

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
                    view()
                        .w(px(5.))
                        .h_full()
                        .items_center()
                        .justify_center()
                        .hover(|s| s.bg(ACCENT))
                        .active(|s| s.bg(ACCENT))
                        .child(view().w(px(1.)).h_full().bg(BORDER))
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
