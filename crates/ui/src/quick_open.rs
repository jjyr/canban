//! One Quick Open and Project picker, built from local metadata and NSH Command.
mod model;
use crate::presenter::*;
use canban_ui_kit::*;
pub(crate) use model::github_search_fields;
pub use model::{Candidate, Target, candidates, candidates_with_tags, ranked};
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

struct DynamicCommand {
    id: String,
    title: String,
    detail: String,
    icon: Icon,
    keywords: String,
}

fn contextual_commands(ui: Ui, raw_query: &str) -> Vec<(usize, SearchChoice)> {
    let trimmed = raw_query.trim_start();
    let cmd_query = trimmed.strip_prefix('>').unwrap_or(trimmed).trim();
    let mut dynamic_cmds = Vec::<DynamicCommand>::new();

    let update = ui.update.get();
    if crate::updates::can_install_value(&update) && !ui.update_busy.get() {
        let ver = crate::updates::available_version(&update).unwrap_or("update");
        if update["state"]["phase"] == "error" {
            dynamic_cmds.push(DynamicCommand {
                id: "cmd:install-update".into(),
                title: format!("Retry Application Update ({ver})"),
                detail: "System · Updates".into(),
                icon: Icon::Refresh,
                keywords: "update upgrade retry download install restart".into(),
            });
        } else {
            dynamic_cmds.push(DynamicCommand {
                id: "cmd:install-update".into(),
                title: format!("Install Application Update ({ver}) and Restart"),
                detail: "System · Updates".into(),
                icon: Icon::Update,
                keywords: "update upgrade download install restart".into(),
            });
        }
    }

    if ui.page.get() == "review" {
        let st = ui.review_state;
        let collapsed = st.collapsed_files.get();
        for file in st
            .compare_data
            .with(|v| v["files"].as_array().cloned().unwrap_or_default())
        {
            let key = crate::review::model::file_key(&file);
            let path = file["filename"].as_str().unwrap_or("");
            let section = file["section"].as_str().unwrap_or("");
            let fold = if collapsed.contains(&key) {
                "Expand"
            } else {
                "Collapse"
            };
            for (action, title) in [
                ("git-fold", format!("Git: {fold} {section} {path}")),
                ("git-file", format!("Git: Reveal {section} {path}")),
            ] {
                dynamic_cmds.push(DynamicCommand {
                    id: format!("cmd:{action}:{key}"),
                    title,
                    detail: "Changes".into(),
                    icon: Icon::Compare,
                    keywords: "diff file collapse expand reveal".into(),
                });
            }
        }
        for checkout in st
            .refs_data
            .with(|v| v["checkouts"].as_array().cloned().unwrap_or_default())
        {
            let path = checkout["path"].as_str().unwrap_or("");
            dynamic_cmds.push(DynamicCommand {
                id: format!("cmd:git-checkout:{path}"),
                title: format!("Git: Select Worktree {path}"),
                detail: "Git".into(),
                icon: Icon::Branch,
                keywords: "worktree checkout directory".into(),
            });
        }
    }
    if let Some(card) = ui.current_card().filter(|c| c.active())
        && let Some(project) = ui
            .index
            .with(|idx| idx.projects.get(&card.project_id).cloned())
            .or_else(|| ui.current_project())
    {
        if let Some(next) = crate::card::next_stage_for_ui(ui, &card) {
            dynamic_cmds.push(DynamicCommand {
                id: "cmd:advance-card-stage".into(),
                title: format!("Advance Card #{} to {}", card.number, next.name),
                detail: format!("{} · Next Stage", project.name),
                icon: Icon::Board,
                keywords: format!(
                    "advance move next stage workflow {} #{}",
                    next.name, card.number
                ),
            });
        }
        for prof in ui.profiles.get().into_iter().filter(|p| p.enabled) {
            let assigned = card.assignee.as_deref() == Some(prof.id.as_str());
            dynamic_cmds.push(DynamicCommand {
                id: format!("cmd:assign-card:{}", prof.id),
                title: format!("Assign Card #{} to @{}", card.number, prof.id),
                detail: if assigned {
                    "Currently assigned".into()
                } else {
                    format!("{} · Auto-sync replies", project.name)
                },
                icon: Icon::Agent,
                keywords: format!("assign agent @{} #{}", prof.id, card.number),
            });
        }
        if let Some(cur) = card.assignee.as_deref() {
            dynamic_cmds.push(DynamicCommand {
                id: "cmd:unassign-card".into(),
                title: format!("Unassign @{cur} from Card #{}", card.number),
                detail: format!("{} · Clear Assignee", project.name),
                icon: Icon::Agent,
                keywords: format!("unassign clear remove agent @{cur} #{}", card.number),
            });
        }
        for stage in &project.workflow.stages {
            if stage.id != card.stage {
                dynamic_cmds.push(DynamicCommand {
                    id: format!("cmd:move-card:{}", stage.id),
                    title: format!("Move Card #{} to {}", card.number, stage.name),
                    detail: format!("{} · Card Stage", project.name),
                    icon: Icon::Board,
                    keywords: format!("move stage {} #{}", stage.name, card.number),
                });
            }
        }
    }

    let snapshot = ui.snapshot.get();
    let pid = ui.project.get();
    if !pid.is_empty() {
        for (kind, title) in [
            ("", "All cards"),
            ("local", "Local cards"),
            ("github", "GitHub cards"),
            ("issue", "GitHub Issues"),
            ("pr", "GitHub PRs"),
        ] {
            dynamic_cmds.push(DynamicCommand {
                id: format!("cmd:filter-board-type:{kind}"),
                title: format!("Filter Board Type: {title}"),
                detail: "Board filter".into(),
                icon: Icon::Board,
                keywords: "type source filter".into(),
            });
        }
        let authors: std::collections::BTreeSet<_> = snapshot
            .cards
            .iter()
            .filter(|c| c.project_id == pid && c.github_issue().is_none())
            .map(|c| c.created_by.clone())
            .chain(
                snapshot
                    .github_issues
                    .iter()
                    .filter(|v| v.project_id == pid)
                    .map(|v| v.author_login.clone()),
            )
            .filter(|a| !a.is_empty())
            .chain(std::iter::once(String::new()))
            .collect();
        for author in authors {
            dynamic_cmds.push(DynamicCommand {
                id: format!("cmd:filter-board-author:{author}"),
                title: format!(
                    "Filter Board Author: {}",
                    if author.is_empty() {
                        "Anyone".into()
                    } else {
                        format!("@{author}")
                    }
                ),
                detail: "Board filter".into(),
                icon: Icon::Board,
                keywords: "author filter".into(),
            });
        }
        let active = crate::tags::active_board_tags(ui, &pid);
        let project_name = snapshot
            .projects
            .iter()
            .find(|p| p.id == pid)
            .map(|p| p.name.as_str())
            .unwrap_or("Project");
        for tag in crate::tags::ordered_known(&snapshot, &ui.local_tags.get(), Some(&pid)) {
            let is_active = active
                .iter()
                .any(|t| t.to_lowercase() == tag.to_lowercase());
            dynamic_cmds.push(DynamicCommand {
                id: format!("cmd:filter-tag:{tag}"),
                title: if is_active {
                    format!("Clear #{tag} Board Filter")
                } else {
                    format!("Filter Board by #{tag}")
                },
                detail: format!("{project_name} · Board Tag Filter"),
                icon: Icon::Board,
                keywords: format!("tag filter board #{tag} {tag}"),
            });
        }
    }
    for wt in &snapshot.worktrees {
        let project_name = snapshot
            .projects
            .iter()
            .find(|p| p.id == wt.project_id)
            .map(|p| p.name.as_str())
            .unwrap_or("Project");
        dynamic_cmds.push(DynamicCommand {
            id: format!("cmd:worktree-shell:{}", wt.id),
            title: format!("Open Shell in Worktree {}", wt.branch),
            detail: format!("{project_name} · Worktrees"),
            icon: Icon::Terminal,
            keywords: format!("worktree shell terminal {}", wt.branch),
        });
        dynamic_cmds.push(DynamicCommand {
            id: format!("cmd:worktree-promote:{}", wt.id),
            title: format!("Promote Worktree {} to Board Card…", wt.branch),
            detail: format!("{project_name} · Worktrees"),
            icon: Icon::Board,
            keywords: format!("worktree promote card {}", wt.branch),
        });
        dynamic_cmds.push(DynamicCommand {
            id: format!("cmd:worktree-diff:{}", wt.id),
            title: format!("Compare Worktree {} in Git", wt.branch),
            detail: format!("{project_name} · Worktrees"),
            icon: Icon::Compare,
            keywords: format!("worktree git diff review compare {}", wt.branch),
        });
        dynamic_cmds.push(DynamicCommand {
            id: format!("cmd:worktree-delete:{}", wt.id),
            title: format!("Delete Worktree {}…", wt.branch),
            detail: format!("{project_name} · Worktrees"),
            icon: Icon::Branch,
            keywords: format!("worktree delete remove {}", wt.branch),
        });
    }
    for st in &snapshot.schedule_states {
        let project_name = if canban_types::is_global_project_id(&st.project_id) {
            "Global · Host"
        } else {
            snapshot
                .projects
                .iter()
                .find(|p| p.id == st.project_id)
                .map(|p| p.name.as_str())
                .unwrap_or("Project")
        };
        let name = if st.rule_id == "schedule" {
            st.workflow_name.clone()
        } else {
            format!("{} / {}", st.workflow_name, st.rule_id)
        };
        dynamic_cmds.push(DynamicCommand {
            id: format!("cmd:schedule-open:{}", st.id),
            title: format!("Open Schedule: {name}"),
            detail: format!("{project_name} · {}", st.cron),
            icon: Icon::Clock,
            keywords: format!("schedule cron open history {} {}", name, st.filename),
        });
        dynamic_cmds.push(DynamicCommand {
            id: format!("cmd:schedule-run:{}", st.id),
            title: format!("Run Schedule Now: {name}"),
            detail: format!("{project_name} · {}", st.cron),
            icon: Icon::Clock,
            keywords: format!("schedule cron run trigger now {} {}", name, st.filename),
        });
        dynamic_cmds.push(DynamicCommand {
            id: format!("cmd:schedule-toggle:{}", st.id),
            title: if st.enabled {
                format!("Pause Schedule: {name}")
            } else {
                format!("Enable Schedule: {name}")
            },
            detail: format!("{project_name} · {}", st.cron),
            icon: Icon::Clock,
            keywords: format!("schedule cron toggle pause enable {} {}", name, st.filename),
        });
    }

    dynamic_cmds
        .into_iter()
        .filter_map(|dc| {
            let fields = vec![dc.title.clone(), dc.detail.clone(), dc.keywords.clone()];
            let score = canban_ui_kit::search::score(&fields, cmd_query)?;
            Some((
                score,
                SearchChoice {
                    attention: 0,
                    selected: false,
                    id: dc.id,
                    title: dc.title,
                    detail: dc.detail,
                    icon: dc.icon,
                },
            ))
        })
        .collect()
}

pub(crate) fn command_mode_choices(ui: Ui, raw_query: &str) -> Vec<SearchChoice> {
    let contextual = contextual_commands(ui, raw_query);
    let has_dynamic_advance = contextual
        .iter()
        .any(|(_, ch)| ch.id == "cmd:advance-card-stage");
    let mut scored: Vec<(usize, usize, SearchChoice)> = matching_commands_with_scores(raw_query)
        .into_iter()
        .filter(|(_, c)| !(has_dynamic_advance && c.id == "advance-card-stage"))
        .enumerate()
        .map(|(idx, (score, c))| {
            (
                score,
                idx,
                SearchChoice {
                    attention: 0,
                    selected: false,
                    id: format!("cmd:{}", c.id),
                    title: c.title.into(),
                    detail: if c.display.is_empty() || c.display == c.context {
                        c.context.into()
                    } else {
                        format!("{} · {}", c.display, c.context)
                    },
                    icon: if c.id.contains("insights") {
                        Icon::Insights
                    } else if c.id.contains("schedule") {
                        Icon::Clock
                    } else if c.id.contains("agent") {
                        Icon::Agent
                    } else if c.id.contains("worktree") {
                        Icon::Branch
                    } else if c.id.contains("terminal") {
                        Icon::Terminal
                    } else if c.id.contains("notifications") || c.id == "mark-all-read" {
                        Icon::Overview
                    } else {
                        Icon::Keyboard
                    },
                },
            )
        })
        .collect();
    let base = scored.len();
    for (offset, (score, choice)) in contextual.into_iter().enumerate() {
        scored.push((score.saturating_add(1), base + offset, choice));
    }
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, ch)| ch).collect()
}

pub fn bindings(ui: Ui) {
    on_key_with(
        crate::commands::shortcuts::QUICK_OPEN_KEY,
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
            let projects =
                computed(move || crate::board::activity::model::projects(&ui.snapshot.get()));
            let all = computed(move || {
                let snapshot = ui.snapshot.get();
                let local_tags = ui.local_tags.get();
                let mut rows = candidates_with_tags(&snapshot, &local_tags);
                if ui.project_picker.get() {
                    for row in &mut rows {
                        if let Target::Project(_) = &row.target {
                            row.detail = row
                                .detail
                                .strip_prefix("Project · ")
                                .unwrap_or(&row.detail)
                                .into();
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
                    return command_mode_choices(ui, &raw_query);
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
                            "Worktree" => Icon::Branch,
                            "Schedule" => Icon::Clock,
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
                    if let Some(row) = candidates_with_tags(
                        &ui.snapshot.get_untracked(),
                        &ui.local_tags.get_untracked(),
                    )
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
                        "Search projects, cards, schedules, #tags, worktrees, agents… or type > for commands"
                            .into()
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
        Target::Worktree(wt_id) => {
            crate::terminals::worktrees::activate_scope(
                ui,
                &crate::terminals::worktrees::WorktreeScope::ManagedWorktree(wt_id),
            );
        }
        Target::Schedule(sid) => {
            let scope = ui.snapshot.with_untracked(|s| {
                s.schedule_states
                    .iter()
                    .find(|st| st.id == sid)
                    .map(|st| st.project_id.clone())
            });
            crate::schedules::open(ui, scope, Some(sid));
        }
    }
}
