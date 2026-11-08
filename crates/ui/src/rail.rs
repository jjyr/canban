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
                !ui.return_overview.get()
                    && matches!(
                        ui.page.get().as_str(),
                        "board" | "card" | "terminal" | "review" | "new-project" | "new-card"
                    )
            },
            || 0,
            move |_| ui.navigate("board"),
        ))
        .child(icon_action(
            Icon::Overview,
            "Overview".into(),
            move || ui.page.get() == "overview" || ui.return_overview.get(),
            move || {
                canban_notifications::overview(&ui.snapshot.get())
                    .iter()
                    .filter(|r| r.actionable())
                    .count()
            },
            move |_| ui.navigate("overview"),
        ));
    let mut rail = rail;
    for (kind, title, page) in [
        (Icon::Note, "Notes", "notes"),
        (Icon::Archive, "Archived Cards", "archive"),
    ] {
        rail = rail.child(icon_action(
            kind,
            title.into(),
            move || ui.page.get() == page,
            || 0,
            move |_| ui.navigate(page),
        ));
    }
    rail.child(view().flex_1()).child(icon_action(
        Icon::Settings,
        "Settings".into(),
        move || ui.page.get() == "settings",
        || 0,
        move |_| ui.navigate("settings"),
    ))
}
