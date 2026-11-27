//! Daily Inbox: Linear × Shortwave single-page triage workspace with visual-archetype rows,
//! Google-Inbox-style project bundles, live Agent Terminal viewports, Schedule health track,
//! and End-of-Day Defer.
pub mod state;
pub use state::{
    State, cutoff_minutes_from_utility, defer_remaining_all, init, local_tz_offset_secs,
};

use crate::presenter::*;
use canban_notifications::{
    DailyInbox, DeferredInboxItem, DoneInboxItem, GitHubProjectStack, GitHubStackItem,
    InboxCardWork, InboxTerminalWork, NeedsYouItem, NeedsYouKind, ScheduleCardStack,
    SchedulePulseItem,
};
use canban_types::{GitHubReference, HerdrStatus};
use canban_ui_kit::{compact_home, tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;
use state::*;

fn engine_icon(engine: Option<&str>) -> Icon {
    match engine
        .map(|e| e.strip_prefix("herdr:").unwrap_or(e))
        .unwrap_or("")
    {
        "codex" => Icon::Codex,
        "claude" => Icon::Claude,
        "agy" | "antigravity" => Icon::Agy,
        "gemini" => Icon::Gemini,
        "opencode" => Icon::OpenCode,
        "cursor" | "cursor-agent" => Icon::Cursor,
        "pi" => Icon::Pi,
        "omp" => Icon::Omp,
        "copilot" => Icon::Copilot,
        "droid" => Icon::Droid,
        "amp" => Icon::Amp,
        "kimi" => Icon::Kimi,
        "kiro" | "kiro-cli" => Icon::Kiro,
        "qwen" => Icon::Qwen,
        "devin" => Icon::Devin,
        "cline" => Icon::Cline,
        "grok" => Icon::Grok,
        "hermes" => Icon::Hermes,
        "kilo" => Icon::Kilo,
        "qodercli" | "qoder" => Icon::Qoder,
        "letta" => Icon::Letta,
        "mastracode" | "mastra" => Icon::Mastra,
        "maki" => Icon::Maki,
        "muse" => Icon::Muse,
        _ => Icon::Agent,
    }
}

fn terminal_status_visual(status: HerdrStatus) -> (&'static str, Tone, bool, ThemeColor) {
    match status {
        HerdrStatus::Blocked => ("Waiting", Tone::Attention, false, ATTENTION),
        HerdrStatus::Done => ("Done", Tone::Success, false, SUCCESS),
        HerdrStatus::Working => ("Running", Tone::Info, true, ACCENT),
        HerdrStatus::Starting | HerdrStatus::Queued => ("Starting", Tone::Info, true, ACCENT),
        HerdrStatus::Idle => ("Idle", Tone::Neutral, false, MUTED),
        HerdrStatus::Error => ("Error", Tone::Danger, false, DANGER),
        HerdrStatus::Disconnected => ("Disconnected", Tone::Danger, false, DANGER),
        _ => ("Open", Tone::Neutral, false, MUTED),
    }
}

fn shorten_path(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    compact_home(path, &home)
}

fn format_today_date() -> String {
    chrono::Local::now().format("%a, %b %-d").to_string()
}

/// 32×32 rounded visual-archetype icon tile with optional ringed corner status dot.
fn icon_tile_32(
    kind: Icon,
    bg: ThemeColor,
    fg: ThemeColor,
    corner_dot: Option<ThemeColor>,
) -> View {
    let mut tile = view()
        .relative()
        .flex_shrink_0()
        .w(px(32.))
        .h(px(32.))
        .rounded(px(8.))
        .bg(bg)
        .items_center()
        .justify_center()
        .pointer_events(PointerEvents::None)
        .child(icon_tinted(kind, fg).w(px(16.)).h(px(16.)));
    if let Some(dot) = corner_dot {
        tile = tile.child(corner_badge_dot(dot));
    }
    tile
}

/// 32×32 tile showing a full-color branded Agent provider logo + optional corner status dot.
fn agent_icon_tile_32(
    engine: Option<&str>,
    bg: ThemeColor,
    corner_dot: Option<ThemeColor>,
) -> View {
    let mut tile = view()
        .relative()
        .flex_shrink_0()
        .w(px(32.))
        .h(px(32.))
        .rounded(px(8.))
        .bg(bg)
        .items_center()
        .justify_center()
        .pointer_events(PointerEvents::None)
        .child(icon_colored(engine_icon(engine), 18.));
    if let Some(dot) = corner_dot {
        tile = tile.child(corner_badge_dot(dot));
    }
    tile
}

/// 32×32 dark terminal-chrome tile so CLI/Agent terminals look unmistakably like terminal sessions.
fn terminal_icon_tile_32(engine: Option<&str>, corner_dot: Option<ThemeColor>) -> View {
    let glyph = if let Some(eng) = engine {
        icon_colored(engine_icon(Some(eng)), 17.).into_element()
    } else {
        icon_tinted(Icon::Terminal, TEXT)
            .w(px(16.))
            .h(px(16.))
            .into_element()
    };
    let mut tile = view()
        .relative()
        .flex_shrink_0()
        .w(px(32.))
        .h(px(32.))
        .rounded(px(8.))
        .bg(BACKGROUND)
        .border_1()
        .border_color(BORDER)
        .items_center()
        .justify_center()
        .pointer_events(PointerEvents::None)
        .child(glyph);
    if let Some(dot) = corner_dot {
        tile = tile.child(corner_badge_dot(dot));
    }
    tile
}

fn corner_badge_dot(color: ThemeColor) -> View {
    view()
        .absolute()
        .right(px(-2.))
        .bottom(px(-2.))
        .w(px(11.))
        .h(px(11.))
        .rounded_full()
        .bg(SURFACE)
        .items_center()
        .justify_center()
        .child(view().w(px(7.)).h(px(7.)).rounded_full().bg(color))
}

fn stacked_deck_lip() -> View {
    view()
        .mx_3()
        .h(px(4.))
        .bg(INSET)
        .border_l_1()
        .border_r_1()
        .border_b_1()
        .border_color(BORDER)
        .rounded_b(px(6.))
}

fn sheet_container() -> View {
    view()
        .bg(SURFACE)
        .border_1()
        .border_color(BORDER)
        .rounded(px(10.))
        .overflow_hidden()
}

fn section_heading(title: impl Into<String>, count: usize, trailing: Option<AnyElement>) -> View {
    let mut row = view()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .pt_1()
        .px_0p5()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    label(title.into())
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(MUTED),
                )
                .child(
                    muted(count.to_string())
                        .font_family(MONO_FONT)
                        .text_size(px(11.)),
                ),
        );
    if let Some(el) = trailing {
        row = row.child(el);
    }
    row
}

fn colored_count_chip(dot: ThemeColor, text_str: String) -> View {
    view()
        .flex_row()
        .items_center()
        .gap_1()
        .child(view().w(px(6.)).h(px(6.)).rounded_full().bg(dot))
        .child(label(text_str).text_size(px(12.)).text_color(TEXT))
}

fn inline_reply_box(ui: Ui, target: ReplyTarget) -> impl IntoElement {
    let target_check = target.clone();
    dynamic(move || {
        let active = ui.inbox.reply_target.get().as_ref() == Some(&target_check);
        if !active {
            return view().into_element();
        }
        let sending = ui.inbox.reply_sending.get();
        let placeholder = match ui.inbox.reply_target.get_untracked() {
            Some(ReplyTarget::Terminal { label, .. }) => {
                format!("Send input to {label} (Enter to send)…")
            }
            Some(ReplyTarget::GitHub { label, .. }) => {
                format!("Comment on GitHub {label}…")
            }
            Some(ReplyTarget::Card { label, .. }) => {
                format!("Reply to {label} (@agent supported)…")
            }
            None => "Write a reply…".into(),
        };
        let input = field(ui.inbox.reply_draft, &placeholder)
            .flex_1()
            .min_w(px(0.))
            .on_submit(move |_: &str| submit_inline_reply(ui));

        view()
            .mt_2()
            .p_2()
            .bg(INSET)
            .border_1()
            .border_color(ACCENT)
            .rounded(px(8.))
            .flex_row()
            .items_center()
            .gap_2()
            .child(input)
            .child(if sending {
                status("Sending…", Tone::Info).into_element()
            } else {
                compact_action_with_icon(Icon::MessageSquare, "Send", Variant::Primary, move |_| {
                    submit_inline_reply(ui)
                })
                .into_element()
            })
            .child(chrome_button(Icon::Close, "Cancel reply", move |_| {
                batch(|| {
                    ui.inbox.reply_target.set(None);
                    ui.inbox.reply_draft.set(String::new());
                });
            }))
            .into_element()
    })
}

/// Top 42pt unified chrome bar for the Daily Inbox route.
pub fn chrome_bar(ui: Ui) -> impl IntoElement {
    let summary = computed(move || {
        let inbox = ui.daily_inbox.get();
        (
            inbox.completed_today,
            inbox.total_today,
            inbox.deferred_items.len(),
            inbox.remaining_today,
            inbox.cutoff_minutes,
        )
    });
    let drawer = ui.inbox.drawer;

    view()
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .pl_2()
        .window_drag(true)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2p5()
                .min_w(px(0.))
                .window_drag(false)
                .child(icon_tinted(Icon::Inbox, ACCENT).w(px(15.)).h(px(15.)))
                .child(
                    label("Today")
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(TEXT),
                )
                .child(muted(format_today_date()).text_size(px(12.)))
                .child(dynamic(move || {
                    let (done, total, _, rem, _) = summary.get();
                    let ratio = if total == 0 {
                        1.0_f32
                    } else {
                        (done as f32 / total as f32).clamp(0.0, 1.0)
                    };
                    let fill_w = (ratio * 64.0).round();
                    let status_text = if total == 0 {
                        "All clear".to_owned()
                    } else if rem == 0 {
                        format!("Cleared · {done}/{total}")
                    } else {
                        format!("{done}/{total} cleared")
                    };
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            view()
                                .w(px(64.))
                                .h(px(4.))
                                .rounded_full()
                                .bg(INSET)
                                .overflow_hidden()
                                .child(view().h_full().w(px(fill_w)).bg(if rem == 0 {
                                    SUCCESS
                                } else {
                                    ACCENT
                                })),
                        )
                        .child(muted(status_text).font_family(MONO_FONT).text_size(px(11.)))
                        .into_element()
                })),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .flex_shrink_0()
                .window_drag(false)
                .child(dynamic(move || {
                    let (done, _, deferred, _, _) = summary.get();
                    let deferred_lbl = if deferred > 0 {
                        format!("Deferred {deferred}")
                    } else {
                        "Deferred".into()
                    };
                    let done_lbl = if done > 0 {
                        format!("Done {done}")
                    } else {
                        "Done".into()
                    };
                    view()
                        .flex_row()
                        .items_center()
                        .bg(INSET)
                        .p_0p5()
                        .rounded(px(7.))
                        .gap_0p5()
                        .child(compact_segment(
                            "Today",
                            None,
                            move || drawer.with(|d| d == "today"),
                            move |_| drawer.set("today".into()),
                        ))
                        .child(compact_segment(
                            deferred_lbl,
                            None,
                            move || drawer.with(|d| d == "deferred"),
                            move |_| drawer.set("deferred".into()),
                        ))
                        .child(compact_segment(
                            done_lbl,
                            None,
                            move || drawer.with(|d| d == "done"),
                            move |_| drawer.set("done".into()),
                        ))
                        .into_element()
                }))
                .child(dynamic(move || {
                    let (_, _, _, rem, _) = summary.get();
                    if rem == 0 || drawer.get() != "today" {
                        return view().into_element();
                    }
                    compact_action_with_icon(
                        Icon::Clock,
                        "Defer rest ▾",
                        Variant::Secondary,
                        move |_| defer_remaining_all(ui),
                    )
                    .into_element()
                }))
                .child(compact_action(
                    move || format!("{} ▾", format_cutoff(summary.get().4)),
                    Variant::Quiet,
                    move |_| open_cutoff_menu(ui),
                ))
                .child(chrome_button(
                    Icon::Refresh,
                    "Refresh GitHub notifications & terminal previews",
                    move |_| {
                        refresh_github_notifications(ui, true);
                        prefetch_agent_terminal_previews(ui);
                    },
                )),
        )
}

/// Calm single-line Schedule health strip with visual status dots; click to inspect today's runs.
fn render_schedule_strip(ui: Ui, runs: &[SchedulePulseItem]) -> Option<View> {
    if runs.is_empty() {
        return None;
    }
    let expanded = ui.inbox.schedule_expanded.get();
    let passed = runs
        .iter()
        .filter(|r| matches!(r.state.as_str(), "completed" | "succeeded"))
        .count();
    let failed = runs
        .iter()
        .filter(|r| matches!(r.state.as_str(), "failed" | "blocked"))
        .count();
    let running = runs
        .iter()
        .filter(|r| matches!(r.state.as_str(), "running" | "starting" | "queued"))
        .count();
    let total = runs.len();

    let mut summary_parts = Vec::new();
    if failed > 0 {
        summary_parts.push(format!("{failed} failed"));
    }
    if running > 0 {
        summary_parts.push(format!("{running} running"));
    }
    if passed > 0 {
        if failed == 0 && running == 0 {
            summary_parts.push(format!("{passed}/{total} passed"));
        } else {
            summary_parts.push(format!("{passed} passed"));
        }
    }
    if summary_parts.is_empty() {
        summary_parts.push(format!("{total} scheduled"));
    }

    let mut dot_track = view().flex_row().items_center().gap_1();
    for run in runs.iter().take(12) {
        let color = match run.state.as_str() {
            "completed" | "succeeded" => SUCCESS,
            "failed" => DANGER,
            "blocked" => ATTENTION,
            "running" | "starting" | "queued" => ACCENT,
            _ => MUTED,
        };
        dot_track = dot_track.child(view().w(px(7.)).h(px(7.)).rounded_full().bg(color));
    }

    let trigger_tint = if failed > 0 {
        DANGER
    } else if running > 0 {
        ACCENT
    } else {
        SUCCESS
    };

    let header = press_target()
        .w_full()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .px_3()
        .py_2()
        .bg(CLEAR)
        .hover(|s| s.bg(CARD_HOVER))
        .on_press(move |_: &()| {
            ui.inbox.schedule_expanded.update(|v| *v = !*v);
        })
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2p5()
                .child(
                    icon_tinted(Icon::Trigger, trigger_tint)
                        .w(px(14.))
                        .h(px(14.)),
                )
                .child(
                    label("Schedules today")
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(TEXT),
                )
                .child(dot_track)
                .child(
                    muted(summary_parts.join(" · "))
                        .text_size(px(12.))
                        .text_color(if failed > 0 { DANGER } else { MUTED }),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(muted(if expanded { "Hide" } else { "Details" }).text_size(px(11.5)))
                .child(
                    icon_tinted(
                        if expanded {
                            Icon::ChevronDown
                        } else {
                            Icon::Forward
                        },
                        MUTED,
                    )
                    .w(px(13.))
                    .h(px(13.)),
                ),
        );

    let mut container = sheet_container().child(header);
    if expanded {
        for run in runs.iter().take(12) {
            let time_str = format_clock_hm(run.scheduled_at);
            let (dot_color, state_label, state_tone) = match run.state.as_str() {
                "completed" | "succeeded" => (SUCCESS, "Passed".to_owned(), Tone::Success),
                "failed" => (DANGER, "Failed".to_owned(), Tone::Danger),
                "blocked" => (ATTENTION, "Waiting".to_owned(), Tone::Attention),
                "running" | "starting" | "queued" => (ACCENT, "Running".to_owned(), Tone::Info),
                other => (MUTED, other.to_owned(), Tone::Neutral),
            };
            let detail = run
                .error
                .clone()
                .or_else(|| run.summary.clone())
                .unwrap_or_default();
            let pid = run.project_id.clone();
            let sid = run.schedule_id.clone();
            let cid = run.card_id.clone();
            let pid_retry = run.project_id.clone();
            let sid_retry = run.schedule_id.clone();
            let is_failed = run.state == "failed";

            let mut row_left = view()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_1()
                .min_w(px(0.))
                .child(
                    muted(time_str)
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .flex_shrink_0(),
                )
                .child(status_dot(dot_color))
                .child(
                    label(run.schedule_name.clone())
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .truncate(),
                )
                .child(muted(run.project_name.clone()).text_size(px(11.5)));
            if let Some(num) = run.card_number {
                row_left = row_left.child(badge(format!("#{num}")));
            }
            if !detail.is_empty() {
                row_left = row_left.child(
                    muted(detail)
                        .text_size(px(11.5))
                        .text_color(if is_failed { DANGER } else { MUTED })
                        .truncate()
                        .flex_1()
                        .min_w(px(0.)),
                );
            }

            let mut right = view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .flex_shrink_0()
                .child(status(state_label, state_tone));
            if is_failed {
                right = right.child(compact_action_with_icon(
                    Icon::Refresh,
                    "Retry",
                    Variant::Secondary,
                    move |_| {
                        dispatch(
                            ui,
                            "schedule.trigger",
                            json!({ "project_id": pid_retry, "schedule_id": sid_retry }),
                        );
                        ui.notify("Retried scheduled run");
                    },
                ));
            }
            container = container.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(BORDER)
                    .child(
                        press_target()
                            .flex_1()
                            .min_w(px(0.))
                            .bg(CLEAR)
                            .rounded(px(4.))
                            .hover(|s| s.bg(CARD_HOVER))
                            .on_press(move |_: &()| {
                                if let Some(card_id) = &cid {
                                    ui.open_card(card_id.clone());
                                } else {
                                    crate::schedules::open(
                                        ui,
                                        Some(pid.clone()),
                                        Some(sid.clone()),
                                    );
                                }
                            })
                            .child(row_left),
                    )
                    .child(right),
            );
        }
    }
    Some(container)
}

fn render_undo_banner(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let Some(banner) = ui.inbox.undo_banner.get() else {
            return view().into_element();
        };
        view()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .bg(SELECTED)
            .border_1()
            .border_color(ACCENT)
            .rounded(px(8.))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(icon_tinted(Icon::Check, ACCENT).w(px(14.)).h(px(14.)))
                    .child(label(banner.label).text_size(px(12.5))),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .child(compact_action_with_icon(
                        Icon::Undo,
                        "Undo",
                        Variant::Primary,
                        move |_| undo_last(ui),
                    ))
                    .child(chrome_button(Icon::Close, "Dismiss", move |_| {
                        ui.inbox.undo_banner.set(None);
                    })),
            )
            .into_element()
    })
}

/// Visual-archetype row inside the `WAITING ON YOU` sheet:
/// 1. Agent asking/blocked -> branded provider icon tile + amber chat quote bubble + Primary `Reply`
/// 2. Review / Ready to Merge -> purple `Eye` or blue `GitMerge` tile + PR/CI delivery bar + Primary `Merge PR`/`Review`
/// 3. Schedule Failed -> red `TriangleAlert` tile + monospace red error console strip + Primary `Retry`
fn render_needs_you_row(ui: Ui, item: NeedsYouItem, now_ms: u64, first: bool) -> View {
    let key_defer = item.key.clone();
    let key_dismiss = item.key.clone();
    let wm = item.watermark;
    let rel = format_relative(item.timestamp_ms, now_ms);

    let row_base = |accent: ThemeColor| {
        let mut r = view().flex_row().items_stretch().min_w(px(0.));
        if !first {
            r = r.border_t_1().border_color(BORDER);
        }
        r.child(view().w(px(3.)).flex_shrink_0().bg(accent))
    };

    match item.kind {
        NeedsYouKind::Card {
            card_id,
            card_number,
            card_title,
            project_name,
            host_label,
            terminal_id,
            agent_engine,
            pr_number,
            pr_checks_summary,
            pr_ci_state,
            branch,
            headline,
            detail,
            body,
            actor,
            is_review,
            is_blocked,
            unread_seq,
            unread_count,
            can_quick_merge,
            ..
        } => {
            let reply_target = ReplyTarget::Card {
                card_id: card_id.clone(),
                label: format!("#{card_number} {card_title}"),
            };
            let reply_target_btn = reply_target.clone();
            let cid_open = card_id.clone();
            let cid_term = card_id.clone();
            let cid_merge = card_id.clone();
            let cid_dismiss = card_id.clone();
            let term_open = terminal_id.clone();
            let defer_title = format!("#{card_number} {card_title}");
            let dismiss_title = format!("#{card_number} {card_title}");

            let is_code_gate = !is_blocked && (is_review || can_quick_merge);
            let accent_bar = if is_blocked {
                ATTENTION
            } else if can_quick_merge {
                ACCENT
            } else if is_review {
                HIGHLIGHT
            } else {
                ACCENT
            };

            // Distinct 32×32 visual anchor tile
            let left_tile = if is_blocked {
                agent_icon_tile_32(agent_engine.as_deref(), ATTENTION_SURFACE, Some(ATTENTION))
            } else if can_quick_merge {
                icon_tile_32(Icon::GitMerge, SELECTED, ACCENT, None)
            } else if is_review {
                icon_tile_32(Icon::Eye, HIGHLIGHT_SURFACE, HIGHLIGHT, None)
            } else if agent_engine.is_some() {
                agent_icon_tile_32(agent_engine.as_deref(), SELECTED, None)
            } else {
                icon_tile_32(Icon::MessageSquare, SELECTED, ACCENT, None)
            };

            // Intent-first title line
            let intent_line = if is_blocked {
                match &actor {
                    Some(a) => format!("{a} is waiting for your reply"),
                    None => "Agent is waiting for your reply".into(),
                }
            } else if can_quick_merge {
                if let Some(n) = pr_number {
                    format!("Ready to merge · PR #{n}")
                } else {
                    "Ready to merge".into()
                }
            } else if is_review {
                "Review & approval requested".into()
            } else {
                headline.clone()
            };

            // Quiet context breadcrumb: `#184 Title · project`
            let mut meta_row = view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .min_w(px(0.))
                .child(
                    muted(format!("#{card_number}"))
                        .font_family(MONO_FONT)
                        .text_size(px(11.5)),
                )
                .child(
                    label(card_title.clone())
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(TEXT)
                        .truncate()
                        .flex_1()
                        .min_w(px(0.)),
                )
                .child(muted(project_name).text_size(px(11.5)).flex_shrink_0());
            if let Some(h) = host_label {
                meta_row = meta_row.child(badge(h));
            }
            if unread_count > 1 {
                meta_row = meta_row.child(badge(format!("+{unread_count}")));
            }
            if !rel.is_empty() {
                meta_row = meta_row.child(muted(rel).text_size(px(11.)).flex_shrink_0());
            }

            let excerpt = body
                .filter(|b| !b.trim().is_empty())
                .or(detail.filter(|d| !d.trim().is_empty() && d != &headline));

            // Archetype-specific content body
            let mut body_col = view().flex_1().min_w(px(0.)).gap_1p5().child(
                press_target()
                    .w_full()
                    .bg(CLEAR)
                    .rounded(px(5.))
                    .hover(|s| s.bg(CARD_HOVER))
                    .on_press(move |_: &()| ui.open_card(cid_open.clone()))
                    .child(
                        view().gap_0p5().child(meta_row).child(
                            label(intent_line)
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if is_blocked {
                                    ATTENTION
                                } else if is_review && !can_quick_merge {
                                    HIGHLIGHT
                                } else {
                                    ACCENT
                                }),
                        ),
                    ),
            );

            if is_code_gate {
                // Code delivery / PR check strip
                let mut pr_strip = view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .px_2p5()
                    .py_1p5()
                    .bg(INSET)
                    .rounded(px(6.));
                if let Some(n) = pr_number {
                    pr_strip = pr_strip.child(
                        label(format!("PR #{n}"))
                            .font_family(MONO_FONT)
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(TEXT),
                    );
                }
                if let Some(ci) = pr_ci_state.as_deref() {
                    let (ci_text, ci_tone) = match ci {
                        "success" => (
                            pr_checks_summary
                                .clone()
                                .unwrap_or_else(|| "Checks passed".into()),
                            Tone::Success,
                        ),
                        "failure" | "error" => (
                            pr_checks_summary
                                .clone()
                                .unwrap_or_else(|| "CI failed".into()),
                            Tone::Danger,
                        ),
                        _ => (
                            pr_checks_summary
                                .clone()
                                .unwrap_or_else(|| "Checks running".into()),
                            Tone::Info,
                        ),
                    };
                    pr_strip = pr_strip.child(status(ci_text, ci_tone));
                }
                if let Some(br) = branch {
                    pr_strip = pr_strip.child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .child(icon_tinted(Icon::Branch, MUTED).w(px(12.)).h(px(12.)))
                            .child(muted(br).font_family(MONO_FONT).text_size(px(11.5))),
                    );
                }
                if pr_number.is_some() || pr_ci_state.is_some() || excerpt.is_some() {
                    if let Some(ex) = excerpt {
                        pr_strip =
                            pr_strip.child(muted(ex).text_size(px(12.)).truncate().min_w(px(0.)));
                    }
                    body_col = body_col.child(pr_strip);
                }
            } else if let Some(ex) = excerpt {
                // Conversational speech quote block (high-contrast text, warm left bar when blocked)
                body_col = body_col.child(
                    view()
                        .flex_row()
                        .items_start()
                        .gap_2()
                        .px_2p5()
                        .py_2()
                        .bg(if is_blocked { ATTENTION_SURFACE } else { INSET })
                        .border_l_1()
                        .border_color(if is_blocked { ATTENTION } else { ACCENT })
                        .rounded(px(6.))
                        .child(
                            icon_tinted(
                                Icon::MessageSquare,
                                if is_blocked { ATTENTION } else { MUTED },
                            )
                            .w(px(13.))
                            .h(px(13.))
                            .mt(px(2.)),
                        )
                        .child(
                            label(ex)
                                .text_size(px(12.5))
                                .text_color(TEXT)
                                .select_text()
                                .flex_1()
                                .min_w(px(0.)),
                        ),
                );
            }

            // Left primary CTA + subtle secondary; Right fixed Defer + Done muscle memory icons
            let mut left_actions = view().flex_row().items_center().gap_1p5();
            if can_quick_merge {
                left_actions = left_actions
                    .child(compact_action_with_icon(
                        Icon::GitMerge,
                        "Merge PR",
                        Variant::Primary,
                        move |_| {
                            if let Some(card) = ui.snapshot.with_untracked(|s| {
                                s.cards.iter().find(|c| c.id == cid_merge).cloned()
                            }) {
                                crate::card::quick_merge_start(ui, card);
                            }
                        },
                    ))
                    .child(compact_action("Reply", Variant::Quiet, move |_| {
                        start_inline_reply(ui, reply_target_btn.clone())
                    }));
            } else if is_blocked {
                left_actions = left_actions.child(compact_action_with_icon(
                    Icon::MessageSquare,
                    "Reply",
                    Variant::Primary,
                    move |_| start_inline_reply(ui, reply_target_btn.clone()),
                ));
            } else if is_review {
                let cid_rev = card_id.clone();
                left_actions = left_actions
                    .child(compact_action_with_icon(
                        Icon::Eye,
                        "Review",
                        Variant::Primary,
                        move |_| ui.open_card(cid_rev.clone()),
                    ))
                    .child(compact_action("Reply", Variant::Quiet, move |_| {
                        start_inline_reply(ui, reply_target_btn.clone())
                    }));
            } else {
                left_actions = left_actions.child(compact_action_with_icon(
                    Icon::MessageSquare,
                    "Reply",
                    Variant::Secondary,
                    move |_| start_inline_reply(ui, reply_target_btn.clone()),
                ));
            }
            if let Some(tid) = term_open {
                left_actions = left_actions.child(compact_action_with_icon(
                    Icon::Terminal,
                    "Terminal ↗",
                    Variant::Quiet,
                    move |_| {
                        ui.open_card_with_terminal(cid_term.clone(), Some(tid.clone()));
                    },
                ));
            }

            let right_triage = view()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .child(chrome_button(Icon::Clock, "Defer", move |_| {
                    open_defer_menu(ui, vec![(key_defer.clone(), wm)], defer_title.clone())
                }))
                .child(chrome_button(Icon::Check, "Mark handled", move |_| {
                    if unread_seq > 0 {
                        crate::overview::mark_card_read(ui, cid_dismiss.clone(), unread_seq);
                    }
                    dismiss_items(
                        ui,
                        vec![(key_dismiss.clone(), wm)],
                        format!("Handled {dismiss_title}"),
                    );
                }));

            let action_bar = view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .pt_0p5()
                .child(left_actions)
                .child(right_triage);

            row_base(accent_bar).child(
                view()
                    .flex_1()
                    .min_w(px(0.))
                    .px_3p5()
                    .py_3()
                    .flex_row()
                    .items_start()
                    .gap_3()
                    .child(left_tile)
                    .child(
                        body_col
                            .child(action_bar)
                            .child(inline_reply_box(ui, reply_target)),
                    ),
            )
        }
        NeedsYouKind::ScheduleFailed {
            schedule_id,
            schedule_name,
            project_id,
            project_name,
            host_label,
            state,
            summary,
            error,
            terminal_id,
            ..
        } => {
            let defer_title = format!("Schedule · {schedule_name}");
            let dismiss_title = defer_title.clone();
            let pid_retry = project_id.clone();
            let sid_retry = schedule_id.clone();
            let pid_open = project_id.clone();
            let sid_open = schedule_id.clone();
            let pid_row = project_id.clone();
            let sid_row = schedule_id.clone();
            let is_failed = state == "failed";
            let accent = if is_failed { DANGER } else { ATTENTION };

            let left_tile = icon_tile_32(
                Icon::TriangleAlert,
                if is_failed {
                    DANGER_SURFACE
                } else {
                    ATTENTION_SURFACE
                },
                accent,
                None,
            );

            let mut meta_row = view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .min_w(px(0.))
                .child(
                    label(schedule_name.clone())
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(TEXT)
                        .truncate()
                        .flex_1()
                        .min_w(px(0.)),
                )
                .child(muted(project_name).text_size(px(11.5)).flex_shrink_0());
            if let Some(h) = host_label {
                meta_row = meta_row.child(badge(h));
            }
            if !rel.is_empty() {
                meta_row = meta_row.child(muted(rel).text_size(px(11.)).flex_shrink_0());
            }

            let detail_text = error
                .or(summary)
                .unwrap_or_else(|| "Scheduled run did not complete cleanly.".into());

            let mut left_actions =
                view()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .child(compact_action_with_icon(
                        Icon::Refresh,
                        "Retry",
                        Variant::Primary,
                        move |_| {
                            dispatch(
                                ui,
                                "schedule.trigger",
                                json!({ "project_id": pid_retry, "schedule_id": sid_retry }),
                            );
                            ui.notify("Retried scheduled run");
                        },
                    ));
            if let Some(tid) = terminal_id {
                left_actions = left_actions.child(compact_action_with_icon(
                    Icon::Terminal,
                    "Terminal ↗",
                    Variant::Quiet,
                    move |_| ui.open_terminal(tid.clone()),
                ));
            }
            left_actions =
                left_actions.child(compact_action("Inspect ↗", Variant::Quiet, move |_| {
                    crate::schedules::open(ui, Some(pid_open.clone()), Some(sid_open.clone()));
                }));

            let right_triage = view()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .child(chrome_button(Icon::Clock, "Defer", move |_| {
                    open_defer_menu(ui, vec![(key_defer.clone(), wm)], defer_title.clone())
                }))
                .child(chrome_button(Icon::Check, "Dismiss", move |_| {
                    dismiss_items(
                        ui,
                        vec![(key_dismiss.clone(), wm)],
                        format!("Dismissed {dismiss_title}"),
                    )
                }));

            row_base(accent).child(
                view()
                    .flex_1()
                    .min_w(px(0.))
                    .px_3p5()
                    .py_3()
                    .flex_row()
                    .items_start()
                    .gap_3()
                    .child(left_tile)
                    .child(
                        view()
                            .flex_1()
                            .min_w(px(0.))
                            .gap_1p5()
                            .child(
                                press_target()
                                    .w_full()
                                    .bg(CLEAR)
                                    .rounded(px(5.))
                                    .hover(|s| s.bg(CARD_HOVER))
                                    .on_press(move |_: &()| {
                                        crate::schedules::open(
                                            ui,
                                            Some(pid_row.clone()),
                                            Some(sid_row.clone()),
                                        );
                                    })
                                    .child(
                                        view().gap_0p5().child(meta_row).child(
                                            label(if is_failed {
                                                "Scheduled run failed"
                                            } else {
                                                "Scheduled run waiting for input"
                                            })
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(accent),
                                        ),
                                    ),
                            )
                            // Monospace error console strip
                            .child(
                                view()
                                    .px_2p5()
                                    .py_2()
                                    .bg(BACKGROUND)
                                    .border_1()
                                    .border_color(if is_failed {
                                        DANGER_SURFACE
                                    } else {
                                        ATTENTION_SURFACE
                                    })
                                    .rounded(px(6.))
                                    .child(
                                        label(detail_text)
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.5))
                                            .text_color(accent)
                                            .select_text(),
                                    ),
                            )
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .pt_0p5()
                                    .child(left_actions)
                                    .child(right_triage),
                            ),
                    ),
            )
        }
    }
}

fn render_github_stack_item(
    ui: Ui,
    item: GitHubStackItem,
    project_id: Option<String>,
    now_ms: u64,
) -> View {
    let key_str = item.key.clone();
    let key_expand = key_str.clone();
    let key_body = key_str.clone();
    let key_defer = key_str.clone();
    let wm = item.watermark;
    let rel = format_relative(item.timestamp_ms, now_ms);
    let url_open = item.url.clone();
    let item_for_load = item.clone();
    let item_for_dismiss = item.clone();
    let defer_title = item.title.clone();
    let can_reply = item.number.is_some() && matches!(item.kind.as_str(), "pr" | "issue");
    let reply_target = item.number.map(|num| ReplyTarget::GitHub {
        key: item.key.clone(),
        repository: item.repository.clone(),
        kind: item.kind.clone(),
        number: num,
        url: item.url.clone(),
        label: format!("{} #{num}", item.repository),
    });

    // Small semantic icon differentiating CI failure vs review vs mention vs synced card
    let (row_icon, row_tint) = if item.is_ci_failure {
        (Icon::TriangleAlert, DANGER)
    } else if item.is_synced_card {
        (Icon::Board, MUTED)
    } else if item.reason.to_ascii_lowercase().contains("review") {
        (Icon::Eye, HIGHLIGHT)
    } else if item.reason.to_ascii_lowercase().contains("mention") {
        (Icon::MessageSquare, ATTENTION)
    } else {
        (Icon::GitHub, MUTED)
    };

    let mut top = view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .child(icon_tinted(row_icon, row_tint).w(px(14.)).h(px(14.)))
        .child(
            label(item.title.clone())
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(12.5))
                .truncate()
                .flex_1()
                .min_w(px(0.)),
        )
        .child(
            muted(item.subtitle.clone())
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(if item.is_ci_failure { DANGER } else { MUTED }),
        );
    if let Some(a) = &item.author {
        top = top.child(muted(format!("@{a}")).text_size(px(11.5)));
    }
    if !rel.is_empty() {
        top = top.child(muted(rel).text_size(px(11.)).flex_shrink_0());
    }

    let failed_checks = item.failed_checks.clone();
    let static_body = item.body.clone();
    let body_preview = dynamic(move || {
        let expanded = ui
            .inbox
            .expanded_github_items
            .with(|s| s.contains(&key_body));
        let cached = ui
            .inbox
            .github_item_bodies
            .with(|m| m.get(&key_body).cloned());
        let loading = ui.inbox.github_item_loading.with(|s| s.contains(&key_body));

        let mut col = view().gap_1p5();
        if !failed_checks.is_empty() {
            let mut pills = view().flex_row().flex_wrap().gap_1().pt_0p5();
            for chk in &failed_checks {
                pills = pills.child(status(format!("✗ {chk}"), Tone::Danger));
            }
            col = col.child(pills);
        }
        let text_body = static_body.clone().or(cached);
        if loading && text_body.is_none() {
            return col
                .child(muted("Loading GitHub details…").text_size(px(11.5)))
                .into_element();
        }
        if let Some(body_str) = text_body.filter(|b| !b.trim().is_empty()) {
            let shown = if expanded || body_str.chars().count() <= 180 {
                body_str
            } else {
                format!("{}…", body_str.chars().take(180).collect::<String>())
            };
            col = col.child(
                view()
                    .px_2p5()
                    .py_1p5()
                    .bg(INSET)
                    .rounded(px(5.))
                    .child(muted(shown).text_size(px(12.)).select_text()),
            );
        }
        col.into_element()
    });

    let mut left_actions = view().flex_row().items_center().gap_1p5();
    if can_reply && let Some(rt) = reply_target.clone() {
        left_actions = left_actions.child(compact_action_with_icon(
            Icon::MessageSquare,
            "Reply",
            Variant::Secondary,
            move |_| start_inline_reply(ui, rt.clone()),
        ));
    }
    if item.number.is_some() && !item.is_synced_card {
        left_actions =
            left_actions.child(compact_action("Preview ▾", Variant::Quiet, move |_| {
                ui.inbox.expanded_github_items.update(|s| {
                    if !s.remove(&key_expand) {
                        s.insert(key_expand.clone());
                    }
                });
                load_github_item_body(ui, &item_for_load);
            }));
    }
    if let Some(cid) = item.card_id.clone() {
        let lbl = item
            .card_number
            .map(|n| format!("Card #{n} ↗"))
            .unwrap_or_else(|| "Open Card ↗".into());
        left_actions = left_actions.child(compact_action(lbl, Variant::Quiet, move |_| {
            ui.open_card(cid.clone());
        }));
    } else if let (Some(pid), Some(num)) = (project_id, item.number) {
        let reference = GitHubReference {
            repository: item.repository.clone(),
            kind: item.kind.clone(),
            number: Some(num),
            sha: None,
            url: item.url.clone(),
        };
        let title_for_card = item.title.clone();
        left_actions =
            left_actions.child(compact_action("Create Card", Variant::Quiet, move |_| {
                dispatch(
                    ui,
                    "github.create_card",
                    json!({
                        "project_id": pid,
                        "reference": reference,
                        "title": title_for_card,
                    }),
                );
            }));
    }
    if !url_open.is_empty() {
        left_actions =
            left_actions.child(compact_action("GitHub ↗", Variant::Quiet, move |_| {
                let _ = crate::platform::open_external(&url_open);
            }));
    }

    let right_triage = view()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .child(chrome_button(Icon::Clock, "Defer", move |_| {
            open_defer_menu(ui, vec![(key_defer.clone(), wm)], defer_title.clone())
        }))
        .child(chrome_button(Icon::Check, "Mark read", move |_| {
            dismiss_github_item(ui, item_for_dismiss.clone())
        }));

    let mut row = view()
        .pl(px(46.))
        .pr_3p5()
        .py_2p5()
        .gap_1p5()
        .border_t_1()
        .border_color(BORDER)
        .child(top)
        .child(body_preview)
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(left_actions)
                .child(right_triage),
        );
    if let Some(rt) = reply_target {
        row = row.child(inline_reply_box(ui, rt));
    }
    row
}

fn render_github_stack(ui: Ui, stack: GitHubProjectStack, now_ms: u64) -> View {
    let stack_id = stack.stack_id.clone();
    let stack_id_toggle = stack_id.clone();
    let expanded = ui.inbox.expanded_stacks.with(|s| s.contains(&stack_id));
    let count = stack.items.len();
    let rel = format_relative(stack.latest_at, now_ms);
    let defer_keys: Vec<(String, Option<u64>)> = stack
        .items
        .iter()
        .map(|i| (i.key.clone(), i.watermark))
        .collect();
    let stack_subject = format!("GitHub · {} ({count})", stack.display_name);
    let stack_for_dismiss = stack.clone();

    let tile = icon_tile_32(
        Icon::GitHub,
        if stack.has_ci_failure {
            DANGER_SURFACE
        } else {
            INSET
        },
        if stack.has_ci_failure { DANGER } else { TEXT },
        stack.has_ci_failure.then_some(DANGER),
    );

    // Visual semantic counter line when collapsed (colored dots instead of a wall of truncated text!)
    let mut breakdown = view().flex_row().flex_wrap().items_center().gap_2p5();
    let mut has_counters = false;
    if stack.ci_failed_count > 0 {
        breakdown = breakdown.child(colored_count_chip(
            DANGER,
            format!("{} CI failed", stack.ci_failed_count),
        ));
        has_counters = true;
    }
    if stack.review_count > 0 {
        breakdown = breakdown.child(colored_count_chip(
            HIGHLIGHT,
            format!(
                "{} {}",
                stack.review_count,
                if stack.review_count == 1 {
                    "review"
                } else {
                    "reviews"
                }
            ),
        ));
        has_counters = true;
    }
    if stack.mention_count > 0 {
        breakdown = breakdown.child(colored_count_chip(
            ATTENTION,
            format!(
                "{} {}",
                stack.mention_count,
                if stack.mention_count == 1 {
                    "mention"
                } else {
                    "mentions"
                }
            ),
        ));
        has_counters = true;
    }
    if stack.synced_card_count > 0 {
        breakdown = breakdown.child(colored_count_chip(
            ACCENT,
            format!("{} synced today", stack.synced_card_count),
        ));
        has_counters = true;
    }
    let accounted =
        stack.ci_failed_count + stack.review_count + stack.mention_count + stack.synced_card_count;
    if has_counters && count > accounted {
        breakdown =
            breakdown.child(muted(format!("+{} updates", count - accounted)).text_size(px(12.)));
    }

    let mut header_body = view().flex_1().min_w(px(0.)).gap_1().child(
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                label(format!("GitHub · {}", stack.display_name))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(13.))
                    .truncate(),
            )
            .child(badge(count.to_string()))
            .child(if !rel.is_empty() {
                muted(rel).text_size(px(11.)).into_element()
            } else {
                view().into_element()
            }),
    );
    if !expanded {
        if has_counters {
            header_body = header_body.child(breakdown);
        } else if !stack.preview.is_empty() {
            header_body =
                header_body.child(muted(stack.preview.clone()).text_size(px(12.)).truncate());
        }
    }

    let header = view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3p5()
        .py_2p5()
        .child(
            press_target()
                .flex_1()
                .min_w(px(0.))
                .bg(CLEAR)
                .rounded(px(6.))
                .hover(|s| s.bg(CARD_HOVER))
                .on_press(move |_: &()| toggle_stack(ui, stack_id_toggle.clone()))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2p5()
                        .child(
                            icon_tinted(
                                if expanded {
                                    Icon::ChevronDown
                                } else {
                                    Icon::Forward
                                },
                                MUTED,
                            )
                            .w(px(14.))
                            .h(px(14.)),
                        )
                        .child(tile)
                        .child(header_body),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .child(chrome_button(Icon::Clock, "Defer bundle", move |_| {
                    open_defer_menu(ui, defer_keys.clone(), stack_subject.clone())
                }))
                .child(chrome_button(Icon::Check, "Mark all read", move |_| {
                    dismiss_github_stack(ui, stack_for_dismiss.clone())
                })),
        );

    let mut box_view = sheet_container().child(header);
    if stack.has_ci_failure {
        box_view = box_view.border_color(DANGER);
    }

    if expanded {
        let pid = stack.project_id.clone();
        for item in stack.items {
            box_view = box_view.child(render_github_stack_item(ui, item, pid.clone(), now_ms));
        }
    }

    let mut wrapper = view().gap_0().child(box_view);
    if !expanded && count > 1 {
        wrapper = wrapper.child(stacked_deck_lip());
    }
    wrapper
}

fn render_schedule_stack(ui: Ui, stack: ScheduleCardStack, now_ms: u64) -> View {
    let stack_id = stack.stack_id.clone();
    let stack_id_toggle = stack_id.clone();
    let expanded = ui.inbox.expanded_stacks.with(|s| s.contains(&stack_id));
    let count = stack.cards.len();
    let rel = format_relative(stack.latest_at, now_ms);
    let preview = stack
        .cards
        .iter()
        .take(3)
        .map(|c| format!("#{} {}", c.card_number, c.card_title))
        .collect::<Vec<_>>()
        .join(" · ");
    let defer_keys: Vec<(String, Option<u64>)> = stack
        .cards
        .iter()
        .map(|c| (c.key.clone(), c.watermark))
        .collect();
    let dismiss_keys = defer_keys.clone();
    let stack_name = format!("Scheduled · {}", stack.schedule_name);
    let stack_name_dismiss = stack_name.clone();

    let tile = icon_tile_32(Icon::Trigger, SELECTED, ACCENT, None);

    let mut header_body = view().flex_1().min_w(px(0.)).gap_1().child(
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                label(stack_name.clone())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(13.))
                    .truncate(),
            )
            .child(status(
                format!(
                    "{count} {} to triage",
                    if count == 1 { "card" } else { "cards" }
                ),
                Tone::Info,
            ))
            .child(muted(stack.project_name.clone()).text_size(px(11.5)))
            .child(if !rel.is_empty() {
                muted(rel).text_size(px(11.)).into_element()
            } else {
                view().into_element()
            }),
    );
    if !expanded && !preview.is_empty() {
        header_body = header_body.child(muted(preview).text_size(px(12.)).truncate());
    }

    let header = view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3p5()
        .py_2p5()
        .child(
            press_target()
                .flex_1()
                .min_w(px(0.))
                .bg(CLEAR)
                .rounded(px(6.))
                .hover(|s| s.bg(CARD_HOVER))
                .on_press(move |_: &()| toggle_stack(ui, stack_id_toggle.clone()))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2p5()
                        .child(
                            icon_tinted(
                                if expanded {
                                    Icon::ChevronDown
                                } else {
                                    Icon::Forward
                                },
                                MUTED,
                            )
                            .w(px(14.))
                            .h(px(14.)),
                        )
                        .child(tile)
                        .child(header_body),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .child(chrome_button(Icon::Clock, "Defer bundle", move |_| {
                    open_defer_menu(ui, defer_keys.clone(), stack_name.clone())
                }))
                .child(chrome_button(Icon::Check, "Dismiss all", move |_| {
                    dismiss_items(
                        ui,
                        dismiss_keys.clone(),
                        format!("Dismissed {stack_name_dismiss}"),
                    )
                })),
        );

    let mut box_view = sheet_container().child(header);
    if expanded {
        for c in stack.cards {
            let cid_open = c.card_id.clone();
            let cid_archive = c.card_id.clone();
            let key_defer = c.key.clone();
            let key_dismiss = c.key.clone();
            let wm = c.watermark;
            let title_str = format!("#{} {}", c.card_number, c.card_title);
            let title_dismiss = title_str.clone();
            let excerpt = {
                let t = c.card_body.trim();
                if t.chars().count() > 220 {
                    format!("{}…", t.chars().take(220).collect::<String>())
                } else {
                    t.to_owned()
                }
            };
            let mut item_view = view()
                .pl(px(46.))
                .pr_3p5()
                .py_2p5()
                .gap_1p5()
                .border_t_1()
                .border_color(BORDER)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(icon_tinted(Icon::Board, MUTED).w(px(14.)).h(px(14.)))
                        .child(
                            muted(format!("#{}", c.card_number))
                                .font_family(MONO_FONT)
                                .text_size(px(11.5)),
                        )
                        .child(
                            label(c.card_title.clone())
                                .font_weight(FontWeight::MEDIUM)
                                .text_size(px(12.5))
                                .flex_1()
                                .truncate(),
                        )
                        .child(badge(c.stage.clone())),
                );
            if !excerpt.is_empty() {
                item_view = item_view.child(
                    view()
                        .px_2p5()
                        .py_1p5()
                        .bg(INSET)
                        .rounded(px(5.))
                        .child(muted(excerpt).text_size(px(12.)).select_text()),
                );
            }
            item_view = item_view.child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .child(compact_action(
                                "Keep / Open ↗",
                                Variant::Secondary,
                                move |_| {
                                    ui.open_card(cid_open.clone());
                                },
                            ))
                            .child(compact_action_with_icon(
                                Icon::Archive,
                                "Archive",
                                Variant::Quiet,
                                move |_| {
                                    if let Some(card) = ui.snapshot.with_untracked(|s| {
                                        s.cards.iter().find(|x| x.id == cid_archive).cloned()
                                    }) {
                                        dispatch(
                                            ui,
                                            "card.archive",
                                            json!({
                                                "card_id": card.id,
                                                "revision": card.revision,
                                            }),
                                        );
                                    }
                                },
                            )),
                    )
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .child(chrome_button(Icon::Clock, "Defer", move |_| {
                                open_defer_menu(
                                    ui,
                                    vec![(key_defer.clone(), wm)],
                                    title_str.clone(),
                                )
                            }))
                            .child(chrome_button(Icon::Check, "Dismiss", move |_| {
                                dismiss_items(
                                    ui,
                                    vec![(key_dismiss.clone(), wm)],
                                    format!("Dismissed {title_dismiss}"),
                                )
                            })),
                    ),
            );
            box_view = box_view.child(item_view);
        }
    }

    let mut wrapper = view().gap_0().child(box_view);
    if !expanded && count > 1 {
        wrapper = wrapper.child(stacked_deck_lip());
    }
    wrapper
}

/// Archetype 5: Unclosed Agent Terminal row with dark terminal tile + mini dark terminal viewport.
fn render_agent_terminal_row(ui: Ui, term: InboxTerminalWork, now_ms: u64, first: bool) -> View {
    let tid = term.terminal_id.clone();
    let tid_open = tid.clone();
    let tid_close = tid.clone();
    let tid_expand = tid.clone();
    let tid_preview = tid.clone();
    let card_for_open = term.card_id.clone();
    let key_defer = term.key.clone();
    let title_close = term.title.clone();
    let title_defer = term.title.clone();
    let rel = format_relative(term.timestamp_ms, now_ms);
    let (status_lbl, status_tone, is_live, status_color) = terminal_status_visual(term.status);
    let reply_target = ReplyTarget::Terminal {
        terminal_id: tid.clone(),
        label: term.title.clone(),
    };
    let reply_target_btn = reply_target.clone();

    let tile = terminal_icon_tile_32(term.agent_engine.as_deref(), Some(status_color));

    let mut header = view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .child(
            label(term.title.clone())
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(13.))
                .truncate(),
        )
        .child(if is_live {
            live_status(status_lbl, status_tone)
        } else {
            status(status_lbl, status_tone)
        });
    if let Some(proj) = &term.project_name {
        header = header.child(badge(proj.clone()));
    } else {
        header =
            header.child(muted(format!("Unattached ({})", term.host_label)).text_size(px(11.5)));
    }
    if let Some(num) = term.card_number {
        header = header.child(
            muted(format!("#{num}"))
                .font_family(MONO_FONT)
                .text_size(px(11.5)),
        );
    }
    header = header.child(view().flex_1());
    if !rel.is_empty() {
        header = header.child(muted(rel).text_size(px(11.)).flex_shrink_0());
    }

    // Working directory + branch strip
    let cwd_fallback = term.cwd.clone();
    let branch_opt = term.branch.clone();
    let tid_cwd = tid.clone();
    let location_row = dynamic(move || {
        let preview_cwd = ui
            .inbox
            .terminal_previews
            .with(|m| m.get(&tid_cwd).and_then(|p| p.cwd.clone()));
        let cwd_text = preview_cwd
            .or_else(|| cwd_fallback.clone())
            .map(|p| shorten_path(&p))
            .unwrap_or_else(|| "(unknown directory)".into());
        let mut line = view().flex_row().flex_wrap().items_center().gap_2().child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(icon_tinted(Icon::Folder, MUTED).w(px(12.)).h(px(12.)))
                .child(muted(cwd_text).font_family(MONO_FONT).text_size(px(11.5))),
        );
        if let Some(br) = &branch_opt {
            line = line.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(icon_tinted(Icon::Branch, MUTED).w(px(12.)).h(px(12.)))
                    .child(muted(br.clone()).font_family(MONO_FONT).text_size(px(11.5))),
            );
        }
        line.into_element()
    });

    // Mini dark terminal output viewport
    let status_detail = term.status_detail.clone();
    let shell_cmd = term.shell_command.clone();
    let shell_exit = term.shell_exit_code;
    let output_viewport = dynamic(move || {
        let expanded = ui
            .inbox
            .expanded_terminals
            .with(|s| s.contains(&tid_preview));
        let preview = ui
            .inbox
            .terminal_previews
            .with(|m| m.get(&tid_preview).cloned());

        let mut lines: Vec<String> = Vec::new();
        if let Some(p) = &preview {
            if p.loading && p.lines.is_empty() {
                return view()
                    .px_2p5()
                    .py_2()
                    .bg(BACKGROUND)
                    .border_1()
                    .border_color(BORDER)
                    .rounded(px(6.))
                    .child(
                        muted("Reading terminal output…")
                            .font_family(MONO_FONT)
                            .text_size(px(11.5)),
                    )
                    .into_element();
            }
            let take_n = if expanded { 14 } else { 2 };
            let start = p.lines.len().saturating_sub(take_n);
            lines.extend(p.lines[start..].iter().cloned());
        }
        if lines.is_empty() {
            if let Some(d) = &status_detail {
                lines.push(d.clone());
            } else if let Some(cmd) = &shell_cmd {
                let suffix = shell_exit
                    .map(|c| format!(" (exit {c})"))
                    .unwrap_or_default();
                lines.push(format!("{cmd}{suffix}"));
            }
        }
        if lines.is_empty() {
            return view().into_element();
        }
        let mut box_out = view()
            .px_2p5()
            .py_2()
            .bg(BACKGROUND)
            .border_1()
            .border_color(if term.status == HerdrStatus::Blocked {
                ATTENTION
            } else {
                BORDER
            })
            .rounded(px(6.))
            .gap_0p5();
        for (idx, ln) in lines.into_iter().enumerate() {
            let prefix = if idx == 0 { "❯ " } else { "  " };
            box_out = box_out.child(
                label(format!("{prefix}{ln}"))
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .text_color(if term.status == HerdrStatus::Blocked {
                        TEXT
                    } else {
                        MUTED
                    })
                    .select_text()
                    .truncate(),
            );
        }
        box_out.into_element()
    });

    let left_actions = view()
        .flex_row()
        .items_center()
        .gap_1p5()
        .child(compact_action_with_icon(
            Icon::MessageSquare,
            "Send",
            if term.status == HerdrStatus::Blocked {
                Variant::Primary
            } else {
                Variant::Secondary
            },
            move |_| start_inline_reply(ui, reply_target_btn.clone()),
        ))
        .child(compact_action("Output ▾", Variant::Quiet, move |_| {
            let now_expanded = ui
                .inbox
                .expanded_terminals
                .with_untracked(|s| !s.contains(&tid_expand));
            ui.inbox.expanded_terminals.update(|s| {
                if !s.remove(&tid_expand) {
                    s.insert(tid_expand.clone());
                }
            });
            if now_expanded {
                fetch_terminal_preview(ui, tid_expand.clone(), 24);
            }
        }))
        .child(compact_action("Terminal ↗", Variant::Quiet, move |_| {
            if let Some(cid) = &card_for_open {
                ui.open_card_with_terminal(cid.clone(), Some(tid_open.clone()));
            } else {
                ui.open_terminal(tid_open.clone());
            }
        }))
        .child(compact_action_with_icon(
            Icon::Close,
            "Close",
            Variant::Quiet,
            move |_| close_terminal_from_inbox(ui, tid_close.clone(), title_close.clone()),
        ));

    let right_triage = chrome_button(Icon::Clock, "Defer", move |_| {
        open_defer_menu(ui, vec![(key_defer.clone(), None)], title_defer.clone())
    });

    let mut row = view().px_3p5().py_3().flex_row().items_start().gap_3();
    if !first {
        row = row.border_t_1().border_color(BORDER);
    }
    row.child(tile).child(
        view()
            .flex_1()
            .min_w(px(0.))
            .gap_1p5()
            .child(header)
            .child(location_row)
            .child(output_viewport)
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .pt_0p5()
                    .child(left_actions)
                    .child(right_triage),
            )
            .child(inline_reply_box(ui, reply_target)),
    )
}

/// Archetype 6: Lightweight flat row for Cards you or your agents genuinely worked on today.
fn render_open_card_row(ui: Ui, card: InboxCardWork, now_ms: u64, first: bool) -> View {
    let rel = format_relative(card.last_activity_at, now_ms);
    let cid_open = card.card_id.clone();
    let cid_resolve = card.card_id.clone();
    let key_defer = card.key.clone();
    let key_dismiss = card.key.clone();
    let wm = card.watermark;
    let label_title = format!("#{} {}", card.card_number, card.card_title);
    let label_dismiss = label_title.clone();

    let mut left = view()
        .flex_row()
        .items_center()
        .gap_2()
        .flex_1()
        .min_w(px(0.))
        .child(
            icon_tinted(
                Icon::Board,
                if card.has_live_agent { ACCENT } else { MUTED },
            )
            .w(px(15.))
            .h(px(15.)),
        )
        .child(
            muted(format!("#{}", card.card_number))
                .font_family(MONO_FONT)
                .text_size(px(11.5)),
        )
        .child(
            label(card.card_title.clone())
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(12.5))
                .truncate()
                .flex_1()
                .min_w(px(0.)),
        )
        .child(badge(card.stage.clone()));
    if let Some(pr) = card.pr_summary {
        let tone = match card.pr_ci_state.as_deref() {
            Some("failure" | "error") => Tone::Danger,
            Some("success") => Tone::Success,
            _ => Tone::Neutral,
        };
        left = left.child(status(pr, tone));
    }
    if card.pinned_today {
        left = left.child(status("Pinned", Tone::Info));
    }
    if let Some(h) = card.host_label {
        left = left.child(badge(h));
    }
    if !rel.is_empty() {
        left = left.child(muted(rel).text_size(px(11.)).flex_shrink_0());
    }

    let right = view()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .child(compact_action("Resolve", Variant::Quiet, move |_| {
            if let Some(c) = ui
                .snapshot
                .with_untracked(|s| s.cards.iter().find(|x| x.id == cid_resolve).cloned())
            {
                dispatch(
                    ui,
                    "card.resolve",
                    json!({ "card_id": c.id, "revision": c.revision }),
                );
                ui.notify(format!("Resolved #{}", c.number));
            }
        }))
        .child(chrome_button(Icon::Clock, "Defer", move |_| {
            open_defer_menu(ui, vec![(key_defer.clone(), wm)], label_title.clone())
        }))
        .child(chrome_button(Icon::Check, "Remove from Today", move |_| {
            dismiss_items(
                ui,
                vec![(key_dismiss.clone(), wm)],
                format!("Removed from Today · {label_dismiss}"),
            )
        }));

    let mut row = view().flex_row().items_center().gap_2().px_3p5().py_2();
    if !first {
        row = row.border_t_1().border_color(BORDER);
    }
    row.child(
        press_target()
            .flex_1()
            .min_w(px(0.))
            .bg(CLEAR)
            .py_0p5()
            .px_1()
            .rounded(px(5.))
            .hover(|s| s.bg(CARD_HOVER))
            .on_press(move |_: &()| ui.open_card(cid_open.clone()))
            .child(left),
    )
    .child(right)
}

fn render_plain_shells_section(
    ui: Ui,
    shells: Vec<InboxTerminalWork>,
    now_ms: u64,
    first: bool,
) -> Option<View> {
    if shells.is_empty() {
        return None;
    }
    let stack_id = "plain-shells-stack".to_owned();
    let expanded = ui.inbox.expanded_stacks.with(|s| s.contains(&stack_id));
    let count = shells.len();
    let defer_keys: Vec<(String, Option<u64>)> =
        shells.iter().map(|s| (s.key.clone(), None)).collect();
    let shells_close_all = shells.clone();
    let preview = shells
        .iter()
        .take(3)
        .map(|s| {
            let dir = s
                .cwd
                .as_deref()
                .map(shorten_path)
                .unwrap_or_else(|| s.host_label.clone());
            match &s.project_name {
                Some(p) => format!("{p} ({dir})"),
                None => dir,
            }
        })
        .collect::<Vec<_>>()
        .join(" · ");

    let tile = terminal_icon_tile_32(None, None);

    let mut header_body = view().flex_1().min_w(px(0.)).gap_0p5().child(
        view().flex_row().items_center().gap_2().child(
            label(format!(
                "{count} plain {} open",
                if count == 1 { "shell" } else { "shells" }
            ))
            .font_weight(FontWeight::MEDIUM)
            .text_size(px(12.5)),
        ),
    );
    if !expanded && !preview.is_empty() {
        header_body = header_body.child(
            muted(preview)
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .truncate(),
        );
    }

    let mut container = view();
    if !first {
        container = container.border_t_1().border_color(BORDER);
    }
    container = container.child(
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3p5()
            .py_2p5()
            .child(
                press_target()
                    .flex_1()
                    .min_w(px(0.))
                    .bg(CLEAR)
                    .rounded(px(6.))
                    .hover(|s| s.bg(CARD_HOVER))
                    .on_press(move |_: &()| toggle_stack(ui, stack_id.clone()))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2p5()
                            .child(
                                icon_tinted(
                                    if expanded {
                                        Icon::ChevronDown
                                    } else {
                                        Icon::Forward
                                    },
                                    MUTED,
                                )
                                .w(px(14.))
                                .h(px(14.)),
                            )
                            .child(tile)
                            .child(header_body),
                    ),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .flex_shrink_0()
                    .child(compact_action("Close all", Variant::Quiet, move |_| {
                        close_all_plain_shells(ui, shells_close_all.clone())
                    }))
                    .child(chrome_button(Icon::Clock, "Defer shells", move |_| {
                        open_defer_menu(ui, defer_keys.clone(), format!("{count} plain shells"))
                    })),
            ),
    );

    if expanded {
        for sh in shells {
            let tid_open = sh.terminal_id.clone();
            let tid_close = sh.terminal_id.clone();
            let title_close = sh.title.clone();
            let rel = format_relative(sh.timestamp_ms, now_ms);
            let cwd_str = sh
                .cwd
                .as_deref()
                .map(shorten_path)
                .unwrap_or_else(|| "(unknown directory)".into());
            let mut left = view()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_1()
                .min_w(px(0.))
                .child(
                    label(sh.title.clone())
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM),
                )
                .child(
                    muted(cwd_str)
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .truncate(),
                );
            if let Some(proj) = &sh.project_name {
                left = left.child(badge(proj.clone()));
            }
            if let Some(cmd) = &sh.shell_command {
                left = left.child(
                    muted(cmd.clone())
                        .font_family(MONO_FONT)
                        .text_size(px(11.5)),
                );
            }
            if !rel.is_empty() {
                left = left.child(muted(rel).text_size(px(11.)).flex_shrink_0());
            }
            container = container.child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .pl(px(46.))
                    .pr_3p5()
                    .py_2()
                    .border_t_1()
                    .border_color(BORDER)
                    .child(
                        press_target()
                            .flex_1()
                            .min_w(px(0.))
                            .bg(CLEAR)
                            .rounded(px(4.))
                            .hover(|s| s.bg(CARD_HOVER))
                            .on_press(move |_: &()| ui.open_terminal(tid_open.clone()))
                            .child(left),
                    )
                    .child(chrome_button(Icon::Close, "Close shell", move |_| {
                        close_terminal_from_inbox(ui, tid_close.clone(), title_close.clone())
                    })),
            );
        }
    }
    Some(container)
}

/// Unified quiet bottom fold for older GitHub notifications (< today) & carry-over cards.
fn render_earlier_and_carryover(
    ui: Ui,
    earlier_github: Vec<GitHubStackItem>,
    carryover: Vec<InboxCardWork>,
    day_start_ms: u64,
    now_ms: u64,
) -> Option<View> {
    if earlier_github.is_empty() && carryover.is_empty() {
        return None;
    }
    let stack_id = "earlier-carryover-stack".to_owned();
    let expanded = ui.inbox.expanded_stacks.with(|s| s.contains(&stack_id));
    let gh_count = earlier_github.len();
    let card_count = carryover.len();

    let mut summary_parts = Vec::new();
    if gh_count > 0 {
        summary_parts.push(format!("{gh_count} older GitHub"));
    }
    if card_count > 0 {
        summary_parts.push(format!(
            "{card_count} previous {}",
            if card_count == 1 { "card" } else { "cards" }
        ));
    }

    let gh_for_clear_all = earlier_github.clone();
    let header = view()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .px_3p5()
        .py_2()
        .child(
            press_target()
                .flex_1()
                .min_w(px(0.))
                .bg(CLEAR)
                .rounded(px(5.))
                .hover(|s| s.bg(CARD_HOVER))
                .on_press(move |_: &()| toggle_stack(ui, stack_id.clone()))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            icon_tinted(
                                if expanded {
                                    Icon::ChevronDown
                                } else {
                                    Icon::Forward
                                },
                                MUTED,
                            )
                            .w(px(14.))
                            .h(px(14.)),
                        )
                        .child(
                            label("Earlier & Carry-over")
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(MUTED),
                        )
                        .child(muted(summary_parts.join(" · ")).text_size(px(11.5))),
                ),
        )
        .child(if gh_count > 0 {
            compact_action("Mark older GitHub read ✓", Variant::Quiet, move |_| {
                let mut keys = Vec::new();
                for item in &gh_for_clear_all {
                    if let Some(tid) = &item.thread_id
                        && crate::backend::is_connected()
                    {
                        dispatch_complete(
                            ui,
                            "github.notification.done",
                            json!({ "thread_id": tid }),
                            |_| {},
                        );
                    }
                    if let Some(cid) = &item.card_id
                        && item.read_sequence > 0
                    {
                        crate::overview::mark_card_read(ui, cid.clone(), item.read_sequence);
                    }
                    keys.push((item.key.clone(), item.watermark));
                }
                let n = keys.len();
                dismiss_items(ui, keys, format!("Cleared {n} older GitHub items"));
            })
            .into_element()
        } else {
            view().into_element()
        });

    let mut box_view = view()
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .rounded(px(10.))
        .overflow_hidden()
        .child(header);

    if expanded {
        for item in earlier_github.into_iter().take(20) {
            let rel = format_relative(item.timestamp_ms, now_ms);
            let url = item.url.clone();
            let cid_opt = item.card_id.clone();
            let key_pin = item.key.clone();
            let wm = item.watermark;
            let item_dismiss = item.clone();

            box_view = box_view.child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3p5()
                    .py_2()
                    .border_t_1()
                    .border_color(BORDER)
                    .bg(SURFACE)
                    .child(
                        press_target()
                            .flex_1()
                            .min_w(px(0.))
                            .bg(CLEAR)
                            .rounded(px(4.))
                            .hover(|s| s.bg(CARD_HOVER))
                            .on_press(move |_: &()| {
                                if let Some(cid) = &cid_opt {
                                    ui.open_card(cid.clone());
                                } else if !url.is_empty() {
                                    let _ = crate::platform::open_external(&url);
                                }
                            })
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .min_w(px(0.))
                                    .child(icon_tinted(Icon::GitHub, MUTED).w(px(14.)).h(px(14.)))
                                    .child(badge(item.repository.clone()))
                                    .child(
                                        label(item.title.clone())
                                            .text_size(px(12.5))
                                            .truncate()
                                            .flex_1()
                                            .min_w(px(0.)),
                                    )
                                    .child(muted(item.subtitle.clone()).text_size(px(11.5)))
                                    .child(muted(rel).text_size(px(11.)).flex_shrink_0()),
                            ),
                    )
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(compact_action("Add to Today", Variant::Quiet, move |_| {
                                pin_to_today(ui, key_pin.clone(), day_start_ms, wm)
                            }))
                            .child(chrome_button(Icon::Check, "Mark read", move |_| {
                                dismiss_github_item(ui, item_dismiss.clone())
                            })),
                    ),
            );
        }
        for c in carryover.into_iter().take(20) {
            let rel = format_relative(c.last_activity_at, now_ms);
            let cid = c.card_id.clone();
            let key_pin = c.key.clone();
            let wm = c.watermark;
            box_view = box_view.child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3p5()
                    .py_2()
                    .border_t_1()
                    .border_color(BORDER)
                    .bg(SURFACE)
                    .child(
                        press_target()
                            .flex_1()
                            .min_w(px(0.))
                            .bg(CLEAR)
                            .rounded(px(4.))
                            .hover(|s| s.bg(CARD_HOVER))
                            .on_press(move |_: &()| ui.open_card(cid.clone()))
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .min_w(px(0.))
                                    .child(icon_tinted(Icon::Board, MUTED).w(px(14.)).h(px(14.)))
                                    .child(badge(c.project_name.clone()))
                                    .child(
                                        muted(format!("#{}", c.card_number))
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.5)),
                                    )
                                    .child(
                                        label(c.card_title.clone())
                                            .text_size(px(12.5))
                                            .truncate()
                                            .flex_1()
                                            .min_w(px(0.)),
                                    )
                                    .child(badge(c.stage.clone()))
                                    .child(muted(rel).text_size(px(11.)).flex_shrink_0()),
                            ),
                    )
                    .child(compact_action(
                        "Add to Today",
                        Variant::Secondary,
                        move |_| pin_to_today(ui, key_pin.clone(), day_start_ms, wm),
                    )),
            );
        }
    }
    Some(box_view)
}

fn render_inbox_zero(ui: Ui, inbox: &DailyInbox) -> View {
    let done = inbox.completed_today;
    let deferred = inbox.deferred_items.len();
    let cutoff = format_cutoff(inbox.cutoff_minutes);
    let mut actions = view().flex_row().items_center().gap_2().pt_2();
    if deferred > 0 {
        actions = actions.child(action(
            format!("View deferred ({deferred})"),
            Variant::Secondary,
            move |_| ui.inbox.drawer.set("deferred".into()),
        ));
    }
    actions = actions.child(action("Open Board", Variant::Quiet, move |_| {
        ui.navigate("board");
    }));

    sheet_container()
        .py(px(48.))
        .px_6()
        .items_center()
        .gap_2()
        .child(icon_tile_32(Icon::Check, SUCCESS_SURFACE, SUCCESS, None))
        .child(
            label("All clear for today")
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(15.)),
        )
        .child(
            muted(format!(
                "{done} completed · {deferred} deferred · Next cutoff at {cutoff}"
            ))
            .text_size(px(12.5)),
        )
        .child(actions)
}

fn render_deferred_drawer(ui: Ui, items: Vec<DeferredInboxItem>) -> View {
    if items.is_empty() {
        return sheet_container()
            .py(px(44.))
            .items_center()
            .gap_2()
            .child(muted("No deferred items."))
            .child(action("Back to Today", Variant::Secondary, move |_| {
                ui.inbox.drawer.set("today".into())
            }));
    }
    let mut list = sheet_container();
    for (idx, item) in items.into_iter().enumerate() {
        let key_restore = item.key.clone();
        let cid_opt = item.card_id.clone();
        let tid_opt = item.terminal_id.clone();
        let url_opt = item.url.clone();
        let mut row = view()
            .px_3p5()
            .py_2p5()
            .flex_row()
            .items_center()
            .gap_2()
            .child(badge(item.kind_label))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.))
                    .gap_0p5()
                    .child(
                        label(item.title.clone())
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(px(12.5))
                            .truncate(),
                    )
                    .child(muted(item.context.clone()).text_size(px(11.5)).truncate()),
            )
            .child(status(
                format!("Until {}", format_defer_until(item.defer_until)),
                Tone::Neutral,
            ))
            .child(compact_action("Restore", Variant::Secondary, move |_| {
                restore_item(ui, key_restore.clone())
            }));
        if idx > 0 {
            row = row.border_t_1().border_color(BORDER);
        }
        if let Some(cid) = cid_opt {
            row = row.child(compact_action("Open ↗", Variant::Quiet, move |_| {
                ui.open_card(cid.clone())
            }));
        } else if let Some(tid) = tid_opt {
            row = row.child(compact_action("Terminal ↗", Variant::Quiet, move |_| {
                ui.open_terminal(tid.clone())
            }));
        } else if let Some(u) = url_opt {
            row = row.child(compact_action("GitHub ↗", Variant::Quiet, move |_| {
                let _ = crate::platform::open_external(&u);
            }));
        }
        list = list.child(row);
    }
    list
}

fn render_done_drawer(ui: Ui, items: Vec<DoneInboxItem>, now_ms: u64) -> View {
    if items.is_empty() {
        return sheet_container()
            .py(px(44.))
            .items_center()
            .gap_2()
            .child(muted("Nothing completed yet today."))
            .child(action("Back to Today", Variant::Secondary, move |_| {
                ui.inbox.drawer.set("today".into())
            }));
    }
    let mut list = sheet_container();
    for (idx, item) in items.into_iter().enumerate() {
        let rel = format_relative(item.completed_at, now_ms);
        let key_restore = item.key.clone();
        let cid_opt = item.card_id.clone();
        let mut row = view()
            .px_3p5()
            .py_2p5()
            .flex_row()
            .items_center()
            .gap_2()
            .child(icon_tinted(Icon::Check, SUCCESS).w(px(14.)).h(px(14.)))
            .child(badge(item.kind_label))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.))
                    .gap_0p5()
                    .child(label(item.title.clone()).text_size(px(12.5)).truncate())
                    .child(muted(item.context.clone()).text_size(px(11.5)).truncate()),
            )
            .child(muted(rel).text_size(px(11.)).flex_shrink_0());
        if idx > 0 {
            row = row.border_t_1().border_color(BORDER);
        }
        if item.can_restore {
            row = row.child(compact_action("Restore", Variant::Quiet, move |_| {
                restore_item(ui, key_restore.clone())
            }));
        }
        if let Some(cid) = cid_opt {
            row = row.child(compact_action("Open ↗", Variant::Quiet, move |_| {
                ui.open_card(cid.clone())
            }));
        }
        list = list.child(row);
    }
    list
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let gh_connected = computed(move || {
        ui.github
            .gh_status
            .with(|s| s["state"].as_str() == Some("connected"))
    });

    view()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(
            scroll().flex_1().min_h(px(0.)).child(
                view()
                    .w_full()
                    .items_center()
                    .px_5()
                    .py_4()
                    .child(
                        view()
                            .w_full()
                            .max_w(px(880.))
                            .gap_4()
                            .child(render_undo_banner(ui))
                            .child(dynamic(move || {
                        let inbox = ui.daily_inbox.get();
                        let active_drawer = ui.inbox.drawer.get();
                        let now_ms = canban_types::now();

                        if active_drawer == "deferred" {
                            return render_deferred_drawer(ui, inbox.deferred_items.clone())
                                .into_element();
                        }
                        if active_drawer == "done" {
                            return render_done_drawer(ui, inbox.done_today_items.clone(), now_ms)
                                .into_element();
                        }

                        let mut col = view().gap_4();

                        // 1. Calm single-line Schedule health strip
                        if let Some(strip) = render_schedule_strip(ui, &inbox.schedule_pulse) {
                            col = col.child(strip);
                        }

                        // Subtle GitHub connect hint when gh CLI isn't configured yet
                        if !gh_connected.get()
                            && inbox.github_stacks.is_empty()
                            && inbox.earlier_github.is_empty()
                        {
                            col = col.child(
                                view()
                                    .px_3p5()
                                    .py_2()
                                    .bg(SURFACE)
                                    .border_1()
                                    .border_color(BORDER)
                                    .rounded(px(8.))
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                icon_tinted(Icon::GitHub, MUTED)
                                                    .w(px(14.))
                                                    .h(px(14.)),
                                            )
                                            .child(
                                                muted("Connect GitHub CLI (gh) to auto-stack PR & issue notifications by project here.")
                                                    .text_size(px(12.)),
                                            ),
                                    )
                                    .child(compact_action(
                                        "GitHub Account →",
                                        Variant::Quiet,
                                        move |_| ui.navigate("github-account"),
                                    )),
                            );
                        }

                        let all_today_empty = inbox.needs_you.is_empty()
                            && inbox.github_stacks.is_empty()
                            && inbox.schedule_stacks.is_empty()
                            && inbox.open_cards.is_empty()
                            && inbox.agent_terminals.is_empty()
                            && inbox.plain_shells.is_empty();

                        if all_today_empty {
                            col = col.child(render_inbox_zero(ui, &inbox));
                        } else {
                            // 2. WAITING ON YOU (unified sheet, visual-archetype rows)
                            if !inbox.needs_you.is_empty() {
                                let count = inbox.needs_you.len();
                                let defer_keys: Vec<(String, Option<u64>)> = inbox
                                    .needs_you
                                    .iter()
                                    .map(|i| (i.key.clone(), i.watermark))
                                    .collect();
                                col = col.child(section_heading(
                                    "WAITING ON YOU",
                                    count,
                                    Some(
                                        compact_action("Defer all ▾", Variant::Quiet, move |_| {
                                            open_defer_menu(
                                                ui,
                                                defer_keys.clone(),
                                                format!("Waiting on You ({count})"),
                                            )
                                        })
                                        .into_element(),
                                    ),
                                ));
                                let mut sheet = sheet_container();
                                for (idx, item) in inbox.needs_you.iter().cloned().enumerate() {
                                    sheet =
                                        sheet.child(render_needs_you_row(ui, item, now_ms, idx == 0));
                                }
                                col = col.child(sheet);
                            }

                            // 3. BUNDLES (GitHub by project including today's synced cards + Scheduled new cards)
                            let stack_count =
                                inbox.github_stacks.len() + inbox.schedule_stacks.len();
                            if stack_count > 0 {
                                col = col.child(section_heading("BUNDLES", stack_count, None));
                                let mut stacks_col = view().gap_2p5();
                                for stack in inbox.github_stacks.iter().cloned() {
                                    stacks_col =
                                        stacks_col.child(render_github_stack(ui, stack, now_ms));
                                }
                                for stack in inbox.schedule_stacks.iter().cloned() {
                                    stacks_col =
                                        stacks_col.child(render_schedule_stack(ui, stack, now_ms));
                                }
                                col = col.child(stacks_col);
                            }

                            // 4. OPEN TODAY (unclosed Agent Terminals + Cards worked on today grouped by project + Plain Shells)
                            let open_work_count = inbox.agent_terminals.len()
                                + inbox.open_cards.len()
                                + usize::from(!inbox.plain_shells.is_empty());
                            if open_work_count > 0 {
                                let mut group_keys: Vec<(String, Option<u64>)> = Vec::new();
                                for t in &inbox.agent_terminals {
                                    group_keys.push((t.key.clone(), None));
                                }
                                for c in &inbox.open_cards {
                                    group_keys.push((c.key.clone(), c.watermark));
                                }
                                for s in &inbox.plain_shells {
                                    group_keys.push((s.key.clone(), None));
                                }
                                col = col.child(section_heading(
                                    "OPEN TODAY",
                                    open_work_count,
                                    Some(
                                        compact_action("Defer all ▾", Variant::Quiet, move |_| {
                                            open_defer_menu(
                                                ui,
                                                group_keys.clone(),
                                                format!("Open Today ({open_work_count})"),
                                            )
                                        })
                                        .into_element(),
                                    ),
                                ));

                                let mut open_sheet = sheet_container();
                                let mut row_index = 0usize;

                                for term in inbox.agent_terminals.iter().cloned() {
                                    open_sheet = open_sheet.child(render_agent_terminal_row(
                                        ui,
                                        term,
                                        now_ms,
                                        row_index == 0,
                                    ));
                                    row_index += 1;
                                }

                                // Group today's open cards cleanly by project when multiple projects are present.
                                let distinct_projects: std::collections::BTreeSet<&str> = inbox
                                    .open_cards
                                    .iter()
                                    .map(|c| c.project_name.as_str())
                                    .collect();
                                let show_project_headers = distinct_projects.len() > 1;
                                let mut current_proj = String::new();

                                for card in inbox.open_cards.iter().cloned() {
                                    if show_project_headers && card.project_name != current_proj {
                                        current_proj = card.project_name.clone();
                                        let mut hdr = view()
                                            .px_3p5()
                                            .py_1p5()
                                            .bg(INSET)
                                            .child(
                                                label(current_proj.clone())
                                                    .text_size(px(11.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(MUTED),
                                            );
                                        if row_index > 0 {
                                            hdr = hdr.border_t_1().border_color(BORDER);
                                        }
                                        open_sheet = open_sheet.child(hdr);
                                        row_index += 1;
                                    }
                                    open_sheet = open_sheet.child(render_open_card_row(
                                        ui,
                                        card,
                                        now_ms,
                                        row_index == 0,
                                    ));
                                    row_index += 1;
                                }

                                if let Some(shells_sec) = render_plain_shells_section(
                                    ui,
                                    inbox.plain_shells.clone(),
                                    now_ms,
                                    row_index == 0,
                                ) {
                                    open_sheet = open_sheet.child(shells_sec);
                                }

                                col = col.child(open_sheet);
                            }
                        }

                        // 5. Quiet collapsed Earlier & Carry-over fold
                        if let Some(earlier) = render_earlier_and_carryover(
                            ui,
                            inbox.earlier_github.clone(),
                            inbox.carryover_cards.clone(),
                            inbox.day_start_ms,
                            now_ms,
                        ) {
                            col = col.child(earlier);
                        }

                        col.into_element()
                    })),
                ),
            ),
        )
}
