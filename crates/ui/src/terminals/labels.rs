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
/// Sidebar metadata is visible text, without a nested provider tooltip.
pub fn sidebar_provider(e: &Entry, expanded: bool) -> View {
    let Some((mark, provider)) = e.provider() else {
        return view();
    };
    let profile = e.profile().unwrap_or_default();
    let title = if provider.eq_ignore_ascii_case(profile) {
        provider
    } else {
        format!("{provider} · {profile}")
    };
    let name = muted(title).flex_1().min_w(px(0.));
    view()
        .flex_row()
        .items_start()
        .gap_1()
        .min_w(px(0.))
        .child(icon(mark).w(px(14.)).h(px(14.)).flex_shrink_0())
        .child(if expanded { name } else { name.truncate() })
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
    }
    if let Some(reason) = e.attention_reason() {
        lines.push(reason);
    }
    lines.push(format!("Machine: {machine}"));
    lines.join("\n")
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
        (text + 58.).clamp(136., 230.)
    }
}
pub fn tab(e: &Entry) -> View {
    let title = if e.agent.is_some() && e.saved.name.is_none() {
        let card = e.card.as_ref().unwrap();
        let number = format!("#{}", card.number);
        view()
            .flex_1()
            .min_w(px(text_width(&number) + 2.))
            .flex_row()
            .items_center()
            .gap_1()
            .child(label(number).flex_shrink_0())
            .child(label(card.title.clone()).flex_1().min_w(px(0.)).truncate())
    } else {
        view()
            .flex_1()
            .min_w(px(0.))
            .child(label(e.title()).w_full().truncate())
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
