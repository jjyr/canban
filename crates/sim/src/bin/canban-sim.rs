//! Deterministic Simulation Testing CLI runner (`canban-sim`).
use anyhow::{Result, bail};
use canban_sim::scenarios::{self, Scenario};
use clap::{Parser, Subcommand};
use rand::Rng;
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

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
    /// Run one complete deterministic simulation round (all 8 scenarios + chaos),
    /// or replay a specific `--seed` / `--scenario`.
    Run {
        /// Optional scenario name (e.g. `remote_faults`, `github_sync`, `chaos`).
        /// Omit to run one complete round (all 8 scenarios + chaos).
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
    },
    /// Continuously run random-seed simulations until Ctrl-C, printing failing seeds
    /// (pass `--fail-fast` to stop immediately on the first failure).
    Random {
        /// Stop immediately on the first failing seed instead of continuing until Ctrl-C.
        #[arg(long, short = 'x')]
        fail_fast: bool,
        /// Target scenario: `chaos` (default), `all` (random mix of all 8 + chaos), or a specific scenario name.
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
        /// Directory where failure logs and `failed-seeds.txt` are persisted.
        #[arg(long, default_value = ".local/sim-failures")]
        trace_dir: PathBuf,
    },
}

fn replay_cmd(seed: u64, scenario: &str, ops: usize) -> String {
    if scenario == "chaos" {
        format!("mise run sim -- --seed {seed} --scenario chaos --ops {ops}")
    } else {
        format!("mise run sim -- --seed {seed} --scenario {scenario}")
    }
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
    let path = dir.join(format!("seed-{seed}.log"));
    let body = format!(
        "=== CANBAN DST FAILURE ===\nScenario: {scenario}\nSeed: {seed}\nOps: {ops}\nReplay: {replay}\n\nError:\n{err:#}\n"
    );
    std::fs::write(&path, body).ok()?;
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

fn run_suite(scenario: Option<String>, base_seed: u64, seeds: u64, ops: usize) -> Result<()> {
    let list: Vec<Scenario> = match scenario {
        Some(name) => vec![Scenario::parse(&name)?],
        None => {
            let mut full = Scenario::ALL_NAMED.to_vec();
            full.push(Scenario::Chaos);
            full
        }
    };
    let started = Instant::now();
    let mut passed = 0u64;
    for sc in list {
        for offset in 0..seeds {
            let seed = base_seed.wrapping_add(offset);
            let t0 = Instant::now();
            match scenarios::run_scenario(sc, seed, ops) {
                Ok(digest) => {
                    passed += 1;
                    println!(
                        "PASS {:<16} seed={:<12} digest={:#018x} ({:.2}s)",
                        sc.name(),
                        seed,
                        digest,
                        t0.elapsed().as_secs_f64()
                    );
                }
                Err(e) => {
                    let replay = replay_cmd(seed, sc.name(), ops);
                    eprintln!(
                        "FAIL {:<16} seed={seed}\nReplay: {replay}\nError: {e:#}",
                        sc.name(),
                    );
                    return Err(e);
                }
            }
        }
    }
    println!(
        "\nAll {passed} deterministic simulation run(s) passed in {:.2}s.",
        started.elapsed().as_secs_f64()
    );
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
                // 50% full-spectrum chaos, 50% one of the 8 focused scenarios.
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

fn run_random(
    fail_fast: bool,
    scenario_arg: String,
    max_seeds: Option<u64>,
    duration_secs: Option<u64>,
    jobs: usize,
    ops: usize,
    trace_dir: PathBuf,
) -> Result<()> {
    let target = Arc::new(RandomTarget::parse(&scenario_arg)?);
    let workers = jobs.max(1);
    let started = Instant::now();
    let deadline = duration_secs.map(|s| started + Duration::from_secs(s));
    let claimed = Arc::new(AtomicU64::new(0));
    let passed_count = Arc::new(AtomicU64::new(0));
    let failed_count = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let failures: Arc<Mutex<Vec<(u64, String, String)>>> = Arc::new(Mutex::new(Vec::new()));

    spawn_ctrlc_watcher(Arc::clone(&stop));

    println!(
        "Starting Canban random-sim (scenario={scenario_arg}, jobs={workers}, ops={ops}, fail_fast={fail_fast}, seeds={}, duration={})...",
        max_seeds
            .map(|n| n.to_string())
            .unwrap_or_else(|| "infinite (Ctrl-C to stop)".into()),
        duration_secs
            .map(|s| format!("{s}s"))
            .unwrap_or_else(|| "unlimited".into()),
    );

    std::thread::scope(|scope| {
        for worker_id in 0..workers {
            let target = Arc::clone(&target);
            let claimed = Arc::clone(&claimed);
            let passed_count = Arc::clone(&passed_count);
            let failed_count = Arc::clone(&failed_count);
            let stop = Arc::clone(&stop);
            let failures = Arc::clone(&failures);
            let trace_dir = trace_dir.clone();
            scope.spawn(move || {
                let mut seed_rng = rand::rng();
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
                    let seed: u64 = seed_rng.random();
                    let sc = target.pick(&mut seed_rng);
                    let t0 = Instant::now();
                    let n = slot + 1;
                    match scenarios::run_scenario(sc, seed, ops) {
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
                                "\n[w{worker_id} #{n:<5}] FAIL {:<15} seed={seed}\n  Replay: {replay}\n{}  Error:  {e:#}\n",
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

    let passed = passed_count.load(Ordering::Relaxed);
    let failed = failed_count.load(Ordering::Relaxed);
    let elapsed = started.elapsed().as_secs_f64();
    let recorded = failures.lock().unwrap();

    println!(
        "\nrandom-sim summary: {passed} passed, {failed} failed ({} total) in {elapsed:.2}s.",
        passed + failed
    );
    if !recorded.is_empty() {
        eprintln!("Failed seed(s):");
        for (seed, sc, replay) in recorded.iter() {
            eprintln!("  - seed={seed} ({sc}) -> {replay}");
        }
        bail!("{failed} random-sim seed(s) failed");
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => run_suite(None, 2026, 1, 40),
        Some(Command::Run {
            scenario,
            seed,
            seeds,
            ops,
        }) => run_suite(scenario, seed, seeds, ops),
        Some(Command::Random {
            fail_fast,
            scenario,
            seeds,
            duration,
            jobs,
            ops,
            trace_dir,
        }) => run_random(fail_fast, scenario, seeds, duration, jobs, ops, trace_dir),
    }
}
