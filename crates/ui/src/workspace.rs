//! Board -> selected Card -> focused work; viewer close never controls the PTY.
use crate::{board, card, presenter::*, review};
use nsh_core::prelude::*;

pub fn screen(ui: Ui) -> impl IntoElement {
    if std::env::var_os("CANBAN_MOTION_TRACE").is_some() {
        eprintln!("motion workspace mounted");
    }
    if ui.page.get_untracked() == "review" {
        return review::screen(ui).into_element();
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
    let full = computed(move || {
        ui.narrow.get()
            || ui.card_standalone.get()
            || (ui.page.get() == "card" && ui.card_terminal_open.get())
    });
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
