//! Navigation row and sub-page header controls; selection stays distinct from hover and focus.
use super::{Icon, chrome_button, heading, label, tokens::*};
use nsh_core::prelude::*;
pub fn navigation(
    title: String,
    selected: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    crate::press_target()
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

/// Consistent left-aligned Back button + page heading for sub-pages and secondary routes.
pub fn subpage_header(
    back_title: impl Into<String>,
    title: impl IntoReactive<SharedString>,
    on_back: impl Fn(&()) + 'static,
) -> View {
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .child(chrome_button(Icon::Back, back_title, on_back))
        .child(heading(title).flex_1().min_w(px(0.)).truncate())
}
