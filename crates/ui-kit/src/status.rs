//! Semantic status; consumers map product state to a generic visual tone.
use super::tokens::*;
use nsh_core::prelude::*;
#[derive(Clone, Copy)]
pub enum Tone {
    Neutral,
    Info,
    Success,
    Attention,
    Danger,
}
/// Compact numeric attention badge, with no zero/empty state label.
pub fn attention_count(count: impl Fn() -> usize + Copy + 'static) -> impl IntoElement {
    when(
        move || count() > 0,
        move || {
            view()
                .min_w(px(20.))
                .h(px(18.))
                .px_1()
                .rounded(px(6.))
                .bg(ATTENTION_SURFACE)
                .items_center()
                .justify_center()
                .child(
                    text(move || {
                        if count() > 99 {
                            "99+".into()
                        } else {
                            count().to_string()
                        }
                    })
                    .text_size(px(10.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ATTENTION),
                )
        },
    )
}
pub fn status(value: impl IntoReactive<SharedString>, tone: Tone) -> impl IntoElement {
    let (bg, fg) = match tone {
        Tone::Neutral => (INSET, MUTED),
        Tone::Info => (SELECTED, ACCENT),
        Tone::Success => (SUCCESS_SURFACE, SUCCESS),
        Tone::Attention => (ATTENTION_SURFACE, ATTENTION),
        Tone::Danger => (DANGER_SURFACE, DANGER),
    };
    view()
        .self_start()
        .flex_row()
        .items_center()
        .px_2()
        .py_1()
        .rounded(px(5.0))
        .bg(bg)
        .child(
            text(value)
                .text_size(px(CAPTION_SIZE))
                .font_weight(FontWeight::MEDIUM)
                .text_color(fg),
        )
}
pub fn notice(message: impl IntoReactive<SharedString>, tone: Tone) -> impl IntoElement {
    let color = match tone {
        Tone::Danger => DANGER,
        Tone::Attention => ATTENTION,
        _ => MUTED,
    };
    view()
        .px_3()
        .py_2()
        .bg(INSET)
        .rounded(px(RADIUS))
        .child(text(message).text_size(px(BODY_SIZE)).text_color(color))
}
