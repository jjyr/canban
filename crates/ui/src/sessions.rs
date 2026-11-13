//! One reusable Card/profile entry and native terminal screen.
use crate::presenter::*;
use serde_json::json;

/// Explicit Start/Resume intent shared by Card components. A late operation
/// receipt updates saved state without redirecting a user who has navigated away.
pub fn open(ui: Ui, card: String, profile: String) {
    if ui.pending.get_untracked() {
        return;
    }
    let in_card = ui.page.get_untracked() == "card" && ui.card.get_untracked() == card;
    let key = canban_types::Conversation::key(&card, &profile);
    if let Some(row) = crate::terminals::model::conversations(&ui.snapshot.get_untracked())
        .into_iter()
        .find(|r| r.id == key)
    {
        if crate::terminals::model::closed(&row) {
            crate::terminals::navigation::resume(ui, &key);
        } else if in_card {
            ui.open_card_with_terminal(card, Some(key));
        } else {
            ui.open_terminal(key);
        }
        return;
    }
    let origin = ui.location();
    let conversation = canban_types::Conversation::key(&card, &profile);
    let card_for_open = card.clone();
    dispatch_then(
        ui,
        "agent.open",
        json!({"card_id":card,"profile_id":profile}),
        move |v| {
            if ui.location() == origin
                && let Some(request) = v["request_id"].as_str()
            {
                if in_card {
                    ui.open_card_with_terminal(card_for_open, Some(conversation));
                } else {
                    crate::terminals::navigation::open_after_launch(
                        ui,
                        conversation,
                        request.into(),
                    );
                }
            }
        },
    );
}

/// The configured default Agent, falling back to `developer` and then the first
/// enabled profile. Shared by every "Send to Agent" entry and review submission.
pub fn default_profile(ui: Ui) -> String {
    let def = ui.default_agent.get_untracked();
    ui.profiles
        .with_untracked(|profiles| {
            profiles
                .iter()
                .find(|p| p.id == def)
                .or_else(|| profiles.iter().find(|p| p.id == "developer"))
                .or_else(|| profiles.first())
                .map(|p| p.id.clone())
        })
        .unwrap_or_else(|| {
            if def.is_empty() {
                "developer".into()
            } else {
                def
            }
        })
}

/// "Send to Agent": open (or resume/start) the default Agent's conversation for
/// this Card. A new conversation receives the Card context manifest.
pub fn send_card(ui: Ui, card: &str) {
    open(ui, card.to_owned(), default_profile(ui));
}
