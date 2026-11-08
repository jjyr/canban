//! Shared production window frame, including split Terminal chrome and overlays.
use crate::{presenter::*, quick_open, screens, sidebar, terminals};
use canban_ui_kit::tokens::*;
use nsh_core::prelude::*;
pub fn screen(ui: Ui) -> impl IntoElement {
    view()
        .flex_1()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .on_layout(move |b: &Bounds<Points>| ui.narrow.set(b.size.width.0 < 950.))
        .child(dynamic(move || {
            let page = ui.page.get();
            let _ = ui.card.get();
            let _ = ui.project.get();
            untracked(|| {
                if page == "terminals" {
                    terminals::screen(ui).into_element()
                } else {
                    view()
                        .flex_1()
                        .min_h(px(0.))
                        .child(
                            view()
                                .h(px(42.))
                                .flex_shrink_0()
                                .flex_row()
                                .items_center()
                                .bg(canban_ui_kit::tokens::SURFACE)
                                .window_drag(true)
                                .pl(move || {
                                    px(nsh_core::element::desktop::title_bar().left.0.max(78.)
                                        + 12.)
                                })
                                .child(canban_ui_kit::muted("Canban")),
                        )
                        .child(
                            view()
                                .flex_1()
                                .min_h(px(0.))
                                .flex_row()
                                .child(sidebar(ui))
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .child(screens::screen(ui, &page)),
                                ),
                        )
                        .into_element()
                }
            })
        }))
        .child(when(
            move || !ui.operation_error.get().is_empty(),
            move || {
                view()
                    .absolute()
                    .bottom(px(16.))
                    .right(px(16.))
                    .max_w(px(520.))
                    .p_3()
                    .bg(canban_ui_kit::tokens::ATTENTION_SURFACE)
                    .rounded(px(8.))
                    .flex_row()
                    .gap_2()
                    .child(canban_ui_kit::muted(move || ui.operation_error.get()))
                    .child(canban_ui_kit::chrome_button(
                        canban_ui_kit::Icon::Close,
                        "Dismiss message",
                        move |_| ui.operation_error.set(String::new()),
                    ))
            },
        ))
        .child(quick_open::overlay(ui))
}
