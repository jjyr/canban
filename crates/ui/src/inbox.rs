//! Daily Briefing: substantial, single-layer editorial engineering report with
//! clickable 4-column decision KPI strip (`WAITING ON YOU | READY TO MERGE | AGENTS | SHIPPED TODAY`),
//! live hosts/quota pulse, active project shortcuts, 2-column borderless `SURFACE` Agent cards,
//! toggleable Markdown GitHub PR reader with colored PR state/CI/comments/avatars,
//! upcoming schedules, Shipped Today log, floating auto-dismiss Undo toast, and First-Run launchpad.
pub mod state;
pub use state::{
    GitHubItemDetail, State, cutoff_minutes_from_utility, defer_remaining_all, init,
    local_tz_offset_secs,
};

use crate::presenter::*;
use canban_notifications::{
    ActiveWorkProjectGroup, DailyInbox, DeferredInboxItem, DoneInboxItem, GitHubProjectStack,
    GitHubStackItem, InboxCardWork, InboxTerminalWork, NeedsYouItem, NeedsYouKind,
    ProjectPulseItem, ScheduleCardStack, SchedulePulseItem, ShippedTodayItem, UpcomingScheduleItem,
};
use canban_types::HerdrStatus;
use canban_ui_kit::{compact_home, tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;
use state::*;

pub(crate) fn engine_icon(engine: Option<&str>) -> Icon {
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

pub(crate) fn format_duration_short(since_ms: u64, now_ms: u64) -> String {
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

pub(crate) fn format_age_compact(ts_ms: u64, now_ms: u64) -> String {
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
pub(crate) fn github_avatar(login: &str, avatar_url: Option<&str>, size: f32) -> View {
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

/// Colored semantic PR/Issue state mark (`Open` green, `Merged` purple, `Draft` muted, `Closed` danger).
pub(crate) fn gh_state_mark(
    kind: &str,
    pr_state: Option<&str>,
    merged: bool,
    draft: bool,
) -> AnyElement {
    if kind == "pr" {
        if merged || pr_state.is_some_and(|s| s.eq_ignore_ascii_case("merged")) {
            return icon_tinted(Icon::GitMerge, HIGHLIGHT)
                .w(px(14.))
                .h(px(14.))
                .flex_shrink_0()
                .into_element();
        }
        if draft || pr_state.is_some_and(|s| s.eq_ignore_ascii_case("draft")) {
            return icon_tinted(Icon::GitMerge, MUTED)
                .w(px(14.))
                .h(px(14.))
                .flex_shrink_0()
                .into_element();
        }
        if pr_state.is_some_and(|s| s.eq_ignore_ascii_case("closed")) {
            return icon_tinted(Icon::Close, DANGER)
                .w(px(13.))
                .h(px(13.))
                .flex_shrink_0()
                .into_element();
        }
        return icon_tinted(Icon::GitMerge, SUCCESS)
            .w(px(14.))
            .h(px(14.))
            .flex_shrink_0()
            .into_element();
    }
    let tint = if pr_state.is_some_and(|s| s.eq_ignore_ascii_case("closed")) {
        HIGHLIGHT
    } else {
        ACCENT
    };
    icon_tinted(Icon::GitHub, tint)
        .w(px(13.))
        .h(px(13.))
        .flex_shrink_0()
        .into_element()
}

/// Compact inline CI indicator (`✓ 12/12`, `✗ 2/12`, `● 3/12`).
pub(crate) fn ci_indicator(
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
pub(crate) fn comments_indicator(count: u32) -> View {
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

/// Borderless icon glyph (28×28 soft surface).
fn glyph_28(kind: Icon, bg: ThemeColor, fg: ThemeColor) -> View {
    view()
        .flex_shrink_0()
        .w(px(28.))
        .h(px(28.))
        .rounded(px(7.))
        .bg(bg)
        .items_center()
        .justify_center()
        .pointer_events(PointerEvents::None)
        .child(icon_tinted(kind, fg).w(px(15.)).h(px(15.)))
}

/// Borderless full-color Agent provider icon with optional status dot.
fn agent_glyph_28(engine: Option<&str>, dot: Option<ThemeColor>) -> View {
    let glyph = if let Some(eng) = engine {
        icon_colored(engine_icon(Some(eng)), 20.).into_element()
    } else {
        icon_tinted(Icon::Terminal, MUTED)
            .w(px(16.))
            .h(px(16.))
            .into_element()
    };
    let mut box_v = view()
        .relative()
        .flex_shrink_0()
        .w(px(28.))
        .h(px(28.))
        .rounded(px(7.))
        .bg(INSET)
        .items_center()
        .justify_center()
        .pointer_events(PointerEvents::None)
        .child(glyph);
    if let Some(color) = dot {
        box_v = box_v.child(
            view()
                .absolute()
                .right(px(-1.))
                .bottom(px(-1.))
                .w(px(9.))
                .h(px(9.))
                .rounded_full()
                .bg(SURFACE)
                .items_center()
                .justify_center()
                .child(view().w(px(6.)).h(px(6.)).rounded_full().bg(color)),
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
        .child(icon_tinted(kind, tint).w(px(15.)).h(px(15.)))
        .child(
            label(title.into())
                .text_size(px(14.))
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
                            move |_| {
                                drawer.set("today".into());
                                ui.inbox.focus_filter.set(String::new());
                            },
                        ))
                        .child(compact_segment(
                            deferred_lbl,
                            None,
                            move || drawer.with(|d| d == "deferred"),
                            move |_| {
                                drawer.set("deferred".into());
                                ui.inbox.focus_filter.set(String::new());
                            },
                        ))
                        .child(compact_segment(
                            done_lbl,
                            None,
                            move || drawer.with(|d| d == "done"),
                            move |_| {
                                drawer.set("done".into());
                                ui.inbox.focus_filter.set(String::new());
                            },
                        ))
                        .into_element()
                }))
                .child(chrome_button(Icon::Refresh, "Refresh GitHub", move |_| {
                    refresh_github_notifications(ui, true)
                })),
        )
}

/// Compact live hosts + AI quota strip for the top-right Masthead.
fn render_environment_pulse(ui: Ui) -> View {
    let hosts = ui.hosts.get();
    let mut host_row = view().flex_row().items_center().gap_2p5();
    if hosts.is_empty() {
        host_row = host_row.child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(status_dot(SUCCESS))
                .child(muted("local").font_family(MONO_FONT).text_size(px(11.5))),
        );
    } else {
        for h in hosts.iter().take(4) {
            let ok = h.is_local || h.connection.is_connected();
            let dot = if ok { SUCCESS } else { DANGER };
            let name = if h.is_local {
                "local".to_owned()
            } else {
                h.name.clone()
            };
            host_row = host_row.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(status_dot(dot))
                    .child(
                        muted(name)
                            .font_family(MONO_FONT)
                            .text_size(px(11.5))
                            .text_color(if ok { MUTED } else { DANGER }),
                    ),
            );
        }
    }

    let usage_val = ui.usage.get();
    let readings = crate::utilities::readings(&usage_val);
    let mut quota_parts = Vec::new();
    for (prov, lbl) in [("claude", "Claude"), ("codex", "Codex")] {
        let max_used = readings
            .iter()
            .filter(|r| r["provider"] == prov)
            .flat_map(|r| r["windows"].as_array().into_iter().flatten())
            .filter_map(|w| w["used"].as_f64())
            .reduce(f64::max);
        if let Some(pct) = max_used {
            quota_parts.push(format!("{lbl} {:.0}%", pct.clamp(0.0, 100.0)));
        }
    }

    let mut col = view().items_end().gap_0p5().child(host_row);
    if !quota_parts.is_empty() {
        col = col.child(
            muted(quota_parts.join(" · "))
                .font_family(MONO_FONT)
                .text_size(px(11.)),
        );
    }
    col
}

pub(crate) struct ReadyMergeSummary {
    pub needs_you_items: Vec<NeedsYouItem>,
    pub open_cards: Vec<InboxCardWork>,
    pub gh_items: Vec<GitHubStackItem>,
    pub refs: Vec<String>,
}

impl ReadyMergeSummary {
    pub fn total(&self) -> usize {
        self.needs_you_items.len() + self.open_cards.len() + self.gh_items.len()
    }
}

pub(crate) fn toggle_focus_filter(ui: Ui, key: &'static str) {
    ui.inbox.focus_filter.update(|cur| {
        if cur == key {
            cur.clear();
        } else {
            *cur = key.to_owned();
        }
    });
}

pub(crate) fn collect_ready_to_merge(
    inbox: &DailyInbox,
    gh_details: &std::collections::BTreeMap<String, GitHubItemDetail>,
) -> ReadyMergeSummary {
    let mut needs_you_items = Vec::new();
    let mut open_cards = Vec::new();
    let mut gh_items = Vec::new();
    let mut refs = Vec::new();
    let mut seen_card_ids = std::collections::BTreeSet::<String>::new();
    let mut seen_pr_keys = std::collections::BTreeSet::<(String, u64)>::new();

    for item in &inbox.needs_you {
        if let NeedsYouKind::Card {
            card_id,
            card_number,
            pr_number,
            can_quick_merge: true,
            ..
        } = &item.kind
        {
            seen_card_ids.insert(card_id.clone());
            if let Some(n) = pr_number {
                refs.push(format!("#{n}"));
            } else {
                refs.push(format!("#{card_number}"));
            }
            needs_you_items.push(item.clone());
        }
    }

    for card in &inbox.open_cards {
        if card.stage == "review"
            && card.pr_ci_state.as_deref() == Some("success")
            && seen_card_ids.insert(card.card_id.clone())
        {
            refs.push(format!("#{}", card.card_number));
            open_cards.push(card.clone());
        }
    }

    let is_pr_green_open = |item: &GitHubStackItem| -> bool {
        if item.kind != "pr" {
            return false;
        }
        let detail = gh_details.get(&item.key);
        let is_open = if let Some(d) = detail {
            !d.merged && !d.draft && d.state.eq_ignore_ascii_case("open")
        } else {
            !matches!(
                item.pr_state.as_deref(),
                Some("merged") | Some("closed") | Some("draft")
            )
        };
        if !is_open {
            return false;
        }
        let ci = detail
            .and_then(|d| d.ci_state.as_deref())
            .or(item.ci_state.as_deref());
        ci.is_some_and(|s| s.eq_ignore_ascii_case("success"))
    };

    let mut maybe_push_gh = |item: &GitHubStackItem| {
        if !is_pr_green_open(item) {
            return;
        }
        if let Some(cid) = &item.card_id
            && seen_card_ids.contains(cid)
        {
            return;
        }
        if let Some(n) = item.number {
            let k = (item.repository.to_ascii_lowercase(), n);
            if !seen_pr_keys.insert(k) {
                return;
            }
            refs.push(format!("#{n}"));
        }
        if let Some(cid) = &item.card_id {
            seen_card_ids.insert(cid.clone());
        }
        gh_items.push(item.clone());
    };

    for item in &inbox.needs_you {
        if let NeedsYouKind::GitHub { item: gh, .. } = &item.kind {
            maybe_push_gh(gh);
        }
    }
    for stack in &inbox.github_stacks {
        for item in &stack.items {
            maybe_push_gh(item);
        }
    }

    ReadyMergeSummary {
        needs_you_items,
        open_cards,
        gh_items,
        refs,
    }
}

/// Editorial Masthead + clickable 4-column decision KPI strip
/// (`WAITING ON YOU | READY TO MERGE | AGENTS | SHIPPED TODAY`).
fn render_editorial_masthead(
    ui: Ui,
    inbox: &DailyInbox,
    ready: &ReadyMergeSummary,
    active_focus: &str,
    now_ms: u64,
) -> View {
    let rem = inbox.remaining_today;

    // KPI #1: Waiting On You (1:1 with `needs_you` + oldest waiting age)
    let mut card_n = 0usize;
    let mut term_n = 0usize;
    let mut gh_n = 0usize;
    let mut sched_fail_n = 0usize;
    let mut oldest_wait_ts = u64::MAX;
    for item in &inbox.needs_you {
        if item.timestamp_ms > 0 && item.timestamp_ms < now_ms {
            oldest_wait_ts = oldest_wait_ts.min(item.timestamp_ms);
        }
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
    let k1_extra = if inbox.actionable_count > 0 && oldest_wait_ts < now_ms {
        let dur = format_duration_short(oldest_wait_ts, now_ms);
        if !dur.is_empty() {
            let stale = now_ms.saturating_sub(oldest_wait_ts) >= 3_600_000;
            Some(
                label(format!("oldest {dur}"))
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .text_color(if stale { ATTENTION } else { MUTED })
                    .into_element(),
            )
        } else {
            None
        }
    } else {
        None
    };

    // KPI #2: Ready to Merge (CI green PRs & quick-merge Cards)
    let ready_n = ready.total();
    let k2_val = ready_n.to_string();
    let k2_extra = (ready_n > 0).then(|| {
        label("CI green")
            .font_family(MONO_FONT)
            .text_size(px(11.))
            .text_color(SUCCESS)
            .into_element()
    });
    let k2_sub = if ready.refs.is_empty() {
        "—".to_owned()
    } else if ready.refs.len() <= 3 {
        ready.refs.join(" · ")
    } else {
        format!("{} +{}", ready.refs[..2].join(" · "), ready.refs.len() - 2)
    };

    // KPI #3: Agents (running vs waiting + per-machine distribution)
    let mut seen_agent_ids = std::collections::BTreeSet::<String>::new();
    let mut running_n = 0usize;
    let mut waiting_n = 0usize;
    let mut error_n = 0usize;
    let mut idle_n = 0usize;
    let mut by_host = std::collections::BTreeMap::<String, usize>::new();

    let mut count_agent = |t: &InboxTerminalWork| {
        if !t.is_agent || !seen_agent_ids.insert(t.terminal_id.clone()) {
            return;
        }
        match t.status {
            HerdrStatus::Working | HerdrStatus::Starting | HerdrStatus::Queued => running_n += 1,
            HerdrStatus::Blocked => waiting_n += 1,
            HerdrStatus::Error | HerdrStatus::Disconnected => error_n += 1,
            _ => idle_n += 1,
        }
        *by_host.entry(t.host_label.clone()).or_default() += 1;
    };
    for item in &inbox.needs_you {
        if let NeedsYouKind::Terminal { terminal } = &item.kind {
            count_agent(terminal);
        }
    }
    for t in &inbox.agent_terminals {
        count_agent(t);
    }

    let total_agents = running_n + waiting_n + error_n + idle_n;
    let (k3_val, k3_color) = if running_n > 0 {
        (format!("{running_n} running"), ACCENT)
    } else if waiting_n > 0 {
        (format!("{waiting_n} waiting"), ATTENTION)
    } else if error_n > 0 {
        (format!("{error_n} error"), DANGER)
    } else if total_agents > 0 {
        (format!("{total_agents} idle"), TEXT)
    } else {
        ("0".to_owned(), MUTED)
    };
    let k3_extra = if running_n > 0 && waiting_n > 0 {
        Some(
            label(format!("{waiting_n} wait"))
                .font_family(MONO_FONT)
                .text_size(px(11.))
                .text_color(ATTENTION)
                .into_element(),
        )
    } else if error_n > 0 && (running_n > 0 || waiting_n > 0) {
        Some(
            label(format!("{error_n} err"))
                .font_family(MONO_FONT)
                .text_size(px(11.))
                .text_color(DANGER)
                .into_element(),
        )
    } else if running_n > 0 && idle_n > 0 {
        Some(
            label(format!("{idle_n} idle"))
                .font_family(MONO_FONT)
                .text_size(px(11.))
                .text_color(MUTED)
                .into_element(),
        )
    } else {
        None
    };

    let k3_sub = if !by_host.is_empty() {
        let mut hosts: Vec<(String, usize)> = by_host.into_iter().collect();
        hosts.sort_by(|a, b| {
            let a_local = a.0 == "local";
            let b_local = b.0 == "local";
            b_local
                .cmp(&a_local)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.0.cmp(&b.0))
        });
        hosts
            .into_iter()
            .take(3)
            .map(|(h, c)| format!("{h} {c}"))
            .collect::<Vec<_>>()
            .join(" · ")
    } else if !inbox.plain_shells.is_empty() {
        let s = inbox.plain_shells.len();
        format!("{s} {}", if s == 1 { "shell" } else { "shells" })
    } else {
        "—".to_owned()
    };

    // KPI #4: Shipped Today (merged PRs + resolved cards + 7D daily average pace)
    let shipped_prs = inbox.shipped_today.iter().filter(|s| s.is_pr).count();
    let shipped_cards = inbox.shipped_today.iter().filter(|s| !s.is_pr).count();
    let shipped_total = inbox.shipped_today.len();
    let k4_val = shipped_total.to_string();
    let k4_extra = (inbox.shipped_prev_7d_total > 0).then(|| {
        let avg = inbox.shipped_prev_7d_total as f64 / 7.0;
        label(format!("avg {avg:.1}/d"))
            .font_family(MONO_FONT)
            .text_size(px(11.))
            .text_color(MUTED)
            .into_element()
    });
    let mut k4_parts = Vec::new();
    if shipped_prs > 0 {
        k4_parts.push(format!(
            "{shipped_prs} {} merged",
            if shipped_prs == 1 { "PR" } else { "PRs" }
        ));
    }
    if shipped_cards > 0 {
        k4_parts.push(format!(
            "{shipped_cards} {}",
            if shipped_cards == 1 { "card" } else { "cards" }
        ));
    }
    let k4_sub = if k4_parts.is_empty() {
        "—".to_owned()
    } else {
        k4_parts.join(" · ")
    };

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
    if !active_focus.is_empty() {
        meta_line = meta_line.child(compact_action("Show all", Variant::Quiet, move |_| {
            ui.inbox.focus_filter.set(String::new())
        }));
    }

    let mut right_bar = view()
        .flex_row()
        .items_center()
        .gap_3()
        .child(render_environment_pulse(ui));
    if rem > 0 {
        right_bar = right_bar.child(compact_action_with_icon(
            Icon::Clock,
            "Defer rest ▾",
            Variant::Quiet,
            move |_| defer_remaining_all(ui),
        ));
    }

    let masthead = view()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_4()
        .child(
            view()
                .gap_0p5()
                .child(
                    label(format_today_date_long())
                        .text_size(px(22.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(TEXT),
                )
                .child(meta_line),
        )
        .child(right_bar);

    let v_sep = || view().w(px(1.)).my_1p5().bg(BORDER).flex_shrink_0();

    let kpi_cell = |heading_lbl: &'static str,
                    value: String,
                    val_color: ThemeColor,
                    sub: String,
                    inline_extra: Option<AnyElement>,
                    selected: bool,
                    on_click: Box<dyn Fn()>| {
        let mut val_row = view().flex_row().items_baseline().gap_2().child(
            label(value)
                .font_family(MONO_FONT)
                .text_size(px(21.))
                .font_weight(FontWeight::BOLD)
                .text_color(val_color),
        );
        if let Some(el) = inline_extra {
            val_row = val_row.child(el);
        }
        press_target()
            .flex_1()
            .min_w(px(0.))
            .px_2p5()
            .py_2()
            .rounded(px(7.))
            .bg(if selected { SELECTED } else { CLEAR })
            .hover(|s| s.bg(CARD_HOVER))
            .accessibility_label(heading_lbl)
            .on_press(move |_: &()| on_click())
            .child(
                view()
                    .w_full()
                    .min_w(px(0.))
                    .gap_0p5()
                    .child(
                        label(heading_lbl)
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if selected { ACCENT } else { MUTED }),
                    )
                    .child(val_row)
                    .child(
                        muted(sub)
                            .font_family(MONO_FONT)
                            .text_size(px(11.))
                            .truncate(),
                    ),
            )
    };

    let kpi_strip = view()
        .flex_row()
        .items_stretch()
        .gap_2()
        .py_2()
        .border_t_1()
        .border_b_1()
        .border_color(BORDER)
        .child(kpi_cell(
            "WAITING ON YOU",
            inbox.actionable_count.to_string(),
            if inbox.actionable_count > 0 {
                ATTENTION
            } else {
                SUCCESS
            },
            k1_sub,
            k1_extra,
            active_focus == "waiting",
            Box::new(move || toggle_focus_filter(ui, "waiting")),
        ))
        .child(v_sep())
        .child(kpi_cell(
            "READY TO MERGE",
            k2_val,
            if ready_n > 0 { SUCCESS } else { TEXT },
            k2_sub,
            k2_extra,
            active_focus == "merge",
            Box::new(move || toggle_focus_filter(ui, "merge")),
        ))
        .child(v_sep())
        .child(kpi_cell(
            "AGENTS",
            k3_val,
            k3_color,
            k3_sub,
            k3_extra,
            active_focus == "agents",
            Box::new(move || toggle_focus_filter(ui, "agents")),
        ))
        .child(v_sep())
        .child(kpi_cell(
            "SHIPPED TODAY",
            k4_val,
            if shipped_total > 0 { SUCCESS } else { TEXT },
            k4_sub,
            k4_extra,
            active_focus == "shipped",
            Box::new(move || {
                if shipped_total > 0 {
                    toggle_focus_filter(ui, "shipped");
                } else {
                    ui.navigate("insights");
                }
            }),
        ));

    view().gap_3().child(masthead).child(kpi_strip)
}

/// Active Projects quick-jump strip (single-layer borderless `SURFACE` pills).
fn render_project_pulses(ui: Ui, pulses: &[ProjectPulseItem]) -> Option<View> {
    if pulses.is_empty() {
        return None;
    }
    let mut row = view().flex_row().flex_wrap().items_center().gap_2().child(
        muted("PROJECTS")
            .text_size(px(10.5))
            .font_weight(FontWeight::SEMIBOLD),
    );
    for p in pulses.iter().take(6) {
        let pid = p.project_id.clone();
        let dot_color = if p.attention_count > 0 {
            ATTENTION
        } else if p.live_agent_count > 0 {
            ACCENT
        } else if p.merged_today_count > 0 {
            SUCCESS
        } else {
            MUTED
        };
        let mut metrics = Vec::new();
        for sc in &p.stage_counts {
            if sc.count > 0 {
                metrics.push(format!("{} {}", sc.count, sc.stage_name));
            }
        }
        if p.open_pr_count > 0 {
            metrics.push(format!(
                "{} {}",
                p.open_pr_count,
                if p.open_pr_count == 1 { "PR" } else { "PRs" }
            ));
        }
        if p.live_agent_count > 0 {
            metrics.push(format!(
                "{} {}",
                p.live_agent_count,
                if p.live_agent_count == 1 {
                    "agent"
                } else {
                    "agents"
                }
            ));
        }
        if metrics.is_empty() && p.merged_today_count > 0 {
            metrics.push(format!("{} shipped", p.merged_today_count));
        }
        let mut pill = view()
            .flex_row()
            .items_center()
            .gap_1p5()
            .px_2p5()
            .py_1()
            .child(status_dot(dot_color))
            .child(
                label(p.project_name.clone())
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(TEXT),
            );
        if !metrics.is_empty() {
            pill = pill.child(
                muted(metrics.join(" · "))
                    .font_family(MONO_FONT)
                    .text_size(px(11.)),
            );
        }
        row = row.child(
            press_target()
                .bg(SURFACE)
                .rounded(px(7.))
                .hover(|s| s.bg(CARD_HOVER))
                .on_press(move |_: &()| {
                    ui.project.set(pid.clone());
                    ui.navigate("board");
                })
                .child(pill),
        );
    }
    Some(row)
}

/// Floating bottom-center Undo toast (auto-dismisses after 4s via root Owner timer).
fn render_floating_undo_toast(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let Some(banner) = ui.inbox.undo_banner.get() else {
            return view().into_element();
        };
        view()
            .absolute()
            .bottom(px(20.))
            .left(px(0.))
            .right(px(0.))
            .items_center()
            .pointer_events(PointerEvents::None)
            .child(
                view()
                    .pointer_events(PointerEvents::Auto)
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_3p5()
                    .py_2()
                    .bg(SURFACE)
                    .border_1()
                    .border_color(ACCENT)
                    .rounded(px(9.))
                    .shadow_lg()
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
                            .gap_1()
                            .child(compact_action_with_icon(
                                Icon::Undo,
                                "Undo",
                                Variant::Primary,
                                move |_| undo_last(ui),
                            ))
                            .child(chrome_button(Icon::Close, "Dismiss", move |_| {
                                ui.inbox.undo_banner.set(None);
                            })),
                    ),
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
                        .text_size(px(11.)),
                ),
        );
    }
    row
}

fn render_terminal_diff_strip(term: &InboxTerminalWork) -> View {
    let mut row = view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .overflow_hidden();
    if term.dirty || term.additions > 0 || term.deletions > 0 {
        let mut diff_row = view().flex_row().items_center().gap_1().flex_shrink_0();
        if term.additions > 0 {
            diff_row = diff_row.child(
                label(format!("+{}", term.additions))
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(SUCCESS),
            );
        }
        if term.deletions > 0 {
            diff_row = diff_row.child(
                label(format!("-{}", term.deletions))
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(DANGER),
            );
        }
        if term.additions == 0 && term.deletions == 0 {
            diff_row = diff_row.child(muted("uncommitted").text_size(px(11.5)));
        }
        row = row.child(diff_row);
    } else if term.branch.is_some() {
        row = row.child(muted("clean").text_size(px(11.5)).flex_shrink_0());
    }
    if term.ahead > 0 {
        row = row.child(
            label(format!("↑{}", term.ahead))
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(ACCENT)
                .flex_shrink_0(),
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
    let static_pr_state = item.pr_state.clone();
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
        let state_badge = if let Some(d) = &detail {
            if d.merged {
                Some(("Merged", Tone::Highlight))
            } else if d.draft {
                Some(("Draft", Tone::Neutral))
            } else if d.state.eq_ignore_ascii_case("open") {
                Some(("Open", Tone::Success))
            } else if d.state.eq_ignore_ascii_case("closed") {
                Some(("Closed", Tone::Neutral))
            } else {
                None
            }
        } else {
            match static_pr_state.as_deref() {
                Some("merged") => Some(("Merged", Tone::Highlight)),
                Some("draft") => Some(("Draft", Tone::Neutral)),
                Some("open") => Some(("Open", Tone::Success)),
                Some("closed") => Some(("Closed", Tone::Neutral)),
                _ => None,
            }
        };
        if let Some((lbl, tone)) = state_badge {
            meta_left = meta_left.child(status(lbl, tone));
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

/// Tier 1: Single-layer `SURFACE` row with 3px left semantic bar (no outer box border, no bare `Close`).
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
            .rounded(px(9.))
            .overflow_hidden()
            .bg(SURFACE)
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
                agent_glyph_28(agent_engine.as_deref(), Some(ATTENTION))
            } else if can_quick_merge {
                glyph_28(Icon::GitMerge, SELECTED, ACCENT)
            } else if is_review {
                glyph_28(Icon::Eye, HIGHLIGHT_SURFACE, HIGHLIGHT)
            } else if agent_engine.is_some() {
                agent_glyph_28(agent_engine.as_deref(), None)
            } else {
                glyph_28(Icon::MessageSquare, SELECTED, ACCENT)
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
                        .rounded(px(5.))
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
                    .px_4()
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
            let term_for_menu = term.clone();
            let tid = term.terminal_id.clone();
            let tid_open = tid.clone();
            let tid_btn = tid.clone();
            let card_open = term.card_id.clone();
            let card_btn = term.card_id.clone();
            let defer_title = term.title.clone();
            let dismiss_title = term.title.clone();
            let is_err = matches!(term.status, HerdrStatus::Error | HerdrStatus::Disconnected);
            let accent = if is_err { DANGER } else { ATTENTION };
            let reply_target = ReplyTarget::Terminal {
                terminal_id: tid.clone(),
                label: term.title.clone(),
            };
            let reply_target_btn = reply_target.clone();

            let left_tile = agent_glyph_28(term.agent_engine.as_deref(), Some(accent));

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
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_3()
                        .child(render_terminal_git_telemetry(&term))
                        .child(render_terminal_diff_strip(&term)),
                );

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
                        .rounded(px(5.))
                        .child(detail_lbl),
                );
            }

            // Safe actions only (no bare `Close` next to `Dismiss`; terminate lives inside `⋯` menu).
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
                .child(compact_action("Terminal ↗", Variant::Quiet, move |_| {
                    if let Some(cid) = &card_btn {
                        ui.open_card_with_terminal(cid.clone(), Some(tid_btn.clone()));
                    } else {
                        ui.open_terminal(tid_btn.clone());
                    }
                }));

            let right_triage = view()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .child(chrome_button(Icon::Clock, "Defer", move |_| {
                    open_defer_menu(ui, vec![(key_defer.clone(), wm)], defer_title.clone())
                }))
                .child(chrome_button(Icon::Check, "Hide from Today", move |_| {
                    dismiss_items(
                        ui,
                        vec![(key_dismiss.clone(), wm)],
                        format!("Hidden · {dismiss_title}"),
                    )
                }))
                .child(chrome_button(Icon::More, "Terminal options", move |_| {
                    open_terminal_menu(ui, term_for_menu.clone())
                }));

            row_base(accent).child(
                view()
                    .flex_1()
                    .min_w(px(0.))
                    .px_4()
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
            let left_tile = glyph_28(
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
                    .px_4()
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

            let left_tile = glyph_28(
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
                    view()
                        .pl_3()
                        .py_1p5()
                        .pr_2()
                        .bg(BACKGROUND)
                        .rounded(px(5.))
                        .child(
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
                    .px_4()
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

/// Hairline-ruled `Scheduled Runs` report table (past/live runs + upcoming ticks today + created cards).
fn render_schedules_report(
    ui: Ui,
    runs: &[SchedulePulseItem],
    upcoming: &[UpcomingScheduleItem],
    stacks: Vec<ScheduleCardStack>,
) -> Option<View> {
    if runs.is_empty() && upcoming.is_empty() && stacks.is_empty() {
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
    if !upcoming.is_empty() {
        summary_parts.push(format!("{} upcoming", upcoming.len()));
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
        Icon::Clock,
        if failed > 0 { DANGER } else { ACCENT },
        "Scheduled Runs",
        summary_parts.join(" · "),
        Some(trailing.into_element()),
    ));

    if !runs.is_empty() || !upcoming.is_empty() {
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
                        .w(px(240.)),
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
            let is_failed = matches!(run.state.as_str(), "failed" | "unknown");
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
                                            .w(px(240.))
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

        // Upcoming ticks still scheduled for later today
        for up in upcoming.iter().take(3) {
            let time_str = format_clock_hm(up.next_run_at);
            let pid = up.project_id.clone();
            let sid = up.schedule_id.clone();
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
                                crate::schedules::open(ui, Some(pid.clone()), Some(sid.clone()));
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
                                            .w(px(240.))
                                            .flex_shrink_0()
                                            .flex_row()
                                            .items_center()
                                            .gap_1p5()
                                            .min_w(px(0.))
                                            .child(status_dot(MUTED))
                                            .child(
                                                muted(up.schedule_name.clone())
                                                    .text_size(px(12.5))
                                                    .text_color(TEXT)
                                                    .truncate(),
                                            )
                                            .child(
                                                muted(format!("· {}", up.project_name))
                                                    .text_size(px(11.5))
                                                    .truncate(),
                                            ),
                                    )
                                    .child(
                                        muted(up.cron.clone())
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.5))
                                            .flex_1()
                                            .min_w(px(0.)),
                                    ),
                            ),
                    )
                    .child(status("Upcoming", Tone::Neutral)),
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

/// Unified borderless GitHub PR/Issue row with colored state mark, expand chevron,
/// labels, CI state, comment icon+count, avatar+@author, age, and toggleable Markdown reader.
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
    let kind_str = item.kind.clone();
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
    let static_pr_state = item.pr_state.clone();
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
        let merged = detail.as_ref().is_some_and(|d| d.merged)
            || static_pr_state
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("merged"));
        let draft = detail.as_ref().is_some_and(|d| d.draft)
            || static_pr_state
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("draft"));
        let pr_state_ref = detail
            .as_ref()
            .map(|d| d.state.as_str())
            .filter(|s| !s.is_empty())
            .or(static_pr_state.as_deref());
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

        let mut title_group = view()
            .flex_1()
            .min_w(px(0.))
            .flex_row()
            .items_center()
            .gap_2()
            .overflow_hidden();
        if has_number {
            title_group = title_group.child(
                icon_tinted(
                    if expanded {
                        Icon::ChevronDown
                    } else {
                        Icon::Forward
                    },
                    MUTED,
                )
                .w(px(12.))
                .h(px(12.))
                .flex_shrink_0(),
            );
        } else {
            title_group = title_group.child(view().w(px(12.)).h(px(12.)).flex_shrink_0());
        }
        title_group = title_group.child(gh_state_mark(&kind_str, pr_state_ref, merged, draft));
        if show_repo {
            title_group = title_group.child(
                muted(repo_label.clone())
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .max_w(px(140.))
                    .truncate()
                    .flex_shrink_0(),
            );
        }
        title_group = title_group
            .child(
                muted(ref_col.clone())
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .w(px(46.))
                    .flex_shrink_0(),
            )
            .child(
                label(title_text.clone())
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(TEXT)
                    .truncate()
                    .flex_1()
                    .min_w(px(0.)),
            );
        if is_pr && merged {
            title_group = title_group.child(status("Merged", Tone::Highlight).flex_shrink_0());
        } else if is_pr && draft {
            title_group = title_group.child(status("Draft", Tone::Neutral).flex_shrink_0());
        }
        for lbl in labels.into_iter().take(2) {
            title_group = title_group.child(view().flex_shrink_0().child(badge(lbl)));
        }

        let mut ci_slot = view()
            .w(px(60.))
            .flex_shrink_0()
            .flex_row()
            .items_center()
            .justify_end();
        if let Some(ci_el) = ci_indicator(ci_state.as_deref(), ci_passed, ci_total, failed_n) {
            ci_slot = ci_slot.child(ci_el);
        }

        let mut comments_slot = view()
            .w(px(34.))
            .flex_shrink_0()
            .flex_row()
            .items_center()
            .justify_end();
        if let Some(c) = comments
            && (is_pr || c > 0)
        {
            comments_slot = comments_slot.child(comments_indicator(c));
        }

        let mut author_slot = view()
            .w(px(112.))
            .flex_shrink_0()
            .flex_row()
            .items_center()
            .gap_1()
            .overflow_hidden();
        if let Some(a) = &author {
            author_slot = author_slot
                .child(github_avatar(a, avatar_url.as_deref(), 16.))
                .child(
                    muted(format!("@{a}"))
                        .text_size(px(11.5))
                        .truncate()
                        .flex_1()
                        .min_w(px(0.)),
                );
        }

        let age_slot = view()
            .w(px(34.))
            .flex_shrink_0()
            .flex_row()
            .items_center()
            .justify_end()
            .child(muted(age.clone()).font_family(MONO_FONT).text_size(px(11.)));

        view()
            .w_full()
            .min_w(px(0.))
            .flex_row()
            .items_center()
            .gap_2p5()
            .child(title_group)
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_shrink_0()
                    .child(ci_slot)
                    .child(comments_slot)
                    .child(author_slot)
                    .child(age_slot),
            )
            .into_element()
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
                        .overflow_hidden()
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

/// GitHub & PRs chapter: Direct updates + collapsible Watching feeds.
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
        "GitHub & PRs",
        sub_parts.join(" · "),
        None,
    ));

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

/// Single-layer borderless `SURFACE` card for a live Agent Terminal (no bare `Close` button; `⋯` menu only).
fn render_agent_session_card(
    ui: Ui,
    term: InboxTerminalWork,
    now_ms: u64,
    in_project_group: bool,
) -> View {
    let term_for_menu = term.clone();
    let tid_open = term.terminal_id.clone();
    let card_open = term.card_id.clone();
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
        format!("{status_lbl} · T{turn}")
    } else {
        status_lbl.to_owned()
    };

    let tile = agent_glyph_28(term.agent_engine.as_deref(), Some(status_color));

    // Row 1 (Eyebrow): When grouped under a named Project header, omit the redundant project name
    // and dedicate the eyebrow to branch/cwd so parallel terminals in one project are easy to tell apart.
    // Full project/branch/cwd is always disclosed on hover tooltip.
    let mut ctx_tip_parts = Vec::new();
    if let Some(proj) = &term.project_name {
        ctx_tip_parts.push(proj.clone());
    } else {
        ctx_tip_parts.push(term.host_label.clone());
    }
    if let Some(br) = &term.branch {
        ctx_tip_parts.push(br.clone());
    }
    if let Some(cwd) = &term.cwd {
        ctx_tip_parts.push(shorten_path(cwd));
    }
    let ctx_tooltip = ctx_tip_parts.join(" · ");

    let mut ctx_left = view()
        .flex_row()
        .items_center()
        .gap_1p5()
        .flex_1()
        .min_w(px(0.))
        .overflow_hidden()
        .tooltip(hint_tooltip(ctx_tooltip));
    if in_project_group {
        if let Some(br) = &term.branch {
            ctx_left = ctx_left.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .child(icon_tinted(Icon::Branch, MUTED).w(px(11.5)).h(px(11.5)))
                    .child(
                        muted(br.clone())
                            .font_family(MONO_FONT)
                            .text_size(px(11.5))
                            .truncate()
                            .flex_1()
                            .min_w(px(0.)),
                    ),
            );
        } else if let Some(cwd) = &term.cwd {
            ctx_left = ctx_left.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .child(icon_tinted(Icon::Folder, MUTED).w(px(11.5)).h(px(11.5)))
                    .child(
                        muted(shorten_path(cwd))
                            .font_family(MONO_FONT)
                            .text_size(px(11.))
                            .truncate()
                            .flex_1()
                            .min_w(px(0.)),
                    ),
            );
        } else {
            ctx_left = ctx_left.child(
                muted(term.host_label.clone())
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .truncate()
                    .flex_1()
                    .min_w(px(0.)),
            );
        }
    } else {
        let scope_label = term
            .project_name
            .clone()
            .unwrap_or_else(|| term.host_label.clone());
        ctx_left = ctx_left.child(
            label(scope_label)
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if term.project_name.is_some() {
                    TEXT
                } else {
                    MUTED
                })
                .truncate()
                .max_w(px(136.))
                .flex_shrink_0(),
        );
        if let Some(br) = &term.branch {
            ctx_left = ctx_left.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .child(icon_tinted(Icon::Branch, MUTED).w(px(11.5)).h(px(11.5)))
                    .child(
                        muted(br.clone())
                            .font_family(MONO_FONT)
                            .text_size(px(11.5))
                            .truncate()
                            .flex_1()
                            .min_w(px(0.)),
                    ),
            );
        } else if term.project_name.is_none()
            && let Some(cwd) = &term.cwd
        {
            ctx_left = ctx_left.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .child(icon_tinted(Icon::Folder, MUTED).w(px(11.5)).h(px(11.5)))
                    .child(
                        muted(shorten_path(cwd))
                            .font_family(MONO_FONT)
                            .text_size(px(11.))
                            .truncate()
                            .flex_1()
                            .min_w(px(0.)),
                    ),
            );
        }
    }

    let eyebrow = view()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .min_w(px(0.))
        .child(ctx_left)
        .child(view().flex_shrink_0().child(if is_live {
            live_status(state_pill_text, status_tone)
        } else {
            status(state_pill_text, status_tone)
        }));

    // Row 2 (Primary identity): Card `#N Title` when linked to a Card, else terminal/session title.
    let has_card = term.card_number.is_some() && term.card_title.is_some();
    let primary_row: AnyElement = match (term.card_number, &term.card_title) {
        (Some(n), Some(ct)) => view()
            .flex_row()
            .items_center()
            .gap_1p5()
            .min_w(px(0.))
            .child(
                label(format!("#{n}"))
                    .font_family(MONO_FONT)
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ACCENT)
                    .flex_shrink_0(),
            )
            .child(
                label(ct.clone())
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(TEXT)
                    .truncate()
                    .flex_1()
                    .min_w(px(0.)),
            )
            .into_element(),
        _ => label(term.title.clone())
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(TEXT)
            .truncate()
            .min_w(px(0.))
            .into_element(),
    };

    // Row 3 (Secondary activity line): agent tag + current command or status detail.
    let agent_tag = term
        .agent_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| (!term.title.trim().is_empty()).then_some(term.title.as_str()));
    let secondary_line: Option<(String, bool)> = if let Some(cmd) = &term.shell_command {
        let suffix = term
            .shell_exit_code
            .map(|c| format!(" (exit {c})"))
            .unwrap_or_default();
        let line = if has_card {
            match agent_tag {
                Some(a) => format!("{a} · {cmd}{suffix}"),
                None => format!("{cmd}{suffix}"),
            }
        } else {
            format!("{cmd}{suffix}")
        };
        Some((line, true))
    } else if let Some(detail) = &term.status_detail {
        let line = if has_card {
            match agent_tag {
                Some(a) => format!("{a} · {detail}"),
                None => detail.clone(),
            }
        } else {
            detail.clone()
        };
        Some((line, false))
    } else if has_card {
        agent_tag.map(|a| (a.to_owned(), false))
    } else {
        None
    };

    let mut content = view()
        .flex_1()
        .min_w(px(0.))
        .gap_1()
        .child(eyebrow)
        .child(primary_row);
    if let Some((line, is_mono)) = secondary_line {
        let mut lbl = muted(line)
            .text_size(px(if is_mono { 11.5 } else { 12. }))
            .truncate()
            .min_w(px(0.));
        if is_mono {
            lbl = lbl.font_family(MONO_FONT);
        }
        content = content.child(lbl);
    }
    content = content.child(
        view()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .min_w(px(0.))
            .child(render_terminal_diff_strip(&term))
            .child(
                muted(age)
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .flex_shrink_0(),
            ),
    );

    let body = view()
        .flex_row()
        .items_start()
        .gap_2p5()
        .min_w(px(0.))
        .child(tile)
        .child(content);

    view()
        .w_full()
        .min_w(px(0.))
        .overflow_hidden()
        .bg(SURFACE)
        .rounded(px(10.))
        .px_3p5()
        .py_3()
        .flex_row()
        .items_start()
        .gap_1p5()
        .child(
            press_target()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .bg(CLEAR)
                .rounded(px(6.))
                .hover(|s| s.bg(CARD_HOVER))
                .on_press(move |_: &()| {
                    if let Some(cid) = &card_open {
                        ui.open_card_with_terminal(cid.clone(), Some(tid_open.clone()));
                    } else {
                        ui.open_terminal(tid_open.clone());
                    }
                })
                .child(body),
        )
        .child(chrome_button(Icon::More, "Terminal options", move |_| {
            open_terminal_menu(ui, term_for_menu.clone())
        }))
}

/// Active Work chapter: project-grouped editorial sections with 2-column borderless `SURFACE`
/// Agent cards, hairline Today's Cards rows, and per-project collapsible Idle Shells.
fn render_active_work(
    ui: Ui,
    groups: Vec<ActiveWorkProjectGroup>,
    now_ms: u64,
    narrow: bool,
) -> Option<View> {
    if groups.is_empty() {
        return None;
    }
    let total_agents: usize = groups.iter().map(|g| g.agent_terminals.len()).sum();
    let total_cards: usize = groups.iter().map(|g| g.open_cards.len()).sum();
    let total_shells: usize = groups.iter().map(|g| g.plain_shells.len()).sum();
    let mut sub_parts = Vec::new();
    if total_agents > 0 {
        sub_parts.push(format!(
            "{total_agents} {}",
            if total_agents == 1 { "agent" } else { "agents" }
        ));
    }
    if total_cards > 0 {
        sub_parts.push(format!(
            "{total_cards} {}",
            if total_cards == 1 { "card" } else { "cards" }
        ));
    }
    if total_shells > 0 {
        sub_parts.push(format!(
            "{total_shells} {}",
            if total_shells == 1 { "shell" } else { "shells" }
        ));
    }

    let mut sec = view().gap_4().child(chapter_header(
        Icon::Terminal,
        ACCENT,
        "Active Work",
        sub_parts.join(" · "),
        None,
    ));

    for group in groups {
        let is_named_project = group.project_id.is_some();
        let dot_color = if group.live_agent_count > 0 {
            ACCENT
        } else if group
            .agent_terminals
            .iter()
            .any(|t| t.status == HerdrStatus::Done)
        {
            SUCCESS
        } else {
            MUTED
        };

        let mut grp_parts = Vec::new();
        if !group.agent_terminals.is_empty() {
            let n = group.agent_terminals.len();
            grp_parts.push(format!("{n} {}", if n == 1 { "agent" } else { "agents" }));
        }
        if !group.open_cards.is_empty() {
            let n = group.open_cards.len();
            grp_parts.push(format!("{n} {}", if n == 1 { "card" } else { "cards" }));
        }
        if !group.plain_shells.is_empty() {
            let n = group.plain_shells.len();
            grp_parts.push(format!("{n} {}", if n == 1 { "shell" } else { "shells" }));
        }

        let name_el: AnyElement = if let Some(pid) = group.project_id.clone() {
            press_target()
                .bg(CLEAR)
                .rounded(px(4.))
                .hover(|s| s.bg(CARD_HOVER))
                .on_press(move |_: &()| {
                    ui.project.set(pid.clone());
                    ui.navigate("board");
                })
                .child(
                    label(group.display_name.clone())
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(TEXT),
                )
                .into_element()
        } else {
            label(group.display_name.clone())
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(MUTED)
                .into_element()
        };

        let mut hdr_left = view()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .child(status_dot(dot_color))
            .child(name_el);
        if let Some(h) = &group.host_label {
            hdr_left = hdr_left.child(
                muted(h.clone())
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .flex_shrink_0(),
            );
        }
        if !grp_parts.is_empty() {
            hdr_left = hdr_left.child(
                muted(grp_parts.join(" · "))
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .truncate()
                    .min_w(px(0.)),
            );
        }

        let mut hdr = view()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .py_1()
            .border_b_1()
            .border_color(BORDER)
            .child(hdr_left);
        if let Some(pid) = group.project_id.clone() {
            hdr = hdr.child(compact_action("Board →", Variant::Quiet, move |_| {
                ui.project.set(pid.clone());
                ui.navigate("board");
            }));
        }

        let has_agents = !group.agent_terminals.is_empty();
        let has_cards = !group.open_cards.is_empty();
        let mut group_block = view().gap_2p5().child(hdr);

        // 1. Live Agent Sessions (strictly equal-width CSS Grid: `repeat(2, minmax(0, 1fr))` on desktop)
        if has_agents {
            let mut grid = view()
                .w_full()
                .grid()
                .grid_cols(if narrow { 1 } else { 2 })
                .gap_3();
            for term in group.agent_terminals {
                grid = grid.child(render_agent_session_card(
                    ui,
                    term,
                    now_ms,
                    is_named_project,
                ));
            }
            group_block = group_block.child(grid);
        }

        // 2. Today's Cards for this project (hairline-ruled table)
        if has_cards {
            let mut table = view();
            if has_agents {
                table = table.border_t_1().border_color(BORDER);
            }
            for card in group.open_cards {
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
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .flex_1()
                            .min_w(px(0.)),
                    );
                if let Some(br) = card.branch {
                    let full_br = br.clone();
                    row_left = row_left.child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .tooltip(hint_tooltip(full_br))
                            .child(icon_tinted(Icon::Branch, MUTED).w(px(11.5)).h(px(11.5)))
                            .child(
                                muted(br)
                                    .font_family(MONO_FONT)
                                    .text_size(px(11.5))
                                    .max_w(px(148.))
                                    .truncate(),
                            ),
                    );
                }
                row_left = row_left.child(
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
                if let Some(h) = card.host_label
                    && group.host_label.is_none()
                {
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
            group_block = group_block.child(table);
        }

        // 3. Per-project borderless Idle Shells collapsible row (no bare Close; `⋯` menu per shell)
        if !group.plain_shells.is_empty() {
            let stack_id = format!("plain-shells-stack:{}", group.group_id);
            let expanded = ui.inbox.expanded_stacks.with(|s| s.contains(&stack_id));
            let count = group.plain_shells.len();
            let preview = group
                .plain_shells
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

            let mut band = view();
            if has_agents && !has_cards {
                band = band.border_t_1().border_color(BORDER);
            }
            band = band.child(
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
                    ),
            );

            if expanded {
                for sh in group.plain_shells {
                    let sh_menu = sh.clone();
                    let tid_open = sh.terminal_id.clone();
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
                            .child(chrome_button(Icon::More, "Shell options", move |_| {
                                open_terminal_menu(ui, sh_menu.clone())
                            })),
                    );
                }
            }
            group_block = group_block.child(band);
        }

        sec = sec.child(group_block);
    }

    Some(sec)
}

/// Shipped Today chapter: Merged PRs (purple `GitMerge` icon) & Resolved Cards today.
fn render_shipped_today(ui: Ui, items: &[ShippedTodayItem], now_ms: u64) -> Option<View> {
    if items.is_empty() {
        return None;
    }
    let prs = items.iter().filter(|s| s.is_pr).count();
    let cards = items.iter().filter(|s| !s.is_pr).count();
    let mut parts = Vec::new();
    if prs > 0 {
        parts.push(format!(
            "{prs} {} merged",
            if prs == 1 { "PR" } else { "PRs" }
        ));
    }
    if cards > 0 {
        parts.push(format!(
            "{cards} {} resolved",
            if cards == 1 { "card" } else { "cards" }
        ));
    }

    let mut list = view().border_t_1().border_color(BORDER);
    for item in items.iter().take(12) {
        let age = format_age_compact(item.shipped_at, now_ms);
        let cid = item.card_id.clone();
        let url = item.url.clone();
        let ref_lbl = if let Some(n) = item.pr_number {
            format!("#{n}")
        } else if let Some(n) = item.card_number {
            format!("#{n}")
        } else {
            "—".into()
        };
        let ctx = item
            .repository
            .clone()
            .unwrap_or_else(|| item.project_name.clone());

        let mut row = view()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .child(if item.is_pr {
                icon_tinted(Icon::GitMerge, HIGHLIGHT)
                    .w(px(14.))
                    .h(px(14.))
                    .flex_shrink_0()
            } else {
                icon_tinted(Icon::Check, SUCCESS)
                    .w(px(14.))
                    .h(px(14.))
                    .flex_shrink_0()
            })
            .child(
                muted(ref_lbl)
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .w(px(44.))
                    .flex_shrink_0(),
            )
            .child(
                label(item.title.clone())
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(TEXT)
                    .truncate()
                    .flex_1()
                    .min_w(px(0.)),
            )
            .child(muted(ctx).text_size(px(11.5)).flex_shrink_0());
        if let Some(a) = &item.author {
            row = row.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .child(github_avatar(a, None, 16.))
                    .child(muted(format!("@{a}")).text_size(px(11.5))),
            );
        }
        row = row.child(
            muted(age)
                .font_family(MONO_FONT)
                .text_size(px(11.))
                .flex_shrink_0(),
        );

        list = list.child(
            view().py_2().border_b_1().border_color(BORDER).child(
                press_target()
                    .w_full()
                    .bg(CLEAR)
                    .rounded(px(4.))
                    .hover(|s| s.bg(CARD_HOVER))
                    .on_press(move |_: &()| {
                        if let Some(c) = &cid {
                            ui.open_card(c.clone());
                        } else if let Some(u) = &url {
                            let _ = crate::platform::open_external(u);
                        }
                    })
                    .child(row),
            ),
        );
    }

    Some(
        view()
            .gap_2()
            .child(chapter_header(
                Icon::Check,
                SUCCESS,
                "Shipped Today",
                parts.join(" · "),
                Some(
                    compact_action("Insights ↗", Variant::Quiet, move |_| {
                        ui.navigate("insights");
                    })
                    .into_element(),
                ),
            ))
            .child(list),
    )
}

/// Focused view when clicking `READY TO MERGE` KPI.
fn render_ready_to_merge_focus(ui: Ui, ready: ReadyMergeSummary, now_ms: u64) -> View {
    let total = ready.total();
    let mut sec = view().gap_2p5().child(chapter_header(
        Icon::GitMerge,
        SUCCESS,
        "Ready to Merge",
        format!("{total}"),
        Some(
            compact_action("Show all", Variant::Quiet, move |_| {
                ui.inbox.focus_filter.set(String::new());
            })
            .into_element(),
        ),
    ));

    if total == 0 {
        return sec.child(
            view()
                .py_6()
                .items_center()
                .gap_2()
                .child(muted("No PRs ready to merge")),
        );
    }

    if !ready.needs_you_items.is_empty() {
        let mut list = view().gap_2();
        for item in ready.needs_you_items {
            list = list.child(render_needs_you_row(ui, item, now_ms));
        }
        sec = sec.child(list);
    }

    if !ready.open_cards.is_empty() {
        let mut list = view().border_t_1().border_color(BORDER);
        for c in ready.open_cards {
            let cid = c.card_id.clone();
            let age = format_age_compact(c.last_activity_at, now_ms);
            list = list.child(
                view().py_2().border_b_1().border_color(BORDER).child(
                    press_target()
                        .w_full()
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
                                    icon_tinted(Icon::GitMerge, SUCCESS)
                                        .w(px(14.))
                                        .h(px(14.))
                                        .flex_shrink_0(),
                                )
                                .child(
                                    muted(format!("#{}", c.card_number))
                                        .font_family(MONO_FONT)
                                        .text_size(px(11.5))
                                        .w(px(44.))
                                        .flex_shrink_0(),
                                )
                                .child(
                                    label(c.card_title)
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .truncate()
                                        .flex_1()
                                        .min_w(px(0.)),
                                )
                                .child(muted(c.project_name).text_size(px(11.5)).flex_shrink_0())
                                .child(
                                    muted(age)
                                        .font_family(MONO_FONT)
                                        .text_size(px(11.))
                                        .flex_shrink_0(),
                                ),
                        ),
                ),
            );
        }
        sec = sec.child(list);
    }

    if !ready.gh_items.is_empty() {
        let mut list = view().border_t_1().border_color(BORDER);
        for item in ready.gh_items {
            list = list.child(render_github_row(ui, item, now_ms, true, 0., None));
        }
        sec = sec.child(list);
    }

    sec
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

/// First-run launchpad for brand-new users (no projects, no terminals yet — hides all-zero KPIs & fake "All clear").
fn render_first_run(ui: Ui, gh_connected: bool) -> View {
    let launch_card = |icon_kind: Icon,
                       tint: ThemeColor,
                       bg_tint: ThemeColor,
                       title: &'static str,
                       sub: &'static str,
                       on_click: Box<dyn Fn()>| {
        press_target()
            .flex_1()
            .min_w(px(180.))
            .bg(SURFACE)
            .rounded(px(10.))
            .p_4()
            .hover(|s| s.bg(CARD_HOVER))
            .on_press(move |_: &()| on_click())
            .child(
                view()
                    .gap_2()
                    .child(glyph_28(icon_kind, bg_tint, tint))
                    .child(
                        view()
                            .gap_0p5()
                            .child(
                                label(title)
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(TEXT),
                            )
                            .child(muted(sub).text_size(px(12.))),
                    ),
            )
    };

    view()
        .gap_6()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .pb_4()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    label(format_today_date_long())
                        .text_size(px(22.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(TEXT),
                )
                .child(render_environment_pulse(ui)),
        )
        .child(
            view()
                .pt_6()
                .gap_4()
                .child(
                    label("Get Started")
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(TEXT),
                )
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_3()
                        .child(launch_card(
                            Icon::Plus,
                            ACCENT,
                            SELECTED,
                            "Add Project",
                            "Local repository",
                            Box::new(move || ui.navigate("new-project")),
                        ))
                        .child(launch_card(
                            Icon::GitHub,
                            if gh_connected { SUCCESS } else { TEXT },
                            if gh_connected { SUCCESS_SURFACE } else { INSET },
                            if gh_connected {
                                "GitHub Connected"
                            } else {
                                "Connect GitHub"
                            },
                            "PRs & reviews",
                            Box::new(move || ui.navigate("github-account")),
                        ))
                        .child(launch_card(
                            Icon::Terminal,
                            HIGHLIGHT,
                            HIGHLIGHT_SURFACE,
                            "New Terminal",
                            "Agent or shell",
                            Box::new(move || {
                                ui.navigate("terminals");
                                crate::terminals::new_tab(ui);
                            }),
                        )),
                ),
        )
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
        .py(px(36.))
        .px_6()
        .items_center()
        .gap_2()
        .child(glyph_28(Icon::Check, SUCCESS_SURFACE, SUCCESS))
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
        .relative()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .bg(BACKGROUND)
        .child(
            scroll().flex_1().min_h(px(0.)).child(
                view().w_full().items_center().px_6().py_5().child(
                    view()
                        .w_full()
                        .max_w(px(1060.))
                        .pb_10()
                        .child(dynamic(move || {
                            let inbox = ui.daily_inbox.get();
                            let active_drawer = ui.inbox.drawer.get();
                            let narrow = ui.narrow.get();
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

                            let is_first_run = !inbox.has_projects
                                && inbox.agent_terminals.is_empty()
                                && inbox.plain_shells.is_empty()
                                && inbox.needs_you.is_empty()
                                && inbox.github_stacks.is_empty()
                                && inbox.github_watching.is_empty()
                                && inbox.shipped_today.is_empty()
                                && inbox.schedule_pulse.is_empty()
                                && inbox.upcoming_schedules.is_empty();

                            if is_first_run {
                                return render_first_run(ui, gh_connected.get()).into_element();
                            }

                            let focus = ui.inbox.focus_filter.get();
                            let gh_details = ui.inbox.github_item_details.get();
                            let ready = collect_ready_to_merge(&inbox, &gh_details);

                            let mut col = view().gap(px(26.));

                            // 1. Editorial Masthead + clickable 4-Column Decision KPI Strip
                            col = col.child(render_editorial_masthead(
                                ui, &inbox, &ready, &focus, now_ms,
                            ));

                            if !focus.is_empty() {
                                match focus.as_str() {
                                    "waiting" => {
                                        if !inbox.needs_you.is_empty() {
                                            let count = inbox.needs_you.len();
                                            let mut sec = view().gap_2p5().child(chapter_header(
                                                Icon::ShieldAlert,
                                                ATTENTION,
                                                "Waiting On You",
                                                format!("{count}"),
                                                Some(
                                                    compact_action(
                                                        "Show all",
                                                        Variant::Quiet,
                                                        move |_| {
                                                            ui.inbox.focus_filter.set(String::new())
                                                        },
                                                    )
                                                    .into_element(),
                                                ),
                                            ));
                                            let mut list = view().gap_2();
                                            for item in inbox.needs_you.iter().cloned() {
                                                list = list.child(render_needs_you_row(
                                                    ui, item, now_ms,
                                                ));
                                            }
                                            sec = sec.child(list);
                                            col = col.child(sec);
                                        } else {
                                            col = col.child(render_inbox_zero(ui, &inbox));
                                        }
                                    }
                                    "merge" => {
                                        col = col.child(render_ready_to_merge_focus(
                                            ui, ready, now_ms,
                                        ));
                                    }
                                    "agents" => {
                                        let blocked_terms: Vec<NeedsYouItem> = inbox
                                            .needs_you
                                            .iter()
                                            .filter(|i| {
                                                matches!(
                                                    &i.kind,
                                                    NeedsYouKind::Terminal { terminal } if terminal.is_agent
                                                )
                                            })
                                            .cloned()
                                            .collect();
                                        let has_blocked = !blocked_terms.is_empty();
                                        if has_blocked {
                                            let mut sec = view().gap_2p5().child(chapter_header(
                                                Icon::ShieldAlert,
                                                ATTENTION,
                                                "Waiting Agents",
                                                format!("{}", blocked_terms.len()),
                                                Some(
                                                    compact_action(
                                                        "Show all",
                                                        Variant::Quiet,
                                                        move |_| {
                                                            ui.inbox.focus_filter.set(String::new())
                                                        },
                                                    )
                                                    .into_element(),
                                                ),
                                            ));
                                            let mut list = view().gap_2();
                                            for item in blocked_terms {
                                                list = list.child(render_needs_you_row(
                                                    ui, item, now_ms,
                                                ));
                                            }
                                            sec = sec.child(list);
                                            col = col.child(sec);
                                        }
                                        if let Some(work) = render_active_work(
                                            ui,
                                            inbox.active_work_groups.clone(),
                                            now_ms,
                                            narrow,
                                        ) {
                                            col = col.child(work);
                                        } else if !has_blocked {
                                            col = col.child(
                                                view()
                                                    .py_6()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(muted("No active agents")),
                                            );
                                        }
                                    }
                                    "shipped" => {
                                        if let Some(shipped) =
                                            render_shipped_today(ui, &inbox.shipped_today, now_ms)
                                        {
                                            col = col.child(shipped);
                                        } else {
                                            col = col.child(
                                                view()
                                                    .py_6()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(muted("Nothing shipped today"))
                                                    .child(compact_action(
                                                        "Open Insights ↗",
                                                        Variant::Secondary,
                                                        move |_| ui.navigate("insights"),
                                                    )),
                                            );
                                        }
                                    }
                                    _ => {}
                                }
                                return col.into_element();
                            }

                            // 2. Active Projects quick-jump strip
                            if let Some(proj_strip) =
                                render_project_pulses(ui, &inbox.project_pulses)
                            {
                                col = col.child(proj_strip);
                            }

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

                            let all_active_empty = inbox.needs_you.is_empty()
                                && inbox.github_stacks.is_empty()
                                && inbox.github_watching.is_empty()
                                && inbox.schedule_stacks.is_empty()
                                && inbox.schedule_pulse.is_empty()
                                && inbox.upcoming_schedules.is_empty()
                                && inbox.open_cards.is_empty()
                                && inbox.agent_terminals.is_empty()
                                && inbox.plain_shells.is_empty();

                            if all_active_empty {
                                col = col.child(render_inbox_zero(ui, &inbox));
                            } else {
                                // 3. Needs Action (single-layer `SURFACE` rows with 3px semantic left bar)
                                if !inbox.needs_you.is_empty() {
                                    let count = inbox.needs_you.len();
                                    let mut sec = view().gap_2p5().child(chapter_header(
                                        Icon::ShieldAlert,
                                        ATTENTION,
                                        "Needs Action",
                                        format!("{count}"),
                                        None,
                                    ));
                                    let mut list = view().gap_2();
                                    for item in inbox.needs_you.iter().cloned() {
                                        list = list.child(render_needs_you_row(ui, item, now_ms));
                                    }
                                    sec = sec.child(list);
                                    col = col.child(sec);
                                }

                                // 4. Active Work (project-grouped 2-column borderless `SURFACE` Agent cards + Today's Cards + Idle Shells)
                                if let Some(work) = render_active_work(
                                    ui,
                                    inbox.active_work_groups.clone(),
                                    now_ms,
                                    narrow,
                                ) {
                                    col = col.child(work);
                                }

                                // 5. GitHub & PRs (colored PR state, CI, comments, avatars, toggleable Markdown reader)
                                if let Some(gh) = render_github_activity(
                                    ui,
                                    inbox.github_stacks.clone(),
                                    inbox.github_watching.clone(),
                                    now_ms,
                                ) {
                                    col = col.child(gh);
                                }

                                // 6. Scheduled Runs (past/live runs + upcoming ticks + created cards)
                                if let Some(sched) = render_schedules_report(
                                    ui,
                                    &inbox.schedule_pulse,
                                    &inbox.upcoming_schedules,
                                    inbox.schedule_stacks.clone(),
                                ) {
                                    col = col.child(sched);
                                }
                            }

                            // 7. Shipped Today (merged PRs & resolved cards today — shown even on Inbox Zero!)
                            if let Some(shipped) =
                                render_shipped_today(ui, &inbox.shipped_today, now_ms)
                            {
                                col = col.child(shipped);
                            }

                            // 8. Earlier fold
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
        .child(render_floating_undo_toast(ui))
}
