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
/// Only known routine facts collapse; questions, failures and decisions stay visible.
fn routine(event: &Event, narrative: bool) -> bool {
    !narrative
        && event.request_id.is_some()
        && matches!(
            event.kind.as_str(),
            "execution.queued" | "execution.started" | "execution.attached" | "execution.reported"
        )
}
fn groups(events: &[Event]) -> Vec<Vec<(Event, bool)>> {
    let mut groups: Vec<Vec<(Event, bool)>> = Vec::new();
    for (event, narrative) in entries(events) {
        if routine(&event, narrative)
            && let Some(last) = groups.last_mut()
            && last
                .iter()
                .all(|(e, n)| routine(e, *n) && e.request_id == event.request_id)
        {
            last.push((event, narrative));
        } else {
            groups.push(vec![(event, narrative)]);
        }
    }
    groups
}
fn execution(ui: Ui, reading: Reading, request_id: String) -> impl IntoElement {
    dynamic(move || {
        let request = ui.request.get();
        if request != request_id {
            return view().into_element();
        }
        let req = ui.snapshot.with(|s| {
            s.requests
                .iter()
                .find(|r| r.id == request && r.card_id == ui.card_id())
                .cloned()
        });
        if let Some(req) = req {
            let request_id = req.id.clone();
            let target = element_ref();
            let pending = stored(false);
            view()
                .element_ref(&target)
                .on_layout(move |_: &Bounds<Points>| {
                    if reading.revealed_request.get() == request_id || pending.replace(true) {
                        return;
                    }
                    let request_id = request_id.clone();
                    ui.spawn(async move {
                        next_frame().await;
                        next_frame().await;
                        if let Some(bounds) = reading.bounds(target).await
                            && ui.request.get_untracked() == request_id
                            && ui.card_id() == ui.card.get_untracked()
                        {
                            reading.revealed_request.replace(request_id);
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
                .child(crate::components::run_panel(ui, req))
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
    view()
        .gap_3()
        .child(muted("Activity"))
        .child(each(
            move || groups(&events.get()),
            |group| group[0].0.id.clone(),
            move |item, _| {
                let expanded = signal(false);
                dynamic(move || {
                    let group = item.get();
                    let targeted = group.iter().any(|(e, _)| {
                        ui.card_event.get() == Some(e.sequence)
                            || e.request_id.as_deref() == Some(ui.request.get().as_str())
                    });
                    let grouped = group.len() > 1;
                    let visible = expanded.get() || targeted || !grouped;
                    let tracked = group.iter().map(|(e, _)| e.sequence).collect::<Vec<_>>();
                    let target = element_ref();
                    let mut area =
                        view()
                            .element_ref(&target)
                            .gap_1()
                            .on_layout(move |_: &Bounds<Points>| {
                                let tracked = tracked.clone();
                                ui.spawn(async move {
                                    let Some(bounds) = reading.bounds(target).await else {
                                        return;
                                    };
                                    if reading.jump_ready.try_get().is_none() {
                                        return;
                                    }
                                    for sequence in &tracked {
                                        reading.reader.row(ui, *sequence, bounds);
                                    }
                                    if reading.jump_ready.get_untracked()
                                        && ui
                                            .card_event
                                            .get_untracked()
                                            .is_some_and(|target| tracked.contains(&target))
                                        && ui.card_id() == ui.card.get_untracked()
                                    {
                                        ui.card_event.set(None);
                                        expanded.set(true);
                                        reading.follow.set(false);
                                        reading.scroll.scroll_to(
                                            Point::new(
                                                px(0.),
                                                px((bounds.origin.y.0 - 12.).max(0.)),
                                            ),
                                            false,
                                        );
                                    }
                                });
                            });
                    if grouped {
                        let actor = group[0].0.actor.trim_start_matches("agent:");
                        area = area.child(
                            compact_action(
                                format!(
                                    "{} {actor} · {} execution events",
                                    if visible { "▾" } else { "▸" },
                                    group.len()
                                ),
                                Variant::Quiet,
                                move |_| expanded.update(|v| *v = !*v),
                            )
                            .self_start(),
                        );
                    }
                    if visible {
                        for (event, narrative) in &group {
                            area = area.child(crate::components::timeline_item::render(
                                ui,
                                event.clone(),
                                *narrative,
                            ));
                        }
                    }
                    // One detail owner per request, placed beside its earliest event.
                    if let Some(request) = group[0].0.request_id.clone() {
                        let first = events.with(|all| {
                            all.iter()
                                .filter(|e| e.request_id.as_ref() == Some(&request))
                                .map(|e| e.sequence)
                                .min()
                        });
                        if first == Some(group[0].0.sequence) {
                            area = area.child(execution(ui, reading, request));
                        }
                    }
                    area.into_element()
                })
            },
        ))
        .child(dynamic(move || {
            let request = ui.request.get();
            if !request.is_empty()
                && !events.with(|all| all.iter().any(|e| e.request_id.as_ref() == Some(&request)))
            {
                execution(ui, reading, request).into_element()
            } else {
                view().into_element()
            }
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grouping_preserves_decisions_comments_failures_and_request_boundaries() {
        let make = |seq, kind: &str, request: &str| Event {
            id: format!("e{seq}"),
            sequence: seq,
            project_id: "p".into(),
            card_id: "c".into(),
            kind: kind.into(),
            actor: "agent:developer".into(),
            message: kind.into(),
            request_id: Some(request.into()),
            data: serde_json::json!({}),
            created_at: 1,
        };
        let rows = groups(&[
            make(1, "execution.queued", "r"),
            make(2, "execution.started", "r"),
            make(3, "comment.created", "r"),
            make(4, "execution.queued", "r"),
            make(5, "execution.started", "other"),
            make(6, "execution.failed", "other"),
            make(7, "workflow.approved", "other"),
            make(8, "execution.reported", "other"),
        ]);
        assert_eq!(
            rows.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![2, 1, 1, 1, 1, 1, 1]
        );
        assert!(rows.last().unwrap()[0].1);
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
