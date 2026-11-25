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
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

static FAILED_SEEDS_LOCK: Mutex<()> = Mutex::new(());

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
    /// Run one complete deterministic simulation round (all 16 scripted scenarios + chaos),
    /// or replay a specific `--seed` / `--scenario` or `--replay-file`.
    Run {
        /// Optional scenario name (e.g. `remote_faults`, `transport_auth`, `workspaces_delivery`, `project_import_export`, `sessions_and_adoption`, `review_and_events`, `chaos`).
        /// Omit to run one complete round (all 16 scripted scenarios + chaos).
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
        /// Target scenario: `chaos` (default), `all` (random mix of all 16 scripted + chaos), or a specific scenario name.
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
        /// Directory where failure logs and `failed-seeds.txt` are persisted.
        #[arg(long, default_value = ".local/sim-failures")]
        trace_dir: PathBuf,
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

struct WatchdogHandle {
    active: Arc<Mutex<BTreeMap<usize, ActiveSeed>>>,
    done: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WatchdogHandle {
    fn start(timeout_secs: Option<u64>, trace_dir: PathBuf) -> Self {
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

fn parse_replay_line(line: &str, default_ops: usize) -> Result<Option<ReplayEntry>> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
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
    "github.pr_published",
    "github.pr_refreshed",
    "github.pr_merged",
    "github.pr_remote_merge_reconciled",
    "drafts.conflict_and_persistence",
    "events.journal_listed",
    "events.subscription_delivery_and_ack",
    "events.subscription_crash_retry",
];

const CHAOS_RANDOM_REQUIRED_COVERAGE: &[&str] = &[
    "oracle.committed_durability_checked",
    "chaos.net_partition",
    "chaos.server_crash_bounce",
    "chaos.client_bounce",
    "chaos.env_save",
    "chaos.faulty_stream_chunk",
    "chaos.faulty_stream_rst",
    "chaos.faulty_stream_stall",
    "chaos.multi_client_race",
    "chaos.schedule_ops",
    "chaos.worktree_delivery_ops",
    "chaos.auth_revoke_recover",
    "chaos.project_archive_restore",
];

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
    println!(
        "Coverage gate PASSED: {}/{} required properties verified ({} total distinct properties hit).",
        required.len(),
        required.len(),
        tracker.counts().len(),
    );
    Ok(())
}

fn print_shrink_report(report: &scenarios::chaos::ShrinkReport) {
    eprintln!(
        "\n=== CHAOS SHRINK RESULT ===\nSeed:      {}\nMinimized: {} -> {} ops ({} attempts)\nError:     {}\nMinimal op sequence:",
        report.seed, report.original_len, report.minimized_len, report.attempts, report.error
    );
    for (idx, op) in report.ops.iter().enumerate() {
        eprintln!("  [{idx:02}] {op:?}");
    }
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
) -> Result<()> {
    let is_full_suite = replay_file.is_none() && scenario.is_none();
    let plan: Vec<ReplayEntry> = if let Some(path) = replay_file {
        load_replay_file(&path, ops)?
    } else {
        let list: Vec<Scenario> = match scenario {
            Some(name) => vec![Scenario::parse(&name)?],
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

    let watchdog = WatchdogHandle::start(seed_timeout, trace_dir.clone());
    let started = Instant::now();
    let mut passed = 0u64;
    let mut failures: Vec<(u64, &'static str, usize, String)> = Vec::new();
    let mut total_coverage = CoverageTracker::new();

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
        total_coverage.merge(&cov);

        match res {
            Ok(digest) => {
                passed += 1;
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
                if shrink_on_fail
                    && entry.scenario == Scenario::Chaos
                    && let Ok(Some(report)) = scenarios::chaos::shrink(entry.seed, entry.ops)
                {
                    print_shrink_report(&report);
                }
                if !continue_on_error {
                    return Err(e);
                }
                failures.push((entry.seed, entry.scenario.name(), entry.ops, replay));
            }
        }
    }

    drop(watchdog);

    if !failures.is_empty() {
        eprintln!(
            "\nSimulation suite finished with {passed} passed, {} failed in {:.2}s:",
            failures.len(),
            started.elapsed().as_secs_f64()
        );
        for (seed, sc, sc_ops, replay) in &failures {
            eprintln!("  - seed={seed} ({sc}, ops={sc_ops}) -> {replay}");
        }
        bail!("{} deterministic simulation run(s) failed", failures.len());
    }

    println!(
        "\nAll {passed} deterministic simulation run(s) passed in {:.2}s.",
        started.elapsed().as_secs_f64()
    );
    let suite_req: &[&str] = if is_full_suite {
        FULL_SUITE_REQUIRED_COVERAGE
    } else {
        &[]
    };
    verify_coverage(
        &total_coverage,
        assert_coverage,
        &require_coverage,
        suite_req,
    )?;
    Ok(())
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
                // 50% full-spectrum chaos, 50% one of the 16 focused scenarios.
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
    trace_dir: PathBuf,
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
    let failures: Arc<Mutex<Vec<(u64, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let total_coverage: Arc<Mutex<CoverageTracker>> = Arc::new(Mutex::new(CoverageTracker::new()));

    let watchdog = Arc::new(WatchdogHandle::start(seed_timeout, trace_dir.clone()));
    spawn_ctrlc_watcher(Arc::clone(&stop));

    println!(
        "Starting Canban random-sim (scenario={scenario_arg}, jobs={workers}, ops={ops}, master_seed={resolved_master_seed}, fail_fast={fail_fast}, seeds={}, duration={}, seed_timeout={})...",
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
                                    .map(|p| format!("  Log:    {}\n", p.display()))
                                    .unwrap_or_default()
                            );
                            failures.lock().unwrap().push((
                                seed,
                                sc.name().to_owned(),
                                replay,
                            ));
                            if fail_fast {
                                stop.store(true, Ordering::SeqCst);
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

    println!(
        "\nrandom-sim summary: {passed} passed, {failed} failed ({} total, master_seed={resolved_master_seed}) in {elapsed:.2}s.",
        passed + failed
    );
    if !recorded.is_empty() {
        eprintln!("Failed seed(s):");
        for (seed, sc, replay) in recorded.iter() {
            eprintln!("  - seed={seed} ({sc}) -> {replay}");
        }
        bail!("{failed} random-sim seed(s) failed");
    }
    let cov = total_coverage.lock().unwrap();
    let mut random_req: Vec<&str> = Vec::new();
    if scenario_arg.trim().eq_ignore_ascii_case("chaos") && passed >= 10 {
        random_req.extend_from_slice(CHAOS_RANDOM_REQUIRED_COVERAGE);
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
    verify_coverage(&cov, assert_coverage, &require_coverage, &random_req)?;
    Ok(())
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
            trace_dir,
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
            trace_dir,
        ),
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
    fn full_suite_coverage_gate_requires_69_unique_properties() {
        use std::collections::BTreeSet;

        let all: Vec<&str> = BASE_REQUIRED_COVERAGE
            .iter()
            .chain(FULL_SUITE_REQUIRED_COVERAGE.iter())
            .copied()
            .collect();
        let unique: BTreeSet<&str> = all.iter().copied().collect();
        assert_eq!(
            all.len(),
            69,
            "full suite coverage gate must require 69 properties"
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

        let chaos_all: Vec<&str> = BASE_REQUIRED_COVERAGE
            .iter()
            .chain(CHAOS_RANDOM_REQUIRED_COVERAGE.iter())
            .copied()
            .collect();
        let chaos_unique: BTreeSet<&str> = chaos_all.iter().copied().collect();
        assert_eq!(
            chaos_all.len(),
            18,
            "chaos random coverage gate must require 18 properties"
        );
        assert_eq!(
            chaos_unique.len(),
            chaos_all.len(),
            "chaos random coverage gate must not contain duplicate property keys"
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
}
