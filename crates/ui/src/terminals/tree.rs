//! Unified Project -> Worktree -> Terminal hierarchy.
//! Only worktree scopes with >= 1 live Terminal are displayed; all active rows stay expanded.
use super::worktrees::{
    WorktreeScope, activate_scope, create_in_scope, entry_scope, project_scopes_in_tree_order,
    scope_git_summary,
};
use super::*;
use crate::drafts::{capture_scroll, scroll_position};

fn git_status_chips(git: &super::worktrees::ScopeGitSummary) -> View {
    let mut row = view().flex_row().items_center().gap_1().flex_shrink_0();
    if git.dirty {
        row = row
            .child(
                label(format!("+{}", git.additions))
                    .text_size(px(11.))
                    .font_family("Menlo")
                    .text_color(SUCCESS),
            )
            .child(
                label(format!("−{}", git.deletions))
                    .text_size(px(11.))
                    .font_family("Menlo")
                    .text_color(DANGER),
            );
    }
    if git.ahead > 0 || git.behind > 0 {
        let mut ab = Vec::new();
        if git.ahead > 0 {
            ab.push(format!("↑{}", git.ahead));
        }
        if git.behind > 0 {
            ab.push(format!("↓{}", git.behind));
        }
        row = row.child(muted(ab.join(" ")).text_size(px(11.)));
    }
    row
}

fn scope_header(
    ui: Ui,
    s: &canban_types::Snapshot,
    scope: WorktreeScope,
    children: &[Entry],
) -> impl IntoElement {
    let git = scope_git_summary(s, &scope, children);
    let display = scope.display_name(s, children, Some(ui));
    let scope_for_active = scope.clone();
    let is_active_scope = computed(move || {
        let sel = ui.terminal_selection.get();
        if sel.is_empty() || ui.closing_terminals.with(|c| c.contains(&sel)) {
            return false;
        }
        ui.snapshot.with(|snap| {
            ui.terminal_entries.with(|rows| {
                rows.iter()
                    .find(|e| e.saved.id == sel)
                    .is_some_and(|e| entry_scope(snap, e) == scope_for_active)
            })
        })
    });

    let (lead_icon, card_badge, tooltip_text) = match &scope {
        WorktreeScope::ProjectRoot(pid) => {
            let dir = s
                .projects
                .iter()
                .find(|p| &p.id == pid)
                .map(|p| p.directory.clone())
                .unwrap_or_default();
            let mut tip = format!("Project root · {display}");
            let ind = git.tooltip_indicators();
            if !ind.is_empty() {
                tip.push_str(&format!(" ({})", ind.join(" · ")));
            }
            if !dir.is_empty() {
                tip.push_str(&format!("\n{dir}"));
            }
            (Icon::Projects, None, tip)
        }
        WorktreeScope::CardWorktree(cid) => {
            let card = s.cards.iter().find(|c| &c.id == cid);
            let badge = card.map(|c| format!("#{}", c.number));
            let mut tip = match card {
                Some(c) => format!("#{} {}\nBranch: {display}", c.number, c.title),
                None => format!("Card worktree · {display}"),
            };
            let ind = git.tooltip_indicators();
            if !ind.is_empty() {
                tip.push_str(&format!(" ({})", ind.join(" · ")));
            }
            if let Some(wt) = card.and_then(|c| c.worktree.as_deref()) {
                tip.push_str(&format!("\n{wt}"));
            }
            (Icon::Branch, badge, tip)
        }
        WorktreeScope::ManagedWorktree(wid) => {
            let wt = s.worktrees.iter().find(|w| &w.id == wid);
            let mut tip = match wt {
                Some(w) => format!(
                    "Managed worktree `{}`\nBase: {}",
                    w.branch,
                    if w.base_branch.is_empty() {
                        "default"
                    } else {
                        w.base_branch.as_str()
                    }
                ),
                None => format!("Managed worktree `{display}`"),
            };
            let ind = git.tooltip_indicators();
            if !ind.is_empty() {
                tip.push_str(&format!(" ({})", ind.join(" · ")));
            }
            if let Some(w) = wt {
                tip.push_str(&format!("\n{}", w.path));
            }
            (Icon::Branch, None, tip)
        }
        WorktreeScope::Unassigned(machine) => {
            let tip = format!("External directory ({machine})");
            (Icon::Terminal, None, tip)
        }
    };

    let managed_wt = match &scope {
        WorktreeScope::ManagedWorktree(wid) => s.worktrees.iter().find(|w| &w.id == wid).cloned(),
        _ => None,
    };
    let scope_card = match &scope {
        WorktreeScope::CardWorktree(cid) => s.cards.iter().find(|c| &c.id == cid).cloned(),
        _ => None,
    };

    dynamic(move || {
        let active = is_active_scope.get();
        let scope_press = scope.clone();
        let scope_plus = scope.clone();
        let display_label = display.clone();
        let badge = card_badge.clone();
        let tip = tooltip_text.clone();
        let git_now = git.clone();
        let wt_ctx = managed_wt.clone();
        let wt_more = managed_wt.clone();
        let card_ctx = scope_card.clone();
        let card_more = scope_card.clone();
        let more_anchor = element_ref();

        let mut btn_row = canban_ui_kit::press_target()
            .w_full()
            .min_w(px(0.))
            .min_h(px(30.))
            .px_2()
            .rounded(px(5.))
            .bg(if active { INSET } else { CLEAR })
            .hover(|st| st.bg(INSET))
            .focus(|st| st.border_1().border_color(ACCENT))
            .flex_row()
            .items_center()
            .gap_1p5()
            .child(
                icon_tinted(lead_icon, if active { ACCENT } else { MUTED })
                    .w(px(13.))
                    .h(px(13.))
                    .flex_shrink_0(),
            )
            .child(
                label(display_label.clone())
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(12.5))
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .truncate(),
            );

        if let Some(b) = badge {
            btn_row = btn_row.child(muted(b).text_size(px(11.)).flex_shrink_0());
        }
        if git_now.dirty || git_now.ahead > 0 || git_now.behind > 0 {
            btn_row = btn_row.child(git_status_chips(&git_now));
        }
        btn_row = btn_row
            .accessibility_label({
                let lbl = display_label.clone();
                move || Some(format!("Worktree {lbl}").into())
            })
            .on_press(move |_: &()| activate_scope(ui, &scope_press));

        let mut outer = view()
            .ml_2()
            .flex_row()
            .items_center()
            .min_w(px(0.))
            .on_context_menu(move |ev: &nsh_core::ContextMenuEvent| {
                if let Some(wt) = wt_ctx.clone() {
                    ui.spawn(super::worktrees::menu(ui, wt, Some(ev.page_position)));
                } else if let Some(card) = card_ctx.clone() {
                    ui.spawn(super::worktrees::card_scope_menu(
                        ui,
                        card,
                        Some(ev.page_position),
                    ));
                }
            })
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.))
                    .tooltip(hint_tooltip(tip).placement(Placement::Right))
                    .child(btn_row),
            )
            .child(chrome_button_disabled(
                Icon::Plus,
                format!("New terminal in {display_label}"),
                move || ui.pending.get(),
                move |_| create_in_scope(ui, &scope_plus),
            ));

        if let Some(wt) = wt_more {
            outer = outer.child(
                chrome_button(
                    Icon::More,
                    format!("Worktree actions for {}", wt.branch),
                    move |_| {
                        let item = wt.clone();
                        ui.spawn(async move {
                            let pos = more_anchor
                                .measure()
                                .await
                                .map(|b| point(b.page.x, b.page.y + b.frame.size.height));
                            super::worktrees::menu(ui, item, pos).await;
                        });
                    },
                )
                .element_ref(&more_anchor),
            );
        } else if let Some(card) = card_more {
            outer = outer.child(
                chrome_button(
                    Icon::More,
                    format!("Card worktree actions for #{}", card.number),
                    move |_| {
                        let item = card.clone();
                        ui.spawn(async move {
                            let pos = more_anchor
                                .measure()
                                .await
                                .map(|b| point(b.page.x, b.page.y + b.frame.size.height));
                            super::worktrees::card_scope_menu(ui, item, pos).await;
                        });
                    },
                )
                .element_ref(&more_anchor),
            );
        }

        outer
    })
}
fn row(ui: Ui, e: Entry) -> impl IntoElement {
    let id = e.saved.id.clone();
    let id_sel = id.clone();
    let is_selected = computed(move || ui.terminal_selection.get() == id_sel);
    dynamic(move || {
        let selected = is_selected.get();
        let id = id.clone();
        let context_id = id.clone();
        let machine = if e.machine() == "local" {
            "Local".into()
        } else {
            crate::machines::name(ui, e.machine())
        };
        let hint = super::labels::sidebar_hint(&e, &machine);
        let short_title = super::labels::truncate_sidebar_title(&e.title());
        let first_line = view()
            .w_full()
            .min_w(px(0.))
            .flex_row()
            .items_center()
            .child(
                label(short_title)
                    .font_weight(if selected {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .flex_1()
                    .min_w(px(0.))
                    .truncate(),
            );
        let mut second_line = view()
            .w_full()
            .min_w(px(0.))
            .flex_row()
            .items_center()
            .gap_2()
            .child(super::labels::sidebar_meta(&e, &machine));
        if let Some(status_el) = super::labels::sidebar_status(&e) {
            second_line = second_line.child(status_el);
        }
        let content = view()
            .w_full()
            .min_w(px(0.))
            .gap_0p5()
            .child(first_line)
            .child(second_line);
        view()
            .on_context_menu(move |event: &nsh_core::ContextMenuEvent| {
                ui.spawn(super::tabs::menu(
                    ui,
                    context_id.clone(),
                    Some(event.page_position),
                ));
            })
            .tooltip(hint_tooltip(hint.clone()).placement(Placement::Right))
            .child(
                canban_ui_kit::press_target()
                    .w_full()
                    .h(px(46.))
                    .px_2()
                    .py_1()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .rounded(px(5.))
                    .bg(if selected { SELECTED } else { CLEAR })
                    .hover(move |s| s.bg(if selected { SELECTED } else { INSET }))
                    .focus(|s| s.border_1().border_color(ACCENT))
                    .selected(move || selected)
                    .accessibility_label(move || Some(hint.clone().into()))
                    .child(content)
                    .on_press(move |_: &()| ui.open_terminal(id.clone())),
            )
    })
}
pub fn sidebar(ui: Ui) -> impl IntoElement {
    // Runtime observation timestamps must not recreate pointer/focus targets.
    let inventory = computed(move || {
        ui.snapshot.with(|s| {
            let entry_rows = ui.closing_terminals.with(|closing| {
                ui.terminal_entries.with(|rows| {
                    rows.iter()
                        .filter(|e| e.listed() && !closing.contains(&e.saved.id))
                        .map(|e| {
                            let loc = e.location.as_ref();
                            (
                                e.saved.id.clone(),
                                entry_scope(s, e),
                                (
                                    loc.and_then(|l| l.branch.clone()),
                                    loc.is_some_and(|l| l.dirty),
                                    loc.map_or(0, |l| l.additions),
                                    loc.map_or(0, |l| l.deletions),
                                    loc.map_or(0, |l| l.ahead),
                                    loc.map_or(0, |l| l.behind),
                                ),
                                super::labels::sidebar_hint(
                                    e,
                                    &if e.machine() == "local" {
                                        "Local".into()
                                    } else {
                                        crate::machines::name(ui, e.machine())
                                    },
                                ),
                                (e.ended(), e.inactive(), e.needs_attention()),
                                (e.herdr_status(), e.wait_summary()),
                                e.provider(),
                            )
                        })
                        .collect::<Vec<_>>()
                })
            });
            (
                s.projects
                    .iter()
                    .map(|p| (p.id.clone(), p.name.clone(), p.target_branch.clone()))
                    .collect::<Vec<_>>(),
                s.cards
                    .iter()
                    .map(|c| {
                        (
                            c.id.clone(),
                            c.number,
                            c.title.clone(),
                            c.branch.clone(),
                            c.archived_at,
                        )
                    })
                    .collect::<Vec<_>>(),
                s.worktrees
                    .iter()
                    .map(|w| {
                        (
                            w.id.clone(),
                            w.project_id.clone(),
                            w.branch.clone(),
                            w.path.clone(),
                        )
                    })
                    .collect::<Vec<_>>(),
                entry_rows,
            )
        })
    });
    view()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .bg(SURFACE)
        .px_2()
        .py_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .px_2()
                .pb_2()
                .child(label("Terminals").font_weight(FontWeight::SEMIBOLD)),
        )
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .content_offset(scroll_position(ui, "terminals:tree"))
                .on_scroll(move |e: &ScrollEvent| capture_scroll(ui, "terminals:tree".into(), e))
                .child(dynamic(move || {
                    let _ = inventory.get();
                    let snapshot = ui.snapshot.get_untracked();
                    let rows: Vec<_> = ui.closing_terminals.with_untracked(|closing| {
                        ui.terminal_entries.with_untracked(|all| {
                            all.iter()
                                .filter(|e| e.listed() && !closing.contains(&e.saved.id))
                                .cloned()
                                .collect()
                        })
                    });
                    let mut body = view().gap_2();
                    let mut groups: Vec<_> = snapshot
                        .projects
                        .iter()
                        .map(|p| (Some(p.id.clone()), p.name.clone()))
                        .collect();
                    if !project_scopes_in_tree_order(&snapshot, &rows, None).is_empty() {
                        groups.push((None, "Other".into()));
                    }
                    for (project, name) in groups {
                        let active_scopes =
                            project_scopes_in_tree_order(&snapshot, &rows, project.as_deref());
                        let mut group = view().gap_1().pt_2();
                        let mut heading = view().flex_row().items_center().min_w(px(0.)).child(
                            view()
                                .flex_1()
                                .min_w(px(0.))
                                .min_h(px(32.))
                                .px_2()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(icon(Icon::Folder).w(px(14.)).h(px(14.)).flex_shrink_0())
                                .child(
                                    label(name.clone())
                                        .flex_1()
                                        .min_w(px(0.))
                                        .text_color(MUTED)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .truncate(),
                                ),
                        );
                        if let Some(pid) = project.clone() {
                            let pid_wt = pid.clone();
                            heading = heading
                                .child(chrome_button_disabled(
                                    Icon::Branch,
                                    format!("New branch worktree in {name}"),
                                    move || ui.pending.get(),
                                    move |_| ui.worktree_modal.set(Some(pid_wt.clone())),
                                ))
                                .child(chrome_button_disabled(
                                    Icon::Plus,
                                    format!("New terminal in {name}"),
                                    move || ui.pending.get(),
                                    move |_| create(ui, pid.clone(), None),
                                ));
                        }
                        group = group.child(heading);
                        for (scope, children) in active_scopes {
                            let mut scope_block = view()
                                .gap_0p5()
                                .child(scope_header(ui, &snapshot, scope, &children));
                            for child in children {
                                if snapshot
                                    .configuration_sessions
                                    .iter()
                                    .any(|c| c.id == child.saved.id)
                                {
                                    scope_block =
                                        scope_block.child(muted("Configuration").ml_5().pl_2());
                                }
                                scope_block = scope_block.child(
                                    view()
                                        .ml_5()
                                        .pl_2()
                                        .border_l_1()
                                        .border_color(BORDER)
                                        .child(row(ui, child)),
                                );
                            }
                            group = group.child(scope_block);
                        }
                        body = body.child(group);
                    }
                    if rows.is_empty() {
                        body = body.child(
                            action("New terminal", Variant::Quiet, move |_| new_tab(ui))
                                .disabled(move || ui.pending.get()),
                        );
                    }
                    body
                })),
        )
}
