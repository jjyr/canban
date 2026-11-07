use canban_types::{Notice, id, now};
pub fn attention(project: &str, card: &str, message: &str) -> Notice {
    Notice {
        id: id(),
        project_id: project.into(),
        card_id: card.into(),
        message: message.into(),
        read: false,
        created_at: now(),
    }
}

mod overview;
pub use overview::*;
