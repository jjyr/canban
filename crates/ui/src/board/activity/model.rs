//! Project switching order and attention counts, projected from local state.
//! Overview owns actionable-state classification; events never rank Projects.
use canban_types::Snapshot;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub id: String,
    pub name: String,
    /// Unique active Cards with unread updates or unresolved user actions.
    pub attention: usize,
    pub pending: bool,
}
pub fn projects(snapshot: &Snapshot, recent: &[String]) -> Vec<Summary> {
    let mut cards = BTreeMap::<String, BTreeSet<String>>::new();
    let reminders = canban_notifications::reminders(snapshot);
    for item in &reminders {
        cards
            .entry(item.project_id.clone())
            .or_default()
            .insert(item.card_id.clone());
    }
    let mut result = snapshot
        .projects
        .iter()
        .map(|p| Summary {
            id: p.id.clone(),
            name: p.name.clone(),
            attention: cards.get(&p.id).map(BTreeSet::len).unwrap_or(0),
            pending: reminders.iter().any(|i| i.project_id == p.id && i.pending),
        })
        .collect::<Vec<_>>();
    let visit = |id: &str| {
        recent
            .iter()
            .position(|r| r == &format!("project:{id}"))
            .unwrap_or(usize::MAX)
    };
    result.sort_by(|a, b| {
        visit(&a.id)
            .cmp(&visit(&b.id))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    result
}
/// Background changes update badges in place; newly discovered Projects append.
pub fn stable_ids(old: &[String], rows: &[Summary]) -> Vec<String> {
    let mut result = old
        .iter()
        .filter(|id| rows.iter().any(|r| &r.id == *id))
        .cloned()
        .collect::<Vec<_>>();
    for row in rows {
        if !result.contains(&row.id) {
            result.push(row.id.clone());
        }
    }
    result
}
pub fn chip_width(row: &Summary) -> f32 {
    // Reserve the badge slot even at zero, so a new question never shifts a target.
    (row.name
        .chars()
        .map(|c| if c.is_ascii() { 7. } else { 13. })
        .sum::<f32>()
        + 50.)
        .clamp(92., 200.)
}
pub const GAP: f32 = 4.;
pub const OVERFLOW: f32 = 64.;
pub const COMPACT_OVERFLOW: f32 = 96.;
/// Fill available space, reserving overflow only when needed and keeping a drag area.
pub fn visible_count(rows: &[Summary], available: f32) -> usize {
    let budget = (available - 12. - 64.).max(0.);
    let total =
        rows.iter().map(chip_width).sum::<f32>() + GAP * rows.len().saturating_sub(1) as f32;
    if total <= budget {
        return rows.len();
    }
    let mut used = OVERFLOW;
    rows.iter()
        .take_while(|r| {
            used += chip_width(r) + GAP;
            used <= budget
        })
        .count()
}
#[cfg(test)]
mod tests;
