//! Deterministic Simulation Testing & Chaos CLI runner (`canban-sim`).
use anyhow::{Context, Result};
use canban_sim::scenarios::{self, Scenario};
use clap::{Parser, Subcommand};
use rand::Rng;
use std::{
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
    about = "Deterministic Simulation Testing (DST) and Chaos runner for Canban"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run all 8 deterministic scenarios (or one selected scenario) with reproducible seeds.
    Run {
        /// Optional scenario name (e.g. `remote_faults`, `github_sync`, `chaos`). Omit to run all 8.
        #[arg(long)]
        scenario: Option<String>,
        /// Starting seed (default: 2026).
        #[arg(long, default_value_t = 2026)]
        seed: u64,
        /// Number of consecutive seeds per scenario (default: 1).
        #[arg(long, default_value_t = 1)]
        seeds: u64,
        /// Operations per chaos run when `--scenario chaos` is selected.
        #[arg(long, default_value_t = 40)]
        ops: usize,
    },
    /// Continuously explore random seeds across the full-spectrum chaos workload until failure or limit.
    Chaos {
        /// Stop after N seeds (omit to run continuously until Ctrl-C or failure).
        #[arg(long)]
        seeds: Option<u64>,
        /// Stop after N seconds (optional wall-clock budget).
        #[arg(long)]
        duration: Option<u64>,
        /// Parallel worker threads (default: 1; use e.g. 4 for multi-core fuzzing).
        #[arg(long, default_value_t = 1)]
        jobs: usize,
        /// Number of random operations per chaos seed.
        #[arg(long, default_value_t = 40)]
        ops: usize,
        /// Directory where failure reproduction logs are written.
        #[arg(long, default_value = ".local/sim-failures")]
        trace_dir: PathBuf,
    },
    /// Replay a specific seed (from a previous failure or CI log) deterministically.
    Replay {
        /// Exact seed number to replay.
        seed: u64,
        /// Scenario to replay (default: `chaos`).
        #[arg(long, default_value = "chaos")]
        scenario: String,
        /// Number of random operations when replaying `chaos`.
        #[arg(long, default_value_t = 40)]
        ops: usize,
    },
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
    let path = dir.join(format!("seed-{seed}.log"));
    let body = format!(
        "=== CANBAN DST FAILURE ===\nScenario: {scenario}\nSeed: {seed}\nOps: {ops}\nReplay: mise run sim-replay -- {seed} --scenario {scenario} --ops {ops}\n\nError:\n{err:#}\n"
    );
    std::fs::write(&path, body).ok()?;
    Some(path)
}

fn run_suite(scenario: Option<String>, base_seed: u64, seeds: u64, ops: usize) -> Result<()> {
    let list: Vec<Scenario> = match scenario {
        Some(name) => vec![Scenario::parse(&name)?],
        None => Scenario::ALL_NAMED.to_vec(),
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
                    eprintln!(
                        "FAIL {:<16} seed={seed}\nReplay: mise run sim-replay -- {seed} --scenario {}\nError: {e:#}",
                        sc.name(),
                        sc.name()
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

fn run_chaos(
    max_seeds: Option<u64>,
    duration_secs: Option<u64>,
    jobs: usize,
    ops: usize,
    trace_dir: PathBuf,
) -> Result<()> {
    let workers = jobs.max(1);
    let started = Instant::now();
    let deadline = duration_secs.map(|s| started + Duration::from_secs(s));
    let completed = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicBool::new(false));
    let first_error: Arc<Mutex<Option<(u64, String)>>> = Arc::new(Mutex::new(None));

    println!(
        "Starting Canban DST chaos mode (jobs={workers}, ops={ops}, seeds={}, duration={})...",
        max_seeds
            .map(|n| n.to_string())
            .unwrap_or_else(|| "infinite".into()),
        duration_secs
            .map(|s| format!("{s}s"))
            .unwrap_or_else(|| "unlimited".into()),
    );

    std::thread::scope(|scope| {
        for worker_id in 0..workers {
            let completed = Arc::clone(&completed);
            let failed = Arc::clone(&failed);
            let first_error = Arc::clone(&first_error);
            let trace_dir = trace_dir.clone();
            scope.spawn(move || {
                let mut seed_rng = rand::rng();
                loop {
                    if failed.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Some(dl) = deadline
                        && Instant::now() >= dl
                    {
                        break;
                    }
                    let slot = completed.fetch_add(1, Ordering::SeqCst);
                    if let Some(limit) = max_seeds
                        && slot >= limit
                    {
                        completed.fetch_sub(1, Ordering::SeqCst);
                        break;
                    }
                    let seed: u64 = seed_rng.random();
                    let t0 = Instant::now();
                    match scenarios::chaos::run(seed, ops) {
                        Ok(summary) => {
                            let n = slot + 1;
                            println!(
                                "[w{worker_id} #{n:<5}] PASS seed={:<20} steps={:<5} faults={:<3} digest={:#018x} ({:.2}s)",
                                summary.seed,
                                summary.steps,
                                summary.faults,
                                summary.digest,
                                t0.elapsed().as_secs_f64(),
                            );
                        }
                        Err(e) => {
                            failed.store(true, Ordering::SeqCst);
                            let saved = write_failure_log(&trace_dir, seed, "chaos", ops, &e);
                            let msg = format!(
                                "CHAOS FAILURE on seed={seed}\nReplay: mise run sim-replay -- {seed} --scenario chaos --ops {ops}\n{}{e:#}",
                                saved
                                    .map(|p| format!("Saved log: {}\n", p.display()))
                                    .unwrap_or_default()
                            );
                            eprintln!("\n{msg}");
                            let mut guard = first_error.lock().unwrap();
                            if guard.is_none() {
                                *guard = Some((seed, msg));
                            }
                            break;
                        }
                    }
                }
            });
        }
    });

    if let Some((seed, msg)) = first_error.lock().unwrap().take() {
        anyhow::bail!("Chaos run failed on seed {seed}:\n{msg}");
    }
    let total = completed.load(Ordering::SeqCst);
    println!(
        "\nChaos completed cleanly: {total} random seed(s) passed in {:.2}s.",
        started.elapsed().as_secs_f64()
    );
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
        Some(Command::Chaos {
            seeds,
            duration,
            jobs,
            ops,
            trace_dir,
        }) => run_chaos(seeds, duration, jobs, ops, trace_dir),
        Some(Command::Replay {
            seed,
            scenario,
            ops,
        }) => {
            let sc = Scenario::parse(&scenario)?;
            let t0 = Instant::now();
            let digest = scenarios::run_scenario(sc, seed, ops)
                .with_context(|| format!("Replay failed for scenario={} seed={seed}", sc.name()))?;
            println!(
                "REPLAY PASS {:<16} seed={seed} digest={:#018x} ({:.2}s)",
                sc.name(),
                digest,
                t0.elapsed().as_secs_f64()
            );
            Ok(())
        }
    }
}
