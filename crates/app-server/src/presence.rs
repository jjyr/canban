//! In-memory viewer presence leases and scheduled-run follow-up tracking.
//! Zero SQLite writes; bounded strictly to active clients and live terminals.

use super::*;
use std::collections::{BTreeSet, HashMap};

/// Fallback expiration for a client presence lease when no heartbeat or clear arrives (~25s).
pub(super) const PRESENCE_LEASE_TTL_MS: u64 = 25_000;

/// Maximum retained historical view timestamps per map to prevent unbounded growth.
const MAX_VIEW_HISTORY: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ClientLease {
    card_id: Option<String>,
    terminal_id: Option<String>,
    last_seen_at: u64,
    expires_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ScheduledReclaimTrack {
    pub settled_once: bool,
    pub interactive_followup: bool,
    pub idle_since: Option<u64>,
    pub completion_turn: Option<u64>,
    pub last_turn: Option<u64>,
}

type ReclaimEmitKey = Vec<(String, bool, bool, bool, bool)>;

#[derive(Default)]
struct PresenceState {
    leases: HashMap<String, ClientLease>,
    last_viewed_card: HashMap<String, u64>,
    last_viewed_terminal: HashMap<String, u64>,
    scheduled_followups: HashMap<String, ScheduledReclaimTrack>,
    last_emitted_key: ReclaimEmitKey,
}

#[derive(Default)]
pub(super) struct PresenceTracker {
    state: Mutex<PresenceState>,
}

fn non_empty_id(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn record_timestamp(map: &mut HashMap<String, u64>, key: &str, at: u64) {
    let slot = map.entry(key.to_owned()).or_insert(0);
    if at > *slot {
        *slot = at;
    }
    if map.len() > MAX_VIEW_HISTORY
        && let Some(oldest_key) = map
            .iter()
            .min_by_key(|(_, ts)| **ts)
            .map(|(k, _)| k.clone())
    {
        map.remove(&oldest_key);
    }
}

fn active_sets(
    leases: &HashMap<String, ClientLease>,
    at: u64,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut cards = BTreeSet::new();
    let mut terminals = BTreeSet::new();
    for lease in leases.values() {
        if lease.expires_at > at {
            if let Some(cid) = &lease.card_id {
                cards.insert(cid.clone());
            }
            if let Some(tid) = &lease.terminal_id {
                terminals.insert(tid.clone());
            }
        }
    }
    (cards, terminals)
}

fn prune_expired_locked(state: &mut PresenceState, at: u64) -> bool {
    let expired: Vec<String> = state
        .leases
        .iter()
        .filter(|(_, lease)| lease.expires_at <= at)
        .map(|(id, _)| id.clone())
        .collect();
    if expired.is_empty() {
        return false;
    }
    for id in expired {
        if let Some(lease) = state.leases.remove(&id) {
            if let Some(cid) = &lease.card_id {
                record_timestamp(&mut state.last_viewed_card, cid, lease.expires_at);
            }
            if let Some(tid) = &lease.terminal_id {
                record_timestamp(&mut state.last_viewed_terminal, tid, lease.expires_at);
            }
        }
    }
    true
}

impl PresenceTracker {
    /// Updates or clears a client's active viewing lease.
    /// Returns `true` only when the actively protected `(cards, terminals)` set changed.
    pub(super) fn update(
        &self,
        client_id: String,
        card_id: Option<String>,
        terminal_id: Option<String>,
        at: u64,
    ) -> bool {
        let card_id = non_empty_id(card_id.as_deref());
        let terminal_id = non_empty_id(terminal_id.as_deref());
        let mut state = self.state.lock().unwrap();
        let prev_active = active_sets(&state.leases, at);
        prune_expired_locked(&mut state, at);

        if let Some(prev) = state.leases.get(&client_id).cloned() {
            if let Some(cid) = &prev.card_id {
                record_timestamp(&mut state.last_viewed_card, cid, at);
            }
            if let Some(tid) = &prev.terminal_id {
                record_timestamp(&mut state.last_viewed_terminal, tid, at);
            }
        }
        if let Some(cid) = &card_id {
            record_timestamp(&mut state.last_viewed_card, cid, at);
        }
        if let Some(tid) = &terminal_id {
            record_timestamp(&mut state.last_viewed_terminal, tid, at);
        }

        if card_id.is_none() && terminal_id.is_none() {
            state.leases.remove(&client_id);
        } else {
            state.leases.insert(
                client_id,
                ClientLease {
                    card_id,
                    terminal_id,
                    last_seen_at: at,
                    expires_at: at.saturating_add(PRESENCE_LEASE_TTL_MS),
                },
            );
        }
        let next_active = active_sets(&state.leases, at);
        prev_active != next_active
    }

    pub(super) fn prune_expired(&self, at: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        prune_expired_locked(&mut state, at)
    }

    pub(super) fn next_expiry(&self, at: u64) -> Option<u64> {
        let state = self.state.lock().unwrap();
        state
            .leases
            .values()
            .filter(|lease| lease.expires_at > at)
            .map(|lease| lease.expires_at.saturating_add(1))
            .min()
    }

    pub(super) fn is_protected(&self, terminal_id: &str, card_id: Option<&str>, at: u64) -> bool {
        let state = self.state.lock().unwrap();
        state.leases.values().any(|lease| {
            lease.expires_at > at
                && (lease.terminal_id.as_deref() == Some(terminal_id)
                    || card_id.is_some_and(|cid| lease.card_id.as_deref() == Some(cid)))
        })
    }

    pub(super) fn last_viewed_at(&self, terminal_id: &str, card_id: Option<&str>, at: u64) -> u64 {
        let state = self.state.lock().unwrap();
        let active = state.leases.values().any(|lease| {
            lease.expires_at > at
                && (lease.terminal_id.as_deref() == Some(terminal_id)
                    || card_id.is_some_and(|cid| lease.card_id.as_deref() == Some(cid)))
        });
        if active {
            return at;
        }
        let expired_lease_at = state
            .leases
            .values()
            .filter(|lease| {
                lease.expires_at <= at
                    && (lease.terminal_id.as_deref() == Some(terminal_id)
                        || card_id.is_some_and(|cid| lease.card_id.as_deref() == Some(cid)))
            })
            .map(|lease| lease.expires_at)
            .max()
            .unwrap_or(0);
        let term_at = state
            .last_viewed_terminal
            .get(terminal_id)
            .copied()
            .unwrap_or(0);
        let card_at = card_id
            .and_then(|cid| state.last_viewed_card.get(cid).copied())
            .unwrap_or(0);
        term_at.max(card_at).max(expired_lease_at)
    }

    pub(super) fn sync_emitted_reclaim(&self, status: &[TerminalReclaimStatus]) -> bool {
        let mut key: ReclaimEmitKey = status
            .iter()
            .map(|s| {
                (
                    s.terminal_id.clone(),
                    s.paused,
                    s.reclaiming_soon,
                    s.auto_reclaiming,
                    s.interactive_followup,
                )
            })
            .collect();
        key.sort();
        let mut state = self.state.lock().unwrap();
        if state.last_emitted_key == key {
            return false;
        }
        state.last_emitted_key = key;
        true
    }

    pub(super) fn observe_scheduled_runtime(
        &self,
        terminal_id: &str,
        status: Option<&str>,
        turn: Option<u64>,
        viewed_or_protected: bool,
        at: u64,
    ) -> ScheduledReclaimTrack {
        let mut state = self.state.lock().unwrap();
        let entry = state
            .scheduled_followups
            .entry(terminal_id.to_owned())
            .or_default();
        if matches!(status, Some("idle" | "done" | "error")) {
            if !entry.settled_once {
                entry.settled_once = true;
                entry.idle_since = Some(at);
                if entry.completion_turn.is_none() {
                    entry.completion_turn = turn;
                }
                entry.last_turn = turn;
            } else {
                let turn_advanced =
                    turn.is_some_and(|t| entry.last_turn.is_none_or(|prev| t > prev));
                if entry
                    .completion_turn
                    .zip(turn)
                    .is_some_and(|(ct, t)| t > ct)
                {
                    entry.interactive_followup = true;
                }
                if entry.idle_since.is_none() || turn_advanced {
                    entry.idle_since = Some(at);
                }
                if turn.is_some() {
                    entry.last_turn = turn;
                }
            }
        } else if matches!(status, Some("working" | "running" | "blocked")) {
            entry.idle_since = None;
            if entry.settled_once
                || viewed_or_protected
                || entry
                    .completion_turn
                    .zip(turn)
                    .is_some_and(|(ct, t)| t > ct)
            {
                entry.interactive_followup = true;
            }
            if turn.is_some() {
                entry.last_turn = turn;
            }
        }
        entry.clone()
    }

    pub(super) fn get_scheduled_track(&self, terminal_id: &str) -> Option<ScheduledReclaimTrack> {
        self.state
            .lock()
            .unwrap()
            .scheduled_followups
            .get(terminal_id)
            .cloned()
    }

    pub(super) fn remove_scheduled_track(&self, terminal_id: &str) {
        self.state
            .lock()
            .unwrap()
            .scheduled_followups
            .remove(terminal_id);
    }

    pub(super) fn retain_live(&self, snapshot: &Snapshot, at: u64) {
        let live_cards: BTreeSet<&str> = snapshot
            .cards
            .iter()
            .filter(|c| c.active())
            .map(|c| c.id.as_str())
            .collect();
        let live_terminals: BTreeSet<&str> = snapshot
            .terminals
            .iter()
            .filter(|t| {
                t.agent.as_ref().is_some_and(|a| !a.finished())
                    || t.runtime.as_ref().is_some_and(|r| r.state != "finished")
            })
            .map(|t| t.id.as_str())
            .collect();
        let live_scheduled: BTreeSet<&str> = snapshot
            .scheduled_runs
            .iter()
            .filter(|r| !r.creates_card)
            .filter_map(|r| {
                let tid = r.terminal_id.as_deref()?;
                snapshot
                    .terminals
                    .iter()
                    .find(|t| t.id == tid)
                    .and_then(|t| t.runtime.as_ref())
                    .filter(|rt| rt.state != "finished")
                    .map(|_| tid)
            })
            .collect();
        let max_age = 3_600_000_u64;
        let mut state = self.state.lock().unwrap();
        state.last_viewed_card.retain(|cid, ts| {
            live_cards.contains(cid.as_str()) && at.saturating_sub(*ts) < max_age
        });
        state.last_viewed_terminal.retain(|tid, ts| {
            live_terminals.contains(tid.as_str()) && at.saturating_sub(*ts) < max_age
        });
        state
            .scheduled_followups
            .retain(|tid, _| live_scheduled.contains(tid.as_str()));
    }
}

impl AppServer {
    pub(super) fn viewer_presence(&self, p: Value) -> Result<Value> {
        let client_id = strp(&p, "client_id")?.trim().to_owned();
        ensure!(!client_id.is_empty(), "Missing client_id");
        if let Some(sz) = Self::parse_terminal_size(&p["terminal_size"]) {
            self.record_terminal_size(sz, false);
        }
        let card_id = p.get("card_id").and_then(Value::as_str).map(str::to_owned);
        let terminal_id = p
            .get("terminal_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let at = now();
        let changed = self.presence.update(client_id, card_id, terminal_id, at);
        if changed {
            let mut s = self
                .store
                .lock()
                .unwrap()
                .db()
                .runtime_snapshot()
                .unwrap_or_default();
            self.project_observation_freshness(&mut s);
            let status = self.compute_reclaim_status(&s, at);
            if self.presence.sync_emitted_reclaim(&status) {
                self.emit("reclaim", None);
            } else {
                self.wake_runtime();
            }
        }
        Ok(json!({ "ok": true }))
    }
}
