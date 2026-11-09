//! One reusable Card/profile entry and native terminal screen.
use crate::presenter::*;
use serde_json::json;

/// Explicit Start/Resume intent shared by Card components. A late operation
/// receipt updates saved state without redirecting a user who has navigated away.
pub fn open(ui: Ui, card: String, profile: String) {
    let origin = ui.location();
    let conversation = canban_types::Conversation::key(&card, &profile);
    dispatch_then(
        ui,
        "agent.open",
        json!({"card_id":card,"profile_id":profile}),
        move |v| {
            if v["request_id"].is_string() && ui.location() == origin {
                ui.open_terminal(conversation);
            }
        },
    );
}
