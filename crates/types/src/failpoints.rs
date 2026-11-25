//! Per-instance deterministic failpoint injection for crash-window testing (DST).
//!
//! Production runtimes pass `None` and pay zero mutex or allocation overhead via
//! [`Failpoints::check_opt`]. Simulation and unit tests inject an `Arc<Failpoints>`
//! scoped to a single server/client instance (never process-global statics) so
//! parallel `canban-sim random --jobs N` runs remain 100% isolated.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex},
};

/// Crash window #1: right after Herdr spawns a PTY (`terminal.run`) and before
/// committing the terminal session / run binding to SQLite.
pub const FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB: &str = "herdr.after_terminal_run_before_db_commit";

/// Crash window #2: right after Herdr delivers input (`terminal.send`) to a PTY
/// and before committing `"delivered"` state to SQLite.
pub const FP_HERDR_AFTER_SEND_BEFORE_DB: &str = "herdr.after_send_before_db_commit";

/// Crash window #3: right after GitHub PR creation or PR merge succeeds remotely
/// and before persisting PR / merge state to SQLite.
pub const FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB: &str = "github.after_pr_write_before_db_commit";

/// Crash window #4: right after `git push` succeeds on a merge batch and before
/// finalizing batch state / advancing Cards in SQLite.
pub const FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE: &str = "git.after_push_before_batch_finalize";

/// Crash window #5: mid-write torn partial truncation of the client mutation outbox
/// JSON file before returning an `io::Error`.
pub const FP_OUTBOX_MID_WRITE_TRUNCATE: &str = "outbox.mid_write_truncate";

/// Complete static catalog of production dual-write crash-window failpoint names.
pub const ALL_FAILPOINTS: &[&str] = &[
    FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB,
    FP_HERDR_AFTER_SEND_BEFORE_DB,
    FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB,
    FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE,
    FP_OUTBOX_MID_WRITE_TRUNCATE,
];

/// Action executed when an armed failpoint is reached.
#[derive(Clone)]
pub enum FailpointAction {
    /// Fail once with the provided message, then automatically disarm.
    FailOnce(String),
    /// Fail the next `remaining` hits with `message`, then automatically disarm.
    FailN { remaining: usize, message: String },
    /// Fail every hit with the provided message until disarmed or cleared.
    FailAlways(String),
    /// Execute a custom callback outside the registry lock; returning `Err(msg)` counts as fired.
    Callback(Arc<dyn Fn() -> Result<(), String> + Send + Sync>),
}

impl fmt::Debug for FailpointAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FailOnce(msg) => f.debug_tuple("FailOnce").field(msg).finish(),
            Self::FailN { remaining, message } => f
                .debug_struct("FailN")
                .field("remaining", remaining)
                .field("message", message)
                .finish(),
            Self::FailAlways(msg) => f.debug_tuple("FailAlways").field(msg).finish(),
            Self::Callback(_) => f.write_str("Callback(<fn>)"),
        }
    }
}

/// Per-failpoint telemetry counters recorded by [`Failpoints`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailpointStats {
    /// Total times production code evaluated this failpoint (whether armed or not).
    pub reached: u64,
    /// Total times this failpoint was armed via [`Failpoints::arm`].
    pub armed: u64,
    /// Total times this failpoint returned `Err(...)`.
    pub fired: u64,
}

/// Event emitted to an optional [`FailpointObserver`] hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailpointEvent {
    Armed,
    Reached,
    Fired,
}

/// Observer callback invoked outside the internal lock whenever a failpoint is armed,
/// reached, or fired (allowing simulation harnesses to mirror counts into coverage trackers).
pub type FailpointObserver = Arc<dyn Fn(&str, FailpointEvent) + Send + Sync>;

#[derive(Default)]
struct FailpointsInner {
    armed: BTreeMap<String, FailpointAction>,
    stats: BTreeMap<String, FailpointStats>,
    observer: Option<FailpointObserver>,
}

/// Thread-safe, per-instance failpoint registry (`Clone` shares underlying state via `Arc`).
#[derive(Clone, Default)]
pub struct Failpoints {
    inner: Arc<Mutex<FailpointsInner>>,
}

impl fmt::Debug for Failpoints {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guard = self.inner.lock().unwrap();
        let armed_keys: Vec<&String> = guard.armed.keys().collect();
        f.debug_struct("Failpoints")
            .field("armed", &armed_keys)
            .field("stats", &guard.stats)
            .finish()
    }
}

impl Failpoints {
    /// Creates an empty per-instance failpoint registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Convenience constructor returning a shared `Arc<Failpoints>`.
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Installs or clears an observer hook invoked (outside the lock) on arm/reach/fire events.
    pub fn set_observer(&self, observer: Option<FailpointObserver>) {
        self.inner.lock().unwrap().observer = observer;
    }

    /// Arms failpoint `name` with `action`, replacing any previously armed action for `name`.
    pub fn arm(&self, name: impl Into<String>, action: FailpointAction) {
        let name = name.into();
        let observer = {
            let mut guard = self.inner.lock().unwrap();
            guard.armed.insert(name.clone(), action);
            guard.stats.entry(name.clone()).or_default().armed += 1;
            guard.observer.clone()
        };
        if let Some(obs) = observer {
            obs(&name, FailpointEvent::Armed);
        }
    }

    /// Disarms failpoint `name`, returning `true` if it was armed.
    pub fn disarm(&self, name: &str) -> bool {
        self.inner.lock().unwrap().armed.remove(name).is_some()
    }

    /// Disarms all currently armed failpoints while preserving accumulated telemetry stats.
    pub fn clear(&self) {
        self.inner.lock().unwrap().armed.clear();
    }

    /// Returns `true` if `name` is currently armed.
    pub fn is_armed(&self, name: &str) -> bool {
        self.inner.lock().unwrap().armed.contains_key(name)
    }

    /// Evaluates failpoint `name`, updating `reached` and (if triggered) `fired` counters.
    pub fn check(&self, name: &str) -> Result<(), String> {
        enum Outcome {
            Pass,
            Fail(String),
            Invoke(Arc<dyn Fn() -> Result<(), String> + Send + Sync>),
        }

        let (outcome, observer) = {
            let mut guard = self.inner.lock().unwrap();
            guard.stats.entry(name.to_owned()).or_default().reached += 1;
            let observer = guard.observer.clone();
            let outcome = match guard.armed.get_mut(name) {
                None => Outcome::Pass,
                Some(FailpointAction::FailOnce(_)) => {
                    let Some(FailpointAction::FailOnce(msg)) = guard.armed.remove(name) else {
                        unreachable!()
                    };
                    guard.stats.entry(name.to_owned()).or_default().fired += 1;
                    Outcome::Fail(msg)
                }
                Some(FailpointAction::FailN { remaining, message }) => {
                    if *remaining == 0 {
                        guard.armed.remove(name);
                        Outcome::Pass
                    } else {
                        *remaining -= 1;
                        let msg = message.clone();
                        if *remaining == 0 {
                            guard.armed.remove(name);
                        }
                        guard.stats.entry(name.to_owned()).or_default().fired += 1;
                        Outcome::Fail(msg)
                    }
                }
                Some(FailpointAction::FailAlways(msg)) => {
                    let msg = msg.clone();
                    guard.stats.entry(name.to_owned()).or_default().fired += 1;
                    Outcome::Fail(msg)
                }
                Some(FailpointAction::Callback(cb)) => Outcome::Invoke(Arc::clone(cb)),
            };
            (outcome, observer)
        };

        if let Some(obs) = &observer {
            obs(name, FailpointEvent::Reached);
        }

        match outcome {
            Outcome::Pass => Ok(()),
            Outcome::Fail(msg) => {
                if let Some(obs) = &observer {
                    obs(name, FailpointEvent::Fired);
                }
                Err(msg)
            }
            Outcome::Invoke(cb) => {
                let res = cb();
                if res.is_err() {
                    self.inner
                        .lock()
                        .unwrap()
                        .stats
                        .entry(name.to_owned())
                        .or_default()
                        .fired += 1;
                    if let Some(obs) = &observer {
                        obs(name, FailpointEvent::Fired);
                    }
                }
                res
            }
        }
    }

    /// Zero-overhead helper for production call sites: `None` returns `Ok(())` without locking.
    #[inline]
    pub fn check_opt(fp: Option<&Arc<Self>>, name: &str) -> Result<(), String> {
        match fp {
            Some(fp) => fp.check(name),
            None => Ok(()),
        }
    }

    /// Returns a snapshot of accumulated per-failpoint telemetry counters.
    pub fn stats(&self) -> BTreeMap<String, FailpointStats> {
        self.inner.lock().unwrap().stats.clone()
    }

    /// Returns telemetry counters for a single failpoint name (zeroed if never seen).
    pub fn stats_for(&self, name: &str) -> FailpointStats {
        self.inner
            .lock()
            .unwrap()
            .stats
            .get(name)
            .copied()
            .unwrap_or_default()
    }

    /// Resets accumulated telemetry counters without changing armed actions.
    pub fn reset_stats(&self) {
        self.inner.lock().unwrap().stats.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn fail_once_fail_n_fail_always_disarm_and_clear() {
        let fp = Failpoints::shared();

        // Fast None path never touches any registry.
        assert_eq!(
            Failpoints::check_opt(None, FP_OUTBOX_MID_WRITE_TRUNCATE),
            Ok(())
        );

        // Unarmed check increments reached only.
        assert_eq!(
            Failpoints::check_opt(Some(&fp), FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB),
            Ok(())
        );
        assert_eq!(
            fp.stats_for(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB),
            FailpointStats {
                reached: 1,
                armed: 0,
                fired: 0,
            }
        );

        // FailOnce fires exactly once and auto-disarms.
        fp.arm(
            FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB,
            FailpointAction::FailOnce("boom-once".into()),
        );
        assert!(fp.is_armed(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB));
        assert_eq!(
            fp.check(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB),
            Err("boom-once".into())
        );
        assert!(!fp.is_armed(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB));
        assert_eq!(fp.check(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB), Ok(()));
        assert_eq!(
            fp.stats_for(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB),
            FailpointStats {
                reached: 3,
                armed: 1,
                fired: 1,
            }
        );

        // FailN fires N times and auto-disarms.
        fp.arm(
            FP_HERDR_AFTER_SEND_BEFORE_DB,
            FailpointAction::FailN {
                remaining: 2,
                message: "boom-n".into(),
            },
        );
        assert_eq!(
            fp.check(FP_HERDR_AFTER_SEND_BEFORE_DB),
            Err("boom-n".into())
        );
        assert!(fp.is_armed(FP_HERDR_AFTER_SEND_BEFORE_DB));
        assert_eq!(
            fp.check(FP_HERDR_AFTER_SEND_BEFORE_DB),
            Err("boom-n".into())
        );
        assert!(!fp.is_armed(FP_HERDR_AFTER_SEND_BEFORE_DB));
        assert_eq!(fp.check(FP_HERDR_AFTER_SEND_BEFORE_DB), Ok(()));
        assert_eq!(
            fp.stats_for(FP_HERDR_AFTER_SEND_BEFORE_DB),
            FailpointStats {
                reached: 3,
                armed: 1,
                fired: 2,
            }
        );

        // FailN with remaining == 0 passes and disarms immediately.
        fp.arm(
            FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB,
            FailpointAction::FailN {
                remaining: 0,
                message: "never".into(),
            },
        );
        assert_eq!(fp.check(FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB), Ok(()));
        assert!(!fp.is_armed(FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB));

        // FailAlways fires until disarmed or cleared.
        fp.arm(
            FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE,
            FailpointAction::FailAlways("always".into()),
        );
        assert_eq!(
            fp.check(FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE),
            Err("always".into())
        );
        assert_eq!(
            fp.check(FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE),
            Err("always".into())
        );
        assert!(fp.disarm(FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE));
        assert!(!fp.disarm(FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE));
        assert_eq!(fp.check(FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE), Ok(()));

        // clear() disarms everything while preserving telemetry.
        fp.arm(
            FP_OUTBOX_MID_WRITE_TRUNCATE,
            FailpointAction::FailAlways("trunc".into()),
        );
        fp.clear();
        assert!(!fp.is_armed(FP_OUTBOX_MID_WRITE_TRUNCATE));
        assert_eq!(fp.stats_for(FP_OUTBOX_MID_WRITE_TRUNCATE).armed, 1);
        assert_eq!(fp.check(FP_OUTBOX_MID_WRITE_TRUNCATE), Ok(()));
    }

    #[test]
    fn callback_runs_outside_lock_and_observer_receives_events() {
        let fp = Failpoints::shared();
        let events = Arc::new(Mutex::new(Vec::<(String, FailpointEvent)>::new()));
        let events_obs = Arc::clone(&events);
        fp.set_observer(Some(Arc::new(move |name, ev| {
            events_obs.lock().unwrap().push((name.to_owned(), ev));
        })));

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_cb = Arc::clone(&calls);
        let fp_cb = Arc::clone(&fp);
        fp.arm(
            FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB,
            FailpointAction::Callback(Arc::new(move || {
                let n = calls_cb.fetch_add(1, Ordering::SeqCst);
                // Re-entering the registry from inside Callback must not deadlock.
                let _ = fp_cb.stats();
                if n == 0 {
                    Err("first-cb-fail".into())
                } else {
                    fp_cb.disarm(FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB);
                    Ok(())
                }
            })),
        );

        assert_eq!(
            fp.check(FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB),
            Err("first-cb-fail".into())
        );
        assert_eq!(fp.check(FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB), Ok(()));
        assert!(!fp.is_armed(FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB));

        let st = fp.stats_for(FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB);
        assert_eq!(
            st,
            FailpointStats {
                reached: 2,
                armed: 1,
                fired: 1,
            }
        );
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                (
                    FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB.into(),
                    FailpointEvent::Armed
                ),
                (
                    FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB.into(),
                    FailpointEvent::Reached
                ),
                (
                    FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB.into(),
                    FailpointEvent::Fired
                ),
                (
                    FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB.into(),
                    FailpointEvent::Reached
                ),
            ]
        );
    }

    #[test]
    fn separate_instances_are_isolated_across_threads() {
        let handles: Vec<_> = (0..4)
            .map(|idx| {
                std::thread::spawn(move || {
                    let fp = Failpoints::shared();
                    if idx % 2 == 0 {
                        fp.arm(
                            FP_OUTBOX_MID_WRITE_TRUNCATE,
                            FailpointAction::FailOnce(format!("worker-{idx}")),
                        );
                    }
                    let first = fp.check(FP_OUTBOX_MID_WRITE_TRUNCATE);
                    let second = fp.check(FP_OUTBOX_MID_WRITE_TRUNCATE);
                    (idx, first, second, fp.stats())
                })
            })
            .collect();

        for h in handles {
            let (idx, first, second, stats) = h.join().unwrap();
            if idx % 2 == 0 {
                assert_eq!(first, Err(format!("worker-{idx}")));
                assert_eq!(second, Ok(()));
                assert_eq!(stats[FP_OUTBOX_MID_WRITE_TRUNCATE].fired, 1);
            } else {
                assert_eq!(first, Ok(()));
                assert_eq!(second, Ok(()));
                assert_eq!(stats[FP_OUTBOX_MID_WRITE_TRUNCATE].fired, 0);
            }
        }
    }
}
