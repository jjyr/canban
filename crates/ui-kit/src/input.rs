//! Controlled field; NSH Input preserves selection and IME composition.
use super::tokens::*;
use nsh_core::{element::Input, prelude::*};
/// A trailing-edge debounce owned by the input's reactive scope. Each keystroke
/// disposes the previous timeout; native input identity and selection stay put.
pub fn debounced<T: Clone + PartialEq + 'static>(
    value: Signal<T>,
    delay: std::time::Duration,
) -> Memo<T> {
    let settled = signal(value.get_untracked());
    effect(move || {
        let next = value.get();
        if next != settled.get_untracked() {
            set_timeout(delay, move || settled.set(next));
        }
    });
    computed(move || settled.get())
}
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

/// Masked/revealable field backed by reactive `revealed` signal for secret entry.
pub fn secret_field(value: Signal<String>, revealed: Signal<bool>, placeholder: &str) -> Input {
    nsh_base::input()
        .value(move || value.get())
        .on_change(move |v: &str| value.set(v.to_owned()))
        .masked(move || !revealed.get())
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

/// Borderless transparent text input for single-layer tables and grouped steppers.
pub fn cell_field(
    value: impl Fn() -> String + 'static,
    change: impl Fn(&str) + 'static,
    placeholder: &str,
) -> Input {
    nsh_base::input()
        .value(value)
        .on_change(change)
        .placeholder(placeholder.to_owned())
        .min_h(px(32.))
        .px_2p5()
        .py_1p5()
        .bg(CLEAR)
        .text_size(px(BODY_SIZE))
        .text_color(TEXT)
        .focus(|s| s.bg(INSET))
}

/// Borderless transparent masked/revealable secret input for single-layer Env tables.
pub fn cell_secret_field(
    value: Signal<String>,
    revealed: Signal<bool>,
    placeholder: &str,
) -> Input {
    nsh_base::input()
        .value(move || value.get())
        .on_change(move |v: &str| value.set(v.to_owned()))
        .masked(move || !revealed.get())
        .placeholder(placeholder.to_owned())
        .min_h(px(32.))
        .px_2p5()
        .py_1p5()
        .bg(CLEAR)
        .text_size(px(BODY_SIZE))
        .text_color(TEXT)
        .focus(|s| s.bg(INSET))
}
