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
                && events.iter().any(|other| {
                    other.kind == "comment.replied"
                        && other.request_id == event.request_id
                        && event.request_id.is_some()
                        && ((event.data["report_id"].as_str().is_some()
                            && other.data["report_id"] == event.data["report_id"])
                            || (event.data["path"].as_str().is_some()
                                && other.data["path"] == event.data["path"]))
                });
            let narrative = matches!(event.kind.as_str(), "comment.created" | "comment.replied")
                || event.kind == "execution.reported" && !echoed_report;
            (event.clone(), narrative)
        })
        .collect()
}
pub(super) fn render(ui: Ui, reading: Reading) -> impl IntoElement {
    let events = computed(move || {
        let card = ui.card_id();
        ui.snapshot.with(|s| {
            s.events
                .iter()
                .filter(|e| e.card_id == card)
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let show_activity = signal(true);
    let activity_count = computed(move || {
        entries(&events.get())
            .iter()
            .filter(|(_, narrative)| !*narrative)
            .count()
    });
    view()
        .gap_2()
        .child(when(
            move || activity_count.get() > 3,
            move || {
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(muted(move || {
                        format!("Conversation · {} status events", activity_count.get())
                    }))
                    .child(compact_action(
                        move || {
                            if show_activity.get() {
                                "Hide status events".into()
                            } else {
                                format!("Show status events ({})", activity_count.get())
                            }
                        },
                        Variant::Quiet,
                        move |_| show_activity.update(|v| *v = !*v),
                    ))
            },
        ))
        .child(each(
            move || entries(&events.get()),
            |(e, _)| e.id.clone(),
            move |item, _| {
                let narrative = computed(move || item.get().1);
                dynamic(move || {
                    let narrative = narrative.get();
                    let event = item.get_untracked().0;
                    let sequence = event.sequence;
                    if !narrative && !show_activity.get() {
                        return view().into_element();
                    }
                    view()
                        .on_layout(move |b: &Bounds<Points>| {
                            reading.reader.row(ui, sequence, *b);
                            if reading.jump_ready.get_untracked()
                                && ui.card_event.get_untracked() == Some(sequence)
                                && ui.card_id() == ui.card.get_untracked()
                            {
                                ui.card_event.set(None);
                                reading.follow.set(false);
                                reading.scroll.scroll_to(
                                    Point::new(px(0.), px((b.origin.y.0 - 12.).max(0.))),
                                    false,
                                );
                            }
                        })
                        .child(crate::components::timeline_item::render(
                            ui, event, narrative,
                        ))
                        .into_element()
                })
            },
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
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
            request_id: Some("r".into()),
            data: serde_json::json!({"report_id":"result","path":null}),
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
    fn reports_without_a_resource_path_are_never_hidden_as_reply_echoes() {
        let event = Event {
            id: "report".into(),
            sequence: 1,
            project_id: "p".into(),
            card_id: "c".into(),
            kind: "execution.reported".into(),
            actor: "agent:developer".into(),
            message: "Blocked".into(),
            request_id: Some("r".into()),
            created_at: 1,
            data: serde_json::json!({}),
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
            request_id: Some("r".into()),
            created_at: 1,
            data: serde_json::json!({"path":"report.md"}),
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
