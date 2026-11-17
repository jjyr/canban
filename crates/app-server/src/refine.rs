//! On-demand screen-based status refinement cache and re-probe coordinator.
//!
//! Preserves Canban's zero-idle-polling invariant (`docs/runtime-observation.md`):
//! - Terminals that are plain shells, non-`agy` agents, or in `working` / `blocked` / `error` /
//!   `None` states perform zero `terminal.read` RPCs.
//! - When a live `agy` terminal enters `idle` or `done`, AppServer probes `terminal.read(lines: 0)`
//!   once for that status fingerprint.
//! - If no Swarm or `running` background task is present (`RefineDecision::Keep`), the result is
//!   cached for that fingerprint with `next_check_at = None` (zero timers, zero further RPCs).
//! - Only while a terminal is actively refined to `Working` does AppServer schedule a bounded
//!   re-probe (`LOCAL_RECHECK_MS` / `REMOTE_RECHECK_MS`) so background tasks that exit or get
//!   dismissed without waking the root turn promptly revert to `idle` / `done`.

use super::*;
use canban_herdr::refine::{RefineDecision, RefineInput};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicU64,
};

const LOCAL_RECHECK_MS: u64 = 2_500;
const REMOTE_RECHECK_MS: u64 = 5_000;

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceFingerprint {
    raw_state: String,
    turn: Option<u64>,
    status_ms: Option<u64>,
    reported: Option<u64>,
}

impl SourceFingerprint {
    fn from_record(record: &Value) -> Option<Self> {
        let raw_state = record["status"]["state"].as_str()?.to_owned();
        Some(Self {
            raw_state,
            turn: record["turn"].as_u64(),
            status_ms: record["status_ms"]
                .as_u64()
                .or_else(|| record["status"]["updated_ms"].as_u64()),
            reported: record["reported"].as_u64(),
        })
    }
}

#[derive(Clone, Debug)]
struct CachedRefinement {
    fingerprint: SourceFingerprint,
    decision: RefineDecision,
    next_check_at: Option<u64>,
}

#[derive(Default)]
pub(super) struct RefinementTracker {
    entries: Mutex<BTreeMap<(String, String), CachedRefinement>>,
    probes: AtomicU64,
}

impl RefinementTracker {
    pub(super) fn probes(&self) -> u64 {
        self.probes.load(Ordering::SeqCst)
    }

    pub(super) fn active_working_count(&self) -> usize {
        self.entries
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.decision.is_working())
            .count()
    }

    pub(super) fn next_deadline(&self) -> Option<u64> {
        self.entries
            .lock()
            .unwrap()
            .values()
            .filter_map(|e| e.next_check_at)
            .min()
    }
}

/// Resolve the canonical agent ID (`"agy"`, `"codex"`, etc.) for a Herdr record, combining
/// Herdr's `foreground` / `status.app` push metadata with optional Canban `TerminalEntry`
/// profile hints when `foreground` has not been populated yet.
pub(super) fn resolve_record_agent_id(
    record: &Value,
    entry_hint: Option<&str>,
) -> Option<&'static str> {
    if record["state"] == "finished" {
        return None;
    }
    if let Some(detected) = canban_profiles::identify_herdr_agent(record) {
        return Some(detected);
    }
    // If `foreground` is explicitly present, `identify_herdr_agent` already determined that
    // the foreground process returned to an interactive shell (`zsh`, `bash`, etc.) or an
    // unrelated command — never override that with a launch-time profile hint!
    if record.get("foreground").is_some_and(Value::is_object) {
        return None;
    }
    match entry_hint.map(canban_herdr::refine::canonical_agent_id) {
        Some("agy") => Some("agy"),
        _ => None,
    }
}

pub(super) fn entry_agent_hint(
    snapshot: &Snapshot,
    config: &Config,
    entry: &TerminalEntry,
) -> Option<String> {
    if let Some(run) = &entry.agent {
        if run.finished() {
            return None;
        }
        return canban_profiles::catalog_entry_for_profile(&run.profile)
            .map(|c| c.id)
            .or_else(|| run.profile.preset.clone())
            .or_else(|| Some(run.profile.command_name()));
    }
    if let Some(engine) = &entry.agent_engine {
        return Some(engine.clone());
    }
    if let Some(cfg_session) = snapshot
        .configuration_sessions
        .iter()
        .find(|c| c.id == entry.id)
    {
        let proj = snapshot
            .projects
            .iter()
            .find(|p| p.id == cfg_session.project_id);
        if let Ok(prof) =
            canban_scheduler::select_in_project(config, proj, Some(&cfg_session.profile_id))
        {
            return canban_profiles::catalog_entry_for_profile(&prof)
                .map(|c| c.id)
                .or_else(|| prof.preset.clone())
                .or_else(|| Some(prof.command_name()));
        }
    }
    None
}

impl AppServer {
    pub(super) fn refine_deadline(&self) -> Option<u64> {
        self.refinements.next_deadline()
    }

    pub(super) fn refine_probes(&self) -> u64 {
        self.refinements.probes()
    }

    pub(super) fn refined_working_count(&self) -> usize {
        self.refinements.active_working_count()
    }

    /// Direct safety check before sending an idle-close exit/kill to a terminal: verifies via
    /// `terminal.read(lines: 0)` that an `idle`/`done` `agy` session has not started a Swarm or
    /// background task since the last observation.
    pub(super) fn is_refined_working_now(
        &self,
        client: &canban_herdr::Client,
        entry: &TerminalEntry,
        actual_record: &Value,
    ) -> bool {
        let terminal = entry
            .agent
            .as_ref()
            .and_then(|a| a.terminal.as_deref())
            .or_else(|| entry.runtime.as_ref().and_then(|r| r.terminal.as_deref()));
        let Some(terminal) = terminal else {
            return false;
        };
        let hint = entry
            .agent
            .as_ref()
            .and_then(|a| {
                canban_profiles::catalog_entry_for_profile(&a.profile)
                    .map(|c| c.id)
                    .or_else(|| a.profile.preset.clone())
                    .or_else(|| Some(a.profile.command_name()))
            })
            .or_else(|| entry.agent_engine.clone());
        let agent_id = resolve_record_agent_id(actual_record, hint.as_deref());
        let raw_status = actual_record["status"]["state"].as_str();
        let term_state = actual_record["state"].as_str();
        if !canban_herdr::refine::needs_screen_probe(agent_id, term_state, raw_status) {
            return false;
        }
        let Ok(read) = client.read_screen(terminal) else {
            return false;
        };
        self.refinements.probes.fetch_add(1, Ordering::SeqCst);
        let screen_text = read["text"].as_str().unwrap_or("");
        canban_herdr::refine::refine(RefineInput {
            agent_id: agent_id.unwrap_or(""),
            raw_status,
            screen_text,
        })
        .is_working()
    }

    /// Apply cached and due on-demand screen refinements onto `(core_id, records)`.
    pub(super) fn apply_refinements(&self, core_id: &str, records: &mut [Value]) {
        if core_id.is_empty() {
            self.refinements.entries.lock().unwrap().clear();
            return;
        }
        let snapshot = self.store.lock().unwrap().snapshot().ok();
        let config = self.config();
        let mut hints_by_addr: BTreeMap<String, String> = BTreeMap::new();
        if let Some(snap) = &snapshot {
            for item in &snap.terminals {
                if let Some((bound_core, addr)) = Self::terminal_binding(snap, item)
                    && bound_core == core_id
                    && let Some(hint) = entry_agent_hint(snap, &config, item)
                {
                    hints_by_addr.insert(addr, hint);
                }
            }
        }

        let current_ms = now();
        let mut live_candidates: BTreeSet<(String, String)> = BTreeSet::new();
        struct PendingProbe {
            idx: usize,
            address: String,
            agent_id: &'static str,
            fingerprint: SourceFingerprint,
        }
        let mut to_probe: Vec<PendingProbe> = Vec::new();
        let mut cached_decisions: Vec<(usize, RefineDecision)> = Vec::new();

        {
            let guard = self.refinements.entries.lock().unwrap();
            for (idx, record) in records.iter().enumerate() {
                let Some(addr) = record["address"].as_str() else {
                    continue;
                };
                if !self.remote_connected(addr) {
                    continue;
                }
                let hint = hints_by_addr.get(addr).map(String::as_str);
                let agent_id = resolve_record_agent_id(record, hint);
                let term_state = record["state"].as_str();
                let raw_status = record["status"]["state"].as_str();
                if !canban_herdr::refine::needs_screen_probe(agent_id, term_state, raw_status) {
                    continue;
                }
                let Some(fingerprint) = SourceFingerprint::from_record(record) else {
                    continue;
                };
                let key = (core_id.to_owned(), addr.to_owned());
                live_candidates.insert(key.clone());

                if let Some(cached) = guard.get(&key)
                    && cached.fingerprint == fingerprint
                    && cached.next_check_at.is_none_or(|at| current_ms < at)
                {
                    cached_decisions.push((idx, cached.decision));
                } else {
                    to_probe.push(PendingProbe {
                        idx,
                        address: addr.to_owned(),
                        agent_id: agent_id.unwrap_or("agy"),
                        fingerprint,
                    });
                }
            }
        }

        for (idx, decision) in cached_decisions {
            canban_herdr::refine::apply_decision(&mut records[idx], decision);
        }

        if !to_probe.is_empty() {
            let client = canban_herdr::Client { config };
            let mut updates: Vec<((String, String), CachedRefinement, usize)> =
                Vec::with_capacity(to_probe.len());
            for probe in to_probe {
                let key = (core_id.to_owned(), probe.address.clone());
                let interval = if probe.address.contains('/') {
                    REMOTE_RECHECK_MS
                } else {
                    LOCAL_RECHECK_MS
                };
                match client.read_screen(&probe.address) {
                    Ok(reply) => {
                        self.refinements.probes.fetch_add(1, Ordering::SeqCst);
                        let screen_text = reply["text"].as_str().unwrap_or("");
                        let decision = canban_herdr::refine::refine(RefineInput {
                            agent_id: probe.agent_id,
                            raw_status: Some(probe.fingerprint.raw_state.as_str()),
                            screen_text,
                        });
                        let checked_at = now();
                        let next_check_at = decision
                            .is_working()
                            .then(|| checked_at.saturating_add(interval));
                        updates.push((
                            key,
                            CachedRefinement {
                                fingerprint: probe.fingerprint,
                                decision,
                                next_check_at,
                            },
                            probe.idx,
                        ));
                    }
                    Err(_) => {
                        // Preserve prior Working decision briefly on transient RPC error;
                        // never fabricate Working if previously Keep or unprobed.
                        let previous = self
                            .refinements
                            .entries
                            .lock()
                            .unwrap()
                            .get(&key)
                            .map(|c| c.decision)
                            .unwrap_or(RefineDecision::Keep);
                        let checked_at = now();
                        let next_check_at = previous
                            .is_working()
                            .then(|| checked_at.saturating_add(interval));
                        updates.push((
                            key,
                            CachedRefinement {
                                fingerprint: probe.fingerprint,
                                decision: previous,
                                next_check_at,
                            },
                            probe.idx,
                        ));
                    }
                }
            }
            let mut guard = self.refinements.entries.lock().unwrap();
            for (key, entry, idx) in updates {
                canban_herdr::refine::apply_decision(&mut records[idx], entry.decision);
                guard.insert(key, entry);
            }
            guard.retain(|k, _| live_candidates.contains(k));
        } else {
            self.refinements
                .entries
                .lock()
                .unwrap()
                .retain(|k, _| live_candidates.contains(k));
        }
    }
}
