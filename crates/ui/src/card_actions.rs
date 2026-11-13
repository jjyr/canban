//! Shared reversible Card lifecycle intents for detail, Board menus and Archive.
use crate::presenter::*;
use canban_types::Card;
use nsh_core::element::desktop::{MenuItem, popup_menu_at};
use nsh_core::prelude::*;
use serde_json::json;

#[derive(Clone, Copy)]
pub enum Action {
    Archive,
    Remove,
    Restore,
}
/// Board lifecycle actions preserve the Project workspace, including its detail pane.
pub(crate) fn after_archive(ui: Ui) {
    let page = ui.page.get_untracked();
    if page == "board" || (page == "card" && !ui.card_standalone.get_untracked()) {
        ui.card.set(String::new());
        ui.page.set("board".into());
    } else {
        ui.page.set("archive".into());
    }
}

pub fn apply(ui: Ui, card: Card, action: Action) {
    let card = ui
        .snapshot
        .with_untracked(|s| s.cards.iter().find(|c| c.id == card.id).cloned())
        .unwrap_or(card);
    let id = card.id.clone();
    if crate::APP.get().is_none() {
        ui.snapshot.update(|s| {
            if let Some(c) = s.cards.iter_mut().find(|c| c.id == id) {
                match action {
                    Action::Archive => c.archived_at = Some(canban_types::now()),
                    Action::Remove => c.removed_at = Some(canban_types::now()),
                    Action::Restore => {
                        c.archived_at = None;
                        c.removed_at = None;
                    }
                }
                c.revision += 1;
            }
        });
        match action {
            Action::Archive => after_archive(ui),
            Action::Remove => {
                ui.card.set(String::new());
                ui.page.set("board".into());
            }
            Action::Restore => {}
        }
        return;
    }
    let method = match action {
        Action::Archive => "card.archive",
        Action::Remove => "card.remove",
        Action::Restore => "card.restore",
    };
    dispatch(ui, method, json!({"card_id":id,"revision":card.revision}));
}
pub fn items(ui: Ui, card: Card) -> Vec<MenuItem> {
    let inactive = card.archived_at.is_some() || card.removed_at.is_some();
    let saved = card.clone();
    let mut items = vec![
        MenuItem::new(if inactive {
            "Restore Card"
        } else {
            "Archive Card"
        })
        .disabled(ui.pending.get_untracked())
        .on_select(move || {
            apply(
                ui,
                saved.clone(),
                if inactive {
                    Action::Restore
                } else {
                    Action::Archive
                },
            )
        }),
    ];
    let terminal_card = card.id.clone();
    let agent_card = card.id.clone();
    let project_id = card.project_id.clone();
    items.insert(
        0,
        MenuItem::new("Open Agent")
            .disabled(inactive || ui.pending.get_untracked())
            .on_select(move || {
                let s = ui.snapshot.get_untracked();
                let live = crate::terminals::card_live_entries(&s, &agent_card);
                let recent = ui.terminal_recent.with(|m| m.get(&agent_card).cloned());
                let existing_agent = recent
                    .as_deref()
                    .and_then(|rid| {
                        live.iter().find(|e| {
                            e.saved.id == rid
                                && (e.agent.is_some() || e.saved.agent_engine.is_some())
                        })
                    })
                    .or_else(|| {
                        live.iter()
                            .find(|e| e.agent.is_some() || e.saved.agent_engine.is_some())
                    })
                    .map(|e| e.saved.id.clone());
                if let Some(id) = existing_agent {
                    ui.open_card_with_terminal(agent_card.clone(), Some(id));
                } else {
                    crate::terminals::create_in_card(
                        ui,
                        project_id.clone(),
                        agent_card.clone(),
                        true,
                    );
                }
            }),
    );
    items.insert(
        0,
        MenuItem::new("Open Card Terminal")
            .on_select(move || crate::terminals::open_card_quick(ui, &terminal_card)),
    );
    // GitHub removal excludes a source from intake; local Cards only archive.
    if card.removed_at.is_none() && card.github_issue().is_some() {
        items.push(
            MenuItem::new("Remove from Board")
                .disabled(ui.pending.get_untracked())
                .on_select(move || apply(ui, card.clone(), Action::Remove)),
        );
    }
    items
}
pub fn menu(ui: Ui, card: Card, position: Point<Points>) {
    ui.spawn(async move {
        popup_menu_at(items(ui, card), position).await;
    });
}
