//! Shared Terminal presentation: sidebar metadata and compact tab labels.
use super::*;

pub fn provider(e: &Entry) -> View {
    let Some((mark, name)) = e.provider() else {
        return view();
    };
    view()
        .flex_row()
        .items_center()
        .gap_1()
        .min_w(px(0.))
        .tooltip(hint_tooltip(name).placement(Placement::Bottom))
        .child(icon(mark).w(px(14.)).h(px(14.)))
        .child(
            muted(e.profile().unwrap_or_default().to_owned())
                .flex_1()
                .min_w(px(0.))
                .truncate(),
        )
}
pub const SIDEBAR_TITLE_MAX_CHARS: usize = 32;

/// Truncates a sidebar session title to at most `SIDEBAR_TITLE_MAX_CHARS` characters,
/// collapsing internal whitespace/newlines onto a single line and appending `…` when exceeded.
pub fn truncate_sidebar_title(title: &str) -> String {
    let single_line = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = single_line.chars();
    let head: String = chars.by_ref().take(SIDEBAR_TITLE_MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{}…", head.trim_end())
    } else {
        head
    }
}

/// Formats the left-side identity/machine caption for the second line of a Terminal sidebar row:
/// - Card Role Agent: `@role · Local` (or remote machine name), omitting engine display name and Card stage;
/// - Standalone Agent / Shell with observed Agent / Plain Shell: `Local` (or remote machine name).
pub fn sidebar_meta_text(e: &Entry, machine: &str) -> String {
    if let Some(profile) = e.profile().filter(|p| !p.is_empty()) {
        format!("@{profile} · {machine}")
    } else {
        machine.to_owned()
    }
}

/// Leading icon for the second line of a Terminal sidebar row: Agent brand mark when present,
/// otherwise the generic `Icon::Terminal` mark.
pub fn sidebar_meta_icon(e: &Entry) -> Icon {
    e.provider().map(|(mark, _)| mark).unwrap_or(Icon::Terminal)
}

/// Sidebar metadata on the second row: `[Agent/Terminal Icon]` + `(@role · )Machine`, single-line truncated.
pub fn sidebar_meta(e: &Entry, machine: &str) -> View {
    let mark = sidebar_meta_icon(e);
    let caption = sidebar_meta_text(e, machine);
    view()
        .flex_1()
        .min_w(px(0.))
        .flex_row()
        .items_center()
        .gap_1p5()
        .child(icon(mark).w(px(13.)).h(px(13.)).flex_shrink_0())
        .child(muted(caption).flex_1().min_w(px(0.)).truncate())
}

/// Right-aligned compact Herdr status indicator (`[status_mark] + [status_word]`) for the second line
/// of a Terminal sidebar row. Plain idle Shells return `None` to stay quiet.
pub fn sidebar_status(e: &Entry) -> Option<AnyElement> {
    let label_el = sidebar_status_label_raw(e)?;
    let mut group = view().flex_row().items_center().gap_1().flex_shrink_0();
    if let Some(mark_el) = status_mark(e) {
        group = group.child(mark_el);
    }
    group = group.child(label_el);
    Some(group.into_element())
}

pub fn sidebar_hint(e: &Entry, machine: &str) -> String {
    let mut lines = vec![e.title(), format!("Project: {}", e.project_name)];
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

/// Compact leading Herdr status mark for Terminal sidebar rows and top/Card tab strips.
pub fn status_mark(e: &Entry) -> Option<AnyElement> {
    use canban_types::HerdrStatus;
    match e.herdr_status() {
        HerdrStatus::PlainShell | HerdrStatus::Ended => None,
        HerdrStatus::Working => Some(live_dot(ACCENT).into_element()),
        HerdrStatus::Starting | HerdrStatus::Queued | HerdrStatus::Closing => Some(
            view()
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(MUTED)
                .flex_shrink_0()
                .pointer_events(PointerEvents::None)
                .into_element(),
        ),
        HerdrStatus::Blocked => {
            let blocked_icon = match e.runtime_wait().map(|w| w.kind.as_str()) {
                Some("permission" | "approval" | "command" | "tool" | "bash" | "file") => {
                    Icon::ShieldAlert
                }
                _ => Icon::CircleHelp,
            };
            Some(
                icon_tinted(blocked_icon, ATTENTION)
                    .w(px(13.))
                    .h(px(13.))
                    .flex_shrink_0()
                    .into_element(),
            )
        }
        HerdrStatus::Done => {
            if e.needs_attention() {
                Some(
                    icon_tinted(Icon::Info, ATTENTION)
                        .w(px(13.))
                        .h(px(13.))
                        .flex_shrink_0()
                        .into_element(),
                )
            } else {
                Some(
                    icon_tinted(Icon::Check, SUCCESS)
                        .w(px(13.))
                        .h(px(13.))
                        .flex_shrink_0()
                        .into_element(),
                )
            }
        }
        HerdrStatus::Idle => {
            if e.needs_attention() {
                Some(
                    icon_tinted(Icon::Info, ATTENTION)
                        .w(px(13.))
                        .h(px(13.))
                        .flex_shrink_0()
                        .into_element(),
                )
            } else {
                Some(
                    view()
                        .w(px(6.))
                        .h(px(6.))
                        .rounded_full()
                        .bg(MUTED)
                        .flex_shrink_0()
                        .pointer_events(PointerEvents::None)
                        .into_element(),
                )
            }
        }
        HerdrStatus::Error | HerdrStatus::Disconnected => Some(
            icon_tinted(Icon::TriangleAlert, DANGER)
                .w(px(13.))
                .h(px(13.))
                .flex_shrink_0()
                .into_element(),
        ),
    }
}

fn sidebar_status_label_raw(e: &Entry) -> Option<AnyElement> {
    use canban_types::HerdrStatus;
    let st = e.herdr_status();
    let (word, color) = match st {
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
    };
    Some(
        text(word)
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(color)
            .flex_shrink_0()
            .into_element(),
    )
}
pub fn profile_width(e: &Entry) -> f32 {
    (text_width(e.profile().unwrap_or_default()) + 22.).clamp(52., 128.)
}
pub fn stage_width(e: &Entry) -> f32 {
    (text_width(e.stage.as_deref().unwrap_or_default()) + 4.).clamp(40., 112.)
}
fn text_width(text: &str) -> f32 {
    text.chars()
        .map(|c| if c.is_ascii() { 7.5 } else { 12. })
        .sum()
}
pub fn tab_width(e: &Entry) -> f32 {
    let text = e.title().chars().count() as f32 * 7.;
    if e.agent.is_some() {
        (text + profile_width(e) + stage_width(e) + 78.).clamp(280., 380.)
    } else {
        let icon_extra = if e.provider().is_some() { 20. } else { 0. };
        (text + 58. + icon_extra).clamp(136. + icon_extra, 230.)
    }
}
pub fn tab(e: &Entry, selected: bool) -> View {
    let weight = if selected {
        FontWeight::SEMIBOLD
    } else {
        FontWeight::NORMAL
    };
    let title_color = if selected { TEXT } else { MUTED };
    let title = if e.agent.is_some() && e.saved.name.is_none() {
        let card = e.card.as_ref().unwrap();
        let number = format!("#{}", card.number);
        view()
            .flex_1()
            .min_w(px(text_width(&number) + 2.))
            .flex_row()
            .items_center()
            .gap_1()
            .child(
                label(number)
                    .font_weight(weight)
                    .text_color(title_color)
                    .flex_shrink_0(),
            )
            .child(
                label(card.title.clone())
                    .font_weight(weight)
                    .text_color(title_color)
                    .flex_1()
                    .min_w(px(0.))
                    .truncate(),
            )
    } else {
        view().flex_1().min_w(px(0.)).child(
            label(e.title())
                .font_weight(weight)
                .text_color(title_color)
                .w_full()
                .truncate(),
        )
    };
    if e.agent.is_some() {
        view()
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .flex_row()
            .items_center()
            .gap_2()
            .child(title)
            .child(
                muted(e.stage.clone().unwrap_or_default())
                    .w(px(stage_width(e)))
                    .flex_shrink_0()
                    .truncate(),
            )
            .child(provider(e).w(px(profile_width(e))).flex_shrink_0())
    } else if let Some((mark, _)) = e.provider() {
        view()
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .flex_row()
            .items_center()
            .gap_1p5()
            .child(icon(mark).w(px(14.)).h(px(14.)).flex_shrink_0())
            .child(title)
    } else {
        view()
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .flex_row()
            .items_center()
            .gap_2()
            .child(title)
    }
}
