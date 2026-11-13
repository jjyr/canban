//! One accessible, fully clickable entry: icon, title and supporting text.
use super::{Icon, icon_tinted, label, muted, tokens::*};
use nsh_core::prelude::*;
pub fn entry_card(
    kind: Icon,
    title: &str,
    description: &str,
    press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    let title = title.to_owned();
    let accessible = title.clone();
    crate::press_target()
        .w_full()
        .max_w(px(640.))
        .min_w(px(0.))
        .p_4()
        .gap_3()
        .flex_row()
        .items_center()
        .rounded(px(RADIUS))
        .bg(SURFACE)
        .border_1()
        .border_color(BORDER)
        .hover(|s| s.bg(SELECTED).border_color(ACCENT))
        .active(|s| s.opacity(0.8))
        .focus(|s| s.border_color(ACCENT))
        .accessibility_label(move || Some(accessible.clone().into()))
        .child(
            view()
                .w(px(40.))
                .h(px(40.))
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .bg(SELECTED)
                .rounded(px(RADIUS))
                .child(icon_tinted(kind, ACCENT)),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.))
                .gap_1()
                .child(label(title).font_weight(FontWeight::SEMIBOLD))
                .child(muted(description.to_owned())),
        )
        .on_press(press)
}
