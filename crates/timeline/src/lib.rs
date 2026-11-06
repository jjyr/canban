use canban_types::{Event, id, now};
pub fn event(
    project: &str,
    card: &str,
    kind: &str,
    actor: &str,
    message: &str,
    request: Option<String>,
    data: serde_json::Value,
) -> Event {
    Event {
        id: id(),
        project_id: project.into(),
        card_id: card.into(),
        sequence: 0,
        kind: kind.into(),
        actor: actor.into(),
        message: message.into(),
        request_id: request,
        data,
        created_at: now(),
    }
}
