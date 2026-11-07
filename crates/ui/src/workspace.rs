//! Board -> selected Card -> focused work; viewer close never controls the PTY.
use crate::{board, card, presenter::*, review, sessions};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
pub fn screen(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let page = ui.page.get();
        let narrow = ui.narrow.get();
        let expanded = ui.expanded.get();
        let show_card = ui.focused_card.get();
        let _ = ui.card.get();
        untracked(|| {
            let focus = page == "terminal" || page == "review";
            if focus {
                let tool = if page == "terminal" {
                    sessions::screen(ui).into_element()
                } else {
                    review::screen(ui).into_element()
                };
                let body = if expanded || narrow {
                    if show_card {
                        card::screen(ui).into_element()
                    } else {
                        tool
                    }
                } else {
                    split(
                        card::screen(ui).into_element(),
                        tool,
                        ui.focus_width,
                        320.0,
                        400.0,
                    )
                    .into_element()
                };
                return view()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap_0()
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .justify_between()
                            .items_center()
                            .p_3()
                            .bg(SURFACE)
                            .border_b_1()
                            .border_color(BORDER)
                            .child(action("← Board", Variant::Quiet, move |_| {
                                ui.back_board()
                            }))
                            .child(
                                view()
                                    .flex_row()
                                    .gap_2()
                                    .child(action(
                                        if narrow || expanded {
                                            if show_card { "Show work" } else { "Show Card" }
                                        } else {
                                            "Expand work"
                                        },
                                        Variant::Quiet,
                                        move |_| {
                                            if ui.narrow.get_untracked()
                                                || ui.expanded.get_untracked()
                                            {
                                                ui.focused_card
                                                    .set(!ui.focused_card.get_untracked());
                                            } else {
                                                ui.expanded.set(true);
                                            }
                                        },
                                    ))
                                    .child(
                                        action("Restore split", Variant::Quiet, move |_| {
                                            ui.expanded.set(false);
                                            ui.focused_card.set(false);
                                        })
                                        .disabled(!expanded),
                                    )
                                    .child(action("Close panel", Variant::Quiet, move |_| {
                                        ui.back_board()
                                    })),
                            ),
                    )
                    .child(body)
                    .into_element();
            }
            if page == "card" && !ui.card.get().is_empty() {
                if narrow {
                    return card::screen(ui).into_element();
                }
                return split(
                    board::screen(ui).into_element(),
                    card::screen(ui).into_element(),
                    ui.board_width,
                    540.0,
                    340.0,
                )
                .into_element();
            }
            board::screen(ui).into_element()
        })
    })
}
