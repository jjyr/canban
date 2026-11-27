//! Compact primary navigation, shared unchanged by the desktop and Gallery.
use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
pub fn sidebar(ui: Ui) -> impl IntoElement {
    let rail = view()
        .w(px(56.))
        .h_full()
        .flex_shrink_0()
        .pt_3()
        .pb_2()
        .items_center()
        .gap_2()
        .bg(SURFACE)
        .border_r_1()
        .border_color(BORDER)
        .child(icon_action(
            Icon::Search,
            format!(
                "Quick Open · {}",
                crate::commands::shortcuts::QUICK_OPEN_DISPLAY
            ),
            move || ui.quick_open.get() && !ui.project_picker.get(),
            || 0,
            move |_| ui.show_search(false),
        ));
    let inbox_count = computed(move || ui.daily_inbox.with(|i| i.remaining_today));
    let rail = rail
        .child(icon_action(
            Icon::Inbox,
            "Inbox · Today".into(),
            move || ui.page.get() == "inbox",
            move || inbox_count.get(),
            move |_| ui.navigate("inbox"),
        ))
        .child(icon_action(
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
            move |_| ui.open_board(),
        ));
    let terminal_attention = computed(move || {
        ui.closing_terminals.with(|closing| {
            ui.terminal_entries.with(|rows| {
                rows.iter()
                    .filter(|e| {
                        e.listed()
                            && !closing.contains(&e.saved.id)
                            && e.herdr_status().is_attention()
                    })
                    .count()
            })
        })
    });
    let rail = rail
        .child(icon_action(
            Icon::Terminal,
            "Terminals".into(),
            move || ui.page.get() == "terminals",
            move || terminal_attention.get(),
            move |_| ui.navigate("terminals"),
        ))
        .child(icon_action(
            Icon::Compare,
            "Git".into(),
            move || ui.page.get() == "review",
            move || crate::review::rail_pending_comment_count(ui),
            move |_| crate::review::open_compare(ui),
        ));
    rail.child(icon_action(
        Icon::Clock,
        "Schedules".into(),
        move || ui.page.get() == "schedules",
        move || crate::schedules::attention_count(ui),
        move |_| crate::schedules::open(ui, None, None),
    ))
    .child(icon_action(
        Icon::Insights,
        "Insights".into(),
        move || ui.page.get() == "insights",
        || 0,
        move |_| ui.navigate("insights"),
    ))
    .child(view().flex_1())
    .child(crate::updates::rail(ui))
    .child(icon_action(
        Icon::Settings,
        "Settings".into(),
        move || ui.page.get() == "settings",
        || 0,
        move |_| {
            crate::utilities::close(ui);
            ui.navigate("settings");
        },
    ))
}
