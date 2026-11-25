//! One reminder per semantic item; unread and unresolved are independent flags.
use crate::{overview, overview_at};
use canban_types::*;
pub use canban_types::{Reminder, ReminderEventMeta};
use std::collections::{BTreeMap, BTreeSet};

pub fn read_sequence(s: &Snapshot, card: &str) -> u64 {
    s.card_reads
        .iter()
        .find(|r| r.card_id == card)
        .map(|r| r.sequence)
        .unwrap_or(0)
}
/// Only explicit Agent replies and final reports are messages, never PTY output
/// or progress. Echoed replies share their originating report's identity.
pub fn message_id_in_card(card_events: &[Event], e: &Event) -> Option<String> {
    message_id_in_refs(&card_events.iter().collect::<Vec<_>>(), e)
}

fn message_id_in_refs(card_events: &[&Event], e: &Event) -> Option<String> {
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
            if let Some(report) = card_events.iter().copied().find(|r| {
                r.card_id == e.card_id
                    && r.kind == "execution.reported"
                    && (r.data["terminal_id"].as_str().is_some()
                        && r.data["terminal_id"] == e.data["terminal_id"]
                        || r.data["path"].as_str().is_some() && r.data["path"] == e.data["path"])
                    && r.data["path"].as_str().is_some()
                    && r.data["path"] == e.data["path"]
            }) && let Some(id) = message_id_in_refs(card_events, report)
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

fn build_event_meta(e: &Event) -> ReminderEventMeta {
    let state = match e.kind.as_str() {
        "github.pr.state_changed" => e.data["state"]
            .as_str()
            .or_else(|| e.data["to"].as_str())
            .map(str::to_owned),
        "github.pr.reviewed" | "execution.reported" => e.data["state"].as_str().map(str::to_owned),
        _ => None,
    };
    let pr_number = e.data["number"].as_u64().filter(|&n| n > 0);
    let summary = match e.kind.as_str() {
        "github.pr.ci_succeeded" | "github.pr.ci_failed" => e.data["summary"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        "github.pr.updated" => e.data["commits"]
            .as_array()
            .and_then(|c| c.first())
            .and_then(|c| c.as_str().or_else(|| c["message"].as_str()))
            .map(|m| m.lines().next().unwrap_or("").trim().to_owned())
            .filter(|s| !s.is_empty()),
        "github.pr.commented" | "github.pr.reviewed" => e.data["projected_body"]
            .as_str()
            .and_then(|b| b.lines().find(|l| !l.trim().is_empty()))
            .map(|l| l.trim().to_owned())
            .filter(|s| !s.is_empty()),
        _ => None,
    };
    let action = match e.kind.as_str() {
        "github.pr.ci_succeeded" | "github.pr.ci_failed" => e.data["failed_checks"]
            .as_array()
            .filter(|arr| arr.len() == 1)
            .and_then(|arr| arr[0]["url"].as_str())
            .filter(|u| !u.is_empty())
            .or_else(|| e.data["checks_url"].as_str().filter(|u| !u.is_empty()))
            .or_else(|| e.data["url"].as_str().filter(|u| !u.is_empty()))
            .map(|u| (u.to_owned(), "Checks ↗".to_owned())),
        "github.pr.updated" => e.data["diff_url"]
            .as_str()
            .filter(|u| !u.is_empty())
            .map(|u| (u.to_owned(), "Diff ↗".to_owned()))
            .or_else(|| {
                e.data["url"]
                    .as_str()
                    .filter(|u| !u.is_empty())
                    .map(|u| (u.to_owned(), "PR ↗".to_owned()))
            }),
        "github.pr.commented" => e.data["url"]
            .as_str()
            .filter(|u| !u.is_empty())
            .or_else(|| e.data["pr_url"].as_str().filter(|u| !u.is_empty()))
            .map(|u| (u.to_owned(), "View ↗".to_owned())),
        "github.pr.reviewed" => e.data["url"]
            .as_str()
            .filter(|u| !u.is_empty())
            .or_else(|| e.data["pr_url"].as_str().filter(|u| !u.is_empty()))
            .map(|u| (u.to_owned(), "Review ↗".to_owned())),
        "github.pr.state_changed" => e.data["url"]
            .as_str()
            .filter(|u| !u.is_empty())
            .map(|u| (u.to_owned(), "PR ↗".to_owned())),
        _ => None,
    };
    let (action_url, action_label) = match action {
        Some((url, label)) => (Some(url), Some(label)),
        None => (None, None),
    };
    ReminderEventMeta {
        kind: e.kind.clone(),
        actor: e.actor.clone(),
        state,
        pr_number,
        summary,
        action_url,
        action_label,
    }
}

fn event_is_audible_when_unread(id: &str, e: &Event) -> bool {
    if id.starts_with("reply:") {
        return true;
    }
    if e.kind == "comment.replied" {
        return true;
    }
    e.kind == "execution.reported"
        && matches!(
            e.data["state"].as_str(),
            Some("backlog" | "blocked" | "failed")
        )
}

fn runtime_reminder_for_reason_at(
    s: &Snapshot,
    row: &overview::OverviewRow,
    reason: &overview::OverviewReason,
    at: u64,
) -> Option<Reminder> {
    if !matches!(reason.source.as_str(), "runtime" | "recovery") {
        return None;
    }
    let agent = row.agent();
    let target = row
        .terminal
        .as_ref()
        .map(|t| t.id.as_str())
        .unwrap_or(&row.id);
    let id = match reason.source.as_str() {
        "runtime" => agent
            .and_then(|a| a.lifecycle.waiting.as_ref())
            .map(|w| format!("wait:{}", w.id))
            .unwrap_or_else(|| {
                let runtime = agent.map(|a| a.runtime_status_at(at)).unwrap_or("unknown");
                if matches!(runtime, "idle" | "done") {
                    let turn = agent
                        .and_then(|a| a.observed.as_ref())
                        .and_then(|v| v["turn"].as_u64())
                        .unwrap_or(0);
                    if agent.is_some_and(|a| a.trigger.is_automatic()) {
                        let anchor = agent
                            .and_then(|a| crate::unreported_idle_anchor_at(s, target, a, at))
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
        .unwrap_or(row.card.created_at);
    Some(Reminder {
        id,
        project_id: row.card.project_id.clone(),
        card_id: row.card.id.clone(),
        message: reason.message.clone(),
        unread: false,
        pending: true,
        sequence: 0,
        created_at,
        audible_when_unread: false,
        event: None,
    })
}

pub fn runtime_reminders(s: &Snapshot) -> Vec<Reminder> {
    runtime_reminders_at(s, now())
}

/// Pure clock-injected variant of [`runtime_reminders`].
pub fn runtime_reminders_at(s: &Snapshot, at: u64) -> Vec<Reminder> {
    let mut out = Vec::new();
    for row in overview_at(s, at).into_iter().filter(|r| r.card.active()) {
        for reason in &row.reasons {
            if let Some(rem) = runtime_reminder_for_reason_at(s, &row, reason, at) {
                out.push(rem);
            }
        }
    }
    out
}

pub fn is_runtime_reminder_id(id: &str) -> bool {
    id.starts_with("wait:")
        || id.starts_with("stalled:")
        || id.starts_with("idle:")
        || id.starts_with("runtime:")
        || id.starts_with("recovery:")
}

/// Recompute only runtime/recovery reminders after applying a partial `RuntimeState`
/// while preserving event-derived unread/gate/delivery reminders in `s.reminders`.
pub fn reconcile_runtime_reminders(s: &mut Snapshot) {
    reconcile_runtime_reminders_at(s, now());
}

/// Pure clock-injected variant of [`reconcile_runtime_reminders`].
pub fn reconcile_runtime_reminders_at(s: &mut Snapshot, at: u64) {
    let mut items = BTreeMap::<String, Reminder>::new();
    for rem in s
        .reminders
        .drain(..)
        .filter(|r| !is_runtime_reminder_id(&r.id))
    {
        items.insert(rem.id.clone(), rem);
    }
    for rem in runtime_reminders_at(s, at) {
        items
            .entry(rem.id.clone())
            .and_modify(|i| i.pending = true)
            .or_insert(rem);
    }
    s.reminders = items
        .into_values()
        .filter(|i| i.pending || i.unread)
        .collect();
}

pub fn reminders_from_events(s: &Snapshot, events: &[Event]) -> Vec<Reminder> {
    reminders_from_events_at(s, events, now())
}

/// Pure clock-injected variant of [`reminders_from_events`].
pub fn reminders_from_events_at(s: &Snapshot, events: &[Event], at: u64) -> Vec<Reminder> {
    let active_cards: BTreeSet<&str> = s
        .cards
        .iter()
        .filter(|c| c.active())
        .map(|c| c.id.as_str())
        .collect();
    let mut by_card: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
    for e in events {
        if active_cards.contains(e.card_id.as_str()) {
            by_card.entry(e.card_id.as_str()).or_default().push(e);
        }
    }
    for card_evs in by_card.values_mut() {
        card_evs.sort_by_key(|e| e.sequence);
    }

    let mut items = BTreeMap::<String, Reminder>::new();
    for (card_id, card_evs) in &by_card {
        let read_seq = read_sequence(s, card_id);
        for &e in card_evs {
            let Some(id) = message_id_in_refs(card_evs, e) else {
                continue;
            };
            let unread = e.sequence > read_seq;
            let audible = event_is_audible_when_unread(&id, e);
            let meta = build_event_meta(e);
            let item = items.entry(id.clone()).or_insert_with(|| Reminder {
                id,
                project_id: e.project_id.clone(),
                card_id: e.card_id.clone(),
                message: e.message.clone(),
                unread: false,
                pending: false,
                sequence: 0,
                created_at: e.created_at,
                audible_when_unread: false,
                event: None,
            });
            item.sequence = item.sequence.max(e.sequence);
            item.unread |= unread;
            item.audible_when_unread |= audible;
            item.message = e.message.clone();
            item.event = Some(meta);
        }
    }

    for row in overview_at(s, at).into_iter().filter(|r| r.card.active()) {
        let card_evs = by_card
            .get(row.card.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for reason in &row.reasons {
            if reason.source == "question" {
                continue;
            }
            if let Some(rem) = runtime_reminder_for_reason_at(s, &row, reason, at) {
                items
                    .entry(rem.id.clone())
                    .and_modify(|i| i.pending = true)
                    .or_insert(rem);
                continue;
            }
            let report = row
                .card
                .human_gate
                .as_ref()
                .and_then(|g| g.terminal_id.as_ref())
                .and_then(|tid| {
                    card_evs
                        .iter()
                        .copied()
                        .filter(|e| {
                            e.data["terminal_id"].as_str() == Some(tid.as_str())
                                && e.kind == "execution.reported"
                                && message_id_in_refs(card_evs, e).is_some()
                        })
                        .max_by_key(|e| e.sequence)
                })
                .or_else(|| {
                    if row.card.human_gate.is_some() {
                        card_evs
                            .iter()
                            .copied()
                            .filter(|e| {
                                e.kind == "execution.reported"
                                    && message_id_in_refs(card_evs, e).is_some()
                            })
                            .max_by_key(|e| e.sequence)
                    } else {
                        None
                    }
                });
            let id = report
                .and_then(|e| message_id_in_refs(card_evs, e))
                .unwrap_or_else(|| format!("gate:{}", row.card.id));
            let created_at = report.map(|e| e.created_at).unwrap_or(row.card.created_at);
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
                    audible_when_unread: false,
                    event: None,
                });
        }
    }
    for q in s
        .questions
        .iter()
        .filter(|q| q.state == "delivery_unknown" && active_cards.contains(q.card_id.as_str()))
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
                audible_when_unread: false,
                event: None,
            },
        );
    }
    items
        .into_values()
        .filter(|i| i.pending || i.unread)
        .collect()
}

pub fn reminders(s: &Snapshot) -> Vec<Reminder> {
    let active = |id: &str| s.cards.iter().any(|c| c.id == id && c.active());
    s.reminders
        .iter()
        .filter(|r| active(&r.card_id))
        .cloned()
        .map(|mut r| {
            if r.sequence > 0 && read_sequence(s, &r.card_id) >= r.sequence {
                r.unread = false;
            }
            r
        })
        .filter(|r| r.pending || r.unread)
        .collect()
}

pub fn sound_event_ids_from_events(s: &Snapshot, events: &[Event]) -> Vec<String> {
    let active_cards: BTreeSet<&str> = s
        .cards
        .iter()
        .filter(|c| c.active())
        .map(|c| c.id.as_str())
        .collect();
    let mut ids = BTreeSet::<String>::new();
    for e in events {
        if !active_cards.contains(e.card_id.as_str()) {
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
    ids.into_iter().collect()
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
    let mut ids = BTreeSet::<String>::new();

    // Pending items (permission/input wait, human_gate, attention, runtime error,
    // 1-hour stalled automatic run `stalled:*`, recovery, delivery_unknown) require user
    // intervention and are audible, except interactive runtime `idle:*` items which stay
    // visual-only.
    // Unread-only items are audible only when they are explicit `@agent` question replies
    // or `backlog` / `blocked` / `failed` reports — never automated `completed` stage steps.
    for item in reminder_items {
        if (item.pending && !item.id.starts_with("idle:"))
            || (item.unread && item.audible_when_unread)
        {
            ids.insert(item.id.clone());
        }
    }

    for id in &s.sound_event_ids {
        ids.insert(id.clone());
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

#[cfg(test)]
mod tests;
