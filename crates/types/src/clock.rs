//! Scoped wall-clock abstraction for deterministic simulation testing (DST).
//!
//! Production code defaults to [`SystemClock`] (delegating to [`crate::now`]).
//! Simulation code injects a deterministic clock (e.g., backed by `turmoil::sim_elapsed()`)
//! without introducing any global mutable clock state.
use std::sync::Arc;

/// Scoped wall-clock provider returning epoch milliseconds.
pub trait Clock: Send + Sync + 'static {
    /// Current wall-clock timestamp in Unix epoch milliseconds.
    fn now_ms(&self) -> u64;
}

/// Default production wall clock backed by `std::time::SystemTime`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        crate::now()
    }
}

/// Convenience constructor for the default shared production clock.
pub fn system_clock() -> Arc<dyn Clock> {
    Arc::new(SystemClock)
}
