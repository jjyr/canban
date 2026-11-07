//! Shared human-readable metadata for Card and Note activity.
use canban_ui_kit::tokens::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;
pub fn age(time: u64) -> String {
    let seconds = canban_types::now().saturating_sub(time) / 1000;
    match seconds {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}
pub fn activity_header(actor: String, kind: &str, time: u64) -> impl IntoElement {
    let title = match kind {
        "comment.created" => "Comment",
        "comment.replied" => "Agent reply",
        "card.created" => "Card created",
        "card.updated" => "Task updated",
        "card.stage_changed" => "Stage changed",
        "execution.queued" => "Queued",
        "execution.started" => "Agent started",
        "execution.reported" => "Result submitted",
        "card.context_updated" => "Context updated",
        "note.created" => "Note created",
        "note.updated" => "Note updated",
        "note.renamed" => "Note renamed",
        "note.referenced" => "New backlink",
        "note.unreferenced" => "Backlink removed",
        "note.card_linked" => "Card linked",
        "note.card_unlinked" => "Card unlinked",
        _ => "Activity",
    };
    view()
        .flex_row()
        .flex_wrap()
        .justify_between()
        .gap_2()
        .child(label(format!("{actor} · {title}")).font_weight(FontWeight::MEDIUM))
        .child(muted(age(time)))
}
pub fn empty(title: &str, description: &str) -> impl IntoElement {
    view()
        .p_4()
        .gap_2()
        .bg(INSET)
        .rounded(px(RADIUS))
        .child(label(title.to_owned()))
        .child(muted(description.to_owned()))
}
