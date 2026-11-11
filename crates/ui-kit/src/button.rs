//! Button contract: typed variant; NSH owns press/keyboard/disabled semantics.
use super::{Icon, icon_tinted, tokens::*};
use nsh_core::prelude::*;
#[derive(Clone, Copy)]
pub enum Variant {
    Primary,
    Secondary,
    Danger,
    Quiet,
}
pub fn action(
    label: impl IntoReactive<SharedString>,
    variant: Variant,
    on_press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    base(variant)
        .child(action_label(label, variant))
        .on_press(on_press)
}
/// The same action contract with a leading semantic icon.
pub fn action_with_icon(
    kind: Icon,
    label: impl IntoReactive<SharedString>,
    variant: Variant,
    on_press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    base(variant)
        .flex_row()
        .items_center()
        .gap_2()
        .child(icon_tinted(kind, foreground(variant)).w(px(16.)).h(px(16.)))
        .child(
            action_label(label, variant)
                .truncate()
                .min_w(px(0.))
                .flex_1(),
        )
        .on_press(on_press)
}
fn foreground(variant: Variant) -> ThemeColor {
    match variant {
        Variant::Primary => ON_ACCENT,
        Variant::Danger => DANGER,
        Variant::Quiet => MUTED,
        Variant::Secondary => TEXT,
    }
}
fn action_label(label: impl IntoReactive<SharedString>, variant: Variant) -> Text {
    text(label)
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(foreground(variant))
}
fn base(variant: Variant) -> nsh_base::ButtonElement {
    let background = match variant {
        Variant::Primary => ACCENT_FILL,
        Variant::Danger | Variant::Secondary => SURFACE,
        Variant::Quiet => CLEAR,
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
        .disabled_style(|s| s.opacity(0.4));
    if matches!(variant, Variant::Quiet) {
        button.items_start().justify_center()
    } else {
        button
    }
}
