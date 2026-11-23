//! Mobile `Terminals` tab (`Card Agents` / `Shells` / `Worktrees` + creation sheet)
//! and pushed full-screen `Terminal` route.
use super::{
    MobileSheet, MobileState,
    chrome::{offline_sync_banner, trigger_haptic},
    mobile_hosts,
};
use crate::{presenter::*, terminals::Entry};
use canban_terminal::TerminalView;
use canban_types::{HerdrStatus, ManagedTerminalKind};
use canban_ui_kit::{pull_refresh_scroll, tokens::*, *};
use nsh_base::nav_page;
use nsh_core::{Haptic, prelude::*};
use serde_json::json;

#[derive(Clone, PartialEq)]
struct TerminalListRow {
    id: String,
    title: String,
    subtitle: String,
    status_text: String,
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
                || e.saved.origin_project_id.is_some() && e.agent.is_none()
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

fn render_group(
    title: &'static str,
    rows: Vec<TerminalListRow>,
    state: MobileState,
) -> Option<View> {
    if rows.is_empty() {
        return None;
    }
    let mut col = view().w_full().gap_2().child(
        label(title)
            .text_size(px(11.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(MUTED),
    );
    for row in rows {
        let tid = row.id.clone();
        let tone = if row.is_attention {
            Tone::Attention
        } else if row.is_live {
            Tone::Info
        } else {
            Tone::Neutral
        };
        col = col.child(
            panel().p_3().child(
                press_target()
                    .w_full()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .cursor_pointer()
                    .child(
                        view()
                            .flex_1()
                            .min_w(px(0.))
                            .gap_0p5()
                            .child(label(row.title).font_weight(FontWeight::MEDIUM).truncate())
                            .child(muted(row.subtitle).truncate()),
                    )
                    .child(if row.is_live {
                        live_status(row.status_text, tone).into_element()
                    } else {
                        status(row.status_text, tone).into_element()
                    })
                    .on_press(move |_: &()| {
                        trigger_haptic(Haptic::Selection);
                        state.push_terminal(tid.clone());
                    }),
            ),
        );
    }
    Some(col)
}

pub(crate) fn screen(ui: Ui, state: MobileState) -> impl IntoElement {
    let scroll_ref = element_ref();
    let model = computed(move || build_terminals_model(ui));

    let body = pull_refresh_scroll(
        scroll().child(view().w_full().p_4().gap_4().child(dynamic(move || {
            let m = model.get();
            if m.card_agents.is_empty() && m.shells.is_empty() && m.worktrees.is_empty() {
                return panel()
                    .p_5()
                    .gap_3()
                    .items_center()
                    .child(heading("No open terminals"))
                    .child(muted(
                        "Launch a shell or Agent terminal on any paired host.",
                    ))
                    .child(action("+ New Terminal", Variant::Primary, move |_| {
                        state.open_sheet(MobileSheet::CreateTerminal);
                    }))
                    .into_element();
            }
            let mut col = view().w_full().gap_4();
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
        }))),
        scroll_ref,
        move || ui.refreshing.get(),
        move || refresh(ui),
    );

    view()
        .size_full()
        .flex_col()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(
            view()
                .w_full()
                .px_4()
                .py_2p5()
                .border_b_1()
                .border_color(BORDER)
                .bg(SURFACE)
                .flex_row()
                .items_center()
                .justify_between()
                .child(heading("Terminals"))
                .child(compact_action("+ New", Variant::Primary, move |_| {
                    trigger_haptic(Haptic::Selection);
                    state.open_sheet(MobileSheet::CreateTerminal);
                })),
        )
        .child(offline_sync_banner(ui, state, None))
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
        .px_3()
        .py_2p5()
        .border_b_1()
        .border_color(BORDER)
        .bg(SURFACE)
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(compact_action("← Back", Variant::Quiet, move |_| {
            state.terminal_fullscreen.set(false);
            state.pop();
        }))
        .child(
            label(title_fn)
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .flex_1()
                .min_w(px(0.)),
        )
        .child(compact_action(
            move || {
                if state.terminal_fullscreen.get() {
                    "Exit"
                } else {
                    "⤢"
                }
            },
            Variant::Quiet,
            move |_| state.terminal_fullscreen.update(|f| *f = !*f),
        ));

    nav_page()
        .key(format!("mobile-term:{terminal_id}"))
        .title(title_fn)
        .hides_bottom_bar_when_pushed(true)
        .header_visible(move || !state.terminal_fullscreen.get())
        .on_dismiss(move |_: &()| {
            state.terminal_fullscreen.set(false);
            state.pop();
        })
        .header(desktop_header)
        .child(view().size_full().flex_col().min_h(px(0.)).child(dynamic(move || {
            let Some(entry) = entry_memo.get() else {
                return view()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .p_5()
                    .child(muted("Terminal session ended or unavailable."))
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
        })))
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
        .gap_3()
        .children((!hosts.is_empty()).then(|| {
            view().w_full().gap_1p5().child(label("Host")).child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap_1p5()
                    .children(hosts.into_iter().map(move |h| {
                        let hid = h.id.clone();
                        let hid_press = hid.clone();
                        compact_segment(
                            h.name,
                            None,
                            move || selected_host.get() == hid,
                            move |_| selected_host.set(hid_press.clone()),
                        )
                    })),
            )
        }))
        .children((!projects.is_empty()).then(|| {
            view().w_full().gap_1p5().child(label("Project")).child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap_1p5()
                    .children(projects.into_iter().map(move |p| {
                        let pid = p.id.clone();
                        let pid_press = pid.clone();
                        compact_segment(
                            p.name,
                            None,
                            move || selected_proj.get() == pid,
                            move |_| selected_proj.set(pid_press.clone()),
                        )
                    })),
            )
        }))
        .child(action(
            "Start Interactive Shell",
            Variant::Primary,
            move |_| {
                trigger_haptic(Haptic::Selection);
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
            },
        ))
}
