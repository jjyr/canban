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
    let Some(e) = entries(&ui.snapshot.get_untracked())
        .into_iter()
        .find(|e| e.saved.id == id)
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
        MenuItem::new("Close terminal").on_select(move || close_tab(ui, &close)),
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
fn tab(ui: Ui, e: Entry, capacity: f32) -> impl IntoElement {
    let id = e.saved.id.clone();
    let close = id.clone();
    let context = id.clone();
    let rename = id.clone();
    let selected = ui.terminal_selection.get() == id;
    let hint = e.hint();
    view()
        .w(px(super::labels::tab_width(&e).min(capacity)))
        .h_full()
        .flex_shrink_0()
        .window_drag(false)
        .flex_row()
        .items_center()
        .gap_0()
        .px_1()
        .bg(if selected { SELECTED } else { CLEAR })
        .border_r_1()
        .border_color(BORDER)
        .tooltip(
            nsh_base::tooltip()
                .placement(Placement::Bottom)
                .px_3()
                .py_2()
                .bg(SURFACE)
                .border_1()
                .border_color(BORDER)
                .child(label(hint.clone())),
        )
        .child(
            view()
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
                .accessibility_state(move || nsh_core::AccessibilityState {
                    selected,
                    ..Default::default()
                })
                .label(format!("Tab {hint}"))
                .flex_row()
                .gap_2()
                .child(
                    view()
                        .w(px(5.))
                        .h(px(5.))
                        .rounded(px(3.))
                        .bg(if e.needs_attention() {
                            ATTENTION
                        } else if e.ended() || e.inactive() {
                            MUTED
                        } else {
                            SUCCESS
                        })
                        .pointer_events(PointerEvents::None),
                )
                .child(super::labels::tab(&e))
                .on_press_down(move |event: &nsh_core::PressEvent| {
                    if event.clicks >= 2 {
                        ui.terminal_rename.set(Some(rename.clone()));
                    }
                })
                .on_context_menu(move |event: &nsh_core::ContextMenuEvent| {
                    ui.spawn(menu(ui, context.clone(), Some(event.page_position)));
                })
                .on_press(move |_: &nsh_core::PressEvent| ui.open_terminal(id.clone())),
        )
        .child(chrome_button(
            Icon::Close,
            format!("Close terminal {}", e.title()),
            move |_| close_tab(ui, &close),
        ))
}
pub fn header(ui: Ui) -> impl IntoElement {
    let inventory = computed(move || {
        entries(&ui.snapshot.get())
            .into_iter()
            .map(|e| (e.saved.id.clone(), e.title(), e.hint()))
            .collect::<Vec<_>>()
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
            64. + (nsh_core::element::desktop::title_bar().left.0.max(78.) - 56.).max(0.)
        };
        (w - (w * 0.4).min(420.) - 88. - controls).max(40.)
    });
    let strip = element_ref();
    let active = computed(move || ui.terminal_selection.get());
    effect(move || {
        let selected = active.get();
        let snapshot = ui.snapshot.get_untracked();
        let entries = entries(&snapshot);
        let tabs = ui.terminal_tabs.get();
        let cap = capacity.get();
        let _ = inventory.get();
        let mut offset = 0.;
        for id in tabs {
            if id == selected {
                break;
            }
            if let Some(e) = entries.iter().find(|e| e.saved.id == id) {
                offset += super::labels::tab_width(e).min(cap);
            }
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
                (nsh_core::element::desktop::title_bar().left.0.max(78.) - 56.).max(0.)
            })
        })
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
                    px({
                        let rows = entries(&ui.snapshot.get());
                        let total = ui
                            .terminal_tabs
                            .get()
                            .iter()
                            .filter_map(|id| rows.iter().find(|e| &e.saved.id == id))
                            .map(|e| super::labels::tab_width(e).min(capacity.get()))
                            .sum::<f32>();
                        total.min(capacity.get()).max(0.)
                    })
                })
                .min_w(px(0.))
                .h_full()
                .child(dynamic(move || {
                    let _ = inventory.get();
                    let rows = entries(&ui.snapshot.get_untracked());
                    let mut strip = view().h_full().flex_row().items_center().window_drag(true);
                    for id in ui.terminal_tabs.get() {
                        if let Some(e) = rows.iter().find(|e| e.saved.id == id) {
                            strip = strip.child(tab(ui, e.clone(), capacity.get()));
                        }
                    }
                    strip
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
        .child(when(
            move || card_context(ui).is_some_and(|c| c.archived_at.is_none()),
            move || {
                chrome_button_disabled(
                    Icon::Agent,
                    "Choose Card Agent",
                    move || ui.pending.get(),
                    move |_| choose_card_agent(ui),
                )
            },
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
                .w(move || px((width.get() * 0.4).min(420.)))
                .flex_shrink_0();
            if let Some(card) = e.card.clone() {
                context = context.child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .tooltip(nsh_base::tooltip().child(label(format!(
                            "#{} {}\n{}",
                            card.number,
                            card.title,
                            if e.inferred_card {
                                "Card inferred from directory or branch"
                            } else {
                                "Open task Card"
                            }
                        ))))
                        .child(
                            nsh_base::button()
                                .w_full()
                                .min_w(px(0.))
                                .h(px(30.))
                                .justify_center()
                                .px_2()
                                .bg(CLEAR)
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
                    nsh_base::button()
                        .flex_1()
                        .min_w(px(0.))
                        .h(px(30.))
                        .px_2()
                        .bg(CLEAR)
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
                            nsh_base::tooltip()
                                .placement(Placement::Bottom)
                                .px_3()
                                .py_2()
                                .bg(SURFACE)
                                .border_1()
                                .border_color(BORDER)
                                .child(label(e.cwd().unwrap_or_default())),
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
                        .tooltip(
                            nsh_base::tooltip()
                                .placement(Placement::Bottom)
                                .px_3()
                                .py_2()
                                .bg(SURFACE)
                                .border_1()
                                .border_color(BORDER)
                                .child(label(full)),
                        )
                        .child(
                            nsh_base::button()
                                .h(px(30.))
                                .w_full()
                                .min_w(px(0.))
                                .px_2()
                                .flex_row()
                                .gap_1()
                                .items_center()
                                .bg(CLEAR)
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
        .child(view().w(px(10.)).h_full().window_drag(true))
}
pub fn rename_dialog(ui: Ui) -> impl IntoElement {
    when(
        move || ui.terminal_rename.get().is_some(),
        move || {
            let id = ui.terminal_rename.get_untracked().unwrap();
            let initial = entries(&ui.snapshot.get_untracked())
                .into_iter()
                .find(|e| e.saved.id == id)
                .and_then(|e| e.saved.name)
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
