//! Shared destination resolution. Inspecting history never starts an execution.
use super::*;

pub fn history(ui: Ui, row: &OverviewRow) {
    ui.open_card(row.card.id.clone());
    ui.card_tab.set(String::new());
    if let Some(req) = &row.request {
        ui.request.set(req.id.clone());
    }
}
pub fn open_request(ui: Ui, id: &str) {
    let snapshot = ui.snapshot.get_untracked();
    let Some(req) = snapshot.requests.iter().find(|r| r.id == id) else {
        ui.operation_error
            .set("This execution is no longer available".into());
        return;
    };
    if let Some(terminal) = live_request(&snapshot, id) {
        ui.open_terminal(terminal);
    } else {
        ui.open_card(req.card_id.clone());
        ui.card_tab.set(String::new());
        ui.request.set(req.id.clone());
    }
}
/// A historical request must never attach to its profile's newer invocation.
pub fn live_request(snapshot: &canban_types::Snapshot, id: &str) -> Option<String> {
    model::conversations(snapshot)
        .into_iter()
        .find(|r| r.request.as_ref().is_some_and(|r| r.id == id) && model::binding(r).is_some())
        .map(|r| r.id)
}
pub fn label(row: &OverviewRow) -> &'static str {
    if model::binding(row).is_some() {
        "Terminal"
    } else if model::closed(row) {
        "View history"
    } else {
        "View request"
    }
}
pub fn resume(ui: Ui, id: &str) {
    if ui.pending.get_untracked() {
        return;
    }
    let Some(row) = model::conversations(&ui.snapshot.get_untracked())
        .into_iter()
        .find(|r| r.id == id)
    else {
        return;
    };
    if !model::can_resume(&row, &ui.profiles.get_untracked()) {
        ui.operation_error
            .set("Exact resume is unavailable. Open the Card for recovery.".into());
        return;
    }
    let origin = ui.location();
    let in_card = ui.page.get_untracked() == "card" && ui.card.get_untracked() == row.card.id;
    let card_for_open = row.card.id.clone();
    let req = row.request.as_ref().unwrap();
    dispatch_then(
        ui,
        "agent.open",
        json!({"card_id":row.card.id,"profile_id":req.profile.id,"expected_attempt_id":row.attempt.as_ref().map(|a| &a.id)}),
        move |value| {
            if ui.location() == origin
                && let Some(request) = value["request_id"].as_str()
            {
                if in_card {
                    ui.open_card_with_terminal(card_for_open, Some(row.id.clone()));
                } else {
                    open_after_launch(ui, row.id.clone(), request.into());
                }
            }
        },
    );
}
/// Prefer an explicitly selected live Agent, otherwise only an unambiguous live target.
pub fn direct(rows: &[Entry], recent: Option<&str>) -> Option<String> {
    let live: Vec<_> = rows
        .iter()
        .filter(|e| e.listed() && e.binding().is_some())
        .collect();
    if let Some(entry) = live.iter().find(|e| Some(e.saved.id.as_str()) == recent) {
        return Some(entry.saved.id.clone());
    }
    (live.len() == 1).then(|| live[0].saved.id.clone())
}
pub fn open_card(ui: Ui, card: &str) {
    let snapshot = ui.snapshot.get_untracked();
    let Some(c) = snapshot.cards.iter().find(|c| c.id == card).cloned() else {
        ui.operation_error
            .set("This Card is no longer available".into());
        return;
    };
    let live = super::card_live_entries(&snapshot, card);
    let recent = ui.terminal_recent.with(|m| m.get(card).cloned());
    if let Some(id) = recent
        .filter(|rid| live.iter().any(|e| &e.saved.id == rid))
        .or_else(|| live.last().map(|e| e.saved.id.clone()))
    {
        ui.open_terminal(id);
        return;
    }
    if c.archived_at.is_some() {
        ui.operation_error
            .set("Restore this Card before opening a new terminal.".into());
        return;
    }
    super::create(ui, c.project_id, Some(c.id));
}

/// Only an explicit launch may follow a pending request into its eventual PTY.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingOpen {
    pub return_to: Location,
    pub request: String,
    pub origin: Location,
}
pub fn open_after_launch(ui: Ui, conversation: String, request: String) {
    let return_to = ui.location();
    ui.open_terminal(conversation);
    if ui.page.get_untracked() != "terminals" {
        ui.terminal_pending_open.set(Some(PendingOpen {
            return_to,
            request,
            origin: ui.location(),
        }));
    }
}
pub fn watch(ui: Ui) {
    effect(move || {
        let pending = ui.terminal_pending_open.get();
        let snapshot = ui.snapshot.get();
        let _ = (
            ui.page.get(),
            ui.card.get(),
            ui.request.get(),
            ui.terminal_selection.get(),
        );
        let Some(pending) = pending else {
            return;
        };
        if ui.location() != pending.origin {
            ui.terminal_pending_open.set(None);
            return;
        }
        if let Some(id) = live_request(&snapshot, &pending.request) {
            ui.terminal_pending_open.set(None);
            // The temporary queue context is feedback for this launch, not another
            // user destination. Back returns to the original Overview/Card/Review.
            ui.history
                .update(|history| restore_launch_origin(history, &pending));
            ui.open_terminal(id);
        } else if snapshot
            .requests
            .iter()
            .find(|r| r.id == pending.request)
            .is_none_or(|r| {
                !matches!(
                    r.state.as_str(),
                    "queued" | "starting" | "running" | "unknown"
                )
            })
        {
            ui.terminal_pending_open.set(None);
        }
    });
}

fn restore_launch_origin(history: &mut crate::navigation::History, pending: &PendingOpen) {
    history.visit(pending.origin.clone());
    history.entries[history.index] = pending.return_to.clone();
    if history.index > 0 && history.entries[history.index - 1] == pending.return_to {
        history.entries.remove(history.index);
        history.index -= 1;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn location(page: &str) -> Location {
        Location {
            new_draft: String::new(),
            page: page.into(),
            project: "project".into(),
            card: "card".into(),
            request: String::new(),
            terminal: String::new(),
            standalone: true,
            terminal_card: String::new(),
        }
    }
    #[test]
    fn queued_launch_returns_directly_to_its_origin_without_duplicate_history() {
        let source = location("overview");
        let queue = location("card");
        let mut history = crate::navigation::History::default();
        history.visit(source.clone());
        history.visit(queue.clone());
        let pending = PendingOpen {
            request: "new-request".into(),
            origin: queue,
            return_to: source.clone(),
        };
        restore_launch_origin(&mut history, &pending);
        history.visit(location("terminals"));
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.step(false), Some(source));
    }
}
