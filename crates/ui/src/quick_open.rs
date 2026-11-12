//! One Quick Open and Project picker, built from local metadata and NSH Command.
mod model;
use crate::presenter::*;
use canban_ui_kit::*;
pub use model::{Candidate, Target, candidates, ranked};
use nsh_core::{keymap::When, prelude::*};

pub fn matching_commands_with_scores(
    raw_query: &str,
) -> Vec<(usize, &'static crate::commands::Command)> {
    let trimmed = raw_query.trim_start();
    let cmd_query = trimmed.strip_prefix('>').unwrap_or(trimmed).trim();
    let mut scored = crate::commands::CATALOG
        .iter()
        .enumerate()
        .filter_map(|(idx, c)| {
            let fields = vec![
                c.title.to_owned(),
                c.context.to_owned(),
                c.display.to_owned(),
                c.keywords.to_owned(),
            ];
            let score = canban_ui_kit::search::score(&fields, cmd_query)?;
            Some((score, idx, c))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(score, _, c)| (score, c)).collect()
}

pub fn matching_commands(raw_query: &str) -> Vec<&'static crate::commands::Command> {
    matching_commands_with_scores(raw_query)
        .into_iter()
        .map(|(_, c)| c)
        .collect()
}

pub fn is_command_query(raw_query: &str) -> bool {
    raw_query.trim_start().starts_with('>')
}

pub fn bindings(ui: Ui) {
    on_key_with(
        "mod+o",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            ui.utility_popup.set(String::new());
            ui.show_search(false)
        },
    );
}
pub fn overlay(ui: Ui) -> impl IntoElement {
    when(
        move || ui.quick_open.get(),
        move || {
            let projects = computed(move || {
                crate::board::activity::model::projects(
                    &ui.snapshot.get(),
                    &ui.recent_destinations.get(),
                )
            });
            let all = computed(move || {
                let snapshot = ui.snapshot.get();
                let mut rows = candidates(&snapshot);
                if ui.project_picker.get() {
                    for row in &mut rows {
                        if let Target::Project(id) = &row.target
                            && let Some(project) =
                                projects.with(|ps| ps.iter().find(|p| &p.id == id).cloned())
                        {
                            row.detail = if snapshot
                                .projects
                                .iter()
                                .filter(|p| p.name == project.name)
                                .count()
                                > 1
                            {
                                // Duplicate names still need their directory to distinguish them.
                                row.detail
                                    .strip_prefix("Project · ")
                                    .unwrap_or(&row.detail)
                                    .into()
                            } else {
                                String::new()
                            };
                        }
                    }
                }
                rows
            });
            let rows = computed(move || {
                let mut recent = ui.recent_destinations.with(|v| v.clone());
                if ui.project_picker.get() && ui.search_query.get().trim().is_empty() {
                    for p in projects.get() {
                        let id = format!("project:{}", p.id);
                        if !recent.contains(&id) {
                            recent.push(id);
                        }
                    }
                }
                ranked(
                    &all.get(),
                    &ui.search_query.get(),
                    &recent,
                    ui.project_picker.get(),
                )
            });
            let choices = computed(move || {
                let raw_query = ui.search_query.get();
                let command_mode = !ui.project_picker.get() && is_command_query(&raw_query);
                if command_mode {
                    return matching_commands(&raw_query)
                        .into_iter()
                        .map(|c| SearchChoice {
                            attention: 0,
                            selected: false,
                            id: format!("cmd:{}", c.id),
                            title: c.title.into(),
                            detail: if c.display.is_empty() {
                                c.context.into()
                            } else {
                                format!("{} · {}", c.display, c.context)
                            },
                            icon: if c.id.contains("agent") {
                                Icon::Agent
                            } else {
                                Icon::Keyboard
                            },
                        })
                        .collect();
                }
                rows.get()
                    .into_iter()
                    .map(|r| SearchChoice {
                        attention: if ui.project_picker.get() {
                            if let Target::Project(id) = &r.target {
                                projects.with(|ps| {
                                    ps.iter()
                                        .find(|p| &p.id == id)
                                        .map(|p| p.attention)
                                        .unwrap_or(0)
                                })
                            } else {
                                0
                            }
                        } else {
                            0
                        },
                        selected: ui.project_picker.get()
                            && matches!(&r.target, Target::Project(id) if *id == ui.project.get()),
                        id: r.id,
                        title: r.title,
                        detail: r.detail,
                        icon: match r.kind {
                            "Project" => Icon::Projects,
                            "Card" => Icon::Board,
                            "Terminal" => Icon::Terminal,
                            _ => Icon::Agent,
                        },
                    })
                    .collect()
            });
            search_dialog(
                move || {
                    if ui.project_picker.get() {
                        "Switch Project".into()
                    } else if is_command_query(&ui.search_query.get()) {
                        "Commands & Shortcuts".into()
                    } else {
                        "Quick Open".into()
                    }
                },
                ui.search_query,
                choices,
                move |id| {
                    if let Some(cmd_id) = id.strip_prefix("cmd:") {
                        crate::commands::execute(ui, cmd_id);
                        return;
                    }
                    // Resolve against fresh local state, rather than an obsolete retained attempt.
                    if let Some(row) = candidates(&ui.snapshot.get_untracked())
                        .into_iter()
                        .find(|r| r.id == id)
                    {
                        activate(ui, row);
                    } else {
                        ui.notify("This destination is no longer available");
                        ui.quick_open.set(false);
                    }
                },
                move || ui.quick_open.set(false),
                move || {
                    if ui.project_picker.get() {
                        "Search projects…".into()
                    } else {
                        "Search projects, cards, agents… or type > for commands".into()
                    }
                },
                view()
                    .gap_2()
                    .child(muted(move || {
                        format!(
                            "{} results · ↑ ↓ choose · Enter run · > commands · Esc close",
                            choices.get().len()
                        )
                    }))
                    .child(when(
                        move || ui.project_picker.get(),
                        move || {
                            action("+ New Project", Variant::Quiet, move |_| {
                                ui.quick_open.set(false);
                                ui.navigate("new-project");
                            })
                        },
                    )),
            )
        },
    )
}
/// Header summaries and picker selections share fresh-target resolution/history.
pub fn switch_project(ui: Ui, id: &str) {
    if let Some(row) = candidates(&ui.snapshot.get_untracked())
        .into_iter()
        .find(|r| matches!(&r.target, Target::Project(project) if project == id))
    {
        activate(ui, row);
    }
}
pub fn activate(ui: Ui, row: Candidate) {
    ui.quick_open.set(false);
    ui.recent_destinations.with(|ids| {
        ids.retain(|id| id != &row.id);
        ids.insert(0, row.id.clone());
        ids.truncate(32);
    });
    match row.target {
        Target::Project(id) => ui.select_project(id),
        Target::Card(id) => {
            ui.open_card(id);
        }
        Target::Agent(id) | Target::Terminal(id) => ui.open_terminal(id),
    }
}
