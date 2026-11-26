//! Embedded Live Terminal & Agent pane inside the side-by-side Card workspace (`⌘J`).
//!
//! Shares the same native `canban/terminal` viewer and Herdr PTY attachment as
//! `Terminals`, while guaranteeing at most one mounted viewer at a time (`page == "card"`).
//! Hiding this pane detaches the viewer without terminating any PTY.
use super::*;
use crate::terminals::{
    Entry, card_live_entries_for_ui, default_card_terminal_for_ui, select_card_tab,
};
use serde_json::json;

pub fn select_entry_for_card(ui: Ui, card_id: &str) -> Option<Entry> {
    let live = card_live_entries_for_ui(ui, card_id);
    if live.is_empty() {
        return None;
    }
    let selected = ui.card_tab.get();
    if !selected.is_empty()
        && let Some(e) = live.iter().find(|e| e.saved.id == selected)
    {
        return Some(e.clone());
    }
    let recent = ui.terminal_recent.with(|m| m.get(card_id).cloned());
    let fallback = default_card_terminal_for_ui(ui, card_id, recent.as_deref())?;
    live.into_iter().find(|e| e.saved.id == fallback)
}

pub fn toggle(ui: Ui) {
    if ui.page.get_untracked() != "card" || ui.card.get_untracked().is_empty() {
        return;
    }
    if ui.card_terminal_open.get_untracked() {
        batch(|| {
            ui.card_terminal_open.set(false);
            ui.card_terminal_focused.set(false);
        });
        return;
    }
    let Some(card) = ui.current_card() else {
        return;
    };
    let recent = ui.terminal_recent.with(|m| m.get(&card.id).cloned());
    if let Some(id) = untracked(|| default_card_terminal_for_ui(ui, &card.id, recent.as_deref())) {
        batch(|| {
            select_card_tab(ui, &card.id, id);
            ui.card_terminal_open.set(true);
            ui.card_terminal_focused.set(true);
        });
        return;
    }
    if card.archived_at.is_some() {
        ui.operation_error
            .set("Restore this Card before opening a new terminal.".into());
        return;
    }
    if card.assignee.is_some() {
        crate::terminals::open_card_agent(ui, &card.id, None);
    } else {
        crate::terminals::create_in_card(ui, card.project_id, card.id, false);
    }
}

pub fn pop_out(ui: Ui) {
    let cid = ui.card_id();
    if cid.is_empty() {
        return;
    }
    if let Some(entry) = untracked(|| select_entry_for_card(ui, &cid))
        && entry.listed()
        && entry.binding().is_some()
    {
        batch(|| {
            ui.card_terminal_open.set(false);
            ui.card_terminal_focused.set(false);
            ui.open_terminal(entry.saved.id);
        });
    } else {
        batch(|| {
            ui.card_terminal_open.set(false);
            ui.card_terminal_focused.set(false);
            crate::terminals::open_card(ui, &cid);
        });
    }
}

fn card_tab_title(e: &Entry) -> String {
    if let Some(name) = &e.saved.name {
        return name.clone();
    }
    if let Some(profile) = e.profile() {
        format!("@{profile}")
    } else {
        e.title()
    }
}

fn card_tab_item(ui: Ui, cid: String, e: Entry, selected_id: Memo<String>) -> impl IntoElement {
    let id = e.saved.id.clone();
    let id_sel = id.clone();
    let is_selected = computed(move || selected_id.get() == id_sel);
    dynamic(move || {
        let is_sel = is_selected.get();
        let id = id.clone();
        let close_id = id.clone();
        let menu_id = id.clone();
        let rename_id = id.clone();
        let tab_cid = cid.clone();
        let text = card_tab_title(&e);
        let hint = e.hint_with_policy(ui.idle_minutes.get());
        let managed = e.is_managed();
        let reclaim_badge = crate::terminals::labels::tab_reclaim_badge(&e);
        let has_reclaim_badge = reclaim_badge.is_some();
        let provider_mark = e.provider().map(|(ic, _)| ic);
        let mut tab_button = view()
            .h_full()
            .focusable(true)
            .cursor_pointer()
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .px_2p5()
            .bg(CLEAR)
            .flex_row()
            .items_center()
            .gap_1p5()
            .role(AccessibilityRole::Tab)
            .accessibility_state(move || nsh_core::AccessibilityState {
                selected: is_sel,
                ..Default::default()
            })
            .label(format!("Tab {text}"));
        if !has_reclaim_badge && let Some(mark_el) = crate::terminals::labels::status_mark(&e) {
            tab_button = tab_button.child(mark_el);
        }
        if let Some(mark) = provider_mark {
            tab_button = tab_button.child(icon(mark).w(px(13.)).h(px(13.)).flex_shrink_0());
        } else if !managed {
            tab_button =
                tab_button.child(icon(Icon::Terminal).w(px(13.)).h(px(13.)).flex_shrink_0());
        }
        tab_button = tab_button.child(
            label(text.clone())
                .font_weight(if is_sel {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .flex_shrink(1.)
                .truncate()
                .min_w(px(0.)),
        );
        if managed {
            tab_button = tab_button.child(crate::terminals::labels::managed_badge());
        }
        if let Some(badge) = reclaim_badge {
            tab_button = tab_button.child(badge);
        }
        tab_button = tab_button
            .on_press_down(move |event: &nsh_core::PressEvent| {
                if event.clicks >= 2 {
                    ui.terminal_rename.set(Some(rename_id.clone()));
                }
            })
            .on_context_menu(move |event: &nsh_core::ContextMenuEvent| {
                ui.spawn(crate::terminals::tabs::menu(
                    ui,
                    menu_id.clone(),
                    Some(event.page_position),
                ));
            })
            .on_press(move |_: &nsh_core::PressEvent| {
                select_card_tab(ui, &tab_cid, id.clone());
                ui.card_terminal_focused.set(true);
            });
        let bg = if is_sel {
            SURFACE
        } else if managed {
            HIGHLIGHT_SURFACE
        } else {
            CLEAR
        };
        let mut cell = view()
            .relative()
            .h_full()
            .max_w(px(if has_reclaim_badge { 296. } else { 230. }))
            .flex_shrink_0()
            .flex_row()
            .items_center()
            .gap_0()
            .pr_0p5()
            .bg(bg)
            .hover(move |st| st.bg(if is_sel { SURFACE } else { CARD_HOVER }))
            .border_r_1()
            .border_color(BORDER)
            .tooltip(hint_tooltip(hint).placement(Placement::Bottom))
            .child(tab_button)
            .child(chrome_button(
                Icon::Close,
                format!("Close terminal {}", e.title()),
                move |_| crate::terminals::close_entry(ui, &close_id),
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
        cell
    })
}

pub fn pane(ui: Ui) -> impl IntoElement {
    let card_id = computed(move || ui.card_id());
    let active_entry = computed(move || {
        let cid = card_id.get();
        if cid.is_empty() {
            None
        } else {
            select_entry_for_card(ui, &cid)
        }
    });
    let selected_tab_id =
        computed(move || active_entry.get().map(|e| e.saved.id).unwrap_or_default());
    let target = computed(move || {
        active_entry.get().and_then(|e| {
            let b = e.binding()?;
            Some((
                b["id"].as_str().unwrap_or("").to_owned(),
                b["core_id"].as_str().unwrap_or("").to_owned(),
                b["terminal"].as_str().unwrap_or("").to_owned(),
                b["read_only"] == true,
                b["input_status"].as_str().unwrap_or("").to_owned(),
                e.canvas_notice_status().to_owned(),
            ))
        })
    });
    let binding_key = computed(move || {
        target.with(|t| {
            t.as_ref().map(|(entry_id, core_id, terminal, _, _, _)| {
                (entry_id.clone(), core_id.clone(), terminal.clone())
            })
        })
    });
    let has_live_target = computed(move || target.with(|t| t.is_some()));
    let interruptible_id = computed(move || {
        let e = active_entry.get()?;
        let agent = e.agent.as_ref()?;
        let att = agent.agent()?;
        (!att.finished()
            && att.terminal.is_some()
            && e.card.as_ref().is_some_and(|c| c.archived_at.is_none()))
        .then_some(e.saved.id)
    });
    let tab_inventory = computed(move || {
        let cid = card_id.get();
        if cid.is_empty() {
            return Vec::new();
        }
        let idle_minutes = ui.idle_minutes.get();
        card_live_entries_for_ui(ui, &cid)
            .into_iter()
            .map(|e| {
                (
                    e.saved.id.clone(),
                    card_tab_title(&e),
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
    });

    view()
        .w_full()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .bg(crate::utilities::terminal_bg_color)
        .on_touch(move |event: &TouchEvent| {
            if event.phase == TouchPhase::Start && !ui.card_terminal_focused.get_untracked() {
                ui.card_terminal_focused.set(true);
                ui.terminal_focus.update(|v| *v += 1);
            }
        })
        .child(
            view()
                .h(px(38.))
                .flex_shrink_0()
                .flex_row()
                .items_center()
                .gap_0()
                .bg(INSET)
                .border_b_1()
                .border_color(BORDER)
                .child(when(
                    move || ui.narrow.get(),
                    move || {
                        view()
                            .px_1p5()
                            .h_full()
                            .flex_row()
                            .items_center()
                            .border_r_1()
                            .border_color(BORDER)
                            .child(compact_action("← Card", Variant::Quiet, move |_| {
                                ui.card_terminal_open.set(false);
                                ui.card_terminal_focused.set(false);
                            }))
                    },
                ))
                .child(
                    scroll()
                        .horizontal()
                        .scrollbar(false)
                        .flex_1()
                        .min_w(px(0.))
                        .h_full()
                        .child(dynamic(move || {
                            let _ = tab_inventory.get();
                            let cid = untracked(|| card_id.get());
                            if cid.is_empty() {
                                return view();
                            }
                            let live = untracked(|| card_live_entries_for_ui(ui, &cid));
                            let mut strip = view().h_full().flex_row().items_center().gap_0();
                            for e in live {
                                strip = strip.child(card_tab_item(
                                    ui,
                                    cid.clone(),
                                    e,
                                    selected_tab_id,
                                ));
                            }
                            strip
                        })),
                )
                .child(
                    view()
                        .px_1p5()
                        .flex_shrink_0()
                        .flex_row()
                        .items_center()
                        .gap_0p5()
                        .child({
                            let plus_anchor = element_ref();
                            view().element_ref(&plus_anchor).child(
                                chrome_button_disabled(
                                    Icon::Plus,
                                    format!(
                                        "New terminal or open assigned Agent · {}",
                                        crate::commands::shortcuts::NEW_TERMINAL_DISPLAY
                                    ),
                                    move || {
                                        ui.pending.get()
                                            || ui
                                                .current_card()
                                                .is_none_or(|c| c.archived_at.is_some())
                                    },
                                    move |_| {
                                        let Some(c) = ui.current_card() else {
                                            return;
                                        };
                                        let live = card_live_entries_for_ui(ui, &c.id);
                                        let missing_assignee = c.assignee.as_deref().filter(|aid| {
                                            !live.iter().any(|e| e.profile() == Some(*aid))
                                        });
                                        let Some(aid) = missing_assignee.map(str::to_owned) else {
                                            crate::terminals::create_in_card(
                                                ui,
                                                c.project_id,
                                                c.id,
                                                false,
                                            );
                                            return;
                                        };
                                        use nsh_core::element::desktop::{MenuItem, popup_menu_at};
                                        let cid_agent = c.id.clone();
                                        let cid_shell = c.id.clone();
                                        let pid_shell = c.project_id.clone();
                                        let cid_eph = c.id;
                                        let pid_eph = c.project_id;
                                        let aid_label = aid.clone();
                                        ui.spawn(async move {
                                            let Some(bounds) = plus_anchor.measure().await else {
                                                return;
                                            };
                                            let items = vec![
                                                MenuItem::new(format!("Open @{aid_label}"))
                                                    .on_select(move || {
                                                        crate::terminals::open_card_agent(
                                                            ui,
                                                            &cid_agent,
                                                            Some(&aid),
                                                        )
                                                    }),
                                                MenuItem::new("New Shell Terminal").on_select(
                                                    move || {
                                                        crate::terminals::create_in_card(
                                                            ui,
                                                            pid_shell.clone(),
                                                            cid_shell.clone(),
                                                            false,
                                                        )
                                                    },
                                                ),
                                                MenuItem::new("New Ephemeral Agent").on_select(
                                                    move || {
                                                        crate::terminals::create_in_card(
                                                            ui,
                                                            pid_eph.clone(),
                                                            cid_eph.clone(),
                                                            true,
                                                        )
                                                    },
                                                ),
                                            ];
                                            popup_menu_at(
                                                items,
                                                point(
                                                    bounds.page.x,
                                                    bounds.page.y + bounds.frame.size.height,
                                                ),
                                            )
                                            .await;
                                        });
                                    },
                                ),
                            )
                        })
                        .child(dynamic(move || {
                            let Some(terminal_id) = interruptible_id.get() else {
                                return view().into_element();
                            };
                            compact_action("Interrupt", Variant::Quiet, move |_| {
                                dispatch(
                                    ui,
                                    "agent.interrupt",
                                    json!({"terminal_id": terminal_id}),
                                )
                            })
                            .into_element()
                        }))
                        .child(
                            view()
                                .tooltip(hint_tooltip("Open in full-screen Terminals workspace"))
                                .child(compact_action("↗", Variant::Quiet, move |_| pop_out(ui))),
                        )
                        .child(chrome_button(
                            Icon::Close,
                            format!(
                                "Hide Terminal pane · {} (keeps Agent & Shell running)",
                                crate::commands::shortcuts::CARD_TERMINAL_DISPLAY
                            ),
                            move |_| {
                                ui.card_terminal_open.set(false);
                                ui.card_terminal_focused.set(false);
                            },
                        )),
                ),
        )
        .child(dynamic(move || {
            if has_live_target.get() {
                let focus_req = move || canban_terminal::FocusRequest {
                    ticket: ui.terminal_focus.get(),
                    enabled: ui.card_terminal_focused.get()
                        && !ui.quick_open.get()
                        && ui.utility_popup.get().is_empty()
                        && !ui.native_modal.get()
                        && ui.terminal_rename.get().is_none()
                        && ui.terminal_close_confirm.get().is_none()
                        && ui.worktree_modal.get().is_none()
                        && ui.worktree_promote.get().is_none()
                        && ui.worktree_remove_confirm.get().is_none()
                        && ui.project_archive_confirm.get().is_none()
                        && ui.project_restore_modal.get().is_none(),
                };
                let status_fn = move || {
                    target.with(|t| {
                        t.as_ref()
                            .map(|(_, _, _, _, status, _)| status.clone())
                            .unwrap_or_default()
                    })
                };
                let notice_fn = move || {
                    target.with(|t| {
                        t.as_ref()
                            .map(|(_, _, _, _, _, notice)| notice.clone())
                            .unwrap_or_default()
                    })
                };
                if !crate::backend::is_connected() {
                    return canban_terminal::TerminalView::preview(
                        concat!(
                            "\x1b[1;36mCard Live Terminal\x1b[0m · Side-by-side discussion\r\n",
                            "\x1b[2mDiscuss requirements or answer Agent prompts directly beside the Card.\x1b[0m\r\n\r\n",
                            "\x1b[1;32m❯\x1b[0m Ready for input…\r\n",
                        ),
                        crate::utilities::terminal_theme,
                        move || crate::utilities::terminal_font(ui),
                        focus_req,
                    )
                    .ui_command_keys(["o", "n", "/", "i", "j", "t", "w", "[", "]", "{", "}"])
                    .read_only(|| true)
                    .input_status(status_fn)
                    .notice_status(notice_fn)
                    .into_element();
                }
                return canban_terminal::TerminalView::live(
                    move || {
                        binding_key.with(|t| {
                            t.as_ref().map(|(entry_id, core_id, terminal)| {
                                let host_id = ui
                                    .snapshot
                                    .with_untracked(|s| s.terminal_host(entry_id).to_owned());
                                canban_terminal::LiveBinding {
                                    session_epoch: crate::terminals::host_connect_epoch(
                                        ui, &host_id,
                                    ),
                                    host_id: host_id.clone(),
                                    config: crate::config_for_host(&host_id),
                                    core_id: core_id.clone(),
                                    terminal: terminal.clone(),
                                    transport: crate::backend::terminal_transport(&host_id),
                                }
                            })
                        })
                    },
                    crate::utilities::terminal_theme,
                    move || crate::utilities::terminal_font(ui),
                    focus_req,
                )
                .ui_command_keys(["o", "n", "/", "i", "j", "t", "w", "[", "]", "{", "}"])
                .read_only(move || target.with(|t| t.as_ref().is_none_or(|(_, _, _, ro, _, _)| *ro)))
                .input_status(status_fn)
                .notice_status(notice_fn)
                .into_element();
            }

            let status_label = active_entry
                .get()
                .map(|e| format!("{}…", e.display_status()))
                .unwrap_or_else(|| "Starting…".into());
            view()
                .flex_1()
                .min_h(px(0.))
                .bg(crate::utilities::terminal_bg_color)
                .p_5()
                .gap_2()
                .items_start()
                .child(live_status(status_label, Tone::Info))
                .into_element()
        }))
}
