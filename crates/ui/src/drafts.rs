//! Drafts and scroll positions outlive individual route/component owners.
mod list;
pub(crate) mod persistence;
use crate::presenter::*;
use canban_types::Card;
pub use list::screen;
use nsh_core::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Draft {
    pub github_head: String,
    pub github_compose: String,
    pub github_action_key: String,
    pub github_action_payload: String,
    pub github_unknown: bool,
    pub github_comments: Vec<Value>,
    pub github_file: String,
    pub github_line: String,
    pub github_reply: u64,
    pub project_id: String,
    pub updated_at: u64,
    pub card_kind: String,
    pub issue_query: String,
    pub selected_issue: Option<Value>,
    pub instructions: String,
    pub title: String,
    pub body: String,
    pub comment: String,
    #[serde(skip)]
    pub comment_error: String,
    pub recipients: Vec<String>,
    #[serde(default)]
    pub suppressed_recipients: Vec<String>,
    #[serde(skip)]
    pub uploads: Vec<crate::card::attachments::Upload>,
    pub base_title: String,
    pub base_body: String,
    pub revision: u64,
    pub editing: bool,
    pub details: bool,
    pub workspace_details: bool,
    #[serde(skip)]
    pub error: String,
}
impl Draft {
    pub fn card(c: &Card) -> Self {
        Self {
            project_id: c.project_id.clone(),
            title: c.title.clone(),
            body: c.body.clone(),
            base_title: c.title.clone(),
            base_body: c.body.clone(),
            revision: c.revision,
            ..Self::default()
        }
    }
    pub fn has_content(&self, key: &str) -> bool {
        if key.starts_with("new-card:") {
            !self.title.is_empty()
                || !self.body.is_empty()
                || !self.instructions.is_empty()
                || self.selected_issue.is_some()
        } else {
            key.starts_with("card:")
                && (self.dirty() || !self.comment.is_empty() || !self.recipients.is_empty())
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
        put(ui, key.clone(), value);
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
                value.base_title = if ui
                    .current_card()
                    .is_some_and(|c| c.github_issue().is_some())
                {
                    submitted.title.clone()
                } else {
                    card["title"].as_str().unwrap_or(&submitted.title).into()
                };
                value.base_body = card["body"].as_str().unwrap_or(&submitted.body).into();
                value.revision = card["revision"].as_u64().unwrap_or(value.revision);
                value.error.clear();
                if !value.dirty() && method == "card.update" {
                    value.editing = false;
                    value.uploads.retain(|a| a.is_comment());
                }
            }
            Err(error) => value.error = error,
        }
        put(ui, saved_key, value.clone());
        draft.try_set(value);
    });
}
/// Keep every route's in-memory projection and the persistent writer in sync.
pub fn put(ui: Ui, key: String, mut draft: Draft) {
    if key.starts_with("card:") || key.starts_with("new-card:") {
        let changed = ui.drafts.with(|all| all.get(&key) != Some(&draft));
        if changed {
            draft.updated_at = canban_types::now();
        }
        let meaningful = draft.has_content(&key);
        if let Ok(value) = serde_json::to_value(&draft) {
            persistence::queue(&key, if meaningful { value } else { Value::Null });
        }
    }
    ui.drafts.with(|all| {
        all.insert(key, draft);
    });
    ui.draft_epoch.update(|n| *n += 1);
}
pub fn remove(ui: Ui, key: &str) {
    ui.drafts.with(|all| {
        all.remove(key);
    });
    persistence::queue(key, Value::Null);
    ui.draft_epoch.update(|n| *n += 1);
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
    if ui.pending.get_untracked()
        || value.title.trim().is_empty()
        || crate::card::attachments::busy(draft, false)
    {
        return;
    }
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
