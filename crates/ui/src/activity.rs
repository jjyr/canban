//! Event-driven UI dwell tracking for Insights ("My time").
use crate::presenter::Ui;
use canban_types::{DwellLane, Snapshot};
use nsh_core::{platform::Lifecycle, prelude::*};
use std::sync::{Mutex, OnceLock};

fn activity_flush_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Returns `true` for Workflow / `@mention` managed Agent terminals and special
/// automated sessions (`configuration:*`, `merge-queue:*`, `scheduled:*`),
/// which should be excluded from personal `Me · Terminal` dwell tracking.
pub fn is_automated_terminal(snapshot: &Snapshot, terminal_id: &str) -> bool {
    if terminal_id.starts_with("configuration:")
        || terminal_id.starts_with("merge-queue:")
        || terminal_id.starts_with("scheduled:")
    {
        return true;
    }
    if snapshot
        .terminals
        .iter()
        .find(|t| t.id == terminal_id)
        .is_some_and(|t| t.agent.is_some() || t.is_special_session())
    {
        return true;
    }
    snapshot
        .configuration_sessions
        .iter()
        .any(|c| c.id == terminal_id)
        || snapshot.merge_batches.iter().any(|b| b.id == terminal_id)
        || snapshot
            .scheduled_runs
            .iter()
            .any(|r| r.id == terminal_id || r.terminal_id.as_deref() == Some(terminal_id))
}

pub const IDLE_THRESHOLD_SECS: u64 = 300;
pub const IDLE_GRACE_MS: u64 = 60_000;
pub const MIN_DWELL_MS: u64 = 5_000;
pub const CHECKPOINT_MS: u64 = 5 * 60_000;
pub const MAX_PENDING_SEGMENTS: usize = 500;
const FLUSH_BATCH_SIZE: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DwellKey {
    pub lane: DwellLane,
    pub page: String,
    pub project_id: Option<String>,
    pub card_id: Option<String>,
    pub terminal_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct PendingDwell {
    pub start_ms: u64,
    pub end_ms: u64,
    pub lane: DwellLane,
    pub page: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
}

#[derive(Default)]
pub struct DwellMachine {
    pub current: Option<(DwellKey, u64)>,
    pub idle: bool,
    idle_key: Option<DwellKey>,
    pub pending: Vec<PendingDwell>,
    last_wake: Option<u32>,
    last_active_at: Option<u64>,
    flushing: bool,
}

impl DwellMachine {
    fn push_segment(&mut self, key: DwellKey, start_ms: u64, end_ms: u64) {
        if end_ms.saturating_sub(start_ms) < MIN_DWELL_MS {
            return;
        }
        self.pending.push(PendingDwell {
            start_ms,
            end_ms,
            lane: key.lane,
            page: key.page,
            project_id: key.project_id,
            card_id: key.card_id,
            terminal_id: key.terminal_id,
        });
        if self.pending.len() > MAX_PENDING_SEGMENTS {
            let overflow = self.pending.len() - MAX_PENDING_SEGMENTS;
            self.pending.drain(..overflow);
        }
    }

    fn idle_cutoff(now: u64, start: u64, idle_secs: u64) -> u64 {
        start
            .max(
                now.saturating_sub(idle_secs.saturating_mul(1000))
                    .saturating_add(IDLE_GRACE_MS),
            )
            .min(now)
    }

    pub fn transition(&mut self, now: u64, key: Option<DwellKey>, idle_secs: Option<u64>) {
        let Some(next) = key else {
            if let Some((cur, start)) = self.current.take() {
                let end = match idle_secs.filter(|&s| s >= IDLE_THRESHOLD_SECS) {
                    Some(idle) => Self::idle_cutoff(now, start, idle),
                    None => now,
                };
                self.push_segment(cur, start, end);
            }
            self.idle = false;
            self.idle_key = None;
            return;
        };

        if let Some(idle) = idle_secs.filter(|&s| s >= IDLE_THRESHOLD_SECS) {
            if let Some((cur, start)) = self.current.take() {
                let end = Self::idle_cutoff(now, start, idle);
                self.push_segment(cur, start, end);
            }
            self.idle = true;
            self.idle_key = Some(next);
            return;
        }

        if self.idle {
            let resume_start = now.saturating_sub(idle_secs.unwrap_or(0).saturating_mul(1000));
            self.idle = false;
            self.idle_key = None;
            self.current = Some((next, resume_start));
            return;
        }

        if let Some((cur, _)) = &self.current
            && *cur == next
        {
            return;
        }
        if let Some((cur, start)) = self.current.take() {
            self.push_segment(cur, start, now);
        }
        self.current = Some((next, now));
    }

    pub fn idle_check(&mut self, now: u64, idle_secs: Option<u64>) {
        if let Some(idle) = idle_secs.filter(|&s| s >= IDLE_THRESHOLD_SECS) {
            if let Some((cur, start)) = self.current.take() {
                let end = Self::idle_cutoff(now, start, idle);
                self.push_segment(cur.clone(), start, end);
                self.idle = true;
                self.idle_key = Some(cur);
            }
        } else if self.idle {
            let resume_start = now.saturating_sub(idle_secs.unwrap_or(0).saturating_mul(1000));
            self.idle = false;
            if let Some(key) = self.idle_key.take() {
                self.current = Some((key, resume_start));
            }
        }
    }

    pub fn checkpoint(&mut self, now: u64) {
        if let Some((cur, start)) = &mut self.current
            && now.saturating_sub(*start) >= CHECKPOINT_MS
        {
            let key = cur.clone();
            let old_start = *start;
            *start = now;
            self.push_segment(key, old_start, now);
        }
    }

    pub fn take_flush_batch(&mut self) -> Vec<PendingDwell> {
        if self.flushing || self.pending.is_empty() {
            return Vec::new();
        }
        self.flushing = true;
        let count = self.pending.len().min(FLUSH_BATCH_SIZE);
        self.pending.drain(..count).collect()
    }

    pub fn finish_flush(&mut self, batch: Vec<PendingDwell>, ok: bool) -> bool {
        self.flushing = false;
        if !ok {
            let mut restored = batch;
            restored.append(&mut self.pending);
            if restored.len() > MAX_PENDING_SEGMENTS {
                let overflow = restored.len() - MAX_PENDING_SEGMENTS;
                restored.drain(..overflow);
            }
            self.pending = restored;
            return false;
        }
        !self.pending.is_empty()
    }

    fn handle_event(&mut self, now: u64, wake: u32, key: Option<DwellKey>, idle_secs: Option<u64>) {
        if self.last_wake.is_some_and(|prev| prev != wake) {
            if let Some((cur, start)) = self.current.take() {
                let cap = self
                    .last_active_at
                    .map_or(now, |t| t.saturating_add(IDLE_GRACE_MS))
                    .min(now);
                self.push_segment(cur, start, start.max(cap));
            }
            self.idle = false;
            self.idle_key = None;
        }
        self.last_wake = Some(wake);
        self.transition(now, key, idle_secs);
        self.last_active_at = Some(now);
    }
}

#[cfg(target_os = "macos")]
pub fn system_idle_secs() -> Option<u64> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(source_state_id: i32, event_type: u32) -> f64;
    }
    // kCGEventSourceStateCombinedSessionState = 0, kCGAnyInputEventType = u32::MAX.
    let secs = unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) };
    (secs.is_finite() && secs >= 0.0).then_some(secs.floor() as u64)
}

#[cfg(not(target_os = "macos"))]
pub fn system_idle_secs() -> Option<u64> {
    None
}

fn non_empty(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn current_dwell_key(
    ui: Ui,
    page: &str,
    card: &str,
    terminal_sel: &str,
    project: &str,
) -> Option<DwellKey> {
    let terminal_card = ui.terminal_card_context.get_untracked();
    ui.snapshot.with_untracked(|s| match page {
        "terminals" | "terminal" => {
            let terminal_id = non_empty(terminal_sel);
            if terminal_id
                .as_deref()
                .is_some_and(|tid| is_automated_terminal(s, tid))
            {
                return None;
            }
            let entry = terminal_id
                .as_deref()
                .and_then(|tid| s.terminals.iter().find(|t| t.id == tid));
            let project_id = entry
                .and_then(|t| s.terminal_project_id(t).map(str::to_owned))
                .or_else(|| non_empty(project));
            let card_id = entry
                .and_then(|t| t.agent.as_ref().map(|a| a.card_id.clone()))
                .or_else(|| non_empty(&terminal_card))
                .or_else(|| {
                    terminal_id.as_deref().and_then(|tid| {
                        s.terminal_contexts
                            .iter()
                            .find(|c| c.terminal_id == tid)
                            .and_then(|c| c.card_id.clone())
                    })
                })
                .or_else(|| entry.and_then(|t| t.origin_card_id.clone()));
            Some(DwellKey {
                lane: DwellLane::Terminal,
                page: "terminals".into(),
                project_id,
                card_id,
                terminal_id,
            })
        }
        "card" | "review" => {
            let card_id = non_empty(card);
            let project_id = card_id
                .as_deref()
                .and_then(|cid| s.cards.iter().find(|c| c.id == cid))
                .map(|c| c.project_id.clone())
                .or_else(|| non_empty(project));
            Some(DwellKey {
                lane: DwellLane::Card,
                page: page.to_owned(),
                project_id,
                card_id,
                terminal_id: None,
            })
        }
        other => Some(DwellKey {
            lane: DwellLane::Other,
            page: other.to_owned(),
            project_id: non_empty(project),
            card_id: None,
            terminal_id: None,
        }),
    })
}

/// Serializes `activity.record` flushes on blocking threads so Insights queries
/// that run right after entering the Insights page always observe committed dwell.
pub(crate) fn flush_batch_blocking(batch: &[PendingDwell]) -> (bool, u64) {
    let _guard = activity_flush_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if batch.is_empty() {
        return (true, 0);
    }
    let payload = serde_json::json!({ "segments": batch });
    match crate::routed_call("activity.record", payload, None) {
        Ok(val) => {
            let recorded = val
                .get("recorded")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(batch.len() as u64);
            (true, recorded)
        }
        Err(_) => (false, 0),
    }
}

/// Ensures the active dwell segment has transitioned to the current page and
/// drains any pending dwell batch so Insights can flush it before querying.
pub(crate) fn take_pending_for_insights(ui: Ui) -> Vec<PendingDwell> {
    let page = ui.page.get_untracked();
    let card = ui.card.get_untracked();
    let terminal = ui.terminal_selection.get_untracked();
    let project = ui.project.get_untracked();
    let key = (lifecycle() == Lifecycle::Active)
        .then(|| current_dwell_key(ui, &page, &card, &terminal, &project))
        .flatten();
    let now = canban_types::now();
    let wake = wake_count();
    let idle = system_idle_secs();
    ui.dwell
        .try_with(|m| {
            m.handle_event(now, wake, key, idle);
            m.take_flush_batch()
        })
        .unwrap_or_default()
}

pub(crate) fn complete_insights_flush(ui: Ui, batch: Vec<PendingDwell>, ok: bool, recorded: u64) {
    if batch.is_empty() {
        return;
    }
    let Some(flush_more) = ui.dwell.try_with(|m| m.finish_flush(batch, ok)) else {
        return;
    };
    if ok
        && recorded >= 1
        && let Some(cur) = ui.activity_flush_seq.try_get()
    {
        let _ = ui.activity_flush_seq.try_set(cur.wrapping_add(1));
    }
    if flush_more {
        flush_pending(ui);
    }
}

fn flush_pending(ui: Ui) {
    let Some(batch) = ui.dwell.try_with(DwellMachine::take_flush_batch) else {
        return;
    };
    if batch.is_empty() {
        return;
    }
    ui.spawn(async move {
        let batch_for_req = batch.clone();
        let res = blocking(move || flush_batch_blocking(&batch_for_req)).await;
        let (ok, recorded) = res.unwrap_or((false, 0));
        complete_insights_flush(ui, batch, ok, recorded);
    });
}

pub fn init(ui: Ui) {
    if !crate::backend::is_connected() {
        return;
    }
    effect(move || {
        let page = ui.page.get();
        let card = ui.card.get();
        let terminal = ui.terminal_selection.get();
        let project = ui.project.get();
        let lc = lifecycle();
        let wake = wake_count();
        let key = (lc == Lifecycle::Active)
            .then(|| current_dwell_key(ui, &page, &card, &terminal, &project))
            .flatten();
        let now = canban_types::now();
        let idle = system_idle_secs();
        let should_flush = ui.dwell.with(|m| {
            m.handle_event(now, wake, key, idle);
            !m.pending.is_empty() && !m.flushing
        });
        if should_flush && page != "insights" {
            flush_pending(ui);
        }
    });
}

pub fn tick(ui: Ui) {
    if !crate::backend::is_connected() {
        return;
    }
    let now = canban_types::now();
    let idle = system_idle_secs();
    let should_flush = ui.dwell.with(|m| {
        m.idle_check(now, idle);
        m.checkpoint(now);
        m.last_active_at = Some(now);
        !m.pending.is_empty() && !m.flushing
    });
    if should_flush {
        flush_pending(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(lane: DwellLane, page: &str, id: Option<&str>) -> DwellKey {
        DwellKey {
            lane,
            page: page.into(),
            project_id: Some("p1".into()),
            card_id: (lane == DwellLane::Card).then(|| id.unwrap_or("c1").into()),
            terminal_id: (lane == DwellLane::Terminal).then(|| id.unwrap_or("t1").into()),
        }
    }

    #[test]
    fn navigation_drops_short_segments_and_records_longer_ones() {
        let mut m = DwellMachine::default();
        let board = key(DwellLane::Other, "board", None);
        let card = key(DwellLane::Card, "card", Some("c1"));
        let term = key(DwellLane::Terminal, "terminals", Some("t1"));

        m.transition(10_000, Some(board.clone()), Some(0));
        // Duplicate same-key transition does nothing
        m.transition(12_000, Some(board), Some(0));
        // < 5s -> dropped
        m.transition(14_000, Some(card.clone()), Some(0));
        assert!(m.pending.is_empty());

        // Stay 12s on card, then move to terminal
        m.transition(26_000, Some(term), Some(0));
        assert_eq!(m.pending.len(), 1);
        assert_eq!(m.pending[0].lane, DwellLane::Card);
        assert_eq!(m.pending[0].start_ms, 14_000);
        assert_eq!(m.pending[0].end_ms, 26_000);
        assert_eq!(m.pending[0].card_id.as_deref(), Some("c1"));

        // App goes background after 10s -> closes terminal segment
        m.transition(36_000, None, Some(0));
        assert_eq!(m.pending.len(), 2);
        assert_eq!(m.pending[1].lane, DwellLane::Terminal);
        assert_eq!(m.pending[1].start_ms, 26_000);
        assert_eq!(m.pending[1].end_ms, 36_000);
        assert!(m.current.is_none());
    }

    #[test]
    fn idle_truncates_with_grace_and_resumes_at_input_time() {
        let mut m = DwellMachine::default();
        let card = key(DwellLane::Card, "card", Some("c2"));

        m.transition(100_000, Some(card.clone()), Some(0));
        // 6 minutes later, user has been idle for 330 seconds.
        m.idle_check(460_000, Some(330));
        assert!(m.idle);
        assert!(m.current.is_none());
        assert_eq!(m.pending.len(), 1);
        // Expected end = 460_000 - 330_000 + 60_000 = 190_000
        assert_eq!(m.pending[0].start_ms, 100_000);
        assert_eq!(m.pending[0].end_ms, 190_000);

        // User moves mouse 12s before next 60s tick -> resumes at now - 12s
        m.idle_check(520_000, Some(12));
        assert!(!m.idle);
        assert_eq!(m.current, Some((card, 508_000)));
    }

    #[test]
    fn checkpoint_splits_long_segments_every_five_minutes() {
        let mut m = DwellMachine::default();
        let term = key(DwellLane::Terminal, "terminals", Some("t2"));
        m.transition(1_000, Some(term.clone()), Some(0));
        m.checkpoint(200_000);
        assert!(m.pending.is_empty());

        m.checkpoint(1_000 + CHECKPOINT_MS);
        assert_eq!(m.pending.len(), 1);
        assert_eq!(m.pending[0].start_ms, 1_000);
        assert_eq!(m.pending[0].end_ms, 1_000 + CHECKPOINT_MS);
        assert_eq!(m.current, Some((term, 1_000 + CHECKPOINT_MS)));
    }

    #[test]
    fn overflow_caps_pending_at_500_dropping_oldest() {
        let mut m = DwellMachine::default();
        let board = key(DwellLane::Other, "board", None);
        let card = key(DwellLane::Card, "card", Some("c1"));
        for i in 0..510u64 {
            let k = if i % 2 == 0 {
                board.clone()
            } else {
                card.clone()
            };
            m.transition(i * 10_000, Some(k), Some(0));
        }
        assert_eq!(m.pending.len(), MAX_PENDING_SEGMENTS);
        assert_eq!(m.pending[0].start_ms, 90_000);
    }

    #[test]
    fn wake_from_sleep_caps_segment_instead_of_crediting_sleep_gap() {
        let mut m = DwellMachine::default();
        let card = key(DwellLane::Card, "card", Some("c9"));
        m.handle_event(10_000, 0, Some(card.clone()), Some(0));
        // Laptop slept 3 hours -> wake_count bumped to 1
        m.handle_event(10_810_000, 1, Some(card.clone()), Some(0));
        assert_eq!(m.pending.len(), 1);
        assert_eq!(m.pending[0].start_ms, 10_000);
        assert_eq!(m.pending[0].end_ms, 70_000);
        assert_eq!(m.current, Some((card, 10_810_000)));
    }

    #[test]
    fn flush_queue_requeues_on_failure_and_enforces_500_cap() {
        let mut m = DwellMachine::default();
        assert!(m.take_flush_batch().is_empty());

        let board = key(DwellLane::Other, "board", None);
        let card = key(DwellLane::Card, "card", Some("c1"));
        for i in 0..250u64 {
            let k = if i % 2 == 0 {
                board.clone()
            } else {
                card.clone()
            };
            m.transition((i + 1) * 10_000, Some(k), Some(0));
        }
        // Finalize the 250th segment so all 250 sit in pending.
        m.transition(251 * 10_000, None, Some(0));
        assert_eq!(m.pending.len(), 250);

        let batch = m.take_flush_batch();
        assert_eq!(batch.len(), FLUSH_BATCH_SIZE);
        assert_eq!(m.pending.len(), 50);
        // Concurrent flush attempt while in-flight is ignored.
        assert!(m.take_flush_batch().is_empty());

        // New segments arrive while the batch is in flight.
        for i in 251..651u64 {
            let k = if i % 2 == 0 {
                board.clone()
            } else {
                card.clone()
            };
            m.transition(i * 10_000, Some(k), Some(0));
        }
        m.transition(651 * 10_000, None, Some(0));
        assert_eq!(m.pending.len(), 450);

        // Failure re-queues unsent batch ahead of new pending segments and enforces 500-cap.
        assert!(!m.finish_flush(batch, false));
        assert_eq!(m.pending.len(), MAX_PENDING_SEGMENTS);
        assert_eq!(m.pending[0].start_ms, 151 * 10_000);
        assert_eq!(m.pending[MAX_PENDING_SEGMENTS - 1].start_ms, 650 * 10_000);

        // Drain all 500 segments cleanly across three batches on success.
        let b1 = m.take_flush_batch();
        assert_eq!(b1.len(), 200);
        assert!(m.finish_flush(b1, true));
        let b2 = m.take_flush_batch();
        assert_eq!(b2.len(), 200);
        assert!(m.finish_flush(b2, true));
        let b3 = m.take_flush_batch();
        assert_eq!(b3.len(), 100);
        assert!(!m.finish_flush(b3, true));
        assert!(m.pending.is_empty());

        // Re-queue below cap preserves exact chronological order without loss.
        for i in 0..10u64 {
            let k = if i % 2 == 0 {
                board.clone()
            } else {
                card.clone()
            };
            m.transition((i + 1) * 10_000, Some(k), Some(0));
        }
        m.transition(11 * 10_000, None, Some(0));
        let b_small = m.take_flush_batch();
        assert_eq!(b_small.len(), 10);
        m.transition(12 * 10_000, Some(board), Some(0));
        m.transition(13 * 10_000, None, Some(0));
        assert_eq!(m.pending.len(), 1);
        assert!(!m.finish_flush(b_small, false));
        assert_eq!(m.pending.len(), 11);
        assert_eq!(m.pending[0].start_ms, 10_000);
        assert_eq!(m.pending[10].start_ms, 120_000);
    }

    #[test]
    fn automated_terminals_are_excluded_from_dwell_key() {
        use canban_types::{AgentRun, TerminalEntry};

        #[component]
        fn Root() -> impl IntoElement {
            let ui = Ui::new();
            let mut snap = Snapshot::default();
            snap.terminals.push(TerminalEntry {
                id: "shell-manual-1".into(),
                conversation_id: None,
                number: 1,
                name: Some("Manual Shell".into()),
                origin_project_id: Some("p1".into()),
                origin_card_id: None,
                agent_engine: None,
                runtime: None,
                agent: None,
                created_at: 1,
            });
            snap.terminals.push(TerminalEntry {
                id: "card-1:developer".into(),
                conversation_id: None,
                number: 2,
                name: None,
                origin_project_id: Some("p1".into()),
                origin_card_id: Some("card-1".into()),
                agent_engine: None,
                runtime: None,
                agent: Some(
                    serde_json::from_value::<AgentRun>(serde_json::json!({
                        "card_id": "card-1",
                        "project_id": "p1",
                        "state": "running",
                        "profile": { "id": "developer" }
                    }))
                    .unwrap(),
                ),
                created_at: 2,
            });
            ui.snapshot.set(snap);

            // Manual terminal is tracked as Me · Terminal
            let manual = current_dwell_key(ui, "terminals", "", "shell-manual-1", "p1");
            assert_eq!(
                manual.as_ref().map(|k| (&k.lane, k.terminal_id.as_deref())),
                Some((&DwellLane::Terminal, Some("shell-manual-1")))
            );

            // Managed Agent terminal (entry.agent.is_some()) is excluded
            assert!(current_dwell_key(ui, "terminals", "", "card-1:developer", "p1").is_none());

            // Special automated sessions are excluded even without a live TerminalEntry
            assert!(
                current_dwell_key(ui, "terminals", "", "configuration:p1:workflows:dev", "p1")
                    .is_none()
            );
            assert!(current_dwell_key(ui, "terminals", "", "merge-queue:p1:abcd", "p1").is_none());
            assert!(current_dwell_key(ui, "terminals", "", "scheduled:p1:ef12", "p1").is_none());
            view()
        }

        let mut core = nsh_core::Core::new(
            nsh_core::ApproxTextEngine,
            nsh_core::geometry::size(px(100.), px(100.)),
        );
        core.mount_root(&nsh_core::element::Root::of::<Root>());
        let _ = core.commit();
    }
}
