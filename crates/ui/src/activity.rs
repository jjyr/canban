//! Event-driven UI dwell tracking for Insights ("My time").
use crate::presenter::Ui;
use canban_types::DwellLane;
use nsh_core::{platform::Lifecycle, prelude::*};

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
) -> DwellKey {
    let terminal_card = ui.terminal_card_context.get_untracked();
    ui.snapshot.with_untracked(|s| match page {
        "terminals" | "terminal" => {
            let terminal_id = non_empty(terminal_sel);
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
            DwellKey {
                lane: DwellLane::Terminal,
                page: "terminals".into(),
                project_id,
                card_id,
                terminal_id,
            }
        }
        "card" | "review" => {
            let card_id = non_empty(card);
            let project_id = card_id
                .as_deref()
                .and_then(|cid| s.cards.iter().find(|c| c.id == cid))
                .map(|c| c.project_id.clone())
                .or_else(|| non_empty(project));
            DwellKey {
                lane: DwellLane::Card,
                page: page.to_owned(),
                project_id,
                card_id,
                terminal_id: None,
            }
        }
        other => DwellKey {
            lane: DwellLane::Other,
            page: other.to_owned(),
            project_id: non_empty(project),
            card_id: None,
            terminal_id: None,
        },
    })
}

fn flush_pending(ui: Ui) {
    let batch: Vec<PendingDwell> = ui.dwell.with(|m| {
        if m.flushing || m.pending.is_empty() {
            return Vec::new();
        }
        m.flushing = true;
        let count = m.pending.len().min(FLUSH_BATCH_SIZE);
        m.pending.drain(..count).collect()
    });
    if batch.is_empty() {
        return;
    }
    ui.spawn(async move {
        let payload = serde_json::json!({ "segments": batch });
        let res = blocking(move || crate::routed_call("activity.record", payload, None)).await;
        let ok = matches!(res, Ok(Ok(_)));
        let flush_more = ui.dwell.with(|m| {
            m.flushing = false;
            if !ok {
                let mut restored = batch;
                restored.append(&mut m.pending);
                if restored.len() > MAX_PENDING_SEGMENTS {
                    let overflow = restored.len() - MAX_PENDING_SEGMENTS;
                    restored.drain(..overflow);
                }
                m.pending = restored;
                return false;
            }
            !m.pending.is_empty()
        });
        if flush_more {
            flush_pending(ui);
        }
    });
}

pub fn init(ui: Ui) {
    if crate::APP.get().is_none() && crate::HOSTS.get().is_none() {
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
            .then(|| current_dwell_key(ui, &page, &card, &terminal, &project));
        let now = canban_types::now();
        let idle = system_idle_secs();
        let should_flush = ui.dwell.with(|m| {
            m.handle_event(now, wake, key, idle);
            !m.pending.is_empty() && !m.flushing
        });
        if should_flush {
            flush_pending(ui);
        }
    });
}

pub fn tick(ui: Ui) {
    if crate::APP.get().is_none() && crate::HOSTS.get().is_none() {
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
}
