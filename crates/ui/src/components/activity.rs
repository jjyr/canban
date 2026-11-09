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
        "workspace.prepared" => "Workspace prepared",
        "workflow.waiting_for_human" => "Human review required",
        "workflow.approved" => "Workflow approved",
        "execution.queued" => "Queued",
        "execution.started" => "Agent started",
        "execution.reported" => "Result submitted",
        "execution.failed" => "Execution failed",
        "execution.missing" => "Terminal record unavailable",
        "execution.input_required" => "Agent input required",
        "execution.input_resolved" => "Agent input resolved",
        "execution.idle_close_requested" => "Closing idle Agent",
        "execution.idle_closed" => "Idle Agent closed",
        "execution.idle_close_failed" => "Idle exit needs inspection",
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

/// Quiet Cards have no placeholder status; current work and human gates remain visible.
pub fn card_activity(snapshot: &canban_types::Snapshot, card: &canban_types::Card) -> Option<View> {
    let mut row = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .min_w(px(0.));
    let mut visible = false;
    if card.human_gate.is_some() || card.attention.is_some() {
        row = row.child(status(
            if card.human_gate.is_some() && card.stage == "review" {
                "Needs review"
            } else {
                "Needs attention"
            },
            Tone::Attention,
        ));
        visible = true;
    }
    let mut latest = std::collections::BTreeMap::new();
    for request in snapshot
        .requests
        .iter()
        .filter(|r| r.card_id == card.id && r.stage == card.stage)
    {
        let previous: Option<&&canban_types::Request> = latest.get(&request.profile.id);
        if previous.is_none_or(|r| r.created_at <= request.created_at) {
            latest.insert(request.profile.id.clone(), request);
        }
    }
    for request in latest.values().filter(|r| r.state == "failed") {
        row = row.child(status(
            format!("@{} failed", request.profile.id),
            Tone::Danger,
        ));
        visible = true;
    }
    let mut profiles = std::collections::BTreeSet::new();
    for request in snapshot.requests.iter().rev().filter(|r| {
        r.card_id == card.id && matches!(r.state.as_str(), "starting" | "running" | "working")
    }) {
        if !profiles.insert(&request.profile.id) {
            continue;
        }
        let (mark, provider) = crate::profiles::provider(&request.profile);
        row = row.child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .min_w(px(0.))
                .tooltip(
                    nsh_base::tooltip()
                        .child(label(format!("{provider} · @{}", request.profile.id))),
                )
                .child(icon(mark).w(px(14.)).h(px(14.)))
                .child(muted(request.profile.id.clone()).truncate().min_w(px(0.))),
        );
        visible = true;
    }
    visible.then_some(row)
}
