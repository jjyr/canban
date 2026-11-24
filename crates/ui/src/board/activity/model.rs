//! Fixed-order Project tab strip model, attention counts, and scroll reveal helpers.
//! Overview owns actionable-state classification; events never rank Projects.
use canban_types::Snapshot;
use std::collections::{BTreeMap, BTreeSet};

pub const TAB_MIN_WIDTH: f32 = 96.;
pub const TAB_MAX_WIDTH: f32 = 220.;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub directory: String,
    pub host: Option<String>,
    pub offline: bool,
    pub created_at: u64,
    /// Unique active Cards with unread updates or unresolved user actions.
    pub attention: usize,
    pub pending: bool,
}

impl Summary {
    pub fn display_name(&self) -> String {
        match &self.host {
            Some(host) => format!("{} · {host}", self.name),
            None => self.name.clone(),
        }
    }

    pub fn tooltip(&self) -> String {
        let mut lines = Vec::new();
        let mut head = self.display_name();
        if self.offline {
            head.push_str(" (offline)");
        }
        lines.push(head);
        if self.attention > 0 {
            lines.push(format!("Cards with updates: {}", self.attention));
        }
        if !self.directory.is_empty() {
            lines.push(self.directory.clone());
        }
        lines.join("\n")
    }
}

pub fn projects(snapshot: &Snapshot) -> Vec<Summary> {
    let reminders = canban_notifications::reminders(snapshot);
    let hosts = snapshot
        .projects
        .iter()
        .map(|p| {
            let hid = snapshot.project_host(&p.id);
            let label = snapshot.project_host_badge(&p.id);
            let offline = snapshot.is_host_offline(hid);
            (p.id.clone(), (label, offline))
        })
        .collect::<BTreeMap<_, _>>();
    projects_from_reminders_with_hosts(&snapshot.projects, &reminders, &hosts)
}

pub fn projects_from_reminders_with_hosts(
    project_list: &[canban_types::Project],
    reminders: &[canban_notifications::Reminder],
    hosts: &BTreeMap<String, (Option<String>, bool)>,
) -> Vec<Summary> {
    let mut cards = BTreeMap::<String, BTreeSet<String>>::new();
    for item in reminders {
        cards
            .entry(item.project_id.clone())
            .or_default()
            .insert(item.card_id.clone());
    }
    let mut result = project_list
        .iter()
        .map(|p| {
            let (host, offline) = hosts.get(&p.id).cloned().unwrap_or((None, false));
            Summary {
                id: p.id.clone(),
                name: p.name.clone(),
                directory: p.directory.clone(),
                host,
                offline,
                created_at: p.created_at,
                attention: cards.get(&p.id).map(BTreeSet::len).unwrap_or(0),
                pending: reminders.iter().any(|i| i.project_id == p.id && i.pending),
            }
        })
        .collect::<Vec<_>>();
    result.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    result
}

pub fn tab_width(row: &Summary) -> f32 {
    // Width depends only on display_name (never on attention), reserving room for the
    // badge so reminder count updates never shift neighboring project tabs.
    let name_w: f32 = row
        .display_name()
        .chars()
        .map(|c| if c.is_ascii() { 7.5 } else { 13. })
        .sum();
    (name_w + 64.).clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH)
}

pub fn max_tab_width(strip_container_width: f32) -> f32 {
    if strip_container_width <= 0. {
        TAB_MAX_WIDTH
    } else {
        (strip_container_width - 110.).clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH)
    }
}

pub fn reveal_scroll_x(
    rows: &[Summary],
    selected_id: &str,
    viewport_width: f32,
    current_scroll_x: f32,
    max_tab_w: f32,
) -> Option<f32> {
    if viewport_width <= 0. || rows.is_empty() {
        return None;
    }
    let mut total_w = 0_f32;
    let mut selected_span = None;
    for r in rows {
        let w = tab_width(r).min(max_tab_w);
        if r.id == selected_id && selected_span.is_none() {
            selected_span = Some((total_w, w));
        }
        total_w += w;
    }
    let (tab_start, tab_w) = selected_span?;
    let max_scroll = (total_w - viewport_width).max(0.);
    if max_scroll <= 0. {
        return if current_scroll_x > 0.5 {
            Some(0.)
        } else {
            None
        };
    }
    let scroll_x = current_scroll_x.clamp(0., max_scroll);
    let tab_end = tab_start + tab_w;
    if tab_w > viewport_width {
        let target = tab_start.clamp(0., max_scroll);
        return ((target - scroll_x).abs() > 0.5).then_some(target);
    }
    if tab_start >= scroll_x - 0.5 && tab_end <= scroll_x + viewport_width + 0.5 {
        return None;
    }
    if tab_start < scroll_x {
        Some(tab_start.clamp(0., max_scroll))
    } else {
        Some((tab_end - viewport_width).clamp(0., max_scroll))
    }
}

#[cfg(test)]
mod tests;
