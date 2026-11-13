//! Shared default Agent profile lookup and Card session helpers.
use crate::presenter::*;
use serde_json::json;

/// Explicit Start/Resume intent for a specific Card/profile conversation.
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
            ui.open_card_with_terminal(card, Some(key));
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
            if ui.location() == origin && v["attempt_id"].as_str().is_some() {
                ui.open_card_with_terminal(card_for_open, Some(conversation));
            }
        },
    );
}

/// The configured default Agent, falling back to `developer` and then the first
/// enabled profile. Shared by standalone worktree launches and review submission.
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

/// Open a clean ephemeral interactive Agent CLI on this Card without injecting
/// a workflow prompt or resuming a historical session.
pub fn send_card(ui: Ui, card: &str) {
    let Some(c) = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card).cloned())
    else {
        return;
    };
    if ui.page.get_untracked() != "card" || ui.card.get_untracked() != c.id {
        ui.open_card(c.id.clone());
    }
    crate::terminals::create_in_card(ui, c.project_id, c.id, true);
}
