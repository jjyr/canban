//! Navigation row: selection is separate from hover and keyboard focus.
use super::{label, tokens::*};
use nsh_core::prelude::*;
pub fn navigation(
    title: String,
    selected: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    nsh_base::button()
        .min_h(px(CONTROL_HEIGHT))
        .px_3()
        .py_2()
        .rounded(px(RADIUS))
        .bg(move || if selected() { SELECTED } else { SURFACE })
        .border_1()
        .border_color(SURFACE)
        .hover(|s| s.bg(INSET))
        .focus(|s| s.border_color(ACCENT))
        .child(label(title).font_weight(FontWeight::MEDIUM))
        .on_press(press)
}
