//! Mobile `Terminals` tab (`Card Agents` / `Shells` / `Worktrees` + creation Sheet)
//! and pushed full-screen `Terminal` route.
use super::{
    CardSegment, MobileSheet, MobileState,
    chrome::{
        disclosure_chevron, empty_state, grouped_card_with_inset, icon_badge, is_android, is_ios,
        is_mobile_os, mobile_button, mobile_footnote, mobile_inline_button, mobile_title,
        offline_sync_banner, section_header, tab_scroll_bottom_pad, trigger_haptic,
    },
    mobile_hosts,
};
use crate::{presenter::*, terminals::Entry};
use canban_terminal::TerminalView;
use canban_types::{HerdrStatus, ManagedTerminalKind};
use canban_ui_kit::{pull_refresh_scroll, tokens::*, *};
use nsh_base::{MenuButtonItem, context_menu, nav_page};
use nsh_core::{Haptic, element::header::HeaderItem, prelude::*};
use serde_json::json;

#[derive(Clone, PartialEq)]
struct TerminalListRow {
    id: String,
    card_id: Option<String>,
    title: String,
    subtitle: String,
    status_text: String,
    is_agent: bool,
    is_live: bool,
    is_attention: bool,
}

#[derive(Clone, PartialEq, Default)]
struct TerminalsTabModel {
    card_agents: Vec<TerminalListRow>,
    shells: Vec<TerminalListRow>,
    worktrees: Vec<TerminalListRow>,
}

fn to_row(e: &Entry) -> TerminalListRow {
    let herdr = e.herdr_status();
    let subtitle = if !e.project_name.is_empty() {
        format!("{} · {}", e.project_name, e.display_status())
    } else {
        e.display_status()
    };
    TerminalListRow {
        id: e.saved.id.clone(),
        card_id: e.card.as_ref().map(|c| c.id.clone()),
        title: e.title(),
        subtitle,
        status_text: match herdr {
            HerdrStatus::Working | HerdrStatus::Starting => "Live".into(),
            HerdrStatus::Blocked => "Blocked".into(),
            HerdrStatus::Error | HerdrStatus::Disconnected => "Attention".into(),
            HerdrStatus::Done => "Done".into(),
            HerdrStatus::Idle | HerdrStatus::PlainShell => "Idle".into(),
            HerdrStatus::Queued => "Queued".into(),
            HerdrStatus::Closing => "Closing".into(),
            HerdrStatus::Ended => "Ended".into(),
        },
        is_agent: e.agent.is_some(),
        is_live: herdr.is_working(),
        is_attention: herdr.is_attention() || e.needs_attention(),
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
                || (e.project_id.is_some() && e.agent.is_none())
            {
                if e.saved.title().contains("worktree")
                    || e.location.as_ref().is_some_and(|l| l.worktree_id.is_some())
                {
                    out.worktrees.push(row);
                } else {
                    out.shells.push(row);
                }
            } else {
                out.shells.push(row);
            }
        }
        out
    })
}

fn render_terminal_row(row: TerminalListRow, state: MobileState) -> AnyElement {
    let tid = row.id.clone();
    let tid_menu = row.id.clone();
    let card_menu = row.card_id.clone();
    let tone = if row.is_attention {
        Tone::Attention
    } else if row.is_live {
        Tone::Info
    } else {
        Tone::Neutral
    };
    let (badge_bg, badge_fg) = tone_colors(tone);
    let ic = if row.is_agent {
        Icon::Agent
    } else {
        Icon::Terminal
    };

    let row_btn = press_target()
        .w_full()
        .min_h(px(60.))
        .px_4()
        .py_2p5()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .cursor_pointer()
        .active(|s| s.opacity(0.75))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .flex_1()
                .min_w(px(0.))
                .child(icon_badge(ic, badge_fg, badge_bg))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.))
                        .gap_0p5()
                        .child(
                            text(row.title)
                                .text_size(px(16.))
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
                .child(if row.is_live {
                    live_status(row.status_text, tone).into_element()
                } else {
                    status(row.status_text, tone).into_element()
                })
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
                _ => {}
            }
        })
        .into_element()
}

fn render_group(
    title: &'static str,
    rows: Vec<TerminalListRow>,
    state: MobileState,
) -> Option<View> {
    if rows.is_empty() {
        return None;
    }
    let rendered: Vec<AnyElement> = rows
        .into_iter()
        .map(|row| render_terminal_row(row, state))
        .collect();
    Some(
        view()
            .w_full()
            .child(section_header(title, None))
            .child(grouped_card_with_inset(rendered, 60.0)),
    )
}

pub(crate) fn screen(ui: Ui, state: MobileState) -> impl IntoElement {
    let scroll_ref = element_ref();
    let model = computed(move || build_terminals_model(ui));

    let body = pull_refresh_scroll(
        scroll().w_full().bg(BACKGROUND).child(
            view()
                .w_full()
                .px_4()
                .pt_2()
                .pb(px(tab_scroll_bottom_pad()))
                .gap_5()
                .child(offline_sync_banner(ui, state, None))
                .child(dynamic(move || {
                    let m = model.get();
                    if m.card_agents.is_empty() && m.shells.is_empty() && m.worktrees.is_empty() {
                        return empty_state(
                            Icon::Terminal,
                            Tone::Neutral,
                            "No open terminals",
                            "Launch a shell or inspect an active Agent terminal on any paired machine.",
                            Some(
                                mobile_inline_button("+ New Terminal", Variant::Primary, move |_| {
                                    state.open_sheet(MobileSheet::CreateTerminal);
                                })
                                .into_element(),
                            ),
                        )
                        .into_element();
                    }
                    let mut col = view().w_full().gap_6();
                    if let Some(s) = render_group("CARD AGENTS", m.card_agents, state) {
                        col = col.child(s);
                    }
                    if let Some(s) = render_group("SHELLS", m.shells, state) {
                        col = col.child(s);
                    }
                    if let Some(s) = render_group("WORKTREES", m.worktrees, state) {
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

    let desktop_header = view()
        .w_full()
        .h(px(52.))
        .px_3()
        .border_b_1()
        .border_color(BORDER)
        .bg(SURFACE)
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
            press_target()
                .w(px(40.))
                .h(px(40.))
                .rounded(px(9.))
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
                    .w(px(17.))
                    .h(px(17.))
                }))
                .on_press(move |_: &()| state.terminal_fullscreen.update(|f| *f = !*f)),
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
            ]
        });
    } else if is_android() {
        page = page.header_right(
            press_target()
                .w(px(44.))
                .h(px(44.))
                .items_center()
                .justify_center()
                .child(icon_tinted(Icon::Expand, ACCENT).w(px(19.)).h(px(19.)))
                .on_press(move |_: &()| {
                    trigger_haptic(Haptic::Selection);
                    state.terminal_fullscreen.update(|f| *f = !*f);
                }),
        );
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
                            "This terminal session has exited or is unavailable.",
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
                            "\x1b[1;36m{}\x1b[0m · Portable Ghostty VT Preview\r\n$ ls -la\r\ntotal 48\r\ndrwxr-xr-x  crates/\r\n-rw-r--r--  Cargo.toml\r\n> ",
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
                            let hid_border = hid.clone();
                            let hid_bg = hid.clone();
                            let hid_press = hid;
                            press_target()
                                .h(px(38.))
                                .px_3p5()
                                .rounded_full()
                                .border_1()
                                .border_color(move || {
                                    if selected_host.get() == hid_border {
                                        ACCENT
                                    } else {
                                        BORDER
                                    }
                                })
                                .bg(move || {
                                    if selected_host.get() == hid_bg {
                                        SELECTED
                                    } else {
                                        INSET
                                    }
                                })
                                .items_center()
                                .justify_center()
                                .child(
                                    text(h.name)
                                        .text_size(px(14.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(move || {
                                            if selected_host.get() == hid_press {
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
                            let pid_border = pid.clone();
                            let pid_bg = pid.clone();
                            let pid_press = pid;
                            press_target()
                                .h(px(38.))
                                .px_3p5()
                                .rounded_full()
                                .border_1()
                                .border_color(move || {
                                    if selected_proj.get() == pid_border {
                                        ACCENT
                                    } else {
                                        BORDER
                                    }
                                })
                                .bg(move || {
                                    if selected_proj.get() == pid_bg {
                                        SELECTED
                                    } else {
                                        INSET
                                    }
                                })
                                .items_center()
                                .justify_center()
                                .child(
                                    text(p.name)
                                        .text_size(px(14.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(move || {
                                            if selected_proj.get() == pid_press {
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
            mobile_button("Start Interactive Shell", Variant::Primary, move |_| {
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
