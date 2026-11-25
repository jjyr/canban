//! Deterministic event/action trace buffer and FNV-1a digest for replay and self-testing.
use std::collections::VecDeque;

const MAX_TRACE_ENTRIES: usize = 512;

#[derive(Clone, Debug)]
pub struct TraceEntry {
    pub step: u64,
    pub virtual_ms: u64,
    pub category: &'static str,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct EventTrace {
    seed: u64,
    scenario: String,
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
            entries: VecDeque::with_capacity(MAX_TRACE_ENTRIES),
            digest: 0xcbf2_9ce4_8422_2325,
            total_events: 0,
        };
        t.mix_str(&t.scenario.clone());
        t
    }

    pub fn record(
        &mut self,
        step: u64,
        virtual_ms: u64,
        category: &'static str,
        detail: impl Into<String>,
    ) {
        let detail = detail.into();
        self.total_events += 1;
        self.mix_u64(step);
        self.mix_u64(virtual_ms);
        self.mix_str(category);
        self.mix_str(&detail);
        if self.entries.len() == MAX_TRACE_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(TraceEntry {
            step,
            virtual_ms,
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

    pub fn format_failure(&self, reason: &str) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "=== CANBAN DST FAILURE ===\nScenario: {}\nSeed: {}\nDigest: {:#018x}\nReason: {}\nReplay: mise run sim -- --seed {} --scenario {}\n--- Recent Trace ({} total events) ---\n",
            self.scenario,
            self.seed,
            self.digest,
            reason,
            self.seed,
            self.scenario,
            self.total_events
        ));
        for e in &self.entries {
            out.push_str(&format!(
                "[step={:05} t={:07}ms] {:<10} {}\n",
                e.step, e.virtual_ms, e.category, e.detail
            ));
        }
        out
    }
}
