//! Drafts and scroll positions outlive individual route/component owners.
use crate::presenter::*;
use canban_types::Card;
use nsh_core::prelude::*;
use serde_json::{Value, json};
#[derive(Clone, Default, PartialEq)]
pub struct Draft {
    pub card_kind: String,
    pub issue_query: String,
    pub selected_issue: Option<Value>,
    pub instructions: String,
    pub title: String,
    pub body: String,
    pub comment: String,
    pub base_title: String,
    pub base_body: String,
    pub revision: u64,
    pub editing: bool,
    pub details: bool,
    pub task_expanded: bool,
    pub workspace_details: bool,
    pub error: String,
}
impl Draft {
    pub fn card(c: &Card) -> Self {
        Self {
            title: c.title.clone(),
            body: c.body.clone(),
            base_title: c.title.clone(),
            base_body: c.body.clone(),
            revision: c.revision,
            ..Self::default()
        }
    }
    pub fn dirty(&self) -> bool {
        self.title != self.base_title || self.body != self.base_body
    }
    pub fn reconcile(&mut self, saved: Self) {
        if self.base_title == saved.title && self.base_body == saved.body {
            self.revision = saved.revision;
        } else if !self.dirty() {
            self.title = saved.title.clone();
            self.body = saved.body.clone();
            self.base_title = saved.title;
            self.base_body = saved.body;
            self.revision = saved.revision;
        }
    }
}
pub fn document(ui: Ui, key: String, initial: Draft) -> Signal<Draft> {
    let value = ui
        .drafts
        .with(|drafts| drafts.entry(key.clone()).or_insert(initial).clone());
    let draft = signal(value);
    effect(move || {
        let value = draft.get();
        ui.drafts.with(|m| {
            m.insert(key.clone(), value);
        });
    });
    draft
}
pub fn save(ui: Ui, key: String, draft: Signal<Draft>, method: &'static str, params: Value) {
    let submitted = draft.get_untracked();
    let saved_key = key.clone();
    dispatch_complete(ui, method, params, move |result| {
        let mut value = draft.try_get().unwrap_or_else(|| {
            ui.drafts
                .with(|m| m.get(&key).cloned().unwrap_or(submitted.clone()))
        });
        match result {
            Ok(card) => {
                value.base_title = card["title"].as_str().unwrap_or(&submitted.title).into();
                value.base_body = card["body"].as_str().unwrap_or(&submitted.body).into();
                value.revision = card["revision"].as_u64().unwrap_or(value.revision);
                value.error.clear();
                if !value.dirty() && method == "card.update" {
                    value.editing = false;
                }
            }
            Err(error) => value.error = error,
        }
        ui.drafts.with(|m| {
            m.insert(saved_key, value.clone());
        });
        draft.try_set(value);
    });
}
pub fn scroll_position(ui: Ui, key: &str) -> Point<Points> {
    ui.scrolls.with(|m| m.get(key).copied().unwrap_or_default())
}
pub fn capture_scroll(ui: Ui, key: String, e: &ScrollEvent) {
    ui.scrolls.with(|m| {
        m.insert(key, e.offset);
    });
}
pub fn save_card(ui: Ui, id: String, draft: Signal<Draft>) {
    let value = draft.get_untracked();
    save(
        ui,
        format!("card:{id}"),
        draft,
        "card.update",
        json!({"card_id":id,"revision":value.revision,"title":value.title,"body":value.body}),
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_content_conflict_retains_local_draft_but_metadata_updates_advance_revision() {
        let mut d = Draft {
            title: "Task".into(),
            base_title: "Task".into(),
            body: "Local draft".into(),
            base_body: "Original".into(),
            revision: 1,
            ..Default::default()
        };
        d.reconcile(Draft {
            title: "Task".into(),
            body: "Original".into(),
            revision: 2,
            ..Default::default()
        });
        assert_eq!(d.revision, 2);
        d.reconcile(Draft {
            title: "Task".into(),
            body: "Agent edit".into(),
            revision: 3,
            ..Default::default()
        });
        assert_eq!(d.body, "Local draft");
        assert_eq!(d.revision, 2);
    }
}
