//! Unified Project -> Worktree -> Terminal hierarchy.
//! Only worktree scopes with >= 1 live Terminal are displayed; Project and Worktree levels are foldable.
use super::worktrees::{
    WorktreeScope, create_in_scope, entry_scope, scope_git_summary, scope_resolution_label,
};
use super::*;
use crate::drafts::{capture_scroll, scroll_position};
use canban_types::Snapshot;

pub(crate) fn host_fold_key(host_id: &str) -> String {
    format!("host:{host_id}")
}

pub(crate) fn project_fold_key(project_id: &str) -> String {
    format!("project:{project_id}")
}

pub(crate) fn scope_fold_key(location_key: &str) -> String {
    format!("scope:{location_key}")
}

fn tree_icon_button(
    kind: Icon,
    title: impl IntoReactive<SharedString>,
    disabled: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> View {
    let title = title.into_reactive();
    let hint = title.clone();
    view()
        .flex_shrink_0()
        .window_drag(false)
        .tooltip(hint_tooltip(hint).placement(Placement::Bottom))
        .child(
            canban_ui_kit::press_target()
                .disabled(disabled)
                .disabled_style(|s| s.opacity(0.4))
                .w(px(22.))
                .h(px(22.))
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .bg(CLEAR)
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.border_1().border_color(ACCENT))
                .accessibility_label(move || Some(title.get()))
                .child(icon(kind).w(px(13.)).h(px(13.)))
                .on_press(press),
        )
}

fn git_status_chips(git: &super::worktrees::ScopeGitSummary) -> View {
    let mut row = view()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .opacity(0.85);
    if git.dirty {
        row = row
            .child(
                label(format!("+{}", git.additions))
                    .text_size(px(10.5))
                    .font_family(MONO_FONT)
                    .text_color(SUCCESS),
            )
            .child(
                label(format!("−{}", git.deletions))
                    .text_size(px(10.5))
                    .font_family(MONO_FONT)
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
        row = row.child(muted(ab.join(" ")).text_size(px(10.5)));
    }
    row
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ScopeMoreAction {
    ManagedWorktree { id: String, branch: String },
    CardWorktree { id: String, number: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScopeHeaderModel {
    scope: WorktreeScope,
    display_label: String,
    resolution_label: Option<String>,
    git: super::worktrees::ScopeGitSummary,
    lead_icon: Icon,
    card_badge: Option<String>,
    tooltip_text: String,
    folded: bool,
    summary: Option<super::labels::GroupStatusSummary>,
    more_action: Option<ScopeMoreAction>,
    is_external: bool,
}

fn build_scope_header_model(
    ui: Ui,
    s: &Snapshot,
    scope: &WorktreeScope,
    folded: bool,
    children: &[Entry],
) -> ScopeHeaderModel {
    let git = scope_git_summary(s, scope, children);
    let display_label = scope.display_name(s, children, Some(ui));
    let resolution_label = scope_resolution_label(s, scope);
    let summary = folded.then(|| super::labels::group_status_summary(children.iter()));

    let (lead_icon, card_badge, mut tooltip_text) = match scope {
        WorktreeScope::ProjectRoot(pid) => {
            let dir = s
                .projects
                .iter()
                .find(|p| &p.id == pid)
                .map(|p| p.directory.clone())
                .unwrap_or_default();
            let mut tip = format!("Project root · {display_label}");
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
                Some(c) => format!("#{} {}\nBranch: {display_label}", c.number, c.title),
                None => format!("Card worktree · {display_label}"),
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
                None => format!("Managed worktree `{display_label}`"),
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
        WorktreeScope::Directory { path, .. } => {
            let mut tip = path.clone();
            if let Some(ref b) = git.branch {
                tip.push_str(&format!("\nBranch: {b}"));
            }
            let ind = git.tooltip_indicators();
            if !ind.is_empty() {
                tip.push_str(&format!(" ({})", ind.join(" · ")));
            }
            (Icon::Folder, None, tip)
        }
        WorktreeScope::Unassigned(host_id) => {
            let host_label = if host_id == "local" {
                "Local".into()
            } else {
                crate::hosts::host_name(ui, host_id)
            };
            let tip = format!("External directory ({host_label})");
            (Icon::Terminal, None, tip)
        }
    };
    if let Some(label) = &resolution_label {
        tooltip_text.push_str(&format!("\n{label}"));
    }

    let more_action = match scope {
        WorktreeScope::ManagedWorktree(wid) => {
            s.worktrees
                .iter()
                .find(|w| &w.id == wid)
                .map(|w| ScopeMoreAction::ManagedWorktree {
                    id: w.id.clone(),
                    branch: w.branch.clone(),
                })
        }
        WorktreeScope::CardWorktree(cid) => {
            s.cards
                .iter()
                .find(|c| &c.id == cid)
                .map(|c| ScopeMoreAction::CardWorktree {
                    id: c.id.clone(),
                    number: c.number,
                })
        }
        _ => None,
    };
    let is_external = matches!(
        scope,
        WorktreeScope::Directory { project: None, .. } | WorktreeScope::Unassigned(_)
    );

    ScopeHeaderModel {
        scope: scope.clone(),
        display_label,
        resolution_label,
        git,
        lead_icon,
        card_badge,
        tooltip_text,
        folded,
        summary,
        more_action,
        is_external,
    }
}

fn scope_header(
    ui: Ui,
    render_key: String,
    model: Memo<Option<ScopeHeaderModel>>,
) -> impl IntoElement + use<> {
    let scope_for_active = render_key.clone();
    let is_active_scope = computed(move || {
        let sel = ui.terminal_selection.get();
        if sel.is_empty() || ui.closing_terminals.with(|c| c.contains(&sel)) {
            return false;
        }
        ui.terminal_entries.with(|rows| {
            rows.iter()
                .find(|e| e.saved.id == sel)
                .is_some_and(|e| super::worktrees::location_key(e) == scope_for_active)
        })
    });
    let fold_key = scope_fold_key(&render_key);
    let is_folded = computed(move || model.with(|m| m.as_ref().is_some_and(|h| h.folded)));
    let display_label = computed(move || {
        model.with(|m| {
            m.as_ref()
                .map(|h| h.display_label.clone())
                .unwrap_or_default()
        })
    });
    let resolution_label =
        computed(move || model.with(|m| m.as_ref().and_then(|h| h.resolution_label.clone())));
    let git =
        computed(move || model.with(|m| m.as_ref().map(|h| h.git.clone()).unwrap_or_default()));
    let lead_icon =
        computed(move || model.with(|m| m.as_ref().map(|h| h.lead_icon).unwrap_or(Icon::Folder)));
    let card_badge =
        computed(move || model.with(|m| m.as_ref().and_then(|h| h.card_badge.clone())));
    let tooltip_text = computed(move || {
        model.with(|m| {
            m.as_ref()
                .map(|h| h.tooltip_text.clone())
                .unwrap_or_default()
        })
    });
    let summary = computed(move || model.with(|m| m.as_ref().and_then(|h| h.summary)));
    let more_action =
        computed(move || model.with(|m| m.as_ref().and_then(|h| h.more_action.clone())));
    let is_external = computed(move || model.with(|m| m.as_ref().is_some_and(|h| h.is_external)));
    let stacked_git = computed(move || {
        let has_git = git.with(|g| g.dirty || g.ahead > 0 || g.behind > 0);
        resolution_label.with(|r| r.is_some()) && has_git && ui.terminal_width.get() < 340.
    });
    let hovered = signal(false);

    let title_row = view()
        .w_full()
        .min_w(px(0.))
        .flex_row()
        .items_center()
        .gap_1()
        .child(dynamic(move || {
            let active = is_active_scope.get();
            icon_tinted(lead_icon.get(), if active { ACCENT } else { MUTED })
                .w(px(12.))
                .h(px(12.))
                .flex_shrink_0()
        }))
        .child(dynamic(move || {
            let active = is_active_scope.get();
            label(move || SharedString::from(display_label.get()))
                .flex_1()
                .min_w(px(0.))
                .text_size(px(11.5))
                .text_color(MUTED)
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .truncate()
        }))
        .child(dynamic(move || {
            card_badge
                .get()
                .map(|b| muted(b).text_size(px(10.5)).flex_shrink_0())
        }))
        .child(when(
            move || resolution_label.with(|r| r.is_some()),
            || {
                status("Resolved", Tone::Highlight)
                    .px_1()
                    .py(px(1.))
                    .flex_shrink_0()
            },
        ))
        .child(dynamic(move || {
            if stacked_git.get() {
                return None;
            }
            git.with(|g| (g.dirty || g.ahead > 0 || g.behind > 0).then(|| git_status_chips(g)))
        }))
        .child(dynamic(move || {
            summary.get().and_then(super::labels::group_summary_badge)
        }));

    let fold_toggle = fold_key;
    let chevron_btn = canban_ui_kit::press_target()
        .w(px(16.))
        .h(px(26.))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .bg(CLEAR)
        .hover(|st| st.bg(INSET))
        .focus(|st| st.border_1().border_color(ACCENT))
        .accessibility_label(move || {
            let name = display_label.get();
            Some(
                if is_folded.get() {
                    format!("Expand worktree {name}")
                } else {
                    format!("Collapse worktree {name}")
                }
                .into(),
            )
        })
        .child(dynamic(move || {
            icon_tinted(
                if is_folded.get() {
                    Icon::Forward
                } else {
                    Icon::ChevronDown
                },
                MUTED,
            )
            .w(px(10.))
            .h(px(10.))
        }))
        .on_press(move |_: &()| {
            ui.terminal_folds.update(|folds| {
                if !folds.remove(&fold_toggle) {
                    folds.insert(fold_toggle.clone());
                }
            });
        });

    let press_key = render_key.clone();
    let btn_row = canban_ui_kit::press_target()
        .w_full()
        .min_w(px(0.))
        .min_h(px(26.))
        .px_1()
        .rounded(px(5.))
        .bg(CLEAR)
        .focus(|st| st.border_1().border_color(ACCENT))
        .flex_col()
        .justify_center()
        .py(move || px(if stacked_git.get() { 2. } else { 0. }))
        .child(title_row)
        .child(when(
            move || stacked_git.get(),
            move || {
                view()
                    .pl(px(16.))
                    .child(dynamic(move || git.with(git_status_chips)))
            },
        ))
        .accessibility_label(move || {
            let lbl = match resolution_label.get() {
                Some(resolved) => format!("{} · {resolved}", display_label.get()),
                None => display_label.get(),
            };
            Some(
                match summary.get() {
                    Some(sum) => format!(
                        "Worktree {lbl} · Collapsed · {}",
                        super::labels::group_summary_a11y(sum)
                    ),
                    None => format!("Worktree {lbl}"),
                }
                .into(),
            )
        })
        .on_press(move |_: &()| {
            let target = model.with_untracked(|m| {
                let recent_key = m.as_ref().map(|h| h.scope.recent_key());
                let recent_target =
                    recent_key.and_then(|rk| ui.terminal_recent.with(|map| map.get(&rk).cloned()));
                ui.closing_terminals.with_untracked(|closing| {
                    ui.terminal_entries.with_untracked(|rows| {
                        let mut children = rows.iter().filter(|e| {
                            e.listed()
                                && !closing.contains(&e.saved.id)
                                && super::worktrees::location_key(e) == press_key
                        });
                        recent_target
                            .filter(|rid| children.clone().any(|e| e.saved.id == *rid))
                            .or_else(|| children.next().map(|e| e.saved.id.clone()))
                    })
                })
            });
            if let Some(id) = target {
                ui.select_terminal_in_scope(id);
            }
        });

    let plus_key = render_key;
    view()
        .relative()
        .mt(px(2.))
        .ml(move || px(if is_external.get() { 4. } else { 12. }))
        .flex_row()
        .items_center()
        .min_w(px(0.))
        .rounded(px(5.))
        .bg(CLEAR)
        .hover(|st| st.bg(INSET))
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_context_menu(move |ev: &nsh_core::ContextMenuEvent| {
            let action = model.with_untracked(|m| m.as_ref().and_then(|h| h.more_action.clone()));
            match action {
                Some(ScopeMoreAction::ManagedWorktree { id, .. }) => {
                    if let Some(wt) = ui.snapshot.with_untracked(|s| {
                        s.worktrees.iter().find(|w| w.id == id).cloned()
                    }) {
                        ui.spawn(super::worktrees::menu(ui, wt, Some(ev.page_position)));
                    }
                }
                Some(ScopeMoreAction::CardWorktree { id, .. }) => {
                    if let Some(card) = ui
                        .snapshot
                        .with_untracked(|s| s.cards.iter().find(|c| c.id == id).cloned())
                    {
                        ui.spawn(super::worktrees::card_scope_menu(
                            ui,
                            card,
                            Some(ev.page_position),
                        ));
                    }
                }
                None => {}
            }
        })
        .child(chevron_btn)
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .tooltip(hint_tooltip(move || SharedString::from(tooltip_text.get())).placement(Placement::Right))
                .child(btn_row),
        )
        .child(when(
            move || hovered.get(),
            move || {
                let more_anchor = element_ref();
                let plus_key = plus_key.clone();
                view()
                    .absolute()
                    .right(px(2.))
                    .top_0()
                    .bottom_0()
                    .flex_row()
                    .items_center()
                    .gap_0p5()
                    .pl_1()
                    .rounded(px(5.))
                    .bg(INSET)
                    .child(tree_icon_button(
                        Icon::Plus,
                        move || SharedString::from(format!("New terminal in {}", display_label.get())),
                        move || ui.pending.get(),
                        move |_| {
                            let plus_params = ui.closing_terminals.with_untracked(|closing| {
                                ui.terminal_entries.with_untracked(|rows| {
                                    rows.iter()
                                        .find(|e| {
                                            e.listed()
                                                && !closing.contains(&e.saved.id)
                                                && super::worktrees::location_key(e) == plus_key
                                        })
                                        .and_then(|e| {
                                            e.location.as_ref().map(|l| {
                                                json!({
                                                    "host_id": e.host_id,
                                                    "cwd": l.directory_root.as_ref().or(l.cwd.as_ref()),
                                                    "project_id": l.project_id
                                                })
                                            })
                                        })
                                })
                            });
                            if let Some(p) = plus_params {
                                super::launch_custom(ui, p);
                            } else if let Some(scope) =
                                model.with_untracked(|m| m.as_ref().map(|h| h.scope.clone()))
                            {
                                create_in_scope(ui, &scope);
                            }
                        },
                    ))
                    .child(dynamic(move || match more_action.get() {
                        Some(ScopeMoreAction::ManagedWorktree { id, branch }) => Some(
                            tree_icon_button(
                                Icon::More,
                                format!("Worktree actions for {branch}"),
                                || false,
                                move |_| {
                                    let Some(item) = ui.snapshot.with_untracked(|s| {
                                        s.worktrees.iter().find(|w| w.id == id).cloned()
                                    }) else {
                                        return;
                                    };
                                    ui.spawn(async move {
                                        let pos = more_anchor.measure().await.map(|b| {
                                            point(b.page.x, b.page.y + b.frame.size.height)
                                        });
                                        super::worktrees::menu(ui, item, pos).await;
                                    });
                                },
                            )
                            .element_ref(&more_anchor),
                        ),
                        Some(ScopeMoreAction::CardWorktree { id, number }) => Some(
                            tree_icon_button(
                                Icon::More,
                                format!("Card worktree actions for #{number}"),
                                || false,
                                move |_| {
                                    let Some(item) = ui.snapshot.with_untracked(|s| {
                                        s.cards.iter().find(|c| c.id == id).cloned()
                                    }) else {
                                        return;
                                    };
                                    ui.spawn(async move {
                                        let pos = more_anchor.measure().await.map(|b| {
                                            point(b.page.x, b.page.y + b.frame.size.height)
                                        });
                                        super::worktrees::card_scope_menu(ui, item, pos).await;
                                    });
                                },
                            )
                            .element_ref(&more_anchor),
                        ),
                        None => None,
                    }))
            },
        ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TerminalRowModel {
    row_title: String,
    lead_icon: Icon,
    managed: bool,
    host_badge: Option<String>,
    status: Option<super::labels::SidebarStatus>,
    hint: String,
    close_label: String,
    is_external: bool,
}

fn build_terminal_row_model(
    ui: Ui,
    s: &Snapshot,
    e: &Entry,
    idle_minutes: u64,
    terminal_width: f32,
) -> TerminalRowModel {
    let scope = entry_scope(s, e);
    let is_external = scope.project_id(s).is_none();
    let in_own_card_wt = e
        .card
        .as_ref()
        .is_some_and(|c| scope == WorktreeScope::CardWorktree(c.id.clone()));
    let host_in_scope = matches!(scope, WorktreeScope::Unassigned(_));
    let host_label = if e.host_id == "local" {
        "Local".into()
    } else {
        crate::hosts::host_name(ui, &e.host_id)
    };
    let wide_row = terminal_width >= 230.;
    TerminalRowModel {
        row_title: super::labels::sidebar_row_title(e, in_own_card_wt),
        lead_icon: super::labels::sidebar_meta_icon(e),
        managed: e.is_managed(),
        host_badge: (wide_row && !host_in_scope)
            .then(|| super::labels::sidebar_host_badge(e, &host_label))
            .flatten(),
        status: super::labels::sidebar_status_model(e, wide_row),
        hint: super::labels::sidebar_hint(e, &host_label, idle_minutes),
        close_label: format!("Close terminal {}", e.title()),
        is_external,
    }
}

fn row(ui: Ui, id: String, model: Memo<Option<TerminalRowModel>>) -> impl IntoElement {
    let id_sel = id.clone();
    let is_selected = computed(move || ui.terminal_selection.get() == id_sel);
    let hovered = signal(false);
    let lead_icon =
        computed(move || model.with(|m| m.as_ref().map(|r| r.lead_icon).unwrap_or(Icon::Terminal)));
    let row_title = computed(move || {
        model.with(|m| m.as_ref().map(|r| r.row_title.clone()).unwrap_or_default())
    });
    let is_managed = computed(move || model.with(|m| m.as_ref().is_some_and(|r| r.managed)));
    let host_badge =
        computed(move || model.with(|m| m.as_ref().and_then(|r| r.host_badge.clone())));
    let status = computed(move || model.with(|m| m.as_ref().and_then(|r| r.status)));
    let hint =
        computed(move || model.with(|m| m.as_ref().map(|r| r.hint.clone()).unwrap_or_default()));
    let close_label = computed(move || {
        model.with(|m| {
            m.as_ref()
                .map(|r| r.close_label.clone())
                .unwrap_or_default()
        })
    });

    let line = view()
        .w_full()
        .min_w(px(0.))
        .flex_row()
        .items_center()
        .gap_1p5()
        .child(dynamic(move || {
            icon(lead_icon.get()).w(px(14.)).h(px(14.)).flex_shrink_0()
        }))
        .child(dynamic(move || {
            let selected = is_selected.get();
            label(move || SharedString::from(row_title.get()))
                .text_size(px(13.))
                .text_color(TEXT)
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .flex_shrink(1.)
                .min_w(px(0.))
                .truncate()
        }))
        .child(when(move || is_managed.get(), super::labels::managed_badge))
        .child(view().flex_1().min_w(px(0.)))
        .child(when(
            move || !hovered.get(),
            move || {
                dynamic(move || {
                    host_badge.get().map(|host| {
                        muted(format!("· {host}"))
                            .text_size(px(10.5))
                            .max_w(px(72.))
                            .truncate()
                            .flex_shrink_0()
                    })
                })
            },
        ))
        .child(when(
            move || !hovered.get(),
            move || dynamic(move || status.get().map(super::labels::render_sidebar_status)),
        ));

    let open_id = id.clone();
    let close_id = id.clone();
    let context_id = id;
    view()
        .relative()
        .flex_row()
        .items_center()
        .min_w(px(0.))
        .h(px(32.))
        .my(px(1.))
        .rounded(px(6.))
        .bg(move || if is_selected.get() { SELECTED } else { CLEAR })
        .hover(move |s| s.bg(move || if is_selected.get() { SELECTED } else { INSET }))
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_context_menu(move |event: &nsh_core::ContextMenuEvent| {
            ui.spawn(super::tabs::menu(
                ui,
                context_id.clone(),
                Some(event.page_position),
            ));
        })
        .tooltip(hint_tooltip(move || SharedString::from(hint.get())).placement(Placement::Right))
        .child(when(
            move || is_selected.get(),
            || {
                view()
                    .absolute()
                    .left(px(0.))
                    .top(px(7.))
                    .bottom(px(7.))
                    .w(px(2.5))
                    .rounded(px(2.))
                    .bg(ACCENT)
                    .pointer_events(PointerEvents::None)
            },
        ))
        .child(
            canban_ui_kit::press_target()
                .flex_1()
                .min_w(px(0.))
                .h(px(32.))
                .px_2()
                .flex_row()
                .items_center()
                .rounded(px(6.))
                .bg(CLEAR)
                .focus(|s| s.border_1().border_color(ACCENT))
                .selected(move || is_selected.get())
                .accessibility_label(move || Some(SharedString::from(hint.get())))
                .child(line)
                .on_press(move |_: &()| ui.open_terminal(open_id.clone())),
        )
        .child(when(
            move || hovered.get(),
            move || {
                let close_target = close_id.clone();
                view().pr_1().child(tree_icon_button(
                    Icon::Close,
                    move || SharedString::from(close_label.get()),
                    move || ui.pending.get(),
                    move |_| super::close_entry(ui, &close_target),
                ))
            },
        ))
}
/// Flat keys allow a Terminal row to move between parents without remounting.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum TreeNode {
    HostHeader(String),
    Project(String),
    HostDirectoriesHeader(String),
    OtherHeader,
    Scope(String, WorktreeScope),
    Terminal(String),
    Empty,
}
impl TreeNode {
    fn key(&self) -> String {
        match self {
            Self::HostHeader(id) => format!("host:{id}"),
            Self::Project(id) => format!("project:{id}"),
            Self::HostDirectoriesHeader(id) => format!("host-dirs:{id}"),
            Self::OtherHeader => "section:other".into(),
            Self::Scope(key, _) => key.clone(),
            Self::Terminal(id) => format!("terminal:{id}"),
            Self::Empty => "empty".into(),
        }
    }
    fn height(&self, ui: Ui, is_first: bool) -> f32 {
        match self {
            Self::HostHeader(_) => {
                if is_first {
                    28.
                } else {
                    36.
                }
            }
            Self::Project(_) => {
                if is_first {
                    28.
                } else {
                    38.
                }
            }
            Self::HostDirectoriesHeader(_) | Self::OtherHeader => 34.,
            Self::Scope(key, scope) => {
                let stacked = ui.terminal_width.get() < 340.
                    && ui
                        .snapshot
                        .with(|s| scope_resolution_label(s, scope).is_some())
                    && ui.terminal_entries.with(|rows| {
                        rows.iter().any(|e| {
                            e.listed()
                                && super::worktrees::location_key(e) == *key
                                && e.location
                                    .as_ref()
                                    .is_some_and(|l| l.dirty || l.ahead > 0 || l.behind > 0)
                        })
                    });
                if stacked { 42. } else { 28. }
            }
            Self::Terminal(_) => 34.,
            Self::Empty => 40.,
        }
    }
}

fn ordered_host_ids<'a>(s: &Snapshot, extra_hosts: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    seen.insert("local".to_owned());
    out.push("local".to_owned());

    let mut remotes: Vec<(&str, &str)> = s
        .host_summaries
        .iter()
        .filter(|(id, _)| id.as_str() != "local")
        .map(|(id, h)| (h.name.as_str(), id.as_str()))
        .collect();
    remotes.sort_by(|a, b| {
        a.0.to_ascii_lowercase()
            .cmp(&b.0.to_ascii_lowercase())
            .then_with(|| a.1.cmp(b.1))
    });
    for (_, id) in remotes {
        if seen.insert(id.to_owned()) {
            out.push(id.to_owned());
        }
    }
    for p in &s.projects {
        let hid = s.project_host(&p.id);
        if seen.insert(hid.to_owned()) {
            out.push(hid.to_owned());
        }
    }
    for hid in extra_hosts {
        let h = if hid.is_empty() { "local" } else { hid };
        if seen.insert(h.to_owned()) {
            out.push(h.to_owned());
        }
    }
    out
}

#[cfg(test)]
pub(super) fn project_tree<'a>(
    s: &Snapshot,
    rows: impl IntoIterator<Item = &'a Entry>,
) -> Vec<TreeNode> {
    visible_project_tree(s, rows, &std::collections::BTreeSet::new())
}

pub(super) fn visible_project_tree<'a>(
    s: &Snapshot,
    rows: impl IntoIterator<Item = &'a Entry>,
    folds: &std::collections::BTreeSet<String>,
) -> Vec<TreeNode> {
    let row_vec: Vec<&'a Entry> = rows.into_iter().collect();
    if s.has_remote_hosts() {
        let mut by_host = std::collections::BTreeMap::<
            String,
            std::collections::BTreeMap<
                Option<String>,
                std::collections::BTreeMap<String, (WorktreeScope, Vec<String>)>,
            >,
        >::new();
        for e in &row_vec {
            let scope = entry_scope(s, e);
            let pid = scope.project_id(s);
            let hid = pid
                .as_deref()
                .filter(|p| !canban_types::is_global_project_id(p))
                .map(|p| s.project_host(p))
                .unwrap_or_else(|| {
                    if e.host_id.is_empty() {
                        s.terminal_host(&e.saved.id)
                    } else {
                        e.host_id.as_str()
                    }
                })
                .to_owned();
            let key = super::worktrees::location_key(e);
            let group = by_host
                .entry(hid)
                .or_default()
                .entry(pid)
                .or_default()
                .entry(key)
                .or_insert_with(|| (scope, Vec::new()));
            group.1.push(e.saved.id.clone());
        }
        let hosts = ordered_host_ids(s, by_host.keys().map(String::as_str));
        let mut out = Vec::new();
        for hid in hosts {
            out.push(TreeNode::HostHeader(hid.clone()));
            if folds.contains(&host_fold_key(&hid)) {
                continue;
            }
            let mut host_groups = by_host.remove(&hid).unwrap_or_default();
            for pid in s
                .projects
                .iter()
                .filter(|p| s.project_host(&p.id) == hid)
                .map(|p| p.id.clone())
            {
                let folded = folds.contains(&project_fold_key(&pid));
                out.push(TreeNode::Project(pid.clone()));
                if folded {
                    host_groups.remove(&Some(pid));
                    continue;
                }
                if let Some(scopes) = host_groups.remove(&Some(pid)) {
                    for (key, (scope, children)) in scopes {
                        let scope_folded = folds.contains(&scope_fold_key(&key));
                        out.push(TreeNode::Scope(key, scope));
                        if !scope_folded {
                            out.extend(children.into_iter().map(TreeNode::Terminal));
                        }
                    }
                }
            }
            if let Some(other_scopes) = host_groups.remove(&None) {
                out.push(TreeNode::HostDirectoriesHeader(hid.clone()));
                for (key, (scope, children)) in other_scopes {
                    let scope_folded = folds.contains(&scope_fold_key(&key));
                    out.push(TreeNode::Scope(key, scope));
                    if !scope_folded {
                        out.extend(children.into_iter().map(TreeNode::Terminal));
                    }
                }
            }
            // Any remaining global/orphan project scopes belonging to this host.
            for (_pid, scopes) in host_groups {
                for (key, (scope, children)) in scopes {
                    let scope_folded = folds.contains(&scope_fold_key(&key));
                    out.push(TreeNode::Scope(key, scope));
                    if !scope_folded {
                        out.extend(children.into_iter().map(TreeNode::Terminal));
                    }
                }
            }
        }
        return out;
    }

    let mut groups = std::collections::BTreeMap::<
        Option<String>,
        std::collections::BTreeMap<String, (WorktreeScope, Vec<String>)>,
    >::new();
    for e in row_vec {
        let scope = entry_scope(s, e);
        let key = super::worktrees::location_key(e);
        let group = groups
            .entry(scope.project_id(s))
            .or_default()
            .entry(key)
            .or_insert_with(|| (scope, Vec::new()));
        group.1.push(e.saved.id.clone());
    }
    let mut out = Vec::new();
    // Project ordering is stable; no empty registered Projects enter the tree.
    for pid in s
        .projects
        .iter()
        .map(|p| Some(p.id.clone()))
        .chain(std::iter::once(None))
    {
        let Some(scopes) = groups.remove(&pid) else {
            continue;
        };
        let project_folded = if let Some(pid) = pid {
            let folded = folds.contains(&project_fold_key(&pid));
            out.push(TreeNode::Project(pid));
            folded
        } else {
            out.push(TreeNode::OtherHeader);
            false
        };
        if project_folded {
            continue;
        }
        for (key, (scope, children)) in scopes {
            let scope_folded = folds.contains(&scope_fold_key(&key));
            out.push(TreeNode::Scope(key, scope));
            if !scope_folded {
                out.extend(children.into_iter().map(TreeNode::Terminal));
            }
        }
    }
    if out.is_empty() {
        out.push(TreeNode::Empty);
    }
    out
}

fn foldable_other_keys(ui: Ui) -> Vec<String> {
    let active = untracked(|| current(ui));
    ui.snapshot.with_untracked(|s| {
        ui.closing_terminals.with_untracked(|closing| {
            ui.terminal_entries.with_untracked(|rows| {
                let active_loc = active.as_ref().map(super::worktrees::location_key);
                let active_pid = active
                    .as_ref()
                    .and_then(|e| entry_scope(s, e).project_id(s));
                let mut keys = Vec::new();
                let mut seen_projects = std::collections::BTreeSet::new();
                let mut seen_scopes = std::collections::BTreeSet::new();
                for e in rows
                    .iter()
                    .filter(|e| e.listed() && !closing.contains(&e.saved.id))
                {
                    let pid = entry_scope(s, e).project_id(s);
                    if let Some(pid) = pid
                        && active_pid.as_ref() != Some(&pid)
                        && seen_projects.insert(pid.clone())
                    {
                        keys.push(project_fold_key(&pid));
                    }
                    let loc = super::worktrees::location_key(e);
                    if active_loc.as_ref() != Some(&loc) && seen_scopes.insert(loc.clone()) {
                        keys.push(scope_fold_key(&loc));
                    }
                }
                keys
            })
        })
    })
}

pub(crate) fn collapse_other_worktrees(ui: Ui) {
    let others = foldable_other_keys(ui);
    let active = untracked(|| current(ui));
    let active_keys = active.as_ref().map(|e| {
        let pid = ui
            .snapshot
            .with_untracked(|s| entry_scope(s, e).project_id(s));
        let loc = super::worktrees::location_key(e);
        (pid, loc)
    });
    ui.terminal_folds.update(|folds| {
        for key in others {
            folds.insert(key);
        }
        if let Some((pid, loc)) = active_keys {
            if let Some(pid) = pid {
                folds.remove(&project_fold_key(&pid));
            }
            folds.remove(&scope_fold_key(&loc));
        }
        if let Some(ref e) = active {
            let hid = ui.snapshot.with_untracked(|s| {
                e.project_id
                    .as_deref()
                    .filter(|p| !canban_types::is_global_project_id(p))
                    .map(|p| s.project_host(p).to_owned())
                    .unwrap_or_else(|| e.host_id.clone())
            });
            folds.remove(&host_fold_key(&hid));
        }
    });
}

pub(crate) fn expand_all_worktrees(ui: Ui) {
    ui.terminal_folds.update(|folds| folds.clear());
}

fn open_host_launch_menu(ui: Ui, host_id: String, at: Option<Point<Points>>) {
    use nsh_core::element::desktop::{MenuItem, popup_menu, popup_menu_at};
    let (host_name, offline, projects) = ui.snapshot.with_untracked(|s| {
        let name = s.host_display_name(&host_id).to_owned();
        let offline = s.is_host_offline(&host_id);
        let projs: Vec<(String, String)> = s
            .projects
            .iter()
            .filter(|p| s.project_host(&p.id) == host_id)
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        (name, offline, projs)
    });
    ui.spawn(async move {
        let mut items = Vec::new();
        let home_host = host_id.clone();
        items.push(
            MenuItem::new(format!("New Terminal ($HOME · {host_name})"))
                .disabled(offline)
                .on_select(move || super::launch(ui, serde_json::json!({ "host_id": home_host }))),
        );
        if !projects.is_empty() {
            items.push(MenuItem::separator());
            for (pid, pname) in projects {
                let p_open = pid.clone();
                items.push(
                    MenuItem::new(format!("New Terminal in {pname}"))
                        .disabled(offline)
                        .on_select(move || super::create(ui, p_open.clone(), None)),
                );
            }
        }
        if let Some(pt) = at {
            popup_menu_at(items, pt).await;
        } else {
            popup_menu(items).await;
        }
    });
}

fn host_directories_heading() -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(34.))
        .min_w(px(0.))
        .px_1()
        .pt_2()
        .child(
            label("DIRECTORIES")
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(MUTED)
                .flex_shrink_0(),
        )
        .child(view().flex_1().h(px(1.)).bg(BORDER))
}

fn host_heading(ui: Ui, host_id: String, is_first: Memo<bool>) -> impl IntoElement {
    let fold_key = host_fold_key(&host_id);
    let info = computed({
        let host_id = host_id.clone();
        let fold_key = fold_key.clone();
        move || {
            let folded = ui.terminal_folds.with(|folds| folds.contains(&fold_key));
            ui.snapshot.with(|s| {
                let name = s.host_display_name(&host_id).to_owned();
                let offline = s.is_host_offline(&host_id);
                let summary = folded.then(|| {
                    ui.closing_terminals.with(|closing| {
                        ui.terminal_entries.with(|rows| {
                            super::labels::group_status_summary(rows.iter().filter(|e| {
                                e.listed()
                                    && !closing.contains(&e.saved.id)
                                    && (e.host_id == host_id
                                        || (host_id == "local" && e.host_id.is_empty()))
                            }))
                        })
                    })
                });
                (folded, name, offline, summary)
            })
        }
    });
    let is_folded = computed(move || info.with(|(f, ..)| *f));
    let host_name = computed(move || info.with(|(_, n, ..)| n.clone()));
    let is_offline = computed(move || info.with(|(_, _, off, _)| *off));
    let summary = computed(move || info.with(|(_, _, _, s)| *s));
    let fold_toggle = fold_key.clone();
    let fold_ctx = fold_key;
    let host_menu_ctx = host_id.clone();
    let plus_ref = element_ref();
    let host_for_plus = host_id;

    let header_btn = canban_ui_kit::press_target()
        .flex_1()
        .min_w(px(0.))
        .h(px(26.))
        .px_1()
        .flex_row()
        .items_center()
        .gap_1()
        .rounded(px(5.))
        .bg(CLEAR)
        .hover(|s| s.bg(INSET))
        .focus(|s| s.border_1().border_color(ACCENT))
        .accessibility_label(move || {
            let name = host_name.get();
            Some(SharedString::from(format!("Machine {name}")))
        })
        .child(dynamic(move || {
            icon_tinted(
                if is_folded.get() {
                    Icon::Forward
                } else {
                    Icon::ChevronDown
                },
                MUTED,
            )
            .w(px(10.))
            .h(px(10.))
            .flex_shrink_0()
        }))
        .child(
            icon_tinted(Icon::Projects, MUTED)
                .w(px(11.))
                .h(px(11.))
                .flex_shrink_0(),
        )
        .child(
            label(move || SharedString::from(host_name.get().to_uppercase()))
                .flex_1()
                .min_w(px(0.))
                .text_size(px(11.))
                .text_color(move || if is_offline.get() { MUTED } else { TEXT })
                .font_weight(FontWeight::SEMIBOLD)
                .truncate(),
        )
        .child(when(
            move || is_offline.get(),
            move || muted("(offline)").text_size(px(10.)).flex_shrink_0(),
        ))
        .child(dynamic(move || {
            summary.get().and_then(super::labels::group_summary_badge)
        }))
        .on_press(move |_: &()| {
            ui.terminal_folds.update(|folds| {
                if !folds.remove(&fold_toggle) {
                    folds.insert(fold_toggle.clone());
                }
            });
        });

    let row = view()
        .flex_row()
        .items_center()
        .h(px(28.))
        .min_w(px(0.))
        .on_context_menu(move |ev: &nsh_core::ContextMenuEvent| {
            use nsh_core::element::desktop::{MenuItem, popup_menu_at};
            let (folded, name, offline, _) = info.get_untracked();
            let hid_open = host_menu_ctx.clone();
            let fold_key = fold_ctx.clone();
            let at = ev.page_position;
            ui.spawn(async move {
                let toggle_label = if folded {
                    format!("Expand {name}")
                } else {
                    format!("Collapse {name}")
                };
                let items = vec![
                    MenuItem::new(format!("New Terminal ($HOME · {name})"))
                        .disabled(offline)
                        .on_select(move || {
                            super::launch(ui, serde_json::json!({ "host_id": hid_open }))
                        }),
                    MenuItem::separator(),
                    MenuItem::new(toggle_label).on_select(move || {
                        ui.terminal_folds.update(|folds| {
                            if !folds.remove(&fold_key) {
                                folds.insert(fold_key.clone());
                            }
                        });
                    }),
                ];
                popup_menu_at(items, at).await;
            });
        })
        .child(header_btn)
        .child(view().element_ref(&plus_ref).child(tree_icon_button(
            Icon::Plus,
            move || {
                if is_offline.get() {
                    SharedString::from("Machine offline")
                } else {
                    SharedString::from(format!("New terminal on {}", host_name.get()))
                }
            },
            move || ui.pending.get() || is_offline.get(),
            move |_| {
                let hid = host_for_plus.clone();
                let anchor = plus_ref;
                ui.spawn(async move {
                    let at = anchor
                        .measure()
                        .await
                        .map(|b| point(b.page.x, b.page.y + b.frame.size.height));
                    open_host_launch_menu(ui, hid, at);
                });
            },
        )));

    view()
        .flex_col()
        .min_w(px(0.))
        .child(when(
            move || !is_first.get(),
            || {
                view()
                    .h(px(8.))
                    .px_1()
                    .justify_center()
                    .child(view().h(px(1.)).bg(BORDER))
            },
        ))
        .child(row)
}

fn other_heading() -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(34.))
        .min_w(px(0.))
        .px_1()
        .pt_2()
        .child(
            label("OTHER DIRECTORIES")
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(MUTED)
                .flex_shrink_0(),
        )
        .child(view().flex_1().h(px(1.)).bg(BORDER))
}

fn project_heading(ui: Ui, id: String, is_first: Memo<bool>) -> impl IntoElement {
    let fold_key = project_fold_key(&id);
    let hovered = signal(false);
    let info = computed({
        let id = id.clone();
        let fold_key = fold_key.clone();
        move || {
            let folded = ui.terminal_folds.with(|folds| folds.contains(&fold_key));
            ui.snapshot.with(|s| {
                let name = s
                    .projects
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                let offline = s.is_host_offline(s.project_host(&id));
                let has_remote = s.has_remote_hosts();
                let summary = folded.then(|| {
                    ui.closing_terminals.with(|closing| {
                        ui.terminal_entries.with(|rows| {
                            super::labels::group_status_summary(rows.iter().filter(|e| {
                                e.listed()
                                    && !closing.contains(&e.saved.id)
                                    && entry_scope(s, e).project_id(s).as_deref()
                                        == Some(id.as_str())
                            }))
                        })
                    })
                });
                (folded, name, offline, has_remote, summary)
            })
        }
    });
    let is_folded = computed(move || info.with(|(f, ..)| *f));
    let proj_name = computed(move || info.with(|(_, n, ..)| n.clone()));
    let is_offline = computed(move || info.with(|(_, _, off, ..)| *off));
    let in_host_group = computed(move || info.with(|(_, _, _, r, _)| *r));
    let summary = computed(move || info.with(|(_, _, _, _, s)| *s));

    let pid_wt = id.clone();
    let pid_new = id.clone();
    let pid_ctx_new = id.clone();
    let pid_ctx_wt = id;
    let fold_toggle = fold_key.clone();
    let fold_ctx = fold_key;

    let header_btn = canban_ui_kit::press_target()
        .flex_1()
        .min_w(px(0.))
        .h(px(26.))
        .px_1()
        .flex_row()
        .items_center()
        .gap_1()
        .rounded(px(5.))
        .bg(CLEAR)
        .hover(|s| s.bg(INSET))
        .focus(|s| s.border_1().border_color(ACCENT))
        .accessibility_label(move || {
            let name = proj_name.get();
            Some(
                match summary.get() {
                    Some(sum) => format!(
                        "Project {name} · Collapsed · {}",
                        super::labels::group_summary_a11y(sum)
                    ),
                    None => format!("Project {name}"),
                }
                .into(),
            )
        })
        .child(dynamic(move || {
            icon_tinted(
                if is_folded.get() {
                    Icon::Forward
                } else {
                    Icon::ChevronDown
                },
                MUTED,
            )
            .w(px(10.))
            .h(px(10.))
            .flex_shrink_0()
        }))
        .child(
            label(move || SharedString::from(proj_name.get().to_uppercase()))
                .flex_1()
                .min_w(px(0.))
                .text_size(px(11.))
                .text_color(MUTED)
                .font_weight(FontWeight::SEMIBOLD)
                .truncate(),
        )
        .child(dynamic(move || {
            summary.get().and_then(super::labels::group_summary_badge)
        }))
        .on_press(move |_: &()| {
            ui.terminal_folds.update(|folds| {
                if !folds.remove(&fold_toggle) {
                    folds.insert(fold_toggle.clone());
                }
            });
        });

    let row = view()
        .relative()
        .flex_row()
        .items_center()
        .h(px(28.))
        .min_w(px(0.))
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_context_menu(move |ev: &nsh_core::ContextMenuEvent| {
            use nsh_core::element::desktop::{MenuItem, popup_menu_at};
            let (folded, name, offline, _, _) = info.get_untracked();
            let pid_new = pid_ctx_new.clone();
            let pid_wt = pid_ctx_wt.clone();
            let fold_key = fold_ctx.clone();
            let at = ev.page_position;
            ui.spawn(async move {
                let toggle_label = if folded {
                    format!("Expand {name}")
                } else {
                    format!("Collapse {name}")
                };
                let items = vec![
                    MenuItem::new(format!("New Terminal in {name}"))
                        .disabled(offline)
                        .on_select(move || create(ui, pid_new.clone(), None)),
                    MenuItem::new("New Managed Branch Worktree…")
                        .disabled(offline)
                        .on_select(move || ui.worktree_modal.set(Some(pid_wt.clone()))),
                    MenuItem::separator(),
                    MenuItem::new(toggle_label).on_select(move || {
                        ui.terminal_folds.update(|folds| {
                            if !folds.remove(&fold_key) {
                                folds.insert(fold_key.clone());
                            }
                        });
                    }),
                ];
                popup_menu_at(items, at).await;
            });
        })
        .child(header_btn)
        .child(when(
            move || hovered.get(),
            move || {
                let pid_wt = pid_wt.clone();
                let pid_new = pid_new.clone();
                view()
                    .absolute()
                    .right(px(4.))
                    .top(px(2.))
                    .bottom(px(2.))
                    .flex_row()
                    .items_center()
                    .gap_0p5()
                    .pl_1()
                    .rounded(px(5.))
                    .bg(INSET)
                    .child(tree_icon_button(
                        Icon::Branch,
                        "New branch worktree",
                        move || ui.pending.get() || is_offline.get(),
                        move |_| ui.worktree_modal.set(Some(pid_wt.clone())),
                    ))
                    .child(tree_icon_button(
                        Icon::Plus,
                        "New terminal",
                        move || ui.pending.get() || is_offline.get(),
                        move |_| create(ui, pid_new.clone(), None),
                    ))
            },
        ));

    view()
        .flex_col()
        .min_w(px(0.))
        .pl(move || px(if in_host_group.get() { 6. } else { 0. }))
        .child(when(
            move || !is_first.get() && !in_host_group.get(),
            || {
                view()
                    .h(px(10.))
                    .px_1()
                    .justify_center()
                    .child(view().h(px(1.)).bg(BORDER))
            },
        ))
        .child(row)
}

fn node_content(
    ui: Ui,
    node: ReadSignal<TreeNode>,
    indices: Memo<std::collections::BTreeMap<String, usize>>,
    is_first: Memo<bool>,
) -> AnyElement {
    match node.get_untracked() {
        TreeNode::HostHeader(id) => host_heading(ui, id, is_first).into_element(),
        TreeNode::Project(id) => project_heading(ui, id, is_first).into_element(),
        TreeNode::HostDirectoriesHeader(_) => host_directories_heading().into_element(),
        TreeNode::OtherHeader => other_heading().into_element(),
        TreeNode::Terminal(id) => {
            let id_model = id.clone();
            let live_model = computed(move || {
                ui.closing_terminals.with(|closing| {
                    ui.terminal_entries.with(|rows| {
                        let e = indices
                            .with(|map| map.get(&id_model).and_then(|i| rows.get(*i)))
                            .filter(|e| e.listed() && !closing.contains(&e.saved.id))?;
                        let idle_minutes = ui.idle_minutes.get();
                        let terminal_width = ui.terminal_width.get();
                        Some(ui.snapshot.with(|s| {
                            build_terminal_row_model(ui, s, e, idle_minutes, terminal_width)
                        }))
                    })
                })
            });
            let last = stored(live_model.get_untracked());
            let model = computed(move || {
                if let Some(next) = live_model.get() {
                    last.replace(Some(next));
                }
                last.get()
            });
            let is_external =
                computed(move || model.with(|m| m.as_ref().is_some_and(|r| r.is_external)));
            view()
                .ml(move || px(if is_external.get() { 12. } else { 20. }))
                .pl_2()
                .border_l_1()
                .border_color(BORDER)
                .child(row(ui, id, model))
                .into_element()
        }
        TreeNode::Scope(key, _) => {
            let render_key = key.clone();
            let fold_key = scope_fold_key(&key);
            // Expanded headers skip Agent status ticks; collapsed headers track GroupStatusSummary.
            let live_model = computed(move || {
                let TreeNode::Scope(_, scope) = node.get() else {
                    unreachable!()
                };
                let folded = ui.terminal_folds.with(|folds| folds.contains(&fold_key));
                ui.snapshot.with(|s| {
                    ui.closing_terminals.with(|closing| {
                        ui.terminal_entries.with(|rows| {
                            let children: Vec<_> = rows
                                .iter()
                                .filter(|e| {
                                    e.listed()
                                        && !closing.contains(&e.saved.id)
                                        && super::worktrees::location_key(e) == key
                                })
                                .cloned()
                                .collect();
                            if children.is_empty() {
                                return None;
                            }
                            Some(build_scope_header_model(ui, s, &scope, folded, &children))
                        })
                    })
                })
            });
            let last = stored(live_model.get_untracked());
            let model = computed(move || {
                if let Some(next) = live_model.get() {
                    last.replace(Some(next));
                }
                last.get()
            });
            scope_header(ui, render_key, model).into_element()
        }
        TreeNode::Empty => action("New terminal", Variant::Quiet, move |_| new_tab(ui))
            .disabled(move || ui.pending.get())
            .into_element(),
    }
}

pub fn sidebar(ui: Ui) -> impl IntoElement {
    let indices = computed(move || {
        ui.terminal_entries.with(|rows| {
            rows.iter()
                .enumerate()
                .map(|(i, e)| (e.saved.id.clone(), i))
                .collect::<std::collections::BTreeMap<_, _>>()
        })
    });
    let structure = computed(move || {
        ui.snapshot.with(|s| {
            ui.closing_terminals.with(|closing| {
                ui.terminal_entries.with(|rows| {
                    ui.terminal_folds.with(|folds| {
                        visible_project_tree(
                            s,
                            rows.iter()
                                .filter(|e| e.listed() && !closing.contains(&e.saved.id)),
                            folds,
                        )
                    })
                })
            })
        })
    });
    let desired_keys = computed(move || {
        structure.with(|nodes| {
            nodes
                .iter()
                .map(TreeNode::key)
                .collect::<std::collections::BTreeSet<_>>()
        })
    });
    let fold_button_mode = computed(move || {
        let _ = structure.get();
        let has_folds = ui.terminal_folds.with(|f| !f.is_empty());
        let other_keys = foldable_other_keys(ui);
        let has_unfolded_others = ui
            .terminal_folds
            .with(|folds| other_keys.iter().any(|k| !folds.contains(k)));
        if has_unfolded_others {
            Some(true)
        } else if has_folds {
            Some(false)
        } else {
            None
        }
    });
    let region = element_ref();
    let initial = structure.get_untracked();
    let previous = stored(initial.clone());
    let scroll_ref = element_ref();
    let scroll_generation = stored(0u64);
    let retained = signal(initial);
    // Retain outgoing rows for their native fade/collapse. A returning key cancels exit.
    effect(move || {
        let next = structure.get();
        let before = previous.replace(next.clone());
        let offset = scroll_position(ui, "terminals:tree");
        if offset.y.0 > 0. && before != next {
            let mut top = 0.;
            let anchor = before.iter().enumerate().find_map(|(i, row)| {
                let start = top;
                top += row.height(ui, i == 0);
                (top > offset.y.0 && next.iter().any(|n| n.key() == row.key()))
                    .then(|| (row.key(), start))
            });
            if let Some((key, start)) = anchor {
                let new_top: f32 = next
                    .iter()
                    .enumerate()
                    .take_while(|(_, n)| n.key() != key)
                    .map(|(i, row)| row.height(ui, i == 0))
                    .sum();
                let target = point(offset.x, px((new_top + offset.y.0 - start).max(0.)));
                let ticket = scroll_generation.get();
                spawn(async move {
                    next_frame().await;
                    if scroll_generation.get() == ticket {
                        scroll_ref.scroll_to(target, false);
                    }
                });
            }
        }
        let old = retained.get_untracked();
        let mut merged = next.clone();
        for (index, row) in old.into_iter().enumerate() {
            if !desired_keys.with_untracked(|keys| keys.contains(&row.key())) {
                merged.insert(index.min(merged.len()), row);
            }
        }
        if retained.get_untracked() != merged {
            canban_ui_kit::motion::layout(region, 160.);
            retained.set(merged);
        }
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
                .h(px(28.))
                .px_1p5()
                .pb_1()
                .child(label("Terminals").font_weight(FontWeight::SEMIBOLD))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(dynamic(move || match fold_button_mode.get() {
                            Some(true) => Some(tree_icon_button(
                                Icon::Collapse,
                                "Collapse other worktrees",
                                || false,
                                move |_| collapse_other_worktrees(ui),
                            )),
                            Some(false) => Some(tree_icon_button(
                                Icon::Expand,
                                "Expand all worktrees",
                                || false,
                                move |_| expand_all_worktrees(ui),
                            )),
                            None => None,
                        }))
                        .child(tree_icon_button(
                            Icon::Plus,
                            "New terminal…",
                            move || ui.pending.get(),
                            move |_| {
                                if ui.snapshot.with_untracked(|s| s.has_remote_hosts()) {
                                    super::machine_menu(ui);
                                } else {
                                    new_tab(ui);
                                }
                            },
                        )),
                ),
        )
        .child(
            scroll()
                .element_ref(&scroll_ref)
                .flex_1()
                .min_h(px(0.))
                .content_offset(scroll_position(ui, "terminals:tree"))
                .on_scroll(move |e: &ScrollEvent| {
                    scroll_generation.replace(scroll_generation.get().wrapping_add(1));
                    capture_scroll(ui, "terminals:tree".into(), e);
                })
                .child(view().element_ref(&region).child(each(
                    move || retained.get(),
                    TreeNode::key,
                    move |node, _| {
                        let key = node.get_untracked().key();
                        let test_key = key.clone();
                        let present = computed({
                            let key = key.clone();
                            move || desired_keys.with(|keys| keys.contains(&key))
                        });
                        let is_first = computed({
                            let key = key.clone();
                            move || {
                                structure.with(|nodes| {
                                    nodes.first().map(TreeNode::key).as_deref()
                                        == Some(key.as_str())
                                })
                            }
                        });
                        let row_ref = element_ref();
                        let initial = present.get_untracked();
                        let opacity = animated_value(if initial { 1. } else { 0. });
                        let previous = stored((initial, canban_ui_kit::motion::reduced()));
                        effect(move || {
                            let visible = present.get();
                            let reduced = canban_ui_kit::motion::reduced();
                            if previous.replace((visible, reduced)) == (visible, reduced) {
                                return;
                            }
                            canban_ui_kit::motion::layout(row_ref, 160.);
                            let end = canban_ui_kit::motion::timing(
                                opacity,
                                if visible { 1. } else { 0. },
                                160.,
                                Easing::EaseOut,
                            );
                            let key = key.clone();
                            canban_ui_kit::motion::when_done([end], move || {
                                if !visible {
                                    retained.update(|rows| rows.retain(|n| n.key() != key));
                                }
                            });
                        });
                        view()
                            .test_id(&test_key)
                            .element_ref(&row_ref)
                            .overflow_hidden()
                            .h(move || {
                                px(if present.get() {
                                    node.get().height(ui, is_first.get())
                                } else {
                                    0.
                                })
                            })
                            .bind_opacity(&opacity)
                            .child(node_content(ui, node, indices, is_first))
                    },
                ))),
        )
}
