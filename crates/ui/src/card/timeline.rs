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
                        && event.data["path"].as_str().is_some()
                        && other.data["path"] == event.data["path"]
                });
            let narrative = matches!(event.kind.as_str(), "comment.created" | "comment.replied")
                || event.kind == "execution.reported" && !echoed_report;
            (event.clone(), narrative)
        })
        .collect()
}
pub fn render(ui: Ui) -> impl IntoElement {
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
    each(
        move || entries(&events.get()),
        |(e, _)| e.id.clone(),
        move |item, _| {
            let narrative = computed(move || item.get().1);
            dynamic(move || {
                let narrative = narrative.get();
                let event = item.get_untracked().0;
                crate::components::timeline_item::render(ui, event, narrative)
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
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
