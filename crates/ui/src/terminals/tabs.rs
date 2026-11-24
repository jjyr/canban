//! Window chrome and explicit actions. Menus carry secondary runtime details.

use super::*;
use crate::components::git_workspace::compact;
use nsh_core::element::desktop::{MenuItem, current_window, popup_menu, popup_menu_at};
pub fn context_updates(ui: Ui) {
    // Regrouping only reveals the selected row. It never changes the tab/viewer or unfolds
    // an intentionally collapsed scope when switching terminals from the header/tabs.
    let group = computed(move || {
        current(ui).map(|e| {
            let loc = super::worktrees::location_key(&e);
            let pid = ui
                .snapshot
                .with(|s| super::worktrees::entry_scope(s, &e).project_id(s));
            (e.saved.id, pid, loc)
        })
    });
    let prev_group = stored(group.get_untracked());
    effect(move || {
        let next = group.get();
        let prev = prev_group.replace(next.clone());
        if let (Some((prev_id, prev_pid, prev_loc)), Some((cur_id, cur_pid, cur_loc))) =
            (prev, next)
            && prev_id == cur_id
            && (prev_pid != cur_pid || prev_loc != cur_loc)
        {
            reveal_group(ui, &cur_id);
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
        if !matches!(ui.page.get().as_str(), "terminals" | "terminal") {
            return;
        }
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
        let terminal = e.saved.id.clone();
        items.push(
            MenuItem::new("Open Card")
                .on_select(move || ui.terminal_card(card.clone(), Some(terminal.clone()))),
        );
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
fn tab(ui: Ui, e: Entry, max_tab_w: Memo<f32>, show_card_number: bool) -> impl IntoElement {
    let id = e.saved.id.clone();
    let id_sel = id.clone();
    let is_selected = computed(move || ui.terminal_selection.get() == id_sel);
    let base_width = super::labels::tab_width(&e, show_card_number);
    dynamic(move || {
        let selected = is_selected.get();
        let id = id.clone();
        let close = id.clone();
        let context = id.clone();
        let rename = id.clone();
        let hint = e.hint_with_policy(ui.idle_minutes.get());
        let managed = e.is_managed();
        let has_reclaim_badge = e.is_reclaiming_soon() || e.is_auto_reclaiming();
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
        if !has_reclaim_badge && let Some(mark_el) = super::labels::status_mark(&e) {
            tab_button = tab_button.child(mark_el);
        }
        tab_button = tab_button
            .child(super::labels::tab(&e, selected, show_card_number))
            .on_press_down(move |event: &nsh_core::PressEvent| {
                if event.clicks >= 2 {
                    ui.terminal_rename.set(Some(rename.clone()));
                }
            })
            .on_context_menu(move |event: &nsh_core::ContextMenuEvent| {
                ui.spawn(menu(ui, context.clone(), Some(event.page_position)));
            })
            .on_press(move |_: &nsh_core::PressEvent| ui.select_terminal_in_scope(id.clone()));
        let bg = if selected {
            SELECTED
        } else if managed {
            HIGHLIGHT_SURFACE
        } else {
            CLEAR
        };
        let mut cell = view()
            .relative()
            .w(move || px(base_width.min(max_tab_w.get())))
            .h_full()
            .flex_shrink_0()
            .window_drag(false)
            .flex_row()
            .items_center()
            .gap_0()
            .px_1()
            .bg(bg)
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
        if managed {
            cell = cell.child(
                view()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .h(px(1.5))
                    .bg(HIGHLIGHT)
                    .pointer_events(PointerEvents::None),
            );
        }
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
pub fn header(ui: Ui) -> impl IntoElement {
    let active_scope = computed(move || {
        let cur = current(ui)?;
        Some(ui.snapshot.with(|s| super::worktrees::entry_scope(s, &cur)))
    });
    let show_card_number = computed(move || {
        !matches!(
            active_scope.get(),
            Some(super::worktrees::WorktreeScope::CardWorktree(_))
        )
    });
    let inventory = computed(move || {
        let Some(target_scope) = active_scope.get() else {
            return Vec::new();
        };
        let show_num = show_card_number.get();
        ui.snapshot.with(|s| {
            ui.closing_terminals.with(|closing| {
                ui.terminal_entries.with(|rows| {
                    rows.iter()
                        .filter(|e| {
                            e.listed()
                                && !closing.contains(&e.saved.id)
                                && super::worktrees::entry_scope(s, e) == target_scope
                        })
                        .map(|e| {
                            let idle_minutes = ui.idle_minutes.get();
                            (
                                e.saved.id.clone(),
                                super::labels::tab_label_text(e, show_num),
                                e.hint_with_policy(idle_minutes),
                                e.ended(),
                                e.inactive(),
                                e.herdr_status(),
                                e.attention_reason(),
                                e.provider(),
                                e.is_managed(),
                                e.reclaim.clone(),
                            )
                        })
                        .collect::<Vec<_>>()
                })
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
                                c.additions,
                                c.deletions,
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
    let max_tab_w = computed(move || (width.get() * 0.5).max(80.));
    let strip = element_ref();
    let active = computed(move || ui.terminal_selection.get());
    effect(move || {
        let selected = active.get();
        let tabs = super::scope_tabs(ui);
        let cap = max_tab_w.get();
        let show_num = show_card_number.get();
        let _ = inventory.get();
        let mut offset = 0.;
        for e in &tabs {
            if e.saved.id == selected {
                break;
            }
            offset += super::labels::tab_width(e, show_num).min(cap);
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
                (crate::frame::chrome_left_padding() - 56.).max(6.)
            })
        })
        .pr(move || px(crate::frame::chrome_right_padding()))
        .child(when(
            move || !ui.terminal_sidebar.get(),
            move || super::controls(ui),
        ))
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .overflow_hidden()
                .flex_row()
                .items_center()
                .gap_1()
                .window_drag(true)
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .h_full()
                        .overflow_hidden()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .window_drag(true)
                        .child(
                            scroll()
                                .horizontal()
                                .scrollbar(false)
                                .element_ref(&strip)
                                .flex_shrink(1.)
                                .min_w(px(0.))
                                .h_full()
                                .child(dynamic(move || {
                                    let _ = inventory.get();
                                    let show_num = show_card_number.get_untracked();
                                    let tabs = untracked(|| super::scope_tabs(ui));
                                    let mut strip =
                                        view().h_full().flex_row().items_center().window_drag(true);
                                    for e in tabs {
                                        strip = strip.child(tab(ui, e, max_tab_w, show_num));
                                    }
                                    strip
                                })),
                        )
                        .child(chrome_button_disabled(
                            Icon::Plus,
                            format!(
                                "New terminal in current worktree · {}",
                                crate::commands::shortcuts::NEW_TERMINAL_DISPLAY
                            ),
                            move || ui.pending.get(),
                            move |_| new_tab(ui),
                        ))
                        .child(
                            action("▾", Variant::Quiet, move |_| machine_menu(ui))
                                .flex_shrink_0()
                                .accessibility_label(|| {
                                    Some("Choose terminal worktree or machine".into())
                                }),
                        )
                        .child(chrome_button_disabled(
                            Icon::Agent,
                            "Open Agent",
                            move || ui.pending.get(),
                            move |_| open_ephemeral_agent(ui),
                        ))
                        .child(view().flex_1().min_w(px(0.)).h_full().window_drag(true)),
                )
                .child(dynamic(move || {
                    let _ = context.get();
                    let s = ui.snapshot.get_untracked();
                    let cur = untracked(|| current(ui));
                    let scope_tabs_now = untracked(|| super::scope_tabs(ui));
                    let more_anchor = element_ref();

                    let mut context_bar = view()
                        .window_drag(false)
                        .flex_row()
                        .items_center()
                        .justify_end()
                        .gap_1p5()
                        .min_w(px(0.))
                        .max_w(move || px((width.get() * 0.35).min(340.)))
                        .overflow_hidden()
                        .flex_shrink(1.);

                    if let Some(ref e) = cur {
                        if let Some(card) = e.card.clone() {
                            let terminal = e.saved.id.clone();
                            context_bar = context_bar.child(
                                view()
                                    .flex_shrink(1.)
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
                                                    .text_color(if e.inferred_card {
                                                        MUTED
                                                    } else {
                                                        TEXT
                                                    })
                                                    .w_full()
                                                    .truncate(),
                                            )
                                            .accessibility_label(move || {
                                                Some("Open current Card".into())
                                            })
                                            .on_press(move |_: &()| {
                                                ui.terminal_card(
                                                    card.id.clone(),
                                                    Some(terminal.clone()),
                                                )
                                            }),
                                    ),
                            );
                        } else if let Some(project) = e.project_id.clone() {
                            let proj_label = s.project_display_name(&project);
                            context_bar = context_bar.child(
                                canban_ui_kit::press_target()
                                    .flex_shrink(1.)
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .h(px(30.))
                                    .justify_center()
                                    .px_2()
                                    .rounded(px(5.))
                                    .bg(CLEAR)
                                    .hover(|st| st.bg(SURFACE))
                                    .focus(|st| st.border_1().border_color(ACCENT))
                                    .child(label(proj_label).w_full().truncate())
                                    .accessibility_label(move || {
                                        Some("Open current Project".into())
                                    })
                                    .on_press(move |_: &()| ui.select_project(project.clone())),
                            );
                        } else if let Some(cwd) = e.cwd() {
                            let title = super::worktrees::display_directory_path(&cwd, &e.host_id);
                            let host_suffix = if s.has_remote_hosts() {
                                format!(" · {}", s.host_display_name(&e.host_id))
                            } else {
                                String::new()
                            };
                            let display = format!(
                                "{}{host_suffix}",
                                compact(&title, if ui.narrow.get() { 14 } else { 32 })
                            );
                            context_bar = context_bar.child(
                                view()
                                    .flex_shrink(1.)
                                    .min_w(px(0.))
                                    .h(px(30.))
                                    .justify_center()
                                    .px_2()
                                    .overflow_hidden()
                                    .tooltip(
                                        hint_tooltip(format!(
                                            "{}\nHost: {}",
                                            e.cwd().unwrap_or_default(),
                                            s.host_display_name(&e.host_id)
                                        ))
                                        .placement(Placement::Bottom),
                                    )
                                    .child(muted(display).w_full().truncate()),
                            );
                        }
                    }

                    // Compare the observed directory; workspace selection belongs to the tree.
                    if let Some(ref e) = cur {
                        let scope = super::worktrees::entry_scope(&s, e);
                        let git = super::worktrees::scope_git_summary(&s, &scope, &scope_tabs_now);
                        let branch = e.location.as_ref().and_then(|l| l.branch.clone());
                        let target = super::worktrees::compare_target(&s, e);
                        let hint = match &target {
                    Some(_) => format!(
                        "View changes in Git\nBranch: {}\nDirectory: {}\nWorking tree: {}",
                        branch.as_deref().unwrap_or("Detached HEAD"),
                        e.cwd().unwrap_or_default(),
                        git.tooltip_indicators().join(" · "),
                    ),
                    None => {
                        "Changes unavailable: no Git checkout could be resolved for this terminal"
                            .into()
                    }
                };
                        let limit = if ui.narrow.get() { 10 } else { 18 };
                        let mut pill = canban_ui_kit::press_target()
                            .h(px(30.))
                            .max_w(move || px(if ui.narrow.get() { 160. } else { 250. }))
                            .min_w(px(0.))
                            .overflow_hidden()
                            .px_2()
                            .rounded(px(5.))
                            .flex_row()
                            .gap_1()
                            .items_center()
                            .bg(CLEAR)
                            .hover(|st| st.bg(SURFACE))
                            .focus(|st| st.border_1().border_color(ACCENT))
                            .disabled(target.is_none())
                            .accessibility_label(|| Some("View terminal changes in Git".into()))
                            .child(icon(Icon::Branch).w(px(13.)).h(px(13.)).flex_shrink_0())
                            .child(
                                muted(compact(
                                    branch.as_deref().unwrap_or("No Git checkout"),
                                    limit,
                                ))
                                .flex_shrink(1.)
                                .min_w(px(0.))
                                .truncate(),
                            );
                        if git.dirty {
                            pill = pill
                                .child(
                                    label(format!("+{}", git.additions))
                                        .text_size(px(11.))
                                        .font_family(MONO_FONT)
                                        .text_color(SUCCESS)
                                        .flex_shrink_0(),
                                )
                                .child(
                                    label(format!("−{}", git.deletions))
                                        .text_size(px(11.))
                                        .font_family(MONO_FONT)
                                        .text_color(DANGER)
                                        .flex_shrink_0(),
                                );
                        }
                        for (count, arrow) in [(git.ahead, "↑"), (git.behind, "↓")] {
                            if count > 0 {
                                pill = pill.child(
                                    muted(format!("{arrow}{count}"))
                                        .text_size(px(11.))
                                        .flex_shrink_0(),
                                );
                            }
                        }
                        pill = pill.on_press(move |_: &()| {
                            if let Some(target) = &target {
                                super::worktrees::open_compare_target(ui, target);
                            }
                        });
                        context_bar = context_bar.child(
                            view()
                                .flex_shrink(1.)
                                .min_w(px(0.))
                                .tooltip(hint_tooltip(hint).placement(Placement::Bottom))
                                .child(pill),
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
                })),
        )
        .child(
            view()
                .flex_shrink_0()
                .flex_row()
                .items_center()
                .gap_1()
                .window_drag(false)
                .child(crate::hosts::trigger_pill(ui))
                .child(crate::overview::trigger_pill(ui)),
        )
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
                if !crate::backend::is_connected() {
                    ui.snapshot.update(|s| {
                        let trimmed = next.trim();
                        if !trimmed.is_empty() {
                            if let Some(t) = s.terminals.iter_mut().find(|t| t.id == id) {
                                t.name = Some(trimmed.into());
                            }
                        } else if let Some(target) =
                            s.terminals.iter().find(|t| t.id == id).cloned()
                        {
                            let replacement = if target.conversation_id.is_none() {
                                let pid = s.terminal_project_id(&target).map(str::to_owned);
                                Some(s.next_terminal_label(
                                    pid.as_deref(),
                                    target.agent_engine.as_deref(),
                                    Some(&target.id),
                                ))
                            } else {
                                None
                            };
                            if let Some(t) = s.terminals.iter_mut().find(|t| t.id == id) {
                                t.name = replacement;
                            }
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
