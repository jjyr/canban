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

/// Whether a Card is actively merging or queued in a workflow delivery step.
pub fn is_merging(snapshot: &canban_types::Snapshot, card: &canban_types::Card) -> bool {
    if !card.active() || card.merged() {
        return false;
    }
    if snapshot.merging_cards.iter().any(|id| id == &card.id) {
        return true;
    }
    !card.hold_auto_merge
        && snapshot
            .workflow_runs
            .iter()
            .any(|r| r.card_id == card.id && r.state == "waiting_delivery")
}

/// Semantic status descriptors `(label, tone, live)` for a Card on the Board.
pub fn card_status_specs(
    snapshot: &canban_types::Snapshot,
    card: &canban_types::Card,
    ui_merging: bool,
) -> Vec<(String, Tone, bool)> {
    let mut specs = Vec::new();
    let merging = is_merging(snapshot, card) || card.active() && !card.merged() && ui_merging;
    if card.merged() {
        specs.push(("Merged".into(), Tone::Success, false));
    } else if merging {
        specs.push(("Merging".into(), Tone::Info, true));
    }
    if card.human_gate.is_some() || card.attention.is_some() {
        specs.push((
            if card.hold_auto_merge && !card.merged() {
                "Merge held".into()
            } else if card.human_gate.is_some() && card.stage == "review" {
                "Needs review".into()
            } else {
                "Needs attention".into()
            },
            Tone::Attention,
            false,
        ));
    } else if card.hold_auto_merge && !card.merged() {
        specs.push(("Merge held".into(), Tone::Neutral, false));
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
        specs.push((
            format!("@{} failed", request.profile.id),
            Tone::Danger,
            false,
        ));
    }
    specs
}

/// Quiet Cards have no placeholder status; delivery, human gates and failed
/// Agents remain visible as compact chips.
pub fn card_status(
    snapshot: &canban_types::Snapshot,
    card: &canban_types::Card,
) -> Vec<AnyElement> {
    card_status_with_merging(snapshot, card, false)
}

pub fn card_status_with_merging(
    snapshot: &canban_types::Snapshot,
    card: &canban_types::Card,
    ui_merging: bool,
) -> Vec<AnyElement> {
    card_status_specs(snapshot, card, ui_merging)
        .into_iter()
        .map(|(text, tone, live)| {
            if live {
                live_status(text, tone).into_element()
            } else {
                status(text, tone).into_element()
            }
        })
        .collect()
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

    #[test]
    fn card_status_projects_merging_indicator_for_queue_and_active_merges() {
        let mut snapshot = canban_types::Snapshot::default();
        let mut card: canban_types::Card = serde_json::from_value(serde_json::json!({
            "id": "c",
            "project_id": "p",
            "number": 26,
            "title": "Show merging indicator on Board",
            "body": "",
            "stage": "review",
            "revision": 1,
            "created_at": 1
        }))
        .unwrap();

        // Quiet unmerged Card has no placeholder status.
        assert!(card_status_specs(&snapshot, &card, false).is_empty());

        // UI-initiated merge (e.g. quick merge preview/merge in flight) shows live Merging.
        assert_eq!(
            card_status_specs(&snapshot, &card, true),
            vec![("Merging".into(), Tone::Info, true)]
        );

        // AppServer active merge (`merging_cards`) shows live Merging without a workflow run.
        snapshot.merging_cards = vec!["c".into()];
        assert!(is_merging(&snapshot, &card));
        assert_eq!(
            card_status_specs(&snapshot, &card, false),
            vec![("Merging".into(), Tone::Info, true)]
        );
        snapshot.merging_cards.clear();

        // Workflow Queue Merge (`waiting_delivery`) shows live Merging.
        let run: canban_types::WorkflowRun = serde_json::from_value(serde_json::json!({
            "id": "run-1",
            "project_id": "p",
            "card_id": "c",
            "workflow_id": "delivery",
            "definition_revision": 1,
            "definition": {
                "id": "delivery",
                "name": "Delivery",
                "entries": {"review": "deliver"},
                "steps": {
                    "deliver": {
                        "type": "deliver",
                        "sync_upstream": true,
                        "max_conflict_attempts": 3,
                        "next": "done"
                    },
                    "done": {"type": "complete"}
                }
            },
            "state": "waiting_delivery",
            "step": "deliver",
            "visit": 1,
            "request_id": null,
            "trigger_event_id": null,
            "input_digest": "d",
            "inputs": {},
            "outputs": {},
            "history": [],
            "error": null,
            "created_at": 0,
            "updated_at": 0
        }))
        .unwrap();
        snapshot.workflow_runs = vec![run];
        assert!(is_merging(&snapshot, &card));
        assert_eq!(
            card_status_specs(&snapshot, &card, false),
            vec![("Merging".into(), Tone::Info, true)]
        );

        // Holding auto-merge suppresses `waiting_delivery` Merging in favor of Merge held.
        card.hold_auto_merge = true;
        assert!(!is_merging(&snapshot, &card));
        assert_eq!(
            card_status_specs(&snapshot, &card, false),
            vec![("Merge held".into(), Tone::Neutral, false)]
        );
        card.hold_auto_merge = false;

        // Paused workflow (`waiting_approval`) does not claim Merging.
        snapshot.workflow_runs[0].state = "waiting_approval".into();
        assert!(!is_merging(&snapshot, &card));
        assert!(card_status_specs(&snapshot, &card, false).is_empty());

        // Once merged, Merged takes precedence over any transient merging marker.
        snapshot.merging_cards = vec!["c".into()];
        card.delivery = Some(canban_types::Delivery {
            target_branch: "main".into(),
            commit: "abc1234".into(),
            request_key: "k".into(),
            merged_at: 2,
        });
        assert!(!is_merging(&snapshot, &card));
        assert_eq!(
            card_status_specs(&snapshot, &card, true),
            vec![("Merged".into(), Tone::Success, false)]
        );

        // Archived Cards never report as merging.
        card.delivery = None;
        card.archived_at = Some(3);
        assert!(!is_merging(&snapshot, &card));
        assert!(card_status_specs(&snapshot, &card, true).is_empty());
    }
}
