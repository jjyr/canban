//! Shared Card activity labels, including retained historical Note events.
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
        "card.auto_merge_held" => "Auto-merge held",
        "card.auto_merge_resumed" => "Auto-merge resumed",
        "review.merge_started" | "github.pr_merge_started" => "Merge started",
        "review.merged" | "github.pr_merged" | "github.pr_delivered" => "Merged",
        "review.merge_failed" | "github.pr_merge_attention" => "Merge failed",
        "card.archived" => "Card archived",
        "card.restored" => "Card restored",
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

/// Quiet Cards have no placeholder status; delivery, human gates and failed
/// Agents remain visible as compact chips.
pub fn card_status(
    snapshot: &canban_types::Snapshot,
    card: &canban_types::Card,
) -> Vec<AnyElement> {
    let mut chips = Vec::new();
    if card.delivery.is_some() || card.pull_request.as_ref().is_some_and(|p| p.merged) {
        chips.push(status("Merged", Tone::Success).into_element());
    }
    if card.human_gate.is_some() || card.attention.is_some() {
        chips.push(
            status(
                if card.hold_auto_merge {
                    "Merge held"
                } else if card.human_gate.is_some() && card.stage == "review" {
                    "Needs review"
                } else {
                    "Needs attention"
                },
                Tone::Attention,
            )
            .into_element(),
        );
    } else if card.hold_auto_merge {
        chips.push(status("Merge held", Tone::Neutral).into_element());
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
        chips.push(status(format!("@{} failed", request.profile.id), Tone::Danger).into_element());
    }
    chips
}

/// Agents currently executing for this Card: newest request per profile.
pub fn running_agents(
    snapshot: &canban_types::Snapshot,
    card: &canban_types::Card,
) -> Vec<canban_types::Request> {
    let mut profiles = std::collections::BTreeSet::new();
    let mut running: Vec<_> = snapshot
        .requests
        .iter()
        .filter(|r| {
            r.card_id == card.id && matches!(r.state.as_str(), "starting" | "running" | "working")
        })
        .collect();
    running.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    running
        .into_iter()
        .filter(|r| profiles.insert(r.profile.id.clone()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn running_agents_keep_the_newest_live_request_per_profile() {
        let mut snapshot = canban_types::Snapshot::default();
        let card: canban_types::Card = serde_json::from_value(serde_json::json!({"id":"c","project_id":"p","number":1,"title":"T","body":"","stage":"develop","revision":1,"created_at":1})).unwrap();
        let make = |id: &str, profile: &str, state: &str, at: u64| {
            let mut r = canban_scheduler::request(
                "p",
                "c",
                "develop",
                "interactive",
                canban_types::Config::default().profiles[0].clone(),
                "task".into(),
                None,
            );
            r.id = id.into();
            r.state = state.into();
            r.created_at = at;
            r.profile.id = profile.into();
            r
        };
        snapshot.requests = vec![
            make("old", "developer", "running", 1),
            make("new", "developer", "working", 2),
            make("done", "reviewer", "completed", 3),
            make("rev", "reviewer", "starting", 1),
            make("other", "tester", "running", 5),
        ];
        snapshot.requests[4].card_id = "elsewhere".into();
        let ids: Vec<_> = running_agents(&snapshot, &card)
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec!["new", "rev"]);
    }
}
