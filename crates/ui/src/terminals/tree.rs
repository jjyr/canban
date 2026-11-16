//! Unified Project -> Worktree -> Terminal hierarchy.
//! Only worktree scopes with >= 1 live Terminal are displayed; all active rows stay expanded.
use super::worktrees::{WorktreeScope, create_in_scope, entry_scope, scope_git_summary};
use super::*;
use crate::drafts::{capture_scroll, scroll_position};
use canban_types::Snapshot;

fn git_status_chips(git: &super::worktrees::ScopeGitSummary) -> View {
    let mut row = view().flex_row().items_center().gap_1().flex_shrink_0();
    if git.dirty {
        row = row
            .child(
                label(format!("+{}", git.additions))
                    .text_size(px(11.))
                    .font_family(MONO_FONT)
                    .text_color(SUCCESS),
            )
            .child(
                label(format!("−{}", git.deletions))
                    .text_size(px(11.))
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
        row = row.child(muted(ab.join(" ")).text_size(px(11.)));
    }
    row
}

fn scope_header(
    ui: Ui,
    s: &canban_types::Snapshot,
    scope: WorktreeScope,
    children: &[Entry],
) -> impl IntoElement + use<> {
    let git = scope_git_summary(s, &scope, children);
    let display = scope.display_name(s, children, Some(ui));
    let scope_for_active = children.first().map(super::worktrees::location_key);
    let target_terminal = children.first().map(|e| e.saved.id.clone());
    let launch_params = children.first().and_then(|e| e.location.as_ref().map(|l| json!({
        "host_id": e.host_id, "machine": l.machine, "cwd": l.directory_root.as_ref().or(l.cwd.as_ref()), "project_id": l.project_id
    })));
    let is_active_scope = computed(move || {
        let sel = ui.terminal_selection.get();
        if sel.is_empty() || ui.closing_terminals.with(|c| c.contains(&sel)) {
            return false;
        }
        {
            ui.terminal_entries.with(|rows| {
                rows.iter()
                    .find(|e| e.saved.id == sel)
                    .is_some_and(|e| Some(super::worktrees::location_key(e)) == scope_for_active)
            })
        }
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
            .on_press(move |_: &()| {
                if let Some(id) = &target {
                    ui.open_terminal(id.clone());
                }
            });

        let is_external = matches!(
            scope,
            WorktreeScope::Directory { project: None, .. } | WorktreeScope::Unassigned(_)
        );
        let mut outer = view();
        if !is_external {
            outer = outer.ml_2();
        }
        outer = outer
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
                move |_| {
                    if let Some(p) = &params {
                        super::launch_custom(ui, p.clone());
                    } else {
                        create_in_scope(ui, &scope_plus);
                    }
                },
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
        let host_label = if e.host_id == "local" {
            "Local".into()
        } else {
            crate::hosts::host_name(ui, &e.host_id)
        };
        let hint = super::labels::sidebar_hint(&e, &host_label);
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
            .child(super::labels::sidebar_meta(&e, &host_label));
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
    fn height(&self) -> f32 {
        match self {
            Self::Project(_) => 40.,
            Self::OtherHeader => 34.,
            Self::Scope(..) => 34.,
            Self::Terminal(_) => 48.,
            Self::Empty => 40.,
        }
    }
}

pub(super) fn project_tree<'a>(
    s: &Snapshot,
    rows: impl IntoIterator<Item = &'a Entry>,
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
        if let Some(pid) = pid {
            out.push(TreeNode::Project(pid));
        } else {
            out.push(TreeNode::OtherHeader);
        }
        for (key, (scope, children)) in scopes {
            out.push(TreeNode::Scope(key, scope));
            out.extend(children.into_iter().map(TreeNode::Terminal));
        }
    }
    if out.is_empty() {
        out.push(TreeNode::Empty);
    }
    out
}

fn other_heading() -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(34.))
        .min_w(px(0.))
        .px_2()
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

fn project_heading(ui: Ui, id: String) -> impl IntoElement {
    let name = computed({
        let id = id.clone();
        move || {
            ui.snapshot.with(|s| {
                s.projects
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.name.clone())
                    .unwrap_or_default()
            })
        }
    });
    let pid_wt = id.clone();
    view()
        .flex_row()
        .items_center()
        .h(px(40.))
        .min_w(px(0.))
        .px_2()
        .child(icon(Icon::Folder).w(px(14.)).h(px(14.)).flex_shrink_0())
        .child(
            label(move || name.get())
                .flex_1()
                .min_w(px(0.))
                .text_color(MUTED)
                .font_weight(FontWeight::SEMIBOLD)
                .truncate(),
        )
        .child(chrome_button_disabled(
            Icon::Branch,
            "New branch worktree",
            move || ui.pending.get(),
            move |_| ui.worktree_modal.set(Some(pid_wt.clone())),
        ))
        .child(chrome_button_disabled(
            Icon::Plus,
            "New terminal",
            move || ui.pending.get(),
            move |_| create(ui, id.clone(), None),
        ))
}

fn node_content(
    ui: Ui,
    node: ReadSignal<TreeNode>,
    indices: Memo<std::collections::BTreeMap<String, usize>>,
) -> AnyElement {
    match node.get_untracked() {
        TreeNode::Project(id) => project_heading(ui, id).into_element(),
        TreeNode::OtherHeader => other_heading().into_element(),
        TreeNode::Terminal(id) => {
            let id_ext = id.clone();
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
            let last_ext = stored(is_external.get_untracked());
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
                    }
                    last.get().map(|e| row(ui, e).into_element())
                }))
                .into_element()
        }
        TreeNode::Scope(key, _) => {
            let render_key = key.clone();
            // Only header presentation changes rebuild this header, not Agent status.
            let info = computed(move || {
                let TreeNode::Scope(_, scope) = node.get() else {
                    unreachable!()
                };
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
                        (
                            scope.clone(),
                            scope.display_name(s, &children, Some(ui)),
                            scope_git_summary(s, &scope, &children),
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
                let (scope, ..) = info.get();
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
                scope_header(ui, &s, scope, &children)
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
                    project_tree(
                        s,
                        rows.iter()
                            .filter(|e| e.listed() && !closing.contains(&e.saved.id)),
                    )
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
            let anchor = before.iter().find_map(|row| {
                let start = top;
                top += row.height();
                (top > offset.y.0 && next.iter().any(|n| n.key() == row.key()))
                    .then(|| (row.key(), start))
            });
            if let Some((key, start)) = anchor {
                let new_top: f32 = next
                    .iter()
                    .take_while(|n| n.key() != key)
                    .map(TreeNode::height)
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
                .px_2()
                .pb_2()
                .child(label("Terminals").font_weight(FontWeight::SEMIBOLD)),
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
                        let row_ref = element_ref();
                        let opacity = animated_value(if canban_ui_kit::motion::reduced() {
                            1.
                        } else {
                            0.
                        });
                        let generation = stored(0u64);
                        effect(move || {
                            let visible = present.get();
                            let reduced = canban_ui_kit::motion::reduced();
                            let ticket = generation.get().wrapping_add(1);
                            generation.replace(ticket);
                            canban_ui_kit::motion::layout(row_ref, 160.);
                            let end = opacity.timing(
                                if visible { 1. } else { 0. },
                                if reduced { 0. } else { 160. },
                                Easing::EaseOut,
                            );
                            let key = key.clone();
                            spawn(async move {
                                if end.await && generation.get() == ticket && !visible {
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
                                    node.get().height()
                                } else {
                                    0.
                                })
                            })
                            .bind_opacity(&opacity)
                            .child(node_content(ui, node, indices))
                    },
                ))),
        )
}
