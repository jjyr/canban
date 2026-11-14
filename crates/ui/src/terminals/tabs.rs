//! Window chrome and explicit actions. Menus carry secondary runtime details.

use super::*;
use crate::components::git_workspace::compact;
use nsh_core::element::desktop::{MenuItem, current_window, popup_menu, popup_menu_at};
pub fn context_updates(ui: Ui) {
    // Regrouping only reveals the selected row. It never changes the tab/viewer.
    let group =
        computed(move || current(ui).map(|e| (e.saved.id, e.project_id, e.card.map(|c| c.id))));
    effect(move || {
        if let Some((id, _, _)) = group.get() {
            reveal_group(ui, &id);
        }
    });
    let title = computed(move || {
        current(ui)
            .map(|e| {
                if e.agent.is_some() {
                    format!(
                        "{} · {} · {} — Canban",
                        e.title(),
                        e.stage.as_deref().unwrap_or_default(),
                        e.profile().unwrap_or_default()
                    )
                } else {
                    format!("{} — {} — Canban", e.title(), e.project_name)
                }
            })
            .unwrap_or_else(|| "Canban".into())
    });
    let window = current_window();
    effect(move || {
        let mut attributes = nsh_core::desktop::WindowAttributes::default();
        attributes.title = Some(title.get());
        window.set(attributes);
    });
}
pub async fn menu(ui: Ui, id: String, at: Option<Point<Points>>) {
    let Some(e) = ui
        .terminal_entries
        .with_untracked(|rows| rows.iter().find(|e| e.saved.id == id).cloned())
    else {
        return;
    };
    let rename = id.clone();
    let close = id.clone();
    let mut items = vec![
        MenuItem::new(e.hint()).disabled(true),
        MenuItem::separator(),
        MenuItem::new("Rename tab…")
            .on_select(move || ui.terminal_rename.set(Some(rename.clone()))),
        MenuItem::new("Close terminal").on_select(move || close_entry(ui, &close)),
    ];
    if let Some(card) = &e.card {
        let card = card.id.clone();
        items.push(MenuItem::new("Open Card").on_select(move || ui.terminal_card(card.clone())));
    }
    items.push(
        MenuItem::new(e.cwd().unwrap_or_else(|| "Directory unavailable".into())).disabled(true),
    );
    if let Some(branch) = e.location.as_ref().and_then(|c| c.branch.clone()) {
        let mut line = format!("Branch: {branch}");
        if let Some(loc) = &e.location {
            let ind = super::worktrees::format_git_indicators(loc.dirty, loc.ahead, loc.behind);
            if !ind.is_empty() {
                line.push_str(&format!(" ({})", ind.join(" · ")));
            }
        }
        items.push(MenuItem::new(line).disabled(true));
    }
    if let Some(agent) = &e.agent {
        if agent
            .agent()
            .is_some_and(|a| !a.finished() && a.terminal.is_some())
            && e.card.as_ref().is_some_and(|c| c.archived_at.is_none())
        {
            let terminal_id = e.saved.id.clone();
            items.push(MenuItem::new("Interrupt").on_select(move || {
                dispatch(ui, "agent.interrupt", json!({"terminal_id":terminal_id}))
            }));
        }
        items.push(
            MenuItem::new("Context and execution history").on_select(move || {
                ui.open_terminal(id.clone());
                ui.terminal_details.set(true);
            }),
        );
    } else if !e.ended() && e.binding().is_some() {
        let interrupt = e.saved.id.clone();
        items.push(MenuItem::new("Interrupt").on_select(move || {
            dispatch(ui, "terminal.interrupt", json!({"terminal_id":interrupt}))
        }));
    } else if e.ended() || e.inactive() {
        let id = e.saved.id.clone();
        items.push(MenuItem::new("Reopen terminal").on_select(move || reopen(ui, id.clone())));
    }
    if let Some(at) = at {
        popup_menu_at(items, at).await
    } else {
        popup_menu(items).await
    };
    ui.terminal_focus.update(|v| *v += 1);
}
fn anchored(ui: Ui, id: String, anchor: ElementRef) {
    ui.spawn(async move {
        if let Some(m) = anchor.measure().await
            && ui.page.get_untracked() == "terminals"
            && ui.terminal_selection.get_untracked() == id
        {
            menu(
                ui,
                id,
                Some(Point {
                    x: m.page.x,
                    y: px(m.page.y.0 + m.frame.size.height.0),
                }),
            )
            .await;
        }
    });
}
fn tab(ui: Ui, e: Entry, capacity: Memo<f32>) -> impl IntoElement {
    let id = e.saved.id.clone();
    let id_sel = id.clone();
    let is_selected = computed(move || ui.terminal_selection.get() == id_sel);
    let base_width = super::labels::tab_width(&e);
    dynamic(move || {
        let selected = is_selected.get();
        let id = id.clone();
        let close = id.clone();
        let context = id.clone();
        let rename = id.clone();
        let hint = e.hint();
        let attention = e.attention_reason();
        let mut tab_button = view()
            .h_full()
            .focusable(true)
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .px_2()
            .bg(CLEAR)
            .items_center()
            .justify_center()
            .role(AccessibilityRole::Tab)
            .cursor_pointer()
            .accessibility_state(move || nsh_core::AccessibilityState {
                selected,
                ..Default::default()
            })
            .label(format!("Tab {hint}"))
            .flex_row()
            .gap_2();
        if attention.is_some() {
            tab_button = tab_button.child(
                icon_tinted(Icon::Info, ATTENTION)
                    .w(px(13.))
                    .h(px(13.))
                    .flex_shrink_0(),
            );
        } else if e.agent.is_some() {
            tab_button = tab_button.child(
                view()
                    .w(px(5.))
                    .h(px(5.))
                    .rounded(px(3.))
                    .bg(if e.ended() || e.inactive() {
                        MUTED
                    } else {
                        SUCCESS
                    })
                    .flex_shrink_0()
                    .pointer_events(PointerEvents::None),
            );
        }
        tab_button = tab_button
            .child(super::labels::tab(&e, selected))
            .on_press_down(move |event: &nsh_core::PressEvent| {
                if event.clicks >= 2 {
                    ui.terminal_rename.set(Some(rename.clone()));
                }
            })
            .on_context_menu(move |event: &nsh_core::ContextMenuEvent| {
                ui.spawn(menu(ui, context.clone(), Some(event.page_position)));
            })
            .on_press(move |_: &nsh_core::PressEvent| ui.open_terminal(id.clone()));
        let mut cell = view()
            .relative()
            .w(move || px(base_width.min(capacity.get())))
            .h_full()
            .flex_shrink_0()
            .window_drag(false)
            .flex_row()
            .items_center()
            .gap_0()
            .px_1()
            .bg(if selected { SELECTED } else { CLEAR })
            .hover(move |s| s.bg(if selected { SELECTED } else { CARD_HOVER }))
            .border_r_1()
            .border_color(BORDER)
            .tooltip(hint_tooltip(hint.clone()).placement(Placement::Bottom))
            .child(tab_button)
            .child(chrome_button(
                Icon::Close,
                format!("Close terminal {}", e.title()),
                move |_| close_tab(ui, &close),
            ));
        if selected {
            cell = cell.child(
                view()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(2.))
                    .bg(ACCENT)
                    .pointer_events(PointerEvents::None),
            );
        }
        cell
    })
}
async fn switcher_menu(ui: Ui, at: Option<Point<Points>>) {
    use super::worktrees::{WorktreeScope, activate_scope, entry_scope, scope_git_summary};
    let s = ui.snapshot.get_untracked();
    let live_rows: Vec<Entry> = ui.closing_terminals.with_untracked(|closing| {
        ui.terminal_entries.with_untracked(|rows| {
            rows.iter()
                .filter(|e| e.listed() && !closing.contains(&e.saved.id))
                .cloned()
                .collect()
        })
    });
    let current_scope = untracked(|| current(ui)).map(|e| entry_scope(&s, &e));
    let mut items: Vec<MenuItem> = Vec::new();
    let mut idle_managed: Vec<(String, canban_types::ManagedWorktree)> = Vec::new();

    for proj in &s.projects {
        if !items.is_empty() {
            items.push(MenuItem::separator());
        }
        items.push(MenuItem::new(format!("📁 {}", proj.name)).disabled(true));

        let root_scope = WorktreeScope::ProjectRoot(proj.id.clone());
        let root_children: Vec<Entry> = live_rows
            .iter()
            .filter(|e| entry_scope(&s, e) == root_scope)
            .cloned()
            .collect();
        let root_git = scope_git_summary(&s, &root_scope, &root_children);
        let root_name = root_scope.display_name(&s, &root_children, Some(ui));
        let mut root_label = format!(
            "{}📁 {root_name} (Project root)",
            if current_scope.as_ref() == Some(&root_scope) {
                "✓ "
            } else {
                "   "
            }
        );
        let root_ind = root_git.indicators();
        if !root_ind.is_empty() {
            root_label.push_str(&format!(" · {}", root_ind.join(" ")));
        }
        if !root_children.is_empty() {
            root_label.push_str(&format!(" · {} live", root_children.len()));
        }
        let scope_sel = root_scope.clone();
        items.push(MenuItem::new(root_label).on_select(move || activate_scope(ui, &scope_sel)));

        let mut proj_cards: Vec<&canban_types::Card> = s
            .cards
            .iter()
            .filter(|c| {
                c.project_id == proj.id
                    && c.archived_at.is_none()
                    && (c.worktree.as_ref().is_some_and(|w| !w.is_empty())
                        || live_rows.iter().any(|e| {
                            entry_scope(&s, e) == WorktreeScope::CardWorktree(c.id.clone())
                        }))
            })
            .collect();
        proj_cards.sort_by_key(|c| (c.number, c.id.clone()));
        for card in proj_cards {
            let c_scope = WorktreeScope::CardWorktree(card.id.clone());
            let c_children: Vec<Entry> = live_rows
                .iter()
                .filter(|e| entry_scope(&s, e) == c_scope)
                .cloned()
                .collect();
            let c_git = scope_git_summary(&s, &c_scope, &c_children);
            let c_branch = c_scope.display_name(&s, &c_children, Some(ui));
            let mut c_label = format!(
                "{}⎇ {} · #{} {}",
                if current_scope.as_ref() == Some(&c_scope) {
                    "✓ "
                } else {
                    "   "
                },
                c_branch,
                card.number,
                compact(&card.title, 28)
            );
            let c_ind = c_git.indicators();
            if !c_ind.is_empty() {
                c_label.push_str(&format!(" · {}", c_ind.join(" ")));
            }
            if !c_children.is_empty() {
                c_label.push_str(&format!(" · {} live", c_children.len()));
            }
            let scope_sel = c_scope.clone();
            items.push(MenuItem::new(c_label).on_select(move || activate_scope(ui, &scope_sel)));
        }

        for wt in s.worktrees.iter().filter(|w| w.project_id == proj.id) {
            let wt_scope = WorktreeScope::ManagedWorktree(wt.id.clone());
            let wt_children: Vec<Entry> = live_rows
                .iter()
                .filter(|e| entry_scope(&s, e) == wt_scope)
                .cloned()
                .collect();
            let wt_git = scope_git_summary(&s, &wt_scope, &wt_children);
            let mut wt_label = format!(
                "{}⎇ {}",
                if current_scope.as_ref() == Some(&wt_scope) {
                    "✓ "
                } else {
                    "   "
                },
                wt.branch
            );
            let wt_ind = wt_git.indicators();
            if !wt_ind.is_empty() {
                wt_label.push_str(&format!(" · {}", wt_ind.join(" ")));
            }
            if wt_children.is_empty() {
                wt_label.push_str(" · idle");
                idle_managed.push((proj.name.clone(), wt.clone()));
            } else {
                wt_label.push_str(&format!(" · {} live", wt_children.len()));
            }
            let scope_sel = wt_scope.clone();
            items.push(MenuItem::new(wt_label).on_select(move || activate_scope(ui, &scope_sel)));
        }
    }

    let other_scopes = super::worktrees::project_scopes_in_tree_order(&s, &live_rows, None);
    if !other_scopes.is_empty() {
        if !items.is_empty() {
            items.push(MenuItem::separator());
        }
        items.push(MenuItem::new("Other").disabled(true));
        for (u_scope, u_children) in other_scopes {
            let name = u_scope.display_name(&s, &u_children, Some(ui));
            let label_str = format!(
                "{}{} · {} live",
                if current_scope.as_ref() == Some(&u_scope) {
                    "✓ "
                } else {
                    "   "
                },
                name,
                u_children.len()
            );
            let scope_sel = u_scope.clone();
            items.push(MenuItem::new(label_str).on_select(move || activate_scope(ui, &scope_sel)));
        }
    }

    if !idle_managed.is_empty() {
        items.push(MenuItem::separator());
        items.push(MenuItem::new("Idle Managed Worktrees").disabled(true));
        for (proj_name, wt) in idle_managed {
            let wt_diff = wt.clone();
            let wt_promote = wt.id.clone();
            let wt_del = wt.id.clone();
            items.push(
                MenuItem::new(format!("Compare ⎇ {} ({proj_name})", wt.branch))
                    .on_select(move || super::worktrees::open_diff(ui, &wt_diff)),
            );
            items.push(
                MenuItem::new(format!("Promote ⎇ {} to Board Card…", wt.branch))
                    .on_select(move || ui.worktree_promote.set(Some(wt_promote.clone()))),
            );
            items.push(
                MenuItem::new(format!("Delete ⎇ {}…", wt.branch))
                    .on_select(move || super::worktrees::request_remove(ui, wt_del.clone(), false)),
            );
        }
    }

    if !s.projects.is_empty() {
        items.push(MenuItem::separator());
        let active_pid = {
            let pid = ui.project.get_untracked();
            if !pid.is_empty() {
                pid
            } else {
                s.projects[0].id.clone()
            }
        };
        items.push(
            MenuItem::new("New Managed Branch Worktree…")
                .on_select(move || ui.worktree_modal.set(Some(active_pid.clone()))),
        );
    } else {
        items.push(
            MenuItem::new("Create New Project…").on_select(move || ui.navigate("new-project")),
        );
    }

    if let Some(pos) = at {
        popup_menu_at(items, pos).await;
    } else {
        popup_menu(items).await;
    }
    ui.terminal_focus.update(|v| *v += 1);
}

pub fn header(ui: Ui) -> impl IntoElement {
    let active_scope = computed(move || {
        let cur = current(ui)?;
        Some(ui.snapshot.with(|s| super::worktrees::entry_scope(s, &cur)))
    });
    let inventory = computed(move || {
        let Some(target_scope) = active_scope.get() else {
            return Vec::new();
        };
        let s = ui.snapshot.get();
        ui.closing_terminals.with(|closing| {
            ui.terminal_entries.with(|rows| {
                rows.iter()
                    .filter(|e| {
                        e.listed()
                            && !closing.contains(&e.saved.id)
                            && super::worktrees::entry_scope(&s, e) == target_scope
                    })
                    .map(|e| {
                        (
                            e.saved.id.clone(),
                            e.title(),
                            e.hint(),
                            e.ended(),
                            e.inactive(),
                            e.attention_reason(),
                            e.provider(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
        })
    });
    let context = computed(move || {
        (
            ui.project.get(),
            active_scope.get(),
            ui.snapshot.with(|s| {
                (
                    s.projects.len(),
                    s.worktrees.len(),
                    s.projects
                        .iter()
                        .map(|p| (p.id.clone(), p.name.clone(), p.target_branch.clone()))
                        .collect::<Vec<_>>(),
                    s.terminal_contexts
                        .iter()
                        .map(|c| {
                            (
                                c.terminal_id.clone(),
                                c.branch.clone(),
                                c.dirty,
                                c.ahead,
                                c.behind,
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            }),
            current(ui).map(|e| {
                (
                    e.saved.id.clone(),
                    e.card.clone(),
                    e.project_id.clone(),
                    e.location.clone(),
                )
            }),
        )
    });
    let width = signal(1000f32);
    let capacity = computed(move || {
        let w = width.get();
        let controls = if ui.terminal_sidebar.get() {
            0.
        } else {
            96. + (crate::frame::chrome_left_padding() - 56.).max(0.)
        };
        (w - (w * 0.35).min(340.) - 196. - controls).max(40.)
    });
    let strip = element_ref();
    let active = computed(move || ui.terminal_selection.get());
    effect(move || {
        let selected = active.get();
        let tabs = super::scope_tabs(ui);
        let cap = capacity.get();
        let _ = inventory.get();
        let mut offset = 0.;
        for e in &tabs {
            if e.saved.id == selected {
                break;
            }
            offset += super::labels::tab_width(e).min(cap);
        }
        strip.scroll_to(
            Point {
                x: px(offset),
                y: px(0.),
            },
            false,
        );
    });
    view()
        .h(px(42.))
        .on_layout(move |b: &Bounds<Points>| width.set(b.size.width.0))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .gap_1()
        .bg(INSET)
        .border_b_1()
        .border_color(BORDER)
        .window_drag(true)
        .pl(move || {
            px(if ui.terminal_sidebar.get() {
                0.
            } else {
                (crate::frame::chrome_left_padding() - 56.).max(0.)
            })
        })
        .pr(px(10.))
        .child(when(
            move || !ui.terminal_sidebar.get(),
            move || super::controls(ui),
        ))
        .child(
            scroll()
                .horizontal()
                .scrollbar(false)
                .element_ref(&strip)
                .w(move || {
                    let cap = capacity.get();
                    let total = super::scope_tabs(ui)
                        .iter()
                        .map(|e| super::labels::tab_width(e).min(cap))
                        .sum::<f32>();
                    px(total.min(cap).max(0.))
                })
                .min_w(px(0.))
                .h_full()
                .child(dynamic(move || {
                    let _ = inventory.get();
                    let tabs = untracked(|| super::scope_tabs(ui));
                    let mut strip = view().h_full().flex_row().items_center().window_drag(true);
                    for e in tabs {
                        strip = strip.child(tab(ui, e, capacity));
                    }
                    strip
                })),
        )
        .child(chrome_button_disabled(
            Icon::Plus,
            "New terminal in current worktree · ⌘T",
            move || ui.pending.get(),
            move |_| new_tab(ui),
        ))
        .child(
            action("▾", Variant::Quiet, move |_| machine_menu(ui))
                .accessibility_label(|| Some("Choose terminal worktree or machine".into())),
        )
        .child(chrome_button_disabled(
            Icon::Agent,
            "Open Agent",
            move || ui.pending.get(),
            move |_| open_ephemeral_agent(ui),
        ))
        .child(view().flex_1().min_w(px(24.)).h_full().window_drag(true))
        .child(dynamic(move || {
            let _ = context.get();
            let s = ui.snapshot.get_untracked();
            let cur = untracked(|| current(ui));
            let scope_tabs_now = untracked(|| super::scope_tabs(ui));
            let switcher_anchor = element_ref();
            let more_anchor = element_ref();

            let mut context_bar = view()
                .window_drag(false)
                .flex_row()
                .items_center()
                .justify_end()
                .gap_1p5()
                .min_w(px(0.))
                .w(move || px((width.get() * 0.35).min(340.)))
                .flex_shrink_0();

            if let Some(ref e) = cur {
                if let Some(card) = e.card.clone() {
                    context_bar = context_bar.child(
                        view()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .tooltip(
                                hint_tooltip(format!(
                                    "#{} {}\n{}",
                                    card.number,
                                    card.title,
                                    if e.inferred_card {
                                        "Card inferred from directory or branch"
                                    } else {
                                        "Open task Card"
                                    }
                                ))
                                .placement(Placement::Bottom),
                            )
                            .child(
                                canban_ui_kit::press_target()
                                    .w_full()
                                    .min_w(px(0.))
                                    .h(px(30.))
                                    .justify_center()
                                    .px_2()
                                    .rounded(px(5.))
                                    .bg(CLEAR)
                                    .hover(|st| st.bg(SURFACE))
                                    .focus(|st| st.border_1().border_color(ACCENT))
                                    .child(
                                        label(format!("#{} {}", card.number, card.title))
                                            .text_color(if e.inferred_card { MUTED } else { TEXT })
                                            .w_full()
                                            .truncate(),
                                    )
                                    .accessibility_label(move || Some("Open current Card".into()))
                                    .on_press(move |_: &()| ui.terminal_card(card.id.clone())),
                            ),
                    );
                } else if let Some(project) = e.project_id.clone() {
                    context_bar = context_bar.child(
                        canban_ui_kit::press_target()
                            .flex_1()
                            .min_w(px(0.))
                            .h(px(30.))
                            .px_2()
                            .rounded(px(5.))
                            .bg(CLEAR)
                            .hover(|st| st.bg(SURFACE))
                            .focus(|st| st.border_1().border_color(ACCENT))
                            .child(label(e.project_name.clone()).w_full().truncate())
                            .accessibility_label(move || Some("Open current Project".into()))
                            .on_press(move |_: &()| ui.select_project(project.clone())),
                    );
                } else if let Some(cwd) = e.cwd() {
                    let home = std::env::var("HOME").unwrap_or_default();
                    let title = if !home.is_empty() {
                        std::path::Path::new(&cwd)
                            .strip_prefix(&home)
                            .ok()
                            .map(|p| {
                                if p.as_os_str().is_empty() {
                                    "~".into()
                                } else {
                                    format!("~/{}", p.display())
                                }
                            })
                            .unwrap_or(cwd)
                    } else {
                        cwd
                    };
                    context_bar = context_bar.child(
                        view()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .tooltip(
                                hint_tooltip(e.cwd().unwrap_or_default())
                                    .placement(Placement::Bottom),
                            )
                            .child(
                                muted(compact(&title, if ui.narrow.get() { 14 } else { 32 }))
                                    .w_full()
                                    .truncate(),
                            ),
                    );
                }
            }

            // Worktree Switcher pill (always available when projects/worktrees or a live terminal exist)
            let has_switcher_targets =
                cur.is_some() || !s.projects.is_empty() || !s.worktrees.is_empty();
            if has_switcher_targets {
                let (pill_icon, pill_branch, pill_git, pill_tooltip) = if let Some(ref e) = cur {
                    let scope = super::worktrees::entry_scope(&s, e);
                    let git = super::worktrees::scope_git_summary(&s, &scope, &scope_tabs_now);
                    let name = scope.display_name(&s, &scope_tabs_now, Some(ui));
                    let mut tip = format!(
                        "Switch Project / Worktree\nCurrent: {name}\nDirectory: {}",
                        e.cwd().unwrap_or_else(|| "Unavailable".into())
                    );
                    let ind = git.indicators();
                    if !ind.is_empty() {
                        tip.push_str(&format!("\nGit: {}", ind.join(" · ")));
                    }
                    let ic = match scope {
                        super::worktrees::WorktreeScope::ProjectRoot(_) => Icon::Projects,
                        super::worktrees::WorktreeScope::Unassigned(_) => Icon::Terminal,
                        _ => Icon::Branch,
                    };
                    (ic, name, git, tip)
                } else {
                    let pid = ui.project.get_untracked();
                    let proj = s
                        .projects
                        .iter()
                        .find(|p| p.id == pid)
                        .or_else(|| s.projects.first());
                    let name = proj
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| "Worktrees".into());
                    (
                        Icon::Branch,
                        name.clone(),
                        super::worktrees::ScopeGitSummary::default(),
                        format!("Switch Project / Worktree ({name})"),
                    )
                };

                let limit = if ui.narrow.get() { 10 } else { 18 };
                let short = compact(
                    &pill_branch,
                    if pill_branch.is_ascii() {
                        limit
                    } else {
                        limit / 2
                    },
                );
                let mut pill_row = canban_ui_kit::press_target()
                    .h(px(30.))
                    .max_w(move || {
                        px((if ui.narrow.get() { 130f32 } else { 210f32 })
                            .min((width.get() * 0.4).min(420.) - 44.)
                            .max(56.))
                    })
                    .min_w(px(0.))
                    .px_2()
                    .rounded(px(5.))
                    .flex_row()
                    .gap_1()
                    .items_center()
                    .bg(CLEAR)
                    .hover(|st| st.bg(SURFACE))
                    .focus(|st| st.border_1().border_color(ACCENT))
                    .accessibility_label({
                        let b = pill_branch.clone();
                        move || Some(format!("Switch worktree ({b})").into())
                    })
                    .child(icon(pill_icon).w(px(13.)).h(px(13.)).flex_shrink_0())
                    .child(muted(short).min_w(px(0.)).truncate());

                if pill_git.dirty {
                    pill_row = pill_row.child(
                        label("changed")
                            .text_size(px(11.))
                            .text_color(ATTENTION)
                            .flex_shrink_0(),
                    );
                }
                if pill_git.ahead > 0 || pill_git.behind > 0 {
                    let mut ab = Vec::new();
                    if pill_git.ahead > 0 {
                        ab.push(format!("↑{}", pill_git.ahead));
                    }
                    if pill_git.behind > 0 {
                        ab.push(format!("↓{}", pill_git.behind));
                    }
                    pill_row =
                        pill_row.child(muted(ab.join(" ")).text_size(px(11.)).flex_shrink_0());
                }
                pill_row = pill_row
                    .child(
                        icon(Icon::ChevronDown)
                            .w(px(12.))
                            .h(px(12.))
                            .flex_shrink_0(),
                    )
                    .on_press(move |_: &()| {
                        ui.spawn(async move {
                            let pos = switcher_anchor
                                .measure()
                                .await
                                .map(|b| point(b.page.x, b.page.y + b.frame.size.height));
                            switcher_menu(ui, pos).await;
                        });
                    });

                context_bar = context_bar.child(
                    view()
                        .flex_shrink_0()
                        .min_w(px(0.))
                        .element_ref(&switcher_anchor)
                        .tooltip(hint_tooltip(pill_tooltip).placement(Placement::Bottom))
                        .child(pill_row),
                );
            }

            if let Some(e) = cur {
                let id = e.saved.id.clone();
                context_bar = context_bar.child(
                    chrome_button(Icon::More, "Terminal actions", move |_| {
                        anchored(ui, id.clone(), more_anchor)
                    })
                    .element_ref(&more_anchor),
                );
            }
            context_bar
        }))
        .child(crate::overview::trigger_pill(ui))
}
pub fn rename_dialog(ui: Ui) -> impl IntoElement {
    when(
        move || ui.terminal_rename.get().is_some(),
        move || {
            let id = ui.terminal_rename.get_untracked().unwrap();
            let initial = ui
                .terminal_entries
                .with_untracked(|rows| {
                    rows.iter()
                        .find(|e| e.saved.id == id)
                        .and_then(|e| e.saved.name.clone())
                })
                .unwrap_or_default();
            let name = signal(initial);
            let input = element_ref();
            on_key_with(
                "mod+a",
                KeyOptions {
                    when: nsh_core::keymap::When::Always,
                    ..Default::default()
                },
                move |_| input.set_selection(0, name.get_untracked().encode_utf16().count() as u32),
            );
            let error = signal(String::new());
            let close_rename = move || {
                ui.terminal_rename.set(None);
                ui.terminal_focus.update(|n| *n += 1);
            };
            let save = move || {
                let next = name.get_untracked();
                if next.chars().count() > 80 || next.chars().any(char::is_control) {
                    error.set("Use at most 80 printable characters".into());
                    return;
                }
                if crate::APP.get().is_none() {
                    ui.snapshot.update(|s| {
                        if let Some(t) = s.terminals.iter_mut().find(|t| t.id == id) {
                            t.name = (!next.trim().is_empty()).then(|| next.trim().into());
                        }
                    });
                    close_rename();
                    return;
                }
                let source = id.clone();
                dispatch_complete(
                    ui,
                    "terminal.rename",
                    json!({"terminal_id":id,"name":next}),
                    move |r| {
                        if ui.terminal_rename.get_untracked().as_deref() != Some(source.as_str()) {
                            return;
                        }
                        match r {
                            Ok(_) => close_rename(),
                            Err(e) => {
                                let _ = error.try_set(e);
                            }
                        }
                    },
                );
            };
            let enter = save.clone();
            nsh_base::dialog()
                .animation(canban_ui_kit::motion::modal)
                .close_on_escape(true)
                .close_on_backdrop_press(true)
                .on_cancel(close_rename)
                .child(
                    view()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x00000055))
                        .child(
                            panel()
                                .w(px(360.))
                                .border_1()
                                .border_color(BORDER)
                                .child(
                                    view()
                                        .flex_row()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            label("Rename tab").font_weight(FontWeight::SEMIBOLD),
                                        )
                                        .child(chrome_button(
                                            Icon::Close,
                                            "Close rename dialog",
                                            move |_| close_rename(),
                                        )),
                                )
                                .child(
                                    field(name, "Custom name (optional)")
                                        .element_ref(&input)
                                        .on_focus(move || {
                                            input.set_selection(
                                                0,
                                                name.get_untracked().encode_utf16().count() as u32,
                                            )
                                        })
                                        .label("Custom name (optional)")
                                        .auto_focus(true)
                                        .on_submit(move |_: &str| enter()),
                                )
                                .child(muted("Leave empty to restore its automatic title."))
                                .child(muted(move || error.get()))
                                .child(
                                    view()
                                        .flex_row()
                                        .gap_2()
                                        .child(action("Save", Variant::Primary, move |_| save()))
                                        .child(action("Cancel", Variant::Quiet, move |_| {
                                            close_rename()
                                        })),
                                ),
                        ),
                )
        },
    )
}

pub fn close_confirm_dialog(ui: Ui) -> impl IntoElement {
    when(
        move || ui.terminal_close_confirm.get().is_some(),
        move || {
            let id = ui
                .terminal_close_confirm
                .get_untracked()
                .unwrap_or_default();
            let (title, subtitle, reason) = ui.snapshot.with_untracked(|s| {
                let entry = ui
                    .terminal_entries
                    .with_untracked(|rows| rows.iter().find(|e| e.saved.id == id).cloned());
                let title = entry
                    .as_ref()
                    .map(|e| format!("Close {}?", e.title()))
                    .unwrap_or_else(|| "Close terminal?".into());
                let subtitle = entry.as_ref().and_then(|e| {
                    if e.agent.is_some() {
                        Some(format!(
                            "{} · {} · @{}",
                            e.project_name,
                            e.stage.as_deref().unwrap_or("Stage"),
                            e.profile().unwrap_or("agent")
                        ))
                    } else if let Some(card) = &e.card {
                        Some(format!("{} · #{} {}", e.project_name, card.number, card.title))
                    } else if e.project_id.is_some() {
                        Some(e.project_name.clone())
                    } else {
                        None
                    }
                });
                let reason = entry
                    .and_then(|e| e.close_confirm_reason(s))
                    .unwrap_or_else(|| {
                        "An Agent is still running in this terminal and is not done. Closing this terminal terminates its live process.".into()
                    });
                (title, subtitle, reason)
            });
            let dismiss = move || {
                ui.terminal_close_confirm.set(None);
                ui.terminal_focus.update(|n| *n += 1);
            };
            let confirm_id = id.clone();
            let confirm_id_pending = id.clone();
            let mut body = panel()
                .w(px(420.))
                .max_w(Percentage(0.95))
                .border_1()
                .border_color(BORDER)
                .gap_3()
                .child(
                    view()
                        .flex_row()
                        .justify_between()
                        .items_center()
                        .gap_2()
                        .child(
                            label(title)
                                .font_weight(FontWeight::SEMIBOLD)
                                .flex_1()
                                .min_w(px(0.))
                                .truncate(),
                        )
                        .child(chrome_button(
                            Icon::Close,
                            "Cancel terminal close",
                            move |_| dismiss(),
                        )),
                );
            if let Some(sub) = subtitle {
                body = body.child(muted(sub).w_full().min_w(px(0.)).truncate());
            }
            body = body.child(notice(reason, Tone::Attention)).child(
                view()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .child(action("Cancel", Variant::Quiet, move |_| dismiss()))
                    .child(
                        action("Close terminal", Variant::Danger, move |_| {
                            close_entry_confirmed(ui, &confirm_id)
                        })
                        .disabled(move || {
                            ui.closing_terminals
                                .with(|c| c.contains(&confirm_id_pending))
                        }),
                    ),
            );
            nsh_base::dialog()
                .animation(canban_ui_kit::motion::modal)
                .close_on_escape(true)
                .close_on_backdrop_press(true)
                .on_cancel(dismiss)
                .child(
                    view()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x00000055))
                        .child(body),
                )
        },
    )
}
