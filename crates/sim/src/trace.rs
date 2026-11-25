//! Deterministic event/action trace buffer, FNV-1a digest, and property coverage telemetry.
use anyhow::{Result, bail};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Mutex,
};

const MAX_TRACE_ENTRIES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceEntry {
    pub step: u64,
    pub virtual_ms: u64,
    pub actor: String,
    pub category: &'static str,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct EventTrace {
    seed: u64,
    scenario: String,
    ops: Option<usize>,
    entries: VecDeque<TraceEntry>,
    digest: u64,
    total_events: u64,
}

impl EventTrace {
    pub fn new(seed: u64, scenario: impl Into<String>) -> Self {
        let scenario = scenario.into();
        let mut t = Self {
            seed,
            scenario,
            ops: None,
            entries: VecDeque::with_capacity(MAX_TRACE_ENTRIES),
            digest: 0xcbf2_9ce4_8422_2325,
            total_events: 0,
        };
        t.mix_str(&t.scenario.clone());
        t
    }

    pub fn with_ops(mut self, ops: usize) -> Self {
        self.ops = Some(ops);
        self
    }

    pub fn set_ops(&mut self, ops: usize) {
        self.ops = Some(ops);
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn scenario(&self) -> &str {
        &self.scenario
    }

    pub fn ops(&self) -> Option<usize> {
        self.ops
    }

    pub fn entries(&self) -> &VecDeque<TraceEntry> {
        &self.entries
    }

    pub fn record(
        &mut self,
        step: u64,
        virtual_ms: u64,
        category: &'static str,
        detail: impl Into<String>,
    ) {
        self.record_actor(step, virtual_ms, "", category, detail);
    }

    pub fn record_actor(
        &mut self,
        step: u64,
        virtual_ms: u64,
        actor: impl Into<String>,
        category: &'static str,
        detail: impl Into<String>,
    ) {
        let actor = actor.into();
        let detail = detail.into();
        self.total_events += 1;
        self.mix_u64(step);
        self.mix_u64(virtual_ms);
        if !actor.is_empty() {
            self.mix_str(&actor);
        }
        self.mix_str(category);
        self.mix_str(&detail);
        if self.entries.len() == MAX_TRACE_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(TraceEntry {
            step,
            virtual_ms,
            actor,
            category,
            detail,
        });
    }

    pub fn mix_u64(&mut self, value: u64) {
        for b in value.to_le_bytes() {
            self.digest ^= u64::from(b);
            self.digest = self.digest.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub fn mix_str(&mut self, value: &str) {
        for b in value.as_bytes() {
            self.digest ^= u64::from(*b);
            self.digest = self.digest.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.mix_u64(0xff);
    }

    pub fn digest(&self) -> u64 {
        self.digest
    }

    pub fn total_events(&self) -> u64 {
        self.total_events
    }

    pub fn replay_command(&self) -> String {
        match self.ops {
            Some(ops) if self.scenario == "chaos" => format!(
                "mise run sim -- --seed {} --scenario {} --ops {}",
                self.seed, self.scenario, ops
            ),
            Some(ops) => format!(
                "mise run sim -- --seed {} --scenario {} --ops {}",
                self.seed, self.scenario, ops
            ),
            None => format!(
                "mise run sim -- --seed {} --scenario {}",
                self.seed, self.scenario
            ),
        }
    }

    pub fn format_failure(&self, reason: &str) -> String {
        self.format_failure_with_state(reason, None)
    }

    pub fn format_failure_with_state(&self, reason: &str, state_summary: Option<&str>) -> String {
        let mut out = String::new();
        let ops_line = self.ops.map(|o| format!("Ops: {o}\n")).unwrap_or_default();
        out.push_str(&format!(
            "=== CANBAN DST FAILURE ===\nScenario: {}\nSeed: {}\n{}Digest: {:#018x}\nReason: {}\nReplay: {}\n",
            self.scenario,
            self.seed,
            ops_line,
            self.digest,
            reason,
            self.replay_command(),
        ));
        if let Some(summary) = state_summary
            && !summary.trim().is_empty()
        {
            out.push_str("--- State Summary ---\n");
            out.push_str(summary.trim_end());
            out.push('\n');
        }
        out.push_str(&format!(
            "--- Recent Trace ({} shown of {} total events) ---\n",
            self.entries.len(),
            self.total_events
        ));
        for e in &self.entries {
            if e.actor.is_empty() {
                out.push_str(&format!(
                    "[step={:05} t={:07}ms] {:<12} {}\n",
                    e.step, e.virtual_ms, e.category, e.detail
                ));
            } else {
                out.push_str(&format!(
                    "[step={:05} t={:07}ms] [{:<12}] {:<12} {}\n",
                    e.step, e.virtual_ms, e.actor, e.category, e.detail
                ));
            }
        }
        out
    }

    /// Serializes the buffered trace entries as newline-delimited JSON for post-mortem artifacts.
    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        for e in &self.entries {
            let line = serde_json::json!({
                "step": e.step,
                "virtual_ms": e.virtual_ms,
                "actor": e.actor,
                "category": e.category,
                "detail": e.detail,
            });
            out.push_str(&line.to_string());
            out.push('\n');
        }
        out
    }
}

/// Per-fault Armed vs Fired telemetry row for detecting vacuous / dead fault injections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaultTelemetryRow {
    pub name: String,
    pub armed: u64,
    pub fired: u64,
    /// `true` when `armed > 0 && fired == 0` (the fault was armed but never actually exercised).
    pub vacuous: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FaultTelemetry {
    armed: BTreeMap<String, u64>,
    fired: BTreeMap<String, u64>,
}

impl FaultTelemetry {
    fn is_empty(&self) -> bool {
        self.armed.is_empty() && self.fired.is_empty()
    }

    fn merge(&mut self, other: &FaultTelemetry) {
        for (k, &v) in &other.armed {
            *self.armed.entry(k.clone()).or_insert(0) += v;
        }
        for (k, &v) in &other.fired {
            *self.fired.entry(k.clone()).or_insert(0) += v;
        }
    }

    fn report(&self) -> Vec<FaultTelemetryRow> {
        let mut names = BTreeSet::new();
        names.extend(self.armed.keys().cloned());
        names.extend(self.fired.keys().cloned());
        names
            .into_iter()
            .map(|name| {
                let armed = self.armed.get(&name).copied().unwrap_or(0);
                let fired = self.fired.get(&name).copied().unwrap_or(0);
                FaultTelemetryRow {
                    name,
                    armed,
                    fired,
                    vacuous: armed > 0 && fired == 0,
                }
            })
            .collect()
    }

    fn vacuous(&self) -> Vec<String> {
        self.report()
            .into_iter()
            .filter(|row| row.vacuous)
            .map(|row| row.name)
            .collect()
    }
}

/// Snapshot / aggregated summary of property-coverage counts and Armed vs Fired fault telemetry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoverageSummary {
    pub counts: BTreeMap<String, u64>,
    pub fault_armed: BTreeMap<String, u64>,
    pub fault_fired: BTreeMap<String, u64>,
}

impl CoverageSummary {
    pub fn merge(&mut self, other: &CoverageSummary) {
        for (k, &v) in &other.counts {
            *self.counts.entry(k.clone()).or_insert(0) += v;
        }
        for (k, &v) in &other.fault_armed {
            *self.fault_armed.entry(k.clone()).or_insert(0) += v;
        }
        for (k, &v) in &other.fault_fired {
            *self.fault_fired.entry(k.clone()).or_insert(0) += v;
        }
    }

    pub fn armed_vs_fired_report(&self) -> Vec<FaultTelemetryRow> {
        FaultTelemetry {
            armed: self.fault_armed.clone(),
            fired: self.fault_fired.clone(),
        }
        .report()
    }

    pub fn vacuous_armed_faults(&self) -> Vec<String> {
        self.armed_vs_fired_report()
            .into_iter()
            .filter(|row| row.vacuous)
            .map(|row| row.name)
            .collect()
    }

    pub fn cold_properties(&self, limit: usize) -> Vec<(String, u64)> {
        cold_properties_from_map(&self.counts, limit)
    }
}

fn cold_properties_from_map(counts: &BTreeMap<String, u64>, limit: usize) -> Vec<(String, u64)> {
    if limit == 0 {
        return Vec::new();
    }
    let mut rows: Vec<(String, u64)> = counts.iter().map(|(k, &v)| (k.clone(), v)).collect();
    rows.sort_by(|(name_a, count_a), (name_b, count_b)| {
        count_a.cmp(count_b).then_with(|| name_a.cmp(name_b))
    });
    rows.truncate(limit);
    rows
}

/// Property-coverage (`sometimes`) and Armed vs Fired fault telemetry tracker to detect silent
/// test rot and vacuous fault injection.
#[derive(Default)]
pub struct CoverageTracker {
    counts: BTreeMap<String, u64>,
    faults: Mutex<FaultTelemetry>,
}

impl Clone for CoverageTracker {
    fn clone(&self) -> Self {
        Self {
            counts: self.counts.clone(),
            faults: Mutex::new(self.faults.lock().unwrap().clone()),
        }
    }
}

impl std::fmt::Debug for CoverageTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let faults = self.faults.lock().unwrap();
        f.debug_struct("CoverageTracker")
            .field("counts", &self.counts)
            .field("fault_armed", &faults.armed)
            .field("fault_fired", &faults.fired)
            .finish()
    }
}

impl PartialEq for CoverageTracker {
    fn eq(&self, other: &Self) -> bool {
        if std::ptr::eq(self, other) {
            return true;
        }
        if self.counts != other.counts {
            return false;
        }
        let a = self.faults.lock().unwrap().clone();
        let b = other.faults.lock().unwrap().clone();
        a == b
    }
}

impl Eq for CoverageTracker {}

impl CoverageTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn hit(&mut self, property: &str) {
        self.hit_n(property, 1);
    }

    pub fn hit_n(&mut self, property: &str, n: u64) {
        if n == 0 {
            return;
        }
        *self.counts.entry(property.to_owned()).or_insert(0) += n;
    }

    pub fn count(&self, property: &str) -> u64 {
        self.counts.get(property).copied().unwrap_or(0)
    }

    pub fn counts(&self) -> &BTreeMap<String, u64> {
        &self.counts
    }

    /// Records that `fault` was armed once (interior-mutable so callers with `&CoverageTracker`
    /// or `&mut CoverageTracker` can both record fault telemetry).
    pub fn arm_fault(&self, fault: impl Into<String>) {
        self.add_fault_counts(fault, 1, 0);
    }

    /// Records that `fault` fired once.
    pub fn fire_fault(&self, fault: impl Into<String>) {
        self.add_fault_counts(fault, 0, 1);
    }

    /// Adds aggregated `armed` and `fired` counts for `fault` (e.g. from `Failpoints::stats()`).
    pub fn add_fault_counts(&self, fault: impl Into<String>, armed: u64, fired: u64) {
        if armed == 0 && fired == 0 {
            return;
        }
        let name = fault.into();
        let mut st = self.faults.lock().unwrap();
        if armed > 0 {
            *st.armed.entry(name.clone()).or_insert(0) += armed;
        }
        if fired > 0 {
            *st.fired.entry(name).or_insert(0) += fired;
        }
    }

    pub fn fault_armed(&self) -> BTreeMap<String, u64> {
        self.faults.lock().unwrap().armed.clone()
    }

    pub fn fault_fired(&self) -> BTreeMap<String, u64> {
        self.faults.lock().unwrap().fired.clone()
    }

    pub fn fault_armed_count(&self, fault: &str) -> u64 {
        self.faults
            .lock()
            .unwrap()
            .armed
            .get(fault)
            .copied()
            .unwrap_or(0)
    }

    pub fn fault_fired_count(&self, fault: &str) -> u64 {
        self.faults
            .lock()
            .unwrap()
            .fired
            .get(fault)
            .copied()
            .unwrap_or(0)
    }

    /// Returns a deterministic snapshot combining property hits and Armed vs Fired fault maps.
    pub fn to_summary(&self) -> CoverageSummary {
        let faults = self.faults.lock().unwrap();
        CoverageSummary {
            counts: self.counts.clone(),
            fault_armed: faults.armed.clone(),
            fault_fired: faults.fired.clone(),
        }
    }

    /// Returns Armed vs Fired fault rows sorted deterministically by fault name.
    pub fn armed_vs_fired_report(&self) -> Vec<FaultTelemetryRow> {
        self.faults.lock().unwrap().report()
    }

    /// Returns sorted names of all faults with `armed > 0 && fired == 0`.
    pub fn vacuous_armed_faults(&self) -> Vec<String> {
        self.faults.lock().unwrap().vacuous()
    }

    /// Returns up to `limit` lowest hit-count properties sorted by `(count, name)`.
    pub fn cold_properties(&self, limit: usize) -> Vec<(String, u64)> {
        cold_properties_from_map(&self.counts, limit)
    }

    /// Negative (`never!`) gate: fails with all `(property, count)` pairs for keys in `forbidden`
    /// that were hit at least once (`count > 0`), sorted deterministically by property name.
    pub fn assert_never_zero(
        &self,
        forbidden: &[&str],
    ) -> std::result::Result<(), Vec<(String, u64)>> {
        let mut violations: BTreeMap<String, u64> = BTreeMap::new();
        for &prop in forbidden {
            let count = self.count(prop);
            if count > 0 {
                violations.insert(prop.to_owned(), count);
            }
        }
        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations.into_iter().collect())
        }
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty() && self.faults.lock().unwrap().is_empty()
    }

    pub fn merge(&mut self, other: &CoverageTracker) {
        if std::ptr::eq(self, other) {
            return;
        }
        for (k, &v) in &other.counts {
            *self.counts.entry(k.clone()).or_insert(0) += v;
        }
        let other_faults = other.faults.lock().unwrap().clone();
        self.faults.lock().unwrap().merge(&other_faults);
    }

    pub fn summary(&self) -> String {
        if self.counts.is_empty() {
            return "(no coverage events recorded)".into();
        }
        let mut out = String::new();
        for (k, v) in &self.counts {
            out.push_str(&format!("  {k:<40} {v}\n"));
        }
        out
    }

    pub fn assert_required(&self, required: &[&str]) -> Result<()> {
        let missing: Vec<&str> = required
            .iter()
            .copied()
            .filter(|prop| self.count(prop) == 0)
            .collect();
        if !missing.is_empty() {
            bail!(
                "Coverage assertion failed; unhit required properties: [{}]\nRecorded coverage:\n{}",
                missing.join(", "),
                self.summary()
            );
        }
        Ok(())
    }
}

thread_local! {
    static THREAD_COVERAGE: RefCell<CoverageTracker> = RefCell::new(CoverageTracker::new());
}

/// Clears the thread-local coverage collector before running a simulation seed.
pub fn reset_thread_coverage() {
    THREAD_COVERAGE.with(|cell| {
        *cell.borrow_mut() = CoverageTracker::new();
    });
}

/// Records a coverage hit on the current simulation worker thread.
pub fn record_coverage_hit(property: &str) {
    THREAD_COVERAGE.with(|cell| {
        cell.borrow_mut().hit(property);
    });
}

/// Records an armed fault on the current simulation worker thread.
pub fn record_fault_armed(fault: &str) {
    THREAD_COVERAGE.with(|cell| {
        cell.borrow().arm_fault(fault);
    });
}

/// Records a fired fault on the current simulation worker thread.
pub fn record_fault_fired(fault: &str) {
    THREAD_COVERAGE.with(|cell| {
        cell.borrow().fire_fault(fault);
    });
}

/// Adds aggregated `(armed, fired)` fault counts on the current simulation worker thread.
pub fn record_fault_counts(fault: &str, armed: u64, fired: u64) {
    THREAD_COVERAGE.with(|cell| {
        cell.borrow().add_fault_counts(fault, armed, fired);
    });
}

/// Merges a local [`CoverageTracker`] into the current worker thread's collector.
pub fn merge_into_thread_coverage(tracker: &CoverageTracker) {
    THREAD_COVERAGE.with(|cell| {
        cell.borrow_mut().merge(tracker);
    });
}

/// Returns and clears the coverage accumulated on the current worker thread.
pub fn take_thread_coverage() -> CoverageTracker {
    THREAD_COVERAGE.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_trace_structured_entries_and_format_failure_include_ops_and_state() {
        let mut trace = EventTrace::new(42, "chaos").with_ops(75);
        trace.record(1, 50, "client.rpc", "card.create");
        trace.record_actor(2, 100, "remote-a", "fault.part", "ui-client <-> remote-a");

        assert_eq!(trace.total_events(), 2);
        assert_eq!(trace.entries().len(), 2);
        assert_eq!(trace.entries()[1].actor, "remote-a");

        let rendered = trace.format_failure_with_state("outbox stuck", Some("remote-a: 1 queued"));
        assert!(rendered.contains("Scenario: chaos"));
        assert!(rendered.contains("Seed: 42"));
        assert!(rendered.contains("Ops: 75"));
        assert!(rendered.contains("--seed 42 --scenario chaos --ops 75"));
        assert!(rendered.contains("remote-a: 1 queued"));
        assert!(rendered.contains("[remote-a    ]"));

        let jsonl = trace.to_jsonl();
        assert_eq!(jsonl.lines().count(), 2);
        assert!(jsonl.contains("\"actor\":\"remote-a\""));
    }

    #[test]
    fn coverage_tracker_hits_merges_and_asserts_required() {
        let mut a = CoverageTracker::new();
        a.hit("herdr.terminal.run");
        a.hit("herdr.terminal.run");
        a.hit("github.304");

        let mut b = CoverageTracker::new();
        b.hit("github.304");
        b.hit("outbox.conflict");

        a.merge(&b);
        assert_eq!(a.count("herdr.terminal.run"), 2);
        assert_eq!(a.count("github.304"), 2);
        assert_eq!(a.count("outbox.conflict"), 1);
        assert_eq!(a.count("never.hit"), 0);

        a.assert_required(&["herdr.terminal.run", "github.304", "outbox.conflict"])
            .expect("all required properties were hit");
        let err = a
            .assert_required(&["github.304", "never.hit"])
            .expect_err("missing property must fail");
        assert!(err.to_string().contains("never.hit"));
        assert!(a.summary().contains("herdr.terminal.run"));
    }

    #[test]
    fn thread_local_coverage_round_trip() {
        reset_thread_coverage();
        record_coverage_hit("test.alpha");
        record_coverage_hit("test.alpha");
        record_fault_armed("fp.a");
        record_fault_fired("fp.a");
        record_fault_counts("fp.b", 2, 0);
        let mut extra = CoverageTracker::new();
        extra.hit("test.beta");
        extra.add_fault_counts("fp.b", 1, 1);
        merge_into_thread_coverage(&extra);

        let taken = take_thread_coverage();
        assert_eq!(taken.count("test.alpha"), 2);
        assert_eq!(taken.count("test.beta"), 1);
        assert_eq!(taken.fault_armed_count("fp.a"), 1);
        assert_eq!(taken.fault_fired_count("fp.a"), 1);
        assert_eq!(taken.fault_armed_count("fp.b"), 3);
        assert_eq!(taken.fault_fired_count("fp.b"), 1);
        assert!(take_thread_coverage().is_empty());
    }

    #[test]
    fn armed_vs_fired_aggregation_and_vacuous_detection() {
        let mut w1 = CoverageTracker::new();
        w1.arm_fault("zeta.fault");
        w1.arm_fault("alpha.fault");
        w1.fire_fault("alpha.fault");
        w1.add_fault_counts("mid.fault", 3, 2);
        w1.add_fault_counts("noop.fault", 0, 0);
        assert!(!w1.is_empty());

        let w2 = CoverageTracker::new();
        w2.arm_fault("zeta.fault");
        w2.add_fault_counts("mid.fault", 1, 2);
        w2.fire_fault("fired_only.fault");

        w1.merge(&w2);
        let report = w1.armed_vs_fired_report();
        assert_eq!(
            report,
            vec![
                FaultTelemetryRow {
                    name: "alpha.fault".into(),
                    armed: 1,
                    fired: 1,
                    vacuous: false,
                },
                FaultTelemetryRow {
                    name: "fired_only.fault".into(),
                    armed: 0,
                    fired: 1,
                    vacuous: false,
                },
                FaultTelemetryRow {
                    name: "mid.fault".into(),
                    armed: 4,
                    fired: 4,
                    vacuous: false,
                },
                FaultTelemetryRow {
                    name: "zeta.fault".into(),
                    armed: 2,
                    fired: 0,
                    vacuous: true,
                },
            ]
        );
        assert_eq!(w1.vacuous_armed_faults(), vec!["zeta.fault".to_owned()]);

        // CoverageSummary merges identically.
        let mut sum = CoverageSummary::default();
        sum.merge(&w1.to_summary());
        assert_eq!(sum.armed_vs_fired_report(), report);
        assert_eq!(sum.vacuous_armed_faults(), vec!["zeta.fault".to_owned()]);
    }

    #[test]
    fn cold_properties_orders_by_count_then_name() {
        let mut tracker = CoverageTracker::new();
        tracker.hit_n("hot.prop", 50);
        tracker.hit_n("warm.prop", 5);
        tracker.hit_n("cold.b", 1);
        tracker.hit_n("cold.a", 1);
        tracker.hit_n("cool.prop", 2);

        assert_eq!(
            tracker.cold_properties(3),
            vec![
                ("cold.a".into(), 1),
                ("cold.b".into(), 1),
                ("cool.prop".into(), 2),
            ]
        );
        assert!(tracker.cold_properties(0).is_empty());
        assert_eq!(tracker.cold_properties(20).len(), 5);
    }

    #[test]
    fn assert_never_zero_checks_forbidden_properties() {
        let mut tracker = CoverageTracker::new();
        tracker.hit("sim.scenario.passed");

        assert_eq!(
            tracker.assert_never_zero(&["exec.blocked_command", "invariant.corrupt"]),
            Ok(())
        );

        tracker.hit_n("exec.blocked_command", 3);
        tracker.hit("invariant.corrupt");
        let violations = tracker
            .assert_never_zero(&[
                "exec.blocked_command",
                "unused.never",
                "invariant.corrupt",
                "exec.blocked_command",
            ])
            .expect_err("hit forbidden properties must fail");
        assert_eq!(
            violations,
            vec![
                ("exec.blocked_command".into(), 3),
                ("invariant.corrupt".into(), 1),
            ]
        );
    }
}
