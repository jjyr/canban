//! Controlled field; NSH Input preserves selection and IME composition.
use super::tokens::*;
use nsh_core::{element::Input, prelude::*};
pub fn field(value: Signal<String>, placeholder: &str) -> Input {
    nsh_base::input()
        .value(move || value.get())
        .on_change(move |v: &str| value.set(v.to_owned()))
        .placeholder(placeholder.to_owned())
        .min_h(px(CONTROL_HEIGHT))
        .px_3()
        .py_2()
        .rounded(px(RADIUS))
        .bg(SURFACE)
        .border_1()
        .border_color(BORDER)
        .text_size(px(13.0))
        .text_color(TEXT)
        .focus(|s| s.border_color(ACCENT))
}

/// Controlled field for a document draft owned by its presenter.
pub fn bound_field(
    value: impl Fn() -> String + 'static,
    change: impl Fn(&str) + 'static,
    placeholder: &str,
) -> Input {
    nsh_base::input()
        .value(value)
        .on_change(change)
        .placeholder(placeholder.to_owned())
        .min_h(px(CONTROL_HEIGHT))
        .px_3()
        .py_2()
        .rounded(px(RADIUS))
        .bg(SURFACE)
        .border_1()
        .border_color(BORDER)
        .text_size(px(BODY_SIZE))
        .text_color(TEXT)
        .focus(|s| s.border_color(ACCENT))
}
