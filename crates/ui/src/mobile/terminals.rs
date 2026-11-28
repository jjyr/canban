//! Mobile `Terminals` tab (`Card Agents` / `Worktrees` / `Shells` hairline list with Agent brand icons,
//! plain-shell command status, Interrupt/Close actions, and lightweight Shell creation Sheet)
//! and pushed full-screen `Terminal` route.
use super::{
    CardSegment, MobileSheet, MobileState,
    chrome::{
        confirm_destructive, disclosure_chevron, empty_state, hairline_list_inset, is_android,
        is_ios, is_mobile_os, mobile_button, mobile_footnote, mobile_inline_button, mobile_title,
        offline_sync_banner, section_header, tab_scroll_bottom_pad, trigger_haptic,
    },
    mobile_hosts,
};
use crate::{
    presenter::*,
    terminals::{
        Entry,
        labels::{StatusMarkKind, render_status_mark, sidebar_status_spec, status_mark_kind},
    },
};
use canban_terminal::TerminalView;
use canban_types::ManagedTerminalKind;
use canban_ui_kit::{pull_refresh_scroll, tokens::*, *};
use nsh_base::{MenuButtonItem, context_menu, menu_button, nav_page};
use nsh_core::{
    Haptic,
    element::{desktop::MenuItem, header::HeaderItem},
    prelude::*,
};
use serde_json::json;

#[derive(Clone, PartialEq)]
struct TerminalListRow {
    id: String,
    card_id: Option<String>,
    title: String,
    subtitle: String,
    icon: Icon,
    icon_tint: ThemeColor,
    mark: Option<StatusMarkKind>,
    status_word: String,
    status_color: ThemeColor,
}

#[derive(Clone, PartialEq, Default)]
struct TerminalsTabModel {
    card_agents: Vec<TerminalListRow>,
    worktrees: Vec<TerminalListRow>,
    shells: Vec<TerminalListRow>,
}

fn interrupt_terminal(ui: Ui, terminal_id: String) {
    trigger_haptic(Haptic::Selection);
    if !crate::backend::is_connected() {
        ui.notify("Sent Ctrl+C");
        return;
    }
    dispatch(
        ui,
        "terminal.interrupt",
        json!({ "terminal_id": terminal_id }),
    );
}

fn close_terminal_confirmed(
    ui: Ui,
    terminal_id: String,
    title: String,
    on_closed: Option<std::rc::Rc<dyn Fn()>>,
) {
    let tid = terminal_id;
    confirm_destructive(
        ui,
        format!("Close {title}?"),
        "Terminate this terminal process.",
        "Close",
        move || {
            crate::inbox::state::close_terminal_from_inbox(ui, tid.clone(), title.clone());
            if let Some(cb) = &on_closed {
                cb();
            }
        },
    );
}

fn to_row(e: &Entry) -> TerminalListRow {
    let (icon, provider_name) = e
        .provider()
        .or_else(|| {
            e.saved
                .active_agent_engine()
                .map(|eng| (crate::inbox::engine_icon(Some(eng)), eng.to_owned()))
        })
        .unwrap_or((
            if e.agent.is_some() {
                Icon::Agent
            } else {
                Icon::Terminal
            },
            String::new(),
        ));

    let mark = status_mark_kind(e);
    let (status_word, status_color) = sidebar_status_spec(e)
        .map(|(w, c)| (w.into_owned(), c))
        .unwrap_or_else(|| (String::new(), MUTED));

    let icon_tint = if e.needs_attention() || e.herdr_status().is_attention() {
        ATTENTION
    } else if e.herdr_status().is_working() {
        ACCENT
    } else if status_color == DANGER {
        DANGER
    } else {
        MUTED
    };

    let mut sub_parts = Vec::new();
    if !e.project_name.is_empty() {
        sub_parts.push(e.project_name.clone());
    }
    if !provider_name.is_empty() && e.agent.is_some() {
        sub_parts.push(provider_name);
    }
    if let Some(cwd) = e.cwd()
        && sub_parts.is_empty()
    {
        let short = cwd.rsplit('/').next().unwrap_or(&cwd).to_owned();
        if !short.is_empty() {
            sub_parts.push(short);
        }
    }
    if sub_parts.is_empty() {
        sub_parts.push(e.host_id.clone());
    }

    TerminalListRow {
        id: e.saved.id.clone(),
        card_id: e.card.as_ref().map(|c| c.id.clone()),
        title: e.title(),
        subtitle: sub_parts.join(" · "),
        icon,
        icon_tint,
        mark,
        status_word,
        status_color,
    }
}

fn build_terminals_model(ui: Ui) -> TerminalsTabModel {
    ui.terminal_entries.with(|entries| {
        let mut out = TerminalsTabModel::default();
        for e in entries.iter().filter(|e| e.listed()) {
            let row = to_row(e);
            if e.card.is_some() && e.agent.is_some() {
                out.card_agents.push(row);
            } else if matches!(e.managed_kind(), Some(ManagedTerminalKind::MergeQueue))
                || e.location.as_ref().is_some_and(|l| l.worktree_id.is_some())
            {
                out.worktrees.push(row);
            } else {
                out.shells.push(row);
            }
        }
        out
    })
}

fn render_terminal_row(ui: Ui, row: TerminalListRow, state: MobileState) -> AnyElement {
    let tid = row.id.clone();
    let tid_menu = row.id.clone();
    let title_close = row.title.clone();
    let card_menu = row.card_id.clone();

    let row_btn = press_target()
        .w_full()
        .min_h(px(56.))
        .px_4()
        .py_2p5()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .cursor_pointer()
        .active(|s| s.opacity(0.72))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .flex_1()
                .min_w(px(0.))
                .child(
                    icon_tinted(row.icon, row.icon_tint)
                        .w(px(18.))
                        .h(px(18.))
                        .flex_shrink_0(),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .gap_0p5()
                        .child(
                            text(row.title)
                                .text_size(px(15.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT)
                                .truncate(),
                        )
                        .child(mobile_footnote(row.subtitle).truncate()),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_shrink_0()
                .children(row.mark.map(render_status_mark))
                .children((!row.status_word.is_empty()).then(|| {
                    text(row.status_word)
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(row.status_color)
                }))
                .child(disclosure_chevron()),
        )
        .on_press(move |_: &()| {
            trigger_haptic(Haptic::Selection);
            state.push_terminal(tid.clone());
        });

    if !is_mobile_os() {
        return row_btn.into_element();
    }

    let mut items = vec![MenuButtonItem::new("open", "Open Terminal").icon("terminal")];
    if card_menu.is_some() {
        items.push(MenuButtonItem::new("card", "Open Card").icon("doc.text"));
    }
    items.push(MenuButtonItem::new("interrupt", "Interrupt (^C)").icon("stop.circle"));
    items.push(MenuButtonItem::section(
        "",
        [MenuButtonItem::new("close", "Close Terminal").icon("xmark.circle")],
    ));

    context_menu()
        .w_full()
        .items(items)
        .trigger(row_btn)
        .on_select(move |choice: &SharedString| {
            trigger_haptic(Haptic::Selection);
            match choice.as_ref() {
                "open" => state.push_terminal(tid_menu.clone()),
                "card" => {
                    if let Some(cid) = &card_menu {
                        state.push_card(cid.clone(), CardSegment::Conversation);
                    }
                }
                "interrupt" => interrupt_terminal(ui, tid_menu.clone()),
                "close" => {
                    close_terminal_confirmed(ui, tid_menu.clone(), title_close.clone(), None)
                }
                _ => {}
            }
        })
        .into_element()
}

fn render_group(
    ui: Ui,
    title: &'static str,
    rows: Vec<TerminalListRow>,
    state: MobileState,
) -> Option<View> {
    if rows.is_empty() {
        return None;
    }
    let rendered: Vec<AnyElement> = rows
        .into_iter()
        .map(|row| render_terminal_row(ui, row, state))
        .collect();
    Some(
        view()
            .w_full()
            .child(view().px_4().child(section_header(title, None)))
            .child(hairline_list_inset(rendered, 44.0)),
    )
}

pub(crate) fn screen(ui: Ui, state: MobileState) -> impl IntoElement {
    let scroll_ref = element_ref();
    let model = computed(move || build_terminals_model(ui));

    let body = pull_refresh_scroll(
        scroll().w_full().bg(BACKGROUND).child(
            view()
                .w_full()
                .pt_2()
                .pb(px(tab_scroll_bottom_pad()))
                .gap_4()
                .child(
                    view()
                        .w_full()
                        .px_4()
                        .child(offline_sync_banner(ui, state, None)),
                )
                .child(dynamic(move || {
                    let m = model.get();
                    if m.card_agents.is_empty() && m.shells.is_empty() && m.worktrees.is_empty() {
                        return empty_state(
                            Icon::Terminal,
                            Tone::Neutral,
                            "No open terminals",
                            "",
                            Some(
                                mobile_inline_button("New Terminal", Variant::Primary, move |_| {
                                    state.open_sheet(MobileSheet::CreateTerminal);
                                })
                                .into_element(),
                            ),
                        )
                        .into_element();
                    }
                    let mut col = view().w_full().gap_5();
                    if let Some(s) = render_group(ui, "CARD AGENTS", m.card_agents, state) {
                        col = col.child(s);
                    }
                    if let Some(s) = render_group(ui, "WORKTREES", m.worktrees, state) {
                        col = col.child(s);
                    }
                    if let Some(s) = render_group(ui, "SHELLS", m.shells, state) {
                        col = col.child(s);
                    }
                    col.into_element()
                })),
        ),
        scroll_ref,
        move || ui.refreshing.get(),
        move || refresh(ui),
    );

    view()
        .size_full()
        .flex_col()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(body)
}

pub(crate) fn pushed_page(ui: Ui, state: MobileState, terminal_id: String) -> impl IntoElement {
    let tid_lookup = terminal_id.clone();
    let entry_memo = computed(move || {
        ui.terminal_entries
            .with(|rows| rows.iter().find(|e| e.saved.id == tid_lookup).cloned())
    });
    let host_offline = computed(move || {
        let Some(entry) = entry_memo.get() else {
            return false;
        };
        ui.snapshot.with(|s| s.is_host_offline(&entry.host_id))
    });
    let focus_ticket = signal(1_u64);

    let title_fn = move || {
        entry_memo
            .get()
            .map(|e| e.title())
            .unwrap_or_else(|| "Terminal".to_owned())
    };

    let tid_ios = terminal_id.clone();
    let tid_and = terminal_id.clone();
    let tid_gal = terminal_id.clone();

    let desktop_header = view()
        .w_full()
        .h(px(52.))
        .px_3()
        .border_b_1()
        .border_color(BORDER)
        .bg(BACKGROUND)
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            press_target()
                .min_h(px(40.))
                .px_2()
                .flex_row()
                .items_center()
                .gap_1()
                .child(icon_tinted(Icon::Back, ACCENT).w(px(18.)).h(px(18.)))
                .child(
                    text("Back")
                        .text_size(px(15.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ACCENT),
                )
                .on_press(move |_: &()| {
                    state.terminal_fullscreen.set(false);
                    state.pop();
                }),
        )
        .child(mobile_title(title_fn).truncate().flex_1().min_w(px(0.)))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(
                    press_target()
                        .w(px(34.))
                        .h(px(34.))
                        .rounded(px(8.))
                        .items_center()
                        .justify_center()
                        .accessibility_label(Some(SharedString::from("Interrupt")))
                        .child(
                            icon_tinted(Icon::CirclePause, ATTENTION)
                                .w(px(16.))
                                .h(px(16.)),
                        )
                        .on_press(move |_: &()| interrupt_terminal(ui, tid_gal.clone())),
                )
                .child(
                    press_target()
                        .w(px(34.))
                        .h(px(34.))
                        .rounded(px(8.))
                        .items_center()
                        .justify_center()
                        .child(dynamic(move || {
                            icon_tinted(
                                if state.terminal_fullscreen.get() {
                                    Icon::Collapse
                                } else {
                                    Icon::Expand
                                },
                                ACCENT,
                            )
                            .w(px(16.))
                            .h(px(16.))
                        }))
                        .on_press(move |_: &()| state.terminal_fullscreen.update(|f| *f = !*f)),
                ),
        );

    let mut page = nav_page()
        .key(format!("mobile-term:{terminal_id}"))
        .title(title_fn)
        .header_large_title(false)
        .header_shadow_visible(false)
        .header_style_with(|s| s.text_color(TEXT).tint_color(ACCENT))
        .hides_bottom_bar_when_pushed(true)
        .header_visible(move || !state.terminal_fullscreen.get())
        .on_dismiss(move |_: &()| {
            state.terminal_fullscreen.set(false);
            state.pop();
        });

    if is_ios() {
        page = page.header_right_items(move || {
            let entry = entry_memo.get_untracked();
            let card_id = entry
                .as_ref()
                .and_then(|e| e.card.as_ref().map(|c| c.id.clone()));
            let close_title = entry
                .as_ref()
                .map(Entry::title)
                .unwrap_or_else(|| "Terminal".to_owned());
            let tid_int = tid_ios.clone();
            let tid_cls = tid_ios.clone();

            let mut menu_items = Vec::new();
            if let Some(cid) = card_id {
                menu_items.push(MenuItem::new("Open Card").symbol("doc.text").on_select(
                    move || {
                        trigger_haptic(Haptic::Selection);
                        state.push_card(cid.clone(), CardSegment::Conversation);
                    },
                ));
            }
            menu_items.push(
                MenuItem::new("Interrupt (^C)")
                    .symbol("stop.circle")
                    .on_select(move || interrupt_terminal(ui, tid_int.clone())),
            );
            menu_items.push(MenuItem::separator());
            menu_items.push(
                MenuItem::new("Close Terminal")
                    .symbol("xmark.circle")
                    .on_select(move || {
                        close_terminal_confirmed(
                            ui,
                            tid_cls.clone(),
                            close_title.clone(),
                            Some(std::rc::Rc::new(move || state.pop())),
                        );
                    }),
            );

            vec![
                HeaderItem::symbol(if state.terminal_fullscreen.get() {
                    "arrow.down.right.and.arrow.up.left"
                } else {
                    "arrow.up.left.and.arrow.down.right"
                })
                .key("term-fullscreen")
                .label("Toggle fullscreen")
                .on_press(move || {
                    trigger_haptic(Haptic::Selection);
                    state.terminal_fullscreen.update(|f| *f = !*f);
                }),
                HeaderItem::symbol("ellipsis.circle")
                    .key("term-more")
                    .label("Terminal actions")
                    .menu(menu_items),
            ]
        });
    } else if is_android() {
        page = page.header_right(dynamic(move || {
            let entry = entry_memo.get();
            let card_id = entry
                .as_ref()
                .and_then(|e| e.card.as_ref().map(|c| c.id.clone()));
            let close_title = entry
                .as_ref()
                .map(Entry::title)
                .unwrap_or_else(|| "Terminal".to_owned());
            let mut items = Vec::new();
            if card_id.is_some() {
                items.push(MenuButtonItem::new("card", "Open Card"));
            }
            items.push(MenuButtonItem::new("interrupt", "Interrupt (^C)"));
            items.push(MenuButtonItem::new("fullscreen", "Toggle Fullscreen"));
            items.push(MenuButtonItem::new("close", "Close Terminal"));
            let tid_sel = tid_and.clone();
            menu_button()
                .title("Terminal")
                .items(items)
                .on_select(move |choice: &SharedString| {
                    trigger_haptic(Haptic::Selection);
                    match choice.as_ref() {
                        "card" => {
                            if let Some(cid) = &card_id {
                                state.push_card(cid.clone(), CardSegment::Conversation);
                            }
                        }
                        "interrupt" => interrupt_terminal(ui, tid_sel.clone()),
                        "fullscreen" => state.terminal_fullscreen.update(|f| *f = !*f),
                        "close" => close_terminal_confirmed(
                            ui,
                            tid_sel.clone(),
                            close_title.clone(),
                            Some(std::rc::Rc::new(move || state.pop())),
                        ),
                        _ => {}
                    }
                })
                .child(
                    view()
                        .w(px(44.))
                        .h(px(44.))
                        .items_center()
                        .justify_center()
                        .child(icon_tinted(Icon::More, ACCENT).w(px(21.)).h(px(21.))),
                )
        }));
    } else {
        page = page.header(desktop_header);
    }

    page.child(
        view()
            .size_full()
            .flex_col()
            .min_h(px(0.))
            .child(dynamic(move || {
                let Some(entry) = entry_memo.get() else {
                    return view()
                        .flex_1()
                        .justify_center()
                        .child(empty_state(
                            Icon::Terminal,
                            Tone::Neutral,
                            "Terminal ended",
                            "",
                            None,
                        ))
                        .into_element();
                };
                let focus_req = move || canban_terminal::FocusRequest {
                    ticket: focus_ticket.get(),
                    enabled: state.sheet.with(Option::is_none),
                };
                let offline_msg = move || {
                    if host_offline.get() {
                        "Terminal needs a connection".to_owned()
                    } else {
                        String::new()
                    }
                };

                let term_view = if !crate::backend::is_connected() {
                    TerminalView::preview(
                        format!(
                            "\x1b[1;36m{}\x1b[0m · Portable Ghostty VT\r\n$ ls -la\r\ntotal 48\r\ndrwxr-xr-x  crates/\r\n-rw-r--r--  Cargo.toml\r\n> ",
                            entry.title()
                        ),
                        crate::utilities::terminal_theme,
                        move || crate::utilities::terminal_font(ui),
                        focus_req,
                    )
                } else {
                    let bind = entry.binding();
                    let core_id = bind
                        .as_ref()
                        .and_then(|v| v["core_id"].as_str())
                        .unwrap_or("")
                        .to_owned();
                    let tty = bind
                        .as_ref()
                        .and_then(|v| v["terminal"].as_str())
                        .unwrap_or("")
                        .to_owned();
                    let host_id = entry.host_id.clone();
                    let ro = entry.ended()
                        || bind
                            .as_ref()
                            .is_some_and(|v| v["read_only"] == true);
                    TerminalView::live(
                        move || {
                            (!core_id.is_empty() && !tty.is_empty()).then(|| {
                                canban_terminal::LiveBinding {
                                    session_epoch: crate::terminals::host_connect_epoch(
                                        ui, &host_id,
                                    ),
                                    host_id: host_id.clone(),
                                    config: crate::config_for_host(&host_id),
                                    core_id: core_id.clone(),
                                    terminal: tty.clone(),
                                    transport: crate::backend::terminal_transport(&host_id),
                                }
                            })
                        },
                        crate::utilities::terminal_theme,
                        move || crate::utilities::terminal_font(ui),
                        focus_req,
                    )
                    .read_only(move || ro || host_offline.get())
                };

                term_view
                    .mobile_controls(true)
                    .offline_notice(offline_msg)
                    .on_fullscreen_toggle(move || {
                        state.terminal_fullscreen.update(|f| *f = !*f);
                    })
                    .into_element()
            })),
    )
}

pub(crate) fn create_terminal_sheet_body(ui: Ui, state: MobileState) -> impl IntoElement {
    let hosts = untracked(|| mobile_hosts(ui));
    let initial_host = hosts.first().map(|h| h.id.clone()).unwrap_or_default();
    let selected_host = signal(initial_host);
    let projects = untracked(|| ui.snapshot.with_untracked(|s| s.projects.clone()));
    let initial_proj = untracked(|| {
        let cur = ui.project.get_untracked();
        if !cur.is_empty() {
            cur
        } else {
            projects.first().map(|p| p.id.clone()).unwrap_or_default()
        }
    });
    let selected_proj = signal(initial_proj);

    view()
        .w_full()
        .gap_4()
        .children((!hosts.is_empty()).then(|| {
            view()
                .w_full()
                .gap_1p5()
                .child(section_header("HOST", None))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .children(hosts.into_iter().map(move |h| {
                            let hid = h.id.clone();
                            let hid_bg = hid.clone();
                            let hid_fg = hid;
                            press_target()
                                .h(px(36.))
                                .px_3p5()
                                .rounded_full()
                                .bg(move || {
                                    if selected_host.get() == hid_bg {
                                        SELECTED
                                    } else {
                                        SURFACE
                                    }
                                })
                                .items_center()
                                .justify_center()
                                .child(
                                    text(h.name)
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(move || {
                                            if selected_host.get() == hid_fg {
                                                ACCENT
                                            } else {
                                                TEXT
                                            }
                                        }),
                                )
                                .on_press(move |_: &()| {
                                    trigger_haptic(Haptic::Selection);
                                    selected_host.set(h.id.clone());
                                })
                        })),
                )
        }))
        .children((!projects.is_empty()).then(|| {
            view()
                .w_full()
                .gap_1p5()
                .child(section_header("PROJECT", None))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .children(projects.into_iter().map(move |p| {
                            let pid = p.id.clone();
                            let pid_bg = pid.clone();
                            let pid_fg = pid;
                            press_target()
                                .h(px(36.))
                                .px_3p5()
                                .rounded_full()
                                .bg(move || {
                                    if selected_proj.get() == pid_bg {
                                        SELECTED
                                    } else {
                                        SURFACE
                                    }
                                })
                                .items_center()
                                .justify_center()
                                .child(
                                    text(p.name)
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(move || {
                                            if selected_proj.get() == pid_fg {
                                                ACCENT
                                            } else {
                                                TEXT
                                            }
                                        }),
                                )
                                .on_press(move |_: &()| {
                                    trigger_haptic(Haptic::Selection);
                                    selected_proj.set(p.id.clone());
                                })
                        })),
                )
        }))
        .child(
            mobile_button("Start Shell", Variant::Primary, move |_| {
                let pid = selected_proj.get_untracked();
                let hid = selected_host.get_untracked();
                let mut params = json!({});
                if !pid.is_empty() {
                    params["project_id"] = json!(pid);
                }
                if !hid.is_empty() {
                    params["host_id"] = json!(hid);
                }
                if !crate::backend::is_connected() {
                    let before: std::collections::BTreeSet<String> = ui
                        .terminal_entries
                        .with_untracked(|rows| rows.iter().map(|e| e.saved.id.clone()).collect());
                    crate::terminals::launch_custom(ui, params);
                    state.close_sheet();
                    if let Some(created) = ui.terminal_entries.with_untracked(|rows| {
                        rows.iter()
                            .find(|e| !before.contains(&e.saved.id))
                            .map(|e| e.saved.id.clone())
                    }) {
                        state.push_terminal(created);
                    }
                    return;
                }
                crate::terminals::launch_custom(ui, params);
                state.close_sheet();
            })
            .mt_1(),
        )
}
