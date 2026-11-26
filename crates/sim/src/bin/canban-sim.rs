//! Deterministic Simulation Testing CLI runner (`canban-sim`).
use anyhow::{Context, Result, anyhow, bail};
use canban_sim::{
    scenarios::{self, Scenario},
    trace::{self, CoverageTracker},
};
use clap::{Parser, Subcommand};
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::{
    any::Any,
    collections::BTreeMap,
    fs::OpenOptions,
    io::{self, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

static FAILED_SEEDS_LOCK: Mutex<()> = Mutex::new(());
const DEFAULT_CORPUS_FILE: &str = "crates/sim/corpus/regressions.txt";
const AUTO_SHRINK_MAX_ATTEMPTS: usize = 32;
const AUTO_SHRINK_TIME_BUDGET: Duration = Duration::from_secs(30);
const COLD_PROPERTIES_TOP_N: usize = 10;
const NEVER_COVERAGE_PROPERTIES: &[&str] = &["exec.blocked_command"];

#[derive(Parser)]
#[command(
    name = "canban-sim",
    about = "Deterministic Simulation Testing (DST) runner for Canban"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run one complete deterministic simulation round (all 17 scripted scenarios + chaos),
    /// or replay a specific `--seed` / `--scenario` or `--replay-file`.
    Run {
        /// Optional scenario name (e.g. `remote_faults`, `transport_auth`, `workspaces_delivery`, `project_import_export`, `sessions_and_adoption`, `review_and_events`, `scheduler_cross_contention`, `chaos`).
        /// Omit to run one complete round (all 17 scripted scenarios + chaos).
        #[arg(long)]
        scenario: Option<String>,
        /// Deterministic seed (default: 2026). Pass a failing seed to replay it exactly.
        #[arg(long, default_value_t = 2026)]
        seed: u64,
        /// Number of consecutive seeds per scenario (default: 1).
        #[arg(long, default_value_t = 1)]
        seeds: u64,
        /// Operations per chaos run.
        #[arg(long, default_value_t = 40)]
        ops: usize,
        /// When a `chaos` seed fails, automatically run deterministic prefix+ddmin shrinking.
        #[arg(long)]
        shrink: bool,
        /// Optional file of previously recorded failing seeds (e.g. `failed-seeds.txt`) to replay.
        #[arg(long)]
        replay_file: Option<PathBuf>,
        /// Keep running remaining seeds after a failure instead of stopping immediately.
        #[arg(long)]
        continue_on_error: bool,
        /// Optional per-seed wall-clock watchdog timeout in seconds (aborts & logs hung seeds).
        #[arg(long)]
        seed_timeout: Option<u64>,
        /// Print aggregated property coverage and assert required properties were hit.
        #[arg(long)]
        assert_coverage: bool,
        /// Additional required coverage property names to verify (repeatable).
        #[arg(long)]
        require_coverage: Vec<String>,
        /// Directory where failure logs and `failed-seeds.txt` are persisted.
        #[arg(long, default_value = ".local/sim-failures")]
        trace_dir: PathBuf,
        /// Optional path to write a structured JSON summary report (written on both pass and fail).
        #[arg(long)]
        report_json: Option<PathBuf>,
    },
    /// Shrink a failing `chaos` seed down to a minimal reproducing operation sequence.
    Shrink {
        /// Failing chaos seed to minimize.
        #[arg(long)]
        seed: u64,
        /// Original operation budget of the failing chaos run.
        #[arg(long, default_value_t = 40)]
        ops: usize,
    },
    /// Continuously run random-seed simulations until Ctrl-C, printing failing seeds
    /// (pass `--fail-fast` to stop immediately on the first failure).
    Random {
        /// Stop immediately on the first failing seed instead of continuing until Ctrl-C.
        #[arg(long, short = 'x')]
        fail_fast: bool,
        /// Target scenario: `chaos` (default), `all` (random mix of all 17 scripted + chaos), or a specific scenario name.
        #[arg(long, default_value = "chaos")]
        scenario: String,
        /// Optional cap on total seeds (omit to run continuously until Ctrl-C).
        #[arg(long)]
        seeds: Option<u64>,
        /// Optional wall-clock budget in seconds.
        #[arg(long)]
        duration: Option<u64>,
        /// Parallel worker threads (default: 1; e.g. `--jobs 4`).
        #[arg(long, default_value_t = 1)]
        jobs: usize,
        /// Number of random operations per chaos seed.
        #[arg(long, default_value_t = 40)]
        ops: usize,
        /// Optional deterministic master seed controlling the generated `(scenario, seed)` sequence.
        #[arg(long)]
        master_seed: Option<u64>,
        /// Optional per-seed wall-clock watchdog timeout in seconds (aborts & logs hung seeds).
        #[arg(long)]
        seed_timeout: Option<u64>,
        /// Print aggregated property coverage and assert required properties were hit.
        #[arg(long)]
        assert_coverage: bool,
        /// Additional required coverage property names to verify (repeatable).
        #[arg(long)]
        require_coverage: Vec<String>,
        /// Automatically shrink failing `chaos` seeds (with bounded step/time budget) and print
        /// the minimized op sequence + exact repro command.
        #[arg(long)]
        auto_shrink: bool,
        /// Directory where failure logs and `failed-seeds.txt` are persisted.
        #[arg(long, default_value = ".local/sim-failures")]
        trace_dir: PathBuf,
        /// Optional path to write a structured JSON summary report (written on both pass and fail).
        #[arg(long)]
        report_json: Option<PathBuf>,
    },
    /// Replay every regression seed recorded in a corpus file (`<scenario> <seed> [ops]`).
    Corpus {
        /// Path to the corpus file (default: `crates/sim/corpus/regressions.txt`).
        #[arg(long, default_value = DEFAULT_CORPUS_FILE)]
        file: PathBuf,
        /// Parallel worker threads (default: 1; e.g. `--jobs 4`).
        #[arg(long, default_value_t = 1)]
        jobs: usize,
        /// Default operations count when a corpus line omits `[ops]`.
        #[arg(long, default_value_t = 40)]
        ops: usize,
        /// Optional per-seed wall-clock watchdog timeout in seconds (aborts & logs hung seeds).
        #[arg(long)]
        seed_timeout: Option<u64>,
        /// Directory where failure logs and `failed-seeds.txt` are persisted.
        #[arg(long, default_value = ".local/sim-failures")]
        trace_dir: PathBuf,
        /// Optional path to write a structured JSON summary report (written on both pass and fail).
        #[arg(long)]
        report_json: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReplayEntry {
    scenario: Scenario,
    seed: u64,
    ops: usize,
}

#[derive(Clone, Debug)]
struct ActiveSeed {
    scenario: &'static str,
    seed: u64,
    ops: usize,
    started: Instant,
}

type TimeoutHook = Arc<dyn Fn(&ActiveSeed, Duration, &anyhow::Error) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct NamedCount {
    name: String,
    count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArmedVsFiredEntry {
    name: String,
    armed: u64,
    fired: u64,
    vacuous: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CoverageHighlights {
    delivery_batch_entered: u64,
    shell_executed: u64,
    read_your_writes_checked: u64,
    at_most_once_checked: u64,
    committed_durability_checked: u64,
    natural_quiesce_recovered: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CoverageReport {
    distinct_properties: usize,
    required_passed: usize,
    required_total: usize,
    coverage_gate_passed: bool,
    never_gate_passed: bool,
    never_violations: Vec<NamedCount>,
    vacuous_faults: Vec<String>,
    armed_vs_fired: Vec<ArmedVsFiredEntry>,
    cold_top_10: Vec<NamedCount>,
    highlights: CoverageHighlights,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ShrunkReport {
    orig_ops: usize,
    shrunk_ops: usize,
    digest: Option<String>,
    ops_lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FailureReport {
    scenario: String,
    seed: u64,
    ops: usize,
    profile: Option<String>,
    error: String,
    fingerprint: String,
    digest: Option<String>,
    timed_out: bool,
    shrunk: Option<ShrunkReport>,
}

#[derive(Clone, Debug, PartialEq)]
struct SimReport {
    command: &'static str,
    scenario: String,
    master_seed: Option<u64>,
    ops: usize,
    duration_secs: Option<u64>,
    elapsed_secs: f64,
    passed: u64,
    failed: u64,
    timed_out: u64,
    total: u64,
    coverage: CoverageReport,
    failures: Vec<FailureReport>,
}

impl SimReport {
    fn to_json(&self) -> serde_json::Value {
        let never_violations: Vec<serde_json::Value> = self
            .coverage
            .never_violations
            .iter()
            .map(|v| serde_json::json!({ "name": v.name, "count": v.count }))
            .collect();
        let armed_vs_fired: Vec<serde_json::Value> = self
            .coverage
            .armed_vs_fired
            .iter()
            .map(|row| {
                serde_json::json!({
                    "name": row.name,
                    "armed": row.armed,
                    "fired": row.fired,
                    "vacuous": row.vacuous,
                })
            })
            .collect();
        let cold_top_10: Vec<serde_json::Value> = self
            .coverage
            .cold_top_10
            .iter()
            .map(|row| serde_json::json!({ "name": row.name, "count": row.count }))
            .collect();
        let failures: Vec<serde_json::Value> = self
            .failures
            .iter()
            .map(|f| {
                let shrunk = f.shrunk.as_ref().map(|s| {
                    serde_json::json!({
                        "orig_ops": s.orig_ops,
                        "shrunk_ops": s.shrunk_ops,
                        "digest": s.digest,
                        "ops_lines": s.ops_lines,
                    })
                });
                serde_json::json!({
                    "scenario": f.scenario,
                    "seed": f.seed,
                    "ops": f.ops,
                    "profile": f.profile,
                    "error": f.error,
                    "fingerprint": f.fingerprint,
                    "digest": f.digest,
                    "timed_out": f.timed_out,
                    "shrunk": shrunk,
                })
            })
            .collect();
        serde_json::json!({
            "command": self.command,
            "scenario": self.scenario,
            "master_seed": self.master_seed,
            "ops": self.ops,
            "duration_secs": self.duration_secs,
            "elapsed_secs": (self.elapsed_secs * 1000.0).round() / 1000.0,
            "passed": self.passed,
            "failed": self.failed,
            "timed_out": self.timed_out,
            "total": self.total,
            "coverage": {
                "distinct_properties": self.coverage.distinct_properties,
                "required_passed": self.coverage.required_passed,
                "required_total": self.coverage.required_total,
                "coverage_gate_passed": self.coverage.coverage_gate_passed,
                "never_gate_passed": self.coverage.never_gate_passed,
                "never_violations": never_violations,
                "vacuous_faults": self.coverage.vacuous_faults,
                "armed_vs_fired": armed_vs_fired,
                "cold_top_10": cold_top_10,
                "highlights": {
                    "delivery_batch_entered": self.coverage.highlights.delivery_batch_entered,
                    "shell_executed": self.coverage.highlights.shell_executed,
                    "read_your_writes_checked": self.coverage.highlights.read_your_writes_checked,
                    "at_most_once_checked": self.coverage.highlights.at_most_once_checked,
                    "committed_durability_checked": self.coverage.highlights.committed_durability_checked,
                    "natural_quiesce_recovered": self.coverage.highlights.natural_quiesce_recovered,
                },
            },
            "failures": failures,
        })
    }
}

fn extract_digest_from_error(err: &str) -> Option<String> {
    err.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("Digest:")?.trim();
        (!rest.is_empty()).then(|| rest.to_owned())
    })
}

fn build_failure_report(
    scenario: &str,
    seed: u64,
    ops: usize,
    error: String,
    timed_out: bool,
    shrunk: Option<ShrunkReport>,
) -> FailureReport {
    let profile = (scenario == "chaos").then(|| scenarios::chaos::profile_name(seed));
    let fingerprint = scenarios::chaos::error_class_fingerprint(&error);
    let digest = extract_digest_from_error(&error);
    FailureReport {
        scenario: scenario.to_owned(),
        seed,
        ops,
        profile,
        error,
        fingerprint,
        digest,
        timed_out,
        shrunk,
    }
}

fn build_shrunk_report(report: &scenarios::chaos::ShrinkReport) -> ShrunkReport {
    let ops_lines = report
        .ops
        .iter()
        .enumerate()
        .map(|(idx, op)| format!("[{idx:02}] {op:?}"))
        .collect();
    ShrunkReport {
        orig_ops: report.original_len,
        shrunk_ops: report.minimized_len,
        digest: extract_digest_from_error(&report.error),
        ops_lines,
    }
}

fn build_coverage_report(
    tracker: &CoverageTracker,
    assert_coverage: bool,
    extra_required: &[String],
    suite_required: &[&str],
) -> CoverageReport {
    let mut required: Vec<&str> = Vec::new();
    if assert_coverage {
        required.extend_from_slice(BASE_REQUIRED_COVERAGE);
        required.extend_from_slice(suite_required);
    }
    for prop in extra_required {
        required.push(prop.as_str());
    }
    let required_total = required.len();
    let required_passed = required.iter().filter(|p| tracker.count(p) > 0).count();
    let never_violations: Vec<NamedCount> =
        match tracker.assert_never_zero(NEVER_COVERAGE_PROPERTIES) {
            Ok(()) => Vec::new(),
            Err(violations) => violations
                .into_iter()
                .map(|(name, count)| NamedCount { name, count })
                .collect(),
        };
    let never_gate_passed = never_violations.is_empty();
    let vacuous_faults = tracker.vacuous_armed_faults();
    let coverage_gate_passed = if assert_coverage {
        required_passed == required_total && never_gate_passed && vacuous_faults.is_empty()
    } else if !extra_required.is_empty() {
        required_passed == required_total
    } else {
        true
    };
    let armed_vs_fired = tracker
        .armed_vs_fired_report()
        .into_iter()
        .map(|row| ArmedVsFiredEntry {
            name: row.name,
            armed: row.armed,
            fired: row.fired,
            vacuous: row.vacuous,
        })
        .collect();
    let cold_top_10 = tracker
        .cold_properties(COLD_PROPERTIES_TOP_N)
        .into_iter()
        .map(|(name, count)| NamedCount { name, count })
        .collect();
    CoverageReport {
        distinct_properties: tracker.counts().len(),
        required_passed,
        required_total,
        coverage_gate_passed,
        never_gate_passed,
        never_violations,
        vacuous_faults,
        armed_vs_fired,
        cold_top_10,
        highlights: CoverageHighlights {
            delivery_batch_entered: tracker.count("chaos.delivery_batch_entered"),
            shell_executed: tracker.count("exec.shell_executed"),
            read_your_writes_checked: tracker.count("oracle.read_your_writes_checked"),
            at_most_once_checked: tracker.count("oracle.at_most_once_checked"),
            committed_durability_checked: tracker.count("oracle.committed_durability_checked"),
            natural_quiesce_recovered: tracker.count("chaos.natural_quiesce_recovered"),
        },
    }
}

fn write_report_json(path: &Path, report: &SimReport) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create report directory {}", parent.display()))?;
    }
    let payload = serde_json::to_string_pretty(&report.to_json())
        .context("serialize simulation JSON report")?;
    std::fs::write(path, format!("{payload}\n"))
        .with_context(|| format!("write simulation JSON report {}", path.display()))?;
    Ok(())
}

fn persist_optional_report(
    report_json: Option<&Path>,
    report: &SimReport,
    propagate_error: bool,
) -> Result<()> {
    let Some(path) = report_json else {
        return Ok(());
    };
    if let Err(err) = write_report_json(path, report) {
        eprintln!(
            "ERROR: failed to write --report-json {}: {err:#}",
            path.display()
        );
        if propagate_error {
            return Err(err);
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct CorpusFailure {
    index: usize,
    entry: ReplayEntry,
    replay: String,
    report: FailureReport,
}

#[derive(Clone, Debug)]
struct SuiteFailure {
    seed: u64,
    scenario: &'static str,
    ops: usize,
    replay: String,
    report: FailureReport,
}

#[derive(Clone, Debug)]
struct RandomFailure {
    seed: u64,
    scenario: String,
    replay: String,
    report: FailureReport,
}

struct WatchdogHandle {
    active: Arc<Mutex<BTreeMap<usize, ActiveSeed>>>,
    done: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WatchdogHandle {
    fn start(
        timeout_secs: Option<u64>,
        trace_dir: PathBuf,
        on_timeout: Option<TimeoutHook>,
    ) -> Self {
        let active: Arc<Mutex<BTreeMap<usize, ActiveSeed>>> = Arc::new(Mutex::new(BTreeMap::new()));
        let done = Arc::new(AtomicBool::new(false));
        let thread = timeout_secs.filter(|&s| s > 0).map(|secs| {
            let active_ref = Arc::clone(&active);
            let done_ref = Arc::clone(&done);
            let limit = Duration::from_secs(secs);
            std::thread::spawn(move || {
                while !done_ref.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(200));
                    if done_ref.load(Ordering::Relaxed) {
                        break;
                    }
                    let timed_out = {
                        let guard = active_ref.lock().unwrap();
                        guard.iter().find_map(|(&worker_id, item)| {
                            let elapsed = item.started.elapsed();
                            (elapsed >= limit).then(|| (worker_id, item.clone(), elapsed))
                        })
                    };
                    if let Some((worker_id, item, elapsed)) = timed_out {
                        let replay = replay_cmd(item.seed, item.scenario, item.ops);
                        let err = anyhow!(
                            "SEED TIMEOUT: worker w{worker_id} exceeded --seed-timeout {secs}s (elapsed {:.2}s) on scenario={} seed={} ops={}",
                            elapsed.as_secs_f64(),
                            item.scenario,
                            item.seed,
                            item.ops,
                        );
                        let saved =
                            write_failure_log(&trace_dir, item.seed, item.scenario, item.ops, &err);
                        if let Some(ref hook) = on_timeout {
                            hook(&item, elapsed, &err);
                        }
                        eprintln!(
                            "\n=== CANBAN DST SEED TIMEOUT ===\nWorker:   w{worker_id}\nScenario: {}\nSeed:     {}\nOps:      {}\nElapsed:  {:.2}s (limit: {secs}s)\nReplay:   {replay}\n{}",
                            item.scenario,
                            item.seed,
                            item.ops,
                            elapsed.as_secs_f64(),
                            saved
                                .map(|p| format!("Log:      {}\n", p.display()))
                                .unwrap_or_default()
                        );
                        let _ = io::stderr().flush();
                        let _ = io::stdout().flush();
                        std::process::exit(124);
                    }
                }
            })
        });
        Self {
            active,
            done,
            thread,
        }
    }

    fn begin_seed(&self, worker_id: usize, sc: Scenario, seed: u64, ops: usize) {
        self.active.lock().unwrap().insert(
            worker_id,
            ActiveSeed {
                scenario: sc.name(),
                seed,
                ops,
                started: Instant::now(),
            },
        );
    }

    fn end_seed(&self, worker_id: usize) {
        self.active.lock().unwrap().remove(&worker_id);
    }
}

impl Drop for WatchdogHandle {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn replay_cmd(seed: u64, scenario: &str, ops: usize) -> String {
    if scenario == "chaos" {
        format!("mise run sim -- --seed {seed} --scenario chaos --ops {ops}")
    } else {
        format!("mise run sim -- --seed {seed} --scenario {scenario}")
    }
}

fn failure_log_filename(scenario: &str, seed: u64, ops: usize) -> String {
    format!("{scenario}-seed-{seed}-ops-{ops}.log")
}

fn write_failure_log(
    dir: &Path,
    seed: u64,
    scenario: &str,
    ops: usize,
    err: &anyhow::Error,
) -> Option<PathBuf> {
    if std::fs::create_dir_all(dir).is_err() {
        return None;
    }
    let replay = replay_cmd(seed, scenario, ops);
    let path = dir.join(failure_log_filename(scenario, seed, ops));
    let body = format!(
        "=== CANBAN DST FAILURE ===\nScenario: {scenario}\nSeed: {seed}\nOps: {ops}\nReplay: {replay}\n\nError:\n{err:#}\n"
    );
    std::fs::write(&path, body).ok()?;
    let _guard = FAILED_SEEDS_LOCK.lock().unwrap();
    if let Ok(mut index) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("failed-seeds.txt"))
    {
        let _ = writeln!(
            index,
            "seed={seed} scenario={scenario} ops={ops} replay=\"{replay}\""
        );
    }
    Some(path)
}

fn format_panic_payload(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".into()
    }
}

fn execute_seed_caught(sc: Scenario, seed: u64, ops: usize) -> (Result<u64>, CoverageTracker) {
    trace::reset_thread_coverage();
    trace::record_coverage_hit("sim.scenario.started");
    trace::record_coverage_hit(&format!("sim.scenario.{}", sc.name()));
    let outcome = match catch_unwind(AssertUnwindSafe(|| scenarios::run_scenario(sc, seed, ops))) {
        Ok(Ok(digest)) => {
            trace::record_coverage_hit("sim.scenario.passed");
            Ok(digest)
        }
        Ok(Err(e)) => Err(e),
        Err(payload) => Err(anyhow!("PANIC: {}", format_panic_payload(payload.as_ref()))),
    };
    let cov = trace::take_thread_coverage();
    (outcome, cov)
}

fn strip_inline_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn parse_replay_line(line: &str, default_ops: usize) -> Result<Option<ReplayEntry>> {
    let trimmed = strip_inline_comment(line).trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.contains("seed=") || trimmed.contains("scenario=") {
        let mut seed: Option<u64> = None;
        let mut scenario: Option<Scenario> = None;
        let mut ops = default_ops;
        for token in trimmed.split_whitespace() {
            if let Some(val) = token.strip_prefix("seed=") {
                seed = Some(
                    val.parse::<u64>()
                        .with_context(|| format!("invalid seed '{val}'"))?,
                );
            } else if let Some(val) = token.strip_prefix("scenario=") {
                scenario = Some(Scenario::parse(val)?);
            } else if let Some(val) = token.strip_prefix("ops=") {
                ops = val
                    .parse::<usize>()
                    .with_context(|| format!("invalid ops '{val}'"))?;
            }
        }
        let seed = seed.with_context(|| format!("replay line missing seed=: {trimmed}"))?;
        let scenario =
            scenario.with_context(|| format!("replay line missing scenario=: {trimmed}"))?;
        return Ok(Some(ReplayEntry {
            scenario,
            seed,
            ops,
        }));
    }
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() < 2 {
        bail!("invalid replay line (expected `<scenario> <seed> [ops]`): {trimmed}");
    }
    let scenario = Scenario::parse(parts[0])?;
    let seed = parts[1]
        .parse::<u64>()
        .with_context(|| format!("invalid seed '{}'", parts[1]))?;
    let ops = if let Some(ops_str) = parts.get(2) {
        ops_str
            .parse::<usize>()
            .with_context(|| format!("invalid ops '{ops_str}'"))?
    } else {
        default_ops
    };
    Ok(Some(ReplayEntry {
        scenario,
        seed,
        ops,
    }))
}

fn load_replay_file(path: &Path, default_ops: usize) -> Result<Vec<ReplayEntry>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read replay file {}", path.display()))?;
    let mut entries = Vec::new();
    for (idx, line) in raw.lines().enumerate() {
        if let Some(entry) = parse_replay_line(line, default_ops)
            .with_context(|| format!("{}:{}", path.display(), idx + 1))?
        {
            entries.push(entry);
        }
    }
    if entries.is_empty() {
        bail!(
            "replay file {} contained no runnable entries",
            path.display()
        );
    }
    Ok(entries)
}

const BASE_REQUIRED_COVERAGE: &[&str] = &[
    "sim.scenario.started",
    "sim.scenario.passed",
    "sim.canonical_digest_mixed",
    "world.heal_and_quiesce",
    "oracle.sqlite_integrity_checked",
];

const FULL_SUITE_REQUIRED_COVERAGE: &[&str] = &[
    "oracle.post_bounce_recovery_checked",
    "oracle.outbox_file_scanned",
    "oracle.committed_durability_checked",
    "world.quiesce_natural",
    "notifications.non_empty_both_hosts",
    "workflow.rework",
    "workflow.stop",
    "remote_faults.three_way_auto_merge",
    "remote_faults.client_bounce_outbox_recovery",
    "github.collaborator_trust_gating",
    "github.pagination_multi_page",
    "env_sync.lww_converged",
    "reclaim.stuck_exit_kill_fallback",
    "schedules_budget.herdr_offline_queued_no_livelock",
    "workflow_faults.condition_external_pr_gated",
    "multi_client.two_client_three_way_auto_merge",
    "auth.invite_single_use",
    "auth.client_pair_persisted",
    "auth.duplicate_server_rejected",
    "auth.revoke_kicks_live_stream",
    "auth.client_revoked_auth_failed",
    "auth.tailscale_ephemeral_enc",
    "auth.client_tailscale_autoseal",
    "auth.api_mismatch",
    "workspaces.worktree_created",
    "workflow.script_executed",
    "workflow.script_herdr_pty",
    "workflow.script_herdr_crash_recovery",
    "workflow.admit_checked",
    "delivery.merge_batch_passed",
    "delivery.test_failure_rework",
    "delivery.hold_auto_merge",
    "delivery.multi_card_batch",
    "delivery.conflict_evict",
    "delivery.squash_or_merge",
    "delivery.sync_upstream_pushed",
    "bundle.exported",
    "bundle.managed_files_roundtrip",
    "bundle.github_linked_import",
    "bundle.schedules_multi_host",
    "bundle.imported_multi_host",
    "bundle.cross_host_ops_isolated",
    "bundle.project_archive_restore",
    "bundle.fault_recovery",
    "storage.full_sync_verified",
    "sessions.agent_lifecycle",
    "sessions.mention_question_delivered",
    "sessions.mention_crash_recovery",
    "sessions.card_adopted",
    "sessions.execution_retried",
    "storage.fault_busy_rollback",
    "storage.fault_ioerror_rollback",
    "storage.corrupt_outbox_recovery",
    "review.preview_checked",
    "review.agent_steered",
    "review.merged_cleanly",
    "review.draft_scope_isolated",
    "github.pr_published",
    "github.pr_refreshed",
    "github.pr_merged",
    "github.pr_remote_merge_reconciled",
    "drafts.conflict_and_persistence",
    "events.journal_listed",
    "events.subscription_delivery_and_ack",
    "events.subscription_crash_retry",
    "sched_contention.four_way_cap",
    "sched_contention.single_card_collision",
    "sched_contention.mixed_pty_reclaim",
    "sched_contention.all_five_failpoints",
    "sched_contention.natural_quiesce",
    "transport.local_framed_rpc",
    "oracle.read_your_writes_checked",
    "oracle.at_most_once_checked",
    "oracle.auth_failed_zero_dials_checked",
    "oracle.committed_merge_batch_checked",
    // Phase 1A: backend DST migrations
    "sched_contention.failpoint_subsites_complete",
    "card_automation.rank_move_compaction",
    "card_automation.resolve_reopen_auto_archive",
    "env_sync.secret_masking_and_warm_pty",
    "workflow_faults.external_secret_stripping",
    "sessions.two_phase_adopt_and_cwd_rename",
    "herdr.foreground_push_and_reminders",
    "delivery.manual_deliver_gc_and_target_advanced",
    "transport_auth.legacy_token_and_polling_fallback",
    // Phase 1B: Shadow UI Projection Oracles & presenter pagination/event batch
    "oracle.ui_committed_writes_checked",
    "oracle.ui_index_checked",
    "oracle.ui_overview_checked",
    "oracle.ui_quick_merge_checked",
    "oracle.column_ranks_ordered_checked",
    "client_pages.ui_presenter_pagination_checked",
    "client_pages.ui_event_batch_checked",
    // Phase 2A: core RPC, schedule, workflow & configuration session migrations
    "review_events.comments_notes_tags",
    "review_events.git_client_rpcs",
    "bundle.project_crud_and_onboarding",
    "schedules_budget.crud_and_artifact_read",
    "schedules_budget.backlog_and_workflow_edges",
    "sessions.configuration_sessions",
    // Phase 2B: GitHub browse/CI/intake & insights/overview/activity migrations
    "github.browse_and_references",
    "github.ci_fast_fail_and_cardless_issue",
    "github.intake_suppression_and_closure",
    "client_pages.insights_and_activity",
    "herdr.overview_saved_output_and_activity_rules",
    // Card #180: card agent assignee & workflow default_agent/override
    "assignee.create_wake",
    "assignee.update_bind_only",
    "assignee.comment_auto_sync",
    "assignee.explicit_mention_excludes_assignee",
    "assignee.workflow_default_agent",
    "assignee.workflow_override_preserved",
    "assignee.adopt_backfill",
    "assignee.offline_sync_converged",
];

const SCHEDULER_CONTENTION_REQUIRED_COVERAGE: &[&str] = &[
    "sched_contention.four_way_cap",
    "sched_contention.single_card_collision",
    "sched_contention.mixed_pty_reclaim",
    "sched_contention.all_five_failpoints",
    "sched_contention.failpoint_subsites_complete",
    "sched_contention.natural_quiesce",
    "transport.local_framed_rpc",
    "oracle.read_your_writes_checked",
    "oracle.at_most_once_checked",
    "oracle.committed_merge_batch_checked",
    "world.quiesce_natural",
];

const TRANSPORT_AUTH_REQUIRED_COVERAGE: &[&str] = &[
    "auth.invite_single_use",
    "auth.client_pair_persisted",
    "auth.duplicate_server_rejected",
    "auth.revoke_kicks_live_stream",
    "auth.client_revoked_auth_failed",
    "auth.tailscale_ephemeral_enc",
    "auth.client_tailscale_autoseal",
    "auth.api_mismatch",
    "transport_auth.legacy_token_and_polling_fallback",
    "oracle.auth_failed_zero_dials_checked",
];

fn required_coverage_for_scenario(sc: Scenario) -> &'static [&'static str] {
    match sc {
        Scenario::SchedulerCrossContention => SCHEDULER_CONTENTION_REQUIRED_COVERAGE,
        Scenario::TransportAuth => TRANSPORT_AUTH_REQUIRED_COVERAGE,
        _ => &[],
    }
}

const CHAOS_RANDOM_REQUIRED_COVERAGE: &[&str] = &[
    "oracle.committed_durability_checked",
    "oracle.committed_merge_batch_checked",
    "oracle.read_your_writes_checked",
    "oracle.at_most_once_checked",
    "oracle.auth_failed_zero_dials_checked",
    "oracle.ui_committed_writes_checked",
    "oracle.ui_index_checked",
    "oracle.ui_overview_checked",
    "oracle.ui_quick_merge_checked",
    "oracle.column_ranks_ordered_checked",
    "transport.local_framed_rpc",
    "world.quiesce_natural",
    "chaos.profile.desktop",
    "chaos.profile.mobile",
    "chaos.net_partition",
    "chaos.server_crash_bounce",
    "chaos.client_bounce",
    "chaos.env_save",
    "chaos.faulty_stream_chunk",
    "chaos.faulty_stream_rst",
    "chaos.faulty_stream_stall",
    "chaos.half_open_blackhole",
    "chaos.keepalive_timeout_fired",
    "chaos.multi_client_race",
    "chaos.schedule_ops",
    "chaos.worktree_delivery_ops",
    "chaos.delivery_batch_entered",
    "exec.shell_executed",
    "chaos.storage_fault_busy",
    "chaos.storage_fault_io_error",
    "chaos.failpoint_herdr_terminal_run",
    "chaos.auth_revoke_recover",
    "chaos.project_archive_restore",
    "chaos.card_resolve_reopen",
    "chaos.ranked_card_move",
    "chaos.natural_quiesce_recovered",
];

const CHAOS_SWARM_PROFILE_REQUIRED_COVERAGE: &[&str] = &[
    "chaos.profile.delivery_and_storage",
    "chaos.profile.workflow_and_schedule",
    "chaos.profile.auth_multi_client_blackhole",
    "chaos.profile.sessions_mention_reclaim_clock",
    "chaos.profile.bundle_github_partition",
    "chaos.profile.full_mix",
];

const RANDOM_SUMMARY_HIGHLIGHT_PROPERTIES: &[&str] = &[
    "chaos.delivery_batch_entered",
    "exec.shell_executed",
    "oracle.read_your_writes_checked",
    "oracle.at_most_once_checked",
    "oracle.committed_durability_checked",
    "chaos.natural_quiesce_recovered",
];

fn format_random_coverage_overview(tracker: &CoverageTracker) -> String {
    let mut out = format!(
        "\nCoverage overview ({} distinct properties hit):\n",
        tracker.counts().len()
    );
    for prop in RANDOM_SUMMARY_HIGHLIGHT_PROPERTIES {
        out.push_str(&format!("  {:<42} {}\n", prop, tracker.count(prop)));
    }
    match tracker.assert_never_zero(NEVER_COVERAGE_PROPERTIES) {
        Ok(()) => {
            let watched: Vec<String> = NEVER_COVERAGE_PROPERTIES
                .iter()
                .map(|p| format!("{p}=0"))
                .collect();
            out.push_str(&format!(
                "Never-property gate: PASS ({})\n",
                watched.join(", ")
            ));
        }
        Err(violations) => {
            let details: Vec<String> = violations.iter().map(|(p, c)| format!("{p}={c}")).collect();
            out.push_str(&format!(
                "Never-property gate: FAIL ({})\n",
                details.join(", ")
            ));
        }
    }
    out
}

fn print_fault_telemetry_table(tracker: &CoverageTracker) {
    let rows = tracker.armed_vs_fired_report();
    if rows.is_empty() {
        println!("\nArmed vs Fired Faults: (none recorded)");
        return;
    }
    let vacuous_count = rows.iter().filter(|r| r.vacuous).count();
    println!(
        "\nArmed vs Fired Faults ({} fault(s), {} vacuous):",
        rows.len(),
        vacuous_count
    );
    println!("  {:<44} {:>8} {:>8}  STATUS", "FAULT", "ARMED", "FIRED");
    println!("  {:-<44} {:-<8} {:-<8}  {:-<28}", "", "", "", "");
    for row in &rows {
        let status = if row.vacuous {
            "VACUOUS (armed > 0, fired == 0)"
        } else {
            "OK"
        };
        println!(
            "  {:<44} {:>8} {:>8}  {status}",
            row.name, row.armed, row.fired
        );
    }
}

fn print_cold_properties_table(tracker: &CoverageTracker, limit: usize) {
    let cold = tracker.cold_properties(limit);
    if cold.is_empty() {
        println!("\nCold / Rare Properties (Top {limit}): (none recorded)");
        return;
    }
    println!("\nCold / Rare Properties (Top {}):", cold.len());
    for (idx, (prop, count)) in cold.iter().enumerate() {
        println!("  {:>2}. {:<42} {count}", idx + 1, prop);
    }
}

fn check_never_zero_gate(tracker: &CoverageTracker) -> Result<()> {
    if let Err(violations) = tracker.assert_never_zero(NEVER_COVERAGE_PROPERTIES) {
        eprintln!("\nNEVER-PROPERTY VIOLATION(S) DETECTED:");
        for (prop, count) in &violations {
            eprintln!("  - {prop}: hit {count} time(s) (must be 0)");
        }
        let summary: Vec<String> = violations
            .into_iter()
            .map(|(prop, count)| format!("{prop}={count}"))
            .collect();
        bail!(
            "Never-property coverage gate failed; forbidden properties hit: [{}]",
            summary.join(", ")
        );
    }
    Ok(())
}

fn verify_coverage(
    tracker: &CoverageTracker,
    assert_coverage: bool,
    extra_required: &[String],
    suite_required: &[&str],
) -> Result<()> {
    if !assert_coverage && extra_required.is_empty() {
        return Ok(());
    }
    println!(
        "\nProperty coverage summary ({} distinct properties):\n{}",
        tracker.counts().len(),
        tracker.summary()
    );
    let mut required: Vec<&str> = Vec::new();
    if assert_coverage {
        required.extend_from_slice(BASE_REQUIRED_COVERAGE);
        required.extend_from_slice(suite_required);
    }
    for prop in extra_required {
        required.push(prop.as_str());
    }
    tracker.assert_required(&required)?;
    if assert_coverage {
        check_never_zero_gate(tracker)?;
        let vacuous = tracker.vacuous_armed_faults();
        if !vacuous.is_empty() {
            eprintln!("\nVACUOUS ARMED FAULT(S) DETECTED (armed > 0, fired == 0):");
            for name in &vacuous {
                eprintln!(
                    "  - {name} (armed={}, fired=0)",
                    tracker.fault_armed_count(name)
                );
            }
            bail!(
                "Coverage gate failed; vacuous armed faults never fired: [{}]",
                vacuous.join(", ")
            );
        }
    }
    println!(
        "Coverage gate PASSED: {}/{} required properties verified ({} total distinct properties hit; never-gate clean).",
        required.len(),
        required.len(),
        tracker.counts().len(),
    );
    Ok(())
}

fn format_shrink_report(
    report: &scenarios::chaos::ShrinkReport,
    orig_ops: usize,
    timed_out: bool,
) -> String {
    let replay = replay_cmd(report.seed, "chaos", orig_ops);
    let mut out = format!(
        "\n=== CHAOS SHRINK RESULT ===\nSeed:      {}\nMinimized: {} -> {} ops ({} attempts{})\nReplay:    {replay}\nShrink:    cargo run --release -p canban-sim --bin canban-sim -- shrink --seed {} --ops {}\nError:     {}\nMinimal op sequence:\n",
        report.seed,
        report.original_len,
        report.minimized_len,
        report.attempts,
        if timed_out {
            ", time budget reached"
        } else {
            ""
        },
        report.seed,
        orig_ops,
        report.error,
    );
    for (idx, op) in report.ops.iter().enumerate() {
        out.push_str(&format!("  [{idx:02}] {op:?}\n"));
    }
    out
}

fn print_shrink_report(report: &scenarios::chaos::ShrinkReport) {
    eprint!(
        "{}",
        format_shrink_report(report, report.original_len, false)
    );
}

fn auto_shrink_chaos_bounded(
    seed: u64,
    ops: usize,
    initial_error: String,
    max_attempts: usize,
    time_budget: Duration,
) -> (scenarios::chaos::ShrinkReport, bool) {
    let started = Instant::now();
    let (profile, generated) = scenarios::chaos::generate_ops(seed, ops);
    let original_len = generated.len();
    let mut timed_out = false;
    let (minimized, attempts, final_err) =
        scenarios::chaos::shrink_ops(generated, initial_error, max_attempts, |candidate| {
            if started.elapsed() >= time_budget {
                timed_out = true;
                return None;
            }
            trace::reset_thread_coverage();
            let outcome = match catch_unwind(AssertUnwindSafe(|| {
                scenarios::chaos::run_ops(seed, &profile, candidate)
            })) {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(format!("{e:#}")),
                Err(payload) => Some(format!("PANIC: {}", format_panic_payload(payload.as_ref()))),
            };
            let _ = trace::take_thread_coverage();
            outcome
        });
    (
        scenarios::chaos::ShrinkReport {
            seed,
            original_len,
            minimized_len: minimized.len(),
            attempts,
            ops: minimized,
            error: final_err,
        },
        timed_out,
    )
}

fn resolve_corpus_path(file: &Path) -> PathBuf {
    if file.exists() {
        return file.to_path_buf();
    }
    if file == Path::new(DEFAULT_CORPUS_FILE) {
        let fallback = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("corpus")
            .join("regressions.txt");
        if fallback.exists() {
            return fallback;
        }
    }
    file.to_path_buf()
}

fn run_corpus(
    file: PathBuf,
    jobs: usize,
    default_ops: usize,
    seed_timeout: Option<u64>,
    trace_dir: PathBuf,
    report_json: Option<PathBuf>,
) -> Result<()> {
    let resolved = resolve_corpus_path(&file);
    let entries = load_replay_file(&resolved, default_ops)?;
    let total = entries.len();
    let workers = jobs.max(1).min(total.max(1));

    println!(
        "Replaying {} corpus regression seed(s) from {} (jobs={workers}, seed_timeout={})...",
        total,
        resolved.display(),
        seed_timeout
            .map(|s| format!("{s}s"))
            .unwrap_or_else(|| "none".into()),
    );
    let _ = io::stdout().flush();

    let next_idx = Arc::new(AtomicUsize::new(0));
    let passed_count = Arc::new(AtomicU64::new(0));
    let failed_count = Arc::new(AtomicU64::new(0));
    let failures: Arc<Mutex<Vec<CorpusFailure>>> = Arc::new(Mutex::new(Vec::new()));
    let total_coverage: Arc<Mutex<CoverageTracker>> = Arc::new(Mutex::new(CoverageTracker::new()));
    let entries_ref = Arc::new(entries);
    let started = Instant::now();

    let timeout_hook: Option<TimeoutHook> = report_json.clone().map(|report_path| {
        let passed_ref = Arc::clone(&passed_count);
        let failed_ref = Arc::clone(&failed_count);
        let failures_ref = Arc::clone(&failures);
        let cov_ref = Arc::clone(&total_coverage);
        Arc::new(
            move |item: &ActiveSeed, _elapsed: Duration, err: &anyhow::Error| {
                let passed = passed_ref.load(Ordering::Relaxed);
                let failed = failed_ref.load(Ordering::Relaxed);
                let mut fail_list: Vec<FailureReport> = failures_ref
                    .try_lock()
                    .map(|guard| guard.iter().map(|f| f.report.clone()).collect())
                    .unwrap_or_default();
                fail_list.push(build_failure_report(
                    item.scenario,
                    item.seed,
                    item.ops,
                    format!("{err:#}"),
                    true,
                    None,
                ));
                let coverage = cov_ref
                    .try_lock()
                    .map(|guard| build_coverage_report(&guard, false, &[], &[]))
                    .unwrap_or_else(|_| {
                        build_coverage_report(&CoverageTracker::new(), false, &[], &[])
                    });
                let report = SimReport {
                    command: "corpus",
                    scenario: "corpus".into(),
                    master_seed: None,
                    ops: default_ops,
                    duration_secs: None,
                    elapsed_secs: started.elapsed().as_secs_f64(),
                    passed,
                    failed,
                    timed_out: 1,
                    total: passed + failed + 1,
                    coverage,
                    failures: fail_list,
                };
                let _ = persist_optional_report(Some(&report_path), &report, false);
            },
        ) as TimeoutHook
    });
    let watchdog = Arc::new(WatchdogHandle::start(
        seed_timeout,
        trace_dir.clone(),
        timeout_hook,
    ));

    std::thread::scope(|scope| {
        for worker_id in 0..workers {
            let entries = Arc::clone(&entries_ref);
            let next_idx = Arc::clone(&next_idx);
            let passed_count = Arc::clone(&passed_count);
            let failed_count = Arc::clone(&failed_count);
            let failures = Arc::clone(&failures);
            let total_coverage = Arc::clone(&total_coverage);
            let watchdog = Arc::clone(&watchdog);
            let trace_dir = trace_dir.clone();
            scope.spawn(move || {
                loop {
                    let idx = next_idx.fetch_add(1, Ordering::SeqCst);
                    let Some(entry) = entries.get(idx) else {
                        break;
                    };
                    let n = idx + 1;
                    println!(
                        "[{n}/{total} w{worker_id}] START {:<15} seed={} ops={}",
                        entry.scenario.name(),
                        entry.seed,
                        entry.ops,
                    );
                    let _ = io::stdout().flush();

                    watchdog.begin_seed(worker_id, entry.scenario, entry.seed, entry.ops);
                    let t0 = Instant::now();
                    let (res, cov) = execute_seed_caught(entry.scenario, entry.seed, entry.ops);
                    watchdog.end_seed(worker_id);
                    total_coverage.lock().unwrap().merge(&cov);

                    match res {
                        Ok(digest) => {
                            passed_count.fetch_add(1, Ordering::Relaxed);
                            println!(
                                "[{n}/{total} w{worker_id}] PASS  {:<15} seed={:<20} ops={:<4} digest={:#018x} ({:.2}s)",
                                entry.scenario.name(),
                                entry.seed,
                                entry.ops,
                                digest,
                                t0.elapsed().as_secs_f64(),
                            );
                        }
                        Err(e) => {
                            failed_count.fetch_add(1, Ordering::Relaxed);
                            let saved = write_failure_log(
                                &trace_dir,
                                entry.seed,
                                entry.scenario.name(),
                                entry.ops,
                                &e,
                            );
                            let replay =
                                replay_cmd(entry.seed, entry.scenario.name(), entry.ops);
                            eprintln!(
                                "[{n}/{total} w{worker_id}] FAIL  {:<15} seed={} ops={}\n  Replay: {replay}\n{}  Error:  {e:#}",
                                entry.scenario.name(),
                                entry.seed,
                                entry.ops,
                                saved
                                    .map(|p| format!("  Log:    {}\n", p.display()))
                                    .unwrap_or_default(),
                            );
                            let fail_report = build_failure_report(
                                entry.scenario.name(),
                                entry.seed,
                                entry.ops,
                                format!("{e:#}"),
                                false,
                                None,
                            );
                            failures.lock().unwrap().push(CorpusFailure {
                                index: n,
                                entry: entry.clone(),
                                replay,
                                report: fail_report,
                            });
                        }
                    }
                }
            });
        }
    });

    drop(watchdog);

    let passed = passed_count.load(Ordering::Relaxed);
    let failed = failed_count.load(Ordering::Relaxed);
    let elapsed = started.elapsed().as_secs_f64();
    let mut recorded = failures.lock().unwrap();
    recorded.sort_by_key(|f| f.index);
    let cov = total_coverage.lock().unwrap();

    let report = SimReport {
        command: "corpus",
        scenario: "corpus".into(),
        master_seed: None,
        ops: default_ops,
        duration_secs: None,
        elapsed_secs: elapsed,
        passed,
        failed,
        timed_out: 0,
        total: passed + failed,
        coverage: build_coverage_report(&cov, false, &[], &[]),
        failures: recorded.iter().map(|f| f.report.clone()).collect(),
    };
    persist_optional_report(report_json.as_deref(), &report, recorded.is_empty())?;

    println!(
        "\nCorpus summary ({}): {passed} passed, {failed} failed ({total} total) in {elapsed:.2}s.",
        resolved.display(),
    );
    if !recorded.is_empty() {
        eprintln!("Failed corpus seed(s):");
        for item in recorded.iter() {
            eprintln!(
                "  - #{} {} seed={} ops={} -> {}",
                item.index,
                item.entry.scenario.name(),
                item.entry.seed,
                item.entry.ops,
                item.replay,
            );
        }
        bail!("{failed} corpus regression seed(s) failed");
    }
    Ok(())
}

fn run_shrink_cmd(seed: u64, ops: usize) -> Result<()> {
    println!("Shrinking chaos seed={seed} ops={ops}...");
    let _ = io::stdout().flush();
    match scenarios::chaos::shrink(seed, ops)? {
        Some(report) => {
            print_shrink_report(&report);
            Ok(())
        }
        None => {
            println!("Seed {seed} (ops={ops}) PASSED without error — nothing to shrink.");
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_suite(
    scenario: Option<String>,
    base_seed: u64,
    seeds: u64,
    ops: usize,
    shrink_on_fail: bool,
    replay_file: Option<PathBuf>,
    continue_on_error: bool,
    seed_timeout: Option<u64>,
    assert_coverage: bool,
    require_coverage: Vec<String>,
    trace_dir: PathBuf,
    report_json: Option<PathBuf>,
) -> Result<()> {
    let is_full_suite = replay_file.is_none() && scenario.is_none();
    let single_scenario = match (&replay_file, &scenario) {
        (None, Some(name)) => Some(Scenario::parse(name)?),
        _ => None,
    };
    let report_scenario = if replay_file.is_some() {
        "replay_file".to_owned()
    } else if let Some(sc) = single_scenario {
        sc.name().to_owned()
    } else {
        "all".to_owned()
    };
    let suite_req: &'static [&'static str] = if is_full_suite {
        FULL_SUITE_REQUIRED_COVERAGE
    } else if let Some(sc) = single_scenario {
        required_coverage_for_scenario(sc)
    } else {
        &[]
    };
    let plan: Vec<ReplayEntry> = if let Some(path) = replay_file {
        load_replay_file(&path, ops)?
    } else {
        let list: Vec<Scenario> = match single_scenario {
            Some(sc) => vec![sc],
            None => {
                let mut full = Scenario::ALL_NAMED.to_vec();
                full.push(Scenario::Chaos);
                full
            }
        };
        let mut items = Vec::new();
        for sc in list {
            for offset in 0..seeds {
                items.push(ReplayEntry {
                    scenario: sc,
                    seed: base_seed.wrapping_add(offset),
                    ops,
                });
            }
        }
        items
    };

    let started = Instant::now();
    let passed_count = Arc::new(AtomicU64::new(0));
    let failures: Arc<Mutex<Vec<SuiteFailure>>> = Arc::new(Mutex::new(Vec::new()));
    let total_coverage: Arc<Mutex<CoverageTracker>> = Arc::new(Mutex::new(CoverageTracker::new()));

    let timeout_hook: Option<TimeoutHook> = report_json.clone().map(|report_path| {
        let passed_ref = Arc::clone(&passed_count);
        let failures_ref = Arc::clone(&failures);
        let cov_ref = Arc::clone(&total_coverage);
        let report_scenario = report_scenario.clone();
        let req_cov = require_coverage.clone();
        Arc::new(
            move |item: &ActiveSeed, _elapsed: Duration, err: &anyhow::Error| {
                let passed = passed_ref.load(Ordering::Relaxed);
                let mut fail_list: Vec<FailureReport> = failures_ref
                    .try_lock()
                    .map(|guard| guard.iter().map(|f| f.report.clone()).collect())
                    .unwrap_or_default();
                let prior_failed = fail_list.len() as u64;
                fail_list.push(build_failure_report(
                    item.scenario,
                    item.seed,
                    item.ops,
                    format!("{err:#}"),
                    true,
                    None,
                ));
                let coverage = cov_ref
                    .try_lock()
                    .map(|guard| {
                        build_coverage_report(&guard, assert_coverage, &req_cov, suite_req)
                    })
                    .unwrap_or_else(|_| {
                        build_coverage_report(
                            &CoverageTracker::new(),
                            assert_coverage,
                            &req_cov,
                            suite_req,
                        )
                    });
                let report = SimReport {
                    command: "run",
                    scenario: report_scenario.clone(),
                    master_seed: None,
                    ops,
                    duration_secs: None,
                    elapsed_secs: started.elapsed().as_secs_f64(),
                    passed,
                    failed: prior_failed,
                    timed_out: 1,
                    total: passed + prior_failed + 1,
                    coverage,
                    failures: fail_list,
                };
                let _ = persist_optional_report(Some(&report_path), &report, false);
            },
        ) as TimeoutHook
    });
    let watchdog = WatchdogHandle::start(seed_timeout, trace_dir.clone(), timeout_hook);
    let mut early_err: Option<anyhow::Error> = None;

    for entry in plan {
        println!(
            "START {:<20} seed={:<12} ops={}",
            entry.scenario.name(),
            entry.seed,
            entry.ops,
        );
        let _ = io::stdout().flush();

        watchdog.begin_seed(0, entry.scenario, entry.seed, entry.ops);
        let t0 = Instant::now();
        let (res, cov) = execute_seed_caught(entry.scenario, entry.seed, entry.ops);
        watchdog.end_seed(0);
        total_coverage.lock().unwrap().merge(&cov);

        match res {
            Ok(digest) => {
                passed_count.fetch_add(1, Ordering::Relaxed);
                println!(
                    "PASS  {:<20} seed={:<12} digest={:#018x} ({:.2}s)",
                    entry.scenario.name(),
                    entry.seed,
                    digest,
                    t0.elapsed().as_secs_f64()
                );
            }
            Err(e) => {
                let saved =
                    write_failure_log(&trace_dir, entry.seed, entry.scenario.name(), entry.ops, &e);
                let replay = replay_cmd(entry.seed, entry.scenario.name(), entry.ops);
                eprintln!(
                    "FAIL  {:<20} seed={} ops={}\nReplay: {replay}\n{}Error:  {e:#}",
                    entry.scenario.name(),
                    entry.seed,
                    entry.ops,
                    saved
                        .map(|p| format!("Log:    {}\n", p.display()))
                        .unwrap_or_default(),
                );
                let mut shrunk = None;
                if shrink_on_fail
                    && entry.scenario == Scenario::Chaos
                    && let Ok(Some(report)) = scenarios::chaos::shrink(entry.seed, entry.ops)
                {
                    print_shrink_report(&report);
                    shrunk = Some(build_shrunk_report(&report));
                }
                let fail_report = build_failure_report(
                    entry.scenario.name(),
                    entry.seed,
                    entry.ops,
                    format!("{e:#}"),
                    false,
                    shrunk,
                );
                failures.lock().unwrap().push(SuiteFailure {
                    seed: entry.seed,
                    scenario: entry.scenario.name(),
                    ops: entry.ops,
                    replay,
                    report: fail_report,
                });
                if !continue_on_error {
                    early_err = Some(e);
                    break;
                }
            }
        }
    }

    drop(watchdog);

    let passed = passed_count.load(Ordering::Relaxed);
    let recorded = failures.lock().unwrap();
    let cov = total_coverage.lock().unwrap();
    let failed = recorded.len() as u64;

    if let Some(err) = early_err {
        let report = SimReport {
            command: "run",
            scenario: report_scenario,
            master_seed: None,
            ops,
            duration_secs: None,
            elapsed_secs: started.elapsed().as_secs_f64(),
            passed,
            failed,
            timed_out: 0,
            total: passed + failed,
            coverage: build_coverage_report(&cov, assert_coverage, &require_coverage, suite_req),
            failures: recorded.iter().map(|f| f.report.clone()).collect(),
        };
        persist_optional_report(report_json.as_deref(), &report, false)?;
        return Err(err);
    }

    if !recorded.is_empty() {
        if let Err(violations) = cov.assert_never_zero(NEVER_COVERAGE_PROPERTIES) {
            eprintln!("\nWARNING: NEVER-PROPERTY VIOLATION(S) DETECTED:");
            for (prop, count) in &violations {
                eprintln!("  - {prop}: hit {count} time(s) (must be 0)");
            }
        }
        eprintln!(
            "\nSimulation suite finished with {passed} passed, {failed} failed in {:.2}s:",
            started.elapsed().as_secs_f64()
        );
        for item in recorded.iter() {
            eprintln!(
                "  - seed={} ({}, ops={}) -> {}",
                item.seed, item.scenario, item.ops, item.replay
            );
        }
        let report = SimReport {
            command: "run",
            scenario: report_scenario,
            master_seed: None,
            ops,
            duration_secs: None,
            elapsed_secs: started.elapsed().as_secs_f64(),
            passed,
            failed,
            timed_out: 0,
            total: passed + failed,
            coverage: build_coverage_report(&cov, assert_coverage, &require_coverage, suite_req),
            failures: recorded.iter().map(|f| f.report.clone()).collect(),
        };
        persist_optional_report(report_json.as_deref(), &report, false)?;
        bail!("{failed} deterministic simulation run(s) failed");
    }

    println!(
        "\nAll {passed} deterministic simulation run(s) passed in {:.2}s.",
        started.elapsed().as_secs_f64()
    );
    if !assert_coverage && let Err(violations) = cov.assert_never_zero(NEVER_COVERAGE_PROPERTIES) {
        eprintln!("\nWARNING: NEVER-PROPERTY VIOLATION(S) DETECTED:");
        for (prop, count) in &violations {
            eprintln!("  - {prop}: hit {count} time(s) (must be 0)");
        }
    }
    if assert_coverage {
        print_fault_telemetry_table(&cov);
        print_cold_properties_table(&cov, COLD_PROPERTIES_TOP_N);
    }
    let cov_res = verify_coverage(&cov, assert_coverage, &require_coverage, suite_req);
    let report = SimReport {
        command: "run",
        scenario: report_scenario,
        master_seed: None,
        ops,
        duration_secs: None,
        elapsed_secs: started.elapsed().as_secs_f64(),
        passed,
        failed: 0,
        timed_out: 0,
        total: passed,
        coverage: build_coverage_report(&cov, assert_coverage, &require_coverage, suite_req),
        failures: Vec::new(),
    };
    persist_optional_report(report_json.as_deref(), &report, cov_res.is_ok())?;
    cov_res
}

enum RandomTarget {
    Single(Scenario),
    AllMixed,
}

impl RandomTarget {
    fn parse(s: &str) -> Result<Self> {
        if s.trim().eq_ignore_ascii_case("all") {
            Ok(Self::AllMixed)
        } else {
            Ok(Self::Single(Scenario::parse(s)?))
        }
    }

    fn pick(&self, rng: &mut impl Rng) -> Scenario {
        match self {
            Self::Single(sc) => *sc,
            Self::AllMixed => {
                // 50% full-spectrum chaos, 50% one of the 17 focused scenarios.
                if rng.random_bool(0.5) {
                    Scenario::Chaos
                } else {
                    let idx = rng.random_range(0..Scenario::ALL_NAMED.len());
                    Scenario::ALL_NAMED[idx]
                }
            }
        }
    }
}

fn spawn_ctrlc_watcher(stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                stop.store(true, Ordering::SeqCst);
                eprintln!(
                    "\nReceived Ctrl-C — finishing in-flight seed(s) and printing summary (press Ctrl-C again to force quit)..."
                );
                if tokio::signal::ctrl_c().await.is_ok() {
                    std::process::exit(130);
                }
            }
        });
    });
}

fn random_required_coverage(
    scenario_arg: &str,
    passed: u64,
    cov: &CoverageTracker,
) -> Vec<&'static str> {
    let mut random_req: Vec<&'static str> = Vec::new();
    if scenario_arg.trim().eq_ignore_ascii_case("chaos") && passed >= 10 {
        random_req.extend_from_slice(CHAOS_RANDOM_REQUIRED_COVERAGE);
        if passed >= 24 {
            random_req.extend_from_slice(CHAOS_SWARM_PROFILE_REQUIRED_COVERAGE);
        }
    } else if scenario_arg.trim().eq_ignore_ascii_case("all") && passed >= 12 {
        random_req.extend_from_slice(&[
            "oracle.post_bounce_recovery_checked",
            "oracle.outbox_file_scanned",
            "oracle.committed_durability_checked",
        ]);
        if cov.count("sim.scenario.chaos") >= 6 {
            random_req.extend_from_slice(&[
                "chaos.net_partition",
                "chaos.server_crash_bounce",
                "chaos.client_bounce",
                "chaos.env_save",
            ]);
        }
    }
    random_req
}

#[allow(clippy::too_many_arguments)]
fn run_random(
    fail_fast: bool,
    scenario_arg: String,
    max_seeds: Option<u64>,
    duration_secs: Option<u64>,
    jobs: usize,
    ops: usize,
    master_seed: Option<u64>,
    seed_timeout: Option<u64>,
    assert_coverage: bool,
    require_coverage: Vec<String>,
    auto_shrink: bool,
    trace_dir: PathBuf,
    report_json: Option<PathBuf>,
) -> Result<()> {
    let target = Arc::new(RandomTarget::parse(&scenario_arg)?);
    let workers = jobs.max(1);
    let resolved_master_seed = master_seed.unwrap_or_else(|| rand::rng().random());
    let generator = Arc::new(Mutex::new(StdRng::seed_from_u64(resolved_master_seed)));
    let started = Instant::now();
    let deadline = duration_secs.map(|s| started + Duration::from_secs(s));
    let claimed = Arc::new(AtomicU64::new(0));
    let passed_count = Arc::new(AtomicU64::new(0));
    let failed_count = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let failures: Arc<Mutex<Vec<RandomFailure>>> = Arc::new(Mutex::new(Vec::new()));
    let total_coverage: Arc<Mutex<CoverageTracker>> = Arc::new(Mutex::new(CoverageTracker::new()));
    let shrink_lock: Arc<Mutex<()>> = Arc::new(Mutex::new(()));

    let timeout_hook: Option<TimeoutHook> = report_json.clone().map(|report_path| {
        let passed_ref = Arc::clone(&passed_count);
        let failed_ref = Arc::clone(&failed_count);
        let failures_ref = Arc::clone(&failures);
        let cov_ref = Arc::clone(&total_coverage);
        let scenario_name = scenario_arg.clone();
        let req_cov = require_coverage.clone();
        Arc::new(
            move |item: &ActiveSeed, _elapsed: Duration, err: &anyhow::Error| {
                let passed = passed_ref.load(Ordering::Relaxed);
                let failed = failed_ref.load(Ordering::Relaxed);
                let mut fail_list: Vec<FailureReport> = failures_ref
                    .try_lock()
                    .map(|guard| guard.iter().map(|f| f.report.clone()).collect())
                    .unwrap_or_default();
                fail_list.push(build_failure_report(
                    item.scenario,
                    item.seed,
                    item.ops,
                    format!("{err:#}"),
                    true,
                    None,
                ));
                let coverage = cov_ref
                    .try_lock()
                    .map(|guard| {
                        let req = random_required_coverage(&scenario_name, passed, &guard);
                        build_coverage_report(&guard, assert_coverage, &req_cov, &req)
                    })
                    .unwrap_or_else(|_| {
                        build_coverage_report(
                            &CoverageTracker::new(),
                            assert_coverage,
                            &req_cov,
                            &[],
                        )
                    });
                let report = SimReport {
                    command: "random",
                    scenario: scenario_name.clone(),
                    master_seed: Some(resolved_master_seed),
                    ops,
                    duration_secs,
                    elapsed_secs: started.elapsed().as_secs_f64(),
                    passed,
                    failed,
                    timed_out: 1,
                    total: passed + failed + 1,
                    coverage,
                    failures: fail_list,
                };
                let _ = persist_optional_report(Some(&report_path), &report, false);
            },
        ) as TimeoutHook
    });
    let watchdog = Arc::new(WatchdogHandle::start(
        seed_timeout,
        trace_dir.clone(),
        timeout_hook,
    ));
    spawn_ctrlc_watcher(Arc::clone(&stop));

    println!(
        "Starting Canban random-sim (scenario={scenario_arg}, jobs={workers}, ops={ops}, master_seed={resolved_master_seed}, fail_fast={fail_fast}, auto_shrink={auto_shrink}, seeds={}, duration={}, seed_timeout={})...",
        max_seeds
            .map(|n| n.to_string())
            .unwrap_or_else(|| "infinite (Ctrl-C to stop)".into()),
        duration_secs
            .map(|s| format!("{s}s"))
            .unwrap_or_else(|| "unlimited".into()),
        seed_timeout
            .map(|s| format!("{s}s"))
            .unwrap_or_else(|| "none".into()),
    );
    let _ = io::stdout().flush();

    std::thread::scope(|scope| {
        for worker_id in 0..workers {
            let target = Arc::clone(&target);
            let generator = Arc::clone(&generator);
            let claimed = Arc::clone(&claimed);
            let passed_count = Arc::clone(&passed_count);
            let failed_count = Arc::clone(&failed_count);
            let stop = Arc::clone(&stop);
            let failures = Arc::clone(&failures);
            let total_coverage = Arc::clone(&total_coverage);
            let shrink_lock = Arc::clone(&shrink_lock);
            let watchdog = Arc::clone(&watchdog);
            let trace_dir = trace_dir.clone();
            scope.spawn(move || {
                loop {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Some(dl) = deadline
                        && Instant::now() >= dl
                    {
                        break;
                    }
                    let slot = claimed.fetch_add(1, Ordering::SeqCst);
                    if let Some(limit) = max_seeds
                        && slot >= limit
                    {
                        claimed.fetch_sub(1, Ordering::SeqCst);
                        break;
                    }
                    let (seed, sc) = {
                        let mut rng = generator.lock().unwrap();
                        let seed: u64 = rng.random();
                        let sc = target.pick(&mut *rng);
                        (seed, sc)
                    };
                    let n = slot + 1;
                    println!(
                        "[w{worker_id} #{n:<5}] RUN  {:<15} seed={seed} ops={ops}",
                        sc.name(),
                    );
                    let _ = io::stdout().flush();

                    watchdog.begin_seed(worker_id, sc, seed, ops);
                    let t0 = Instant::now();
                    let (res, cov) = execute_seed_caught(sc, seed, ops);
                    watchdog.end_seed(worker_id);
                    total_coverage.lock().unwrap().merge(&cov);

                    match res {
                        Ok(digest) => {
                            passed_count.fetch_add(1, Ordering::Relaxed);
                            println!(
                                "[w{worker_id} #{n:<5}] PASS {:<15} seed={:<20} digest={:#018x} ({:.2}s)",
                                sc.name(),
                                seed,
                                digest,
                                t0.elapsed().as_secs_f64(),
                            );
                        }
                        Err(e) => {
                            failed_count.fetch_add(1, Ordering::Relaxed);
                            let saved = write_failure_log(&trace_dir, seed, sc.name(), ops, &e);
                            let replay = replay_cmd(seed, sc.name(), ops);
                            eprintln!(
                                "\n[w{worker_id} #{n:<5}] FAIL {:<15} seed={seed} ops={ops}\n  Replay: {replay}\n{}  Error:  {e:#}\n",
                                sc.name(),
                                saved
                                    .as_ref()
                                    .map(|p| format!("  Log:    {}\n", p.display()))
                                    .unwrap_or_default()
                            );
                            let err_str = format!("{e:#}");
                            let fail_report = build_failure_report(
                                sc.name(),
                                seed,
                                ops,
                                err_str.clone(),
                                false,
                                None,
                            );
                            let fail_idx = {
                                let mut guard = failures.lock().unwrap();
                                guard.push(RandomFailure {
                                    seed,
                                    scenario: sc.name().to_owned(),
                                    replay,
                                    report: fail_report,
                                });
                                guard.len() - 1
                            };
                            if fail_fast {
                                stop.store(true, Ordering::SeqCst);
                            }
                            if auto_shrink && sc == Scenario::Chaos {
                                let _shrink_guard = shrink_lock.lock().unwrap();
                                eprintln!(
                                    "[w{worker_id} #{n:<5}] AUTO-SHRINK chaos seed={seed} ops={ops} (max_attempts={AUTO_SHRINK_MAX_ATTEMPTS}, budget={}s)...",
                                    AUTO_SHRINK_TIME_BUDGET.as_secs()
                                );
                                let (report, timed_out) = auto_shrink_chaos_bounded(
                                    seed,
                                    ops,
                                    err_str,
                                    AUTO_SHRINK_MAX_ATTEMPTS,
                                    AUTO_SHRINK_TIME_BUDGET,
                                );
                                let shrunk = build_shrunk_report(&report);
                                if let Some(entry) = failures.lock().unwrap().get_mut(fail_idx) {
                                    entry.report.shrunk = Some(shrunk);
                                }
                                let formatted = format_shrink_report(&report, ops, timed_out);
                                eprint!("{formatted}");
                                if let Some(ref log_path) = saved
                                    && let Ok(mut f) = OpenOptions::new().append(true).open(log_path)
                                {
                                    let _ = f.write_all(formatted.as_bytes());
                                }
                            }
                            if fail_fast {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });

    drop(watchdog);

    let passed = passed_count.load(Ordering::Relaxed);
    let failed = failed_count.load(Ordering::Relaxed);
    let elapsed = started.elapsed().as_secs_f64();
    let recorded = failures.lock().unwrap();
    let cov = total_coverage.lock().unwrap();
    let random_req = random_required_coverage(&scenario_arg, passed, &cov);

    println!(
        "\nrandom-sim summary: {passed} passed, {failed} failed ({} total, master_seed={resolved_master_seed}) in {elapsed:.2}s.",
        passed + failed
    );
    print_fault_telemetry_table(&cov);
    print_cold_properties_table(&cov, COLD_PROPERTIES_TOP_N);
    print!("{}", format_random_coverage_overview(&cov));
    if (!assert_coverage || !recorded.is_empty())
        && let Err(violations) = cov.assert_never_zero(NEVER_COVERAGE_PROPERTIES)
    {
        eprintln!("\nWARNING: NEVER-PROPERTY VIOLATION(S) DETECTED:");
        for (prop, count) in &violations {
            eprintln!("  - {prop}: hit {count} time(s) (must be 0)");
        }
    }

    if !recorded.is_empty() {
        eprintln!("\nFailed seed(s):");
        for item in recorded.iter() {
            eprintln!(
                "  - seed={} ({}) -> {}",
                item.seed, item.scenario, item.replay
            );
        }
        let report = SimReport {
            command: "random",
            scenario: scenario_arg,
            master_seed: Some(resolved_master_seed),
            ops,
            duration_secs,
            elapsed_secs: elapsed,
            passed,
            failed,
            timed_out: 0,
            total: passed + failed,
            coverage: build_coverage_report(&cov, assert_coverage, &require_coverage, &random_req),
            failures: recorded.iter().map(|f| f.report.clone()).collect(),
        };
        persist_optional_report(report_json.as_deref(), &report, false)?;
        bail!("{failed} random-sim seed(s) failed");
    }

    let cov_res = verify_coverage(&cov, assert_coverage, &require_coverage, &random_req);
    let report = SimReport {
        command: "random",
        scenario: scenario_arg,
        master_seed: Some(resolved_master_seed),
        ops,
        duration_secs,
        elapsed_secs: elapsed,
        passed,
        failed: 0,
        timed_out: 0,
        total: passed,
        coverage: build_coverage_report(&cov, assert_coverage, &require_coverage, &random_req),
        failures: Vec::new(),
    };
    persist_optional_report(report_json.as_deref(), &report, cov_res.is_ok())?;
    cov_res
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => run_suite(
            None,
            2026,
            1,
            40,
            false,
            None,
            false,
            None,
            false,
            Vec::new(),
            PathBuf::from(".local/sim-failures"),
            None,
        ),
        Some(Command::Run {
            scenario,
            seed,
            seeds,
            ops,
            shrink,
            replay_file,
            continue_on_error,
            seed_timeout,
            assert_coverage,
            require_coverage,
            trace_dir,
            report_json,
        }) => run_suite(
            scenario,
            seed,
            seeds,
            ops,
            shrink,
            replay_file,
            continue_on_error,
            seed_timeout,
            assert_coverage,
            require_coverage,
            trace_dir,
            report_json,
        ),
        Some(Command::Shrink { seed, ops }) => run_shrink_cmd(seed, ops),
        Some(Command::Random {
            fail_fast,
            scenario,
            seeds,
            duration,
            jobs,
            ops,
            master_seed,
            seed_timeout,
            assert_coverage,
            require_coverage,
            auto_shrink,
            trace_dir,
            report_json,
        }) => run_random(
            fail_fast,
            scenario,
            seeds,
            duration,
            jobs,
            ops,
            master_seed,
            seed_timeout,
            assert_coverage,
            require_coverage,
            auto_shrink,
            trace_dir,
            report_json,
        ),
        Some(Command::Corpus {
            file,
            jobs,
            ops,
            seed_timeout,
            trace_dir,
            report_json,
        }) => run_corpus(file, jobs, ops, seed_timeout, trace_dir, report_json),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_log_filename_is_non_colliding() {
        assert_eq!(
            failure_log_filename("chaos", 42, 40),
            "chaos-seed-42-ops-40.log"
        );
        assert_eq!(
            failure_log_filename("github_sync", 42, 40),
            "github_sync-seed-42-ops-40.log"
        );
    }

    #[test]
    fn parse_replay_lines_handles_failed_seeds_and_shorthand_formats() {
        assert_eq!(parse_replay_line("  # comment ", 40).unwrap(), None);
        assert_eq!(parse_replay_line("", 40).unwrap(), None);

        let e1 = parse_replay_line(
            "seed=1337 scenario=chaos ops=75 replay=\"mise run sim -- --seed 1337 --scenario chaos --ops 75\"",
            40,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            e1,
            ReplayEntry {
                scenario: Scenario::Chaos,
                seed: 1337,
                ops: 75,
            }
        );

        let e2 = parse_replay_line("remote_faults 99", 40).unwrap().unwrap();
        assert_eq!(
            e2,
            ReplayEntry {
                scenario: Scenario::RemoteFaults,
                seed: 99,
                ops: 40,
            }
        );
    }

    #[test]
    fn panic_payload_formatting_extracts_messages() {
        let s: Box<dyn Any + Send> = Box::new("boom");
        assert_eq!(format_panic_payload(s.as_ref()), "boom");
        let owned: Box<dyn Any + Send> = Box::new(String::from("owned boom"));
        assert_eq!(format_panic_payload(owned.as_ref()), "owned boom");
    }

    #[test]
    fn full_suite_coverage_gate_requires_unique_properties() {
        use std::collections::BTreeSet;

        let all: Vec<&str> = BASE_REQUIRED_COVERAGE
            .iter()
            .chain(FULL_SUITE_REQUIRED_COVERAGE.iter())
            .copied()
            .collect();
        let unique: BTreeSet<&str> = all.iter().copied().collect();
        assert_eq!(
            all.len(),
            115,
            "full suite coverage gate must require 115 properties"
        );
        assert_eq!(
            unique.len(),
            all.len(),
            "full suite coverage gate must not contain duplicate property keys"
        );

        let mut tracker = CoverageTracker::new();
        for key in &all {
            tracker.hit(key);
        }
        verify_coverage(&tracker, true, &[], FULL_SUITE_REQUIRED_COVERAGE).unwrap();

        let mut incomplete = CoverageTracker::new();
        for key in all.iter().skip(1) {
            incomplete.hit(key);
        }
        assert!(verify_coverage(&incomplete, true, &[], FULL_SUITE_REQUIRED_COVERAGE).is_err());

        // Per-scenario required coverage for Scenario 18 and transport_auth must have no duplicates
        // and be subsets of Gate A.
        for sc in [Scenario::SchedulerCrossContention, Scenario::TransportAuth] {
            let per_sc = required_coverage_for_scenario(sc);
            assert!(
                !per_sc.is_empty(),
                "expected per-scenario required coverage for {}",
                sc.name()
            );
            let sc_set: BTreeSet<&str> = per_sc.iter().copied().collect();
            assert_eq!(sc_set.len(), per_sc.len());
            for key in per_sc {
                assert!(
                    unique.contains(key),
                    "per-scenario key '{key}' for {} missing from Gate A FULL_SUITE_REQUIRED_COVERAGE",
                    sc.name()
                );
            }
        }

        let chaos_all: Vec<&str> = BASE_REQUIRED_COVERAGE
            .iter()
            .chain(CHAOS_RANDOM_REQUIRED_COVERAGE.iter())
            .copied()
            .collect();
        let chaos_unique: BTreeSet<&str> = chaos_all.iter().copied().collect();
        assert_eq!(
            chaos_all.len(),
            41,
            "chaos random coverage gate must require 41 properties"
        );
        assert_eq!(
            chaos_unique.len(),
            chaos_all.len(),
            "chaos random coverage gate must not contain duplicate property keys"
        );

        let chaos_24_all: Vec<&str> = chaos_all
            .iter()
            .chain(CHAOS_SWARM_PROFILE_REQUIRED_COVERAGE.iter())
            .copied()
            .collect();
        let chaos_24_unique: BTreeSet<&str> = chaos_24_all.iter().copied().collect();
        assert_eq!(
            chaos_24_all.len(),
            47,
            "chaos 24-seed coverage gate must require 47 properties"
        );
        assert_eq!(
            chaos_24_unique.len(),
            chaos_24_all.len(),
            "chaos 24-seed coverage gate must not contain duplicate property keys"
        );

        let mut chaos_tracker = CoverageTracker::new();
        for key in &chaos_all {
            chaos_tracker.hit(key);
        }
        verify_coverage(&chaos_tracker, true, &[], CHAOS_RANDOM_REQUIRED_COVERAGE).unwrap();

        let mut chaos_incomplete = CoverageTracker::new();
        for key in chaos_all.iter().skip(1) {
            chaos_incomplete.hit(key);
        }
        assert!(
            verify_coverage(&chaos_incomplete, true, &[], CHAOS_RANDOM_REQUIRED_COVERAGE).is_err()
        );
    }

    #[test]
    fn parse_replay_lines_handles_inline_comments_and_corpus_file() {
        let e1 = parse_replay_line("chaos 6581024343186838594 40  # Bug #5: outbox", 40)
            .unwrap()
            .unwrap();
        assert_eq!(
            e1,
            ReplayEntry {
                scenario: Scenario::Chaos,
                seed: 6_581_024_343_186_838_594,
                ops: 40,
            }
        );

        let e2 = parse_replay_line("remote_faults 99 # no explicit ops", 55)
            .unwrap()
            .unwrap();
        assert_eq!(
            e2,
            ReplayEntry {
                scenario: Scenario::RemoteFaults,
                seed: 99,
                ops: 55,
            }
        );

        let corpus_path = resolve_corpus_path(Path::new(DEFAULT_CORPUS_FILE));
        let entries = load_replay_file(&corpus_path, 40).expect("load corpus/regressions.txt");
        assert!(
            entries.len() >= 14,
            "expected at least 14 corpus regression entries, got {}",
            entries.len()
        );
        for (seed, ops) in [
            (6_581_024_343_186_838_594_u64, 40_usize),
            (14_542_862_646_255_385_444, 50),
            (13_580_682_392_153_143_971, 50),
            (10_298_014_112_690_665_752, 50),
            (13_893_003_159_721_802_437, 50),
            (7_015_526_293_144_434_493, 60),
            (10_281_633_911_761_585_129, 60),
            (267_765_175_692_156_043, 60),
            (15_097_008_142_518_856_235, 60),
            (7_914_532_405_096_322_517, 60),
            (5_215_407_837_432_958_784, 60),
            (17_295_058_876_053_817_750, 60),
            (1_983_678_568_783_011_277, 60),
            (18_325_773_721_672_025_168, 60),
        ] {
            assert!(
                entries
                    .iter()
                    .any(|e| e.scenario == Scenario::Chaos && e.seed == seed && e.ops == ops),
                "expected regression entry chaos seed={seed} ops={ops} in corpus/regressions.txt"
            );
        }
    }

    #[test]
    fn verify_coverage_enforces_never_zero_and_vacuous_fault_gates() {
        let mut tracker = CoverageTracker::new();
        for key in BASE_REQUIRED_COVERAGE {
            tracker.hit(key);
        }
        tracker.add_fault_counts("herdr.after_terminal_run_before_db_commit", 2, 2);
        verify_coverage(&tracker, true, &[], &[]).expect("clean tracker passes");

        // Hitting `exec.blocked_command` must fail the `never!` gate.
        let mut with_blocked = tracker.clone();
        with_blocked.hit("exec.blocked_command");
        let err = verify_coverage(&with_blocked, true, &[], &[])
            .expect_err("exec.blocked_command > 0 must fail --assert-coverage");
        assert!(err.to_string().contains("exec.blocked_command"));

        // An armed-but-never-fired fault must fail the vacuous fault gate under `--assert-coverage`.
        let with_vacuous = tracker.clone();
        with_vacuous.arm_fault("git.after_push_before_batch_finalize");
        let err = verify_coverage(&with_vacuous, true, &[], &[])
            .expect_err("vacuous armed fault must fail --assert-coverage");
        assert!(
            err.to_string()
                .contains("git.after_push_before_batch_finalize")
        );
    }

    #[test]
    fn random_coverage_overview_reports_distinct_count_highlights_and_never_gate() {
        let mut tracker = CoverageTracker::new();
        tracker.hit("chaos.delivery_batch_entered");
        tracker.hit("chaos.delivery_batch_entered");
        tracker.hit("exec.shell_executed");
        let clean = format_random_coverage_overview(&tracker);
        assert!(clean.contains("2 distinct properties hit"), "{clean}");
        assert!(clean.contains("chaos.delivery_batch_entered"), "{clean}");
        assert!(clean.contains("exec.shell_executed"), "{clean}");
        assert!(
            clean.contains("Never-property gate: PASS (exec.blocked_command=0)"),
            "{clean}"
        );

        tracker.hit("exec.blocked_command");
        let violated = format_random_coverage_overview(&tracker);
        assert!(violated.contains("3 distinct properties hit"), "{violated}");
        assert!(
            violated.contains("Never-property gate: FAIL (exec.blocked_command=1)"),
            "{violated}"
        );
    }

    #[test]
    fn cli_parses_optional_report_json_on_run_random_and_corpus() {
        let cli_run = Cli::try_parse_from([
            "canban-sim",
            "run",
            "--seeds",
            "1",
            "--report-json",
            "out/gate-a.json",
        ])
        .expect("parse run with --report-json");
        match cli_run.command {
            Some(Command::Run { report_json, .. }) => {
                assert_eq!(report_json, Some(PathBuf::from("out/gate-a.json")));
            }
            _ => panic!("expected Run command"),
        }

        let cli_run_default =
            Cli::try_parse_from(["canban-sim", "run"]).expect("parse run without --report-json");
        match cli_run_default.command {
            Some(Command::Run { report_json, .. }) => assert_eq!(report_json, None),
            _ => panic!("expected Run command"),
        }

        let cli_random = Cli::try_parse_from([
            "canban-sim",
            "random",
            "--scenario",
            "chaos",
            "--duration",
            "300",
            "--report-json",
            "out/soak.json",
        ])
        .expect("parse random with --report-json");
        match cli_random.command {
            Some(Command::Random { report_json, .. }) => {
                assert_eq!(report_json, Some(PathBuf::from("out/soak.json")));
            }
            _ => panic!("expected Random command"),
        }

        let cli_corpus = Cli::try_parse_from([
            "canban-sim",
            "corpus",
            "--jobs",
            "4",
            "--report-json",
            "out/corpus.json",
        ])
        .expect("parse corpus with --report-json");
        match cli_corpus.command {
            Some(Command::Corpus { report_json, .. }) => {
                assert_eq!(report_json, Some(PathBuf::from("out/corpus.json")));
            }
            _ => panic!("expected Corpus command"),
        }
    }

    #[test]
    fn report_json_serialization_and_parent_dir_creation() {
        let mut tracker = CoverageTracker::new();
        for key in BASE_REQUIRED_COVERAGE {
            tracker.hit(key);
        }
        tracker.hit_n("chaos.delivery_batch_entered", 5);
        tracker.hit_n("exec.shell_executed", 9);
        tracker.hit_n("oracle.read_your_writes_checked", 42);
        tracker.hit_n("oracle.at_most_once_checked", 17);
        tracker.hit_n("oracle.committed_durability_checked", 11);
        tracker.hit_n("chaos.natural_quiesce_recovered", 6);
        tracker.add_fault_counts("herdr.after_terminal_run_before_db_commit", 3, 2);
        tracker.arm_fault("git.after_push_before_batch_finalize");

        let coverage = build_coverage_report(&tracker, true, &[], &[]);
        assert!(!coverage.coverage_gate_passed);
        assert!(coverage.never_gate_passed);
        assert_eq!(
            coverage.vacuous_faults,
            vec!["git.after_push_before_batch_finalize".to_owned()]
        );

        let err_text = "=== CANBAN DST FAILURE ===\nScenario: chaos\nSeed: 42\nOps: 60\nDigest: 0x0011223344556677\nReason: [primary] RYW VIOLATION: Card c9 regressed to rev 2 < 3\n";
        let shrunk = ShrunkReport {
            orig_ops: 60,
            shrunk_ops: 2,
            digest: Some("0x0011223344556677".into()),
            ops_lines: vec!["[00] CreateCard".into(), "[01] BounceClient".into()],
        };
        let fail_rep = build_failure_report("chaos", 42, 60, err_text.into(), false, Some(shrunk));
        assert!(
            fail_rep
                .profile
                .as_deref()
                .unwrap_or_default()
                .starts_with("delivery_and_storage (")
        );
        assert_eq!(fail_rep.digest.as_deref(), Some("0x0011223344556677"));
        assert_eq!(
            fail_rep.fingerprint,
            "RYW VIOLATION: Card # regressed to rev # < #"
        );

        let sim_report = SimReport {
            command: "random",
            scenario: "chaos".into(),
            master_seed: Some(2026),
            ops: 60,
            duration_secs: Some(300),
            elapsed_secs: 12.3456,
            passed: 19,
            failed: 1,
            timed_out: 0,
            total: 20,
            coverage,
            failures: vec![fail_rep],
        };

        let tmp = tempfile::tempdir().expect("tempdir");
        let report_path = tmp.path().join("nested").join("reports").join("soak.json");
        write_report_json(&report_path, &sim_report).expect("write report json");
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report_path).expect("read report"))
                .expect("parse report json");

        assert_eq!(parsed["command"], "random");
        assert_eq!(parsed["scenario"], "chaos");
        assert_eq!(parsed["master_seed"], 2026);
        assert_eq!(parsed["ops"], 60);
        assert_eq!(parsed["duration_secs"], 300);
        assert_eq!(parsed["passed"], 19);
        assert_eq!(parsed["failed"], 1);
        assert_eq!(parsed["timed_out"], 0);
        assert_eq!(parsed["total"], 20);
        assert_eq!(parsed["coverage"]["coverage_gate_passed"], false);
        assert_eq!(parsed["coverage"]["never_gate_passed"], true);
        assert_eq!(
            parsed["coverage"]["highlights"]["delivery_batch_entered"],
            5
        );
        assert_eq!(parsed["coverage"]["highlights"]["shell_executed"], 9);
        assert_eq!(
            parsed["coverage"]["highlights"]["read_your_writes_checked"],
            42
        );
        assert_eq!(parsed["coverage"]["highlights"]["at_most_once_checked"], 17);
        assert_eq!(
            parsed["coverage"]["highlights"]["natural_quiesce_recovered"],
            6
        );
        assert_eq!(
            parsed["coverage"]["armed_vs_fired"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(
            !parsed["coverage"]["cold_top_10"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let f0 = &parsed["failures"][0];
        assert_eq!(f0["scenario"], "chaos");
        assert_eq!(f0["seed"], 42);
        assert_eq!(f0["ops"], 60);
        assert_eq!(f0["timed_out"], false);
        assert_eq!(f0["digest"], "0x0011223344556677");
        assert_eq!(f0["shrunk"]["orig_ops"], 60);
        assert_eq!(f0["shrunk"]["shrunk_ops"], 2);
        assert_eq!(f0["shrunk"]["ops_lines"][0], "[00] CreateCard");
    }
}
