//! Shared production window frame, including split Terminal chrome and overlays.
use crate::{presenter::*, quick_open, screens, sidebar, terminals};
use canban_ui_kit::tokens::*;
use nsh_core::prelude::*;

/// Keep production and Gallery custom chrome on the same native window API.
pub fn configure_window_chrome() {
    let mut title_bar = nsh_core::desktop::TitleBarOptions::default();
    title_bar.style = nsh_core::desktop::TitleBarStyle::Hidden;
    title_bar.height = Some(px(42.));
    let mut attributes = nsh_core::desktop::WindowAttributes::default();
    attributes.title_bar = Some(title_bar);
    nsh_core::element::desktop::current_window().set(attributes);
}

pub fn screen(ui: Ui) -> impl IntoElement {
    let route = computed(move || {
        let page = ui.page.get();
        if matches!(page.as_str(), "board" | "card" | "new-card") {
            ("workspace".to_owned(), String::new(), ui.project.get())
        } else {
            (page, ui.card.get(), ui.project.get())
        }
    });
    view()
        .flex_1()
        .min_h(px(0.))
        .bg(BACKGROUND)
        .on_layout(move |b: &Bounds<Points>| ui.narrow.set(b.size.width.0 < 950.))
        .child(dynamic(move || {
            let (page, _, _) = route.get();
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
                                .child(crate::navigation::controls(ui))
                                .child(when(
                                    move || {
                                        ui.page.get() == "board"
                                            || ui.page.get() == "card"
                                                && !ui.card_standalone.get()
                                                && !ui.narrow.get()
                                    },
                                    move || crate::board::activity::strip(ui),
                                )),
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
            move || !ui.backend_notice.get().is_empty(),
            move || {
                canban_ui_kit::notice(
                    move || ui.backend_notice.get(),
                    canban_ui_kit::Tone::Attention,
                )
            },
        ))
        .child(crate::utilities::bar(ui))
        .child(when(
            move || ui.operation_error.get().is_empty() && !ui.toast.get().is_empty(),
            move || {
                view()
                    .absolute()
                    .bottom(px(44.))
                    .right(px(16.))
                    .max_w(px(420.))
                    .px_3()
                    .py_2()
                    .bg(canban_ui_kit::tokens::SURFACE)
                    .border_1()
                    .border_color(canban_ui_kit::tokens::BORDER)
                    .rounded(px(8.))
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .pointer_events(PointerEvents::None)
                    .child(
                        canban_ui_kit::icon_tinted(
                            canban_ui_kit::Icon::Check,
                            canban_ui_kit::tokens::ACCENT,
                        )
                        .w(px(15.))
                        .h(px(15.)),
                    )
                    .child(
                        canban_ui_kit::label(move || ui.toast.get())
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .min_w(px(0.)),
                    )
            },
        ))
        .child(when(
            move || !ui.operation_error.get().is_empty(),
            move || {
                view()
                    .absolute()
                    .bottom(px(44.))
                    .right(px(16.))
                    .max_w(px(520.))
                    .px_3()
                    .py_2()
                    .bg(canban_ui_kit::tokens::ATTENTION_SURFACE)
                    .border_1()
                    .border_color(canban_ui_kit::tokens::ATTENTION)
                    .rounded(px(8.))
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        canban_ui_kit::label(move || ui.operation_error.get())
                            .text_color(canban_ui_kit::tokens::ATTENTION)
                            .flex_1()
                            .min_w(px(0.)),
                    )
                    .child(canban_ui_kit::chrome_button(
                        canban_ui_kit::Icon::Close,
                        "Dismiss message",
                        move |_| ui.operation_error.set(String::new()),
                    ))
            },
        ))
        .child(quick_open::overlay(ui))
        .child(crate::utilities::overlay(ui))
}
