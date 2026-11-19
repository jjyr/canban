//! Semantic status; consumers map product state to a generic visual tone.
use super::{Icon, icon_tinted, tokens::*};
use nsh_core::prelude::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Info,
    Highlight,
    Success,
    Attention,
    Danger,
}
pub fn tone_colors(tone: Tone) -> (ThemeColor, ThemeColor) {
    match tone {
        Tone::Neutral => (INSET, MUTED),
        Tone::Info => (SELECTED, ACCENT),
        Tone::Highlight => (HIGHLIGHT_SURFACE, HIGHLIGHT),
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

/// Interactive semantic status pill with optional live dot or leading icon.
pub fn status_button(
    value: impl IntoReactive<SharedString>,
    tone: Tone,
    live: bool,
    leading: Option<Icon>,
    compact: bool,
    on_press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    let (bg, fg) = tone_colors(tone);
    let mut chip = super::press_target()
        .self_start()
        .flex_row()
        .items_center()
        .gap_1()
        .h(px(if compact { 20. } else { 24. }))
        .px(px(if compact { 6. } else { 8. }))
        .rounded(px(5.0))
        .bg(bg)
        .border_1()
        .border_color(CLEAR)
        .hover(|s| s.border_color(BORDER).opacity(0.92))
        .active(|s| s.opacity(0.72))
        .focus(|s| s.border_color(ACCENT))
        .on_press(on_press);
    if live {
        chip = chip.child(live_dot(fg));
    } else if let Some(kind) = leading {
        let size = if compact { 11. } else { 12. };
        chip = chip.child(
            icon_tinted(kind, fg)
                .w(px(size))
                .h(px(size))
                .flex_shrink_0(),
        );
    }
    chip.child(
        text(value)
            .text_size(px(if compact { 11. } else { CAPTION_SIZE }))
            .font_weight(FontWeight::MEDIUM)
            .text_color(fg)
            .truncate(),
    )
}
pub fn notice(message: impl IntoReactive<SharedString>, tone: Tone) -> impl IntoElement {
    let (bg, border, color) = match tone {
        Tone::Danger => (DANGER_SURFACE, DANGER, DANGER),
        Tone::Attention => (ATTENTION_SURFACE, ATTENTION, ATTENTION),
        Tone::Success => (SUCCESS_SURFACE, SUCCESS, SUCCESS),
        Tone::Info => (SELECTED, ACCENT, ACCENT),
        Tone::Highlight => (HIGHLIGHT_SURFACE, HIGHLIGHT, HIGHLIGHT),
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

/// Continuously rotating spinner icon using native NSH timing animation.
/// Respects reduced motion by rendering a static icon while keeping the loading state clear.
pub fn loading_spinner(size: f32, color: ThemeColor) -> View {
    use nsh_core::element::Animation;
    let angle = animated_value(0.);
    if !crate::motion::reduced() {
        let animation =
            Animation::timing(&angle, 360., Timing::new(900., Easing::Linear)).looped(None);
        let _ = animation.start();
        cleanup(move || animation.stop());
    }
    view()
        .w(px(size))
        .h(px(size))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .pointer_events(PointerEvents::None)
        .bind_rotate(&angle)
        .child(icon_tinted(Icon::Refresh, color).w(px(size)).h(px(size)))
}

/// 28x28 rounded semantic badge containing a continuously rotating spinner.
pub fn loading_icon_badge(tone: Tone) -> View {
    let (bg, fg) = tone_colors(tone);
    view()
        .relative()
        .flex_shrink_0()
        .w(px(28.))
        .h(px(28.))
        .rounded(px(7.))
        .bg(bg)
        .items_center()
        .justify_center()
        .pointer_events(PointerEvents::None)
        .child(loading_spinner(15., fg))
}

/// Horizontal progress bar with rounded track and semantic fill.
pub fn progress_bar(fraction: f32, tone: Tone) -> View {
    let (_, fg) = tone_colors(tone);
    let clamped = fraction.clamp(0.0, 1.0);
    view()
        .flex_1()
        .min_w(px(0.))
        .h(px(6.))
        .rounded_full()
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .overflow_hidden()
        .child(view().h_full().w(Percentage(clamped)).rounded_full().bg(fg))
}

/// Compact inline loading indicator with spinner animation and muted status message.
pub fn loading_indicator(message: impl IntoReactive<SharedString>) -> View {
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .child(loading_spinner(13., ACCENT))
        .child(super::muted(message).min_w(px(0.)).flex_1())
}

/// Elevated placeholder card shown while an async section or settings panel is loading.
pub fn loading_panel(
    title: impl IntoReactive<SharedString>,
    detail: impl IntoReactive<SharedString>,
) -> View {
    let skeleton_bar = |width: f32| {
        let bar = view()
            .h(px(8.))
            .w(Percentage(width))
            .rounded_full()
            .bg(BORDER);
        if crate::motion::reduced() {
            bar.opacity(0.55)
        } else {
            bar.opacity(0.45)
        }
    };
    super::panel()
        .p_4()
        .gap_2p5()
        .bg(INSET)
        .border_1()
        .border_color(BORDER)
        .min_w(px(0.))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w(px(0.))
                .child(loading_spinner(15., ACCENT))
                .child(
                    super::label(title)
                        .font_weight(FontWeight::MEDIUM)
                        .min_w(px(0.))
                        .flex_1(),
                )
                .child(live_dot(ACCENT)),
        )
        .child(super::muted(detail).min_w(px(0.)))
        .child(
            view()
                .pt_1()
                .gap_1p5()
                .child(skeleton_bar(0.72))
                .child(skeleton_bar(0.48)),
        )
}
