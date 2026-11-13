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
    let rail = rail.child(icon_action(
        Icon::Board,
        "Board".into(),
        move || {
            matches!(
                ui.page.get().as_str(),
                "board"
                    | "card"
                    | "new-project"
                    | "new-card"
                    | "drafts"
                    | "archive"
                    | "workflows"
                    | "project-location"
            )
        },
        || 0,
        move |_| {
            if ui.page.get_untracked() == "card" {
                ui.close_card();
            } else {
                ui.navigate("board");
            }
        },
    ));
    let rail = rail
        .child(icon_action(
            Icon::Terminal,
            "Terminals".into(),
            move || ui.page.get() == "terminals",
            || 0,
            move |_| ui.navigate("terminals"),
        ))
        .child(icon_action(
            Icon::Compare,
            "Diff & Review".into(),
            move || ui.page.get() == "review",
            move || ui.review_state.pending_comments.with(Vec::len),
            move |_| crate::review::open_compare(ui),
        ));
    rail.child(icon_action(
        Icon::GitHub,
        "GitHub".into(),
        move || ui.page.get().starts_with("github"),
        || 0,
        move |_| ui.navigate("github"),
    ))
    .child(view().flex_1())
    .child(crate::updates::rail(ui))
    .child(icon_action(
        Icon::Settings,
        "Settings".into(),
        move || matches!(ui.page.get().as_str(), "settings" | "machines"),
        || 0,
        move |_| {
            crate::utilities::close(ui);
            ui.navigate("settings");
        },
    ))
}
