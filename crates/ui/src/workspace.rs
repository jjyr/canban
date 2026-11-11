//! Board -> selected Card -> focused work; viewer close never controls the PTY.
use crate::{board, card, presenter::*, review};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
pub fn screen(ui: Ui) -> impl IntoElement {
    if std::env::var_os("CANBAN_MOTION_TRACE").is_some() {
        eprintln!("motion workspace mounted");
    }
    if ui.page.get_untracked() == "review" {
        return review_workspace(ui).into_element();
    }
    let selected = signal((
        ui.page.get_untracked() == "new-card",
        if ui.page.get_untracked() == "new-card" {
            ui.new_draft.get_untracked()
        } else {
            ui.card.get_untracked()
        },
    ));
    let requested = computed(move || {
        ui.page.get() == "new-card" || ui.page.get() == "card" && !ui.card.get().is_empty()
    });
    effect(move || {
        if requested.get() {
            selected.set((
                ui.page.get() == "new-card",
                if ui.page.get() == "new-card" {
                    ui.new_draft.get()
                } else {
                    ui.card.get()
                },
            ));
        }
    });
    let full = computed(move || ui.narrow.get() || ui.card_standalone.get());
    canban_ui_kit::split::reveal_split(
        board::screen(ui).into_element(),
        move || {
            dynamic(move || {
                let (new, id) = selected.get();
                untracked(|| {
                    if new {
                        crate::github::new_card(ui).into_element()
                    } else {
                        card::at(ui, id).into_element()
                    }
                })
            })
            .into_element()
        },
        ui.board_width,
        full,
        requested,
    )
    .into_element()
}
fn review_workspace(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let page = ui.page.get();
        let narrow = ui.narrow.get();
        let expanded = ui.expanded.get();
        let show_card = ui.focused_card.get();
        let _ = ui.card.get();
        untracked(|| {
            if page == "card" && ui.card_standalone.get_untracked() {
                return card::screen(ui).into_element();
            }
            let focus = page == "review";
            if focus {
                let tool = review::screen(ui).into_element();
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
                            .px_3()
                            .py_2()
                            .bg(SURFACE)
                            .border_b_1()
                            .border_color(BORDER)
                            .child(compact_action("← Back", Variant::Quiet, move |_| {
                                ui.back()
                            }))
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .child(compact_action(
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
                                    .child(chrome_button_disabled(
                                        Icon::Collapse,
                                        "Restore split",
                                        move || !expanded,
                                        move |_| {
                                            ui.expanded.set(false);
                                            ui.focused_card.set(false);
                                        },
                                    ))
                                    .child(chrome_button(Icon::Close, "Close panel", move |_| {
                                        ui.back()
                                    })),
                            ),
                    )
                    .child(body)
                    .into_element();
            }
            if page == "card" && !ui.card.get().is_empty() {
                if narrow || ui.card_standalone.get_untracked() {
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
