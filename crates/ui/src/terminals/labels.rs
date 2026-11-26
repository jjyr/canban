//! Shared Terminal presentation: sidebar metadata and compact tab labels.
use super::*;

/// Leading icon for a Terminal sidebar row: Agent brand mark when present,
/// `Icon::Agent` for managed sessions without a brand mark, otherwise `Icon::Terminal`.
pub fn sidebar_meta_icon(e: &Entry) -> Icon {
    e.provider()
        .map(|(mark, _)| mark)
        .unwrap_or(if e.is_managed() {
            Icon::Agent
        } else {
            Icon::Terminal
        })
}

/// Compact bot badge distinguishing Canban-managed terminals from user-created manual terminals.
pub fn managed_badge() -> View {
    view()
        .w(px(16.))
        .h(px(16.))
        .rounded(px(4.))
        .bg(HIGHLIGHT_SURFACE)
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .pointer_events(PointerEvents::None)
        .child(icon_tinted(Icon::Agent, HIGHLIGHT).w(px(10.)).h(px(10.)))
}

/// Calm static pre-reclaim / active auto-reclaim indicator pill for Terminal tabs.
pub(crate) fn reclaim_badge_spec(e: &Entry) -> Option<(&'static str, ThemeColor, ThemeColor)> {
    if e.is_auto_reclaiming() {
        Some(("Auto-reclaiming…", INSET, MUTED))
    } else if e.is_reclaiming_soon() {
        Some(("Reclaiming soon", INSET, MUTED))
    } else {
        None
    }
}

pub fn tab_reclaim_badge(e: &Entry) -> Option<View> {
    let (label_text, bg, fg) = reclaim_badge_spec(e)?;
    Some(
        view()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1p5()
            .py(px(1.))
            .rounded(px(4.))
            .bg(bg)
            .flex_shrink_0()
            .pointer_events(PointerEvents::None)
            .child(icon_tinted(Icon::Clock, fg).w(px(10.)).h(px(10.)))
            .child(
                text(label_text)
                    .text_size(px(10.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(fg),
            ),
    )
}

/// Single-line title for compact 30px Terminal sidebar rows:
/// - Card Role Agent inside its own `CardWorktree(#N)`: `@role` (or custom name), avoiding duplicating `#N` from the parent worktree header;
/// - Card Role Agent `cd`'d into another worktree or directory: `#N @role` (or custom name);
/// - Standalone CLI Agent / Shell with observed Agent / Plain Shell: `e.title()`.
pub fn sidebar_row_title(e: &Entry, in_own_card_worktree: bool) -> String {
    tab_label_text(e, !in_own_card_worktree)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Optional host badge for single-line sidebar rows: omitted for `local` terminals,
/// returning the remote host display name otherwise.
pub fn sidebar_host_badge(e: &Entry, machine: &str) -> Option<String> {
    if e.host_id.is_empty() || e.host_id == "local" {
        None
    } else {
        Some(machine.to_owned())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GroupStatusSummary {
    pub total: usize,
    pub error: usize,
    pub blocked: usize,
    pub attention: usize,
    pub working: usize,
    pub starting: usize,
    pub done: usize,
}

/// Aggregates terminal counts and highest-priority runtime signals for collapsed Project/Worktree headers.
pub fn group_status_summary<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
) -> GroupStatusSummary {
    use canban_types::HerdrStatus;
    let mut out = GroupStatusSummary::default();
    for e in entries {
        out.total += 1;
        match e.herdr_status() {
            HerdrStatus::Error | HerdrStatus::Disconnected => out.error += 1,
            HerdrStatus::Blocked => out.blocked += 1,
            HerdrStatus::Done if e.needs_attention() => out.attention += 1,
            HerdrStatus::Idle if e.needs_attention() => out.attention += 1,
            HerdrStatus::Working => out.working += 1,
            HerdrStatus::Starting | HerdrStatus::Queued => out.starting += 1,
            HerdrStatus::Done => out.done += 1,
            HerdrStatus::PlainShell
            | HerdrStatus::Idle
            | HerdrStatus::Closing
            | HerdrStatus::Ended => {}
        }
    }
    out
}

pub fn group_summary_a11y(summary: GroupStatusSummary) -> String {
    let mut parts = vec![format!(
        "{} terminal{}",
        summary.total,
        if summary.total == 1 { "" } else { "s" }
    )];
    if summary.error > 0 {
        parts.push(format!("{} error", summary.error));
    }
    if summary.blocked > 0 {
        parts.push(format!("{} blocked", summary.blocked));
    }
    if summary.attention > 0 {
        parts.push(format!("{} needs attention", summary.attention));
    }
    if summary.working > 0 {
        parts.push(format!("{} working", summary.working));
    }
    if summary.error == 0
        && summary.blocked == 0
        && summary.attention == 0
        && summary.working == 0
        && summary.done > 0
    {
        parts.push(format!("{} done", summary.done));
    }
    parts.join(", ")
}

/// Compact `[count] [status marks]` badge shown on collapsed Project and Worktree headers.
pub fn group_summary_badge(summary: GroupStatusSummary) -> Option<View> {
    if summary.total == 0 {
        return None;
    }
    let (pill_bg, pill_fg) = if summary.error > 0 {
        (DANGER_SURFACE, DANGER)
    } else if summary.blocked + summary.attention > 0 {
        (ATTENTION_SURFACE, ATTENTION)
    } else {
        (INSET, MUTED)
    };
    let mut row = view()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .child(
            view()
                .min_w(px(18.))
                .h(px(18.))
                .px_1()
                .rounded(px(5.))
                .bg(pill_bg)
                .items_center()
                .justify_center()
                .child(
                    text(summary.total.to_string())
                        .text_size(px(10.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(pill_fg),
                ),
        );
    if summary.error > 0 {
        row = row.child(
            icon_tinted(Icon::TriangleAlert, DANGER)
                .w(px(12.))
                .h(px(12.))
                .flex_shrink_0(),
        );
    }
    if summary.blocked > 0 {
        row = row.child(
            icon_tinted(Icon::ShieldAlert, ATTENTION)
                .w(px(12.))
                .h(px(12.))
                .flex_shrink_0(),
        );
    } else if summary.attention > 0 {
        row = row.child(
            icon_tinted(Icon::Info, ATTENTION)
                .w(px(12.))
                .h(px(12.))
                .flex_shrink_0(),
        );
    }
    if summary.working > 0 {
        row = row.child(live_dot(ACCENT));
    } else if summary.error == 0
        && summary.blocked == 0
        && summary.attention == 0
        && summary.done > 0
    {
        row = row.child(
            icon_tinted(Icon::Check, SUCCESS)
                .w(px(12.))
                .h(px(12.))
                .flex_shrink_0(),
        );
    } else if summary.error == 0
        && summary.blocked == 0
        && summary.attention == 0
        && summary.starting > 0
    {
        row = row.child(
            view()
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(MUTED)
                .flex_shrink_0(),
        );
    }
    Some(row)
}

pub fn sidebar_hint(e: &Entry, machine: &str, idle_minutes: u64) -> String {
    let mut lines = vec![
        e.title(),
        e.lifecycle_policy_summary(idle_minutes),
        format!("Project: {}", e.project_name),
    ];
    if let Some(profile) = e.profile() {
        lines.push(format!(
            "{} · {} · @{profile}",
            e.stage.as_deref().unwrap_or(""),
            e.provider().map(|(_, p)| p).unwrap_or_default()
        ));
        if e.saved.name.is_some()
            && let Some(c) = &e.card
        {
            lines.push(format!("#{} {}", c.number, c.title));
        }
    } else if let Some((_, provider)) = e.provider() {
        lines.push(format!("Agent: {provider}"));
    }
    lines.push(format!("Status: {}", e.display_status()));
    if let Some(reason) = e.attention_reason() {
        lines.push(reason);
    }
    lines.push(format!("Host: {machine}"));
    lines.join("\n")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StatusMarkKind {
    Clock,
    LiveWorking,
    MutedDot,
    BlockedShield,
    BlockedHelp,
    AttentionInfo,
    DoneCheck,
    ErrorTriangle,
}

pub(crate) fn status_mark_kind(e: &Entry) -> Option<StatusMarkKind> {
    use canban_types::HerdrStatus;
    if e.is_auto_reclaiming() || e.is_reclaiming_soon() {
        return Some(StatusMarkKind::Clock);
    }
    match e.herdr_status() {
        HerdrStatus::PlainShell | HerdrStatus::Ended => None,
        HerdrStatus::Working => Some(StatusMarkKind::LiveWorking),
        HerdrStatus::Starting | HerdrStatus::Queued | HerdrStatus::Closing => {
            Some(StatusMarkKind::MutedDot)
        }
        HerdrStatus::Blocked => match e.runtime_wait().map(|w| w.kind.as_str()) {
            Some("permission" | "approval" | "command" | "tool" | "bash" | "file") => {
                Some(StatusMarkKind::BlockedShield)
            }
            _ => Some(StatusMarkKind::BlockedHelp),
        },
        HerdrStatus::Done => {
            if e.needs_attention() {
                Some(StatusMarkKind::AttentionInfo)
            } else {
                Some(StatusMarkKind::DoneCheck)
            }
        }
        HerdrStatus::Idle => {
            if e.needs_attention() {
                Some(StatusMarkKind::AttentionInfo)
            } else {
                Some(StatusMarkKind::MutedDot)
            }
        }
        HerdrStatus::Error | HerdrStatus::Disconnected => Some(StatusMarkKind::ErrorTriangle),
    }
}

pub(crate) fn render_status_mark(kind: StatusMarkKind) -> AnyElement {
    match kind {
        StatusMarkKind::Clock => icon_tinted(Icon::Clock, MUTED)
            .w(px(12.))
            .h(px(12.))
            .flex_shrink_0()
            .into_element(),
        StatusMarkKind::LiveWorking => live_dot(ACCENT).into_element(),
        StatusMarkKind::MutedDot => view()
            .w(px(6.))
            .h(px(6.))
            .rounded_full()
            .bg(MUTED)
            .flex_shrink_0()
            .pointer_events(PointerEvents::None)
            .into_element(),
        StatusMarkKind::BlockedShield => icon_tinted(Icon::ShieldAlert, ATTENTION)
            .w(px(13.))
            .h(px(13.))
            .flex_shrink_0()
            .into_element(),
        StatusMarkKind::BlockedHelp => icon_tinted(Icon::CircleHelp, ATTENTION)
            .w(px(13.))
            .h(px(13.))
            .flex_shrink_0()
            .into_element(),
        StatusMarkKind::AttentionInfo => icon_tinted(Icon::Info, ATTENTION)
            .w(px(13.))
            .h(px(13.))
            .flex_shrink_0()
            .into_element(),
        StatusMarkKind::DoneCheck => icon_tinted(Icon::Check, SUCCESS)
            .w(px(13.))
            .h(px(13.))
            .flex_shrink_0()
            .into_element(),
        StatusMarkKind::ErrorTriangle => icon_tinted(Icon::TriangleAlert, DANGER)
            .w(px(13.))
            .h(px(13.))
            .flex_shrink_0()
            .into_element(),
    }
}

/// Compact leading Herdr status mark for Terminal sidebar rows and top/Card tab strips.
pub fn status_mark(e: &Entry) -> Option<AnyElement> {
    status_mark_kind(e).map(render_status_mark)
}

pub(crate) fn sidebar_status_spec(e: &Entry) -> Option<(&'static str, ThemeColor)> {
    use canban_types::HerdrStatus;
    let (word, color) = if e.is_auto_reclaiming() {
        ("Auto-reclaiming…", MUTED)
    } else if e.is_reclaiming_soon() {
        ("Reclaiming soon", MUTED)
    } else {
        let st = e.herdr_status();
        match st {
            HerdrStatus::PlainShell | HerdrStatus::Ended => return None,
            HerdrStatus::Working => ("Working", ACCENT),
            HerdrStatus::Starting => ("Starting", MUTED),
            HerdrStatus::Queued => ("Queued", MUTED),
            HerdrStatus::Closing => ("Closing", MUTED),
            HerdrStatus::Blocked => ("Blocked", ATTENTION),
            HerdrStatus::Done => {
                if e.needs_attention() {
                    ("Needs you", ATTENTION)
                } else {
                    ("Done", SUCCESS)
                }
            }
            HerdrStatus::Idle => {
                if e.needs_attention() {
                    ("Needs you", ATTENTION)
                } else {
                    ("Idle", MUTED)
                }
            }
            HerdrStatus::Error => ("Error", DANGER),
            HerdrStatus::Disconnected => ("Offline", DANGER),
        }
    };
    Some((word, color))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SidebarStatus {
    pub mark: Option<StatusMarkKind>,
    pub word: &'static str,
    pub color: ThemeColor,
    pub show_word: bool,
}

pub(crate) fn sidebar_status_model(e: &Entry, show_word: bool) -> Option<SidebarStatus> {
    sidebar_status_spec(e).map(|(word, color)| SidebarStatus {
        mark: status_mark_kind(e),
        word,
        color,
        show_word,
    })
}

pub(crate) fn render_sidebar_status(status: SidebarStatus) -> AnyElement {
    let mut group = view().flex_row().items_center().gap_1().flex_shrink_0();
    if let Some(mark) = status.mark {
        group = group.child(render_status_mark(mark));
    }
    if status.show_word {
        group = group.child(
            text(status.word)
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(status.color)
                .flex_shrink_0(),
        );
    }
    group.into_element()
}
fn text_width(text: &str) -> f32 {
    text.chars()
        .map(|c| if c.is_ascii() { 7.5 } else { 12. })
        .sum()
}

/// Compact display label for a Terminal tab; full Card title and column stay in
/// `context_bar`, the sidebar, and the hover tooltip.
pub fn tab_label_text(e: &Entry, show_card_number: bool) -> String {
    if let Some(name) = &e.saved.name {
        return name.clone();
    }
    if e.agent.is_some() {
        let profile = e.profile().unwrap_or("agent");
        if show_card_number && let Some(card) = &e.card {
            return format!("#{} @{profile}", card.number);
        }
        return format!("@{profile}");
    }
    e.title()
}

pub fn has_status_mark(e: &Entry) -> bool {
    use canban_types::HerdrStatus;
    e.is_auto_reclaiming()
        || e.is_reclaiming_soon()
        || !matches!(
            e.herdr_status(),
            HerdrStatus::PlainShell | HerdrStatus::Ended
        )
}

pub fn tab_width(e: &Entry, show_card_number: bool) -> f32 {
    let label_w = text_width(&tab_label_text(e, show_card_number));
    let reclaim_badge = e.is_reclaiming_soon() || e.is_auto_reclaiming();
    let status_extra = if reclaim_badge {
        108.
    } else if has_status_mark(e) {
        18.
    } else {
        0.
    };
    let managed_extra = if e.is_managed() { 20. } else { 0. };
    let icon_extra = if e.provider().is_some() { 20. } else { 0. };
    let max_w = if reclaim_badge { 296. } else { 226. };
    (label_w + 68. + status_extra + managed_extra + icon_extra).clamp(112., max_w)
}

pub fn tab(e: &Entry, selected: bool, show_card_number: bool) -> View {
    let weight = if selected {
        FontWeight::SEMIBOLD
    } else {
        FontWeight::NORMAL
    };
    let title_color = if selected { TEXT } else { MUTED };
    let label_text = tab_label_text(e, show_card_number);
    let mut row = view()
        .flex_1()
        .min_w(px(0.))
        .overflow_hidden()
        .flex_row()
        .items_center()
        .gap_1p5();
    if let Some((mark, _)) = e.provider() {
        row = row.child(icon(mark).w(px(14.)).h(px(14.)).flex_shrink_0());
    }
    row = row.child(
        label(label_text)
            .font_weight(weight)
            .text_color(title_color)
            .flex_shrink(1.)
            .min_w(px(0.))
            .truncate(),
    );
    if e.is_managed() {
        row = row.child(managed_badge());
    }
    if let Some(badge) = tab_reclaim_badge(e) {
        row = row.child(badge);
    }
    row.child(view().flex_1().min_w(px(0.)))
}
