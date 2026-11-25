//! Deterministic wall clock backed by `turmoil::sim_elapsed()` with per-host skew,
//! dynamic NTP clock jumps, and configurable drift.
use canban_types::Clock;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

/// Fixed deterministic epoch base (`2023-11-14T22:13:20Z` in epoch ms).
pub const SIM_EPOCH_BASE_MS: u64 = 1_700_000_000_000;

#[derive(Debug)]
struct ClockAdjust {
    /// Initial boot skew so callers can inspect or reset dynamic adjustments.
    initial_skew_ms: i64,
    /// Cumulative offset in milliseconds (initial skew + NTP jumps + banked drift).
    offset_ms: i64,
    /// Linear drift rate in parts-per-million (`+1_000` = +0.1% faster, `-1_000` = -0.1% slower).
    drift_ppm: i64,
    /// `elapsed_ms()` snapshot when `drift_ppm` was last set or re-anchored.
    drift_anchor_ms: u64,
}

/// Deterministic wall clock synchronized with `turmoil`'s virtual simulation clock.
///
/// Inside a Turmoil host/client task, `turmoil::sim_elapsed()` is authoritative.
/// When inspected from outside a host task (for example, by the driver or oracle between
/// `sim.step()` calls), `shared_elapsed_ms` supplies the exact elapsed milliseconds recorded
/// after the latest `sim.step()`.
///
/// Cloning a `TurmoilClock` shares its dynamic adjustment state (`jump_ms` / `set_drift_ppm`),
/// so fault injectors holding a handle immediately affect the server/client using the clock.
#[derive(Clone, Debug)]
pub struct TurmoilClock {
    epoch_base_ms: u64,
    shared_elapsed_ms: Arc<AtomicU64>,
    adjust: Arc<Mutex<ClockAdjust>>,
}

impl TurmoilClock {
    pub fn new(shared_elapsed_ms: Arc<AtomicU64>) -> Self {
        Self::with_skew(shared_elapsed_ms, 0)
    }

    pub fn with_skew(shared_elapsed_ms: Arc<AtomicU64>, skew_ms: i64) -> Self {
        let anchor = turmoil::sim_elapsed()
            .map(|d| d.as_millis() as u64)
            .unwrap_or_else(|| shared_elapsed_ms.load(Ordering::SeqCst));
        Self {
            epoch_base_ms: SIM_EPOCH_BASE_MS,
            shared_elapsed_ms,
            adjust: Arc::new(Mutex::new(ClockAdjust {
                initial_skew_ms: skew_ms,
                offset_ms: skew_ms,
                drift_ppm: 0,
                drift_anchor_ms: anchor,
            })),
        }
    }

    pub fn elapsed_ms(&self) -> u64 {
        turmoil::sim_elapsed()
            .map(|d| d.as_millis() as u64)
            .unwrap_or_else(|| self.shared_elapsed_ms.load(Ordering::SeqCst))
    }

    /// Applies a dynamic NTP-style clock jump in milliseconds (positive = forward, negative = backward).
    pub fn jump_ms(&self, delta_ms: i64) {
        let mut adj = self.adjust.lock().unwrap();
        adj.offset_ms = adj.offset_ms.saturating_add(delta_ms);
    }

    /// Configures a deterministic clock drift rate in parts-per-million (`ppm`).
    ///
    /// Any drift accrued prior to this call is banked into the cumulative offset so changing
    /// drift rate never introduces a sudden discontinuity.
    pub fn set_drift_ppm(&self, drift_ppm: i64) {
        let elapsed = self.elapsed_ms();
        let mut adj = self.adjust.lock().unwrap();
        let banked = Self::accrued_drift_ms(&adj, elapsed);
        adj.offset_ms = adj.offset_ms.saturating_add(banked);
        adj.drift_anchor_ms = elapsed;
        adj.drift_ppm = drift_ppm;
    }

    /// Returns the currently configured drift rate in parts-per-million.
    pub fn drift_ppm(&self) -> i64 {
        self.adjust.lock().unwrap().drift_ppm
    }

    /// Returns the current effective skew relative to unskewed virtual time (in ms).
    pub fn effective_skew_ms(&self) -> i64 {
        let elapsed = self.elapsed_ms();
        let adj = self.adjust.lock().unwrap();
        adj.offset_ms
            .saturating_add(Self::accrued_drift_ms(&adj, elapsed))
    }

    /// Sets an exact skew offset (replacing prior jumps and banked drift) and re-anchors drift.
    pub fn set_skew_ms(&self, skew_ms: i64) {
        let elapsed = self.elapsed_ms();
        let mut adj = self.adjust.lock().unwrap();
        adj.offset_ms = skew_ms;
        adj.drift_anchor_ms = elapsed;
    }

    /// Resets dynamic jumps and drift back to the clock's initial boot skew.
    pub fn reset_adjustments(&self) {
        let elapsed = self.elapsed_ms();
        let mut adj = self.adjust.lock().unwrap();
        adj.offset_ms = adj.initial_skew_ms;
        adj.drift_ppm = 0;
        adj.drift_anchor_ms = elapsed;
    }

    fn accrued_drift_ms(adj: &ClockAdjust, elapsed: u64) -> i64 {
        if adj.drift_ppm == 0 {
            return 0;
        }
        let delta = i128::from(elapsed) - i128::from(adj.drift_anchor_ms);
        let drifted = (delta * i128::from(adj.drift_ppm)) / 1_000_000;
        drifted.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
    }
}

impl Clock for TurmoilClock {
    fn now_ms(&self) -> u64 {
        let elapsed = self.elapsed_ms();
        let total_offset = {
            let adj = self.adjust.lock().unwrap();
            i128::from(adj.offset_ms) + i128::from(Self::accrued_drift_ms(&adj, elapsed))
        };
        let now = i128::from(self.epoch_base_ms) + i128::from(elapsed) + total_offset;
        now.clamp(0, i128::from(u64::MAX)) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_skew_and_dynamic_jumps_are_shared_across_clones() {
        let elapsed = Arc::new(AtomicU64::new(10_000));
        let clock = TurmoilClock::with_skew(Arc::clone(&elapsed), -500);
        let cloned = clock.clone();

        assert_eq!(clock.now_ms(), SIM_EPOCH_BASE_MS + 9_500);
        assert_eq!(cloned.now_ms(), SIM_EPOCH_BASE_MS + 9_500);

        // Forward NTP step
        clock.jump_ms(2_000);
        assert_eq!(clock.now_ms(), SIM_EPOCH_BASE_MS + 11_500);
        assert_eq!(cloned.now_ms(), SIM_EPOCH_BASE_MS + 11_500);
        assert_eq!(clock.effective_skew_ms(), 1_500);

        // Backward NTP step
        clock.jump_ms(-5_000);
        assert_eq!(clock.now_ms(), SIM_EPOCH_BASE_MS + 6_500);
        assert_eq!(cloned.effective_skew_ms(), -3_500);

        // Reset restores initial skew
        clock.reset_adjustments();
        assert_eq!(clock.now_ms(), SIM_EPOCH_BASE_MS + 9_500);
        assert_eq!(clock.drift_ppm(), 0);
    }

    #[test]
    fn drift_accrues_linearly_and_changes_continuously() {
        let elapsed = Arc::new(AtomicU64::new(0));
        let clock = TurmoilClock::new(Arc::clone(&elapsed));

        // +10_000 ppm = +1% fast (+10ms per 1_000ms virtual time)
        clock.set_drift_ppm(10_000);
        elapsed.store(100_000, Ordering::SeqCst);
        assert_eq!(clock.now_ms(), SIM_EPOCH_BASE_MS + 101_000);
        assert_eq!(clock.effective_skew_ms(), 1_000);

        // Changing drift mid-run banks accrued +1_000ms with no sudden jump.
        clock.set_drift_ppm(-5_000);
        assert_eq!(clock.now_ms(), SIM_EPOCH_BASE_MS + 101_000);

        // Advance another 100_000ms at -0.5% -> loses 500ms of skew (net +500ms).
        elapsed.store(200_000, Ordering::SeqCst);
        assert_eq!(clock.now_ms(), SIM_EPOCH_BASE_MS + 200_500);
        assert_eq!(clock.effective_skew_ms(), 500);
    }

    #[test]
    fn extreme_negative_jump_clamps_to_zero() {
        let elapsed = Arc::new(AtomicU64::new(0));
        let clock = TurmoilClock::new(elapsed);
        clock.jump_ms(i64::MIN);
        assert_eq!(clock.now_ms(), 0);
    }
}
