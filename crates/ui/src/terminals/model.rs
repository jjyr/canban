//! Pure, stable conversation projection shared by navigation and the tree.
use canban_notifications::OverviewRow;
use canban_types::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectGroup {
    pub id: String,
    pub name: String,
    pub cards: Vec<CardGroup>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CardGroup {
    pub id: String,
    pub title: String,
    pub number: u64,
    pub branch: Option<String>,
    pub rows: Vec<OverviewRow>,
}
pub fn conversations(snapshot: &Snapshot) -> Vec<OverviewRow> {
    canban_notifications::overview(snapshot)
        .into_iter()
        .filter(|r| r.request.is_some())
        .collect()
}
pub fn closed(row: &OverviewRow) -> bool {
    row.attempt
        .as_ref()
        .map(|a| a.finished())
        .unwrap_or_else(|| {
            row.request.as_ref().is_some_and(|r| {
                !matches!(
                    r.state.as_str(),
                    "queued" | "starting" | "running" | "working" | "unknown"
                )
            })
        })
}
pub fn can_resume(row: &OverviewRow, profiles: &[Profile]) -> bool {
    row.card.archived_at.is_none()
        && closed(row)
        && row
            .attempt
            .as_ref()
            .is_some_and(|a| a.provider_session_id.is_some())
        && row.request.as_ref().is_some_and(|r| {
            profiles
                .iter()
                .any(|p| p.id == r.profile.id && !p.resume_command().is_empty())
        })
}
/// Only the selected live attempt can have a viewer. History never attaches.
pub fn binding(row: &OverviewRow) -> Option<Value> {
    let a = row.attempt.as_ref()?;
    if a.finished() || row.card.archived_at.is_some() {
        return None;
    }
    let input_status = if matches!(a.close_state.as_str(), "pending" | "closing" | "closed") {
        "Closing"
    } else if a.connection_state == "connecting" {
        "Connecting"
    } else if a.connection_state != "connected" {
        "Disconnected"
    } else {
        ""
    };
    Some(
        json!({"id":format!("{}:{}",a.core_id.as_deref().unwrap_or_default(),a.terminal.as_deref().unwrap_or_default()),"terminal":a.terminal.as_ref()?,"core_id":a.core_id.as_ref()?,"read_only":!input_status.is_empty(),"input_status":input_status}),
    )
}
pub fn groups(snapshot: &Snapshot, history: bool) -> Vec<ProjectGroup> {
    let mut projects: BTreeMap<String, ProjectGroup> = BTreeMap::new();
    for row in conversations(snapshot)
        .into_iter()
        .filter(|r| closed(r) == history)
    {
        let p = projects
            .entry(row.card.project_id.clone())
            .or_insert_with(|| ProjectGroup {
                id: row.card.project_id.clone(),
                name: row.project_name.clone(),
                cards: Vec::new(),
            });
        if let Some(card) = p.cards.iter_mut().find(|c| c.id == row.card.id) {
            card.rows.push(row);
        } else {
            p.cards.push(CardGroup {
                id: row.card.id.clone(),
                title: row.card.title.clone(),
                number: row.card.number,
                branch: row.card.branch.clone(),
                rows: vec![row],
            });
        }
    }
    let mut groups: Vec<_> = projects.into_values().collect();
    for p in &mut groups {
        p.cards
            .sort_by(|a, b| (a.number, &a.id).cmp(&(b.number, &b.id)));
        for c in &mut p.cards {
            c.rows.sort_by(|a, b| {
                (&a.request.as_ref().unwrap().profile.id, &a.id)
                    .cmp(&(&b.request.as_ref().unwrap().profile.id, &b.id))
            });
        }
    }
    groups.sort_by(|a, b| (a.name.to_lowercase(), &a.id).cmp(&(b.name.to_lowercase(), &b.id)));
    groups
}
#[cfg(test)]
mod tests;
