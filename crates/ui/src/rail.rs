//! Compact primary navigation, shared unchanged by the desktop and Gallery.
use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;
pub fn sidebar(ui: Ui) -> impl IntoElement {
    let mut rail = view()
        .w(px(56.))
        .flex_shrink_0()
        .py_3()
        .items_center()
        .gap_2()
        .bg(SURFACE)
        .border_r_1()
        .border_color(BORDER)
        .child(
            view()
                .h(px(28.))
                .items_center()
                .justify_center()
                .child(label("C").font_weight(FontWeight::BOLD)),
        )
        .child(icon_action(
            Icon::Search,
            "Quick Open · ⌘O".into(),
            move || ui.quick_open.get() && !ui.project_picker.get(),
            || 0,
            move |_| ui.show_search(false),
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
        ))
        .child(view().w(px(28.)).h(px(1.)).my_2().bg(BORDER))
        .child(dynamic(move || {
            let _ = ui.project.get();
            let name = ui
                .current_project()
                .map(|p| p.name)
                .unwrap_or_else(|| "Choose Project".into());
            icon_action(
                Icon::Projects,
                format!("Switch Project · {name}"),
                move || ui.quick_open.get() && ui.project_picker.get(),
                || 0,
                move |_| ui.show_search(true),
            )
        }));
    for (kind, title, page) in [
        (Icon::Board, "Board", "board"),
        (Icon::Note, "Notes", "notes"),
        (Icon::Archive, "Archived Cards", "archive"),
    ] {
        rail = rail.child(icon_action(
            kind,
            title.into(),
            move || {
                if page == "board" {
                    !ui.return_overview.get()
                        && matches!(
                            ui.page.get().as_str(),
                            "board" | "card" | "terminal" | "review"
                        )
                } else {
                    ui.page.get() == page
                }
            },
            || 0,
            move |_| ui.navigate(page),
        ));
    }
    rail.child(icon_action(
        Icon::Plus,
        "New Project".into(),
        || false,
        || 0,
        move |_| ui.navigate("new-project"),
    ))
    .child(view().flex_1())
    .child(icon_action(
        Icon::Connection,
        "Check Herdr connection".into(),
        || false,
        || 0,
        move |_| dispatch(ui, "herdr.status", json!({})),
    ))
    .child(icon_action(
        Icon::Settings,
        "Settings".into(),
        move || ui.page.get() == "settings",
        || 0,
        move |_| ui.navigate("settings"),
    ))
}
