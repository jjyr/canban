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
    match e.kind.as_str() {
        "github.pr.ci_succeeded" => return Some(format!("ci-pass:{}", e.id)),
        "github.pr.ci_failed" => return Some(format!("ci-fail:{}", e.id)),
        "github.pr.updated"
        | "github.pr.commented"
        | "github.pr.reviewed"
        | "github.pr.state_changed"
            if e.data["self_action"] != true =>
        {
            return Some(format!("pr-update:{}", e.id));
        }
        _ => {}
    }
    if !e.actor.starts_with("agent:") {
        return None;
    }
    match e.kind.as_str() {
        "execution.reported"
            if e.data["comment_replied"] != true
                && matches!(
                    e.data["state"].as_str(),
                    Some("completed" | "backlog" | "blocked" | "failed")
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
                    && (r.data["terminal_id"].as_str().is_some()
                        && r.data["terminal_id"] == e.data["terminal_id"]
                        || r.data["path"].as_str().is_some() && r.data["path"] == e.data["path"])
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
                .and_then(|g| g.terminal_id.as_ref())
                .and_then(|tid| {
                    s.events
                        .iter()
                        .filter(|e| {
                            e.card_id == row.card.id
                                && e.data["terminal_id"].as_str() == Some(tid.as_str())
                                && e.kind == "execution.reported"
                                && message_id(s, e).is_some()
                        })
                        .max_by_key(|e| e.sequence)
                })
                .or_else(|| {
                    if row.card.human_gate.is_some() {
                        s.events
                            .iter()
                            .filter(|e| {
                                e.card_id == row.card.id
                                    && e.kind == "execution.reported"
                                    && message_id(s, e).is_some()
                            })
                            .max_by_key(|e| e.sequence)
                    } else {
                        None
                    }
                });
            let agent = row.agent();
            let target = row
                .terminal
                .as_ref()
                .map(|t| t.id.as_str())
                .unwrap_or(&row.id);
            let id = match reason.source.as_str() {
                "task" | "review" => report
                    .and_then(|e| message_id(s, e))
                    .unwrap_or_else(|| format!("gate:{}", row.card.id)),
                "runtime" => agent
                    .and_then(|a| a.lifecycle.waiting.as_ref())
                    .map(|w| format!("wait:{}", w.id))
                    .unwrap_or_else(|| {
                        let runtime = agent.map(|a| a.runtime_status()).unwrap_or("unknown");
                        if matches!(runtime, "idle" | "done") {
                            let turn = agent
                                .and_then(|a| a.observed.as_ref())
                                .and_then(|v| v["turn"].as_u64())
                                .unwrap_or(0);
                            if agent.is_some_and(|a| a.trigger.is_automatic()) {
                                let anchor = agent
                                    .and_then(|a| crate::unreported_idle_anchor(s, target, a))
                                    .unwrap_or(turn);
                                format!("stalled:{target}:{anchor}")
                            } else {
                                format!("idle:{target}:{turn}")
                            }
                        } else {
                            format!("runtime:{target}")
                        }
                    }),
                _ => format!("recovery:{target}"),
            };
            let created_at = agent
                .and_then(|a| a.lifecycle.waiting.as_ref())
                .map(|w| w.since)
                .or_else(|| agent.and_then(|a| a.lifecycle.idle_since))
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

/// Audible notification identities, deliberately narrower than visual `Reminder` badges.
///
/// Sounds chime only for:
/// 1. Human intervention / replies: runtime input/permission waits (`wait:*`),
///    task/review/approval `human_gate` or `attention`, Agent `blocked` or `backlog`
///    reports, and explicit `comment.replied` answers to `@agent` questions.
///    Interactive runtime `idle` / `done` turns (`idle:*`) remain visual indicators only
///    and do not play a sound, while automatic runs transiently pausing in `idle` / `done`
///    stay suppressed until they have remained stalled without reporting for at least
///    `UNREPORTED_IDLE_ALERT_MS` (`stalled:*`, which alerts visually and chimes).
/// 2. Agent failures: `failed` reports, `execution.failed`, `execution.missing`, runtime
///    errors, 1-hour unreported stalls (`stalled:*`), recovery states, and uncertain
///    question deliveries.
/// 3. Merge failures / held deliveries: `review.merge_failed`, `review.upstream_sync_failed`,
///    `github.pr_merge_attention`, or exhausted conflict budgets / held deliveries (`human_gate`).
///    Automatic in-budget `review.merge_conflict` rework stays silent.
/// 4. Completed merges: automatic `review.merged` (non-user actor) and GitHub PR merge
///    reconciliation (`github.pr_delivered` / non-user `github.pr_merged`).
///
/// Ordinary automated `completed` stage reports without a human gate or `@agent` question
/// reply remain unread badges in UI without playing a sound.
pub fn sound_items(s: &Snapshot) -> Vec<String> {
    sound_items_with_reminders(s, &reminders(s))
}

pub fn sound_items_with_reminders(s: &Snapshot, reminder_items: &[Reminder]) -> Vec<String> {
    let active = |id: &str| s.cards.iter().any(|c| c.id == id && c.active());
    let mut ids = std::collections::BTreeSet::<String>::new();

    // Pending items (permission/input wait, human_gate, attention, runtime error,
    // 1-hour stalled automatic run `stalled:*`, recovery, delivery_unknown) require user
    // intervention and are audible, except interactive runtime `idle:*` items which stay
    // visual-only.
    // Unread-only items are audible only when they are explicit `@agent` question replies
    // or `backlog` / `blocked` / `failed` reports — never automated `completed` stage steps.
    for item in reminder_items {
        if (item.pending && !item.id.starts_with("idle:")) || unread_item_is_audible(s, item) {
            ids.insert(item.id.clone());
        }
    }

    for e in &s.events {
        if !active(&e.card_id) {
            continue;
        }
        match e.kind.as_str() {
            "execution.failed" | "execution.missing" => {
                ids.insert(format!("event:{}", e.id));
            }
            "review.merge_failed"
            | "review.upstream_sync_failed"
            | "github.pr_merge_attention"
            | "github.pr.ci_failed" => {
                ids.insert(format!("event:{}", e.id));
            }
            "github.pr.reviewed"
                if e.data["state"] == "CHANGES_REQUESTED" && e.data["self_action"] != true =>
            {
                ids.insert(format!("event:{}", e.id));
            }
            "github.pr.state_changed"
                if e.data["state"] == "closed" && e.data["self_action"] != true =>
            {
                ids.insert(format!("event:{}", e.id));
            }
            "review.merged" | "github.pr_merged" | "github.pr_delivered" if e.actor != "user" => {
                let key = e.data["request_key"]
                    .as_str()
                    .filter(|k| !k.is_empty())
                    .map(|k| format!("merged:{}:{k}", e.card_id))
                    .or_else(|| {
                        e.data["number"]
                            .as_u64()
                            .map(|n| format!("merged:{}:pr:{n}", e.card_id))
                    })
                    .unwrap_or_else(|| format!("merged:{}:{}", e.card_id, e.id));
                ids.insert(key);
            }
            "card.resolved" if e.actor != "user" => {
                ids.insert(format!("resolved:{}:{}", e.card_id, e.id));
            }
            _ => {}
        }
    }

    for t in &s.terminals {
        if t.conversation_id.is_some() {
            continue;
        }
        let is_managed_special = s.configuration_sessions.iter().any(|c| c.id == t.id)
            || s.merge_batches.iter().any(|b| b.id == t.id);
        if !t.has_active_agent() && !is_managed_special {
            continue;
        }
        match t.herdr_status() {
            HerdrStatus::Blocked => {
                if let Some(w) = t.runtime_wait() {
                    ids.insert(format!("wait:{}", w.id));
                } else {
                    ids.insert(format!("terminal-attn:{}:blocked", t.id));
                }
            }
            HerdrStatus::Error => {
                ids.insert(format!("terminal-attn:{}:error", t.id));
            }
            _ => {}
        }
    }

    ids.into_iter().collect()
}

fn unread_item_is_audible(s: &Snapshot, item: &Reminder) -> bool {
    if !item.unread {
        return false;
    }
    if item.id.starts_with("reply:") {
        return true;
    }
    let Some(report_id) = item.id.strip_prefix("report:") else {
        return false;
    };
    let mut is_audible = false;
    for e in &s.events {
        if e.card_id != item.card_id {
            continue;
        }
        if e.kind == "execution.reported" {
            let eid = e.data["report_id"].as_str().unwrap_or(&e.id);
            if eid == report_id
                && matches!(
                    e.data["state"].as_str(),
                    Some("backlog" | "blocked" | "failed")
                )
            {
                is_audible = true;
                break;
            }
        } else if e.kind == "comment.replied" && message_id(s, e).as_deref() == Some(&item.id) {
            is_audible = true;
            break;
        }
    }
    is_audible
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
    pub fn baseline<T: AsRef<str>>(&mut self, items: &[T]) {
        self.seen
            .get_or_insert_with(Default::default)
            .extend(items.iter().map(|i| i.as_ref().to_owned()));
    }
    pub fn observe<T: AsRef<str>>(&mut self, items: &[T]) -> Vec<String> {
        let Some(seen) = &mut self.seen else {
            self.baseline(items);
            return Vec::new();
        };
        items
            .iter()
            .filter(|i| seen.insert(i.as_ref().to_owned()))
            .map(|i| i.as_ref().to_owned())
            .collect()
    }
}

impl AsRef<str> for Reminder {
    fn as_ref(&self) -> &str {
        &self.id
    }
}

#[cfg(test)]
mod tests;
