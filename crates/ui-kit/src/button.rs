//! Button contract: typed variant; NSH owns press/keyboard/disabled semantics.
use super::tokens::*;
use nsh_core::prelude::*;
#[derive(Clone, Copy)]
pub enum Variant {
    Primary,
    Secondary,
    Danger,
    Quiet,
}
pub fn action(
    label: impl Into<SharedString>,
    variant: Variant,
    on_press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    let (background, foreground) = match variant {
        Variant::Primary => (ACCENT_FILL, ON_ACCENT),
        Variant::Danger => (SURFACE, DANGER),
        Variant::Quiet => (CLEAR, MUTED),
        Variant::Secondary => (SURFACE, TEXT),
    };
    let button = nsh_base::button()
        .min_h(px(CONTROL_HEIGHT))
        .px_3()
        .justify_center()
        .items_center()
        .rounded(px(RADIUS))
        .bg(background)
        .border_1()
        .border_color(if matches!(variant, Variant::Quiet) {
            CLEAR
        } else {
            BORDER
        })
        .hover(move |s| {
            if matches!(variant, Variant::Primary) {
                s.opacity(0.9)
            } else {
                s.bg(INSET)
            }
        })
        .active(|s| s.opacity(0.6))
        .focus(|s| s.border_color(ACCENT))
        .disabled_style(|s| s.opacity(0.4))
        .child(
            text(label.into())
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(foreground),
        )
        .on_press(on_press);
    if matches!(variant, Variant::Quiet) {
        button.items_start().justify_center()
    } else {
        button
    }
}
