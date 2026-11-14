//! Semantic status; consumers map product state to a generic visual tone.
use super::{Icon, icon_tinted, tokens::*};
use nsh_core::prelude::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Info,
    Success,
    Attention,
    Danger,
}
pub fn tone_colors(tone: Tone) -> (ThemeColor, ThemeColor) {
    match tone {
        Tone::Neutral => (INSET, MUTED),
        Tone::Info => (SELECTED, ACCENT),
        Tone::Success => (SUCCESS_SURFACE, SUCCESS),
        Tone::Attention => (ATTENTION_SURFACE, ATTENTION),
        Tone::Danger => (DANGER_SURFACE, DANGER),
    }
}
/// 28x28 rounded semantic icon tile for event-first activity and notification rows.
/// An optional live indicator anchors at the bottom-right corner without hiding the glyph.
pub fn tone_icon_badge(kind: Icon, tone: Tone, live: bool) -> View {
    let (bg, fg) = tone_colors(tone);
    let mut tile = view()
        .relative()
        .flex_shrink_0()
        .w(px(28.))
        .h(px(28.))
        .rounded(px(7.))
        .bg(bg)
        .items_center()
        .justify_center()
        .pointer_events(PointerEvents::None)
        .child(icon_tinted(kind, fg).w(px(15.)).h(px(15.)));
    if live {
        tile = tile.child(
            view()
                .absolute()
                .right(px(-2.))
                .bottom(px(-2.))
                .w(px(10.))
                .h(px(10.))
                .rounded_full()
                .bg(SURFACE)
                .items_center()
                .justify_center()
                .child(view().w(px(6.)).h(px(6.)).rounded_full().bg(SUCCESS)),
        );
    }
    tile
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
pub fn status(value: impl IntoReactive<SharedString>, tone: Tone) -> View {
    status_chip(value, tone, false)
}
pub fn live_status(value: impl IntoReactive<SharedString>, tone: Tone) -> View {
    status_chip(value, tone, true)
}
fn status_chip(value: impl IntoReactive<SharedString>, tone: Tone, live: bool) -> View {
    let (bg, fg) = tone_colors(tone);
    let mut chip = view()
        .self_start()
        .flex_row()
        .items_center()
        .gap_1p5()
        .px_2()
        .py_1()
        .rounded(px(5.0))
        .bg(bg);
    if live {
        chip = chip.child(live_dot(fg));
    }
    chip.child(
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
