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
    reminder_count(count, || true)
}
/// One count with urgency supplied by its owner; no icon or empty-state label.
pub fn reminder_count(
    count: impl Fn() -> usize + Copy + 'static,
    pending: impl Fn() -> bool + Copy + 'static,
) -> impl IntoElement {
    when(
        move || count() > 0,
        move || {
            view()
                .min_w(px(20.))
                .h(px(18.))
                .px_1()
                .rounded(px(6.))
                .bg(move || {
                    if pending() {
                        ATTENTION_SURFACE.resolve()
                    } else {
                        INSET.resolve()
                    }
                })
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
                    .text_color(move || {
                        if pending() {
                            ATTENTION.resolve()
                        } else {
                            MUTED.resolve()
                        }
                    }),
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
    let (bg, border, color) = match tone {
        Tone::Danger => (DANGER_SURFACE, DANGER, DANGER),
        Tone::Attention => (ATTENTION_SURFACE, ATTENTION, ATTENTION),
        Tone::Success => (SUCCESS_SURFACE, SUCCESS, SUCCESS),
        Tone::Info => (SELECTED, ACCENT, ACCENT),
        Tone::Neutral => (INSET, BORDER, MUTED),
    };
    view()
        .px_3()
        .py_2()
        .bg(bg)
        .border_1()
        .border_color(border)
        .rounded(px(RADIUS))
        .child(text(message).text_size(px(BODY_SIZE)).text_color(color))
}
/// Looping "work in progress" mark: a steady dot with a ping halo. Reduced
/// motion keeps the steady dot so the state stays visible without movement.
pub fn live_dot(color: ThemeColor) -> View {
    const SIZE: f32 = 8.0;
    let dot = || {
        view()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .w(px(SIZE))
            .h(px(SIZE))
            .rounded(px(SIZE / 2.))
            .bg(color)
    };
    let mark = view()
        .relative()
        .flex_shrink_0()
        .w(px(SIZE))
        .h(px(SIZE))
        .pointer_events(PointerEvents::None);
    if crate::motion::reduced() {
        mark.child(dot())
    } else {
        mark.child(dot().opacity(0.6).animate_ping()).child(dot())
    }
}
