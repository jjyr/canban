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
    let has_live_terminal = !crate::terminals::card_live_entries_for_ui(ui, &card.id).is_empty();
    let mut insert_at = 0;
    items.insert(
        insert_at,
        MenuItem::new("Open Card Terminal")
            .disabled((inactive && !has_live_terminal) || ui.pending.get_untracked())
            .on_select(move || crate::terminals::open_card_quick(ui, &terminal_card)),
    );
    insert_at += 1;
    if !inactive && !card.resolved() {
        let resolve_card = card.clone();
        items.insert(
            insert_at,
            MenuItem::new("Mark as Resolved")
                .disabled(crate::card::resolve_busy(ui) || ui.pending.get_untracked())
                .on_select(move || crate::card::resolve_start(ui, resolve_card.clone())),
        );
        insert_at += 1;
    } else if !inactive && card.resolved() && !card.merged() {
        let reopen_card = card.clone();
        items.insert(
            insert_at,
            MenuItem::new("Reopen Card")
                .disabled(crate::card::resolve_busy(ui) || ui.pending.get_untracked())
                .on_select(move || crate::card::resolve_reopen(ui, reopen_card.clone())),
        );
        insert_at += 1;
    }
    if !inactive
        && !card.resolved()
        && (crate::review::card_has_pr(&card) || card.worktree.is_some() || card.branch.is_some())
    {
        let default_prof = crate::sessions::default_review_profile(ui);
        let review_cid = card.id.clone();
        items.insert(
            insert_at,
            MenuItem::new(format!("Agent Review PR (@{default_prof})"))
                .disabled(ui.pending.get_untracked())
                .on_select(move || crate::review::start_agent_review(ui, &review_cid, None, None)),
        );
    }
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
