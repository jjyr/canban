//! Daily Briefing: flat editorial engineering report on `BACKGROUND` canvas with
//! borderless 4-column hairline KPI strip, 1:1 `Needs Action` <-> rail-badge parity,
//! tabular Scheduled Runs, toggleable Markdown GitHub PR reader with avatars, CI & comments,
//! and structured Agent Terminal telemetry (zero nested borders, zero explanatory copy).
pub mod state;
pub use state::{
    GitHubItemDetail, State, cutoff_minutes_from_utility, defer_remaining_all, init,
    local_tz_offset_secs,
};

use crate::presenter::*;
use canban_notifications::{
    DailyInbox, DeferredInboxItem, DoneInboxItem, GitHubProjectStack, GitHubStackItem,
    InboxCardWork, InboxTerminalWork, NeedsYouItem, NeedsYouKind, ScheduleCardStack,
    SchedulePulseItem,
};
use canban_types::HerdrStatus;
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

fn format_today_date_long() -> String {
    chrono::Local::now().format("%A, %B %-d").to_string()
}

fn format_duration_short(since_ms: u64, now_ms: u64) -> String {
    if since_ms == 0 || now_ms <= since_ms {
        return String::new();
    }
    let s = (now_ms - since_ms) / 1000;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

fn format_age_compact(ts_ms: u64, now_ms: u64) -> String {
    if ts_ms == 0 {
        return String::new();
    }
    let s = now_ms.saturating_sub(ts_ms) / 1000;
    if s < 60 {
        "now".into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// Borderless circular GitHub avatar with 2-letter initials fallback.
fn github_avatar(login: &str, avatar_url: Option<&str>, size: f32) -> View {
    let short = login
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let src = avatar_url
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("https://github.com/{login}.png?size=64"));
    view()
        .relative()
        .flex_shrink_0()
        .w(px(size))
        .h(px(size))
        .rounded_full()
        .overflow_hidden()
        .bg(INSET)
        .items_center()
        .justify_center()
        .tooltip(hint_tooltip(format!("@{login}")))
        .child(
            text(if short.is_empty() { "GH".into() } else { short })
                .text_size(px((size * 0.48).max(8.)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(MUTED),
        )
        .child(
            image(src)
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .w(px(size))
                .h(px(size)),
        )
}

/// Compact inline CI indicator (`✓ 12/12`, `✗ 2`, `● 3/12`).
fn ci_indicator(
    state: Option<&str>,
    passed: u32,
    total: u32,
    failed_count: usize,
) -> Option<AnyElement> {
    let st = state?;
    match st.to_ascii_lowercase().as_str() {
        "success" => {
            let mut row = view()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .child(icon_tinted(Icon::Check, SUCCESS).w(px(12.)).h(px(12.)));
            if total > 0 {
                row = row.child(
                    label(format!("{passed}/{total}"))
                        .font_family(MONO_FONT)
                        .text_size(px(11.))
                        .text_color(SUCCESS),
                );
            }
            Some(row.into_element())
        }
        "failure" | "error" => {
            let txt = if failed_count > 0 && total > 0 {
                format!("{failed_count}/{total}")
            } else if failed_count > 0 {
                format!("{failed_count}")
            } else if total > 0 {
                format!("{passed}/{total}")
            } else {
                "CI".to_owned()
            };
            Some(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .child(
                        icon_tinted(Icon::TriangleAlert, DANGER)
                            .w(px(12.))
                            .h(px(12.)),
                    )
                    .child(
                        label(txt)
                            .font_family(MONO_FONT)
                            .text_size(px(11.))
                            .text_color(DANGER),
                    )
                    .into_element(),
            )
        }
        "running" | "pending" => {
            let mut row = view()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .child(status_dot(ACCENT));
            if total > 0 {
                row = row.child(
                    label(format!("{passed}/{total}"))
                        .font_family(MONO_FONT)
                        .text_size(px(11.))
                        .text_color(ACCENT),
                );
            }
            Some(row.into_element())
        }
        _ => None,
    }
}

/// Compact comment icon + count indicator (`💬 4`).
fn comments_indicator(count: u32) -> View {
    view()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .child(
            icon_tinted(Icon::MessageSquare, MUTED)
                .w(px(12.))
                .h(px(12.)),
        )
        .child(
            muted(count.to_string())
                .font_family(MONO_FONT)
                .text_size(px(11.)),
        )
}

/// Borderless icon glyph (24×24 soft surface or plain colored provider mark).
fn glyph_24(kind: Icon, bg: ThemeColor, fg: ThemeColor) -> View {
    view()
        .flex_shrink_0()
        .w(px(24.))
        .h(px(24.))
        .rounded(px(6.))
        .bg(bg)
        .items_center()
        .justify_center()
        .pointer_events(PointerEvents::None)
        .child(icon_tinted(kind, fg).w(px(14.)).h(px(14.)))
}

/// Borderless full-color Agent provider icon with optional small status dot.
fn agent_glyph_24(engine: Option<&str>, dot: Option<ThemeColor>) -> View {
    let glyph = if let Some(eng) = engine {
        icon_colored(engine_icon(Some(eng)), 18.).into_element()
    } else {
        icon_tinted(Icon::Terminal, MUTED)
            .w(px(15.))
            .h(px(15.))
            .into_element()
    };
    let mut box_v = view()
        .relative()
        .flex_shrink_0()
        .w(px(24.))
        .h(px(24.))
        .items_center()
        .justify_center()
        .pointer_events(PointerEvents::None)
        .child(glyph);
    if let Some(color) = dot {
        box_v = box_v.child(
            view()
                .absolute()
                .right(px(0.))
                .bottom(px(0.))
                .w(px(8.))
                .h(px(8.))
                .rounded_full()
                .bg(BACKGROUND)
                .items_center()
                .justify_center()
                .child(view().w(px(5.5)).h(px(5.5)).rounded_full().bg(color)),
        );
    }
    box_v
}

/// Editorial chapter header on `BACKGROUND` canvas.
fn chapter_header(
    kind: Icon,
    tint: ThemeColor,
    title: impl Into<String>,
    subtitle: impl Into<String>,
    trailing: Option<AnyElement>,
) -> View {
    let sub = subtitle.into();
    let mut left = view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .child(icon_tinted(kind, tint).w(px(14.)).h(px(14.)))
        .child(
            label(title.into())
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(TEXT),
        );
    if !sub.is_empty() {
        left = left.child(muted(sub).text_size(px(12.)).truncate().min_w(px(0.)));
    }
    let mut row = view()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(left);
    if let Some(el) = trailing {
        row = row.child(el);
    }
    row
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
            Some(ReplyTarget::Terminal { label, .. }) => format!("Send to {label}…"),
            Some(ReplyTarget::GitHub { label, .. }) => format!("Comment on {label}…"),
            Some(ReplyTarget::Card { label, .. }) => format!("Reply to {label}…"),
            None => "Reply…".into(),
        };
        let input = field(ui.inbox.reply_draft, &placeholder)
            .flex_1()
            .min_w(px(0.))
            .on_submit(move |_: &str| submit_inline_reply(ui));

        view()
            .mt_2()
            .pl_3()
            .py_1p5()
            .pr_2()
            .bg(INSET)
            .border_l_1()
            .border_color(ACCENT)
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
            .child(chrome_button(Icon::Close, "Cancel", move |_| {
                batch(|| {
                    ui.inbox.reply_target.set(None);
                    ui.inbox.reply_draft.set(String::new());
                });
            }))
            .into_element()
    })
}

/// Minimal 42pt unified chrome bar.
pub fn chrome_bar(ui: Ui) -> impl IntoElement {
    let counts = computed(move || {
        let inbox = ui.daily_inbox.get();
        (inbox.completed_today, inbox.deferred_items.len())
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
                .gap_2()
                .window_drag(false)
                .child(icon_tinted(Icon::Inbox, ACCENT).w(px(15.)).h(px(15.)))
                .child(
                    label("Daily Briefing")
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(TEXT),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .flex_shrink_0()
                .window_drag(false)
                .child(dynamic(move || {
                    let (done, deferred) = counts.get();
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
                .child(chrome_button(Icon::Refresh, "Refresh GitHub", move |_| {
                    refresh_github_notifications(ui, true)
                })),
        )
}

/// Editorial Masthead + Borderless 4-column Hairline KPI Strip on `BACKGROUND` (no progress bar).
fn render_editorial_masthead(ui: Ui, inbox: &DailyInbox) -> View {
    let rem = inbox.remaining_today;

    // KPI #1: Needs Action (strictly 1:1 with `needs_you.len()` & left-rail badge)
    let mut card_n = 0usize;
    let mut term_n = 0usize;
    let mut gh_n = 0usize;
    let mut sched_fail_n = 0usize;
    for item in &inbox.needs_you {
        match item.kind {
            NeedsYouKind::Card { .. } => card_n += 1,
            NeedsYouKind::Terminal { .. } => term_n += 1,
            NeedsYouKind::GitHub { .. } => gh_n += 1,
            NeedsYouKind::ScheduleFailed { .. } => sched_fail_n += 1,
        }
    }
    let mut k1_parts = Vec::new();
    if card_n + term_n > 0 {
        let n = card_n + term_n;
        k1_parts.push(format!(
            "{n} {}",
            if n == 1 { "agent/card" } else { "agents/cards" }
        ));
    }
    if gh_n > 0 {
        k1_parts.push(format!("{gh_n} GitHub"));
    }
    if sched_fail_n > 0 {
        k1_parts.push(format!("{sched_fail_n} schedule"));
    }
    let k1_sub = if k1_parts.is_empty() {
        "—".to_owned()
    } else {
        k1_parts.join(" · ")
    };

    // KPI #2: Scheduled Runs
    let sched_total = inbox.schedule_pulse.len();
    let sched_passed = inbox
        .schedule_pulse
        .iter()
        .filter(|r| matches!(r.state.as_str(), "completed" | "succeeded"))
        .count();
    let sched_failed = inbox
        .schedule_pulse
        .iter()
        .filter(|r| matches!(r.state.as_str(), "failed" | "blocked"))
        .count();
    let sched_running = inbox
        .schedule_pulse
        .iter()
        .filter(|r| matches!(r.state.as_str(), "running" | "starting" | "queued"))
        .count();
    let sched_cards: usize = inbox.schedule_stacks.iter().map(|s| s.cards.len()).sum();
    let k2_val = if sched_total == 0 {
        "0".to_owned()
    } else {
        format!("{sched_passed}/{sched_total}")
    };
    let mut k2_parts = Vec::new();
    if sched_failed > 0 {
        k2_parts.push(format!("{sched_failed} failed"));
    }
    if sched_running > 0 {
        k2_parts.push(format!("{sched_running} running"));
    }
    if sched_cards > 0 {
        k2_parts.push(format!(
            "{sched_cards} {}",
            if sched_cards == 1 { "card" } else { "cards" }
        ));
    }
    let k2_sub = if k2_parts.is_empty() {
        "—".to_owned()
    } else {
        k2_parts.join(" · ")
    };

    // KPI #3: GitHub Today
    let gh_direct_total: usize = inbox.github_stacks.iter().map(|s| s.items.len()).sum();
    let gh_direct_all = gh_direct_total + gh_n;
    let gh_watching_total: usize = inbox.github_watching.iter().map(|s| s.items.len()).sum();
    let gh_watching_repos = inbox.github_watching.len();
    let k3_val = format!("{gh_direct_all} direct");
    let k3_sub = if gh_watching_total > 0 {
        format!(
            "+{gh_watching_total} watching ({gh_watching_repos} {})",
            if gh_watching_repos == 1 {
                "repo"
            } else {
                "repos"
            }
        )
    } else {
        "—".to_owned()
    };

    // KPI #4: Active Work
    let live_agents = inbox.agent_terminals.len();
    let open_cards = inbox.open_cards.len();
    let shells = inbox.plain_shells.len();
    let k4_val = format!("{} live", live_agents + open_cards);
    let mut k4_parts = Vec::new();
    if live_agents > 0 {
        k4_parts.push(format!(
            "{live_agents} {}",
            if live_agents == 1 { "agent" } else { "agents" }
        ));
    }
    if open_cards > 0 {
        k4_parts.push(format!(
            "{open_cards} {}",
            if open_cards == 1 { "card" } else { "cards" }
        ));
    }
    if shells > 0 {
        k4_parts.push(format!(
            "{shells} {}",
            if shells == 1 { "shell" } else { "shells" }
        ));
    }
    let k4_sub = if k4_parts.is_empty() {
        "—".to_owned()
    } else {
        k4_parts.join(" · ")
    };

    let mut sched_dots = view().flex_row().items_center().gap_1();
    for run in inbox.schedule_pulse.iter().take(6) {
        let c = match run.state.as_str() {
            "completed" | "succeeded" => SUCCESS,
            "failed" => DANGER,
            "blocked" => ATTENTION,
            "running" | "starting" | "queued" => ACCENT,
            _ => MUTED,
        };
        sched_dots = sched_dots.child(view().w(px(6.)).h(px(6.)).rounded_full().bg(c));
    }

    let cutoff_str = format_cutoff(inbox.cutoff_minutes);
    let deferred_n = inbox.deferred_items.len();

    let mut meta_line = view()
        .flex_row()
        .items_center()
        .gap_1p5()
        .child(compact_action(
            format!("Since {cutoff_str} ▾"),
            Variant::Quiet,
            move |_| open_cutoff_menu(ui),
        ));
    if deferred_n > 0 {
        meta_line = meta_line.child(muted(format!("· {deferred_n} deferred")).text_size(px(12.)));
    }

    let mut masthead = view()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_4()
        .child(
            view()
                .gap_0p5()
                .child(
                    label(format_today_date_long())
                        .text_size(px(20.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(TEXT),
                )
                .child(meta_line),
        );
    if rem > 0 {
        masthead = masthead.child(compact_action_with_icon(
            Icon::Clock,
            "Defer rest ▾",
            Variant::Quiet,
            move |_| defer_remaining_all(ui),
        ));
    }

    let kpi_cell = |heading_lbl: &'static str,
                    value: String,
                    val_color: ThemeColor,
                    sub: String,
                    inline_extra: Option<AnyElement>,
                    first: bool| {
        let mut val_row = view().flex_row().items_center().gap_2().child(
            label(value)
                .font_family(MONO_FONT)
                .text_size(px(20.))
                .font_weight(FontWeight::BOLD)
                .text_color(val_color),
        );
        if let Some(el) = inline_extra {
            val_row = val_row.child(el);
        }
        let mut cell = view()
            .flex_1()
            .min_w(px(0.))
            .py_2()
            .gap_0p5()
            .child(
                label(heading_lbl)
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(MUTED),
            )
            .child(val_row)
            .child(muted(sub).text_size(px(11.5)).truncate());
        if !first {
            cell = cell.pl_4().border_l_1().border_color(BORDER);
        }
        cell
    };

    let kpi_strip = view()
        .flex_row()
        .items_stretch()
        .gap_3()
        .py_2p5()
        .border_t_1()
        .border_b_1()
        .border_color(BORDER)
        .child(kpi_cell(
            "NEEDS ACTION",
            inbox.actionable_count.to_string(),
            if inbox.actionable_count > 0 {
                ATTENTION
            } else {
                SUCCESS
            },
            k1_sub,
            None,
            true,
        ))
        .child(kpi_cell(
            "SCHEDULED RUNS",
            k2_val,
            if sched_failed > 0 {
                DANGER
            } else if sched_total > 0 && sched_passed == sched_total {
                SUCCESS
            } else {
                TEXT
            },
            k2_sub,
            (!inbox.schedule_pulse.is_empty()).then(|| sched_dots.into_element()),
            false,
        ))
        .child(kpi_cell(
            "GITHUB TODAY",
            k3_val,
            if inbox.needs_you.iter().any(|n| {
                matches!(
                    &n.kind,
                    NeedsYouKind::GitHub { item, .. } if item.is_ci_failure
                )
            }) {
                DANGER
            } else {
                TEXT
            },
            k3_sub,
            None,
            false,
        ))
        .child(kpi_cell("ACTIVE WORK", k4_val, TEXT, k4_sub, None, false));

    view().gap_3().child(masthead).child(kpi_strip)
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
            .pl_3p5()
            .pr_2()
            .py_2()
            .bg(SELECTED)
            .border_l_1()
            .border_color(ACCENT)
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

/// Structured Git telemetry strip for Agent Terminal sessions (no raw TUI chrome).
fn render_terminal_git_telemetry(term: &InboxTerminalWork) -> View {
    let mut row = view().flex_row().flex_wrap().items_center().gap_2();
    if let Some(proj) = &term.project_name {
        row = row.child(muted(proj.clone()).text_size(px(11.5)));
    } else {
        row = row.child(muted(term.host_label.clone()).text_size(px(11.5)));
    }
    if let Some(cwd) = &term.cwd {
        row = row.child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(icon_tinted(Icon::Folder, MUTED).w(px(12.)).h(px(12.)))
                .child(
                    muted(shorten_path(cwd))
                        .font_family(MONO_FONT)
                        .text_size(px(11.5)),
                ),
        );
    }
    if let Some(br) = &term.branch {
        row = row.child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(icon_tinted(Icon::Branch, MUTED).w(px(12.)).h(px(12.)))
                .child(muted(br.clone()).font_family(MONO_FONT).text_size(px(11.5))),
        );
    }

    if term.dirty || term.additions > 0 || term.deletions > 0 {
        let mut diff_row = view().flex_row().items_center().gap_1();
        if term.additions > 0 {
            diff_row = diff_row.child(
                label(format!("+{}", term.additions))
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .text_color(SUCCESS),
            );
        }
        if term.deletions > 0 {
            diff_row = diff_row.child(
                label(format!("-{}", term.deletions))
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .text_color(DANGER),
            );
        }
        diff_row = diff_row.child(muted("uncommitted").text_size(px(11.5)));
        row = row.child(diff_row);
    } else if term.branch.is_some() {
        row = row.child(muted("clean").text_size(px(11.5)));
    }
    if term.ahead > 0 {
        row = row.child(
            label(format!("↑{}", term.ahead))
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(ACCENT),
        );
    }
    row
}

/// Borderless expanded GitHub PR/Issue Markdown drawer (left quote rule, avatar, CI, comments, Collapse).
fn render_expanded_github_detail(ui: Ui, item: &GitHubStackItem) -> AnyElement {
    let key_body = item.key.clone();
    let key_collapse = item.key.clone();
    let static_body = item.body.clone();
    let static_author = item.author.clone();
    let static_ci_state = item.ci_state.clone();
    let static_ci_passed = item.ci_passed;
    let static_ci_total = item.ci_total;
    let static_failed = item.failed_checks.clone();
    let static_comments = item.comments_count;
    let url_open = item.url.clone();
    let cid_open = item.card_id.clone();
    let is_pr = item.kind == "pr";

    dynamic(move || {
        let open = ui
            .inbox
            .expanded_github_items
            .with(|s| s.contains(&key_body));
        if !open {
            return view().into_element();
        }
        let detail = ui
            .inbox
            .github_item_details
            .with(|m| m.get(&key_body).cloned());
        let loading = ui.inbox.github_item_loading.with(|s| s.contains(&key_body));

        let author = detail
            .as_ref()
            .and_then(|d| d.author_login.clone())
            .or_else(|| static_author.clone());
        let avatar_url = detail.as_ref().and_then(|d| d.author_avatar_url.clone());
        let ci_state = detail
            .as_ref()
            .and_then(|d| d.ci_state.clone())
            .or_else(|| static_ci_state.clone());
        let (ci_passed, ci_total) = match detail.as_ref().filter(|d| d.ci_total > 0) {
            Some(d) => (d.ci_passed, d.ci_total),
            None => (static_ci_passed, static_ci_total),
        };
        let failed_checks: Vec<String> = detail
            .as_ref()
            .filter(|d| !d.failed_checks.is_empty())
            .map(|d| d.failed_checks.clone())
            .unwrap_or_else(|| static_failed.clone());
        let comments = detail
            .as_ref()
            .and_then(|d| d.comments_count)
            .or(static_comments);
        let body_md = detail
            .as_ref()
            .map(|d| d.body.clone())
            .filter(|b| !b.trim().is_empty())
            .or_else(|| static_body.clone())
            .unwrap_or_default();

        // Top metadata bar: author avatar + @login + PR state + CI + comments + branch/diff + actions
        let mut meta_left = view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .flex_1()
            .min_w(px(0.));
        if let Some(a) = &author {
            meta_left = meta_left.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .child(github_avatar(a, avatar_url.as_deref(), 18.))
                    .child(
                        label(format!("@{a}"))
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(TEXT),
                    ),
            );
        }
        if let Some(d) = &detail {
            let state_badge = if d.merged {
                Some(("Merged", Tone::Highlight))
            } else if d.draft {
                Some(("Draft", Tone::Neutral))
            } else if d.state.eq_ignore_ascii_case("open") {
                Some(("Open", Tone::Success))
            } else if d.state.eq_ignore_ascii_case("closed") {
                Some(("Closed", Tone::Neutral))
            } else {
                None
            };
            if let Some((lbl, tone)) = state_badge {
                meta_left = meta_left.child(status(lbl, tone));
            }
        }
        if let Some(ci_el) = ci_indicator(
            ci_state.as_deref(),
            ci_passed,
            ci_total,
            failed_checks.len(),
        ) {
            meta_left = meta_left.child(ci_el);
        }
        if let Some(c) = comments
            && (is_pr || c > 0)
        {
            meta_left = meta_left.child(comments_indicator(c));
        }
        if let Some(d) = &detail {
            if let (Some(head), Some(base)) = (&d.head_ref, &d.base_ref) {
                meta_left = meta_left.child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(icon_tinted(Icon::Branch, MUTED).w(px(11.)).h(px(11.)))
                        .child(
                            muted(format!("{head} → {base}"))
                                .font_family(MONO_FONT)
                                .text_size(px(11.)),
                        ),
                );
            }
            if d.additions.is_some() || d.deletions.is_some() {
                let mut diff = view().flex_row().items_center().gap_1();
                if let Some(a) = d.additions {
                    diff = diff.child(
                        label(format!("+{a}"))
                            .font_family(MONO_FONT)
                            .text_size(px(11.))
                            .text_color(SUCCESS),
                    );
                }
                if let Some(del) = d.deletions {
                    diff = diff.child(
                        label(format!("-{del}"))
                            .font_family(MONO_FONT)
                            .text_size(px(11.))
                            .text_color(DANGER),
                    );
                }
                meta_left = meta_left.child(diff);
            }
        }

        let mut meta_right = view().flex_row().items_center().gap_1().flex_shrink_0();
        if let Some(cid) = cid_open.clone() {
            meta_right = meta_right.child(compact_action("Card ↗", Variant::Quiet, move |_| {
                ui.open_card(cid.clone())
            }));
        }
        if !url_open.is_empty() {
            let u = url_open.clone();
            meta_right =
                meta_right.child(compact_action("GitHub ↗", Variant::Quiet, move |_| {
                    let _ = crate::platform::open_external(&u);
                }));
        }
        let kc = key_collapse.clone();
        meta_right = meta_right.child(compact_action("Collapse ▴", Variant::Quiet, move |_| {
            collapse_github_item(ui, &kc);
        }));

        let mut drawer = view()
            .ml(px(20.))
            .mt_2()
            .mb_1()
            .pl_3p5()
            .border_l_1()
            .border_color(BORDER)
            .gap_2()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(meta_left)
                    .child(meta_right),
            );

        if !failed_checks.is_empty() {
            let mut pills = view().flex_row().flex_wrap().gap_1p5();
            for chk in failed_checks {
                pills = pills.child(status(format!("✗ {chk}"), Tone::Danger));
            }
            drawer = drawer.child(pills);
        }

        if loading && detail.is_none() && body_md.is_empty() {
            drawer = drawer.child(muted("Loading…").text_size(px(12.)));
        } else if !body_md.is_empty() {
            let md = body_md.clone();
            drawer = drawer.child(
                view()
                    .w_full()
                    .min_w(px(0.))
                    .child(crate::editor::reader(ui, move || md.clone())),
            );
        } else {
            drawer = drawer.child(muted("No description").text_size(px(12.)));
        }

        drawer.into_element()
    })
    .into_element()
}

/// Tier 1: Flat `Needs Action` row (strictly 1:1 with left-rail badge, no outer box border).
fn render_needs_you_row(ui: Ui, item: NeedsYouItem, now_ms: u64) -> View {
    let key_defer = item.key.clone();
    let key_dismiss = item.key.clone();
    let wm = item.watermark;
    let age = format_age_compact(item.timestamp_ms, now_ms);

    let row_base = |accent: ThemeColor| {
        view()
            .flex_row()
            .items_stretch()
            .min_w(px(0.))
            .bg(SURFACE)
            .border_b_1()
            .border_color(BORDER)
            .child(view().w(px(3.)).flex_shrink_0().bg(accent))
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
            let dismiss_title = defer_title.clone();

            let is_code_gate = !is_blocked && (is_review || can_quick_merge);
            let accent = if is_blocked {
                ATTENTION
            } else if can_quick_merge {
                ACCENT
            } else if is_review {
                HIGHLIGHT
            } else {
                ACCENT
            };

            let left_tile = if is_blocked {
                agent_glyph_24(agent_engine.as_deref(), Some(ATTENTION))
            } else if can_quick_merge {
                glyph_24(Icon::GitMerge, SELECTED, ACCENT)
            } else if is_review {
                glyph_24(Icon::Eye, HIGHLIGHT_SURFACE, HIGHLIGHT)
            } else if agent_engine.is_some() {
                agent_glyph_24(agent_engine.as_deref(), None)
            } else {
                glyph_24(Icon::MessageSquare, SELECTED, ACCENT)
            };

            let intent_line = if is_blocked {
                match &actor {
                    Some(a) => format!("{a} · Waiting"),
                    None => "Waiting".into(),
                }
            } else if can_quick_merge {
                match pr_number {
                    Some(n) => format!("Ready to merge · PR #{n}"),
                    None => "Ready to merge".into(),
                }
            } else if is_review {
                "Review".into()
            } else {
                headline.clone()
            };

            let ctx_text = match host_label {
                Some(h) => format!("{project_name} · {h}"),
                None => project_name,
            };

            let title_line = view()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w(px(0.))
                .child(
                    muted(format!("#{card_number}"))
                        .font_family(MONO_FONT)
                        .text_size(px(12.)),
                )
                .child(
                    label(card_title.clone())
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(TEXT)
                        .truncate()
                        .flex_1()
                        .min_w(px(0.)),
                )
                .child(muted(ctx_text).text_size(px(11.5)).flex_shrink_0())
                .child(
                    muted(age)
                        .font_family(MONO_FONT)
                        .text_size(px(11.))
                        .flex_shrink_0(),
                );

            let excerpt = body
                .filter(|b| !b.trim().is_empty())
                .or(detail.filter(|d| !d.trim().is_empty() && d != &headline));

            let mut body_col = view().flex_1().min_w(px(0.)).gap_1p5().child(
                press_target()
                    .w_full()
                    .bg(CLEAR)
                    .rounded(px(4.))
                    .hover(|s| s.bg(CARD_HOVER))
                    .on_press(move |_: &()| ui.open_card(cid_open.clone()))
                    .child(
                        view()
                            .gap_0p5()
                            .child(title_line)
                            .child(label(intent_line).text_size(px(12.)).text_color(accent)),
                    ),
            );

            if is_code_gate {
                let mut pr_strip = view().flex_row().flex_wrap().items_center().gap_2();
                if let Some(n) = pr_number {
                    pr_strip = pr_strip.child(
                        label(format!("PR #{n}"))
                            .font_family(MONO_FONT)
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
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
                body_col = body_col.child(pr_strip);
            } else if let Some(ex) = excerpt {
                body_col = body_col.child(
                    view()
                        .pl_3()
                        .py_1p5()
                        .pr_2()
                        .bg(INSET)
                        .child(label(ex).text_size(px(12.5)).text_color(TEXT).select_text()),
                );
            }

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
                left_actions =
                    left_actions.child(compact_action("Terminal ↗", Variant::Quiet, move |_| {
                        ui.open_card_with_terminal(cid_term.clone(), Some(tid.clone()));
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
                .child(chrome_button(Icon::Check, "Done", move |_| {
                    if unread_seq > 0 {
                        crate::overview::mark_card_read(ui, cid_dismiss.clone(), unread_seq);
                    }
                    dismiss_items(
                        ui,
                        vec![(key_dismiss.clone(), wm)],
                        format!("Done · {dismiss_title}"),
                    );
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
                        body_col
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
                    ),
            )
        }
        NeedsYouKind::Terminal { terminal } => {
            let term = *terminal;
            let tid = term.terminal_id.clone();
            let tid_open = tid.clone();
            let tid_close = tid.clone();
            let card_open = term.card_id.clone();
            let title_close = term.title.clone();
            let defer_title = term.title.clone();
            let dismiss_title = term.title.clone();
            let is_err = matches!(term.status, HerdrStatus::Error | HerdrStatus::Disconnected);
            let accent = if is_err { DANGER } else { ATTENTION };
            let reply_target = ReplyTarget::Terminal {
                terminal_id: tid.clone(),
                label: term.title.clone(),
            };
            let reply_target_btn = reply_target.clone();

            let left_tile = agent_glyph_24(term.agent_engine.as_deref(), Some(accent));

            let intent_line = if is_err {
                "Error".to_owned()
            } else if let Some(wt) = &term.waiting_title {
                wt.clone()
            } else {
                "Waiting".to_owned()
            };

            let mut body_col = view()
                .flex_1()
                .min_w(px(0.))
                .gap_1p5()
                .child(
                    press_target()
                        .w_full()
                        .bg(CLEAR)
                        .rounded(px(4.))
                        .hover(|s| s.bg(CARD_HOVER))
                        .on_press(move |_: &()| {
                            if let Some(cid) = &card_open {
                                ui.open_card_with_terminal(cid.clone(), Some(tid_open.clone()));
                            } else {
                                ui.open_terminal(tid_open.clone());
                            }
                        })
                        .child(
                            view()
                                .gap_0p5()
                                .child(
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            label(term.title.clone())
                                                .text_size(px(13.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(TEXT)
                                                .truncate()
                                                .flex_1()
                                                .min_w(px(0.)),
                                        )
                                        .child(
                                            muted(age)
                                                .font_family(MONO_FONT)
                                                .text_size(px(11.))
                                                .flex_shrink_0(),
                                        ),
                                )
                                .child(label(intent_line).text_size(px(12.)).text_color(accent)),
                        ),
                )
                .child(render_terminal_git_telemetry(&term));

            if let Some(detail) = &term.status_detail {
                let mut detail_lbl = label(detail.clone())
                    .text_size(px(if is_err { 11.5 } else { 12.5 }))
                    .text_color(if is_err { DANGER } else { TEXT })
                    .select_text();
                if is_err {
                    detail_lbl = detail_lbl.font_family(MONO_FONT);
                }
                body_col = body_col.child(
                    view()
                        .pl_3()
                        .py_1p5()
                        .pr_2()
                        .bg(if is_err { BACKGROUND } else { INSET })
                        .child(detail_lbl),
                );
            }

            let left_actions = view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .child(compact_action_with_icon(
                    Icon::MessageSquare,
                    "Reply",
                    Variant::Primary,
                    move |_| start_inline_reply(ui, reply_target_btn.clone()),
                ))
                .child(compact_action("Close", Variant::Quiet, move |_| {
                    close_terminal_from_inbox(ui, tid_close.clone(), title_close.clone())
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
                        body_col
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
                    ),
            )
        }
        NeedsYouKind::GitHub {
            item: gh_box,
            project_name,
            ..
        } => {
            let gh = *gh_box;
            let gh_toggle = gh.clone();
            let gh_key_for_meta = gh.key.clone();
            let gh_static_author = gh.author.clone();
            let gh_static_ci = gh.ci_state.clone();
            let gh_static_passed = gh.ci_passed;
            let gh_static_total = gh.ci_total;
            let gh_static_failed = gh.failed_checks.clone();
            let gh_static_comments = gh.comments_count;
            let gh_is_pr = gh.kind == "pr";
            let gh_has_num = gh.number.is_some();
            let url_open = gh.url.clone();
            let url_btn = gh.url.clone();
            let cid_btn = gh.card_id.clone();
            let defer_title = gh.title.clone();
            let gh_dismiss = gh.clone();
            let is_ci = gh.is_ci_failure;
            let is_rev = gh.reason.to_ascii_lowercase().contains("review")
                || gh.reason.to_ascii_lowercase().contains("changes");
            let accent = if is_ci {
                DANGER
            } else if is_rev {
                HIGHLIGHT
            } else {
                ATTENTION
            };
            let left_tile = glyph_24(
                if is_ci {
                    Icon::TriangleAlert
                } else if is_rev {
                    Icon::Eye
                } else {
                    Icon::GitHub
                },
                if is_ci {
                    DANGER_SURFACE
                } else if is_rev {
                    HIGHLIGHT_SURFACE
                } else {
                    ATTENTION_SURFACE
                },
                accent,
            );
            let reply_target = gh.number.map(|num| ReplyTarget::GitHub {
                key: gh.key.clone(),
                repository: gh.repository.clone(),
                kind: gh.kind.clone(),
                number: num,
                url: gh.url.clone(),
                label: format!("{} #{num}", gh.repository),
            });
            let reply_btn = reply_target.clone();
            let ref_label = gh
                .number
                .map(|n| format!("{} #{n}", gh.repository))
                .unwrap_or_else(|| gh.repository.clone());
            let reason_line = if is_ci {
                "CI failed".to_owned()
            } else if is_rev {
                "Review requested".to_owned()
            } else {
                gh.reason.clone()
            };

            let live_meta = dynamic(move || {
                let detail = ui
                    .inbox
                    .github_item_details
                    .with(|m| m.get(&gh_key_for_meta).cloned());
                let is_open = ui
                    .inbox
                    .expanded_github_items
                    .with(|s| s.contains(&gh_key_for_meta));
                let author = detail
                    .as_ref()
                    .and_then(|d| d.author_login.clone())
                    .or_else(|| gh_static_author.clone());
                let avatar_url = detail.as_ref().and_then(|d| d.author_avatar_url.clone());
                let ci_state = detail
                    .as_ref()
                    .and_then(|d| d.ci_state.clone())
                    .or_else(|| gh_static_ci.clone());
                let (ci_passed, ci_total) = match detail.as_ref().filter(|d| d.ci_total > 0) {
                    Some(d) => (d.ci_passed, d.ci_total),
                    None => (gh_static_passed, gh_static_total),
                };
                let failed_n = detail
                    .as_ref()
                    .filter(|d| !d.failed_checks.is_empty())
                    .map(|d| d.failed_checks.len())
                    .unwrap_or(gh_static_failed.len());
                let comments = detail
                    .as_ref()
                    .and_then(|d| d.comments_count)
                    .or(gh_static_comments);

                let mut row = view().flex_row().items_center().gap_2().flex_wrap();
                if gh_has_num {
                    row = row.child(
                        icon_tinted(
                            if is_open {
                                Icon::ChevronDown
                            } else {
                                Icon::Forward
                            },
                            MUTED,
                        )
                        .w(px(12.))
                        .h(px(12.)),
                    );
                }
                row = row.child(
                    label(reason_line.clone())
                        .text_size(px(12.))
                        .text_color(accent),
                );
                if let Some(ci_el) =
                    ci_indicator(ci_state.as_deref(), ci_passed, ci_total, failed_n)
                {
                    row = row.child(ci_el);
                }
                if let Some(c) = comments
                    && (gh_is_pr || c > 0)
                {
                    row = row.child(comments_indicator(c));
                }
                if let Some(a) = &author {
                    row = row.child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .child(github_avatar(a, avatar_url.as_deref(), 16.))
                            .child(muted(format!("@{a}")).text_size(px(11.5))),
                    );
                }
                row.into_element()
            });

            let expanded_panel = render_expanded_github_detail(ui, &gh);

            let mut body_col = view()
                .flex_1()
                .min_w(px(0.))
                .gap_1p5()
                .child(
                    press_target()
                        .w_full()
                        .bg(CLEAR)
                        .rounded(px(4.))
                        .hover(|s| s.bg(CARD_HOVER))
                        .on_press(move |_: &()| {
                            if gh_toggle.number.is_some() {
                                toggle_github_item(ui, &gh_toggle);
                            } else if !url_open.is_empty() {
                                let _ = crate::platform::open_external(&url_open);
                            }
                        })
                        .child(
                            view()
                                .gap_1()
                                .child(
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .min_w(px(0.))
                                        .child(
                                            muted(ref_label)
                                                .font_family(MONO_FONT)
                                                .text_size(px(11.5))
                                                .flex_shrink_0(),
                                        )
                                        .child(
                                            label(gh.title.clone())
                                                .text_size(px(13.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(TEXT)
                                                .truncate()
                                                .flex_1()
                                                .min_w(px(0.)),
                                        )
                                        .child(
                                            muted(project_name).text_size(px(11.5)).flex_shrink_0(),
                                        )
                                        .child(
                                            muted(age)
                                                .font_family(MONO_FONT)
                                                .text_size(px(11.))
                                                .flex_shrink_0(),
                                        ),
                                )
                                .child(live_meta),
                        ),
                )
                .child(expanded_panel);

            if !gh.failed_checks.is_empty() {
                let mut pills = view().flex_row().flex_wrap().gap_1p5();
                for chk in &gh.failed_checks {
                    pills = pills.child(status(format!("✗ {chk}"), Tone::Danger));
                }
                body_col = body_col.child(pills);
            }

            let mut left_actions = view().flex_row().items_center().gap_1p5();
            if is_ci {
                if !url_btn.is_empty() {
                    left_actions = left_actions.child(compact_action(
                        "GitHub ↗",
                        Variant::Primary,
                        move |_| {
                            let _ = crate::platform::open_external(&url_btn);
                        },
                    ));
                }
                if let Some(rt) = reply_btn {
                    left_actions =
                        left_actions.child(compact_action("Reply", Variant::Quiet, move |_| {
                            start_inline_reply(ui, rt.clone())
                        }));
                }
            } else {
                if let Some(rt) = reply_btn {
                    left_actions = left_actions.child(compact_action_with_icon(
                        Icon::MessageSquare,
                        "Reply",
                        Variant::Primary,
                        move |_| start_inline_reply(ui, rt.clone()),
                    ));
                }
                if let Some(cid) = cid_btn {
                    left_actions =
                        left_actions.child(compact_action("Card ↗", Variant::Quiet, move |_| {
                            ui.open_card(cid.clone())
                        }));
                }
                if !url_btn.is_empty() {
                    left_actions = left_actions.child(compact_action(
                        "GitHub ↗",
                        Variant::Quiet,
                        move |_| {
                            let _ = crate::platform::open_external(&url_btn);
                        },
                    ));
                }
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
                    dismiss_github_item(ui, gh_dismiss.clone())
                }));

            let mut col = body_col.child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .pt_0p5()
                    .child(left_actions)
                    .child(right_triage),
            );
            if let Some(rt) = reply_target {
                col = col.child(inline_reply_box(ui, rt));
            }

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
                    .child(col),
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
            let pid_row = project_id.clone();
            let sid_row = schedule_id.clone();
            let is_failed = state == "failed";
            let accent = if is_failed { DANGER } else { ATTENTION };

            let left_tile = glyph_24(
                Icon::TriangleAlert,
                if is_failed {
                    DANGER_SURFACE
                } else {
                    ATTENTION_SURFACE
                },
                accent,
            );
            let ctx_text = match host_label {
                Some(h) => format!("{project_name} · {h}"),
                None => project_name,
            };
            let detail_text = error.or(summary);

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
                left_actions =
                    left_actions.child(compact_action("Terminal ↗", Variant::Quiet, move |_| {
                        ui.open_terminal(tid.clone())
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
                .child(chrome_button(Icon::Check, "Dismiss", move |_| {
                    dismiss_items(
                        ui,
                        vec![(key_dismiss.clone(), wm)],
                        format!("Dismissed {dismiss_title}"),
                    )
                }));

            let mut col = view().flex_1().min_w(px(0.)).gap_1p5().child(
                press_target()
                    .w_full()
                    .bg(CLEAR)
                    .rounded(px(4.))
                    .hover(|s| s.bg(CARD_HOVER))
                    .on_press(move |_: &()| {
                        crate::schedules::open(ui, Some(pid_row.clone()), Some(sid_row.clone()));
                    })
                    .child(
                        view()
                            .gap_0p5()
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        label(schedule_name)
                                            .text_size(px(13.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(TEXT)
                                            .truncate()
                                            .flex_1()
                                            .min_w(px(0.)),
                                    )
                                    .child(muted(ctx_text).text_size(px(11.5)).flex_shrink_0())
                                    .child(
                                        muted(age)
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.))
                                            .flex_shrink_0(),
                                    ),
                            )
                            .child(
                                label(if is_failed { "Failed" } else { "Waiting" })
                                    .text_size(px(12.))
                                    .text_color(accent),
                            ),
                    ),
            );
            if let Some(err) = detail_text {
                col = col.child(
                    view().pl_3().py_1p5().pr_2().bg(BACKGROUND).child(
                        label(format!("> {err}"))
                            .font_family(MONO_FONT)
                            .text_size(px(11.5))
                            .text_color(accent)
                            .select_text(),
                    ),
                );
            }
            col = col.child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .pt_0p5()
                    .child(left_actions)
                    .child(right_triage),
            );

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
                    .child(col),
            )
        }
    }
}

/// Hairline-ruled `Scheduled Runs` report table on `BACKGROUND` + inline `Created cards` sub-list.
fn render_schedules_report(
    ui: Ui,
    runs: &[SchedulePulseItem],
    stacks: Vec<ScheduleCardStack>,
) -> Option<View> {
    if runs.is_empty() && stacks.is_empty() {
        return None;
    }
    let show_all = ui.inbox.schedule_expanded.get();
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
    let new_cards_total: usize = stacks.iter().map(|s| s.cards.len()).sum();

    let mut summary_parts = Vec::new();
    if passed > 0 {
        summary_parts.push(format!("{passed} passed"));
    }
    if failed > 0 {
        summary_parts.push(format!("{failed} failed"));
    }
    if running > 0 {
        summary_parts.push(format!("{running} running"));
    }
    if new_cards_total > 0 {
        summary_parts.push(format!(
            "{new_cards_total} {}",
            if new_cards_total == 1 {
                "card"
            } else {
                "cards"
            }
        ));
    }

    let mut dots = view().flex_row().items_center().gap_1();
    for run in runs.iter().take(10) {
        let c = match run.state.as_str() {
            "completed" | "succeeded" => SUCCESS,
            "failed" => DANGER,
            "blocked" => ATTENTION,
            "running" | "starting" | "queued" => ACCENT,
            _ => MUTED,
        };
        dots = dots.child(view().w(px(6.)).h(px(6.)).rounded_full().bg(c));
    }

    let mut trailing = view().flex_row().items_center().gap_2().child(dots);
    if runs.len() > 5 {
        trailing = trailing.child(compact_action(
            if show_all {
                "Fewer ▴".to_owned()
            } else {
                format!("All {} ▾", runs.len())
            },
            Variant::Quiet,
            move |_| ui.inbox.schedule_expanded.update(|v| *v = !*v),
        ));
    }

    let mut sec = view().gap_2().child(chapter_header(
        Icon::Trigger,
        if failed > 0 { DANGER } else { ACCENT },
        "Scheduled Runs",
        summary_parts.join(" · "),
        Some(trailing.into_element()),
    ));

    if !runs.is_empty() {
        let mut table = view().child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .py_1p5()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    muted("TIME")
                        .font_family(MONO_FONT)
                        .text_size(px(10.5))
                        .w(px(48.)),
                )
                .child(
                    muted("SCHEDULE")
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .w(px(230.)),
                )
                .child(
                    muted("OUTCOME")
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .flex_1()
                        .min_w(px(0.)),
                )
                .child(
                    muted("STATUS")
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD),
                ),
        );

        let limit = if show_all { runs.len() } else { 5 };
        for run in runs.iter().take(limit) {
            let time_str = format_clock_hm(run.scheduled_at);
            let (dot_color, state_label, state_tone, is_live) = match run.state.as_str() {
                "completed" | "succeeded" => (SUCCESS, "Passed".to_owned(), Tone::Success, false),
                "failed" => (DANGER, "Failed".to_owned(), Tone::Danger, false),
                "blocked" => (ATTENTION, "Waiting".to_owned(), Tone::Attention, false),
                "running" | "starting" | "queued" => {
                    (ACCENT, "Running".to_owned(), Tone::Info, true)
                }
                other => (MUTED, other.to_owned(), Tone::Neutral, false),
            };
            let is_failed = run.state == "failed";
            let outcome_text = if let Some(err) = &run.error {
                err.clone()
            } else if let Some(sum) = &run.summary {
                sum.clone()
            } else if let Some(num) = run.card_number {
                format!("Created #{num}")
            } else if is_live {
                "Running…".into()
            } else {
                "—".into()
            };
            let pid = run.project_id.clone();
            let sid = run.schedule_id.clone();
            let cid = run.card_id.clone();
            let pid_retry = run.project_id.clone();
            let sid_retry = run.schedule_id.clone();

            let mut right = view().flex_row().items_center().gap_1p5().flex_shrink_0();
            if is_failed {
                right = right.child(compact_action("Retry", Variant::Quiet, move |_| {
                    dispatch(
                        ui,
                        "schedule.trigger",
                        json!({ "project_id": pid_retry, "schedule_id": sid_retry }),
                    );
                    ui.notify("Retried scheduled run");
                }));
            }
            right = right.child(if is_live {
                live_status(state_label, state_tone)
            } else {
                status(state_label, state_tone)
            });

            table = table.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .py_2()
                    .border_b_1()
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
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_3()
                                    .min_w(px(0.))
                                    .child(
                                        muted(time_str)
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.5))
                                            .w(px(48.))
                                            .flex_shrink_0(),
                                    )
                                    .child(
                                        view()
                                            .w(px(230.))
                                            .flex_shrink_0()
                                            .flex_row()
                                            .items_center()
                                            .gap_1p5()
                                            .min_w(px(0.))
                                            .child(status_dot(dot_color))
                                            .child(
                                                label(run.schedule_name.clone())
                                                    .text_size(px(12.5))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .truncate(),
                                            )
                                            .child(
                                                muted(format!("· {}", run.project_name))
                                                    .text_size(px(11.5))
                                                    .truncate(),
                                            ),
                                    )
                                    .child({
                                        let mut out_lbl = muted(outcome_text)
                                            .text_size(px(12.))
                                            .text_color(if is_failed { DANGER } else { MUTED })
                                            .truncate()
                                            .flex_1()
                                            .min_w(px(0.));
                                        if is_failed {
                                            out_lbl = out_lbl.font_family(MONO_FONT);
                                        }
                                        out_lbl
                                    }),
                            ),
                    )
                    .child(right),
            );
        }
        sec = sec.child(table);
    }

    if !stacks.is_empty() {
        let all_cards: Vec<_> = stacks.into_iter().flat_map(|s| s.cards).collect();
        let count = all_cards.len();
        let dismiss_keys: Vec<(String, Option<u64>)> = all_cards
            .iter()
            .map(|c| (c.key.clone(), c.watermark))
            .collect();

        let mut sub = view().pl_5().pt_1().gap_1().child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    muted(format!("Created cards ({count})"))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM),
                )
                .child(compact_action("Dismiss all", Variant::Quiet, move |_| {
                    dismiss_items(
                        ui,
                        dismiss_keys.clone(),
                        format!("Dismissed {count} scheduled cards"),
                    )
                })),
        );
        for c in all_cards {
            let cid_open = c.card_id.clone();
            let cid_arch = c.card_id.clone();
            let key_dismiss = c.key.clone();
            let wm = c.watermark;
            let title_lbl = format!("#{} {}", c.card_number, c.card_title);
            sub = sub.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .py_1p5()
                    .border_b_1()
                    .border_color(BORDER)
                    .child(
                        press_target()
                            .flex_1()
                            .min_w(px(0.))
                            .bg(CLEAR)
                            .rounded(px(4.))
                            .hover(|s| s.bg(CARD_HOVER))
                            .on_press(move |_: &()| ui.open_card(cid_open.clone()))
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .min_w(px(0.))
                                    .child(
                                        muted(format!("#{}", c.card_number))
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.5))
                                            .w(px(44.))
                                            .flex_shrink_0(),
                                    )
                                    .child(
                                        label(c.card_title.clone())
                                            .text_size(px(12.5))
                                            .truncate()
                                            .flex_1()
                                            .min_w(px(0.)),
                                    )
                                    .child(muted(c.project_name.clone()).text_size(px(11.5)))
                                    .child(muted(c.stage.clone()).text_size(px(11.5))),
                            ),
                    )
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(compact_action("Archive", Variant::Quiet, move |_| {
                                if let Some(card) = ui.snapshot.with_untracked(|s| {
                                    s.cards.iter().find(|x| x.id == cid_arch).cloned()
                                }) {
                                    dispatch(
                                        ui,
                                        "card.archive",
                                        json!({ "card_id": card.id, "revision": card.revision }),
                                    );
                                }
                            }))
                            .child(chrome_button(Icon::Check, "Dismiss", move |_| {
                                dismiss_items(
                                    ui,
                                    vec![(key_dismiss.clone(), wm)],
                                    format!("Dismissed {title_lbl}"),
                                )
                            })),
                    ),
            );
        }
        sec = sec.child(sub);
    }

    Some(sec)
}

/// Unified borderless GitHub PR/Issue row (Direct, Watching child, and Earlier).
/// Shows expand chevron, `#number`, title, labels, CI state, comment icon+count, avatar+@author, age,
/// and toggles an indented Markdown reader on click.
fn render_github_row(
    ui: Ui,
    item: GitHubStackItem,
    now_ms: u64,
    show_repo: bool,
    indent_left: f32,
    pin_day_start: Option<u64>,
) -> View {
    let key_str = item.key.clone();
    let key_row = key_str.clone();
    let key_defer = key_str.clone();
    let key_pin = key_str.clone();
    let wm = item.watermark;
    let age = format_age_compact(item.timestamp_ms, now_ms);
    let url_open = item.url.clone();
    let item_for_toggle = item.clone();
    let item_for_dismiss = item.clone();
    let defer_title = item.title.clone();
    let is_pr = item.kind == "pr";
    let has_number = item.number.is_some();
    let reply_target = item.number.map(|num| ReplyTarget::GitHub {
        key: item.key.clone(),
        repository: item.repository.clone(),
        kind: item.kind.clone(),
        number: num,
        url: item.url.clone(),
        label: format!("{} #{num}", item.repository),
    });
    let reply_btn = reply_target.clone();
    let ref_col = item
        .number
        .map(|n| format!("#{n}"))
        .unwrap_or_else(|| "—".into());
    let repo_label = item.repository.clone();
    let title_text = item.title.clone();
    let static_author = item.author.clone();
    let static_ci_state = item.ci_state.clone();
    let static_ci_passed = item.ci_passed;
    let static_ci_total = item.ci_total;
    let static_failed = item.failed_checks.clone();
    let static_labels = item.labels.clone();
    let static_comments = item.comments_count;

    let left_dynamic = dynamic(move || {
        let expanded = ui
            .inbox
            .expanded_github_items
            .with(|s| s.contains(&key_row));
        let detail = ui
            .inbox
            .github_item_details
            .with(|m| m.get(&key_row).cloned());

        let author = detail
            .as_ref()
            .and_then(|d| d.author_login.clone())
            .or_else(|| static_author.clone());
        let avatar_url = detail.as_ref().and_then(|d| d.author_avatar_url.clone());
        let ci_state = detail
            .as_ref()
            .and_then(|d| d.ci_state.clone())
            .or_else(|| static_ci_state.clone());
        let (ci_passed, ci_total) = match detail.as_ref().filter(|d| d.ci_total > 0) {
            Some(d) => (d.ci_passed, d.ci_total),
            None => (static_ci_passed, static_ci_total),
        };
        let failed_n = detail
            .as_ref()
            .filter(|d| !d.failed_checks.is_empty())
            .map(|d| d.failed_checks.len())
            .unwrap_or(static_failed.len());
        let labels = detail
            .as_ref()
            .filter(|d| !d.labels.is_empty())
            .map(|d| d.labels.clone())
            .unwrap_or_else(|| static_labels.clone());
        let comments = detail
            .as_ref()
            .and_then(|d| d.comments_count)
            .or(static_comments);

        let mut left = view().flex_row().items_center().gap_2().min_w(px(0.));
        if has_number {
            left = left.child(
                icon_tinted(
                    if expanded {
                        Icon::ChevronDown
                    } else {
                        Icon::Forward
                    },
                    MUTED,
                )
                .w(px(12.))
                .h(px(12.)),
            );
        } else {
            left = left.child(icon_tinted(Icon::GitHub, MUTED).w(px(13.)).h(px(13.)));
        }
        if show_repo {
            left = left.child(
                muted(repo_label.clone())
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .flex_shrink_0(),
            );
        }
        left = left
            .child(
                muted(ref_col.clone())
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .w(px(44.))
                    .flex_shrink_0(),
            )
            .child(
                label(title_text.clone())
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(TEXT)
                    .truncate()
                    .flex_1()
                    .min_w(px(0.)),
            );
        for lbl in labels.into_iter().take(2) {
            left = left.child(badge(lbl));
        }
        if let Some(ci_el) = ci_indicator(ci_state.as_deref(), ci_passed, ci_total, failed_n) {
            left = left.child(ci_el);
        }
        if let Some(c) = comments
            && (is_pr || c > 0)
        {
            left = left.child(comments_indicator(c));
        }
        if let Some(a) = &author {
            left = left.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .child(github_avatar(a, avatar_url.as_deref(), 16.))
                    .child(muted(format!("@{a}")).text_size(px(11.5))),
            );
        }
        left = left.child(
            muted(age.clone())
                .font_family(MONO_FONT)
                .text_size(px(11.))
                .flex_shrink_0(),
        );
        left.into_element()
    });

    let mut right = view().flex_row().items_center().gap_1().flex_shrink_0();
    if let Some(day_start) = pin_day_start {
        right = right.child(compact_action("Today", Variant::Quiet, move |_| {
            pin_to_today(ui, key_pin.clone(), day_start, wm)
        }));
    } else if let Some(rt) = reply_btn {
        right = right.child(compact_action("Reply", Variant::Quiet, move |_| {
            start_inline_reply(ui, rt.clone())
        }));
        right = right.child(chrome_button(Icon::Clock, "Defer", move |_| {
            open_defer_menu(ui, vec![(key_defer.clone(), wm)], defer_title.clone())
        }));
    }
    right = right.child(chrome_button(Icon::Check, "Mark read", move |_| {
        dismiss_github_item(ui, item_for_dismiss.clone())
    }));

    let expanded_preview = render_expanded_github_detail(ui, &item);

    let mut wrapper = view()
        .py_2()
        .pl(px(indent_left))
        .border_b_1()
        .border_color(BORDER)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    press_target()
                        .flex_1()
                        .min_w(px(0.))
                        .bg(CLEAR)
                        .rounded(px(4.))
                        .hover(|s| s.bg(CARD_HOVER))
                        .on_press(move |_: &()| {
                            if item_for_toggle.number.is_some() {
                                toggle_github_item(ui, &item_for_toggle);
                            } else if !url_open.is_empty() {
                                let _ = crate::platform::open_external(&url_open);
                            }
                        })
                        .child(left_dynamic),
                )
                .child(right),
        )
        .child(expanded_preview);
    if let Some(rt) = reply_target {
        wrapper = wrapper.child(inline_reply_box(ui, rt));
    }
    wrapper
}

/// GitHub Activity chapter: flat borderless hairline rows for both Direct & collapsed Watching feeds.
fn render_github_activity(
    ui: Ui,
    direct_stacks: Vec<GitHubProjectStack>,
    watching_stacks: Vec<GitHubProjectStack>,
    now_ms: u64,
) -> Option<View> {
    if direct_stacks.is_empty() && watching_stacks.is_empty() {
        return None;
    }
    let direct_total: usize = direct_stacks.iter().map(|s| s.items.len()).sum();
    let watching_total: usize = watching_stacks.iter().map(|s| s.items.len()).sum();
    let mut sub_parts = Vec::new();
    if direct_total > 0 {
        sub_parts.push(format!("{direct_total} direct"));
    }
    if watching_total > 0 {
        sub_parts.push(format!("{watching_total} watching"));
    }

    let mut sec = view().gap_2p5().child(chapter_header(
        Icon::GitHub,
        TEXT,
        "GitHub Activity",
        sub_parts.join(" · "),
        None,
    ));

    // Direct updates (unfolded, grouped by repo on crisp hairlines)
    for stack in direct_stacks {
        let count = stack.items.len();
        let stack_dismiss = stack.clone();
        let mut repo_block = view().child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .py_1()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            label(stack.display_name.clone())
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(TEXT),
                        )
                        .child(
                            muted(format!("{count}"))
                                .font_family(MONO_FONT)
                                .text_size(px(11.)),
                        ),
                )
                .child(compact_action("Mark read", Variant::Quiet, move |_| {
                    dismiss_github_stack(ui, stack_dismiss.clone())
                })),
        );
        for item in stack.items {
            repo_block = repo_block.child(render_github_row(ui, item, now_ms, false, 0., None));
        }
        sec = sec.child(repo_block);
    }

    // Watching feeds (flat borderless collapsible hairline rows — no outer box border, no explanatory banner)
    if !watching_stacks.is_empty() {
        let mut watch_list = view();
        for stack in watching_stacks {
            let sid = stack.stack_id.clone();
            let sid_toggle = sid.clone();
            let expanded = ui.inbox.expanded_stacks.with(|s| s.contains(&sid));
            let count = stack.items.len();
            let age = format_age_compact(stack.latest_at, now_ms);
            let stack_dismiss = stack.clone();

            watch_list = watch_list.child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .py_2()
                    .border_b_1()
                    .border_color(BORDER)
                    .child(
                        press_target()
                            .flex_1()
                            .min_w(px(0.))
                            .bg(CLEAR)
                            .rounded(px(4.))
                            .hover(|s| s.bg(CARD_HOVER))
                            .on_press(move |_: &()| toggle_stack(ui, sid_toggle.clone()))
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .min_w(px(0.))
                                    .child(
                                        icon_tinted(
                                            if expanded {
                                                Icon::ChevronDown
                                            } else {
                                                Icon::Forward
                                            },
                                            MUTED,
                                        )
                                        .w(px(12.))
                                        .h(px(12.)),
                                    )
                                    .child(muted("Watching").text_size(px(11.5)))
                                    .child(
                                        label(stack.repository.clone())
                                            .font_family(MONO_FONT)
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(TEXT)
                                            .truncate(),
                                    )
                                    .child(
                                        muted(format!("{count}"))
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.))
                                            .flex_1(),
                                    )
                                    .child(
                                        muted(age)
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.))
                                            .flex_shrink_0(),
                                    ),
                            ),
                    )
                    .child(compact_action("Mark read", Variant::Quiet, move |_| {
                        dismiss_github_stack(ui, stack_dismiss.clone())
                    })),
            );

            if expanded {
                for item in stack.items.into_iter().take(20) {
                    watch_list =
                        watch_list.child(render_github_row(ui, item, now_ms, false, 18., None));
                }
            }
        }
        sec = sec.child(watch_list);
    }

    Some(sec)
}

/// Flat borderless Live Agent Session row (no outer box, no bordered tile, structured Git/Turn telemetry).
fn render_agent_session_row(ui: Ui, term: InboxTerminalWork, now_ms: u64) -> View {
    let tid = term.terminal_id.clone();
    let tid_open = tid.clone();
    let tid_close = tid.clone();
    let card_open = term.card_id.clone();
    let title_close = term.title.clone();
    let age = format_age_compact(term.timestamp_ms, now_ms);
    let (status_lbl, status_tone, is_live, status_color) = terminal_status_visual(term.status);

    let state_pill_text = if is_live {
        let elapsed = term
            .started_at
            .map(|s| format_duration_short(s, now_ms))
            .filter(|s| !s.is_empty());
        match elapsed {
            Some(dur) => format!("{status_lbl} · {dur}"),
            None => status_lbl.to_owned(),
        }
    } else if let Some(turn) = term.agent_turn.filter(|&t| t > 0) {
        format!("{status_lbl} · Turn {turn}")
    } else {
        status_lbl.to_owned()
    };

    let tile = agent_glyph_24(term.agent_engine.as_deref(), Some(status_color));

    let title_text = match (term.card_number, &term.card_title) {
        (Some(n), Some(ct)) => format!("{} · #{n} {ct}", term.title),
        _ => term.title.clone(),
    };

    let header = view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .child(
            label(title_text)
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(13.))
                .truncate()
                .flex_1()
                .min_w(px(0.)),
        )
        .child(if is_live {
            live_status(state_pill_text, status_tone)
        } else {
            status(state_pill_text, status_tone)
        })
        .child(
            muted(age)
                .font_family(MONO_FONT)
                .text_size(px(11.))
                .flex_shrink_0(),
        );

    let mut col = view()
        .flex_1()
        .min_w(px(0.))
        .gap_1()
        .child(header)
        .child(render_terminal_git_telemetry(&term));

    if let Some(cmd) = &term.shell_command {
        let suffix = term
            .shell_exit_code
            .map(|c| format!(" (exit {c})"))
            .unwrap_or_default();
        col = col.child(
            muted(format!("{cmd}{suffix}"))
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .truncate(),
        );
    } else if let Some(detail) = &term.status_detail {
        col = col.child(muted(detail.clone()).text_size(px(12.)).truncate());
    }

    view()
        .flex_row()
        .items_center()
        .gap_3()
        .py_2p5()
        .border_b_1()
        .border_color(BORDER)
        .child(
            press_target()
                .flex_1()
                .min_w(px(0.))
                .bg(CLEAR)
                .rounded(px(4.))
                .hover(|s| s.bg(CARD_HOVER))
                .on_press(move |_: &()| {
                    if let Some(cid) = &card_open {
                        ui.open_card_with_terminal(cid.clone(), Some(tid_open.clone()));
                    } else {
                        ui.open_terminal(tid_open.clone());
                    }
                })
                .child(
                    view()
                        .flex_row()
                        .items_start()
                        .gap_2p5()
                        .child(tile)
                        .child(col),
                ),
        )
        .child(compact_action("Close", Variant::Quiet, move |_| {
            close_terminal_from_inbox(ui, tid_close.clone(), title_close.clone())
        }))
}

/// Active Work chapter: borderless hairline Agent Sessions + Today's Cards + Idle Shells.
fn render_active_work(
    ui: Ui,
    agent_terminals: Vec<InboxTerminalWork>,
    open_cards: Vec<InboxCardWork>,
    plain_shells: Vec<InboxTerminalWork>,
    now_ms: u64,
) -> Option<View> {
    if agent_terminals.is_empty() && open_cards.is_empty() && plain_shells.is_empty() {
        return None;
    }
    let mut sub_parts = Vec::new();
    if !agent_terminals.is_empty() {
        let n = agent_terminals.len();
        sub_parts.push(format!("{n} {}", if n == 1 { "agent" } else { "agents" }));
    }
    if !open_cards.is_empty() {
        let n = open_cards.len();
        sub_parts.push(format!("{n} {}", if n == 1 { "card" } else { "cards" }));
    }
    if !plain_shells.is_empty() {
        let n = plain_shells.len();
        sub_parts.push(format!("{n} {}", if n == 1 { "shell" } else { "shells" }));
    }

    let mut sec = view().gap_2p5().child(chapter_header(
        Icon::Terminal,
        ACCENT,
        "Active Work",
        sub_parts.join(" · "),
        None,
    ));

    // 1. Live Agent Sessions (flat borderless hairline list — zero box-in-box)
    if !agent_terminals.is_empty() {
        let mut list = view().border_t_1().border_color(BORDER);
        for term in agent_terminals {
            list = list.child(render_agent_session_row(ui, term, now_ms));
        }
        sec = sec.child(list);
    }

    // 2. Today's Cards (hairline-ruled table)
    if !open_cards.is_empty() {
        let count = open_cards.len();
        let mut table = view().child(
            view().py_1p5().border_b_1().border_color(BORDER).child(
                muted(format!("TODAY'S CARDS ({count})"))
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD),
            ),
        );
        for card in open_cards {
            let age = format_age_compact(card.last_activity_at, now_ms);
            let cid_open = card.card_id.clone();
            let cid_resolve = card.card_id.clone();
            let key_defer = card.key.clone();
            let key_dismiss = card.key.clone();
            let wm = card.watermark;
            let label_title = format!("#{} {}", card.card_number, card.card_title);
            let label_dismiss = label_title.clone();

            let mut row_left = view()
                .flex_row()
                .items_center()
                .gap_2p5()
                .min_w(px(0.))
                .child(
                    muted(format!("#{}", card.card_number))
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .w(px(46.))
                        .flex_shrink_0(),
                )
                .child(
                    label(card.card_title.clone())
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .flex_1()
                        .min_w(px(0.)),
                )
                .child(
                    muted(card.project_name.clone())
                        .text_size(px(11.5))
                        .flex_shrink_0(),
                )
                .child(
                    muted(card.stage.clone())
                        .text_size(px(11.5))
                        .flex_shrink_0(),
                );
            if let Some(pr) = card.pr_summary {
                let tone = match card.pr_ci_state.as_deref() {
                    Some("failure" | "error") => Tone::Danger,
                    Some("success") => Tone::Success,
                    _ => Tone::Neutral,
                };
                row_left = row_left.child(status(pr, tone));
            }
            if let Some(h) = card.host_label {
                row_left = row_left.child(muted(h).font_family(MONO_FONT).text_size(px(11.)));
            }
            row_left = row_left.child(
                muted(age)
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .flex_shrink_0(),
            );

            table = table.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .py_2()
                    .border_b_1()
                    .border_color(BORDER)
                    .child(
                        press_target()
                            .flex_1()
                            .min_w(px(0.))
                            .bg(CLEAR)
                            .rounded(px(4.))
                            .hover(|s| s.bg(CARD_HOVER))
                            .on_press(move |_: &()| ui.open_card(cid_open.clone()))
                            .child(row_left),
                    )
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(compact_action("Resolve", Variant::Quiet, move |_| {
                                if let Some(c) = ui.snapshot.with_untracked(|s| {
                                    s.cards.iter().find(|x| x.id == cid_resolve).cloned()
                                }) {
                                    dispatch(
                                        ui,
                                        "card.resolve",
                                        json!({ "card_id": c.id, "revision": c.revision }),
                                    );
                                    ui.notify(format!("Resolved #{}", c.number));
                                }
                            }))
                            .child(chrome_button(Icon::Clock, "Defer", move |_| {
                                open_defer_menu(
                                    ui,
                                    vec![(key_defer.clone(), wm)],
                                    label_title.clone(),
                                )
                            }))
                            .child(chrome_button(Icon::Close, "Hide", move |_| {
                                dismiss_items(
                                    ui,
                                    vec![(key_dismiss.clone(), wm)],
                                    format!("Hidden · {label_dismiss}"),
                                )
                            })),
                    ),
            );
        }
        sec = sec.child(table);
    }

    // 3. Borderless Idle Shells collapsible row
    if !plain_shells.is_empty() {
        let stack_id = "plain-shells-stack".to_owned();
        let expanded = ui.inbox.expanded_stacks.with(|s| s.contains(&stack_id));
        let count = plain_shells.len();
        let shells_close_all = plain_shells.clone();
        let preview = plain_shells
            .iter()
            .take(3)
            .map(|s| {
                s.cwd
                    .as_deref()
                    .map(shorten_path)
                    .unwrap_or_else(|| s.host_label.clone())
            })
            .collect::<Vec<_>>()
            .join(", ");

        let mut band = view().child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .py_2()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    press_target()
                        .flex_1()
                        .min_w(px(0.))
                        .bg(CLEAR)
                        .rounded(px(4.))
                        .hover(|s| s.bg(CARD_HOVER))
                        .on_press(move |_: &()| toggle_stack(ui, stack_id.clone()))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .min_w(px(0.))
                                .child(
                                    icon_tinted(
                                        if expanded {
                                            Icon::ChevronDown
                                        } else {
                                            Icon::Forward
                                        },
                                        MUTED,
                                    )
                                    .w(px(12.))
                                    .h(px(12.)),
                                )
                                .child(
                                    label(format!("Idle shells · {count}"))
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(MUTED),
                                )
                                .child(
                                    muted(preview)
                                        .font_family(MONO_FONT)
                                        .text_size(px(11.5))
                                        .truncate()
                                        .flex_1(),
                                ),
                        ),
                )
                .child(compact_action("Close all", Variant::Quiet, move |_| {
                    close_all_plain_shells(ui, shells_close_all.clone())
                })),
        );

        if expanded {
            for sh in plain_shells {
                let tid_open = sh.terminal_id.clone();
                let tid_close = sh.terminal_id.clone();
                let title_close = sh.title.clone();
                let age = format_age_compact(sh.timestamp_ms, now_ms);
                let cwd = sh
                    .cwd
                    .as_deref()
                    .map(shorten_path)
                    .unwrap_or_else(|| "—".into());
                band = band.child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .pl(px(20.))
                        .py_1p5()
                        .border_b_1()
                        .border_color(BORDER)
                        .child(
                            press_target()
                                .flex_1()
                                .min_w(px(0.))
                                .bg(CLEAR)
                                .rounded(px(4.))
                                .hover(|s| s.bg(CARD_HOVER))
                                .on_press(move |_: &()| ui.open_terminal(tid_open.clone()))
                                .child(
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .min_w(px(0.))
                                        .child(label(sh.title.clone()).text_size(px(12.)))
                                        .child(
                                            muted(cwd)
                                                .font_family(MONO_FONT)
                                                .text_size(px(11.5))
                                                .truncate()
                                                .flex_1(),
                                        )
                                        .child(
                                            muted(age)
                                                .font_family(MONO_FONT)
                                                .text_size(px(11.))
                                                .flex_shrink_0(),
                                        ),
                                ),
                        )
                        .child(chrome_button(Icon::Close, "Close", move |_| {
                            close_terminal_from_inbox(ui, tid_close.clone(), title_close.clone())
                        })),
                );
            }
        }
        sec = sec.child(band);
    }

    Some(sec)
}

/// Borderless bottom fold for older GitHub items (< today) & carry-over cards.
fn render_earlier_section(
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
    let mut parts = Vec::new();
    if gh_count > 0 {
        parts.push(format!("{gh_count} GitHub"));
    }
    if card_count > 0 {
        parts.push(format!(
            "{card_count} {}",
            if card_count == 1 { "card" } else { "cards" }
        ));
    }

    let gh_clear_all = earlier_github.clone();
    let mut band = view().child(
        view()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .py_2()
            .border_t_1()
            .border_b_1()
            .border_color(BORDER)
            .child(
                press_target()
                    .flex_1()
                    .min_w(px(0.))
                    .bg(CLEAR)
                    .rounded(px(4.))
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
                                .w(px(12.))
                                .h(px(12.)),
                            )
                            .child(
                                label("Earlier")
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(MUTED),
                            )
                            .child(muted(format!("· {}", parts.join(" · "))).text_size(px(11.5))),
                    ),
            )
            .child(if gh_count > 0 {
                compact_action("Mark read", Variant::Quiet, move |_| {
                    let mut keys = Vec::new();
                    for item in &gh_clear_all {
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
                    dismiss_items(ui, keys, format!("Cleared {n} GitHub items"));
                })
                .into_element()
            } else {
                view().into_element()
            }),
    );

    if expanded {
        for item in earlier_github.into_iter().take(20) {
            band = band.child(render_github_row(
                ui,
                item,
                now_ms,
                true,
                16.,
                Some(day_start_ms),
            ));
        }
        for c in carryover.into_iter().take(20) {
            let age = format_age_compact(c.last_activity_at, now_ms);
            let cid = c.card_id.clone();
            let key_pin = c.key.clone();
            let wm = c.watermark;
            band = band.child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .pl(px(16.))
                    .py_1p5()
                    .border_b_1()
                    .border_color(BORDER)
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
                                    .child(
                                        muted(format!("#{}", c.card_number))
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.5))
                                            .w(px(44.))
                                            .flex_shrink_0(),
                                    )
                                    .child(
                                        label(c.card_title.clone())
                                            .text_size(px(12.))
                                            .truncate()
                                            .flex_1()
                                            .min_w(px(0.)),
                                    )
                                    .child(muted(c.project_name.clone()).text_size(px(11.5)))
                                    .child(
                                        muted(age)
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.))
                                            .flex_shrink_0(),
                                    ),
                            ),
                    )
                    .child(compact_action("Today", Variant::Quiet, move |_| {
                        pin_to_today(ui, key_pin.clone(), day_start_ms, wm)
                    })),
            );
        }
    }

    Some(band)
}

fn render_inbox_zero(ui: Ui, inbox: &DailyInbox) -> View {
    let done = inbox.completed_today;
    let deferred = inbox.deferred_items.len();
    let mut actions = view().flex_row().items_center().gap_2().pt_2();
    if deferred > 0 {
        actions = actions.child(action(
            format!("Deferred ({deferred})"),
            Variant::Secondary,
            move |_| ui.inbox.drawer.set("deferred".into()),
        ));
    }
    actions = actions.child(action("Open Board", Variant::Quiet, move |_| {
        ui.navigate("board");
    }));

    view()
        .py(px(44.))
        .px_6()
        .items_center()
        .gap_2()
        .child(glyph_24(Icon::Check, SUCCESS_SURFACE, SUCCESS))
        .child(
            label("All clear")
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(15.)),
        )
        .child(muted(format!("{done} done · {deferred} deferred")).text_size(px(12.5)))
        .child(actions)
}

fn render_deferred_drawer(ui: Ui, items: Vec<DeferredInboxItem>) -> View {
    if items.is_empty() {
        return view()
            .py(px(44.))
            .items_center()
            .gap_2()
            .child(muted("No deferred items"))
            .child(action("Today", Variant::Secondary, move |_| {
                ui.inbox.drawer.set("today".into())
            }));
    }
    let mut list = view().border_t_1().border_color(BORDER);
    for item in items {
        let key_restore = item.key.clone();
        let cid_opt = item.card_id.clone();
        let tid_opt = item.terminal_id.clone();
        let url_opt = item.url.clone();
        let mut row = view()
            .py_2p5()
            .border_b_1()
            .border_color(BORDER)
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
        return view()
            .py(px(44.))
            .items_center()
            .gap_2()
            .child(muted("No completed items"))
            .child(action("Today", Variant::Secondary, move |_| {
                ui.inbox.drawer.set("today".into())
            }));
    }
    let mut list = view().border_t_1().border_color(BORDER);
    for item in items {
        let age = format_age_compact(item.completed_at, now_ms);
        let key_restore = item.key.clone();
        let cid_opt = item.card_id.clone();
        let mut row = view()
            .py_2p5()
            .border_b_1()
            .border_color(BORDER)
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
            .child(
                muted(age)
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .flex_shrink_0(),
            );
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
                view().w_full().items_center().px_6().py_5().child(
                    view()
                        .w_full()
                        .max_w(px(900.))
                        .gap(px(28.))
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
                                return render_done_drawer(
                                    ui,
                                    inbox.done_today_items.clone(),
                                    now_ms,
                                )
                                .into_element();
                            }

                            let mut col = view().gap(px(28.));

                            // 1. Editorial Masthead + Borderless 4-Column Hairline KPI Strip
                            col = col.child(render_editorial_masthead(ui, &inbox));

                            if !gh_connected.get()
                                && inbox.github_stacks.is_empty()
                                && inbox.github_watching.is_empty()
                                && inbox.earlier_github.is_empty()
                            {
                                col = col.child(
                                    view()
                                        .py_2()
                                        .border_b_1()
                                        .border_color(BORDER)
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
                                                    muted("GitHub disconnected").text_size(px(12.)),
                                                ),
                                        )
                                        .child(compact_action(
                                            "Connect →",
                                            Variant::Quiet,
                                            move |_| ui.navigate("github-account"),
                                        )),
                                );
                            }

                            let all_today_empty = inbox.needs_you.is_empty()
                                && inbox.github_stacks.is_empty()
                                && inbox.github_watching.is_empty()
                                && inbox.schedule_stacks.is_empty()
                                && inbox.schedule_pulse.is_empty()
                                && inbox.open_cards.is_empty()
                                && inbox.agent_terminals.is_empty()
                                && inbox.plain_shells.is_empty();

                            if all_today_empty {
                                col = col.child(render_inbox_zero(ui, &inbox));
                            } else {
                                // 2. Needs Action (flat hairline list with 3px left semantic bar)
                                if !inbox.needs_you.is_empty() {
                                    let count = inbox.needs_you.len();
                                    let mut sec = view().gap_2().child(chapter_header(
                                        Icon::ShieldAlert,
                                        ATTENTION,
                                        "Needs Action",
                                        format!("{count}"),
                                        None,
                                    ));
                                    let mut list = view().border_t_1().border_color(BORDER);
                                    for item in inbox.needs_you.iter().cloned() {
                                        list = list.child(render_needs_you_row(ui, item, now_ms));
                                    }
                                    sec = sec.child(list);
                                    col = col.child(sec);
                                }

                                // 3. Scheduled Runs
                                if let Some(sched) = render_schedules_report(
                                    ui,
                                    &inbox.schedule_pulse,
                                    inbox.schedule_stacks.clone(),
                                ) {
                                    col = col.child(sched);
                                }

                                // 4. GitHub Activity (Direct + Watching)
                                if let Some(gh) = render_github_activity(
                                    ui,
                                    inbox.github_stacks.clone(),
                                    inbox.github_watching.clone(),
                                    now_ms,
                                ) {
                                    col = col.child(gh);
                                }

                                // 5. Active Work (Agent Sessions + Today's Cards + Idle Shells)
                                if let Some(work) = render_active_work(
                                    ui,
                                    inbox.agent_terminals.clone(),
                                    inbox.open_cards.clone(),
                                    inbox.plain_shells.clone(),
                                    now_ms,
                                ) {
                                    col = col.child(work);
                                }
                            }

                            // 6. Earlier fold
                            if let Some(earlier) = render_earlier_section(
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
