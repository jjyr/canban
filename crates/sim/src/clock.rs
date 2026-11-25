//! Deterministic wall clock backed by `turmoil::sim_elapsed()` with optional per-host skew.
use canban_types::Clock;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Fixed deterministic epoch base (`2023-11-14T22:13:20Z` in epoch ms).
pub const SIM_EPOCH_BASE_MS: u64 = 1_700_000_000_000;

/// Deterministic wall clock synchronized with `turmoil`'s virtual simulation clock.
///
/// Inside a Turmoil host/client task, `turmoil::sim_elapsed()` is authoritative.
/// When inspected from outside a host task (for example, by the driver or oracle between
/// `sim.step()` calls), `shared_elapsed_ms` supplies the exact elapsed milliseconds recorded
/// after the latest `sim.step()`.
#[derive(Clone, Debug)]
pub struct TurmoilClock {
    epoch_base_ms: u64,
    skew_ms: i64,
    shared_elapsed_ms: Arc<AtomicU64>,
}

impl TurmoilClock {
    pub fn new(shared_elapsed_ms: Arc<AtomicU64>) -> Self {
        Self::with_skew(shared_elapsed_ms, 0)
    }

    pub fn with_skew(shared_elapsed_ms: Arc<AtomicU64>, skew_ms: i64) -> Self {
        Self {
            epoch_base_ms: SIM_EPOCH_BASE_MS,
            skew_ms,
            shared_elapsed_ms,
        }
    }

    pub fn elapsed_ms(&self) -> u64 {
        turmoil::sim_elapsed()
            .map(|d| d.as_millis() as u64)
            .unwrap_or_else(|| self.shared_elapsed_ms.load(Ordering::SeqCst))
    }
}

impl Clock for TurmoilClock {
    fn now_ms(&self) -> u64 {
        let base = self.epoch_base_ms.saturating_add(self.elapsed_ms());
        if self.skew_ms >= 0 {
            base.saturating_add(self.skew_ms as u64)
        } else {
            base.saturating_sub(self.skew_ms.unsigned_abs())
        }
    }
}
