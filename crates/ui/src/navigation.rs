//! Window-local route history. Traversal restores views and never replays operations.
use crate::presenter::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    pub entries: Vec<Location>,
    pub index: usize,
}
impl History {
    pub fn visit(&mut self, route: Location) {
        if self.entries.get(self.index) == Some(&route) {
            return;
        }
        if self.entries.len() == 1
            && self.entries[0].page == "board"
            && self.entries[0].project.is_empty()
            && route.page == "board"
        {
            self.entries[0] = route;
            return;
        }
        self.entries.truncate(self.index + 1);
        self.entries.push(route);
        self.index = self.entries.len() - 1;
    }
    pub fn replace_current(&mut self, route: Location) {
        if let Some(current) = self.entries.get_mut(self.index) {
            *current = route;
        } else {
            self.visit(route);
        }
    }
    pub fn step(&mut self, forward: bool) -> Option<Location> {
        if forward {
            if self.index + 1 >= self.entries.len() {
                return None;
            }
            self.index += 1;
        } else {
            if self.index == 0 {
                return None;
            }
            self.index -= 1;
        }
        self.entries.get(self.index).cloned()
    }
}
pub fn watch(ui: Ui) {
    let generation = signal(0_u64);
    ui.history.update(|h| h.visit(ui.location()));
    effect(move || {
        let _ = (
            ui.page.get(),
            ui.new_draft.get(),
            ui.project.get(),
            ui.card.get(),
            ui.request.get(),
            ui.terminal_selection.get(),
            ui.card_standalone.get(),
            ui.terminal_card_context.get(),
        );
        let ticket = generation.get_untracked() + 1;
        generation.set(ticket);
        set_timeout(std::time::Duration::ZERO, move || {
            if generation.try_get() == Some(ticket) {
                ui.history.update(|h| h.visit(ui.location()));
            }
        });
    });
}
pub fn replace(ui: Ui) {
    let route = ui.location();
    ui.history.update(|h| h.replace_current(route));
}
pub fn go(ui: Ui, forward: bool) {
    let mut history = ui.history.get_untracked();
    // Include a navigation from the same native event before traversing.
    history.visit(ui.location());
    let Some(mut target) = history.step(forward) else {
        return;
    };
    if target.page == "terminals" && !target.terminal.is_empty() {
        let entry = crate::terminals::entries(&ui.snapshot.get_untracked())
            .into_iter()
            .find(|e| e.saved.id == target.terminal);
        if entry.as_ref().is_none_or(|e| !e.listed()) {
            target.terminal.clear();
            if let Some(card) = entry.and_then(|e| e.card) {
                target.page = "card".into();
                target.project = card.project_id;
                target.card = card.id;
                target.standalone = true;
            }
        }
    }
    if target.page == "card"
        && !ui
            .snapshot
            .with_untracked(|s| s.cards.iter().any(|c| c.id == target.card))
    {
        target.page = "board".into();
        target.card.clear();
        target.request.clear();
        target.standalone = false;
    } else if target.page == "review"
        && !target.card.is_empty()
        && !ui
            .snapshot
            .with_untracked(|s| s.cards.iter().any(|c| c.id == target.card))
    {
        target.card.clear();
        target.request.clear();
    }
    if target.page == "new-card" && !ui.drafts.with(|all| all.contains_key(&target.new_draft)) {
        target.page = "board".into();
        target.new_draft.clear();
    }
    history.entries[history.index] = target.clone();
    ui.history.set(history);
    batch(move || {
        ui.quick_open.set(false);
        ui.utility_popup.set(String::new());
        ui.clear_output();
        ui.review_request.update(|n| *n += 1);
        ui.expanded.set(false);
        ui.focused_card.set(false);
        if target.page == "new-card" {
            if let Some(project) = ui
                .drafts
                .with(|all| all.get(&target.new_draft).map(|d| d.project_id.clone()))
            {
                ui.card_target.set(project);
            }
            ui.new_draft.set(target.new_draft);
        }
        ui.project.set(target.project);
        ui.card.set(target.card);
        ui.request.set(target.request);
        ui.card_standalone.set(target.standalone);
        ui.terminal_card_context.set(target.terminal_card);
        if target.page == "terminals" && !target.terminal.is_empty() {
            let id = target.terminal.clone();
            ui.terminal_tabs.update(|tabs| {
                if !tabs.contains(&id) {
                    tabs.push(id.clone());
                }
            });
            ui.terminal_sidebar.set(!ui.narrow.get_untracked());
            crate::terminals::reveal(ui, &id);
        }
        ui.terminal_selection.set(target.terminal);
        ui.page.set(target.page);
        ui.terminal_focus.update(|n| *n += 1);
    });
}
pub fn controls(ui: Ui) -> View {
    view()
        .flex_row()
        .gap_0()
        .child(chrome_button_disabled(
            Icon::Back,
            "Back · ⌘[",
            move || ui.history.with(|h| h.index == 0),
            move |_| go(ui, false),
        ))
        .child(chrome_button_disabled(
            Icon::Forward,
            "Forward · ⌘]",
            move || ui.history.with(|h| h.index + 1 >= h.entries.len()),
            move |_| go(ui, true),
        ))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn route(page: &str) -> Location {
        Location {
            new_draft: String::new(),
            page: page.into(),
            project: String::new(),
            card: String::new(),
            request: String::new(),
            terminal: String::new(),
            standalone: false,
            terminal_card: String::new(),
        }
    }
    #[test]
    fn distinct_new_drafts_have_distinct_history_and_submission_locations() {
        let mut first = route("new-card");
        first.new_draft = "new-card:first".into();
        let mut second = first.clone();
        second.new_draft = "new-card:second".into();
        assert_ne!(first, second);
        let mut history = History::default();
        history.visit(first.clone());
        history.visit(second);
        assert_eq!(history.step(false), Some(first));
    }
    #[test]
    fn traversal_does_not_duplicate_and_new_navigation_discards_forward_only() {
        let mut h = History::default();
        h.visit(route("board"));
        h.visit(route("card"));
        h.visit(route("terminals"));
        assert_eq!(h.step(false).unwrap().page, "card");
        h.visit(route("card"));
        assert_eq!(h.entries.len(), 3);
        assert_eq!(h.step(true).unwrap().page, "terminals");
        h.step(false);
        h.visit(route("archive"));
        assert!(h.step(true).is_none());
        assert_eq!(h.entries.len(), 3);
    }
    #[test]
    fn consecutive_new_card_replacement_keeps_single_back_step_to_origin() {
        let mut h = History::default();
        let board = route("board");
        let mut first = route("new-card");
        first.new_draft = "new-card:first".into();
        let mut second = route("new-card");
        second.new_draft = "new-card:second".into();
        h.visit(board.clone());
        h.visit(first);
        h.replace_current(second.clone());
        // Subsequent async history watcher visit is a no-op on the replaced route.
        h.visit(second);
        assert_eq!(h.entries.len(), 2);
        assert_eq!(h.step(false), Some(board));
    }
}
