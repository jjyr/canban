//! Swarm-Testing Randomized Chaos DST + Deterministic Shrinker (`chaos::shrink`):
//! - Per-seed randomized `ChaosProfile` (Desktop vs Mobile, clock skews, latencies, fail rates,
//!   enabled fault families, workflow+GitHub co-location, `max_agents`, `FaultyStream` byte-stream
//!   faults, 2-client concurrent races, cron schedules, worktree+script+delivery+git conflicts,
//!   live `device.revoke`+re-pair, secondary `project.archive`/`restore`, `FsDurability::Full`
//!   + `sqlite_synchronous_off: false` profile slice, and operation weights).
//! - Deterministic materialized `Vec<ChaosOp>` enabling exact replay & prefix+ddmin shrinking.
//! - Explicit expected-error classifier (no blanket `let _ =`), never discards unexpected
//!   `SyncItemState::Failed`, exercises ALL `SyncAction` variants (`Retry`, `UseMine`, `UseEdited`,
//!   `Discard` on expected `Conflict` only), fixed `env.save` object payload with offline secret
//!   tracking and convergence checks, and `oracle.check_step_invariants` after EVERY op!
use crate::{
    exec::{SimGitRepo, SimShellOutcome},
    github::SimIssue,
    herdr::RunFault,
    net::StreamFaultConfig,
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{FsDurability, PRIMARY_CLIENT, SIM_PORT, SimWorld, SimWorldConfig, run_with_world},
};
use anyhow::{Context, Result, bail, ensure};
use canban_types::{EnvEntryMeta, HostTransport, SavedHost, SyncAction, SyncItemState};
use rand::{Rng, SeedableRng, rngs::StdRng};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

const SECONDARY_CLIENT: &str = "ui-client-2";

const CHAOS_WORKFLOW_YAML: &str = r#"schema: 12
id: chaos-flow
name: Chaos Flow
workspace:
  create_worktree: false
  sync_base: false
on:
  start:
    event: manual
    start_at: plan
steps:
  - id: plan
    uses: agent
    with:
      profile: codex
      instruction: Execute chaos step
    next: approve
  - id: approve
    uses: approval
    with:
      title: Chaos Approval
    next: finish
    on_changes_requested: plan
  - id: finish
    uses: complete
"#;

const CHAOS_DELIVERY_WORKFLOW_YAML: &str = r#"schema: 12
id: chaos-flow
name: Chaos Delivery Flow
workspace:
  create_worktree: true
  sync_base: false
on:
  start:
    event: manual
    start_at: chaos_precheck
steps:
  - id: chaos_precheck
    stage: develop
    uses: script
    with:
      run: echo chaos-precheck
    next: plan
    on_failure: script_gate
  - id: script_gate
    stage: develop
    uses: approval
    with:
      title: Chaos Precheck Gate
    next: plan
    on_changes_requested: chaos_precheck
  - id: plan
    stage: develop
    uses: agent
    with:
      profile: codex
      instruction: Execute chaos step in worktree
    next: approve
  - id: approve
    stage: review
    uses: approval
    with:
      title: Chaos Approval
    next: ship
    on_changes_requested: plan
  - id: ship
    stage: review
    uses: deliver
    with:
      target_branch: main
      strategy: rebase
      sync_upstream: false
      batch_size: 2
      test_command: sim-chaos-test --workspace
      max_conflict_attempts: 2
    next: finish
    on_changes_requested: plan
  - id: finish
    stage: review
    uses: complete
"#;

const CHAOS_CRON_YAML: &str = r#"schema: 12
id: chaos-cron
name: Chaos Cron Flow
workspace:
  create_worktree: false
  sync_base: false
on:
  cron_card:
    cron: "* * * * *"
    timezone: UTC
    overlap: skip
    start_at: cron_work
    card:
      column: backlog
      title: "Chaos Cron Card #{{run_number}}"
      body: "Scheduled under chaos"
  cron_queue:
    cron: "* * * * *"
    timezone: UTC
    overlap: queue
    start_at: cron_work
steps:
  - id: cron_work
    uses: agent
    with:
      profile: codex
      instruction: Run scheduled chaos task
    next: cron_done
  - id: cron_done
    uses: complete
"#;

/// Per-seed Swarm-Testing profile randomizing topology, fault envelopes, and op weights.
#[derive(Clone, Debug)]
pub struct ChaosProfile {
    pub desktop_local: bool,
    pub remote_hosts: Vec<String>,
    pub host_clock_skew_ms: BTreeMap<String, i64>,
    pub min_latency_ms: u64,
    pub max_latency_ms: u64,
    pub base_fail_rate: f64,
    pub max_agents: usize,
    pub enable_partitions: bool,
    pub enable_crashes: bool,
    pub enable_client_bounce: bool,
    pub enable_herdr_faults: bool,
    pub enable_github_faults: bool,
    pub enable_workflows: bool,
    pub enable_reclaim_jumps: bool,
    pub enable_env_sync: bool,
    pub enable_clock_faults: bool,
    pub colocate_workflow_and_github: bool,
    pub enable_stream_faults: bool,
    pub enable_multi_client: bool,
    pub enable_schedules: bool,
    pub enable_worktree_delivery: bool,
    pub enable_auth_faults: bool,
    pub enable_project_archive: bool,
    pub full_durability: bool,
    pub weights: [u32; 30],
}

impl ChaosProfile {
    pub fn from_seed(seed: u64) -> Self {
        let mut rng = StdRng::seed_from_u64(seed ^ 0xA5A5_5A5A_C0DE_CAFE);
        let desktop_local = rng.random_bool(0.65);
        let two_remotes = !desktop_local || rng.random_bool(0.8);
        let remote_hosts: Vec<String> = if two_remotes {
            vec!["remote-a".into(), "remote-b".into()]
        } else {
            vec!["remote-a".into()]
        };
        let mut host_clock_skew_ms = BTreeMap::new();
        for h in &remote_hosts {
            host_clock_skew_ms.insert(h.clone(), rng.random_range(-15_000..=15_000));
        }
        let min_latency_ms = rng.random_range(1..=10);
        let max_latency_ms = min_latency_ms + rng.random_range(10..=50);
        let base_fail_rate = if rng.random_bool(0.3) {
            rng.random_range(0.01..=0.05)
        } else {
            0.0
        };
        let max_agents = rng.random_range(1..=4);

        // Swarm-testing toggles: most features on, occasionally disabled to concentrate
        // deeper stress onto the remaining active subsystems.
        let enable_partitions = rng.random_bool(0.90);
        let enable_crashes = rng.random_bool(0.85);
        let enable_client_bounce = rng.random_bool(0.80);
        let enable_herdr_faults = rng.random_bool(0.90);
        let enable_github_faults = rng.random_bool(0.85);
        let enable_workflows = rng.random_bool(0.90);
        let enable_reclaim_jumps = rng.random_bool(0.75);
        let enable_env_sync = desktop_local && rng.random_bool(0.90);
        let enable_clock_faults = rng.random_bool(0.85);
        let colocate_workflow_and_github = rng.random_bool(0.60);
        let enable_stream_faults = rng.random_bool(0.80);
        let enable_multi_client = rng.random_bool(0.55);
        let enable_schedules = rng.random_bool(0.60);
        let enable_worktree_delivery = enable_workflows && rng.random_bool(0.55);
        let enable_auth_faults = rng.random_bool(0.50);
        let enable_project_archive = rng.random_bool(0.55);
        // Deterministic full-durability slice (`FsDurability::Full` + `PRAGMA synchronous = FULL`)
        // on 1/8th of seeds so CI stays fast while verifying real fsync paths under chaos.
        let full_durability = seed.is_multiple_of(8);

        let mut weights = [0u32; 30];
        for w in &mut weights {
            *w = rng.random_range(1..=5);
        }
        // Keep core mutations & newly expanded chaos seams well-represented.
        weights[0] += 3; // CreateCard
        weights[1] += 3; // UpdateCard
        weights[4] += 2; // AddComment
        weights[24] += 3; // StreamFault
        weights[25] += 2; // MultiClientRace
        weights[26] += 2; // ScheduleOp
        weights[27] += 2; // WorktreeDeliveryOp
        weights[28] += 2; // AuthRevokeRecover
        weights[29] += 2; // ProjectArchiveRestore

        if !enable_env_sync {
            weights[10] = 0;
        }
        if !enable_workflows {
            weights[11] = 0;
            weights[12] = 0;
            weights[13] = 0;
            weights[14] = 0;
        }
        if !enable_github_faults {
            weights[17] = 0;
        }
        if !enable_herdr_faults {
            weights[18] = 0;
        }
        if !enable_partitions {
            weights[19] = 0;
        }
        if !enable_crashes {
            weights[20] = 0;
        }
        if !enable_client_bounce {
            weights[21] = 0;
        }
        if !enable_clock_faults {
            weights[23] = 0;
        }
        if !enable_stream_faults {
            weights[24] = 0;
        }
        if !enable_multi_client {
            weights[25] = 0;
        }
        if !enable_schedules {
            weights[26] = 0;
        }
        if !enable_worktree_delivery {
            weights[27] = 0;
        }
        if !enable_auth_faults {
            weights[28] = 0;
        }
        if !enable_project_archive {
            weights[29] = 0;
        }

        Self {
            desktop_local,
            remote_hosts,
            host_clock_skew_ms,
            min_latency_ms,
            max_latency_ms,
            base_fail_rate,
            max_agents,
            enable_partitions,
            enable_crashes,
            enable_client_bounce,
            enable_herdr_faults,
            enable_github_faults,
            enable_workflows,
            enable_reclaim_jumps,
            enable_env_sync,
            enable_clock_faults,
            colocate_workflow_and_github,
            enable_stream_faults,
            enable_multi_client,
            enable_schedules,
            enable_worktree_delivery,
            enable_auth_faults,
            enable_project_archive,
            full_durability,
            weights,
        }
    }

    pub fn hosts(&self) -> Vec<String> {
        let mut hosts = Vec::new();
        if self.desktop_local {
            hosts.push("local".into());
        }
        hosts.extend(self.remote_hosts.iter().cloned());
        hosts
    }
}

/// Deterministic materialized operation for Chaos DST replay and shrinking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChaosOp {
    CreateCard {
        host_idx: usize,
        tag: u32,
    },
    UpdateCard {
        card_pick: usize,
        tag: u32,
    },
    MoveCard {
        card_pick: usize,
        stage_idx: usize,
    },
    ArchiveCard {
        card_pick: usize,
    },
    AddComment {
        card_pick: usize,
        tag: u32,
    },
    ListCards {
        host_idx: usize,
    },
    ReadTimeline {
        card_pick: usize,
    },
    OpenAndCheckCard {
        card_pick: usize,
    },
    CreateShellTerminal {
        host_idx: usize,
        tag: u32,
    },
    EndShellTerminal {
        term_pick: usize,
    },
    SaveGlobalEnv {
        tag: u32,
        with_secret: bool,
    },
    StartWorkflow {
        card_pick: usize,
        tag: u32,
    },
    DecideWorkflow {
        run_pick: usize,
        approve: bool,
        tag: u32,
    },
    StopWorkflow {
        run_pick: usize,
    },
    ReportExecution {
        term_pick: usize,
        state_idx: usize,
        tag: u32,
    },
    ViewerPresence {
        card_pick: usize,
        attach: bool,
    },
    NotificationsReadAll,
    GitHubUpsertAndSync {
        issue_num: u64,
        is_pr: bool,
        fault_kind: u8,
    },
    HerdrFault {
        host_idx: usize,
        kind: u8,
        tag: u32,
    },
    NetworkFault {
        remote_idx: usize,
        kind: u8,
        second_client: bool,
    },
    CrashAndBounceServer {
        host_idx: usize,
    },
    BounceClient {
        second_client: bool,
    },
    AdvanceTime {
        secs: u64,
    },
    ClockFault {
        host_idx: usize,
        kind: u8,
        delta_ms: i64,
        drift_ppm: i64,
    },
    /// Byte-stream fault injection via `FaultyStream` (1-byte chunking, read/write RST,
    /// EOF truncation, transient half-open read/write stalls, and explicit recovery).
    StreamFault {
        remote_idx: usize,
        kind: u8,
        byte_limit: u64,
        mutate: bool,
        card_pick: usize,
        tag: u32,
    },
    /// Overlapping 2-client concurrent RPC races (`ui-client` vs `ui-client-2` via `client_spawn_on`).
    MultiClientRace {
        kind: u8,
        remote_idx: usize,
        card_pick: usize,
        stage_idx: usize,
        tag: u32,
    },
    /// Cron schedule operations (`schedule.trigger`, `schedule.toggle`, `schedule.list`, and run completion).
    ScheduleOp {
        kind: u8,
        sched_pick: usize,
        enable: bool,
        tag: u32,
    },
    /// Worktree + `uses: script` / `uses: deliver` + git conflict + `batch.report` operations.
    WorktreeDeliveryOp {
        kind: u8,
        card_pick: usize,
        tag: u32,
    },
    /// Live `device.revoke` + re-pair recovery exercising `HostConnectionState::AuthFailed`.
    AuthRevokeRecover {
        recover: bool,
        tag: u32,
    },
    /// Secondary project `project.archive` / `project.archived.list` / `project.restore` + `note.save`.
    ProjectArchiveRestore {
        archive: bool,
        tag: u32,
    },
}

/// Result of deterministic shrinking for a failing Chaos seed.
#[derive(Clone, Debug)]
pub struct ShrinkReport {
    pub seed: u64,
    pub original_len: usize,
    pub minimized_len: usize,
    pub attempts: usize,
    pub ops: Vec<ChaosOp>,
    pub error: String,
}

/// Generate a deterministic `(ChaosProfile, Vec<ChaosOp>)` for `(seed, max_ops)`.
pub fn generate_ops(seed: u64, max_ops: usize) -> (ChaosProfile, Vec<ChaosOp>) {
    let profile = ChaosProfile::from_seed(seed);
    let mut rng = StdRng::seed_from_u64(seed ^ 0x517C_C1B7_2722_0A95);
    let total_weight: u32 = profile.weights.iter().sum::<u32>().max(1);
    let mut ops = Vec::with_capacity(max_ops);

    for step in 0..max_ops {
        let tag = step as u32 + 1;
        let mut roll = rng.random_range(0..total_weight);
        let mut kind = 0usize;
        for (idx, &w) in profile.weights.iter().enumerate() {
            if roll < w {
                kind = idx;
                break;
            }
            roll -= w;
        }
        let op = match kind {
            0 => ChaosOp::CreateCard {
                host_idx: rng.random_range(0..16),
                tag,
            },
            1 => ChaosOp::UpdateCard {
                card_pick: rng.random_range(0..64),
                tag,
            },
            2 => ChaosOp::MoveCard {
                card_pick: rng.random_range(0..64),
                stage_idx: rng.random_range(0..3),
            },
            3 => ChaosOp::ArchiveCard {
                card_pick: rng.random_range(0..64),
            },
            4 => ChaosOp::AddComment {
                card_pick: rng.random_range(0..64),
                tag,
            },
            5 => ChaosOp::ListCards {
                host_idx: rng.random_range(0..16),
            },
            6 => ChaosOp::ReadTimeline {
                card_pick: rng.random_range(0..64),
            },
            7 => ChaosOp::OpenAndCheckCard {
                card_pick: rng.random_range(0..64),
            },
            8 => ChaosOp::CreateShellTerminal {
                host_idx: rng.random_range(0..16),
                tag,
            },
            9 => ChaosOp::EndShellTerminal {
                term_pick: rng.random_range(0..32),
            },
            10 => ChaosOp::SaveGlobalEnv {
                tag,
                with_secret: rng.random_bool(0.75),
            },
            11 => ChaosOp::StartWorkflow {
                card_pick: rng.random_range(0..64),
                tag,
            },
            12 => ChaosOp::DecideWorkflow {
                run_pick: rng.random_range(0..32),
                approve: rng.random_bool(0.65),
                tag,
            },
            13 => ChaosOp::StopWorkflow {
                run_pick: rng.random_range(0..32),
            },
            14 => ChaosOp::ReportExecution {
                term_pick: rng.random_range(0..32),
                state_idx: rng.random_range(0..3),
                tag,
            },
            15 => ChaosOp::ViewerPresence {
                card_pick: rng.random_range(0..64),
                attach: rng.random_bool(0.6),
            },
            16 => ChaosOp::NotificationsReadAll,
            17 => ChaosOp::GitHubUpsertAndSync {
                issue_num: rng.random_range(1..=12),
                is_pr: rng.random_bool(0.35),
                fault_kind: rng.random_range(0..9),
            },
            18 => ChaosOp::HerdrFault {
                host_idx: rng.random_range(0..16),
                kind: rng.random_range(0..16),
                tag,
            },
            19 => ChaosOp::NetworkFault {
                remote_idx: rng.random_range(0..16),
                kind: rng.random_range(0..5),
                second_client: profile.enable_multi_client && rng.random_bool(0.35),
            },
            20 => ChaosOp::CrashAndBounceServer {
                host_idx: rng.random_range(0..16),
            },
            21 => ChaosOp::BounceClient {
                second_client: profile.enable_multi_client && rng.random_bool(0.40),
            },
            22 => {
                let secs = if profile.enable_reclaim_jumps && rng.random_bool(0.35) {
                    rng.random_range(65..=95)
                } else {
                    rng.random_range(1..=6)
                };
                ChaosOp::AdvanceTime { secs }
            }
            23 => ChaosOp::ClockFault {
                host_idx: rng.random_range(0..16),
                kind: rng.random_range(0..2),
                delta_ms: rng.random_range(-4_000..=6_000),
                drift_ppm: rng.random_range(-5_000..=5_000),
            },
            24 => ChaosOp::StreamFault {
                remote_idx: rng.random_range(0..16),
                kind: rng.random_range(0..7),
                byte_limit: rng.random_range(16..=96),
                mutate: rng.random_bool(0.45),
                card_pick: rng.random_range(0..64),
                tag,
            },
            25 => ChaosOp::MultiClientRace {
                kind: rng.random_range(0..5),
                remote_idx: rng.random_range(0..16),
                card_pick: rng.random_range(0..64),
                stage_idx: rng.random_range(0..3),
                tag,
            },
            26 => ChaosOp::ScheduleOp {
                kind: rng.random_range(0..4),
                sched_pick: rng.random_range(0..16),
                enable: rng.random_bool(0.55),
                tag,
            },
            27 => ChaosOp::WorktreeDeliveryOp {
                kind: rng.random_range(0..5),
                card_pick: rng.random_range(0..64),
                tag,
            },
            28 => ChaosOp::AuthRevokeRecover {
                recover: rng.random_bool(0.55),
                tag,
            },
            _ => ChaosOp::ProjectArchiveRestore {
                archive: rng.random_bool(0.55),
                tag,
            },
        };
        ops.push(op);
    }

    (profile, ops)
}

/// Explicit expected-error classifier for Chaos RPCs:
/// allows ONLY expected operational errors under partitions, races, crashes, or injected faults,
/// and fails fast on schema/serialization/unknown-method/internal invariant bugs!
fn check_expected_chaos_err(op: &str, res: Result<Value>) -> Result<Option<Value>> {
    match res {
        Ok(val) => Ok(Some(val)),
        Err(err) => {
            let msg = format!("{err:#}").to_ascii_lowercase();
            let expected = [
                "offline",
                "unavailable",
                "connecting",
                "reconnecting",
                "partition",
                "disconnected",
                "not connected",
                "connection",
                "timed out",
                "timeout",
                "broken pipe",
                "reset by peer",
                "unexpected eof",
                "closed",
                "unreachable",
                "not reachable",
                "channel",
                "auth_failed",
                "revoked",
                "unauthorized",
                "re-authentication",
                "revision",
                "stale",
                "conflict",
                "archived",
                "removed",
                "restore the card before editing",
                "restore the card before commenting",
                "terminal has no core binding",
                "terminal has no runtime binding",
                "not found",
                "unknown card",
                "unknown terminal",
                "unknown workflow",
                "unknown project",
                "already active",
                "already has an active",
                "already running",
                "already completed",
                "already stopped",
                "already ended",
                "already exists",
                "resolve the existing human decision",
                "not waiting",
                "not active",
                "workflow run is not",
                "batch is not",
                "live terminal",
                "unresolved",
                "cherry-pick",
                "worktree is dirty",
                "schedule is disabled",
                "terminal finished",
                "finished",
                "rate limit",
                "403",
                "429",
                "500",
                "503",
                "simulated github",
                "simulated herdr",
                "herdr",
                "no live",
                "cursor_stale",
            ]
            .iter()
            .any(|needle| msg.contains(needle));
            if expected {
                Ok(None)
            } else {
                bail!("Unexpected non-operational error during chaos op `{op}`: {err:#}");
            }
        }
    }
}

/// Entrypoint for Chaos DST (`canban-sim --scenario chaos`).
pub fn run(seed: u64, ops: usize) -> Result<u64> {
    let (profile, seq) = generate_ops(seed, ops);
    run_ops(seed, &profile, &seq)
}

/// Execute an explicit `(profile, ops)` sequence deterministically.
pub fn run_ops(seed: u64, profile: &ChaosProfile, ops: &[ChaosOp]) -> Result<u64> {
    let mut cfg = if profile.desktop_local {
        SimWorldConfig::desktop_default(seed, "chaos")
    } else {
        SimWorldConfig::mobile_default(seed, "chaos")
    };
    cfg.remote_hosts = profile.remote_hosts.clone();
    cfg.host_clock_skew_ms = profile.host_clock_skew_ms.clone();
    cfg.min_latency_ms = profile.min_latency_ms;
    cfg.max_latency_ms = profile.max_latency_ms;
    // Keep network clean during deterministic world bootstrap; apply `profile.base_fail_rate`
    // right before entering the chaos operation loop.
    cfg.fail_rate = 0.0;
    if profile.enable_multi_client {
        cfg.extra_clients = vec![SECONDARY_CLIENT.into()];
    }
    if profile.full_durability {
        cfg.fs_durability = FsDurability::Full;
        cfg.sqlite_synchronous_off = false;
    }

    run_with_world(cfg, |world| {
        world.trace.set_ops(ops.len());
        world.trace.mix_str(if profile.desktop_local {
            "profile:desktop"
        } else {
            "profile:mobile"
        });
        world.hit(if profile.desktop_local {
            "chaos.profile.desktop"
        } else {
            "chaos.profile.mobile"
        });
        if profile.colocate_workflow_and_github {
            world.hit("chaos.profile.colocated_wf_github");
        }
        if profile.full_durability {
            world.hit("chaos.full_durability_profile");
        }

        let mut oracle = InvariantOracle::default();
        let all_hosts = profile.hosts();
        let remotes = profile.remote_hosts.clone();

        // Track partition/offline/auth state cleanly so we know when offline secret writes occur
        // and guarantee strict auth & archive recovery before quiescence.
        let mut partitioned_remotes: BTreeSet<String> = BTreeSet::new();
        let mut offline_herdr_hosts: BTreeSet<String> = BTreeSet::new();

        // Workflow host is always `remote-a` (present in both desktop & mobile topologies).
        let wf_host = "remote-a".to_owned();
        let mut wf_dir = String::new();

        // Create one active project per host in deterministic order.
        let mut host_projects: Vec<(String, String)> = Vec::new();
        for host in &all_hosts {
            let dir = world.project_dir(&format!("chaos-{host}"))?;
            if host == &wf_host {
                wf_dir = dir.clone();
                if profile.enable_worktree_delivery {
                    let exec = world.exec_runner(&wf_host).unwrap();
                    exec.register_git_repo(&dir, SimGitRepo::default());
                    exec.set_script_default(
                        "chaos_precheck",
                        SimShellOutcome::ok("chaos precheck passed\n")
                            .with_output("verdict", "green"),
                    );
                    exec.set_shell_default(
                        "sim-chaos-test",
                        SimShellOutcome::ok("chaos batch tests passed\n"),
                    );
                    if seed.is_multiple_of(2) {
                        exec.enqueue_script_outcomes(
                            "chaos_precheck",
                            [SimShellOutcome::fail(
                                2,
                                "chaos precheck attempt 1\n",
                                "simulated precheck lint fail\n",
                            )],
                        );
                    }
                }
            }
            let mut create_params = json!({
                "host_id": host,
                "name": format!("Chaos {host}"),
                "directory": dir,
            });
            if host == &wf_host && profile.enable_worktree_delivery {
                create_params["target_branch"] = json!("main");
            }
            let p = world.client_call("project.create", create_params, None)?;
            let pid = p["id"].as_str().unwrap().to_owned();
            // Configure fast idle reclaim (1 min) + per-seed agent budget!
            let _ = world.client_call(
                "scheduler.configure",
                json!({ "max_agents": profile.max_agents, "idle_minutes": 1 }),
                Some(&pid),
            )?;
            host_projects.push((host.clone(), pid));
        }

        // Install chaos workflow(s) (normal or worktree+script+deliver, plus optional cron schedule)
        // on `remote-a`.
        let wf_pid = host_projects
            .iter()
            .find(|(h, _)| h == &wf_host)
            .map(|(_, p)| p.clone())
            .unwrap();
        let mut schedule_entries: Vec<(String, String)> = Vec::new(); // (schedule_id, rule_id)
        if profile.enable_workflows || profile.enable_schedules {
            let sources = world.client_call(
                "workflow.sources",
                json!({ "project_id": wf_pid }),
                Some(&wf_pid),
            )?;
            let mut yaml_sources = Vec::new();
            if profile.enable_workflows {
                let wf_yaml = if profile.enable_worktree_delivery {
                    CHAOS_DELIVERY_WORKFLOW_YAML
                } else {
                    CHAOS_WORKFLOW_YAML
                };
                yaml_sources.push(json!({ "filename": "chaos-flow.yaml", "source": wf_yaml }));
            }
            if profile.enable_schedules {
                yaml_sources
                    .push(json!({ "filename": "chaos-cron.yaml", "source": CHAOS_CRON_YAML }));
            }
            let _ = world.client_call(
                "workflow.apply",
                json!({
                    "project_id": wf_pid,
                    "revision": sources["revision"],
                    "sources": yaml_sources,
                }),
                Some(&wf_pid),
            )?;
            if profile.enable_schedules {
                let sched_list = world.client_call(
                    "schedule.list",
                    json!({ "project_id": wf_pid }),
                    Some(&wf_pid),
                )?;
                if let Some(items) = sched_list["schedules"].as_array() {
                    for item in items {
                        if let (Some(sid), Some(rid)) =
                            (item["id"].as_str(), item["rule_id"].as_str())
                        {
                            schedule_entries.push((sid.to_owned(), rid.to_owned()));
                        }
                    }
                }
            }
        }

        // Optional live `device.revoke` + re-pair setup: give `ui-client` its own dedicated paired
        // device token on `auth_host` so revoking it hits `ui-client` cleanly without breaking
        // `ui-client-2`'s independent session.
        let auth_host = remotes
            .last()
            .cloned()
            .unwrap_or_else(|| "remote-a".to_owned());
        let mut auth_paired_device_id: Option<String> = None;
        let mut auth_revoked = false;
        if profile.enable_auth_faults && !remotes.is_empty() {
            let dev_id = pair_host_on_primary_client(world, &auth_host)?;
            auth_paired_device_id = Some(dev_id);
        }

        // Optional secondary project dedicated to `project.archive` / `project.restore` + `note.save`
        // while all primary `host_projects` remain continuously active!
        let archive_host = if remotes.len() >= 2 {
            "remote-b".to_owned()
        } else if profile.desktop_local {
            "local".to_owned()
        } else {
            "remote-a".to_owned()
        };
        let mut archive_proj: Option<(String, String, String, String, Option<String>)> = None;
        // (host, qualified_pid, raw_pid, raw_card_id, raw_note_id)
        let mut archive_proj_archived = false;
        if profile.enable_project_archive {
            let arch_dir = world.project_dir("chaos-archive-secondary")?;
            let ap = world.client_call(
                "project.create",
                json!({
                    "host_id": archive_host,
                    "name": format!("Chaos Secondary {seed}"),
                    "directory": arch_dir,
                }),
                None,
            )?;
            let arch_pid = ap["id"].as_str().unwrap().to_owned();
            let arch_raw_pid = strip_host_id(&arch_pid);
            let arch_card = world.client_call(
                "card.create",
                json!({
                    "project_id": arch_pid,
                    "title": format!("Secondary Archive Card {seed}"),
                    "body": "must survive archive/restore",
                }),
                Some(&arch_pid),
            )?;
            let arch_cid = arch_card["id"].as_str().unwrap().to_owned();
            let arch_raw_cid = strip_host_id(&arch_cid);
            oracle.record_committed_card(
                server_key(&archive_host),
                &arch_raw_cid,
                &arch_raw_pid,
                arch_card["revision"].as_u64().unwrap_or(1),
            );
            let arch_note = world.client_call(
                "note.save",
                json!({
                    "project_id": arch_pid,
                    "title": "Chaos Archive Note",
                    "body": format!("seed-{seed}-initial-note"),
                }),
                Some(&arch_pid),
            )?;
            let arch_raw_nid = arch_note["id"].as_str().map(strip_host_id);
            if let Some(ref nid) = arch_raw_nid {
                oracle.record_committed_note(
                    server_key(&archive_host),
                    nid,
                    &arch_raw_pid,
                    arch_note["revision"].as_u64().unwrap_or(1),
                );
            }
            archive_proj = Some((
                archive_host.clone(),
                arch_pid,
                arch_raw_pid,
                arch_raw_cid,
                arch_raw_nid,
            ));
        }

        // Bind simulated GitHub repo — optionally CO-LOCATED on `remote-a` alongside workflows!
        let gh_host = if profile.colocate_workflow_and_github || remotes.len() == 1 {
            "remote-a".to_owned()
        } else {
            "remote-b".to_owned()
        };
        let gh_pid = host_projects
            .iter()
            .find(|(h, _)| h == &gh_host)
            .map(|(_, p)| p.clone())
            .unwrap();
        let gh_repo = "acme/chaos-repo";
        if profile.enable_github_faults {
            world.github.add_repo(gh_repo, 9001, "main");
            world
                .github
                .set_collaborators(gh_repo, vec!["alice".into()]);
            world.github.upsert_issue(
                gh_repo,
                SimIssue {
                    number: 1,
                    title: "Seeded Chaos Issue".into(),
                    body: "Initial issue".into(),
                    state: "open".into(),
                    author: "alice".into(),
                    labels: vec!["chaos".into()],
                    updated_at: "2026-06-01T00:00:00Z".into(),
                    is_pr: false,
                    draft: false,
                    merged: false,
                    head_sha: String::new(),
                    head_ref: String::new(),
                    base_ref: String::new(),
                    check_conclusion: None,
                },
            );
            let _ = world.client_call(
                "project.github",
                json!({
                    "project_id": gh_pid,
                    "repository": gh_repo,
                    "intake": "all",
                    "pr_intake": "all",
                    "include_drafts": true,
                }),
                Some(&gh_pid),
            )?;
        }

        // Deterministic creation-ordered entity lists (NEVER indexed by random UUID sort order!).
        // TrackedCard records whether the ID is a real server-confirmed ID (`shared = true`)
        // or a client-local optimistic offline temp ID (`shared = false`, valid only on `creator`
        // while still present in `creator`'s merged_snapshot before outbox rewrite).
        let mut cards: Vec<TrackedCard> = Vec::new();
        let mut shell_terms: Vec<(String, String)> = Vec::new(); // (pid, qualified_tid)
        let mut tracked_wf_runs: Vec<(String, String, String)> = Vec::new(); // (host, pid, qualified_run_id)
        let mut archived_cards: BTreeSet<String> = BTreeSet::new();

        // Seed 1 initial card per host so early ops always have targets.
        for (idx, (host, pid)) in host_projects.iter().enumerate() {
            let c = world.client_call(
                "card.create",
                json!({
                    "project_id": pid,
                    "title": format!("Seed Card {idx}"),
                    "body": "initial",
                }),
                Some(pid),
            )?;
            let cid = c["id"].as_str().unwrap().to_owned();
            oracle.record_committed_card(
                server_key(host),
                &cid,
                pid,
                c["revision"].as_u64().unwrap_or(1),
            );
            cards.push(TrackedCard {
                host: host.clone(),
                pid: pid.clone(),
                cid,
                creator: PRIMARY_CLIENT,
                shared: true,
            });
        }

        world.advance(Duration::from_millis(200))?;
        oracle.check_step_invariants(world)?;

        if profile.base_fail_rate > 0.0 {
            for r in &remotes {
                world.set_link_fail_rate(PRIMARY_CLIENT, r, profile.base_fail_rate);
                if profile.enable_multi_client {
                    world.set_link_fail_rate(SECONDARY_CLIENT, r, profile.base_fail_rate);
                }
            }
        }

        let stages = ["backlog", "develop", "review"];
        let mut env_secret_saved_count = 0u32;

        for (step_idx, op) in ops.iter().enumerate() {
            world.trace.mix_u64(step_idx as u64);

            match op {
                ChaosOp::CreateCard { host_idx, tag } => {
                    let (host, pid) = &host_projects[*host_idx % host_projects.len()];
                    if let Some(res) = check_expected_chaos_err(
                        "card.create",
                        world.client_call(
                            "card.create",
                            json!({
                                "project_id": pid,
                                "title": format!("Chaos Card #{tag}"),
                                "body": format!("seed-{seed}-tag-{tag}"),
                            }),
                            Some(pid),
                        ),
                    )? && let Some(cid) = res["id"].as_str()
                    {
                        let srv_name = server_key(host);
                        let raw_cid = canban_types::split_qualified_host_id(cid)
                            .map(|(_, r)| r)
                            .unwrap_or(cid);
                        // Only record durability when confirmed on server (not offline-queued).
                        let confirmed = if let Some(srv_card) = world
                            .server(srv_name)
                            .and_then(|s| s.snapshot().ok())
                            .and_then(|snap| snap.cards.into_iter().find(|c| c.id == raw_cid))
                        {
                            oracle.record_committed_card(
                                srv_name,
                                &srv_card.id,
                                &srv_card.project_id,
                                srv_card.revision,
                            );
                            true
                        } else {
                            false
                        };
                        cards.push(TrackedCard {
                            host: host.clone(),
                            pid: pid.clone(),
                            cid: cid.to_owned(),
                            creator: PRIMARY_CLIENT,
                            shared: confirmed,
                        });
                    }
                }
                ChaosOp::UpdateCard { card_pick, tag } => {
                    if let Some((_host, pid, cid)) =
                        pick_active_card(world, PRIMARY_CLIENT, &cards, &archived_cards, *card_pick)
                    {
                        let rev = lookup_card_rev(world, &cid);
                        let _ = check_expected_chaos_err(
                            "card.update",
                            world.client_call(
                                "card.update",
                                json!({
                                    "card_id": cid,
                                    "revision": rev,
                                    "title": format!("Updated #{tag}"),
                                    "body": format!("body-{tag}"),
                                }),
                                Some(&pid),
                            ),
                        )?;
                    }
                }
                ChaosOp::MoveCard {
                    card_pick,
                    stage_idx,
                } => {
                    if let Some((_host, pid, cid)) =
                        pick_active_card(world, PRIMARY_CLIENT, &cards, &archived_cards, *card_pick)
                    {
                        let rev = lookup_card_rev(world, &cid);
                        let stage = stages[*stage_idx % stages.len()];
                        let _ = check_expected_chaos_err(
                            "card.move",
                            world.client_call(
                                "card.move",
                                json!({ "card_id": cid, "revision": rev, "stage": stage }),
                                Some(&pid),
                            ),
                        )?;
                    }
                }
                ChaosOp::ArchiveCard { card_pick } => {
                    // Always preserve at least 1 active server-confirmed card per host.
                    let active_count = cards
                        .iter()
                        .filter(|c| c.shared && !archived_cards.contains(&c.cid))
                        .count();
                    if active_count > host_projects.len()
                        && let Some((host, pid, cid)) = pick_active_card(
                            world,
                            PRIMARY_CLIENT,
                            &cards,
                            &archived_cards,
                            *card_pick,
                        )
                    {
                        let rev = lookup_card_rev(world, &cid);
                        let srv_name = server_key(&host);
                        let raw_cid = canban_types::split_qualified_host_id(&cid)
                            .map(|(_, r)| r.to_owned())
                            .unwrap_or_else(|| cid.clone());
                        let archived_ok = check_expected_chaos_err(
                            "card.archive",
                            world.client_call(
                                "card.archive",
                                json!({ "card_id": cid, "revision": rev }),
                                Some(&pid),
                            ),
                        )?
                        .is_some();
                        // Under link loss, `card.archive` can commit on the server and drop the
                        // reply packet; inspect server state so chaos doesn't treat the card as active.
                        let server_archived = world
                            .server(srv_name)
                            .and_then(|s| s.snapshot().ok())
                            .and_then(|snap| snap.cards.into_iter().find(|c| c.id == raw_cid))
                            .is_some_and(|c| !c.active());
                        if archived_ok || server_archived {
                            archived_cards.insert(cid);
                        }
                    }
                }
                ChaosOp::AddComment { card_pick, tag } => {
                    if let Some((host, pid, cid)) =
                        pick_active_card(world, PRIMARY_CLIENT, &cards, &archived_cards, *card_pick)
                    {
                        let body = format!("chaos-comment-{tag}");
                        if check_expected_chaos_err(
                            "comment.add",
                            world.client_call(
                                "comment.add",
                                json!({ "card_id": cid, "body": body }),
                                Some(&pid),
                            ),
                        )?
                        .is_some()
                        {
                            let srv_name = server_key(&host);
                            let raw_cid = strip_host_id(&cid);
                            if world
                                .server(srv_name)
                                .and_then(|s| s.events().ok())
                                .is_some_and(|evs| {
                                    evs.iter().any(|e| {
                                        e.card_id == raw_cid
                                            && e.kind == "comment.created"
                                            && (e.data["body"].as_str() == Some(body.as_str())
                                                || e.message == body)
                                    })
                                })
                            {
                                oracle.record_committed_comment(srv_name, &raw_cid, body);
                            }
                        }
                    }
                }
                ChaosOp::ListCards { host_idx } => {
                    let (_host, pid) = &host_projects[*host_idx % host_projects.len()];
                    let _ = check_expected_chaos_err(
                        "card.list",
                        world.client_call(
                            "card.list",
                            json!({ "project_id": pid, "limit": 5 }),
                            Some(pid),
                        ),
                    )?;
                    world.hit("chaos.card_list");
                }
                ChaosOp::ReadTimeline { card_pick } => {
                    if let Some((_host, pid, cid)) =
                        pick_active_card(world, PRIMARY_CLIENT, &cards, &archived_cards, *card_pick)
                    {
                        let _ = check_expected_chaos_err(
                            "timeline.list",
                            world.client_call(
                                "timeline.list",
                                json!({ "card_id": cid, "tail": true, "limit": 30, "view": "full" }),
                                Some(&pid),
                            ),
                        )?;
                    }
                }
                ChaosOp::OpenAndCheckCard { card_pick } => {
                    if let Some((host, pid, cid)) =
                        pick_active_card(world, PRIMARY_CLIENT, &cards, &archived_cards, *card_pick)
                    {
                        let visible = world
                            .client_merged_snapshot()?
                            .cards
                            .iter()
                            .any(|c| c.id == cid && c.active());
                        if visible {
                            let res = InvariantOracle::assert_card_complete_and_readable(
                                world, &cid, &pid,
                            )
                            .map(|_| Value::Null);
                            if auth_revoked && host == auth_host {
                                // A revoked host rejects routed `card.get` cleanly with `auth_failed`.
                                let _ = check_expected_chaos_err("open_card(revoked)", res)?;
                            } else {
                                let _ = res?;
                            }
                        }
                    }
                }
                ChaosOp::CreateShellTerminal { host_idx, tag } => {
                    let (_host, pid) = &host_projects[*host_idx % host_projects.len()];
                    if let Some(res) = check_expected_chaos_err(
                        "terminal.create",
                        world.client_call(
                            "terminal.create",
                            json!({ "project_id": pid, "name": format!("Chaos Shell {tag}") }),
                            Some(pid),
                        ),
                    )? && let Some(tid) = res["id"].as_str()
                    {
                        let tid = tid.to_owned();
                        if world
                            .client_merged_snapshot()?
                            .terminals
                            .iter()
                            .any(|t| t.id == tid)
                        {
                            InvariantOracle::assert_terminal_in_project(world, &tid, pid)?;
                        }
                        shell_terms.push((pid.clone(), tid));
                    }
                }
                ChaosOp::EndShellTerminal { term_pick } => {
                    if !shell_terms.is_empty() {
                        let idx = *term_pick % shell_terms.len();
                        let (pid, tid) = shell_terms.remove(idx);
                        // Verify close-fallback priority invariant before ending!
                        let snap = world.client_merged_snapshot()?;
                        let rows = canban_ui::terminals::entries(&snap);
                        if rows.iter().any(|e| e.saved.id == tid) {
                            let _ = InvariantOracle::assert_close_fallback_priority(
                                &snap,
                                &rows,
                                &BTreeSet::new(),
                                &tid,
                            )?;
                        }
                        let _ = check_expected_chaos_err(
                            "terminal.end",
                            world.client_call(
                                "terminal.end",
                                json!({ "terminal_id": tid }),
                                Some(&pid),
                            ),
                        )?;
                    }
                }
                ChaosOp::SaveGlobalEnv { tag, with_secret } => {
                    if profile.desktop_local {
                        let mut entries = serde_json::Map::new();
                        entries.insert(
                            "CHAOS_PLAIN".into(),
                            json!({ "kind": "plain", "value": format!("val-{tag}") }),
                        );
                        if *with_secret {
                            let sec = format!("sk-chaos-{seed}-{tag}");
                            oracle.track_secret(sec.clone());
                            entries.insert(
                                "CHAOS_SECRET".into(),
                                json!({ "kind": "secret", "value": sec }),
                            );
                            env_secret_saved_count += 1;
                            if !partitioned_remotes.is_empty() {
                                oracle.mark_secret_queued_offline();
                            }
                        }
                        let expected_plain = EnvEntryMeta::Plain {
                            value: format!("val-{tag}"),
                        };
                        if check_expected_chaos_err(
                            "env.save",
                            world.client_call(
                                "env.save",
                                json!({
                                    "scope": "global",
                                    "target_id": "",
                                    "entries": Value::Object(entries),
                                }),
                                None,
                            ),
                        )?
                        .is_some()
                            && world
                                .server("local-server")
                                .and_then(|s| s.snapshot().ok())
                                .is_some_and(|snap| {
                                    snap.global_env.get("CHAOS_PLAIN") == Some(&expected_plain)
                                })
                        {
                            oracle.record_committed_env(
                                "local-server",
                                "CHAOS_PLAIN",
                                expected_plain,
                            );
                        }
                        let _ = check_expected_chaos_err(
                            "env.sync",
                            world.client_sync_global_env_now().map(|()| Value::Null),
                        )?;
                        world.hit("chaos.env_save");
                    }
                }
                ChaosOp::StartWorkflow { card_pick, tag } => {
                    if profile.enable_workflows {
                        let wf_cards: Vec<_> = cards
                            .iter()
                            .filter(|c| {
                                c.shared && c.host == wf_host && !archived_cards.contains(&c.cid)
                            })
                            .collect();
                        if !wf_cards.is_empty() {
                            let tc = wf_cards[*card_pick % wf_cards.len()];
                            let (pid, cid) = (&tc.pid, &tc.cid);
                            if let Some(res) = check_expected_chaos_err(
                                "workflow.start",
                                world.client_call(
                                    "workflow.start",
                                    json!({
                                        "workflow_id": "chaos-flow",
                                        "card_id": cid,
                                        "entry": "start",
                                        "request_key": format!("chaos-wf-{seed}-{tag}"),
                                    }),
                                    Some(pid),
                                ),
                            )? && let Some(rid) = res["id"].as_str()
                            {
                                let q_rid = if rid.contains("::") {
                                    rid.to_owned()
                                } else {
                                    canban_types::qualify_host_id(&wf_host, rid)
                                };
                                let raw_rid = strip_host_id(&q_rid);
                                if let Some(srv_run) = world
                                    .server(&wf_host)
                                    .and_then(|s| s.snapshot().ok())
                                    .and_then(|snap| {
                                        snap.workflow_runs.into_iter().find(|r| r.id == raw_rid)
                                    })
                                {
                                    oracle.record_committed_workflow_run(
                                        &wf_host,
                                        &srv_run.id,
                                        &srv_run.project_id,
                                        &srv_run.workflow_id,
                                        srv_run.visit,
                                    );
                                }
                                tracked_wf_runs.push((wf_host.clone(), pid.clone(), q_rid));
                                world.hit("chaos.workflow_start");
                            }
                        }
                    }
                }
                ChaosOp::DecideWorkflow {
                    run_pick,
                    approve,
                    tag,
                } => {
                    if let Some(srv) = world.server(&wf_host) {
                        let snap = srv.snapshot()?;
                        let mut waiting: Vec<_> = snap
                            .workflow_runs
                            .iter()
                            .filter(|r| r.state == "waiting_approval")
                            .collect();
                        waiting.sort_by_key(|r| (r.visit, r.definition_revision));
                        if !waiting.is_empty() {
                            let wf = waiting[*run_pick % waiting.len()];
                            if let Some(raw_cid) = wf.card_id.as_deref()
                                && let Some(card) = snap.cards.iter().find(|c| c.id == raw_cid)
                            {
                                let q_rid = canban_types::qualify_host_id(&wf_host, &wf.id);
                                let decision = if *approve { "approve" } else { "rework" };
                                let _ = check_expected_chaos_err(
                                    "workflow.decide",
                                    world.client_call(
                                        "workflow.decide",
                                        json!({
                                            "run_id": q_rid,
                                            "visit": wf.visit,
                                            "revision": card.revision,
                                            "decision": decision,
                                            "feedback": "chaos decision",
                                            "request_key": format!("chaos-dec-{seed}-{tag}"),
                                        }),
                                        Some(&wf_pid),
                                    ),
                                )?;
                                world.hit("chaos.workflow_decide");
                            }
                        }
                    }
                }
                ChaosOp::StopWorkflow { run_pick } => {
                    if !tracked_wf_runs.is_empty() {
                        let (_, pid, q_rid) = &tracked_wf_runs[*run_pick % tracked_wf_runs.len()];
                        let _ = check_expected_chaos_err(
                            "workflow.stop",
                            world.client_call(
                                "workflow.stop",
                                json!({ "run_id": q_rid }),
                                Some(pid),
                            ),
                        )?;
                    }
                }
                ChaosOp::ReportExecution {
                    term_pick,
                    state_idx,
                    tag,
                } => {
                    if let Some(srv) = world.server(&wf_host) {
                        let snap = srv.snapshot()?;
                        let mut running: Vec<_> = snap
                            .terminals
                            .iter()
                            .filter(|t| {
                                t.agent
                                    .as_ref()
                                    .is_some_and(|a| a.state == "running" && a.terminal.is_some())
                            })
                            .collect();
                        running.sort_by_key(|t| t.created_at);
                        if !running.is_empty() {
                            let t = running[*term_pick % running.len()];
                            let states = ["working", "completed", "blocked"];
                            let state = states[*state_idx % states.len()];
                            // When worktree delivery is active and an agent completes, commit its
                            // simulated work inside the card's worktree first so `uses: deliver`
                            // enters real `merge_batch` integration!
                            let mut finished_addr: Option<String> = None;
                            if state == "completed"
                                && profile.enable_worktree_delivery
                                && let Some(agent) = t.agent.as_ref()
                            {
                                if let Some(wt_path) = snap
                                    .cards
                                    .iter()
                                    .find(|c| c.id == agent.card_id)
                                    .and_then(|c| c.worktree.as_deref())
                                    && Path::new(wt_path).is_dir()
                                    && let Some(exec) = world.exec_runner(&wf_host)
                                {
                                    let _ = exec.commit_in_worktree(
                                        wt_path,
                                        &format!("feat: chaos commit {tag}"),
                                    );
                                }
                                finished_addr = agent.terminal.clone();
                            }
                            let _ = check_expected_chaos_err(
                                "execution.report",
                                world.server_call(
                                    &wf_host,
                                    "execution.report",
                                    json!({
                                        "terminal_id": t.id,
                                        "state": state,
                                        "message": format!("chaos report {tag}"),
                                        "request_key": format!("chaos-rep-{seed}-{tag}"),
                                    }),
                                ),
                            )?;
                            if let Some(addr) = finished_addr
                                && let Some(herdr) = world.herdr(&wf_host)
                            {
                                herdr.finish_terminal(&addr, 0, "exited");
                            }
                            world.hit("chaos.execution_report");
                        }
                    }
                }
                ChaosOp::ViewerPresence { card_pick, attach } => {
                    if let Some((_host, pid, cid)) =
                        pick_active_card(world, PRIMARY_CLIENT, &cards, &archived_cards, *card_pick)
                    {
                        let payload = if *attach {
                            json!({
                                "client_id": "chaos-viewer",
                                "card_id": cid,
                                "terminal_id": null,
                            })
                        } else {
                            json!({
                                "client_id": "chaos-viewer",
                                "card_id": null,
                                "terminal_id": null,
                            })
                        };
                        let _ = check_expected_chaos_err(
                            "viewer.presence",
                            world.client_call("viewer.presence", payload, Some(&pid)),
                        )?;
                        world.hit("chaos.viewer_presence");
                    }
                }
                ChaosOp::NotificationsReadAll => {
                    let _ = check_expected_chaos_err(
                        "notifications.read_all",
                        world.client_call("notifications.read_all", json!({}), None),
                    )?;
                }
                ChaosOp::GitHubUpsertAndSync {
                    issue_num,
                    is_pr,
                    fault_kind,
                } => {
                    if profile.enable_github_faults {
                        world.github.upsert_issue(
                            gh_repo,
                            SimIssue {
                                number: *issue_num,
                                title: format!("Chaos GH #{issue_num}"),
                                body: format!("seed-{seed}"),
                                state: "open".into(),
                                author: if *issue_num % 3 == 0 {
                                    "mallory".into()
                                } else {
                                    "alice".into()
                                },
                                labels: vec!["chaos".into()],
                                updated_at: format!(
                                    "2026-06-01T00:{:02}:{:02}Z",
                                    (step_idx / 60) % 60,
                                    step_idx % 60
                                ),
                                is_pr: *is_pr,
                                draft: false,
                                merged: false,
                                head_sha: if *is_pr {
                                    format!("sha-{issue_num}-{step_idx}")
                                } else {
                                    String::new()
                                },
                                head_ref: if *is_pr {
                                    format!("feat/{issue_num}")
                                } else {
                                    String::new()
                                },
                                base_ref: if *is_pr { "main".into() } else { String::new() },
                                check_conclusion: is_pr.then(|| "success".into()),
                            },
                        );
                        match fault_kind {
                            1 => world.github.inject_error(500, vec![]),
                            2 => world
                                .github
                                .inject_stale_response_once("/issues", json!([])),
                            3 => world.github.inject_secondary_rate_limit(429, 10),
                            5 => world.github.inject_stale_response_once("/pulls", json!([])),
                            6 => {
                                world.github.inject_io_error(
                                    std::io::ErrorKind::ConnectionReset,
                                    "simulated github io reset",
                                );
                                world.hit("chaos.github_io_error");
                            }
                            7 => {
                                world.github.set_host_offline(&gh_host, true);
                                world.hit("chaos.github_host_offline");
                            }
                            8 => {
                                let reset_secs = (world.now_ms() / 1000).saturating_add(5);
                                world.github.set_host_rate_limit_budget(
                                    &gh_host,
                                    Some(1),
                                    reset_secs,
                                );
                                world.hit("chaos.github_rate_limit_budget");
                            }
                            _ => {
                                // Ensure transient host-offline and rate-limit GitHub faults recover on clean ops.
                                world.github.set_host_offline(&gh_host, false);
                                world.github.set_host_rate_limit_budget(&gh_host, None, 0);
                                world.github.set_rate_limit_budget(None, 0);
                            }
                        }
                        let _ = check_expected_chaos_err(
                            "github.sync.request",
                            world.client_call(
                                "github.sync.request",
                                json!({ "project_id": gh_pid }),
                                Some(&gh_pid),
                            ),
                        )?;
                        world.hit("chaos.github_sync");
                    }
                }
                ChaosOp::HerdrFault {
                    host_idx,
                    kind,
                    tag,
                } => {
                    let host = &all_hosts[*host_idx % all_hosts.len()];
                    let srv_name = server_key(host);
                    if let Some(herdr) = world.herdr(srv_name) {
                        let addrs = herdr.terminal_addresses();
                        match kind {
                            0..=3 => {
                                if let Some(addr) = addrs.first() {
                                    let st = match kind {
                                        0 => "working",
                                        1 => "idle",
                                        2 => "done",
                                        _ => "blocked",
                                    };
                                    herdr.set_status(addr, st, Some(u64::from(*tag)));
                                }
                            }
                            4 => {
                                if let Some(addr) = addrs.first() {
                                    let stale = herdr.seq().saturating_sub(2).max(1);
                                    herdr.inject_stale_event(addr, stale, "error");
                                }
                            }
                            5 => herdr.emit_missed_snapshot(),
                            6 => herdr.drop_subscribers(),
                            7 => {
                                let now_off = !offline_herdr_hosts.contains(host);
                                if now_off {
                                    offline_herdr_hosts.insert(host.clone());
                                } else {
                                    offline_herdr_hosts.remove(host);
                                }
                                herdr.set_offline(now_off);
                                world.hit("chaos.herdr_offline_toggle");
                            }
                            8 => {
                                herdr.restart_core(format!("chaos-core-{host}-{seed}-{tag}"));
                                world.hit("chaos.herdr_restart_core");
                            }
                            9 => {
                                let fault = match tag % 3 {
                                    0 => RunFault::FailBeforeCreate(
                                        "simulated herdr run failure".into(),
                                    ),
                                    1 => RunFault::OrphanAfterCreate(
                                        "simulated herdr orphan timeout".into(),
                                    ),
                                    _ => RunFault::FirstMessageUnsent,
                                };
                                herdr.inject_next_run_fault(fault);
                                world.hit("chaos.herdr_run_fault");
                            }
                            10 => {
                                herdr.drop_next_events(1);
                                world.hit("chaos.herdr_drop_events");
                            }
                            11 => {
                                herdr.duplicate_next_events(1);
                                world.hit("chaos.herdr_dup_events");
                            }
                            12 => {
                                if let Some(addr) = addrs.first() {
                                    herdr.buffer_next_events_for_reorder(2);
                                    herdr.set_status(addr, "working", Some(u64::from(*tag)));
                                    herdr.set_status(addr, "idle", Some(u64::from(*tag) + 1));
                                    herdr.flush_reordered_events();
                                    world.hit("chaos.herdr_reorder_events");
                                }
                            }
                            13 => {
                                if let Some(addr) = addrs.first() {
                                    herdr.simulate_subscriber_overflow(addr);
                                    world.hit("chaos.herdr_sub_overflow");
                                }
                            }
                            14 => {
                                if let Some(addr) = addrs.first() {
                                    herdr.set_hang_kill(addr, true);
                                    world.hit("chaos.herdr_hang_kill");
                                }
                            }
                            _ => {
                                // Only lose a live terminal when Herdr is online and event delivery
                                // is clean so the observer immediately observes the `lost` event.
                                if !offline_herdr_hosts.contains(host)
                                    && let Some(addr) = addrs.first()
                                {
                                    herdr.clear_injected_faults();
                                    herdr.lose_terminal(addr, "simulated_pty_lost");
                                    world.advance(Duration::from_millis(200))?;
                                    world.hit("chaos.herdr_lose_terminal");
                                }
                            }
                        }
                    }
                }
                ChaosOp::NetworkFault {
                    remote_idx,
                    kind,
                    second_client,
                } => {
                    if !remotes.is_empty() {
                        let target = &remotes[*remote_idx % remotes.len()];
                        let client_name = if *second_client && profile.enable_multi_client {
                            SECONDARY_CLIENT
                        } else {
                            PRIMARY_CLIENT
                        };
                        match kind {
                            0 => {
                                world.partition(client_name, target);
                                if client_name == PRIMARY_CLIENT {
                                    partitioned_remotes.insert(target.clone());
                                }
                                world.hit("chaos.net_partition");
                            }
                            1 => {
                                world.partition_oneway(target, client_name);
                                if client_name == PRIMARY_CLIENT {
                                    partitioned_remotes.insert(target.clone());
                                }
                                world.hit("chaos.net_oneway");
                            }
                            2 => {
                                world.repair(client_name, target);
                                if client_name == PRIMARY_CLIENT {
                                    partitioned_remotes.remove(target);
                                }
                            }
                            3 => {
                                world.hold(client_name, target);
                                world.advance(Duration::from_millis(60))?;
                                world.release(client_name, target);
                            }
                            _ => {
                                world.set_link_fail_rate(client_name, target, 0.05);
                                world.advance(Duration::from_millis(80))?;
                                world.set_link_fail_rate(
                                    client_name,
                                    target,
                                    profile.base_fail_rate,
                                );
                            }
                        }
                    }
                }
                ChaosOp::CrashAndBounceServer { host_idx } => {
                    // Can crash ANY host (`remote-a`, `remote-b`, AND `local-server` when desktop!).
                    let target = &all_hosts[*host_idx % all_hosts.len()];
                    let srv_name = server_key(target);
                    if world.server(srv_name).is_some() {
                        world.crash_server(srv_name)?;
                        world.advance(Duration::from_millis(120))?;
                        world.bounce_server(srv_name)?;
                        world.hit("chaos.server_crash_bounce");
                        if target == "local" {
                            world.hit("chaos.local_server_bounce");
                        }
                    }
                }
                ChaosOp::BounceClient { second_client } => {
                    if *second_client && profile.enable_multi_client {
                        world.crash_client_named(SECONDARY_CLIENT)?;
                        world.advance(Duration::from_millis(120))?;
                        world.bounce_client_named(SECONDARY_CLIENT)?;
                    } else {
                        world.crash_client()?;
                        world.advance(Duration::from_millis(120))?;
                        world.bounce_client()?;
                    }
                    world.hit("chaos.client_bounce");
                }
                ChaosOp::AdvanceTime { secs } => {
                    if *secs >= 60 {
                        // Step in bounded chunks so background idle reclaimer & cron evaluators tick cleanly.
                        let mut remaining = *secs;
                        while remaining > 0 {
                            let chunk = remaining.min(15);
                            world.advance(Duration::from_secs(chunk))?;
                            remaining -= chunk;
                        }
                        world.hit("chaos.reclaim_time_jump");
                    } else {
                        world.advance(Duration::from_secs(*secs))?;
                    }
                }
                ChaosOp::ClockFault {
                    host_idx,
                    kind,
                    delta_ms,
                    drift_ppm,
                } => {
                    let host = &all_hosts[*host_idx % all_hosts.len()];
                    let srv_name = server_key(host);
                    if let Some(handle) = world.servers.get(srv_name) {
                        if *kind == 0 {
                            handle.clock.jump_ms(*delta_ms);
                            world.hit("chaos.clock_jump");
                        } else {
                            handle.clock.set_drift_ppm(*drift_ppm);
                            world.hit("chaos.clock_drift");
                        }
                    }
                }
                ChaosOp::StreamFault {
                    remote_idx,
                    kind,
                    byte_limit,
                    mutate,
                    card_pick,
                    tag,
                } => {
                    if profile.enable_stream_faults && !remotes.is_empty() {
                        let target = remotes[*remote_idx % remotes.len()].clone();
                        let target_pid = host_projects
                            .iter()
                            .find(|(h, _)| h == &target)
                            .map(|(_, p)| p.clone())
                            .unwrap();
                        let target_card = cards
                            .iter()
                            .filter(|c| {
                                c.shared && c.host == target && !archived_cards.contains(&c.cid)
                            })
                            .nth(
                                *card_pick
                                    % cards
                                        .iter()
                                        .filter(|c| {
                                            c.shared
                                                && c.host == target
                                                && !archived_cards.contains(&c.cid)
                                        })
                                        .count()
                                        .max(1),
                            )
                            .map(|c| c.cid.clone());
                        let can_drive_live = !partitioned_remotes.contains(&target)
                            && !(auth_revoked && target == auth_host);

                        let spawn_probe =
                            |w: &mut SimWorld| -> Result<crate::world::ClientCallHandle> {
                                if *mutate && let Some(ref cid) = target_card {
                                    w.client_spawn(
                                        "comment.add",
                                        json!({
                                            "card_id": cid,
                                            "body": format!("stream-fault-comment-{tag}"),
                                        }),
                                        Some(&target_pid),
                                    )
                                } else {
                                    w.client_spawn(
                                        "card.list",
                                        json!({ "project_id": target_pid, "limit": 5 }),
                                        Some(&target_pid),
                                    )
                                }
                            };

                        match kind % 7 {
                            0 => {
                                // 1-byte framing chunking across live yamux + JSON-RPC frames.
                                world.stream_fault_policy().set_for_host(
                                    target.clone(),
                                    StreamFaultConfig {
                                        max_read_chunk: Some(1),
                                        ..StreamFaultConfig::default()
                                    },
                                );
                                if can_drive_live {
                                    let h = spawn_probe(world)?;
                                    let _ = check_expected_chaos_err(
                                        "stream_fault.chunk",
                                        world.await_call(h, 60_000),
                                    )?;
                                }
                                world.stream_fault_policy().clear_for_host(&target);
                                world.hit("chaos.faulty_stream_chunk");
                            }
                            1 => {
                                // Mid-stream TCP RST during write.
                                world.stream_fault_policy().set_for_host(
                                    target.clone(),
                                    StreamFaultConfig {
                                        rst_after_write_bytes: Some(*byte_limit),
                                        ..StreamFaultConfig::default()
                                    },
                                );
                                if can_drive_live {
                                    let h = spawn_probe(world)?;
                                    world.advance(Duration::from_millis(120))?;
                                    world.stream_fault_policy().clear_for_host(&target);
                                    let _ = check_expected_chaos_err(
                                        "stream_fault.write_rst",
                                        world.await_call(h, 60_000),
                                    )?;
                                } else {
                                    world.stream_fault_policy().clear_for_host(&target);
                                }
                                world.hit("chaos.faulty_stream_rst");
                            }
                            2 => {
                                // Mid-stream TCP RST during read.
                                world.stream_fault_policy().set_for_host(
                                    target.clone(),
                                    StreamFaultConfig {
                                        rst_after_read_bytes: Some(*byte_limit),
                                        ..StreamFaultConfig::default()
                                    },
                                );
                                if can_drive_live {
                                    let h = spawn_probe(world)?;
                                    world.advance(Duration::from_millis(120))?;
                                    world.stream_fault_policy().clear_for_host(&target);
                                    let _ = check_expected_chaos_err(
                                        "stream_fault.read_rst",
                                        world.await_call(h, 60_000),
                                    )?;
                                } else {
                                    world.stream_fault_policy().clear_for_host(&target);
                                }
                                world.hit("chaos.faulty_stream_rst");
                            }
                            3 => {
                                // Mid-stream clean 0-byte EOF truncation.
                                world.stream_fault_policy().set_for_host(
                                    target.clone(),
                                    StreamFaultConfig {
                                        eof_after_read_bytes: Some(*byte_limit),
                                        ..StreamFaultConfig::default()
                                    },
                                );
                                if can_drive_live {
                                    let h = spawn_probe(world)?;
                                    world.advance(Duration::from_millis(120))?;
                                    world.stream_fault_policy().clear_for_host(&target);
                                    let _ = check_expected_chaos_err(
                                        "stream_fault.read_eof",
                                        world.await_call(h, 60_000),
                                    )?;
                                } else {
                                    world.stream_fault_policy().clear_for_host(&target);
                                }
                                world.hit("chaos.faulty_stream_rst");
                                world.hit("chaos.faulty_stream_eof");
                            }
                            4 => {
                                // Transient half-open read blackhole stall -> clear to resume.
                                world.stream_fault_policy().set_for_host(
                                    target.clone(),
                                    StreamFaultConfig {
                                        stall_reads: true,
                                        ..StreamFaultConfig::default()
                                    },
                                );
                                if can_drive_live {
                                    let h = spawn_probe(world)?;
                                    world.advance(Duration::from_millis(240))?;
                                    world.stream_fault_policy().clear_for_host(&target);
                                    let _ = check_expected_chaos_err(
                                        "stream_fault.stall_reads",
                                        world.await_call(h, 60_000),
                                    )?;
                                } else {
                                    world.advance(Duration::from_millis(120))?;
                                    world.stream_fault_policy().clear_for_host(&target);
                                }
                                world.hit("chaos.faulty_stream_stall");
                            }
                            5 => {
                                // Transient half-open write blackhole stall -> clear to resume.
                                world.stream_fault_policy().set_for_host(
                                    target.clone(),
                                    StreamFaultConfig {
                                        stall_writes: true,
                                        ..StreamFaultConfig::default()
                                    },
                                );
                                if can_drive_live {
                                    let h = spawn_probe(world)?;
                                    world.advance(Duration::from_millis(240))?;
                                    world.stream_fault_policy().clear_for_host(&target);
                                    let _ = check_expected_chaos_err(
                                        "stream_fault.stall_writes",
                                        world.await_call(h, 60_000),
                                    )?;
                                } else {
                                    world.advance(Duration::from_millis(120))?;
                                    world.stream_fault_policy().clear_for_host(&target);
                                }
                                world.hit("chaos.faulty_stream_stall");
                            }
                            _ => {
                                // Explicit stream-fault recovery on `target`.
                                world.stream_fault_policy().clear_for_host(&target);
                            }
                        }
                    }
                }
                ChaosOp::MultiClientRace {
                    kind,
                    remote_idx,
                    card_pick,
                    stage_idx,
                    tag,
                } => {
                    if profile.enable_multi_client && !remotes.is_empty() {
                        let target_host = remotes[*remote_idx % remotes.len()].clone();
                        let target_pid = host_projects
                            .iter()
                            .find(|(h, _)| h == &target_host)
                            .map(|(_, p)| p.clone())
                            .unwrap();
                        // Multi-client races target server-confirmed cards (never another client's
                        // private offline optimistic temp ID that does not exist on the server).
                        let remote_active: Vec<(String, String, String)> = cards
                            .iter()
                            .filter(|c| {
                                c.shared && c.host != "local" && !archived_cards.contains(&c.cid)
                            })
                            .map(|c| (c.host.clone(), c.pid.clone(), c.cid.clone()))
                            .collect();

                        match kind % 5 {
                            0 => {
                                // Concurrent `card.create` from both clients onto the same remote project!
                                let h1 = world.client_spawn_on(
                                    PRIMARY_CLIENT,
                                    "card.create",
                                    json!({
                                        "project_id": target_pid,
                                        "title": format!("Race C1 #{tag}"),
                                        "body": format!("c1-{seed}-{tag}"),
                                    }),
                                    Some(&target_pid),
                                )?;
                                let h2 = world.client_spawn_on(
                                    SECONDARY_CLIENT,
                                    "card.create",
                                    json!({
                                        "project_id": target_pid,
                                        "title": format!("Race C2 #{tag}"),
                                        "body": format!("c2-{seed}-{tag}"),
                                    }),
                                    Some(&target_pid),
                                )?;
                                world.advance(Duration::from_millis(20))?;
                                let r1 = check_expected_chaos_err(
                                    "race.c1.card.create",
                                    world.await_call(h1, 60_000),
                                )?;
                                let r2 = check_expected_chaos_err(
                                    "race.c2.card.create",
                                    world.await_call(h2, 60_000),
                                )?;
                                for (creator, res_opt) in
                                    [(PRIMARY_CLIENT, r1), (SECONDARY_CLIENT, r2)]
                                {
                                    if let Some(res) = res_opt
                                        && let Some(cid) = res["id"].as_str()
                                    {
                                        let raw_cid = strip_host_id(cid);
                                        let confirmed = if let Some(srv_card) = world
                                            .server(&target_host)
                                            .and_then(|s| s.snapshot().ok())
                                            .and_then(|snap| {
                                                snap.cards.into_iter().find(|c| c.id == raw_cid)
                                            }) {
                                            oracle.record_committed_card(
                                                &target_host,
                                                &srv_card.id,
                                                &srv_card.project_id,
                                                srv_card.revision,
                                            );
                                            true
                                        } else {
                                            false
                                        };
                                        cards.push(TrackedCard {
                                            host: target_host.clone(),
                                            pid: target_pid.clone(),
                                            cid: cid.to_owned(),
                                            creator,
                                            shared: confirmed,
                                        });
                                    }
                                }
                                world.hit("chaos.multi_client_race");
                            }
                            1 if !remote_active.is_empty() => {
                                // Concurrent `card.update` (alternating same-field vs disjoint-field race!).
                                let (_h, pid, cid) =
                                    remote_active[*card_pick % remote_active.len()].clone();
                                let rev1 = lookup_card_rev_on(world, PRIMARY_CLIENT, &cid);
                                let rev2 = lookup_card_rev_on(world, SECONDARY_CLIENT, &cid);
                                let p1 = json!({
                                    "card_id": cid,
                                    "revision": rev1,
                                    "title": format!("Race Title C1 #{tag}"),
                                });
                                let p2 = if tag.is_multiple_of(2) {
                                    json!({
                                        "card_id": cid,
                                        "revision": rev2,
                                        "body": format!("Race Disjoint Body C2 #{tag}"),
                                    })
                                } else {
                                    json!({
                                        "card_id": cid,
                                        "revision": rev2,
                                        "title": format!("Race Title C2 #{tag}"),
                                    })
                                };
                                let h1 = world.client_spawn_on(
                                    PRIMARY_CLIENT,
                                    "card.update",
                                    p1,
                                    Some(&pid),
                                )?;
                                let h2 = world.client_spawn_on(
                                    SECONDARY_CLIENT,
                                    "card.update",
                                    p2,
                                    Some(&pid),
                                )?;
                                world.advance(Duration::from_millis(20))?;
                                let _ = check_expected_chaos_err(
                                    "race.c1.card.update",
                                    world.await_call(h1, 60_000),
                                )?;
                                let _ = check_expected_chaos_err(
                                    "race.c2.card.update",
                                    world.await_call(h2, 60_000),
                                )?;
                                world.hit("chaos.multi_client_race");
                            }
                            2 if !remote_active.is_empty() => {
                                // Concurrent `card.move` from both clients onto the same card!
                                let (_h, pid, cid) =
                                    remote_active[*card_pick % remote_active.len()].clone();
                                let rev1 = lookup_card_rev_on(world, PRIMARY_CLIENT, &cid);
                                let rev2 = lookup_card_rev_on(world, SECONDARY_CLIENT, &cid);
                                let s1 = stages[*stage_idx % stages.len()];
                                let s2 = stages[(*stage_idx + 1) % stages.len()];
                                let h1 = world.client_spawn_on(
                                    PRIMARY_CLIENT,
                                    "card.move",
                                    json!({ "card_id": cid, "revision": rev1, "stage": s1 }),
                                    Some(&pid),
                                )?;
                                let h2 = world.client_spawn_on(
                                    SECONDARY_CLIENT,
                                    "card.move",
                                    json!({ "card_id": cid, "revision": rev2, "stage": s2 }),
                                    Some(&pid),
                                )?;
                                world.advance(Duration::from_millis(20))?;
                                let _ = check_expected_chaos_err(
                                    "race.c1.card.move",
                                    world.await_call(h1, 60_000),
                                )?;
                                let _ = check_expected_chaos_err(
                                    "race.c2.card.move",
                                    world.await_call(h2, 60_000),
                                )?;
                                world.hit("chaos.multi_client_race");
                            }
                            3 if !remote_active.is_empty() => {
                                // Concurrent `comment.add` from both clients onto the same card!
                                let (host, pid, cid) =
                                    remote_active[*card_pick % remote_active.len()].clone();
                                let body1 = format!("race-c1-comment-{tag}");
                                let body2 = format!("race-c2-comment-{tag}");
                                let h1 = world.client_spawn_on(
                                    PRIMARY_CLIENT,
                                    "comment.add",
                                    json!({ "card_id": cid, "body": body1 }),
                                    Some(&pid),
                                )?;
                                let h2 = world.client_spawn_on(
                                    SECONDARY_CLIENT,
                                    "comment.add",
                                    json!({ "card_id": cid, "body": body2 }),
                                    Some(&pid),
                                )?;
                                world.advance(Duration::from_millis(20))?;
                                let r1 = check_expected_chaos_err(
                                    "race.c1.comment.add",
                                    world.await_call(h1, 60_000),
                                )?;
                                let r2 = check_expected_chaos_err(
                                    "race.c2.comment.add",
                                    world.await_call(h2, 60_000),
                                )?;
                                let raw_cid = strip_host_id(&cid);
                                if let Some(evs) = world.server(&host).and_then(|s| s.events().ok())
                                {
                                    for (ok, body) in [(r1.is_some(), body1), (r2.is_some(), body2)]
                                    {
                                        if ok
                                            && evs.iter().any(|e| {
                                                e.card_id == raw_cid
                                                    && e.kind == "comment.created"
                                                    && (e.data["body"].as_str()
                                                        == Some(body.as_str())
                                                        || e.message == body)
                                            })
                                        {
                                            oracle.record_committed_comment(&host, &raw_cid, body);
                                        }
                                    }
                                }
                                world.hit("chaos.multi_client_race");
                            }
                            _ if remote_active.len() > remotes.len() => {
                                // Concurrent `card.archive` (from C2) vs `card.update` (from C1)!
                                let (host, pid, cid) =
                                    remote_active[*card_pick % remote_active.len()].clone();
                                let rev1 = lookup_card_rev_on(world, PRIMARY_CLIENT, &cid);
                                let rev2 = lookup_card_rev_on(world, SECONDARY_CLIENT, &cid);
                                let h1 = world.client_spawn_on(
                                    PRIMARY_CLIENT,
                                    "card.update",
                                    json!({
                                        "card_id": cid,
                                        "revision": rev1,
                                        "body": format!("race-vs-archive-{tag}"),
                                    }),
                                    Some(&pid),
                                )?;
                                let h2 = world.client_spawn_on(
                                    SECONDARY_CLIENT,
                                    "card.archive",
                                    json!({ "card_id": cid, "revision": rev2 }),
                                    Some(&pid),
                                )?;
                                world.advance(Duration::from_millis(20))?;
                                let _ = check_expected_chaos_err(
                                    "race.c1.update_vs_archive",
                                    world.await_call(h1, 60_000),
                                )?;
                                let arch_ok = check_expected_chaos_err(
                                    "race.c2.card.archive",
                                    world.await_call(h2, 60_000),
                                )?
                                .is_some();
                                let raw_cid = strip_host_id(&cid);
                                let server_archived = world
                                    .server(&host)
                                    .and_then(|s| s.snapshot().ok())
                                    .and_then(|snap| {
                                        snap.cards.into_iter().find(|c| c.id == raw_cid)
                                    })
                                    .is_some_and(|c| !c.active());
                                if arch_ok || server_archived {
                                    archived_cards.insert(cid);
                                }
                                world.hit("chaos.multi_client_race");
                            }
                            _ => {}
                        }
                    }
                }
                ChaosOp::ScheduleOp {
                    kind,
                    sched_pick,
                    enable,
                    tag,
                } => {
                    if profile.enable_schedules && !schedule_entries.is_empty() {
                        let (sid, rule_id) =
                            &schedule_entries[*sched_pick % schedule_entries.len()];
                        match kind % 4 {
                            0 => {
                                // Manual `schedule.trigger` alongside partitions & server bounces!
                                let before_ids: BTreeSet<String> = world
                                    .server(&wf_host)
                                    .and_then(|s| s.snapshot().ok())
                                    .map(|snap| {
                                        snap.scheduled_runs.into_iter().map(|r| r.id).collect()
                                    })
                                    .unwrap_or_default();
                                let _ = check_expected_chaos_err(
                                    "schedule.trigger",
                                    world.client_call(
                                        "schedule.trigger",
                                        json!({
                                            "project_id": wf_pid,
                                            "workflow_id": "chaos-cron",
                                            "rule_id": rule_id,
                                        }),
                                        Some(&wf_pid),
                                    ),
                                )?;
                                if let Some(snap) =
                                    world.server(&wf_host).and_then(|s| s.snapshot().ok())
                                {
                                    for sr in &snap.scheduled_runs {
                                        if !before_ids.contains(&sr.id) {
                                            oracle.record_committed_scheduled_run(
                                                &wf_host,
                                                &sr.id,
                                                &sr.project_id,
                                                &sr.rule_id,
                                            );
                                        }
                                    }
                                }
                                world.hit("chaos.schedule_ops");
                            }
                            1 => {
                                let _ = check_expected_chaos_err(
                                    "schedule.toggle",
                                    world.client_call(
                                        "schedule.toggle",
                                        json!({ "schedule_id": sid, "enabled": *enable }),
                                        Some(&wf_pid),
                                    ),
                                )?;
                                world.hit("chaos.schedule_ops");
                            }
                            2 => {
                                let _ = check_expected_chaos_err(
                                    "schedule.list",
                                    world.client_call(
                                        "schedule.list",
                                        json!({ "project_id": wf_pid }),
                                        Some(&wf_pid),
                                    ),
                                )?;
                                world.hit("chaos.schedule_ops");
                            }
                            _ => {
                                // Complete an active cardless `ScheduledRun` if any is running so
                                // queued triggers drain cleanly under chaos.
                                if let Some(srv) = world.server(&wf_host) {
                                    let snap = srv.snapshot()?;
                                    let mut active_scheds: Vec<_> = snap
                                        .scheduled_runs
                                        .iter()
                                        .filter(|r| !r.creates_card && r.active())
                                        .collect();
                                    active_scheds.sort_by_key(|r| (r.scheduled_at, r.number));
                                    if let Some(sr) = active_scheds.first() {
                                        oracle.record_committed_scheduled_run(
                                            &wf_host,
                                            &sr.id,
                                            &sr.project_id,
                                            &sr.rule_id,
                                        );
                                        let _ = check_expected_chaos_err(
                                            "execution.report(schedule)",
                                            world.server_call(
                                                &wf_host,
                                                "execution.report",
                                                json!({
                                                    "schedule_run_id": sr.id,
                                                    "state": "completed",
                                                    "message": format!("chaos schedule done {tag}"),
                                                    "request_key": format!("chaos-srep-{seed}-{tag}"),
                                                }),
                                            ),
                                        )?;
                                        world.hit("chaos.schedule_ops");
                                    }
                                }
                            }
                        }
                    }
                }
                ChaosOp::WorktreeDeliveryOp {
                    kind,
                    card_pick,
                    tag,
                } => {
                    if profile.enable_worktree_delivery && !wf_dir.is_empty() {
                        match kind % 5 {
                            0 => {
                                // Inject script or batch test failure to exercise rework & `@merger`!
                                if let Some(exec) = world.exec_runner(&wf_host) {
                                    if tag.is_multiple_of(2) {
                                        exec.enqueue_script_outcomes(
                                            "chaos_precheck",
                                            [SimShellOutcome::fail(
                                                2,
                                                format!("chaos script {tag}\n"),
                                                "simulated chaos precheck failure\n",
                                            )],
                                        );
                                    } else {
                                        exec.enqueue_shell_outcomes(
                                            "sim-chaos-test",
                                            [SimShellOutcome::fail(
                                                1,
                                                format!("chaos batch test {tag}\n"),
                                                "simulated chaos batch test failure\n",
                                            )],
                                        );
                                    }
                                    world.hit("chaos.worktree_delivery_ops");
                                }
                            }
                            1 => {
                                // Inject a git merge conflict on the next `deliver` batch integration!
                                if let Some(exec) = world.exec_runner(&wf_host) {
                                    exec.inject_merge_conflict(
                                        &wf_dir,
                                        vec![format!("src/chaos_conflict_{tag}.rs")],
                                    );
                                    world.hit("chaos.worktree_delivery_ops");
                                }
                            }
                            2 => {
                                // Drive any active `@merger` `MergeBatch` (`state == "running"`)
                                // via `batch.report` (either resolving or evicting the conflicting card!).
                                if let Some(srv) = world.server(&wf_host) {
                                    let snap = srv.snapshot()?;
                                    let mut batches: Vec<_> = snap
                                        .merge_batches
                                        .iter()
                                        .filter(|b| b.state == "running")
                                        .collect();
                                    batches.sort_by_key(|b| b.created_at);
                                    if let Some(batch) = batches.first() {
                                        oracle.record_committed_merge_batch(
                                            &wf_host,
                                            &batch.id,
                                            &batch.project_id,
                                        );
                                        if let Some(exec) = world.exec_runner(&wf_host)
                                            && Path::new(&batch.worktree).is_dir()
                                        {
                                            let evict_card = batch
                                                .conflict_card_number
                                                .filter(|_| tag.is_multiple_of(2));
                                            let params = if let Some(cnum) = evict_card {
                                                let _ =
                                                    exec.abort_worktree_conflict(&batch.worktree);
                                                json!({
                                                    "batch_id": batch.id,
                                                    "state": "completed",
                                                    "evict": [cnum],
                                                    "message": format!("chaos evict card {cnum}"),
                                                    "request_key": format!("chaos-batch-{seed}-{tag}"),
                                                })
                                            } else {
                                                let _ = exec
                                                    .resolve_worktree_and_commit(&batch.worktree);
                                                json!({
                                                    "batch_id": batch.id,
                                                    "state": "completed",
                                                    "message": format!("chaos resolve batch {tag}"),
                                                    "request_key": format!("chaos-batch-{seed}-{tag}"),
                                                })
                                            };
                                            let _ = check_expected_chaos_err(
                                                "batch.report",
                                                world.server_call(&wf_host, "batch.report", params),
                                            )?;
                                        }
                                        world.hit("chaos.worktree_delivery_ops");
                                    } else {
                                        // If no `@merger` batch is currently running, run a standalone
                                        // `worktree.create` -> `worktree.status` -> `worktree.remove` cycle!
                                        if let Some(wt_res) = check_expected_chaos_err(
                                            "worktree.create",
                                            world.client_call(
                                                "worktree.create",
                                                json!({
                                                    "project_id": wf_pid,
                                                    "branch": format!("chaos/wt-{seed}-{tag}"),
                                                    "launch": "none",
                                                }),
                                                Some(&wf_pid),
                                            ),
                                        )? && let Some(wt_id) = wt_res["worktree"]["id"].as_str()
                                        {
                                            let _ = check_expected_chaos_err(
                                                "worktree.status",
                                                world.client_call(
                                                    "worktree.status",
                                                    json!({ "worktree_id": wt_id }),
                                                    Some(&wf_pid),
                                                ),
                                            )?;
                                            let _ = check_expected_chaos_err(
                                                "worktree.remove",
                                                world.client_call(
                                                    "worktree.remove",
                                                    json!({ "worktree_id": wt_id, "force": true }),
                                                    Some(&wf_pid),
                                                ),
                                            )?;
                                            world.hit("chaos.worktree_delivery_ops");
                                        }
                                    }
                                }
                            }
                            3 => {
                                // Commit deterministic changes inside any card's active worktree on `wf_host`.
                                if let Some(srv) = world.server(&wf_host)
                                    && let Some(exec) = world.exec_runner(&wf_host)
                                {
                                    let snap = srv.snapshot()?;
                                    let mut wt_cards: Vec<_> = snap
                                        .cards
                                        .iter()
                                        .filter(|c| {
                                            c.active()
                                                && c.worktree
                                                    .as_deref()
                                                    .is_some_and(|w| Path::new(w).is_dir())
                                        })
                                        .collect();
                                    wt_cards.sort_by_key(|c| c.number);
                                    if !wt_cards.is_empty() {
                                        let card = wt_cards[*card_pick % wt_cards.len()];
                                        let wt = card.worktree.as_deref().unwrap();
                                        let _ = exec.commit_in_worktree(
                                            wt,
                                            &format!("feat: chaos worktree edit {tag}"),
                                        );
                                        world.hit("chaos.worktree_delivery_ops");
                                    }
                                }
                            }
                            _ => {
                                // Toggle `card.hold_auto_merge` on an active card on `wf_host`.
                                let wf_cards: Vec<_> = cards
                                    .iter()
                                    .filter(|c| {
                                        c.shared
                                            && c.host == wf_host
                                            && !archived_cards.contains(&c.cid)
                                    })
                                    .collect();
                                if !wf_cards.is_empty() {
                                    let tc = wf_cards[*card_pick % wf_cards.len()];
                                    let (pid, cid) = (&tc.pid, &tc.cid);
                                    let rev = lookup_card_rev(world, cid);
                                    let _ = check_expected_chaos_err(
                                        "card.hold_auto_merge",
                                        world.client_call(
                                            "card.hold_auto_merge",
                                            json!({
                                                "card_id": cid,
                                                "revision": rev,
                                                "hold": tag.is_multiple_of(2),
                                            }),
                                            Some(pid),
                                        ),
                                    )?;
                                    world.hit("chaos.worktree_delivery_ops");
                                }
                            }
                        }
                    }
                }
                ChaosOp::AuthRevokeRecover { recover, tag: _ } => {
                    if profile.enable_auth_faults && world.server(&auth_host).is_some() {
                        if !auth_revoked {
                            if let Some(dev_id) = auth_paired_device_id.take() {
                                let res = world.server_call(
                                    &auth_host,
                                    "device.revoke",
                                    json!({ "device_id": dev_id }),
                                );
                                if check_expected_chaos_err("device.revoke", res)?.is_some() {
                                    auth_revoked = true;
                                    world.advance(Duration::from_millis(200))?;
                                    // Probe a routed RPC while revoked so `AuthFailed` handling is actively fuzzed.
                                    let _ = check_expected_chaos_err(
                                        "project.list(revoked)",
                                        world.client_call(
                                            "project.list",
                                            json!({ "host_id": auth_host }),
                                            None,
                                        ),
                                    )?;
                                }
                            }
                        } else if *recover && !partitioned_remotes.contains(&auth_host) {
                            // Clear any transient stream fault on `auth_host` before re-pairing.
                            world.stream_fault_policy().clear_for_host(&auth_host);
                            let repair_res = pair_host_on_primary_client(world, &auth_host)
                                .map(|dev_id| json!({ "device_id": dev_id }));
                            if let Some(val) = check_expected_chaos_err("auth.re_pair", repair_res)?
                                && let Some(new_dev_id) = val["device_id"].as_str()
                            {
                                auth_paired_device_id = Some(new_dev_id.to_owned());
                                auth_revoked = false;
                                world.hit("chaos.auth_revoke_recover");
                            }
                        }
                    }
                }
                ChaosOp::ProjectArchiveRestore { archive, tag } => {
                    if let Some((
                        ref arch_host,
                        ref arch_pid,
                        ref arch_raw_pid,
                        ref arch_raw_cid,
                        ref mut arch_raw_nid,
                    )) = archive_proj
                    {
                        let srv_name = server_key(arch_host);
                        if !archive_proj_archived && *archive {
                            let arch_ok = check_expected_chaos_err(
                                "project.archive",
                                world.client_call(
                                    "project.archive",
                                    json!({ "project_id": arch_pid }),
                                    Some(arch_pid),
                                ),
                            )?
                            .is_some();
                            let gone_on_srv = world
                                .server(srv_name)
                                .and_then(|s| s.snapshot().ok())
                                .is_some_and(|snap| {
                                    !snap.projects.iter().any(|p| p.id == *arch_raw_pid)
                                });
                            if arch_ok || gone_on_srv {
                                oracle.clear_committed_project(srv_name, arch_raw_pid);
                                archive_proj_archived = true;
                                let _ = check_expected_chaos_err(
                                    "project.archived.list",
                                    world.client_call(
                                        "project.archived.list",
                                        json!({ "host_id": arch_host }),
                                        None,
                                    ),
                                )?;
                            }
                        } else if archive_proj_archived {
                            let rest_ok = check_expected_chaos_err(
                                "project.restore",
                                world.client_call(
                                    "project.restore",
                                    json!({ "host_id": arch_host, "project_id": arch_raw_pid }),
                                    None,
                                ),
                            )?
                            .is_some();
                            let back_on_srv = world
                                .server(srv_name)
                                .and_then(|s| s.snapshot().ok())
                                .is_some_and(|snap| {
                                    snap.projects.iter().any(|p| p.id == *arch_raw_pid)
                                });
                            if rest_ok || back_on_srv {
                                archive_proj_archived = false;
                                if let Some(snap) =
                                    world.server(srv_name).and_then(|s| s.snapshot().ok())
                                {
                                    ensure!(
                                        snap.cards.iter().any(|c| c.id == *arch_raw_cid),
                                        "[{srv_name}] Secondary archive card {arch_raw_cid} vanished across project.archive -> project.restore!"
                                    );
                                    if let Some(c) =
                                        snap.cards.iter().find(|c| c.id == *arch_raw_cid)
                                    {
                                        oracle.record_committed_card(
                                            srv_name,
                                            &c.id,
                                            &c.project_id,
                                            c.revision,
                                        );
                                    }
                                    if let Some(nid) = arch_raw_nid.as_deref() {
                                        ensure!(
                                            snap.notes.iter().any(|n| n.id == nid),
                                            "[{srv_name}] Secondary archive note {nid} vanished across project.archive -> project.restore!"
                                        );
                                        if let Some(n) = snap.notes.iter().find(|n| n.id == nid) {
                                            oracle.record_committed_note(
                                                srv_name,
                                                &n.id,
                                                &n.project_id,
                                                n.revision,
                                            );
                                        }
                                    }
                                }
                                world.hit("chaos.project_archive_restore");
                            }
                        } else {
                            // Project is active and `*archive == false`: update its Note and track durability!
                            if let Some(res) = check_expected_chaos_err(
                                "note.save",
                                world.client_call(
                                    "note.save",
                                    json!({
                                        "project_id": arch_pid,
                                        "title": "Chaos Archive Note",
                                        "body": format!("seed-{seed}-note-{tag}"),
                                    }),
                                    Some(arch_pid),
                                ),
                            )? && let Some(nid) = res["id"].as_str()
                            {
                                let raw_nid = strip_host_id(nid);
                                *arch_raw_nid = Some(raw_nid.clone());
                                if let Some(n) = world
                                    .server(srv_name)
                                    .and_then(|s| s.snapshot().ok())
                                    .and_then(|snap| {
                                        snap.notes.into_iter().find(|n| n.id == raw_nid)
                                    })
                                {
                                    oracle.record_committed_note(
                                        srv_name,
                                        &n.id,
                                        &n.project_id,
                                        n.revision,
                                    );
                                }
                            }
                        }
                    }
                }
            }

            // Advance a small virtual tick and check step invariants after EVERY SINGLE OP!
            world.advance(Duration::from_millis(80))?;
            oracle.check_step_invariants(world)?;
        }

        // Clean pre-quiescence recovery for newly fuzzed subsystems:
        // 1. Repair links/stream faults and ensure all servers/clients are running before restoring
        //    cron toggles, revoked auth, or an archived secondary project.
        world.repair_all();
        let crashed_servers: Vec<String> = world
            .servers
            .iter()
            .filter(|(_, h)| h.server().is_none())
            .map(|(n, _)| n.clone())
            .collect();
        for name in crashed_servers {
            world.bounce_server(&name)?;
        }
        let crashed_clients: Vec<String> = world
            .clients
            .iter()
            .filter(|(_, h)| h.client().is_none())
            .map(|(n, _)| n.clone())
            .collect();
        for name in crashed_clients {
            world.bounce_client_named(&name)?;
        }

        // 2. Disable cron schedules so no new cron ticks fire during quiescence drain/polling.
        for (sid, _) in &schedule_entries {
            let _ = world.server_call(
                &wf_host,
                "schedule.toggle",
                json!({ "schedule_id": sid, "enabled": false }),
            )?;
        }

        // 3. Re-pair `auth_host` if still revoked so strict `Connected` + convergence checks run.
        if auth_revoked {
            let _new_dev_id = pair_host_on_primary_client(world, &auth_host)?;
            world.hit("chaos.auth_revoke_recover");
        }

        // 4. Restore secondary archived project if still archived so full roundtrip & convergence check.
        if archive_proj_archived
            && let Some((ref arch_host, _, ref arch_raw_pid, ref arch_raw_cid, ref arch_raw_nid)) =
                archive_proj
        {
            let srv_name = server_key(arch_host);
            let _ = world.server_call(
                srv_name,
                "project.restore",
                json!({ "project_id": arch_raw_pid }),
            )?;
            let snap = world.server(srv_name).unwrap().snapshot()?;
            ensure!(
                snap.cards.iter().any(|c| c.id == *arch_raw_cid),
                "[{srv_name}] Secondary archive card {arch_raw_cid} vanished after pre-quiescence restore!"
            );
            if let Some(c) = snap.cards.iter().find(|c| c.id == *arch_raw_cid) {
                oracle.record_committed_card(srv_name, &c.id, &c.project_id, c.revision);
            }
            if let Some(nid) = arch_raw_nid.as_deref() {
                ensure!(
                    snap.notes.iter().any(|n| n.id == nid),
                    "[{srv_name}] Secondary archive note {nid} vanished after pre-quiescence restore!"
                );
                if let Some(n) = snap.notes.iter().find(|n| n.id == nid) {
                    oracle.record_committed_note(srv_name, &n.id, &n.project_id, n.revision);
                }
            }
            world.hit("chaos.project_archive_restore");
        }

        // Heal all faults (including dynamic clock jumps/drift) & quiesce.
        world.heal_and_quiesce(25_000)?;

        // Resolve ONLY genuine `SyncItemState::Conflict` items (cycling through ALL SyncAction
        // variants: Retry, UseMine, UseEdited, Discard!) plus the specific expected offline-edit
        // vs server-archive race where the server genuinely archived the card while an offline
        // mutation was queued — across ALL live simulated clients!
        // NEVER discard any other `SyncItemState::Failed`!
        let mut retry_used = false;
        for round in 0..8usize {
            let mut any_resolved = false;
            for cname in world.client_names() {
                let sync_items = world.client_sync_items_on(&cname);
                let conflicts: Vec<_> = sync_items
                    .iter()
                    .filter(|i| matches!(i.state, SyncItemState::Conflict))
                    .cloned()
                    .collect();
                let expected_archived_fails: Vec<_> = sync_items
                    .iter()
                    .filter(|i| {
                        if !matches!(i.state, SyncItemState::Failed) {
                            return false;
                        }
                        let msg = i.message.to_ascii_lowercase();
                        let is_archived_err = msg.contains("archived or removed")
                            || msg.contains("restore the card before");
                        if !is_archived_err {
                            return false;
                        }
                        let srv_name = server_key(&i.host_id);
                        let raw_cid = strip_host_id(&i.card_id);
                        world
                            .server(srv_name)
                            .and_then(|s| s.snapshot().ok())
                            .and_then(|snap| snap.cards.into_iter().find(|c| c.id == raw_cid))
                            .is_some_and(|c| !c.active())
                    })
                    .cloned()
                    .collect();
                if conflicts.is_empty() && expected_archived_fails.is_empty() {
                    continue;
                }
                any_resolved = true;
                for item in expected_archived_fails {
                    world.client_sync_action_on(
                        &cname,
                        &item.host_id,
                        &item.mutation_id,
                        SyncAction::Discard,
                    )?;
                    world.hit("chaos.archived_edit_race_discarded");
                }
                for (idx, item) in conflicts.into_iter().enumerate() {
                    let action = if !retry_used && round == 0 {
                        retry_used = true;
                        SyncAction::Retry
                    } else {
                        match (idx + round) % 3 {
                            0 => SyncAction::UseMine,
                            1 => SyncAction::UseEdited {
                                fields: BTreeMap::from([(
                                    "title".into(),
                                    format!("Chaos Resolved {round}-{idx}"),
                                )]),
                            },
                            _ => SyncAction::Discard,
                        }
                    };
                    world.client_sync_action_on(
                        &cname,
                        &item.host_id,
                        &item.mutation_id,
                        action,
                    )?;
                }
            }
            if !any_resolved {
                break;
            }
            world.heal_and_quiesce(20_000)?;
        }

        // Explicitly assert remote global-env convergence when global env was saved on Desktop!
        // Trigger a final sync if a remote host was temporarily in AuthFailed during earlier saves.
        if profile.desktop_local && env_secret_saved_count > 0 {
            let local_env = world.server("local-server").unwrap().snapshot()?.global_env;
            let needs_env_resync = remotes.iter().any(|r| {
                world
                    .server(r)
                    .and_then(|s| s.snapshot().ok())
                    .is_some_and(|snap| snap.global_env != local_env)
            });
            if needs_env_resync {
                world.client_sync_global_env_now()?;
                world.heal_and_quiesce(20_000)?;
            }
            let local_env = world.server("local-server").unwrap().snapshot()?.global_env;
            for r in &remotes {
                let r_env = world.server(r).unwrap().snapshot()?.global_env;
                ensure!(
                    r_env == local_env,
                    "Chaos remote host `{r}` global_env diverged from local at quiescence!"
                );
            }
            world.hit("chaos.global_env_converged");
        }

        // Final strict quiescent invariant check (fails on ANY unexpected `SyncItemState::Failed`,
        // divergence on ANY client, SQLite integrity error, post-bounce stuck state, or secret leak!).
        oracle.check_quiescent_invariants(world)?;

        Ok(finalize_digest(world))
    })
}

fn server_key(host: &str) -> &str {
    if host == "local" {
        "local-server"
    } else {
        host
    }
}

fn strip_host_id(id: &str) -> String {
    canban_types::split_qualified_host_id(id)
        .map(|(_, r)| r.to_owned())
        .unwrap_or_else(|| id.to_owned())
}

#[derive(Clone, Debug)]
struct TrackedCard {
    host: String,
    pid: String,
    cid: String,
    creator: &'static str,
    shared: bool,
}

fn pick_active_card(
    world: &SimWorld,
    client_id: &str,
    cards: &[TrackedCard],
    archived: &BTreeSet<String>,
    pick: usize,
) -> Option<(String, String, String)> {
    let client_snap = world.client_merged_snapshot_on(client_id).ok();
    let active: Vec<&TrackedCard> = cards
        .iter()
        .filter(|c| {
            if archived.contains(&c.cid) {
                return false;
            }
            if c.shared {
                true
            } else {
                // Unconfirmed optimistic temp ID: only usable by its creator while still
                // present (un-rewritten) in that client's local merged snapshot.
                c.creator == client_id
                    && client_snap
                        .as_ref()
                        .is_some_and(|s| s.cards.iter().any(|sc| sc.id == c.cid && sc.active()))
            }
        })
        .collect();
    if active.is_empty() {
        return None;
    }
    let chosen = active[pick % active.len()];
    Some((chosen.host.clone(), chosen.pid.clone(), chosen.cid.clone()))
}

fn lookup_card_rev(world: &SimWorld, qualified_cid: &str) -> u64 {
    lookup_card_rev_on(world, PRIMARY_CLIENT, qualified_cid)
}

fn lookup_card_rev_on(world: &SimWorld, client_id: &str, qualified_cid: &str) -> u64 {
    world
        .client_merged_snapshot_on(client_id)
        .ok()
        .and_then(|snap| {
            snap.cards
                .iter()
                .find(|c| c.id == qualified_cid)
                .map(|c| c.revision)
        })
        .unwrap_or(1)
}

/// Pairs (or re-pairs in-place) `remote_host` on `"ui-client"` using a fresh single-use
/// `device.invite.create` code and returns the newly minted server-side `device_id`.
fn pair_host_on_primary_client(world: &mut SimWorld, remote_host: &str) -> Result<String> {
    let before = world.server_call(remote_host, "device.list", json!({}))?;
    let before_ids: BTreeSet<String> = before["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d["id"].as_str().map(str::to_owned))
        .collect();

    let invite = world.server_call(
        remote_host,
        "device.invite.create",
        json!({ "ttl_secs": 300 }),
    )?;
    let code = invite["code"]
        .as_str()
        .context("device.invite.create missing code")?
        .to_owned();
    let host_owned = remote_host.to_owned();
    let addr = format!("{remote_host}:{SIM_PORT}");
    let _summary = world.client_run_on(PRIMARY_CLIENT, 30_000, move |client| async move {
        client
            .add_host_async(SavedHost {
                id: host_owned.clone(),
                name: host_owned,
                transport: HostTransport::Tcp { addr, token: code },
                ssh_target: None,
                server_id: None,
                enabled: true,
                added_at: 0,
            })
            .await
    })?;

    let after = world.server_call(remote_host, "device.list", json!({}))?;
    let new_dev_id = after["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d["id"].as_str().map(str::to_owned))
        .find(|id| !before_ids.contains(id))
        .with_context(|| format!("No newly paired device found on {remote_host}"))?;
    Ok(new_dev_id)
}

/// Deterministically shrink a failing `(seed, max_ops)` Chaos trace via prefix binary search
/// followed by delta-debugging (`ddmin`) chunk and single-op deletion.
/// Returns `Ok(None)` if the seed passes without failing.
pub fn shrink(seed: u64, max_ops: usize) -> Result<Option<ShrinkReport>> {
    let (profile, ops) = generate_ops(seed, max_ops);
    let original_len = ops.len();
    let initial_err = match run_ops(seed, &profile, &ops) {
        Ok(_) => return Ok(None),
        Err(e) => format!("{e:#}"),
    };
    let (minimized, attempts, final_err) = shrink_ops(ops, initial_err, 64, |candidate| {
        run_ops(seed, &profile, candidate)
            .err()
            .map(|e| format!("{e:#}"))
    });
    Ok(Some(ShrinkReport {
        seed,
        original_len,
        minimized_len: minimized.len(),
        attempts: attempts + 1,
        ops: minimized,
        error: final_err,
    }))
}

/// Generic deterministic shrinker (prefix binary search + bounded ddmin deletion).
pub fn shrink_ops<T: Clone, F>(
    ops: Vec<T>,
    initial_error: String,
    max_attempts: usize,
    mut reproduces: F,
) -> (Vec<T>, usize, String)
where
    F: FnMut(&[T]) -> Option<String>,
{
    let mut best = ops;
    let mut best_err = initial_error;
    let mut attempts = 0usize;

    // 1. Prefix binary search: find shortest failing prefix `best[..hi]`.
    if !best.is_empty() && attempts < max_attempts {
        let mut lo = 0usize;
        let mut hi = best.len();
        while lo < hi && attempts < max_attempts {
            let mid = lo + (hi - lo) / 2;
            attempts += 1;
            if let Some(err) = reproduces(&best[..mid]) {
                hi = mid;
                best_err = err;
            } else {
                lo = mid + 1;
            }
        }
        best.truncate(hi);
    }

    // 2. Bounded ddmin: remove chunks of size `granularity` from `best`, halving down to 1.
    let mut chunk = (best.len() / 2).max(1);
    while !best.is_empty() && attempts < max_attempts {
        let mut progressed = false;
        let mut start = 0usize;
        while start < best.len() && attempts < max_attempts {
            let end = (start + chunk).min(best.len());
            let mut candidate = Vec::with_capacity(best.len() - (end - start));
            candidate.extend_from_slice(&best[..start]);
            candidate.extend_from_slice(&best[end..]);
            attempts += 1;
            if let Some(err) = reproduces(&candidate) {
                best = candidate;
                best_err = err;
                progressed = true;
                // Keep `start` at the same offset since elements shifted left.
            } else {
                start = end;
            }
        }
        if chunk == 1 {
            if !progressed {
                break;
            }
        } else {
            chunk = (chunk / 2).max(1);
        }
    }

    (best, attempts, best_err)
}
