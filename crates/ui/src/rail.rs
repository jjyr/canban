//! Compact primary navigation, shared unchanged by the desktop and Gallery.
use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
pub fn sidebar(ui: Ui) -> impl IntoElement {
    let rail = view()
        .w(px(56.))
        .flex_shrink_0()
        .py_3()
        .items_center()
        .gap_2()
        .bg(SURFACE)
        .border_r_1()
        .border_color(BORDER)
        .child(icon_action(
            Icon::Search,
            "Quick Open · ⌘O".into(),
            move || ui.quick_open.get() && !ui.project_picker.get(),
            || 0,
            move |_| ui.show_search(false),
        ));
    let rail = rail
        .child(icon_action(
            Icon::Board,
            "Board".into(),
            move || {
                matches!(
                    ui.page.get().as_str(),
                    "board" | "card" | "review" | "new-project" | "new-card"
                )
            },
            || 0,
            move |_| ui.navigate("board"),
        ))
        .child(icon_action(
            Icon::Overview,
            "Overview".into(),
            move || ui.page.get() == "overview",
            move || ui.reminder_counts.get().len(),
            move |_| ui.navigate("overview"),
        ));
    let rail = rail.child(icon_action(
        Icon::Terminal,
        "Terminals".into(),
        move || ui.page.get() == "terminals",
        || 0,
        move |_| ui.navigate("terminals"),
    ));
    rail.child(icon_action(
        Icon::Archive,
        "Archived Cards".into(),
        move || ui.page.get() == "archive",
        || 0,
        move |_| ui.navigate("archive"),
    ))
    .child(view().flex_1())
    .child(crate::updates::rail(ui))
}
