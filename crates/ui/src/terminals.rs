//! Independent Terminal workspace: split window chrome, numbered tabs and one viewer.
pub mod entries;
mod labels;
pub mod model;
pub mod navigation;
pub(crate) mod tabs;
mod tree;
pub(crate) mod viewer;
pub mod worktrees;
use crate::presenter::*;
use canban_notifications::OverviewRow;
use canban_ui_kit::{tokens::*, *};
pub use entries::{Entry, entries};
use nsh_core::prelude::*;
use serde_json::json;

/// Finished/archived entries retain Card history but never leave stale tabs in Terminals.
pub fn watch(ui: Ui) {
    navigation::watch(ui);
    effect(move || {
        let terminal = if ui.page.get() == "terminals" {
            ui.terminal_selection.get()
        } else if ui.page.get() == "card" {
            ui.card_tab.get()
        } else {
            String::new()
        };
        if terminal.is_empty() {
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
        let active_card_tab = ui.card_tab.get();
        if !active_card_tab.is_empty() && !ids.contains(&active_card_tab) {
            let cid = ui.card.get_untracked();
            let live = card_live_entries(&ui.snapshot.get_untracked(), &cid);
            let next = live.last().map(|e| e.saved.id.clone()).unwrap_or_default();
            ui.card_tab.set(next);
            ui.terminal_focus.update(|v| *v += 1);
        }
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
pub fn card_live_entries(snapshot: &canban_types::Snapshot, card_id: &str) -> Vec<Entry> {
    if card_id.is_empty() {
        return Vec::new();
    }
    entries(snapshot)
        .into_iter()
        .filter(|e| {
            e.card.as_ref().is_some_and(|c| c.id == card_id)
                && e.listed()
                && (e.agent.is_none() || e.binding().is_some())
        })
        .collect()
}

fn select_card_tab(ui: Ui, card_id: &str, terminal_id: String) {
    if !card_id.is_empty() && !terminal_id.is_empty() {
        ui.terminal_recent.with(|m| {
            m.insert(card_id.to_owned(), terminal_id.clone());
        });
    }
    ui.card_tab.set(terminal_id);
    ui.terminal_focus.update(|v| *v += 1);
}

fn gallery_create_entry(
    ui: Ui,
    project_id: Option<String>,
    card_id: Option<String>,
    agent: bool,
) -> String {
    let prefix = if agent {
        "gallery-agent"
    } else {
        "gallery-shell"
    };
    let id = format!("{prefix}-{}", canban_types::id());
    let agent_engine = agent.then(|| {
        let def = ui.default_agent.get_untracked();
        let project = project_id.as_deref().and_then(|pid| {
            ui.snapshot
                .with_untracked(|s| s.projects.iter().find(|p| p.id == pid).cloned())
        });
        ui.profiles
            .with_untracked(|ps| {
                ps.iter()
                    .find(|p| p.id == def)
                    .or_else(|| ps.first())
                    .map(|p| {
                        let (_, _, _) = crate::profiles::effective_provider(p, project.as_ref());
                        project
                            .as_ref()
                            .and_then(|proj| proj.agent_overrides.get(&p.id).cloned())
                            .or_else(|| p.preset.clone())
                            .unwrap_or_else(|| p.command_name())
                    })
            })
            .unwrap_or_else(|| "codex".into())
    });
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
        origin_card_id: card_id.clone(),
        agent_engine,
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
    let inherited_location = current(ui).and_then(|e| e.location);
    ui.snapshot.update(|s| {
        s.terminals.push(saved);
        if let Some(mut loc) = inherited_location {
            loc.terminal_id = id.clone();
            loc.core_id = "gallery".into();
            loc.terminal = id.clone();
            if let Some(ref pid) = project_id {
                loc.project_id = Some(pid.clone());
            }
            if let Some(ref cid) = card_id {
                loc.card_id = Some(cid.clone());
            }
            s.terminal_contexts.push(loc);
        } else if project_id.is_some() || card_id.is_some() {
            s.terminal_contexts.push(canban_types::TerminalContext {
                machine: "local".into(),
                terminal_id: id.clone(),
                core_id: "gallery".into(),
                terminal: id.clone(),
                cwd: Some("/workspace/project".into()),
                project_id,
                card_id,
                branch: None,
            });
        }
    });
    id
}

/// Open the Card's quick terminal view directly (selecting an active tab or spawning a shell).
pub fn open_card_quick(ui: Ui, card_id: &str) {
    let Some(card) = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card_id).cloned())
    else {
        return;
    };
    ui.open_card(card.id.clone());
    if !ui.card_tab.get_untracked().is_empty() {
        return;
    }
    if card.archived_at.is_some() {
        ui.operation_error
            .set("Restore this Card before opening a new terminal.".into());
        return;
    }
    create_in_card(ui, card.project_id, card.id, false);
}

/// Spawn a new Shell Terminal (`agent == false`) or clean ephemeral CLI Agent (`agent == true`) on a Card.
pub fn create_in_card(ui: Ui, project_id: String, card_id: String, agent: bool) {
    let archived = ui.snapshot.with_untracked(|s| {
        s.cards
            .iter()
            .any(|c| c.id == card_id && c.archived_at.is_some())
    });
    if archived {
        ui.operation_error.set(if agent {
            "Restore this Card before opening an Agent.".into()
        } else {
            "Restore this Card before opening a new terminal.".into()
        });
        return;
    }
    if crate::APP.get().is_none() {
        let id = gallery_create_entry(ui, Some(project_id), Some(card_id.clone()), agent);
        if ui.page.get_untracked() == "card" && ui.card.get_untracked() == card_id {
            select_card_tab(ui, &card_id, id);
        } else {
            ui.open_terminal(id);
        }
        return;
    }
    let mut params = json!({"project_id": project_id, "card_id": card_id});
    if agent {
        params["agent"] = json!(true);
    }
    let source_page = ui.page.get_untracked();
    let target_card = card_id;
    dispatch_then(ui, "terminal.create", params, move |v| {
        let Some(id) = v["id"].as_str().map(str::to_owned) else {
            return;
        };
        if ui.page.get_untracked() == "card" && ui.card.get_untracked() == target_card {
            select_card_tab(ui, &target_card, id);
        } else if source_page == "terminals" || source_page == "review" {
            ui.open_terminal(id);
        }
    });
}

/// Open Card terminal from global/menu surfaces without intermediate prompts.
pub fn open_card(ui: Ui, card_id: &str) {
    navigation::open_card(ui, card_id);
}

pub fn card_context(ui: Ui) -> Option<canban_types::Card> {
    if let Some(entry) = current(ui) {
        return entry.card;
    }
    let card = ui.terminal_card_context.get();
    ui.snapshot
        .with(|s| s.cards.iter().find(|c| c.id == card).cloned())
}

/// Launch a clean ephemeral interactive CLI Agent in the current terminal/card context (never Resume).
pub fn open_ephemeral_agent(ui: Ui) {
    if let Some(card) = card_context(ui).filter(|c| c.archived_at.is_none()) {
        create_in_card(ui, card.project_id, card.id, true);
        return;
    }
    let selected = ui.terminal_selection.get_untracked();
    let mut params = json!({"agent": true});
    if !selected.is_empty() {
        params["inherit_terminal_id"] = json!(selected);
    } else if !ui.project.get_untracked().is_empty() {
        params["project_id"] = json!(ui.project.get_untracked());
    }
    launch(ui, params);
}

pub fn open_request(ui: Ui, request_id: &str) {
    navigation::open_request(ui, request_id);
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
pub(crate) fn launch_custom(ui: Ui, params: serde_json::Value) {
    launch(ui, params);
}

fn launch(ui: Ui, params: serde_json::Value) {
    let source = ui.location();
    let selection = ui.terminal_selection.get_untracked();
    if crate::APP.get().is_none() {
        let project_id = params["project_id"].as_str().map(str::to_owned);
        let card_id = params["card_id"].as_str().map(str::to_owned);
        let agent = params["agent"] == true;
        let id = gallery_create_entry(ui, project_id, card_id, agent);
        ui.open_terminal(id);
        return;
    }
    dispatch_then(ui, "terminal.create", params, move |v| {
        if ui.location() == source
            && let Some(id) = v["id"].as_str()
        {
            if ui.page.get_untracked() == "card" && !ui.card.get_untracked().is_empty() {
                if ui.card_terminal_open.get_untracked() {
                    ui.open_card_with_terminal(ui.card.get_untracked(), Some(id.into()));
                } else {
                    select_card_tab(ui, &ui.card.get_untracked(), id.into());
                }
            } else if ui.terminal_selection.get_untracked() == selection {
                ui.open_terminal(id.into());
            }
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
        let mut items = vec![
            MenuItem::new("New Terminal (Local)")
                .on_select(move || launch(ui, json!({"machine":"local"}))),
        ];
        if !ui.project.get_untracked().is_empty()
            || ui.snapshot.with_untracked(|s| !s.projects.is_empty())
        {
            items.push(
                MenuItem::new("New Managed Branch Worktree…").on_select(move || {
                    let pid = ui.project.get_untracked();
                    let target = if pid.is_empty() {
                        ui.snapshot
                            .with_untracked(|s| s.projects.first().map(|p| p.id.clone()))
                            .unwrap_or_default()
                    } else {
                        pid
                    };
                    ui.worktree_modal.set(Some(target));
                }),
            );
        }
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
    if let Some((project, profile)) = ui.snapshot.with_untracked(|s| {
        s.configuration_sessions
            .iter()
            .find(|c| c.id == id)
            .map(|c| (c.project_id.clone(), c.profile_id.clone()))
    }) {
        dispatch_complete(
            ui,
            "configuration.open",
            json!({
                "project_id": project,
                "profile_id": profile,
                "auto_fresh": true,
            }),
            move |result| {
                if let Ok(value) = result
                    && let Some(tid) = value["terminal_id"].as_str()
                {
                    ui.open_terminal(tid.into());
                }
            },
        );
        return;
    }

    launch(ui, json!({"inherit_terminal_id":id}));
}
pub fn close_entry(ui: Ui, id: &str) {
    if id.is_empty() {
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

pub fn close_tab(ui: Ui, id: &str) {
    if id.is_empty()
        || !ui
            .terminal_tabs
            .with_untracked(|tabs| tabs.iter().any(|t| t == id))
    {
        return;
    }
    close_entry(ui, id);
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
    if ui.page.get_untracked() == "card"
        && let Some(c) = ui.current_card()
    {
        let live = card_live_entries(&ui.snapshot.get_untracked(), &c.id);
        if live.is_empty() {
            return;
        }
        let mut slots = vec![String::new()];
        slots.extend(live.into_iter().map(|e| e.saved.id));
        let cur = ui.card_tab.get_untracked();
        let old = slots.iter().position(|s| *s == cur).unwrap_or(0);
        let next = if forward {
            (old + 1) % slots.len()
        } else {
            (old + slots.len() - 1) % slots.len()
        };
        select_card_tab(ui, &c.id, slots[next].clone());
        return;
    }
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
    ("mod+t", 0),
    ("mod+w", 1),
    ("mod+shift+]", 2),
    ("mod+shift+[", 3),
];
pub fn bindings(ui: Ui) {
    use nsh_core::keymap::When;
    effect(move || {
        let on_terminals = ui.page.get() == "terminals";
        let on_card_terminal = ui.page.get() == "card" && !ui.card_tab.get().is_empty();
        if (!on_terminals && !on_card_terminal)
            || ui.quick_open.get()
            || ui.terminal_rename.get().is_some()
            || ui.worktree_modal.get().is_some()
            || ui.worktree_promote.get().is_some()
            || ui.worktree_remove_confirm.get().is_some()
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
                        0 => {
                            if ui.page.get_untracked() == "card"
                                && let Some(c) = ui.current_card()
                            {
                                create_in_card(ui, c.project_id, c.id, false);
                            } else {
                                new_tab(ui);
                            }
                        }
                        1 => {
                            if ui.page.get_untracked() == "card" {
                                close_entry(ui, &ui.card_tab.get_untracked());
                            } else {
                                close_tab(ui, &ui.terminal_selection.get_untracked());
                            }
                        }
                        2 => cycle(ui, true),
                        _ => cycle(ui, false),
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
        .child(when(|| true, move || crate::navigation::controls(ui)))
        .child(chrome_button(
            Icon::Sidebar,
            "Show / hide terminal list",
            move |_| {
                ui.terminal_sidebar
                    .set(!ui.terminal_sidebar.get_untracked());
                ui.terminal_focus.update(|v| *v += 1);
            },
        ))
}
pub fn screen(ui: Ui) -> impl IntoElement {
    tabs::context_updates(ui);

    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .flex_row()
        .child(
            view()
                .w(px(56.))
                .flex_shrink_0()
                .min_h(px(0.))
                .bg(SURFACE)
                .child(
                    view()
                        .h(px(42.))
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(BORDER)
                        .window_drag(true),
                )
                .child(
                    view()
                        .flex_1()
                        .min_h(px(0.))
                        .child(crate::rail::sidebar(ui)),
                ),
        )
        .child(
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
                                ui.terminal_width.set(size.0.max(190.));
                                ui.terminal_focus.update(|v| *v += 1);
                            }
                        })
                        .child(
                            nsh_base::resizable_panel()
                                .visible(move || ui.terminal_sidebar.get())
                                .size(move || {
                                    if ui.narrow.get() {
                                        None
                                    } else {
                                        Some(px(ui.terminal_width.get()))
                                    }
                                })
                                .min_size(px(190.))
                                .max_size(move || px(if ui.narrow.get() { 10000. } else { 544. }))
                                .fixed(move || !ui.narrow.get())
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
                                                .border_b_1()
                                                .border_color(BORDER)
                                                .window_drag(true)
                                                .pl(move || {
                                                    px((crate::frame::chrome_left_padding() - 56.)
                                                        .max(0.))
                                                })
                                                .child(controls(ui)),
                                        )
                                        .child(tree::sidebar(ui)),
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
                .child(crate::frame::backend_notice(ui))
                .child(crate::utilities::bar(ui)),
        )
}
