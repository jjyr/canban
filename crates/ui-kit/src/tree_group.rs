//! Controlled outline group: NSH owns disclosure layout and button keys; the
//! host owns expansion and supplies already-filtered children and counts.
use super::{tokens::*, *};
use nsh_core::prelude::*;
pub fn tree_group(
    title: impl Fn() -> String + Copy + 'static,
    count: impl Fn() -> usize + Copy + 'static,
    open: Memo<bool>,
    toggle: impl Fn() + 'static,
    children: impl IntoElement,
) -> impl IntoElement {
    nsh_base::collapsible()
        .open(move || open.get())
        .reveal(move || !super::motion::reduced())
        .min_w(px(0.))
        .child(
            view()
                .role(nsh_core::AccessibilityRole::Button)
                .focusable(true)
                .w_full()
                .min_w(px(0.))
                .min_h(px(36.))
                .flex_row()
                .items_center()
                .gap_2()
                .px_2()
                .rounded(px(RADIUS))
                .bg(CLEAR)
                .hover(|s| s.bg(INSET))
                .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
                .label(move || {
                    format!(
                        "{} · {} item{}",
                        title(),
                        count(),
                        if count() == 1 { "" } else { "s" }
                    )
                })
                .accessibility_state(move || nsh_core::AccessibilityState {
                    expanded: Some(open.get()),
                    ..Default::default()
                })
                .child(dynamic(move || {
                    icon(if open.get() {
                        Icon::ChevronDown
                    } else {
                        Icon::Forward
                    })
                    .w(px(12.))
                    .h(px(12.))
                }))
                .child(icon(Icon::Folder).w(px(15.)).h(px(15.)))
                .child(
                    label(title)
                        .font_weight(FontWeight::SEMIBOLD)
                        .flex_1()
                        .min_w(px(0.))
                        .truncate(),
                )
                .child(muted(move || count().to_string()).text_size(px(12.)))
                .on_press(toggle),
        )
        .content(
            view()
                .ml_3()
                .pl_3()
                .gap_2()
                .min_w(px(0.))
                .border_l_1()
                .border_color(BORDER)
                .accessibility_elements_hidden(move || !open.get())
                .pointer_events(move || {
                    if open.get() {
                        PointerEvents::Auto
                    } else {
                        PointerEvents::None
                    }
                })
                .child(children),
        )
}
