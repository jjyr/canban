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
        "comment.updated" => "Comment edited",
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

/// Whether a Card is actively merging right now (not merely waiting behind another Card or paused).
pub fn is_merging(snapshot: &canban_types::Snapshot, card: &canban_types::Card) -> bool {
    if !card.active() || card.merged() {
        return false;
    }
    if snapshot.merging_cards.iter().any(|id| id == &card.id) {
        return true;
    }
    if card.hold_auto_merge {
        return false;
    }
    if let Some((pos, _, head)) = snapshot.merge_queue_status(&card.id) {
        return pos == 1 && head.state == "waiting_delivery" && head.queue_pause_reason().is_none();
    }
    snapshot.workflow_runs.iter().any(|r| {
        r.card_id == card.id && r.state == "waiting_delivery" && r.queue_pause_reason().is_none()
    })
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
    } else if card.active()
        && !card.hold_auto_merge
        && let Some((pos, _, head)) = snapshot.merge_queue_status(&card.id)
    {
        if pos == 1 {
            if head.state == "waiting_delivery" && head.queue_pause_reason().is_some() {
                specs.push(("Queue waiting".into(), Tone::Attention, false));
            }
        } else {
            specs.push((format!("Queue #{pos}"), Tone::Info, false));
        }
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
    for attempt in snapshot
        .terminals
        .iter()
        .filter_map(|t| t.agent.as_ref())
        .filter(|a| a.card_id == card.id && a.stage == card.stage)
    {
        let previous: Option<&&canban_types::AgentRun> = latest.get(&attempt.profile.id);
        if previous.is_none_or(|a| a.updated_at <= attempt.updated_at) {
            latest.insert(attempt.profile.id.clone(), attempt);
        }
    }
    for attempt in latest.values().filter(|a| a.state == "failed") {
        specs.push((
            format!("@{} failed", attempt.profile.id),
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

/// Agents currently executing for this Card: newest live terminal per profile.
pub fn running_agents(
    snapshot: &canban_types::Snapshot,
    card: &canban_types::Card,
) -> Vec<canban_types::TerminalEntry> {
    let mut profiles = std::collections::BTreeSet::new();
    let mut running: Vec<_> = snapshot
        .terminals
        .iter()
        .filter(|t| {
            t.agent.as_ref().is_some_and(|a| {
                a.card_id == card.id
                    && matches!(a.state.as_str(), "starting" | "running" | "working")
            })
        })
        .collect();
    running.sort_by_key(|t| std::cmp::Reverse(t.agent.as_ref().map(|a| a.updated_at).unwrap_or(0)));
    running
        .into_iter()
        .filter(|t| {
            t.agent
                .as_ref()
                .is_some_and(|a| profiles.insert(a.profile.id.clone()))
        })
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
        let make = |num: u64, id: &str, card_id: &str, profile: &str, state: &str, at: u64| {
            let mut a = canban_scheduler::agent_run(
                "p",
                card_id,
                1,
                "develop",
                "interactive",
                canban_types::Config::default().profiles[0].clone(),
                "task".into(),
                None,
            );
            a.state = state.into();
            a.updated_at = at;
            a.profile.id = profile.into();
            canban_types::TerminalEntry {
                id: id.into(),
                conversation_id: Some(id.into()),
                number: num,
                name: None,
                manual_number: None,
                origin_card_id: Some(card_id.into()),
                agent_engine: None,
                runtime: None,
                agent: Some(a),
                created_at: at,
            }
        };
        snapshot.terminals = vec![
            make(1, "old", "c", "developer", "running", 1),
            make(2, "new", "c", "developer", "working", 2),
            make(3, "done", "c", "reviewer", "completed", 3),
            make(4, "rev", "c", "reviewer", "starting", 1),
            make(5, "other", "elsewhere", "tester", "running", 5),
        ];
        let ids: Vec<_> = running_agents(&snapshot, &card)
            .into_iter()
            .map(|t| t.id)
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
            "terminal_id": null,
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
        snapshot.cards = vec![card.clone()];
        snapshot.workflow_runs = vec![run.clone()];
        assert!(is_merging(&snapshot, &card));
        assert_eq!(
            card_status_specs(&snapshot, &card, false),
            vec![("Merging".into(), Tone::Info, true)]
        );

        // Transient main-checkout wait at queue head shows Queue waiting rather than live Merging.
        snapshot.workflow_runs[0].inputs = serde_json::json!({"queued_at": 10, "queue_pause_reason": "Main repository checkout (/repo) has uncommitted local changes"});
        assert!(!is_merging(&snapshot, &card));
        assert_eq!(
            card_status_specs(&snapshot, &card, false),
            vec![("Queue waiting".into(), Tone::Attention, false)]
        );
        snapshot.workflow_runs[0].inputs = serde_json::json!({"queued_at": 10});

        // A second Card queued behind the head shows its FIFO queue position (`Queue #2`) without claiming Merging.
        let card2: canban_types::Card = serde_json::from_value(serde_json::json!({
            "id": "c2",
            "project_id": "p",
            "number": 27,
            "title": "Second in queue",
            "body": "",
            "stage": "review",
            "revision": 1,
            "created_at": 2
        }))
        .unwrap();
        let mut run2 = run;
        run2.id = "run-2".into();
        run2.card_id = "c2".into();
        run2.inputs = serde_json::json!({"queued_at": 20});
        snapshot.cards.push(card2.clone());
        snapshot.workflow_runs.push(run2);
        assert!(!is_merging(&snapshot, &card2));
        assert_eq!(
            card_status_specs(&snapshot, &card2, false),
            vec![("Queue #2".into(), Tone::Info, false)]
        );
        snapshot.cards.truncate(1);
        snapshot.workflow_runs.truncate(1);

        // Holding auto-merge suppresses `waiting_delivery` Merging in favor of Merge held.
        card.hold_auto_merge = true;
        snapshot.cards[0] = card.clone();
        assert!(!is_merging(&snapshot, &card));
        assert_eq!(
            card_status_specs(&snapshot, &card, false),
            vec![("Merge held".into(), Tone::Neutral, false)]
        );
        card.hold_auto_merge = false;
        snapshot.cards[0] = card.clone();

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
