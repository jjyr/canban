//! Compact window-chrome controls. NSH owns keyboard, focus and native tooltips.
use super::{Icon, icon, label, tokens::*};
use nsh_core::prelude::*;
pub fn chrome_button(kind: Icon, title: impl Into<String>, press: impl Fn(&()) + 'static) -> View {
    chrome_button_disabled(kind, title, || false, press)
}
pub fn chrome_button_disabled(
    kind: Icon,
    title: impl Into<String>,
    disabled: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> View {
    let title = title.into();
    let hint = title.clone();
    view()
        .flex_shrink_0()
        .window_drag(false)
        .tooltip(
            nsh_base::tooltip()
                .placement(Placement::Bottom)
                .px_3()
                .py_2()
                .rounded(px(RADIUS))
                .bg(SURFACE)
                .border_1()
                .border_color(BORDER)
                .child(label(hint)),
        )
        .child(
            nsh_base::button()
                .disabled(disabled)
                .disabled_style(|s| s.opacity(0.4))
                .w(px(30.))
                .h(px(30.))
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .bg(CLEAR)
                .hover(|s| s.bg(INSET))
                .focus(|s| s.border_1().border_color(ACCENT))
                .accessibility_label(move || Some(title.clone().into()))
                .child(icon(kind).w(px(16.)).h(px(16.)))
                .on_press(press),
        )
}
