//! Embedded Live Terminal & Agent pane inside the side-by-side Card workspace (`⌘J`).
//!
//! Shares the same native `canban/terminal` viewer and Herdr PTY attachment as
//! `Terminals`, while guaranteeing at most one mounted viewer at a time (`page == "card"`).
//! Hiding this pane detaches the viewer without terminating any PTY.
use super::*;
use crate::terminals::{Entry, card_live_entries, default_card_terminal, select_card_tab};
use serde_json::json;

pub fn select_entry_for_card(ui: Ui, card_id: &str) -> Option<Entry> {
    let s = ui.snapshot.get();
    let live = card_live_entries(&s, card_id);
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
    let fallback = default_card_terminal(&s, card_id, recent.as_deref())?;
    live.into_iter().find(|e| e.saved.id == fallback)
}

pub fn toggle(ui: Ui) {
    if ui.page.get_untracked() != "card" || ui.card.get_untracked().is_empty() {
        return;
    }
    if ui.card_terminal_open.get_untracked() {
        ui.card_terminal_open.set(false);
        ui.card_terminal_focused.set(false);
        return;
    }
    let Some(card) = ui.current_card() else {
        return;
    };
    let s = ui.snapshot.get_untracked();
    let recent = ui.terminal_recent.with(|m| m.get(&card.id).cloned());
    if let Some(id) = default_card_terminal(&s, &card.id, recent.as_deref()) {
        select_card_tab(ui, &card.id, id);
        ui.card_terminal_open.set(true);
        ui.card_terminal_focused.set(true);
        return;
    }
    if card.archived_at.is_some() {
        ui.operation_error
            .set("Restore this Card before opening a new terminal.".into());
        return;
    }
    crate::terminals::create_in_card(ui, card.project_id, card.id, false);
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
        ui.card_terminal_open.set(false);
        ui.card_terminal_focused.set(false);
        ui.open_terminal(entry.saved.id);
    } else {
        ui.card_terminal_open.set(false);
        ui.card_terminal_focused.set(false);
        crate::terminals::open_card(ui, &cid);
    }
}

fn card_tab_title(e: &Entry) -> String {
    if let Some(name) = &e.saved.name {
        return name.clone();
    }
    if let Some(profile) = e.profile() {
        format!("@{profile} · {}", e.display_status())
    } else {
        e.title()
    }
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
    let target = computed(move || {
        active_entry.get().and_then(|e| {
            let b = e.binding()?;
            Some((
                b["id"].as_str().unwrap_or("").to_owned(),
                b["core_id"].as_str().unwrap_or("").to_owned(),
                b["terminal"].as_str().unwrap_or("").to_owned(),
                b["read_only"] == true,
                b["input_status"].as_str().unwrap_or("").to_owned(),
            ))
        })
    });

    view()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .bg(BACKGROUND)
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
                            let cid = card_id.get();
                            if cid.is_empty() {
                                return view();
                            }
                            let s = ui.snapshot.get();
                            let live = card_live_entries(&s, &cid);
                            let selected_id = active_entry
                                .get()
                                .map(|e| e.saved.id)
                                .unwrap_or_default();
                            let mut strip = view().h_full().flex_row().items_center().gap_0();
                            for e in live {
                                let id = e.saved.id.clone();
                                let close_id = id.clone();
                                let menu_id = id.clone();
                                let rename_id = id.clone();
                                let tab_cid = cid.clone();
                                let is_sel = selected_id == id;
                                let text = card_tab_title(&e);
                                let hint = e.hint();
                                let mark =
                                    e.provider().map(|(ic, _)| ic).unwrap_or(Icon::Terminal);
                                let dot_color = if e.needs_attention() {
                                    ATTENTION
                                } else if e.ended() || e.inactive() {
                                    MUTED
                                } else {
                                    SUCCESS
                                };
                                strip = strip.child(
                                    view()
                                        .h_full()
                                        .max_w(px(230.))
                                        .flex_shrink_0()
                                        .flex_row()
                                        .items_center()
                                        .gap_0()
                                        .pr_0p5()
                                        .bg(if is_sel { SURFACE } else { CLEAR })
                                        .hover(move |st| {
                                            st.bg(if is_sel { SURFACE } else { CARD_HOVER })
                                        })
                                        .border_r_1()
                                        .border_color(BORDER)
                                        .tooltip(
                                            hint_tooltip(hint.clone())
                                                .placement(Placement::Bottom),
                                        )
                                        .child(
                                            view()
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
                                                .accessibility_state(move || {
                                                    nsh_core::AccessibilityState {
                                                        selected: is_sel,
                                                        ..Default::default()
                                                    }
                                                })
                                                .label(format!("Tab {text}"))
                                                .child(
                                                    view()
                                                        .w(px(5.))
                                                        .h(px(5.))
                                                        .rounded(px(3.))
                                                        .bg(dot_color)
                                                        .flex_shrink_0()
                                                        .pointer_events(PointerEvents::None),
                                                )
                                                .child(
                                                    icon(mark)
                                                        .w(px(13.))
                                                        .h(px(13.))
                                                        .flex_shrink_0(),
                                                )
                                                .child(
                                                    label(text.clone())
                                                        .font_weight(if is_sel {
                                                            FontWeight::SEMIBOLD
                                                        } else {
                                                            FontWeight::NORMAL
                                                        })
                                                        .truncate()
                                                        .min_w(px(0.)),
                                                )
                                                .on_press_down(
                                                    move |event: &nsh_core::PressEvent| {
                                                        if event.clicks >= 2 {
                                                            ui.terminal_rename
                                                                .set(Some(rename_id.clone()));
                                                        }
                                                    },
                                                )
                                                .on_context_menu(
                                                    move |event: &nsh_core::ContextMenuEvent| {
                                                        ui.spawn(crate::terminals::tabs::menu(
                                                            ui,
                                                            menu_id.clone(),
                                                            Some(event.page_position),
                                                        ));
                                                    },
                                                )
                                                .on_press(move |_: &nsh_core::PressEvent| {
                                                    select_card_tab(ui, &tab_cid, id.clone());
                                                    ui.card_terminal_focused.set(true);
                                                }),
                                        )
                                        .child(chrome_button(
                                            Icon::Close,
                                            format!("Close terminal {}", e.title()),
                                            move |_| crate::terminals::close_entry(ui, &close_id),
                                        )),
                                );
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
                        .child(chrome_button_disabled(
                            Icon::Plus,
                            "New terminal · ⌘T",
                            move || {
                                ui.pending.get()
                                    || ui.current_card().is_none_or(|c| c.archived_at.is_some())
                            },
                            move |_| {
                                if let Some(c) = ui.current_card() {
                                    crate::terminals::create_in_card(
                                        ui,
                                        c.project_id,
                                        c.id,
                                        false,
                                    );
                                }
                            },
                        ))
                        .child(chrome_button_disabled(
                            Icon::Agent,
                            "New Agent",
                            move || {
                                ui.pending.get()
                                    || ui.current_card().is_none_or(|c| c.archived_at.is_some())
                            },
                            move |_| {
                                if let Some(c) = ui.current_card() {
                                    crate::terminals::create_in_card(ui, c.project_id, c.id, true);
                                }
                            },
                        ))
                        .child(dynamic(move || {
                            let Some(e) = active_entry.get() else {
                                return view().into_element();
                            };
                            if let Some(agent) = &e.agent
                                && let Some(att) = &agent.attempt
                                && !att.finished()
                                && att.terminal.is_some()
                                && e.card.as_ref().is_some_and(|c| c.archived_at.is_none())
                            {
                                let attempt_id = att.id.clone();
                                return compact_action("Interrupt", Variant::Quiet, move |_| {
                                    dispatch(
                                        ui,
                                        "agent.interrupt",
                                        json!({"attempt_id": attempt_id}),
                                    )
                                })
                                .into_element();
                            }
                            view().into_element()
                        }))
                        .child(
                            view()
                                .tooltip(hint_tooltip("Open in full-screen Terminals workspace"))
                                .child(compact_action("↗", Variant::Quiet, move |_| pop_out(ui))),
                        )
                        .child(chrome_button(
                            Icon::Close,
                            "Hide Terminal pane · ⌘J (keeps Agent & Shell running)",
                            move |_| {
                                ui.card_terminal_open.set(false);
                                ui.card_terminal_focused.set(false);
                            },
                        )),
                ),
        )
        .child(dynamic(move || {
            if let Some((id, core_id, terminal, read_only_val, input_status_val)) = target.get() {
                let focus_prop = move || {
                    json!({
                        "ticket": ui.terminal_focus.get(),
                        "enabled": ui.card_terminal_focused.get()
                            && !ui.quick_open.get()
                            && ui.utility_popup.get().is_empty()
                            && !ui.native_modal.get()
                            && ui.terminal_rename.get().is_none()
                            && ui.worktree_modal.get().is_none()
                            && ui.worktree_promote.get().is_none()
                            && ui.worktree_remove_confirm.get().is_none()
                    })
                };
                let Some(app) = crate::APP.get() else {
                    return native("canban/terminal")
                        .prop("theme", crate::utilities::terminal_theme)
                        .prop("font", move || crate::utilities::terminal_font(ui))
                        .prop(
                            "ui_command_keys",
                            json!(["o", "n", "/", "i", "j", "t", "w", "[", "]", "{", "}"]),
                        )
                        .prop("focus", focus_prop)
                        .key(format!("card-term:{id}"))
                        .prop(
                            "content",
                            json!(concat!(
                                "\x1b[1;36mCard Live Terminal\x1b[0m · Side-by-side discussion\r\n",
                                "\x1b[2mDiscuss requirements or answer Agent prompts directly beside the Card.\x1b[0m\r\n\r\n",
                                "\x1b[1;32m❯\x1b[0m Ready for input…\r\n",
                            )),
                        )
                        .prop("read_only", json!(true))
                        .flex_1()
                        .min_h(px(0.))
                        .into_element();
                };
                return native("canban/terminal")
                    .prop("theme", crate::utilities::terminal_theme)
                    .prop("font", move || crate::utilities::terminal_font(ui))
                    .prop(
                        "ui_command_keys",
                        json!(["o", "n", "/", "i", "j", "t", "w", "[", "]", "{", "}"]),
                    )
                    .prop("focus", focus_prop)
                    .key(format!("card-term:{id}"))
                    .prop(
                        "binding",
                        json!({"config": app.config(), "terminal": terminal, "core_id": core_id}),
                    )
                    .prop("read_only", json!(read_only_val))
                    .prop("input_status", json!(input_status_val))
                    .flex_1()
                    .min_h(px(0.))
                    .into_element();
            }

            let status_label = active_entry
                .get()
                .map(|e| format!("{}…", e.display_status()))
                .unwrap_or_else(|| "Starting…".into());
            view()
                .flex_1()
                .min_h(px(0.))
                .p_5()
                .gap_2()
                .items_start()
                .child(live_status(status_label, Tone::Info))
                .into_element()
        }))
}
