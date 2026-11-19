//! Unified Project -> Worktree -> Terminal hierarchy.
//! Only worktree scopes with >= 1 live Terminal are displayed; Project and Worktree levels are foldable.
use super::worktrees::{
    WorktreeScope, create_in_scope, entry_scope, scope_git_summary, scope_resolution_label,
};
use super::*;
use crate::drafts::{capture_scroll, scroll_position};
use canban_types::Snapshot;

pub(crate) fn project_fold_key(project_id: &str) -> String {
    format!("project:{project_id}")
}

pub(crate) fn scope_fold_key(location_key: &str) -> String {
    format!("scope:{location_key}")
}

fn tree_icon_button(
    kind: Icon,
    title: impl Into<String>,
    disabled: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> View {
    let title = title.into();
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
                .accessibility_label(move || Some(title.clone().into()))
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

fn scope_header(
    ui: Ui,
    s: &canban_types::Snapshot,
    scope: WorktreeScope,
    render_key: String,
    children: &[Entry],
) -> impl IntoElement + use<> {
    let git = scope_git_summary(s, &scope, children);
    let display = scope.display_name(s, children, Some(ui));
    let resolution_label = scope_resolution_label(s, &scope);
    let scope_for_active = children.first().map(super::worktrees::location_key);
    let recent_key = scope.recent_key();
    let recent_target = ui.terminal_recent.with(|m| m.get(&recent_key).cloned());
    let target_terminal = recent_target
        .filter(|rid| children.iter().any(|e| e.saved.id == *rid))
        .or_else(|| children.first().map(|e| e.saved.id.clone()));
    let launch_params = children.first().and_then(|e| e.location.as_ref().map(|l| json!({
        "host_id": e.host_id, "machine": l.machine, "cwd": l.directory_root.as_ref().or(l.cwd.as_ref()), "project_id": l.project_id
    })));
    let is_active_scope = computed(move || {
        let sel = ui.terminal_selection.get();
        if sel.is_empty() || ui.closing_terminals.with(|c| c.contains(&sel)) {
            return false;
        }
        ui.terminal_entries.with(|rows| {
            rows.iter()
                .find(|e| e.saved.id == sel)
                .is_some_and(|e| Some(super::worktrees::location_key(e)) == scope_for_active)
        })
    });
    let fold_key = scope_fold_key(&render_key);
    let is_folded = computed({
        let fold_key = fold_key.clone();
        move || ui.terminal_folds.with(|folds| folds.contains(&fold_key))
    });
    let summary = super::labels::group_status_summary(children.iter());

    let (lead_icon, card_badge, mut tooltip_text) = match &scope {
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

    let managed_wt = match &scope {
        WorktreeScope::ManagedWorktree(wid) => s.worktrees.iter().find(|w| &w.id == wid).cloned(),
        _ => None,
    };
    let scope_card = match &scope {
        WorktreeScope::CardWorktree(cid) => s.cards.iter().find(|c| &c.id == cid).cloned(),
        _ => None,
    };
    let hovered = signal(false);

    dynamic(move || {
        let active = is_active_scope.get();
        let folded = is_folded.get();
        let target = target_terminal.clone();
        let params = launch_params.clone();
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
        let fold_toggle = fold_key.clone();
        let chev_label = display_label.clone();

        let has_git = git_now.dirty || git_now.ahead > 0 || git_now.behind > 0;
        let stacked_git = resolution_label.is_some() && has_git && ui.terminal_width.get() < 340.;
        let mut title_row = view()
            .w_full()
            .min_w(px(0.))
            .flex_row()
            .items_center()
            .gap_1()
            .child(
                icon_tinted(lead_icon, if active { ACCENT } else { MUTED })
                    .w(px(12.))
                    .h(px(12.))
                    .flex_shrink_0(),
            )
            .child(
                label(display_label.clone())
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(11.5))
                    .text_color(MUTED)
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .truncate(),
            );
        if let Some(b) = badge {
            title_row = title_row.child(muted(b).text_size(px(10.5)).flex_shrink_0());
        }
        if resolution_label.is_some() {
            title_row = title_row.child(
                status("Resolved", Tone::Highlight)
                    .px_1()
                    .py(px(1.))
                    .flex_shrink_0(),
            );
        }
        if has_git && !stacked_git {
            title_row = title_row.child(git_status_chips(&git_now));
        }
        if folded && let Some(badge_el) = super::labels::group_summary_badge(summary) {
            title_row = title_row.child(badge_el);
        }

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
                Some(
                    if folded {
                        format!("Expand worktree {chev_label}")
                    } else {
                        format!("Collapse worktree {chev_label}")
                    }
                    .into(),
                )
            })
            .child(
                icon_tinted(
                    if folded {
                        Icon::Forward
                    } else {
                        Icon::ChevronDown
                    },
                    MUTED,
                )
                .w(px(10.))
                .h(px(10.)),
            )
            .on_press(move |_: &()| {
                ui.terminal_folds.update(|folds| {
                    if !folds.remove(&fold_toggle) {
                        folds.insert(fold_toggle.clone());
                    }
                });
            });

        let mut btn_row = canban_ui_kit::press_target()
            .w_full()
            .min_w(px(0.))
            .min_h(px(26.))
            .px_1()
            .rounded(px(5.))
            .bg(CLEAR)
            .focus(|st| st.border_1().border_color(ACCENT))
            .flex_col()
            .justify_center()
            .py(px(if stacked_git { 2. } else { 0. }))
            .child(title_row);
        if stacked_git {
            btn_row = btn_row.child(view().pl(px(16.)).child(git_status_chips(&git_now)));
        }
        btn_row = btn_row
            .accessibility_label({
                let lbl = match &resolution_label {
                    Some(resolved) => format!("{display_label} · {resolved}"),
                    None => display_label.clone(),
                };
                let summary_desc = folded.then(|| super::labels::group_summary_a11y(summary));
                move || {
                    Some(
                        match &summary_desc {
                            Some(s) => format!("Worktree {lbl} · Collapsed · {s}"),
                            None => format!("Worktree {lbl}"),
                        }
                        .into(),
                    )
                }
            })
            .on_press(move |_: &()| {
                if let Some(id) = &target {
                    ui.select_terminal_in_scope(id.clone());
                }
            });

        let is_external = matches!(
            scope,
            WorktreeScope::Directory { project: None, .. } | WorktreeScope::Unassigned(_)
        );
        let mut outer = view().relative().mt(px(2.));
        if is_external {
            outer = outer.ml(px(4.));
        } else {
            outer = outer.ml(px(12.));
        }
        outer = outer
            .flex_row()
            .items_center()
            .min_w(px(0.))
            .rounded(px(5.))
            .bg(CLEAR)
            .hover(|st| st.bg(INSET))
            .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
            .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
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
            .child(chevron_btn)
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.))
                    .tooltip(hint_tooltip(tip).placement(Placement::Right))
                    .child(btn_row),
            )
            .child(when(
                move || hovered.get(),
                move || {
                    let plus_params = params.clone();
                    let plus_scope = scope_plus.clone();
                    let plus_label = format!("New terminal in {display_label}");
                    let mut overlay = view()
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
                            plus_label,
                            move || ui.pending.get(),
                            move |_| {
                                if let Some(p) = &plus_params {
                                    super::launch_custom(ui, p.clone());
                                } else {
                                    create_in_scope(ui, &plus_scope);
                                }
                            },
                        ));
                    if let Some(wt) = wt_more.clone() {
                        let branch = wt.branch.clone();
                        overlay = overlay.child(
                            tree_icon_button(
                                Icon::More,
                                format!("Worktree actions for {branch}"),
                                || false,
                                move |_| {
                                    let item = wt.clone();
                                    ui.spawn(async move {
                                        let pos = more_anchor.measure().await.map(|b| {
                                            point(b.page.x, b.page.y + b.frame.size.height)
                                        });
                                        super::worktrees::menu(ui, item, pos).await;
                                    });
                                },
                            )
                            .element_ref(&more_anchor),
                        );
                    } else if let Some(card) = card_more.clone() {
                        let num = card.number;
                        overlay = overlay.child(
                            tree_icon_button(
                                Icon::More,
                                format!("Card worktree actions for #{num}"),
                                || false,
                                move |_| {
                                    let item = card.clone();
                                    ui.spawn(async move {
                                        let pos = more_anchor.measure().await.map(|b| {
                                            point(b.page.x, b.page.y + b.frame.size.height)
                                        });
                                        super::worktrees::card_scope_menu(ui, item, pos).await;
                                    });
                                },
                            )
                            .element_ref(&more_anchor),
                        );
                    }
                    overlay
                },
            ));

        outer
    })
}
fn row(
    ui: Ui,
    e: Entry,
    in_own_card_worktree: bool,
    host_in_scope_header: bool,
) -> impl IntoElement {
    let id = e.saved.id.clone();
    let id_sel = id.clone();
    let is_selected = computed(move || ui.terminal_selection.get() == id_sel);
    let hovered = signal(false);
    dynamic(move || {
        let selected = is_selected.get();
        let id = id.clone();
        let close_id = id.clone();
        let context_id = id.clone();
        let host_label = if e.host_id == "local" {
            "Local".into()
        } else {
            crate::hosts::host_name(ui, &e.host_id)
        };
        let hint = super::labels::sidebar_hint(&e, &host_label);
        let row_title = super::labels::sidebar_row_title(&e, in_own_card_worktree);
        let lead_icon = super::labels::sidebar_meta_icon(&e);
        let wide_row = ui.terminal_width.get() >= 230.;
        let host_badge = (wide_row && !host_in_scope_header)
            .then(|| super::labels::sidebar_host_badge(&e, &host_label))
            .flatten();
        let status_el = super::labels::sidebar_status_compact(&e, wide_row);
        let close_label = format!("Close terminal {}", e.title());

        let mut line = view()
            .w_full()
            .min_w(px(0.))
            .flex_row()
            .items_center()
            .gap_1p5()
            .child(icon(lead_icon).w(px(14.)).h(px(14.)).flex_shrink_0())
            .child(
                label(row_title)
                    .text_size(px(13.))
                    .text_color(TEXT)
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .flex_1()
                    .min_w(px(0.))
                    .truncate(),
            );
        if !hovered.get() {
            if let Some(host) = host_badge {
                line = line.child(
                    muted(format!("· {host}"))
                        .text_size(px(10.5))
                        .max_w(px(72.))
                        .truncate()
                        .flex_shrink_0(),
                );
            }
            if let Some(st) = status_el {
                line = line.child(st);
            }
        }

        view()
            .relative()
            .flex_row()
            .items_center()
            .min_w(px(0.))
            .h(px(32.))
            .my(px(1.))
            .rounded(px(6.))
            .bg(if selected { SELECTED } else { CLEAR })
            .hover(move |s| s.bg(if selected { SELECTED } else { INSET }))
            .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
            .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
            .on_context_menu(move |event: &nsh_core::ContextMenuEvent| {
                ui.spawn(super::tabs::menu(
                    ui,
                    context_id.clone(),
                    Some(event.page_position),
                ));
            })
            .tooltip(hint_tooltip(hint.clone()).placement(Placement::Right))
            .child(when(
                move || selected,
                || {
                    view()
                        .absolute()
                        .left(px(0.))
                        .top(px(7.))
                        .bottom(px(7.))
                        .w(px(2.5))
                        .rounded(px(2.))
                        .bg(ACCENT)
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
                    .selected(move || selected)
                    .accessibility_label(move || Some(hint.clone().into()))
                    .child(line)
                    .on_press(move |_: &()| ui.open_terminal(id.clone())),
            )
            .child(when(
                move || hovered.get(),
                move || {
                    let close_target = close_id.clone();
                    view().pr_1().child(tree_icon_button(
                        Icon::Close,
                        close_label.clone(),
                        move || ui.pending.get(),
                        move |_| super::close_entry(ui, &close_target),
                    ))
                },
            ))
    })
}
/// Flat keys allow a Terminal row to move between parents without remounting.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum TreeNode {
    Project(String),
    OtherHeader,
    Scope(String, WorktreeScope),
    Terminal(String),
    Empty,
}
impl TreeNode {
    fn key(&self) -> String {
        match self {
            Self::Project(id) => format!("project:{id}"),
            Self::OtherHeader => "section:other".into(),
            Self::Scope(key, _) => key.clone(),
            Self::Terminal(id) => format!("terminal:{id}"),
            Self::Empty => "empty".into(),
        }
    }
    fn height(&self, ui: Ui, is_first: bool) -> f32 {
        match self {
            Self::Project(_) => {
                if is_first {
                    28.
                } else {
                    38.
                }
            }
            Self::OtherHeader => 34.,
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
    let mut groups = std::collections::BTreeMap::<
        Option<String>,
        std::collections::BTreeMap<String, (WorktreeScope, Vec<String>)>,
    >::new();
    for e in rows {
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
    });
}

pub(crate) fn expand_all_worktrees(ui: Ui) {
    ui.terminal_folds.update(|folds| folds.clear());
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
                (folded, name, summary)
            })
        }
    });

    dynamic(move || {
        let (folded, proj_name, summary) = info.get();
        let first = is_first.get();
        let pid_wt = id.clone();
        let pid_new = id.clone();
        let pid_ctx_new = id.clone();
        let pid_ctx_wt = id.clone();
        let fold_toggle = fold_key.clone();
        let fold_ctx = fold_key.clone();
        let ctx_name = proj_name.clone();
        let a11y_name = proj_name.clone();
        let mut header_btn = canban_ui_kit::press_target()
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
                Some(
                    match summary {
                        Some(sum) => format!(
                            "Project {a11y_name} · Collapsed · {}",
                            super::labels::group_summary_a11y(sum)
                        ),
                        None => format!("Project {a11y_name}"),
                    }
                    .into(),
                )
            })
            .child(
                icon_tinted(
                    if folded {
                        Icon::Forward
                    } else {
                        Icon::ChevronDown
                    },
                    MUTED,
                )
                .w(px(10.))
                .h(px(10.))
                .flex_shrink_0(),
            )
            .child(
                label(proj_name.to_uppercase())
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(11.))
                    .text_color(MUTED)
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate(),
            );
        if let Some(sum) = summary
            && let Some(badge) = super::labels::group_summary_badge(sum)
        {
            header_btn = header_btn.child(badge);
        }
        header_btn = header_btn.on_press(move |_: &()| {
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
                let pid_new = pid_ctx_new.clone();
                let pid_wt = pid_ctx_wt.clone();
                let fold_key = fold_ctx.clone();
                let name = ctx_name.clone();
                let at = ev.page_position;
                ui.spawn(async move {
                    let toggle_label = if folded {
                        format!("Expand {name}")
                    } else {
                        format!("Collapse {name}")
                    };
                    let items = vec![
                        MenuItem::new(format!("New Terminal in {name}"))
                            .on_select(move || create(ui, pid_new.clone(), None)),
                        MenuItem::new("New Managed Branch Worktree…")
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
                            move || ui.pending.get(),
                            move |_| ui.worktree_modal.set(Some(pid_wt.clone())),
                        ))
                        .child(tree_icon_button(
                            Icon::Plus,
                            "New terminal",
                            move || ui.pending.get(),
                            move |_| create(ui, pid_new.clone(), None),
                        ))
                },
            ));

        let mut container = view().flex_col().min_w(px(0.));
        if !first {
            container = container.child(
                view()
                    .h(px(10.))
                    .px_1()
                    .justify_center()
                    .child(view().h(px(1.)).bg(BORDER)),
            );
        }
        container.child(row)
    })
}

fn node_content(
    ui: Ui,
    node: ReadSignal<TreeNode>,
    indices: Memo<std::collections::BTreeMap<String, usize>>,
    is_first: Memo<bool>,
) -> AnyElement {
    match node.get_untracked() {
        TreeNode::Project(id) => project_heading(ui, id, is_first).into_element(),
        TreeNode::OtherHeader => other_heading().into_element(),
        TreeNode::Terminal(id) => {
            let id_ext = id.clone();
            let id_own = id.clone();
            let id_host = id.clone();
            let entry = computed(move || {
                ui.terminal_entries.with(|rows| {
                    indices.with(|map| map.get(&id).and_then(|i| rows.get(*i)).cloned())
                })
            });
            let last = stored(entry.get_untracked());
            let is_external = computed(move || {
                ui.snapshot.with(|s| {
                    ui.terminal_entries.with(|rows| {
                        indices
                            .with(|map| map.get(&id_ext).and_then(|i| rows.get(*i)))
                            .is_some_and(|e| entry_scope(s, e).project_id(s).is_none())
                    })
                })
            });
            let in_own_card_wt = computed(move || {
                ui.snapshot.with(|s| {
                    ui.terminal_entries.with(|rows| {
                        indices
                            .with(|map| map.get(&id_own).and_then(|i| rows.get(*i)))
                            .is_some_and(|e| {
                                e.card.as_ref().is_some_and(|c| {
                                    entry_scope(s, e) == WorktreeScope::CardWorktree(c.id.clone())
                                })
                            })
                    })
                })
            });
            let host_in_scope = computed(move || {
                ui.snapshot.with(|s| {
                    ui.terminal_entries.with(|rows| {
                        indices
                            .with(|map| map.get(&id_host).and_then(|i| rows.get(*i)))
                            .is_some_and(|e| {
                                matches!(entry_scope(s, e), WorktreeScope::Unassigned(_))
                            })
                    })
                })
            });
            let last_ext = stored(is_external.get_untracked());
            let last_own = stored(in_own_card_wt.get_untracked());
            let last_host_in_scope = stored(host_in_scope.get_untracked());
            view()
                .ml(move || {
                    if entry.get().is_some() {
                        last_ext.replace(is_external.get());
                    }
                    px(if last_ext.get() { 12. } else { 20. })
                })
                .pl_2()
                .border_l_1()
                .border_color(BORDER)
                .child(dynamic(move || {
                    if let Some(e) = entry.get() {
                        last.replace(Some(e));
                        last_own.replace(in_own_card_wt.get());
                        last_host_in_scope.replace(host_in_scope.get());
                    }
                    last.get().map(|e| {
                        row(ui, e, last_own.get(), last_host_in_scope.get()).into_element()
                    })
                }))
                .into_element()
        }
        TreeNode::Scope(key, _) => {
            let render_key = key.clone();
            let fold_key = scope_fold_key(&key);
            // Expanded headers skip Agent status ticks; collapsed headers track GroupStatusSummary.
            let info = computed(move || {
                let TreeNode::Scope(_, scope) = node.get() else {
                    unreachable!()
                };
                let folded = ui.terminal_folds.with(|folds| folds.contains(&fold_key));
                ui.snapshot.with(|s| {
                    ui.terminal_entries.with(|rows| {
                        let closing = ui.closing_terminals.get();
                        let children: Vec<_> = rows
                            .iter()
                            .filter(|e| {
                                e.listed()
                                    && !closing.contains(&e.saved.id)
                                    && super::worktrees::location_key(e) == key
                            })
                            .cloned()
                            .collect();
                        let summary =
                            folded.then(|| super::labels::group_status_summary(children.iter()));
                        (
                            folded,
                            summary,
                            scope.clone(),
                            scope.display_name(s, &children, Some(ui)),
                            scope_git_summary(s, &scope, &children),
                            scope_resolution_label(s, &scope),
                            children
                                .iter()
                                .map(|e| e.saved.id.clone())
                                .collect::<Vec<_>>(),
                            match &scope {
                                WorktreeScope::CardWorktree(id) => s
                                    .cards
                                    .iter()
                                    .find(|c| &c.id == id)
                                    .map(|c| (c.number, c.title.clone())),
                                _ => None,
                            },
                        )
                    })
                })
            });
            dynamic(move || {
                let (_, _, scope, ..) = info.get();
                let s = ui.snapshot.get_untracked();
                let rows = ui.terminal_entries.get_untracked();
                let closing = ui.closing_terminals.get_untracked();
                let children: Vec<_> = rows
                    .into_iter()
                    .filter(|e| {
                        e.listed()
                            && !closing.contains(&e.saved.id)
                            && super::worktrees::location_key(e) == render_key
                    })
                    .collect();
                scope_header(ui, &s, scope, render_key.clone(), &children)
            })
            .into_element()
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
                })),
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
