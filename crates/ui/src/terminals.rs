//! Independent Terminal workspace: split window chrome, numbered tabs and one viewer.
pub mod entries;
pub(crate) mod labels;
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

#[cfg(test)]
pub(crate) fn sidebar_for_test(ui: Ui) -> impl IntoElement {
    tree::sidebar(ui)
}

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
        if crate::APP.get().is_some() {
            ui.spawn(async move {
                let _ = blocking(move || {
                    crate::routed_call("terminal.context", json!({"terminal_id":terminal}), None)
                })
                .await;
            });
        }
    });
    let listed = computed(move || {
        ui.closing_terminals.with(|closing| {
            ui.terminal_entries.with(|rows| {
                rows.iter()
                    .filter(|e| e.listed() && !closing.contains(&e.saved.id))
                    .map(|e| e.saved.id.clone())
                    .collect::<Vec<_>>()
            })
        })
    });
    let prev_entries = signal(Vec::<Entry>::new());
    effect(move || {
        if crate::APP.get().is_some() && !ui.snapshot_ready.get() {
            return;
        }
        let ids = listed.get();
        let current_rows = ui.terminal_entries.get();
        let active_card_tab = ui.card_tab.get();
        if let Some(confirm_id) = ui.terminal_close_confirm.get_untracked()
            && !ids.contains(&confirm_id)
        {
            ui.terminal_close_confirm.set(None);
        }
        if !active_card_tab.is_empty() && !ids.contains(&active_card_tab) {
            let cid = ui.card.get_untracked();
            let recent = ui.terminal_recent.with(|m| m.get(&cid).cloned());
            let next = untracked(|| default_card_terminal_for_ui(ui, &cid, recent.as_deref()))
                .unwrap_or_default();
            if next.is_empty() {
                ui.card_tab.set(String::new());
                if !ui.is_method_pending("terminal.create") && !ui.is_method_pending("agent.open") {
                    ui.card_terminal_open.set(false);
                    ui.card_terminal_focused.set(false);
                }
            } else {
                select_card_tab(ui, &cid, next);
            }
        }
        let tabs = ui.terminal_tabs.get();
        let kept: Vec<_> = tabs.iter().filter(|id| ids.contains(id)).cloned().collect();
        let selected = ui.terminal_selection.get_untracked();
        let selected_gone = !selected.is_empty() && !ids.contains(&selected);
        if tabs != kept || selected_gone {
            let fallback = if selected_gone {
                let s = ui.snapshot.get_untracked();
                let closing = ui.closing_terminals.get_untracked();
                let mut combined = prev_entries.get_untracked();
                for r in &current_rows {
                    if let Some(existing) = combined.iter_mut().find(|e| e.saved.id == r.saved.id) {
                        *existing = r.clone();
                    } else {
                        combined.push(r.clone());
                    }
                }
                fallback_after_close(&s, &combined, &closing, &selected)
                    .filter(|nid| ids.contains(nid))
                    .unwrap_or_default()
            } else {
                selected.clone()
            };
            batch(move || {
                ui.terminal_tabs.set(kept);
                if selected_gone {
                    ui.terminal_selection.set(fallback.clone());
                    if !fallback.is_empty() {
                        ui.terminal_tabs.update(|ts| {
                            if !ts.contains(&fallback) {
                                ts.push(fallback.clone());
                            }
                        });
                        if let Some(pid) = ui.snapshot.with_untracked(|s| {
                            ui.terminal_entries.with_untracked(|rows| {
                                rows.iter()
                                    .find(|e| e.saved.id == fallback)
                                    .and_then(|e| worktrees::entry_scope(s, e).project_id(s))
                            })
                        }) {
                            ui.project.set(pid);
                        }
                    }
                    ui.terminal_card_context.set(String::new());
                    ui.terminal_details.set(false);
                    crate::navigation::replace(ui);
                }
            });
        }
        prev_entries.set(current_rows);
    });
}
pub fn selected(ui: Ui) -> Option<OverviewRow> {
    let sel = ui.terminal_selection.get();
    ui.overview_rows.with(|rows| {
        rows.iter()
            .find(|r| r.id == sel && r.terminal.is_some())
            .cloned()
    })
}
pub fn current(ui: Ui) -> Option<Entry> {
    let sel = ui.terminal_selection.get();
    if sel.is_empty() || ui.closing_terminals.with(|c| c.contains(&sel)) {
        return None;
    }
    ui.terminal_entries
        .with(|rows| rows.iter().find(|r| r.saved.id == sel).cloned())
}

/// Live Terminals belonging to the same `WorktreeScope` as `current(ui)`, in creation order.
pub fn scope_tabs(ui: Ui) -> Vec<Entry> {
    let Some(cur) = current(ui) else {
        return Vec::new();
    };
    ui.snapshot.with(|s| {
        let target_scope = worktrees::entry_scope(s, &cur);
        ui.closing_terminals.with(|closing| {
            ui.terminal_entries.with(|rows| {
                rows.iter()
                    .filter(|e| {
                        e.listed()
                            && !closing.contains(&e.saved.id)
                            && worktrees::entry_scope(s, e) == target_scope
                    })
                    .cloned()
                    .collect()
            })
        })
    })
}

/// Pure fallback resolution when closing a live Terminal:
/// 1. Previous live Terminal in the same `WorktreeScope` (or next if none precedes it);
/// 2. Previous live Terminal in the same Project in tree order (or next if none precedes it);
/// 3. Previous live Terminal globally in sidebar tree order (or next if none precedes it).
pub fn fallback_after_close(
    s: &canban_types::Snapshot,
    rows: &[Entry],
    closing: &std::collections::BTreeSet<String>,
    closed_id: &str,
) -> Option<String> {
    let closed_entry = rows.iter().find(|e| e.saved.id == closed_id)?;
    let target_scope = worktrees::entry_scope(s, closed_entry);
    let is_live_or_target =
        |e: &Entry| e.saved.id == closed_id || (e.listed() && !closing.contains(&e.saved.id));

    let same_scope: Vec<&Entry> = rows
        .iter()
        .filter(|e| is_live_or_target(e) && worktrees::entry_scope(s, e) == target_scope)
        .collect();
    if let Some(pos) = same_scope.iter().position(|e| e.saved.id == closed_id) {
        if pos > 0 {
            return Some(same_scope[pos - 1].saved.id.clone());
        }
        if pos + 1 < same_scope.len() {
            return Some(same_scope[pos + 1].saved.id.clone());
        }
    }

    let project_id = target_scope.project_id(s);
    let candidate_rows: Vec<Entry> = rows
        .iter()
        .filter(|e| is_live_or_target(e))
        .cloned()
        .collect();
    let ordered_scopes =
        worktrees::project_scopes_in_tree_order(s, &candidate_rows, project_id.as_deref());
    let same_project: Vec<Entry> = ordered_scopes
        .into_iter()
        .flat_map(|(_, children)| children)
        .collect();
    if let Some(pos) = same_project.iter().position(|e| e.saved.id == closed_id) {
        if pos > 0 {
            return Some(same_project[pos - 1].saved.id.clone());
        }
        if pos + 1 < same_project.len() {
            return Some(same_project[pos + 1].saved.id.clone());
        }
    }

    let mut global_tree: Vec<Entry> = Vec::new();
    for p in &s.projects {
        for (_, children) in
            worktrees::project_scopes_in_tree_order(s, &candidate_rows, Some(p.id.as_str()))
        {
            global_tree.extend(children);
        }
    }
    for (_, children) in worktrees::project_scopes_in_tree_order(s, &candidate_rows, None) {
        global_tree.extend(children);
    }
    if let Some(pos) = global_tree.iter().position(|e| e.saved.id == closed_id) {
        if pos > 0 {
            return Some(global_tree[pos - 1].saved.id.clone());
        }
        if pos + 1 < global_tree.len() {
            return Some(global_tree[pos + 1].saved.id.clone());
        }
    }

    None
}
pub fn restore_selection(ui: Ui) {
    if !ui.terminal_selection.get_untracked().is_empty() {
        return;
    }
    let entry = ui.closing_terminals.with_untracked(|closing| {
        ui.terminal_entries.with_untracked(|rows| {
            ui.terminal_tabs
                .with_untracked(|ts| {
                    ts.last()
                        .and_then(|id| rows.iter().find(|e| &e.saved.id == id))
                        .cloned()
                })
                .filter(|e| e.listed() && !closing.contains(&e.saved.id))
                .or_else(|| {
                    rows.iter()
                        .find(|e| e.listed() && !closing.contains(&e.saved.id))
                        .cloned()
                })
        })
    });
    if let Some(entry) = entry {
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
    if let Some(row) = ui
        .terminal_entries
        .with_untracked(|rows| rows.iter().find(|r| r.saved.id == id).cloned())
    {
        ui.terminal_folds.update(|folds| {
            folds.remove(&format!("project:{}", row.project_id.unwrap_or_default()));
            if let Some(card) = row.card.map(|c| c.id) {
                folds.remove(&format!("card:{card}"));
            }
        });
    }
}
pub(crate) fn is_card_live_entry(e: &Entry, card_id: &str) -> bool {
    e.card.as_ref().is_some_and(|c| c.id == card_id)
        && e.listed()
        && (e.agent.is_none()
            || e.binding().is_some()
            || matches!(e.status().as_str(), "queued" | "starting"))
}

pub fn card_live_entries(snapshot: &canban_types::Snapshot, card_id: &str) -> Vec<Entry> {
    if card_id.is_empty() {
        return Vec::new();
    }
    entries(snapshot)
        .into_iter()
        .filter(|e| is_card_live_entry(e, card_id))
        .collect()
}

pub fn card_live_entries_for_ui(ui: Ui, card_id: &str) -> Vec<Entry> {
    if card_id.is_empty() {
        return Vec::new();
    }
    ui.closing_terminals.with(|closing| {
        ui.terminal_entries.with(|rows| {
            rows.iter()
                .filter(|e| is_card_live_entry(e, card_id) && !closing.contains(&e.saved.id))
                .cloned()
                .collect()
        })
    })
}

/// Resolve the active Terminal/Agent tab for a Card: prefer the Card's last-selected
/// live entry when still active, otherwise select the first active entry (`live.first()`).
pub fn default_card_terminal(
    snapshot: &canban_types::Snapshot,
    card_id: &str,
    recent: Option<&str>,
) -> Option<String> {
    let live = card_live_entries(snapshot, card_id);
    recent
        .filter(|rid| live.iter().any(|e| e.saved.id.as_str() == *rid))
        .map(str::to_owned)
        .or_else(|| live.first().map(|e| e.saved.id.clone()))
}

pub fn default_card_terminal_for_ui(ui: Ui, card_id: &str, recent: Option<&str>) -> Option<String> {
    let live = card_live_entries_for_ui(ui, card_id);
    recent
        .filter(|rid| live.iter().any(|e| e.saved.id.as_str() == *rid))
        .map(str::to_owned)
        .or_else(|| live.first().map(|e| e.saved.id.clone()))
}

pub(crate) fn select_card_tab(ui: Ui, card_id: &str, terminal_id: String) {
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
    inherit: bool,
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
                            .filter(|_| p.is_identity())
                            .and_then(|proj| {
                                proj.agent_overrides
                                    .get(&p.id)
                                    .and_then(|ov| ov.normalized().agent)
                            })
                            .or_else(|| p.preset.clone())
                            .unwrap_or_else(|| p.command_name())
                    })
            })
            .unwrap_or_else(|| "codex".into())
    });
    let inherited = inherit.then(|| untracked(|| current(ui))).flatten();
    let origin_card = card_id.clone().or_else(|| {
        inherited
            .as_ref()
            .filter(|e| e.agent.is_none())
            .and_then(|e| e.saved.origin_card_id.clone())
    });
    let origin_project = project_id
        .clone()
        .or_else(|| {
            origin_card.as_deref().and_then(|cid| {
                ui.snapshot.with_untracked(|s| {
                    s.cards
                        .iter()
                        .find(|c| c.id == cid)
                        .map(|c| c.project_id.clone())
                })
            })
        })
        .or_else(|| inherited.as_ref().and_then(|e| e.project_id.clone()));
    let resolved_name = ui.snapshot.with_untracked(|s| {
        s.next_terminal_label(origin_project.as_deref(), agent_engine.as_deref(), None)
    });
    let saved = canban_types::TerminalEntry {
        id: id.clone(),
        conversation_id: None,
        number: ui
            .snapshot
            .with_untracked(|s| s.terminals.iter().map(|t| t.number).max().unwrap_or(8))
            + 1,
        name: Some(resolved_name),
        origin_project_id: origin_project,
        origin_card_id: origin_card,
        agent_engine,
        agent: None,
        created_at: 0,
        runtime: Some(canban_types::TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("gallery".into()),
            terminal: Some(id.clone()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
            observed_agent: None,
            waiting: None,
        }),
    };
    let inherited_location = inherited.and_then(|e| e.location);
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
            let resolved_card = card_id
                .as_deref()
                .and_then(|cid| s.cards.iter().find(|c| c.id == cid));
            let resolved_project = project_id
                .as_deref()
                .or_else(|| resolved_card.map(|c| c.project_id.as_str()))
                .and_then(|pid| s.projects.iter().find(|p| p.id == pid));
            let cwd = resolved_card
                .and_then(|c| c.worktree.clone())
                .filter(|w| !w.is_empty())
                .or_else(|| resolved_project.map(|p| p.directory.clone()))
                .unwrap_or_else(|| "/workspace/project".into());
            let branch = resolved_card
                .and_then(|c| c.branch.clone())
                .or_else(|| resolved_project.and_then(|p| p.target_branch.clone()));
            let machine = resolved_project
                .and_then(|p| p.location.as_ref().map(|l| l.machine.clone()))
                .unwrap_or_else(|| "local".into());
            s.terminal_contexts.push(canban_types::TerminalContext {
                machine,
                terminal_id: id.clone(),
                core_id: "gallery".into(),
                terminal: id.clone(),
                cwd: Some(cwd),
                project_id: project_id.or_else(|| resolved_project.map(|p| p.id.clone())),
                card_id,
                branch,
                ..Default::default()
            });
        }
    });
    id
}

/// Open the Card's side-by-side terminal split directly (selecting the active tab or spawning a shell).
pub fn open_card_quick(ui: Ui, card_id: &str) {
    let Some(card) = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card_id).cloned())
    else {
        return;
    };
    if ui.page.get_untracked() != "card" || ui.card.get_untracked() != card.id {
        ui.open_card(card.id.clone());
    }
    if !ui.card_tab.get_untracked().is_empty() {
        ui.card_terminal_open.set(true);
        ui.card_terminal_focused.set(true);
        ui.terminal_focus.update(|v| *v += 1);
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
    let source_page = ui.page.get_untracked();
    let in_card_workspace = source_page != "terminals";
    if in_card_workspace {
        if source_page != "card" || ui.card.get_untracked() != card_id {
            ui.open_card(card_id.clone());
        }
        ui.card_terminal_open.set(true);
        ui.card_terminal_focused.set(true);
    }
    if crate::APP.get().is_none() {
        let id = gallery_create_entry(ui, Some(project_id), Some(card_id.clone()), agent, false);
        if ui.page.get_untracked() == "card" && ui.card.get_untracked() == card_id {
            select_card_tab(ui, &card_id, id);
            ui.card_terminal_open.set(true);
            ui.card_terminal_focused.set(true);
        } else {
            ui.open_terminal(id);
        }
        return;
    }
    let mut params = json!({"project_id": project_id, "card_id": card_id});
    if agent {
        params["agent"] = json!(true);
    }
    let target_card = card_id;
    dispatch_then(ui, "terminal.create", params, move |v| {
        let Some(id) = v["id"].as_str().map(str::to_owned) else {
            return;
        };
        if ui.page.get_untracked() == "card" && ui.card.get_untracked() == target_card {
            select_card_tab(ui, &target_card, id);
            ui.card_terminal_open.set(true);
            ui.card_terminal_focused.set(true);
        } else if source_page == "terminals" {
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
        let custom_name = params["name"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let agent = params["agent"] == true;
        let inherit = params["inherit_terminal_id"].as_str().is_some();
        let id = gallery_create_entry(ui, project_id, card_id, agent, inherit);
        if let Some(hid) = params["host_id"]
            .as_str()
            .filter(|h| !h.is_empty() && *h != "local")
        {
            ui.snapshot.update(|s| {
                s.entity_hosts.insert(id.clone(), hid.to_owned());
            });
        }
        if let Some(name) = custom_name {
            ui.snapshot.update(|s| {
                if let Some(t) = s.terminals.iter_mut().find(|t| t.id == id) {
                    t.name = Some(name);
                }
            });
        }
        ui.open_terminal(id);
        return;
    }
    dispatch_then(ui, "terminal.create", params, move |v| {
        if ui.location() == source
            && let Some(id) = v["id"].as_str()
        {
            if ui.page.get_untracked() == "card" && !ui.card.get_untracked().is_empty() {
                ui.open_card_with_terminal(ui.card.get_untracked(), Some(id.into()));
            } else if ui.terminal_selection.get_untracked() == selection {
                ui.open_terminal(id.into());
            }
        }
    });
}

/// In-page `+` and `⌘T` while already on `Terminals`: inherits the current `WorktreeScope`,
/// or falls back to the active project's root directory when no Terminal is selected.
pub fn new_tab(ui: Ui) {
    if let Some(cur) = untracked(|| current(ui)) {
        if cur.agent.is_some()
            && let Some(card) = cur.card.filter(|c| c.archived_at.is_none())
        {
            launch(
                ui,
                json!({
                    "project_id": card.project_id,
                    "card_id": card.id,
                }),
            );
            return;
        }
        launch(ui, json!({ "inherit_terminal_id": cur.saved.id }));
        return;
    }
    let pid = ui.project.get_untracked();
    if !pid.is_empty() {
        launch(ui, json!({ "project_id": pid }));
    } else {
        launch(ui, json!({}));
    }
}

/// Global New Terminal (`⌘T` / `new-terminal` outside `Terminals`): opens a fresh shell in the
/// active project's root (`ProjectRoot`) without inheriting any previous sub-worktree session.
pub fn open_global(ui: Ui) {
    let return_to = ui.location();
    let pid = ui.project.get_untracked();
    let target_project = if !pid.is_empty() {
        Some(pid)
    } else {
        ui.snapshot
            .with_untracked(|s| s.projects.first().map(|p| p.id.clone()))
    };
    if crate::APP.get().is_none() {
        if let Some(ref p) = target_project {
            ui.project.set(p.clone());
        }
        let id = gallery_create_entry(ui, target_project, None, false, false);
        ui.open_terminal(id);
        ui.terminal_focus.update(|v| *v += 1);
        return;
    }
    if let Some(ref p) = target_project {
        ui.project.set(p.clone());
    }
    ui.navigate("terminals");
    let selection = ui.terminal_selection.get_untracked();
    let intermediate = ui.location();
    let params = match target_project {
        Some(p) => json!({ "project_id": p }),
        None => json!({}),
    };
    dispatch_then(ui, "terminal.create", params, move |v| {
        if ui.location() == intermediate
            && let Some(id) = v["id"].as_str()
            && ui.terminal_selection.get_untracked() == selection
        {
            ui.history.update(|history| {
                history.visit(intermediate.clone());
                history.entries[history.index] = return_to.clone();
                if history.index > 0 && history.entries[history.index - 1] == return_to {
                    history.entries.remove(history.index);
                    history.index -= 1;
                }
            });
            ui.open_terminal(id.into());
        }
    });
}

pub fn machine_menu(ui: Ui) {
    use nsh_core::element::desktop::{MenuItem, popup_menu};
    ui.spawn(async move {
        let mut items = vec![
            MenuItem::new(format!(
                "New Terminal in Current Worktree · {}",
                crate::commands::shortcuts::NEW_TERMINAL_DISPLAY
            ))
            .on_select(move || new_tab(ui)),
        ];
        let active_pid = {
            let pid = ui.project.get_untracked();
            if !pid.is_empty() {
                Some(pid)
            } else {
                ui.snapshot
                    .with_untracked(|s| s.projects.first().map(|p| p.id.clone()))
            }
        };
        if let Some(pid) = active_pid.clone() {
            let proj_name = ui
                .snapshot
                .with_untracked(|s| {
                    s.projects
                        .iter()
                        .find(|p| p.id == pid)
                        .map(|p| p.name.clone())
                })
                .unwrap_or_else(|| "Project".into());
            let pid_root = pid.clone();
            items.push(
                MenuItem::new(format!("New Terminal in {proj_name} Root"))
                    .on_select(move || create(ui, pid_root.clone(), None)),
            );
            let pid_wt = pid.clone();
            items.push(
                MenuItem::new("New Managed Branch Worktree…")
                    .on_select(move || ui.worktree_modal.set(Some(pid_wt.clone()))),
            );
            let worktrees: Vec<_> = ui.snapshot.with_untracked(|s| {
                s.worktrees
                    .iter()
                    .filter(|w| w.project_id == pid)
                    .cloned()
                    .collect()
            });
            if !worktrees.is_empty() {
                items.push(MenuItem::separator());
                for wt in worktrees {
                    let wt_open = wt.clone();
                    items.push(
                        MenuItem::new(format!("Open Shell in ⎇ {}", wt.branch))
                            .on_select(move || worktrees::open_shell(ui, &wt_open)),
                    );
                }
            }
        }
        items.push(MenuItem::separator());
        for h in crate::hosts::effective_hosts(ui) {
            if h.is_local {
                items.push(
                    MenuItem::new(format!(
                        "New Terminal ($HOME · {})",
                        crate::platform::LOCAL_HOST_NAME
                    ))
                    .on_select(move || {
                        launch(ui, json!({ "host_id": "local", "machine": "local" }))
                    }),
                );
            } else if h.is_connected() {
                let hid = h.id.clone();
                items.push(
                    MenuItem::new(format!("New Terminal ($HOME · {})", h.name))
                        .on_select(move || launch(ui, json!({ "host_id": hid }))),
                );
            }
        }
        items.push(MenuItem::separator());
        items.push(
            MenuItem::new("Connect Machine…")
                .on_select(move || crate::hosts::open_connect_modal(ui)),
        );
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
    if id.is_empty() || ui.closing_terminals.with_untracked(|c| c.contains(id)) {
        return;
    }
    let needs_confirm = ui.snapshot.with_untracked(|s| {
        ui.terminal_entries.with_untracked(|rows| {
            rows.iter()
                .find(|e| e.saved.id == id)
                .and_then(|e| e.close_confirm_reason(s))
                .is_some()
        })
    });
    if needs_confirm {
        ui.terminal_close_confirm.set(Some(id.to_owned()));
        return;
    }
    close_entry_confirmed(ui, id);
}

fn mark_entry_finished_in_snapshot(s: &mut canban_types::Snapshot, id: &str) {
    if let Some(t) = s.terminals.iter_mut().find(|t| t.id == id) {
        if let Some(runtime) = &mut t.runtime {
            runtime.state = "finished".into();
        }
        if let Some(run) = &mut t.agent {
            run.observed = Some(json!({"state":"finished"}));
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct OptimisticCloseContext {
    pub id: String,
    pub prev_tab_index: Option<usize>,
    pub was_selected_tab: bool,
    pub fallback_tab: Option<String>,
    pub was_card_tab_for: Option<String>,
}

pub(crate) fn begin_optimistic_close(ui: Ui, id: &str) -> Option<OptimisticCloseContext> {
    if id.is_empty() || ui.closing_terminals.with_untracked(|c| c.contains(id)) {
        return None;
    }
    ui.terminal_close_confirm.set(None);
    let entry = ui
        .terminal_entries
        .with_untracked(|rows| rows.iter().find(|e| e.saved.id == id).cloned());
    if let Some(ref e) = entry
        && let Some(b) = e.binding()
    {
        let core_id = b["core_id"].as_str().unwrap_or("");
        let terminal = b["terminal"].as_str().unwrap_or("");
        if !core_id.is_empty() && !terminal.is_empty() {
            canban_terminal::evict_session(core_id, terminal);
        }
    }
    let prev_tab_index = ui
        .terminal_tabs
        .with_untracked(|tabs| tabs.iter().position(|t| t == id));
    let was_selected_tab = ui.terminal_selection.get_untracked() == id;
    let was_card_tab_for = (ui.page.get_untracked() == "card" && ui.card_tab.get_untracked() == id)
        .then(|| ui.card.get_untracked())
        .filter(|cid| !cid.is_empty());

    let fallback = was_selected_tab
        .then(|| {
            let s = ui.snapshot.get_untracked();
            let closing = ui.closing_terminals.get_untracked();
            ui.terminal_entries
                .with_untracked(|rows| fallback_after_close(&s, rows, &closing, id))
        })
        .flatten();

    batch(|| {
        ui.closing_terminals.update(|c| {
            c.insert(id.to_owned());
        });
        apply_tab_removal(ui, id, fallback.clone());
        if let Some(ref cid) = was_card_tab_for {
            let recent = ui.terminal_recent.with(|m| m.get(cid).cloned());
            let next = untracked(|| default_card_terminal_for_ui(ui, cid, recent.as_deref()))
                .unwrap_or_default();
            if next.is_empty() {
                ui.card_tab.set(String::new());
                if !ui.is_method_pending("terminal.create") && !ui.is_method_pending("agent.open") {
                    ui.card_terminal_open.set(false);
                    ui.card_terminal_focused.set(false);
                }
            } else {
                select_card_tab(ui, cid, next);
            }
        }
    });

    Some(OptimisticCloseContext {
        id: id.to_owned(),
        prev_tab_index,
        was_selected_tab,
        fallback_tab: fallback,
        was_card_tab_for,
    })
}

pub(crate) fn finish_optimistic_close(
    ui: Ui,
    ctx: &OptimisticCloseContext,
    outcome: Result<serde_json::Value, String>,
) {
    match outcome {
        Ok(value) if value["state"] == "closed" => {
            batch(|| {
                ui.snapshot.update(|s| {
                    mark_entry_finished_in_snapshot(s, &ctx.id);
                });
                ui.closing_terminals.update(|c| {
                    c.remove(&ctx.id);
                });
            });
        }
        Ok(value) => {
            let msg = format!(
                "terminal.close: {}",
                value["message"]
                    .as_str()
                    .unwrap_or("Inspect before retrying")
            );
            rollback_optimistic_close(ui, ctx, msg);
        }
        Err(error) => {
            rollback_optimistic_close(ui, ctx, error);
        }
    }
}

fn rollback_optimistic_close(ui: Ui, ctx: &OptimisticCloseContext, message: String) {
    batch(|| {
        ui.closing_terminals.update(|c| {
            c.remove(&ctx.id);
        });
        if let Some(idx) = ctx.prev_tab_index {
            ui.terminal_tabs.update(|tabs| {
                if !tabs.contains(&ctx.id) {
                    let insert_at = idx.min(tabs.len());
                    tabs.insert(insert_at, ctx.id.clone());
                }
            });
        }
        if ctx.was_selected_tab {
            let cur_sel = ui.terminal_selection.get_untracked();
            if cur_sel.is_empty() || ctx.fallback_tab.as_deref() == Some(cur_sel.as_str()) {
                ui.terminal_selection.set(ctx.id.clone());
            }
        }
        if let Some(ref cid) = ctx.was_card_tab_for
            && ui.page.get_untracked() == "card"
            && ui.card.get_untracked() == *cid
            && ui.card_tab.get_untracked().is_empty()
        {
            select_card_tab(ui, cid, ctx.id.clone());
            ui.card_terminal_open.set(true);
        }
        ui.status.set(message.clone());
        ui.operation_error.set(message);
    });
}

pub fn close_entry_confirmed(ui: Ui, id: &str) {
    if id.is_empty() {
        return;
    }
    if crate::APP.get().is_none() {
        // A static Gallery has no runtime; simulate the same closed outcome.
        ui.terminal_close_confirm.set(None);
        let was_selected = ui.terminal_selection.get_untracked() == id;
        let fallback = was_selected
            .then(|| {
                let s = ui.snapshot.get_untracked();
                let closing = ui.closing_terminals.get_untracked();
                ui.terminal_entries
                    .with_untracked(|rows| fallback_after_close(&s, rows, &closing, id))
            })
            .flatten();
        ui.snapshot.update(|s| {
            mark_entry_finished_in_snapshot(s, id);
        });
        apply_tab_removal(ui, id, fallback);
        return;
    }
    let Some(ctx) = begin_optimistic_close(ui, id) else {
        return;
    };
    let rpc_id = ctx.id.clone();
    let request_key = canban_types::id();
    ui.spawn(async move {
        let result = blocking(move || {
            let value = crate::routed_call(
                "terminal.close",
                json!({"terminal_id": rpc_id, "request_key": request_key}),
                None,
            )?;
            let snapshot = crate::merged_snapshot().ok();
            Ok::<_, anyhow::Error>((value, snapshot))
        })
        .await;
        match result {
            Ok(Ok((value, snapshot))) => {
                if let Some(s) = snapshot {
                    ui.snapshot_epoch.update(|v| *v += 1);
                    ui.snapshot.set(s);
                }
                finish_optimistic_close(ui, &ctx, Ok(value));
            }
            Ok(Err(e)) => {
                finish_optimistic_close(ui, &ctx, Err(format!("{e:#}")));
                refresh(ui);
            }
            Err(e) => {
                finish_optimistic_close(ui, &ctx, Err(e.to_string()));
                refresh(ui);
            }
        }
    });
}

pub fn close_tab(ui: Ui, id: &str) {
    if id.is_empty() {
        return;
    }
    let can_close = ui
        .terminal_tabs
        .with_untracked(|tabs| tabs.iter().any(|t| t == id))
        || ui
            .terminal_entries
            .with_untracked(|rows| rows.iter().any(|e| e.saved.id == id && e.listed()));
    if !can_close {
        return;
    }
    close_entry(ui, id);
}

fn apply_tab_removal(ui: Ui, id: &str, fallback: Option<String>) {
    ui.terminal_focus.update(|v| *v += 1);
    let mut tabs = ui.terminal_tabs.get_untracked();
    if let Some(i) = tabs.iter().position(|t| t == id) {
        tabs.remove(i);
    }
    if ui.terminal_selection.get_untracked() == id {
        let next = fallback.unwrap_or_default();
        if !next.is_empty() && !tabs.contains(&next) {
            tabs.push(next.clone());
        }
        ui.terminal_tabs.set(tabs);
        if !next.is_empty()
            && let Some((card_id, scope_key, pid)) = ui.snapshot.with_untracked(|s| {
                ui.terminal_entries.with_untracked(|rows| {
                    rows.iter().find(|e| e.saved.id == next).map(|e| {
                        let scope = worktrees::entry_scope(s, e);
                        (
                            e.card.as_ref().map(|c| c.id.clone()),
                            scope.recent_key(),
                            scope.project_id(s),
                        )
                    })
                })
            })
        {
            ui.terminal_recent.with(|recent| {
                if let Some(cid) = card_id {
                    recent.insert(cid, next.clone());
                }
                recent.insert(scope_key, next.clone());
            });
            if let Some(pid) = pid {
                ui.project.set(pid);
            }
        }
        ui.terminal_selection.set(next);
        ui.terminal_details.set(false);
        if ui.narrow.get_untracked() && ui.terminal_selection.get_untracked().is_empty() {
            ui.terminal_sidebar.set(true);
        }
    } else {
        ui.terminal_tabs.set(tabs);
    }
}

pub fn cycle(ui: Ui, forward: bool) {
    if ui.page.get_untracked() == "card"
        && ui.card_terminal_open.get_untracked()
        && let Some(c) = ui.current_card()
    {
        let slots: Vec<_> = untracked(|| card_live_entries_for_ui(ui, &c.id))
            .into_iter()
            .map(|e| e.saved.id)
            .collect();
        if slots.is_empty() {
            return;
        }
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
    let tabs: Vec<String> = untracked(|| scope_tabs(ui))
        .into_iter()
        .map(|e| e.saved.id)
        .collect();
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
pub fn select_tab_index(ui: Ui, index: usize) {
    if ui.page.get_untracked() == "card"
        && ui.card_terminal_open.get_untracked()
        && let Some(c) = ui.current_card()
    {
        let slots: Vec<_> = untracked(|| card_live_entries_for_ui(ui, &c.id))
            .into_iter()
            .map(|e| e.saved.id)
            .collect();
        if let Some(target) = slots.get(index) {
            select_card_tab(ui, &c.id, target.clone());
        }
        return;
    }
    let tabs: Vec<String> = untracked(|| scope_tabs(ui))
        .into_iter()
        .map(|e| e.saved.id)
        .collect();
    if let Some(target) = tabs.get(index) {
        ui.open_terminal(target.clone());
    }
}

#[cfg(target_os = "macos")]
pub(crate) const TAB_KEYS: [(&str, u8); 12] = [
    ("mod+w", 1),
    ("mod+shift+]", 2),
    ("mod+shift+[", 3),
    ("mod+1", 11),
    ("mod+2", 12),
    ("mod+3", 13),
    ("mod+4", 14),
    ("mod+5", 15),
    ("mod+6", 16),
    ("mod+7", 17),
    ("mod+8", 18),
    ("mod+9", 19),
];

#[cfg(not(target_os = "macos"))]
pub(crate) const TAB_KEYS: [(&str, u8); 12] = [
    ("ctrl+shift+w", 1),
    ("ctrl+shift+]", 2),
    ("ctrl+shift+[", 3),
    ("alt+1", 11),
    ("alt+2", 12),
    ("alt+3", 13),
    ("alt+4", 14),
    ("alt+5", 15),
    ("alt+6", 16),
    ("alt+7", 17),
    ("alt+8", 18),
    ("alt+9", 19),
];
pub fn bindings(ui: Ui) {
    use nsh_core::keymap::When;
    effect(move || {
        let on_terminals = ui.page.get() == "terminals";
        let on_card_terminal = ui.page.get() == "card" && ui.card_terminal_open.get();
        if (!on_terminals && !on_card_terminal)
            || ui.quick_open.get()
            || ui.terminal_rename.get().is_some()
            || ui.terminal_close_confirm.get().is_some()
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
                        1 => {
                            if ui.page.get_untracked() == "card" {
                                let cid = ui.card_id();
                                if let Some(entry) =
                                    crate::card::terminal::select_entry_for_card(ui, &cid)
                                {
                                    close_entry(ui, &entry.saved.id);
                                }
                            } else {
                                close_tab(ui, &ui.terminal_selection.get_untracked());
                            }
                        }
                        2 => cycle(ui, true),
                        3 => cycle(ui, false),
                        n @ 11..=19 => select_tab_index(ui, usize::from(n - 11)),
                        _ => {}
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
                    canban_ui_kit::split::horizontal_group("Resize Terminal sidebar")
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
                                                        .max(6.))
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
