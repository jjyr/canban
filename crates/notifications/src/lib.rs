use canban_types::{Notice, id, now};

pub fn attention(project: &str, card: &str, message: &str) -> Notice {
    attention_at(project, card, message, now())
}

/// Pure clock-injected variant of [`attention`].
pub fn attention_at(project: &str, card: &str, message: &str, now_ms: u64) -> Notice {
    Notice {
        id: id(),
        project_id: project.into(),
        card_id: card.into(),
        message: message.into(),
        read: false,
        created_at: now_ms,
    }
}

mod overview;
pub use overview::*;
mod reminders;
pub use reminders::*;
