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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_at_uses_injected_timestamp() {
        const EPOCH_2023_MS: u64 = 1_700_000_012_345;
        let ev = event_at(
            "p1",
            "c1",
            "card.created",
            "user",
            "Created",
            serde_json::json!({"k": "v"}),
            EPOCH_2023_MS,
        );
        assert_eq!(ev.created_at, EPOCH_2023_MS);
        assert_eq!(ev.project_id, "p1");
        assert_eq!(ev.card_id, "c1");
    }
}
