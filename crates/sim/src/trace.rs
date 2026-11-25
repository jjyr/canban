//! Deterministic event/action trace buffer, FNV-1a digest, and property coverage telemetry.
use anyhow::{Result, bail};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
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

/// Property-coverage (`sometimes`) counter tracker to detect silent test rot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoverageTracker {
    counts: BTreeMap<String, u64>,
}

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

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    pub fn merge(&mut self, other: &CoverageTracker) {
        for (k, &v) in &other.counts {
            *self.counts.entry(k.clone()).or_insert(0) += v;
        }
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
        let mut extra = CoverageTracker::new();
        extra.hit("test.beta");
        merge_into_thread_coverage(&extra);

        let taken = take_thread_coverage();
        assert_eq!(taken.count("test.alpha"), 2);
        assert_eq!(taken.count("test.beta"), 1);
        assert!(take_thread_coverage().is_empty());
    }
}
