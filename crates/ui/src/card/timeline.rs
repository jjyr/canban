use super::*;
use canban_types::Event;
/// A durable event is either a Markdown message or a compact history row.
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
                            && other.data["attempt_id"] == event.data["attempt_id"]
                            && event.data["attempt_id"].as_str().is_some()
                            && ((event.data["report_id"].as_str().is_some()
                                && other.data["report_id"] == event.data["report_id"])
                                || (event.data["path"].as_str().is_some()
                                    && other.data["path"] == event.data["path"]))
                    }));
            let narrative = matches!(event.kind.as_str(), "comment.created" | "comment.replied")
                || event.kind == "execution.reported" && !echoed_report;
            (event.clone(), narrative)
        })
        .collect()
}
/// Process bookkeeping that the conversation already implies. The durable
/// events remain stored (workflow triggers depend on some of them); the Card
/// Activity only shows conversation, decisions, stage moves and problems.
fn noise(kind: &str) -> bool {
    matches!(
        kind,
        "card.updated"
            | "card.order_changed"
            | "card.context_updated"
            | "workspace.prepared"
            | "workflow.started"
            | "workflow.notified"
            | "workflow.completed"
            | "execution.queued"
            | "execution.retry_queued"
            | "execution.started"
            | "execution.attached"
            | "execution.continued"
            | "execution.reconnected"
            | "execution.input_resolved"
            | "execution.control_requested"
            | "execution.close_requested"
            | "execution.idle_close_requested"
            | "execution.idle_closed"
            | "execution.ended"
    )
}
/// Visible Activity rows. An explicitly targeted event (deep link) is always kept.
fn visible(events: &[Event], target: Option<u64>) -> Vec<(Event, bool)> {
    entries(events)
        .into_iter()
        .filter(|(event, narrative)| {
            Some(event.sequence) == target
                || *narrative
                || !(noise(&event.kind) || event.kind == "execution.reported")
        })
        .collect()
}
fn execution(ui: Ui, reading: Reading, attempt_id: String) -> impl IntoElement {
    dynamic(move || {
        let selected = ui.request.get();
        if selected != attempt_id {
            return view().into_element();
        }
        let run = ui.snapshot.with(|s| {
            s.attempts
                .iter()
                .find(|a| a.id == selected && a.card_id == ui.card_id())
                .cloned()
        });
        if let Some(run) = run {
            let attempt_id = run.id.clone();
            let target = element_ref();
            let pending = stored(false);
            view()
                .element_ref(&target)
                .on_layout(move |_: &Bounds<Points>| {
                    if reading.revealed_request.get() == attempt_id || pending.replace(true) {
                        return;
                    }
                    let attempt_id = attempt_id.clone();
                    ui.spawn(async move {
                        next_frame().await;
                        next_frame().await;
                        if let Some(bounds) = reading.bounds(target).await
                            && ui.request.get_untracked() == attempt_id
                            && ui.card_id() == ui.card.get_untracked()
                        {
                            reading.revealed_request.replace(attempt_id);
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
    let events = computed(move || {
        ui.snapshot.with(|s| {
            s.events
                .iter()
                .filter(|e| e.card_id == ui.card_id())
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let rows = computed(move || visible(&events.get(), ui.card_event.get()));
    view()
        .gap_3()
        .child(muted("Activity"))
        .child(each(
            move || rows.get(),
            |(event, _)| event.id.clone(),
            move |item, _| {
                dynamic(move || {
                    let (event, narrative) = item.get();
                    let sequence = event.sequence;
                    let target = element_ref();
                    let mut area = view()
                        .element_ref(&target)
                        .gap_1()
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
                        .child(crate::components::timeline_item::render(
                            ui,
                            event.clone(),
                            narrative,
                        ));
                    // One detail owner per attempt, placed beside its earliest visible row.
                    if let Some(attempt_id) = event.data["attempt_id"].as_str().map(str::to_owned) {
                        let first = rows.with(|all| {
                            all.iter()
                                .filter(|(e, _)| e.data["attempt_id"].as_str() == Some(&attempt_id))
                                .map(|(e, _)| e.sequence)
                                .min()
                        });
                        if first == Some(sequence) {
                            area = area.child(execution(ui, reading, attempt_id));
                        }
                    }
                    area.into_element()
                })
            },
        ))
        .child(dynamic(move || {
            let attempt_id = ui.request.get();
            if !attempt_id.is_empty()
                && !rows.with(|all| {
                    all.iter()
                        .any(|(e, _)| e.data["attempt_id"].as_str() == Some(&attempt_id))
                })
            {
                execution(ui, reading, attempt_id).into_element()
            } else {
                view().into_element()
            }
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activity_hides_bookkeeping_but_keeps_conversation_decisions_and_failures() {
        let make = |seq, kind: &str, attempt: &str| Event {
            id: format!("e{seq}"),
            sequence: seq,
            project_id: "p".into(),
            card_id: "c".into(),
            kind: kind.into(),
            actor: "agent:developer".into(),
            message: kind.into(),
            data: if attempt.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::json!({"attempt_id": attempt})
            },
            created_at: 1,
        };
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
        ];
        let kept = |target| {
            visible(&events, target)
                .iter()
                .map(|(e, _)| e.sequence)
                .collect::<Vec<_>>()
        };
        assert_eq!(kept(None), vec![1, 3, 6, 7, 8, 9, 11]);
        // A deep link to hidden bookkeeping still resolves.
        assert_eq!(kept(Some(4)), vec![1, 3, 4, 6, 7, 8, 9, 11]);
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
            data: serde_json::json!({"report_id":"result","attempt_id":"r"}),
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
            data: serde_json::json!({"report_id":"result","path":null,"attempt_id":"r"}),
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
            data: serde_json::json!({"question_id":"q:agy","path":"replies/q.md","attempt_id":"r"}),
            created_at: 1,
        };
        let report = Event {
            id: "report".into(),
            sequence: 2,
            kind: "execution.reported".into(),
            message: "completed: Summary".into(),
            data: serde_json::json!({"report_id":"rep1","path":null,"state":"completed","comment_replied":true,"attempt_id":"r"}),
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
            data: serde_json::json!({"attempt_id":"r"}),
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
            data: serde_json::json!({"path":"report.md","attempt_id":"r"}),
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
}
