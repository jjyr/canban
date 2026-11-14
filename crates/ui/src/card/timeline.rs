use super::*;
use crate::components::timeline_item::{MilestoneFold, Spine, is_alert_kind, render_connected};
use canban_types::Event;
use std::collections::BTreeSet;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FoldSpec {
    pub group_id: u64,
    pub hidden_count: usize,
    pub expanded: bool,
    pub hidden_alert: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProjectedRow {
    pub event: Event,
    pub narrative: bool,
    pub spine: Spine,
    pub fold: Option<FoldSpec>,
}

/// A durable event is either a Markdown message or a compact milestone row.
/// The sequence is authoritative even if a snapshot arrives out of order.
pub fn entries(events: &[Event]) -> Vec<(Event, bool)> {
    let mut ordered = events.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|e| e.sequence);
    ordered
        .into_iter()
        .map(|event| {
            let echoed_report = event.kind == "execution.reported"
                && (event.data["comment_replied"] == true
                    || events.iter().any(|other| {
                        other.kind == "comment.replied"
                            && other.data["terminal_id"] == event.data["terminal_id"]
                            && event.data["terminal_id"].as_str().is_some()
                            && ((event.data["report_id"].as_str().is_some()
                                && other.data["report_id"] == event.data["report_id"])
                                || (event.data["path"].as_str().is_some()
                                    && other.data["path"] == event.data["path"]))
                    }));
            let narrative = matches!(
                event.kind.as_str(),
                "comment.created" | "comment.replied" | "github.comment"
            ) || event.kind == "execution.reported" && !echoed_report;
            (event.clone(), narrative)
        })
        .collect()
}

/// Process bookkeeping and low-value card metadata mutations that clutter
/// reading. Durable events stay stored (workflow triggers depend on them);
/// Card Activity only shows conversation, stage transitions, decisions,
/// deliveries and unresolved problems.
fn noise(kind: &str) -> bool {
    matches!(
        kind,
        "card.created"
            | "card.updated"
            | "comment.updated"
            | "comment.deleted"
            | "card.content_changed"
            | "card.order_changed"
            | "card.context_updated"
            | "card.column_entered"
            | "card.column_left"
            | "attachment.added"
            | "attention.cleared"
            | "github.reference"
            | "github.issue.discovered"
            | "workspace.prepared"
            | "workspace.promoted"
            | "workflow.admitted"
            | "workflow.started"
            | "workflow.script_started"
            | "workflow.script_completed"
            | "workflow.notified"
            | "workflow.delivery_requeued"
            | "workflow.completed"
            | "workflow.stopped"
            | "workflow.stale"
            | "review.merge_started"
            | "github.pr_publish_started"
            | "github.pr_merge_started"
            | "execution.queued"
            | "execution.retry_queued"
            | "execution.started"
            | "execution.attached"
            | "execution.continued"
            | "execution.disconnected"
            | "execution.reconnected"
            | "execution.input_resolved"
            | "execution.control_requested"
            | "execution.close_requested"
            | "execution.idle_close_requested"
            | "execution.idle_close_failed"
            | "execution.idle_closed"
            | "execution.interrupted"
            | "execution.ended"
            | "execution.archived"
    )
}

/// Transient runtime input waits (e.g. folder trust or sign-in prompts) are
/// actionable while active, but become historical noise once resolved.
fn resolved_input_wait(event: &Event, events: &[Event]) -> bool {
    let Some(terminal_id) = event.data["terminal_id"].as_str() else {
        return false;
    };
    event.kind == "execution.input_required"
        && events.iter().any(|other| {
            other.sequence > event.sequence
                && other.data["terminal_id"].as_str() == Some(terminal_id)
                && matches!(
                    other.kind.as_str(),
                    "execution.input_resolved"
                        | "execution.reported"
                        | "comment.replied"
                        | "execution.failed"
                        | "execution.ended"
                        | "execution.archived"
                )
        })
}

/// Visible Activity rows before milestone folding. An explicitly targeted event
/// (deep link) is always kept.
fn visible(events: &[Event], target: Option<u64>) -> Vec<(Event, bool)> {
    let raw: Vec<(Event, bool)> = entries(events)
        .into_iter()
        .filter(|(event, narrative)| {
            Some(event.sequence) == target
                || *narrative
                || !(noise(&event.kind)
                    || event.kind == "execution.reported"
                    || resolved_input_wait(event, events))
        })
        .collect();

    // Collapse back-to-back identical milestone messages (such as repeated pause notices)
    // into their latest occurrence unless an earlier one is explicitly deep-linked.
    let mut deduped: Vec<(Event, bool)> = Vec::with_capacity(raw.len());
    for (event, narrative) in raw {
        if !narrative
            && let Some((prev, false)) = deduped.last()
            && Some(prev.sequence) != target
            && prev.kind == event.kind
            && prev.message.trim() == event.message.trim()
        {
            deduped.pop();
        }
        deduped.push((event, narrative));
    }
    deduped
}
/// Groups consecutive milestone updates into collapsible runs and computes
/// continuous vertical spine flags across the projected Timeline.
pub(crate) fn project_rows(
    events: &[Event],
    expanded_groups: &BTreeSet<u64>,
    target: Option<u64>,
) -> Vec<ProjectedRow> {
    let items = visible(events, target);
    let mut rows: Vec<ProjectedRow> = Vec::with_capacity(items.len());
    let mut idx = 0;

    while idx < items.len() {
        if items[idx].1 {
            rows.push(ProjectedRow {
                event: items[idx].0.clone(),
                narrative: true,
                spine: Spine::ISOLATED,
                fold: None,
            });
            idx += 1;
            continue;
        }

        let start = idx;
        while idx < items.len() && !items[idx].1 {
            idx += 1;
        }
        let group = &items[start..idx];
        if group.len() == 1 {
            rows.push(ProjectedRow {
                event: group[0].0.clone(),
                narrative: false,
                spine: Spine::ISOLATED,
                fold: None,
            });
            continue;
        }

        let group_id = group[0].0.sequence;
        let earlier = &group[..group.len() - 1];
        let hidden_count = earlier.len();
        let hidden_alert = earlier.iter().any(|(e, _)| is_alert_kind(&e.kind));
        let force_open = target.is_some_and(|t| earlier.iter().any(|(e, _)| e.sequence == t));
        let is_open = expanded_groups.contains(&group_id) || force_open;

        if is_open {
            for (e, _) in earlier {
                rows.push(ProjectedRow {
                    event: e.clone(),
                    narrative: false,
                    spine: Spine::ISOLATED,
                    fold: None,
                });
            }
        }
        let (anchor, _) = &group[group.len() - 1];
        rows.push(ProjectedRow {
            event: anchor.clone(),
            narrative: false,
            spine: Spine::ISOLATED,
            fold: Some(FoldSpec {
                group_id,
                hidden_count,
                expanded: is_open,
                hidden_alert,
            }),
        });
    }

    let total = rows.len();
    for (i, row) in rows.iter_mut().enumerate() {
        row.spine = Spine {
            first: i == 0,
            last: i + 1 == total,
        };
    }
    rows
}

fn execution(ui: Ui, reading: Reading, terminal_id: String) -> impl IntoElement {
    dynamic(move || {
        let selected = ui.request.get();
        if selected != terminal_id {
            return view().into_element();
        }
        let run = ui.snapshot.with(|s| {
            s.terminals
                .iter()
                .find(|t| {
                    t.id == selected && t.agent.as_ref().is_some_and(|a| a.card_id == ui.card_id())
                })
                .cloned()
        });
        if let Some(run) = run {
            let terminal_id = run.id.clone();
            let target = element_ref();
            let pending = stored(false);
            view()
                .element_ref(&target)
                .on_layout(move |_: &Bounds<Points>| {
                    if reading.revealed_request.get() == terminal_id || pending.replace(true) {
                        return;
                    }
                    let terminal_id = terminal_id.clone();
                    ui.spawn(async move {
                        next_frame().await;
                        next_frame().await;
                        if let Some(bounds) = reading.bounds(target).await
                            && ui.request.get_untracked() == terminal_id
                            && ui.card_id() == ui.card.get_untracked()
                        {
                            reading.revealed_request.replace(terminal_id);
                            reading.follow.set(false);
                            reading.scroll.scroll_to(
                                Point::new(px(0.), px((bounds.origin.y.0 - 12.).max(0.))),
                                false,
                            );
                        }
                    });
                })
                .gap_2()
                .child(compact_action(
                    "Close execution details",
                    Variant::Quiet,
                    move |_| {
                        reading.revealed_request.replace(String::new());
                        ui.request.set(String::new());
                        ui.clear_output();
                    },
                ))
                .child(crate::components::run_panel(ui, run))
                .child(when(
                    move || !ui.output.get().is_empty(),
                    move || crate::components::output_panel(ui),
                ))
                .into_element()
        } else {
            view().into_element()
        }
    })
}

pub(super) fn render(ui: Ui, reading: Reading) -> impl IntoElement {
    let expanded_groups = signal(BTreeSet::<u64>::new());
    let events = computed(move || {
        ui.snapshot.with(|s| {
            s.events
                .iter()
                .filter(|e| e.card_id == ui.card_id())
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let rows =
        computed(move || project_rows(&events.get(), &expanded_groups.get(), ui.card_event.get()));
    view()
        .gap_2p5()
        .child(when(
            move || !rows.with(Vec::is_empty),
            move || muted("Activity"),
        ))
        .child(view().gap_0().child(each(
            move || rows.get(),
            |row| row.event.id.clone(),
            move |item, _| {
                dynamic(move || {
                    let row = item.get();
                    let sequence = row.event.sequence;
                    let target = element_ref();
                    let fold = row.fold.map(|spec| {
                        let group_id = spec.group_id;
                        MilestoneFold {
                            hidden_count: spec.hidden_count,
                            expanded: spec.expanded,
                            hidden_alert: spec.hidden_alert,
                            toggle: Rc::new(move || {
                                expanded_groups.update(|set| {
                                    if !set.remove(&group_id) {
                                        set.insert(group_id);
                                    }
                                });
                            }),
                        }
                    });
                    // One detail owner per Agent terminal, placed inside the right column of its
                    // earliest rendered row so the vertical spine stays unbroken.
                    let trailing_detail = row.event.data["terminal_id"]
                        .as_str()
                        .map(str::to_owned)
                        .and_then(|terminal_id| {
                            let first = rows.with(|all| {
                                all.iter()
                                    .filter(|r| {
                                        r.event.data["terminal_id"].as_str()
                                            == Some(terminal_id.as_str())
                                    })
                                    .map(|r| r.event.sequence)
                                    .min()
                            });
                            (first == Some(sequence))
                                .then(|| execution(ui, reading, terminal_id).into_element())
                        });
                    view()
                        .element_ref(&target)
                        .on_layout(move |_: &Bounds<Points>| {
                            ui.spawn(async move {
                                let Some(bounds) = reading.bounds(target).await else {
                                    return;
                                };
                                if reading.jump_ready.try_get().is_none() {
                                    return;
                                }
                                reading.reader.row(ui, sequence, bounds);
                                if reading.jump_ready.get_untracked()
                                    && ui.card_event.get_untracked() == Some(sequence)
                                    && ui.card_id() == ui.card.get_untracked()
                                {
                                    ui.card_event.set(None);
                                    reading.follow.set(false);
                                    reading.scroll.scroll_to(
                                        Point::new(px(0.), px((bounds.origin.y.0 - 12.).max(0.))),
                                        false,
                                    );
                                }
                            });
                        })
                        .child(render_connected(
                            ui,
                            row.event,
                            row.narrative,
                            row.spine,
                            fold,
                            trailing_detail,
                        ))
                        .into_element()
                })
            },
        )))
        .child(dynamic(move || {
            let terminal_id = ui.request.get();
            if !terminal_id.is_empty()
                && !rows.with(|all| {
                    all.iter()
                        .any(|r| r.event.data["terminal_id"].as_str() == Some(&terminal_id))
                })
            {
                execution(ui, reading, terminal_id).into_element()
            } else {
                view().into_element()
            }
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(seq: u64, kind: &str, terminal: &str, message: &str) -> Event {
        Event {
            id: format!("e{seq}"),
            sequence: seq,
            project_id: "p".into(),
            card_id: "c".into(),
            kind: kind.into(),
            actor: "agent:developer".into(),
            message: message.into(),
            data: if terminal.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::json!({"terminal_id": terminal})
            },
            created_at: 1,
        }
    }

    #[test]
    fn activity_hides_card_creation_updates_and_bookkeeping_while_keeping_milestones() {
        let make = |seq, kind: &str, request: &str| make_event(seq, kind, request, kind);
        let events = [
            make(1, "card.created", ""),
            make(2, "card.updated", ""),
            make(3, "comment.created", ""),
            make(4, "execution.queued", "r"),
            make(5, "execution.started", "r"),
            make(6, "card.stage_changed", ""),
            make(7, "execution.failed", "other"),
            make(8, "workflow.approved", "other"),
            make(9, "execution.reported", "other"),
            make(10, "workflow.started", ""),
            make(11, "execution.input_required", "r"),
            make(12, "execution.disconnected", "r"),
            make(13, "execution.archived", "r"),
        ];
        // Note: seq 11 is followed by seq 13 (`execution.archived`) on request `r`,
        // so test an unresolved wait on a live request `live`.
        let mut events = events.to_vec();
        events[10] = make(11, "execution.input_required", "live");

        let kept = |target| {
            visible(&events, target)
                .iter()
                .map(|(e, _)| e.sequence)
                .collect::<Vec<_>>()
        };
        assert_eq!(kept(None), vec![3, 6, 7, 8, 9, 11]);
        // Deep links to hidden bookkeeping or card.created still resolve on demand.
        assert_eq!(kept(Some(1)), vec![1, 3, 6, 7, 8, 9, 11]);
        assert_eq!(kept(Some(4)), vec![3, 4, 6, 7, 8, 9, 11]);
    }

    #[test]
    fn resolved_input_waits_hide_once_answered_or_reported() {
        let events = [
            make_event(
                1,
                "execution.input_required",
                "r1",
                "Agent input required: Agent needs input — Sign in required",
            ),
            make_event(2, "execution.input_resolved", "r1", "Resolved"),
            make_event(3, "execution.reported", "r1", "Done"),
            make_event(
                4,
                "execution.input_required",
                "r2",
                "Agent input required: Agent needs input — Approve command?",
            ),
        ];
        let kept: Vec<u64> = visible(&events, None)
            .into_iter()
            .map(|(e, _)| e.sequence)
            .collect();
        assert_eq!(kept, vec![3, 4]);
    }

    #[test]
    fn consecutive_milestones_collapse_to_latest_and_expand_with_continuous_spine() {
        let events = [
            make_event(1, "card.created", "", "Card created"),
            make_event(2, "comment.created", "", "Please ship this"),
            make_event(3, "card.stage_changed", "", "backlog → develop"),
            make_event(4, "review.merge_conflict", "r1", "Merge conflict with main"),
            make_event(5, "workflow.approved", "", "Human approved delivery"),
            make_event(6, "review.merged", "", "Auto-merged into main"),
            make_event(7, "comment.replied", "r1", "All set!"),
        ];

        // Collapsed by default: [2 (comment), 6 (merged, +3 earlier, alert=true), 7 (reply)].
        let collapsed = project_rows(&events, &BTreeSet::new(), None);
        assert_eq!(
            collapsed
                .iter()
                .map(|r| r.event.sequence)
                .collect::<Vec<_>>(),
            vec![2, 6, 7]
        );
        assert_eq!(
            collapsed.iter().map(|r| r.spine).collect::<Vec<_>>(),
            vec![
                Spine {
                    first: true,
                    last: false
                },
                Spine {
                    first: false,
                    last: false
                },
                Spine {
                    first: false,
                    last: true
                },
            ]
        );
        assert_eq!(
            collapsed[1].fold,
            Some(FoldSpec {
                group_id: 3,
                hidden_count: 3,
                expanded: false,
                hidden_alert: true,
            })
        );

        // Expanded explicitly by group_id = 3.
        let mut open = BTreeSet::new();
        open.insert(3);
        let expanded = project_rows(&events, &open, None);
        assert_eq!(
            expanded
                .iter()
                .map(|r| r.event.sequence)
                .collect::<Vec<_>>(),
            vec![2, 3, 4, 5, 6, 7]
        );
        assert!(expanded[0].spine.first && !expanded[0].spine.last);
        assert!(
            expanded[1..5]
                .iter()
                .all(|r| !r.spine.first && !r.spine.last)
        );
        assert!(!expanded[5].spine.first && expanded[5].spine.last);
        assert_eq!(
            expanded[4].fold,
            Some(FoldSpec {
                group_id: 3,
                hidden_count: 3,
                expanded: true,
                hidden_alert: true,
            })
        );

        // Deep-linking to a folded milestone (seq 4) auto-expands the group.
        let deep_linked = project_rows(&events, &BTreeSet::new(), Some(4));
        assert_eq!(
            deep_linked
                .iter()
                .map(|r| r.event.sequence)
                .collect::<Vec<_>>(),
            vec![2, 3, 4, 5, 6, 7]
        );
    }

    #[test]
    fn duplicate_adjacent_milestones_deduplicate_before_folding() {
        let events = [
            make_event(
                1,
                "workflow.waiting_for_human",
                "",
                "Auto-merge paused: dirty checkout",
            ),
            make_event(
                2,
                "workflow.waiting_for_human",
                "",
                "Auto-merge paused: dirty checkout",
            ),
        ];
        let rows = project_rows(&events, &BTreeSet::new(), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event.sequence, 2);
        assert_eq!(rows[0].fold, None);
        assert_eq!(rows[0].spine, Spine::ISOLATED);
    }

    #[test]
    fn report_echoed_by_a_reply_is_hidden_from_activity() {
        let report = Event {
            id: "report".into(),
            sequence: 1,
            project_id: "p".into(),
            card_id: "c".into(),
            kind: "execution.reported".into(),
            actor: "agent:developer".into(),
            message: "completed: Answer".into(),
            data: serde_json::json!({"report_id":"result","terminal_id":"r"}),
            created_at: 1,
        };
        let reply = Event {
            id: "reply".into(),
            sequence: 2,
            kind: "comment.replied".into(),
            message: "Answer".into(),
            ..report.clone()
        };
        let rows = visible(&[report, reply], None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.kind, "comment.replied");
    }

    #[test]
    fn explicit_report_identity_hides_the_body_echo_without_a_file() {
        let report = Event {
            id: "report".into(),
            sequence: 1,
            project_id: "p".into(),
            card_id: "c".into(),
            kind: "execution.reported".into(),
            actor: "agent:developer".into(),
            message: "completed: Answer".into(),
            data: serde_json::json!({"report_id":"result","path":null,"terminal_id":"r"}),
            created_at: 1,
        };
        let reply = Event {
            id: "reply".into(),
            sequence: 2,
            kind: "comment.replied".into(),
            message: "Answer".into(),
            ..report.clone()
        };
        let rows = entries(&[report, reply]);
        assert!(!rows[0].1);
        assert!(rows[1].1);
    }

    #[test]
    fn comment_report_after_explicit_reply_is_hidden_from_activity() {
        let reply = Event {
            id: "reply".into(),
            sequence: 1,
            project_id: "p".into(),
            card_id: "c".into(),
            kind: "comment.replied".into(),
            actor: "agent:agy".into(),
            message: "Detailed answer".into(),
            data: serde_json::json!({"question_id":"q:agy","path":"replies/q.md","terminal_id":"r"}),
            created_at: 1,
        };
        let report = Event {
            id: "report".into(),
            sequence: 2,
            kind: "execution.reported".into(),
            message: "completed: Summary".into(),
            data: serde_json::json!({"report_id":"rep1","path":null,"state":"completed","comment_replied":true,"terminal_id":"r"}),
            ..reply.clone()
        };
        let rows = visible(&[reply, report], None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.kind, "comment.replied");
    }

    #[test]
    fn reports_without_a_resource_path_are_never_hidden_as_reply_echoes() {
        let event = Event {
            id: "report".into(),
            sequence: 1,
            project_id: "p".into(),
            card_id: "c".into(),
            kind: "execution.reported".into(),
            actor: "agent:developer".into(),
            message: "Blocked".into(),
            created_at: 1,
            data: serde_json::json!({"terminal_id":"r"}),
        };
        let reply = Event {
            id: "reply".into(),
            sequence: 2,
            kind: "comment.replied".into(),
            ..event.clone()
        };
        assert!(
            entries(&[event, reply])
                .iter()
                .all(|(_, narrative)| *narrative)
        );
    }

    #[test]
    fn conversation_is_primary_and_report_echo_keeps_its_history_without_duplicate_body() {
        let make = |kind: &str, seq| canban_types::Event {
            id: format!("e{seq}"),
            sequence: seq,
            project_id: "p".into(),
            card_id: "c".into(),
            kind: kind.into(),
            actor: "user".into(),
            message: kind.into(),
            created_at: 1,
            data: serde_json::json!({"path":"report.md","terminal_id":"r"}),
        };
        let mut events = vec![
            make("comment.created", 1),
            make("execution.queued", 2),
            make("execution.started", 3),
            make("execution.reported", 4),
            make("comment.replied", 5),
        ];
        events.reverse();
        let projected = entries(&events);
        assert_eq!(projected.len(), events.len());
        assert_eq!(
            projected
                .iter()
                .map(|(e, _)| e.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        assert_eq!(
            projected
                .iter()
                .map(|(_, narrative)| *narrative)
                .collect::<Vec<_>>(),
            vec![true, false, false, false, true]
        );
    }

    #[test]
    fn comment_updated_audit_event_is_filtered_while_edited_comment_keeps_original_sequence() {
        let comment = Event {
            id: "c1".into(),
            sequence: 1,
            project_id: "p".into(),
            card_id: "c".into(),
            kind: "comment.created".into(),
            actor: "user".into(),
            message: "Edited text".into(),
            created_at: 100,
            data: serde_json::json!({"body": "Edited text", "revision": 2, "edited_at": 200}),
        };
        let stage = Event {
            id: "s2".into(),
            sequence: 2,
            kind: "card.stage_changed".into(),
            message: "backlog → develop".into(),
            data: serde_json::json!({}),
            ..comment.clone()
        };
        let audit = Event {
            id: "u3".into(),
            sequence: 3,
            kind: "comment.updated".into(),
            message: "Edited text".into(),
            data: serde_json::json!({"comment_id": "c1", "revision": 2}),
            ..comment.clone()
        };
        let rows = visible(&[comment, stage, audit], None);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0.id, "c1");
        assert_eq!(rows[0].0.sequence, 1);
        assert!(rows[0].1);
        assert_eq!(rows[1].0.kind, "card.stage_changed");
    }

    #[test]
    fn comment_deleted_tombstone_and_audit_events_are_hidden_from_card_activity() {
        let deleted = Event {
            id: "c1".into(),
            sequence: 1,
            project_id: "p".into(),
            card_id: "c".into(),
            kind: "comment.deleted".into(),
            actor: "user".into(),
            message: "Comment deleted".into(),
            created_at: 100,
            data: serde_json::json!({"comment_id": "c1", "deleted_at": 200}),
        };
        let stage = Event {
            id: "s2".into(),
            sequence: 2,
            kind: "card.stage_changed".into(),
            message: "backlog → develop".into(),
            data: serde_json::json!({}),
            ..deleted.clone()
        };
        let audit = Event {
            id: "d3".into(),
            sequence: 3,
            kind: "comment.deleted".into(),
            message: "Comment deleted".into(),
            data: serde_json::json!({"comment_id": "c1", "deleted_at": 200}),
            ..deleted.clone()
        };
        let rows = visible(&[deleted, stage, audit], None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.kind, "card.stage_changed");
    }
}
