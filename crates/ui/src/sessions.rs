//! One reusable Card/profile entry and native terminal screen.
use crate::presenter::*;
use serde_json::json;

/// Explicit Start/Resume intent shared by Card components. A late operation
/// receipt updates saved state without redirecting a user who has navigated away.
pub fn open(ui: Ui, card: String, profile: String) {
    if ui.pending.get_untracked() {
        return;
    }
    let key = canban_types::Conversation::key(&card, &profile);
    if let Some(row) = crate::terminals::model::conversations(&ui.snapshot.get_untracked())
        .into_iter()
        .find(|r| r.id == key)
    {
        if crate::terminals::model::closed(&row) {
            crate::terminals::navigation::resume(ui, &key);
        } else {
            ui.open_terminal(key);
        }
        return;
    }
    let origin = ui.location();
    let conversation = canban_types::Conversation::key(&card, &profile);
    dispatch_then(
        ui,
        "agent.open",
        json!({"card_id":card,"profile_id":profile}),
        move |v| {
            if ui.location() == origin
                && let Some(request) = v["request_id"].as_str()
            {
                crate::terminals::navigation::open_after_launch(ui, conversation, request.into());
            }
        },
    );
}
