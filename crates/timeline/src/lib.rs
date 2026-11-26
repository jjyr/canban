use canban_types::{Event, id, now};

pub fn event(
    project: &str,
    card: &str,
    kind: &str,
    actor: &str,
    message: &str,
    data: serde_json::Value,
) -> Event {
    event_at(project, card, kind, actor, message, data, now())
}

/// Pure clock-injected variant of [`event`].
pub fn event_at(
    project: &str,
    card: &str,
    kind: &str,
    actor: &str,
    message: &str,
    data: serde_json::Value,
    now_ms: u64,
) -> Event {
    Event {
        id: id(),
        project_id: project.into(),
        card_id: card.into(),
        sequence: 0,
        kind: kind.into(),
        actor: actor.into(),
        message: message.into(),
        data,
        created_at: now_ms,
    }
}
