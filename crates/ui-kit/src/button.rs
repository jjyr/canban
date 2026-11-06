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
        Variant::Quiet => (BACKGROUND, MUTED),
        Variant::Secondary => (SURFACE, TEXT),
    };
    nsh_base::button()
        .min_h(px(CONTROL_HEIGHT))
        .px_3()
        .justify_center()
        .items_center()
        .rounded(px(RADIUS))
        .bg(background)
        .border_1()
        .border_color(BORDER)
        .hover(|s| s.opacity(0.82))
        .active(|s| s.opacity(0.6))
        .focus(|s| s.border_color(ACCENT))
        .disabled_style(|s| s.opacity(0.4))
        .child(
            text(label.into())
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(foreground),
        )
        .on_press(on_press)
}
