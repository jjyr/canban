use super::*;
use canban_types::Event;
#[derive(Clone, Debug, PartialEq)]
pub enum Group {
    Message(Event),
    Activity(Vec<Event>),
}
pub fn groups(events: &[Event]) -> Vec<Group> {
    let mut groups = Vec::new();
    for event in events {
        let echoed_report = event.kind == "execution.reported"
            && events.iter().any(|other| {
                other.kind == "comment.replied"
                    && other.request_id == event.request_id
                    && event.request_id.is_some()
                    && event.data["path"].as_str().is_some()
                    && other.data["path"] == event.data["path"]
            });
        if matches!(event.kind.as_str(), "comment.created" | "comment.replied")
            || event.kind == "execution.reported" && !echoed_report
        {
            groups.push(Group::Message(event.clone()));
        } else if let Some(Group::Activity(items)) = groups.last_mut() {
            items.push(event.clone());
        } else {
            groups.push(Group::Activity(vec![event.clone()]));
        }
    }
    groups
}
pub fn render(ui: Ui, reading: Reading) -> impl IntoElement {
    let events = computed(move || {
        let card = ui.card.get();
        ui.snapshot.with(|s| {
            s.events
                .iter()
                .filter(|e| e.card_id == card)
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    dynamic(move || {
        let mut content = view().gap_2();
        for group in groups(&events.get()) {
            match group {
                Group::Message(event) => {
                    let key = event.id.clone();
                    content = content.child(
                        view()
                            .key(key)
                            .child(crate::components::timeline_item(ui, event)),
                    );
                }
                Group::Activity(items) => {
                    let open = signal(false);
                    let first = items.first().unwrap();
                    let title = if items.len() == 1 {
                        first.message.clone()
                    } else {
                        format!("Activity · {} events", items.len())
                    };
                    let age = crate::components::activity::age(items.last().unwrap().created_at);
                    content = content.child(
                        view()
                            .key(first.id.clone())
                            .gap_1()
                            .child(action(
                                move || {
                                    format!(
                                        "{} {title} · {age}",
                                        if open.get() { "▾" } else { "▸" }
                                    )
                                },
                                Variant::Quiet,
                                move |_| {
                                    reading.follow.set(false);
                                    open.update(|v| *v = !*v);
                                },
                            ))
                            .child(when(
                                move || open.get(),
                                move || {
                                    view().pl_3().children(
                                        items
                                            .iter()
                                            .cloned()
                                            .map(|e| {
                                                crate::components::timeline_item(ui, e)
                                                    .into_element()
                                            })
                                            .collect::<Vec<_>>(),
                                    )
                                },
                            )),
                    );
                }
            }
        }
        content
    })
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
            groups(&[event, reply])
                .iter()
                .all(|g| matches!(g, Group::Message(_)))
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
        let events = vec![
            make("comment.created", 1),
            make("execution.queued", 2),
            make("execution.started", 3),
            make("execution.reported", 4),
            make("comment.replied", 5),
        ];
        let projected = groups(&events);
        assert_eq!(projected.len(), 3);
        assert!(matches!(&projected[1],Group::Activity(v) if v.len()==3));
        assert_eq!(
            projected
                .iter()
                .map(|g| match g {
                    Group::Message(_) => 1,
                    Group::Activity(v) => v.len(),
                })
                .sum::<usize>(),
            events.len()
        );
    }
}
