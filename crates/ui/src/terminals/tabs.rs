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
        items.push(MenuItem::new(format!("Branch: {branch}")).disabled(true));
    }
    if let Some(agent) = &e.agent {
        if agent
            .attempt
            .as_ref()
            .is_some_and(|a| !a.finished() && a.terminal.is_some())
            && e.card.as_ref().is_some_and(|c| c.archived_at.is_none())
        {
            let attempt = agent.attempt.as_ref().unwrap().id.clone();
            items.push(
                MenuItem::new("Interrupt").on_select(move || {
                    dispatch(ui, "agent.interrupt", json!({"attempt_id":attempt}))
                }),
            );
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
            .child(super::labels::tab(&e))
            .on_press_down(move |event: &nsh_core::PressEvent| {
                if event.clicks >= 2 {
                    ui.terminal_rename.set(Some(rename.clone()));
                }
            })
            .on_context_menu(move |event: &nsh_core::ContextMenuEvent| {
                ui.spawn(menu(ui, context.clone(), Some(event.page_position)));
            })
            .on_press(move |_: &nsh_core::PressEvent| ui.open_terminal(id.clone()));
        view()
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
            ))
    })
}
pub fn header(ui: Ui) -> impl IntoElement {
    let inventory = computed(move || {
        ui.closing_terminals.with(|closing| {
            ui.terminal_entries.with(|rows| {
                rows.iter()
                    .filter(|e| !closing.contains(&e.saved.id))
                    .map(|e| (e.saved.id.clone(), e.title(), e.hint()))
                    .collect::<Vec<_>>()
            })
        })
    });
    let context = computed(move || {
        current(ui).map(|e| {
            (
                e.saved.id.clone(),
                e.card.clone(),
                e.project_id.clone(),
                e.location.clone(),
            )
        })
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
        let tabs = ui.terminal_tabs.get();
        let cap = capacity.get();
        let _ = inventory.get();
        let offset = ui.terminal_entries.with_untracked(|entries| {
            let mut offset = 0.;
            for id in tabs {
                if id == selected {
                    break;
                }
                if let Some(e) = entries.iter().find(|e| e.saved.id == id) {
                    offset += super::labels::tab_width(e).min(cap);
                }
            }
            offset
        });
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
                    px(ui.terminal_entries.with(|rows| {
                        let cap = capacity.get();
                        let total = ui
                            .terminal_tabs
                            .get()
                            .iter()
                            .filter_map(|id| rows.iter().find(|e| &e.saved.id == id))
                            .map(|e| super::labels::tab_width(e).min(cap))
                            .sum::<f32>();
                        total.min(cap).max(0.)
                    }))
                })
                .min_w(px(0.))
                .h_full()
                .child(dynamic(move || {
                    let _ = inventory.get();
                    let tabs = ui.terminal_tabs.get();
                    ui.terminal_entries.with_untracked(|rows| {
                        let mut strip = view().h_full().flex_row().items_center().window_drag(true);
                        for id in tabs {
                            if let Some(e) = rows.iter().find(|e| e.saved.id == id) {
                                strip = strip.child(tab(ui, e.clone(), capacity));
                            }
                        }
                        strip
                    })
                })),
        )
        .child(chrome_button_disabled(
            Icon::Plus,
            "New terminal · ⌘T",
            move || ui.pending.get(),
            move |_| new_tab(ui),
        ))
        .child(
            action("▾", Variant::Quiet, move |_| machine_menu(ui))
                .accessibility_label(|| Some("Choose terminal machine".into())),
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
            let Some(e) = untracked(|| current(ui)) else {
                return view();
            };
            let id = e.saved.id.clone();
            let anchor = element_ref();
            let mut context = view()
                .window_drag(false)
                .flex_row()
                .items_center()
                .justify_end()
                .gap_2()
                .min_w(px(0.))
                .w(move || px((width.get() * 0.35).min(340.)))
                .flex_shrink_0();
            if let Some(card) = e.card.clone() {
                context = context.child(
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
                                .hover(|s| s.bg(INSET))
                                .focus(|s| s.border_1().border_color(ACCENT))
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
                context = context.child(
                    canban_ui_kit::press_target()
                        .flex_1()
                        .min_w(px(0.))
                        .h(px(30.))
                        .px_2()
                        .rounded(px(5.))
                        .bg(CLEAR)
                        .hover(|s| s.bg(INSET))
                        .focus(|s| s.border_1().border_color(ACCENT))
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
                context = context.child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .tooltip(
                            hint_tooltip(e.cwd().unwrap_or_default()).placement(Placement::Bottom),
                        )
                        .child(
                            muted(compact(&title, if ui.narrow.get() { 14 } else { 32 }))
                                .w_full()
                                .truncate(),
                        ),
                );
            }
            let branch = e.location.as_ref().and_then(|c| c.branch.clone());
            if let Some(branch) = branch.filter(|s| !s.is_empty()) {
                let detail = id.clone();
                let full = format!(
                    "Branch: {branch}\nDirectory: {}",
                    e.cwd().unwrap_or_else(|| "Unavailable".into())
                );
                let limit = if ui.narrow.get() { 8 } else { 16 };
                let short = compact(&branch, if branch.is_ascii() { limit } else { limit / 2 });
                context = context.child(
                    view()
                        .w(move || {
                            px((if ui.narrow.get() { 84f32 } else { 140f32 })
                                .min((width.get() * 0.4).min(420.) - 44.)
                                .max(36.))
                        })
                        .flex_shrink_0()
                        .tooltip(hint_tooltip(full).placement(Placement::Bottom))
                        .child(
                            canban_ui_kit::press_target()
                                .h(px(30.))
                                .w_full()
                                .min_w(px(0.))
                                .px_2()
                                .rounded(px(5.))
                                .flex_row()
                                .gap_1()
                                .items_center()
                                .bg(CLEAR)
                                .hover(|s| s.bg(INSET))
                                .focus(|s| s.border_1().border_color(ACCENT))
                                .accessibility_label({
                                    let b = branch.clone();
                                    move || Some(format!("Branch {b}").into())
                                })
                                .child(icon(Icon::Branch).w(px(14.)).h(px(14.)).flex_shrink_0())
                                .child(muted(short).flex_1().min_w(px(0.)).truncate())
                                .on_press(move |_: &()| anchored(ui, detail.clone(), anchor)),
                        ),
                );
            }
            context.child(
                chrome_button(Icon::More, "Terminal actions", move |_| {
                    anchored(ui, id.clone(), anchor)
                })
                .element_ref(&anchor),
            )
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
