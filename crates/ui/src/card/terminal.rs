//! Embedded Live Terminal pane inside the side-by-side Card workspace (`⌘J`).
//!
//! Shares the same native `canban/terminal` viewer and Herdr PTY attachment as
//! `Terminals`, while guaranteeing at most one mounted viewer at a time (`page == "card"`).
//! Hiding or closing this pane detaches the viewer without terminating the PTY.
use super::*;
use crate::terminals::{Entry, entries, model};
use nsh_core::element::desktop::{MenuItem, popup_menu, popup_menu_at};
use serde_json::json;

pub fn select_entry_for_card(ui: Ui, card_id: &str) -> Option<Entry> {
    let s = ui.snapshot.get();
    let all: Vec<_> = entries(&s)
        .into_iter()
        .filter(|e| e.card.as_ref().is_some_and(|c| c.id == card_id))
        .collect();
    if all.is_empty() {
        return None;
    }
    let recent = ui.terminal_recent.with(|m| m.get(card_id).cloned());
    if let Some(rec) = &recent
        && let Some(e) = all.iter().find(|e| &e.saved.id == rec && !e.ended())
    {
        return Some(e.clone());
    }
    if let Some(e) = all.iter().find(|e| !e.ended() && e.binding().is_some()) {
        return Some(e.clone());
    }
    if let Some(rec) = &recent
        && let Some(e) = all.iter().find(|e| &e.saved.id == rec)
    {
        return Some(e.clone());
    }
    all.into_iter().next()
}

pub fn toggle(ui: Ui) {
    if ui.page.get_untracked() != "card" || ui.card.get_untracked().is_empty() {
        return;
    }
    let next = !ui.card_terminal_open.get_untracked();
    ui.card_terminal_open.set(next);
    ui.card_terminal_focused.set(next);
    if next {
        ui.terminal_focus.update(|v| *v += 1);
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
        ui.card_terminal_open.set(false);
        ui.open_terminal(entry.saved.id);
    } else {
        ui.card_terminal_open.set(false);
        crate::terminals::open_card(ui, &cid);
    }
}

fn selector_menu(ui: Ui, card_id: String, anchor: ElementRef) {
    ui.spawn(async move {
        let s = ui.snapshot.get_untracked();
        let card = s.cards.iter().find(|c| c.id == card_id).cloned();
        let card_entries: Vec<_> = entries(&s)
            .into_iter()
            .filter(|e| e.card.as_ref().is_some_and(|c| c.id == card_id))
            .collect();
        let mut items = Vec::new();
        for e in &card_entries {
            let eid = e.saved.id.clone();
            let cid = card_id.clone();
            let label_str = if let Some(prof) = e.profile() {
                format!("@{} · {}", prof, e.display_status())
            } else {
                format!("{} · {}", e.title(), e.display_status())
            };
            if !e.ended() {
                items.push(MenuItem::new(label_str).on_select(move || {
                    ui.open_card_with_terminal(cid.clone(), Some(eid.clone()));
                }));
            } else if let Some(agent) = &e.agent
                && model::can_resume(agent, &ui.profiles.get_untracked())
            {
                let resume_id = agent.id.clone();
                items.push(
                    MenuItem::new(format!("Resume @{}", e.profile().unwrap_or("agent")))
                        .on_select(move || crate::terminals::navigation::resume(ui, &resume_id)),
                );
            }
        }
        if let Some(c) = &card
            && c.active()
        {
            if !items.is_empty() {
                items.push(MenuItem::separator());
            }
            for p in ui.profiles.get_untracked() {
                let exists = card_entries
                    .iter()
                    .any(|e| e.profile() == Some(p.id.as_str()) && !e.ended());
                if !exists {
                    let cid = c.id.clone();
                    let pid = p.id.clone();
                    let has_closed = card_entries
                        .iter()
                        .any(|e| e.profile() == Some(p.id.as_str()));
                    let verb = if has_closed {
                        "Resume / Start"
                    } else {
                        "Start"
                    };
                    items.push(
                        MenuItem::new(format!("{verb} @{}", p.id)).on_select(move || {
                            crate::sessions::open(ui, cid.clone(), pid.clone());
                        }),
                    );
                }
            }
            items.push(MenuItem::separator());
            let pid = c.project_id.clone();
            let cid = c.id.clone();
            items.push(
                MenuItem::new("Open Shell in Card Worktree").on_select(move || {
                    crate::terminals::launch_custom(ui, json!({"project_id": pid, "card_id": cid}));
                }),
            );
        }
        if items.is_empty() {
            return;
        }
        if let Some(bounds) = anchor.measure().await {
            popup_menu_at(
                items,
                point(bounds.page.x, bounds.page.y + bounds.frame.size.height),
            )
            .await;
        } else {
            popup_menu(items).await;
        }
    });
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
    let anchor = element_ref();

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
                .px_2p5()
                .flex_shrink_0()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .bg(SURFACE)
                .border_b_1()
                .border_color(BORDER)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .min_w(px(0.))
                        .flex_1()
                        .child(when(
                            move || ui.narrow.get(),
                            move || {
                                compact_action("← Card", Variant::Quiet, move |_| {
                                    ui.card_terminal_open.set(false);
                                    ui.card_terminal_focused.set(false);
                                })
                            },
                        ))
                        .child(
                            nsh_base::button()
                                .element_ref(&anchor)
                                .h(px(26.))
                                .px_2()
                                .rounded(px(RADIUS))
                                .border_1()
                                .border_color(BORDER)
                                .bg(INSET)
                                .hover(|s| s.bg(CARD_HOVER))
                                .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                                .flex_row()
                                .items_center()
                                .gap_1p5()
                                .min_w(px(0.))
                                .accessibility_label("Select Card Agent or Shell")
                                .on_press(move |_: &()| {
                                    let cid = card_id.get_untracked();
                                    if !cid.is_empty() {
                                        selector_menu(ui, cid, anchor);
                                    }
                                })
                                .child(icon(Icon::Terminal).w(px(12.5)).h(px(12.5)))
                                .child(
                                    label(move || {
                                        if let Some(e) = active_entry.get() {
                                            if let Some(prof) = e.profile() {
                                                format!("@{prof} ▾")
                                            } else {
                                                format!("{} ▾", e.title())
                                            }
                                        } else {
                                            "Start Agent ▾".into()
                                        }
                                    })
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate(),
                                ),
                        )
                        .child(dynamic(move || {
                            if let Some(e) = active_entry.get() {
                                let tone = if e.needs_attention() {
                                    Tone::Attention
                                } else if e.ended() {
                                    Tone::Neutral
                                } else {
                                    Tone::Success
                                };
                                status(e.display_status(), tone).into_element()
                            } else {
                                view().into_element()
                            }
                        })),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .flex_shrink_0()
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
                                .tooltip(
                                    nsh_base::tooltip()
                                        .child(label("Open in full-screen Terminals workspace")),
                                )
                                .child(compact_action("Pop out ↗", Variant::Quiet, move |_| {
                                    pop_out(ui)
                                })),
                        )
                        .child(chrome_button(
                            Icon::Close,
                            "Hide Card Terminal · ⌘J (keeps Agent running)",
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

            // Empty / queued / ended state
            let cid = card_id.get();
            let card = ui.current_card();
            let entry = active_entry.get();
            let mut body = view().flex_1().min_h(px(0.)).p_5().gap_3().items_start();
            if let Some(e) = entry {
                if matches!(e.status().as_str(), "queued" | "starting") {
                    body = body
                        .child(status(format!("{}…", e.display_status()), Tone::Info))
                        .child(muted(
                            "Waiting for scheduler admission and PTY attachment. Terminal will appear automatically.",
                        ));
                } else if let Some(agent) = &e.agent
                    && model::can_resume(agent, &ui.profiles.get())
                {
                    let resume_id = agent.id.clone();
                    let prof = e.profile().unwrap_or("agent").to_owned();
                    body = body
                        .child(label(format!("@{prof} conversation ended")).font_weight(FontWeight::SEMIBOLD))
                        .child(muted("Resume the saved session right beside this Card to continue discussing."))
                        .child(
                            action(format!("Resume @{prof}"), Variant::Primary, move |_| {
                                crate::terminals::navigation::resume(ui, &resume_id)
                            })
                            .disabled(move || ui.pending.get()),
                        );
                } else {
                    body = body.child(muted("Previous terminal session has ended."));
                }
            } else {
                body = body
                    .child(label("Discuss with an Agent live").font_weight(FontWeight::SEMIBOLD))
                    .child(muted(
                        "Launch an interactive Agent CLI in this Card's workspace without leaving the Card context.",
                    ));
            }
            if let Some(c) = card
                && c.active()
            {
                let def = crate::sessions::default_profile(ui);
                let cid_start = cid.clone();
                let cid_shell = cid;
                let pid_shell = c.project_id;
                body = body.child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            action(
                                format!("Start @{def}"),
                                Variant::Primary,
                                move |_| crate::sessions::open(ui, cid_start.clone(), def.clone()),
                            )
                            .disabled(move || ui.pending.get()),
                        )
                        .child(
                            action("Open Shell", Variant::Secondary, move |_| {
                                crate::terminals::launch_custom(
                                    ui,
                                    json!({"project_id": pid_shell, "card_id": cid_shell}),
                                );
                            })
                            .disabled(move || ui.pending.get()),
                        ),
                );
            }
            body.into_element()
        }))
}
