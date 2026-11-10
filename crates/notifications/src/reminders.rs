//! One reminder per semantic item; unread and unresolved are independent flags.
use crate::overview;
use canban_types::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reminder {
    pub id: String,
    pub project_id: String,
    pub card_id: String,
    pub message: String,
    pub unread: bool,
    pub pending: bool,
    /// Card-local Timeline position. Zero denotes a state-only recovery item.
    pub sequence: u64,
    pub created_at: u64,
}
pub fn read_sequence(s: &Snapshot, card: &str) -> u64 {
    s.card_reads
        .iter()
        .find(|r| r.card_id == card)
        .map(|r| r.sequence)
        .unwrap_or(0)
}
/// Only explicit Agent replies and final reports are messages, never PTY output
/// or progress. Echoed replies share their originating report's identity.
pub fn message_id(s: &Snapshot, e: &Event) -> Option<String> {
    if !e.actor.starts_with("agent:") {
        return None;
    }
    match e.kind.as_str() {
        "execution.reported"
            if matches!(
                e.data["state"].as_str(),
                Some("completed" | "blocked" | "failed")
            ) =>
        {
            Some(format!(
                "report:{}",
                e.data["report_id"].as_str().unwrap_or(&e.id)
            ))
        }
        "comment.replied" => {
            if let Some(id) = e.data["report_id"].as_str() {
                return Some(format!("report:{id}"));
            }
            // Older reports linked their echoed reply only by resource path.
            if let Some(report) = s.events.iter().find(|r| {
                r.card_id == e.card_id
                    && r.kind == "execution.reported"
                    && r.request_id.is_some()
                    && r.request_id == e.request_id
                    && r.data["path"].as_str().is_some()
                    && r.data["path"] == e.data["path"]
            }) && let Some(id) = message_id(s, report)
            {
                return Some(id);
            }
            Some(format!(
                "reply:{}",
                e.data["question_id"].as_str().unwrap_or(&e.id)
            ))
        }
        _ => None,
    }
}
pub fn reminders(s: &Snapshot) -> Vec<Reminder> {
    let active = |id: &str| s.cards.iter().any(|c| c.id == id && c.active());
    let mut items = BTreeMap::<String, Reminder>::new();
    let mut events = s
        .events
        .iter()
        .filter(|e| active(&e.card_id))
        .collect::<Vec<_>>();
    events.sort_by_key(|e| e.sequence);
    for e in events {
        let Some(id) = message_id(s, e) else {
            continue;
        };
        let unread = e.sequence > read_sequence(s, &e.card_id);
        let item = items.entry(id.clone()).or_insert_with(|| Reminder {
            id,
            project_id: e.project_id.clone(),
            card_id: e.card_id.clone(),
            message: e.message.clone(),
            unread: false,
            pending: false,
            sequence: 0,
            created_at: e.created_at,
        });
        item.sequence = item.sequence.max(e.sequence);
        item.unread |= unread;
        item.message = e.message.clone();
    }
    for row in overview(s).into_iter().filter(|r| r.card.active()) {
        for reason in &row.reasons {
            if reason.source == "question" {
                continue;
            }
            let report = row
                .card
                .human_gate
                .as_ref()
                .and_then(|g| g.request_id.as_ref())
                .and_then(|req| {
                    s.events
                        .iter()
                        .filter(|e| {
                            e.card_id == row.card.id
                                && e.request_id.as_ref() == Some(req)
                                && e.kind == "execution.reported"
                                && message_id(s, e).is_some()
                        })
                        .max_by_key(|e| e.sequence)
                });
            let id = match reason.source.as_str() {
                "task" | "review" => report
                    .and_then(|e| message_id(s, e))
                    .unwrap_or_else(|| format!("gate:{}", row.card.id)),
                "runtime" => row
                    .attempt
                    .as_ref()
                    .and_then(|a| a.lifecycle.waiting.as_ref())
                    .map(|w| format!("wait:{}", w.id))
                    .unwrap_or_else(|| format!("runtime:{}", row.id)),
                _ => format!(
                    "recovery:{}",
                    row.attempt
                        .as_ref()
                        .map(|a| a.id.as_str())
                        .unwrap_or(&row.id)
                ),
            };
            let created_at = row
                .attempt
                .as_ref()
                .and_then(|a| a.lifecycle.waiting.as_ref())
                .map(|w| w.since)
                .or_else(|| report.map(|e| e.created_at))
                .unwrap_or(row.card.created_at);
            items
                .entry(id.clone())
                .and_modify(|i| i.pending = true)
                .or_insert_with(|| Reminder {
                    id,
                    project_id: row.card.project_id.clone(),
                    card_id: row.card.id.clone(),
                    message: reason.message.clone(),
                    unread: false,
                    pending: true,
                    sequence: 0,
                    created_at,
                });
        }
    }
    for q in s
        .questions
        .iter()
        .filter(|q| q.state == "delivery_unknown" && active(&q.card_id))
    {
        let id = format!("delivery:{}", q.id);
        items.insert(
            id.clone(),
            Reminder {
                id,
                project_id: q.project_id.clone(),
                card_id: q.card_id.clone(),
                message: q
                    .error
                    .clone()
                    .unwrap_or_else(|| "Question delivery uncertain".into()),
                unread: false,
                pending: true,
                sequence: 0,
                created_at: q.created_at,
            },
        );
    }
    items
        .into_values()
        .filter(|i| i.pending || i.unread)
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReminderCount {
    pub total: usize,
    pub unread: usize,
    pub pending: usize,
}
pub fn counts(items: &[Reminder]) -> BTreeMap<String, ReminderCount> {
    let mut counts = BTreeMap::<String, ReminderCount>::new();
    for i in items {
        let count = counts.entry(i.card_id.clone()).or_default();
        count.total += 1;
        count.unread += usize::from(i.unread);
        count.pending += usize::from(i.pending);
    }
    counts
}

/// Local delivery bookkeeping, deliberately separate from durable read state.
/// Initial connection and reconnect establish a silent baseline.
#[derive(Default)]
pub struct DeliveryTracker {
    seen: Option<std::collections::BTreeSet<String>>,
}
impl DeliveryTracker {
    pub fn baseline(&mut self, items: &[Reminder]) {
        self.seen
            .get_or_insert_with(Default::default)
            .extend(items.iter().map(|i| i.id.clone()));
    }
    pub fn observe(&mut self, items: &[Reminder]) -> Vec<String> {
        let Some(seen) = &mut self.seen else {
            self.baseline(items);
            return Vec::new();
        };
        items
            .iter()
            .filter(|i| seen.insert(i.id.clone()))
            .map(|i| i.id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests;
