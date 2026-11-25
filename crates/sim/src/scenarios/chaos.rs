//! Swarm-Testing Randomized Chaos DST + Deterministic Shrinker (`chaos::shrink`):
//! - Per-seed randomized `ChaosProfile` (Desktop vs Mobile, clock skews, latencies, fail rates,
//!   enabled fault families, workflow+GitHub co-location, `max_agents`, operation weights).
//! - Deterministic materialized `Vec<ChaosOp>` enabling exact replay & prefix+ddmin shrinking.
//! - Explicit expected-error classifier (no blanket `let _ =`), never discards unexpected
//!   `SyncItemState::Failed`, exercises ALL `SyncAction` variants (`Retry`, `UseMine`, `UseEdited`,
//!   `Discard` on expected `Conflict` only), fixed `env.save` object payload with offline secret
//!   tracking and convergence checks, and `oracle.check_step_invariants` after EVERY op!
use crate::{
    github::SimIssue,
    herdr::RunFault,
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{SimWorld, SimWorldConfig, run_with_world},
};
use anyhow::{Result, bail, ensure};
use canban_types::{EnvEntryMeta, SyncAction, SyncItemState};
use rand::{Rng, SeedableRng, rngs::StdRng};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

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
    pub weights: [u32; 24],
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

        let mut weights = [0u32; 24];
        for w in &mut weights {
            *w = rng.random_range(1..=5);
        }
        // Keep core mutations & sync actions well-represented.
        weights[0] += 3; // CreateCard
        weights[1] += 3; // UpdateCard
        weights[4] += 2; // AddComment
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
    },
    CrashAndBounceServer {
        host_idx: usize,
    },
    BounceClient,
    AdvanceTime {
        secs: u64,
    },
    ClockFault {
        host_idx: usize,
        kind: u8,
        delta_ms: i64,
        drift_ppm: i64,
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
            },
            20 => ChaosOp::CrashAndBounceServer {
                host_idx: rng.random_range(0..16),
            },
            21 => ChaosOp::BounceClient,
            22 => {
                let secs = if profile.enable_reclaim_jumps && rng.random_bool(0.35) {
                    rng.random_range(65..=95)
                } else {
                    rng.random_range(1..=6)
                };
                ChaosOp::AdvanceTime { secs }
            }
            _ => ChaosOp::ClockFault {
                host_idx: rng.random_range(0..16),
                kind: rng.random_range(0..2),
                delta_ms: rng.random_range(-4_000..=6_000),
                drift_ppm: rng.random_range(-5_000..=5_000),
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
                "channel",
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
                "already active",
                "already has an active",
                "already running",
                "already completed",
                "already stopped",
                "already ended",
                "resolve the existing human decision",
                "not waiting",
                "not active",
                "workflow run is not",
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

        let mut oracle = InvariantOracle::default();
        let all_hosts = profile.hosts();
        let remotes = profile.remote_hosts.clone();

        // Track partition/offline state cleanly so we know when offline secret writes occur.
        let mut partitioned_remotes: BTreeSet<String> = BTreeSet::new();
        let mut offline_herdr_hosts: BTreeSet<String> = BTreeSet::new();

        // Create one project per host in deterministic order.
        let mut host_projects: Vec<(String, String)> = Vec::new();
        for host in &all_hosts {
            let dir = world.project_dir(&format!("chaos-{host}"))?;
            let p = world.client_call(
                "project.create",
                json!({ "host_id": host, "name": format!("Chaos {host}"), "directory": dir }),
                None,
            )?;
            let pid = p["id"].as_str().unwrap().to_owned();
            // Configure fast idle reclaim (1 min) + per-seed agent budget!
            let _ = world.client_call(
                "scheduler.configure",
                json!({ "max_agents": profile.max_agents, "idle_minutes": 1 }),
                Some(&pid),
            )?;
            host_projects.push((host.clone(), pid));
        }

        // Install chaos workflow on `remote-a` (and `local` if desktop) when enabled.
        let wf_host = "remote-a".to_owned();
        let wf_pid = host_projects
            .iter()
            .find(|(h, _)| h == &wf_host)
            .map(|(_, p)| p.clone())
            .unwrap();
        if profile.enable_workflows {
            let sources = world.client_call(
                "workflow.sources",
                json!({ "project_id": wf_pid }),
                Some(&wf_pid),
            )?;
            let _ = world.client_call(
                "workflow.apply",
                json!({
                    "project_id": wf_pid,
                    "revision": sources["revision"],
                    "sources": [{ "filename": "chaos-flow.yaml", "source": CHAOS_WORKFLOW_YAML }],
                }),
                Some(&wf_pid),
            )?;
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
        let mut cards: Vec<(String, String, String)> = Vec::new(); // (host, pid, qualified_cid)
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
            cards.push((host.clone(), pid.clone(), cid));
        }

        world.advance(Duration::from_millis(200))?;
        oracle.check_step_invariants(world)?;

        if profile.base_fail_rate > 0.0 {
            for r in &remotes {
                world.set_link_fail_rate("ui-client", r, profile.base_fail_rate);
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
                        if let Some(srv_card) = world
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
                        }
                        cards.push((host.clone(), pid.clone(), cid.to_owned()));
                    }
                }
                ChaosOp::UpdateCard { card_pick, tag } => {
                    if let Some((_host, pid, cid)) =
                        pick_active_card(&cards, &archived_cards, *card_pick)
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
                        pick_active_card(&cards, &archived_cards, *card_pick)
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
                    // Always preserve at least 1 active card per host.
                    let active_count = cards
                        .iter()
                        .filter(|(_, _, cid)| !archived_cards.contains(cid))
                        .count();
                    if active_count > host_projects.len()
                        && let Some((host, pid, cid)) =
                            pick_active_card(&cards, &archived_cards, *card_pick)
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
                    if let Some((_host, pid, cid)) =
                        pick_active_card(&cards, &archived_cards, *card_pick)
                    {
                        let _ = check_expected_chaos_err(
                            "comment.add",
                            world.client_call(
                                "comment.add",
                                json!({ "card_id": cid, "body": format!("chaos-comment-{tag}") }),
                                Some(&pid),
                            ),
                        )?;
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
                        pick_active_card(&cards, &archived_cards, *card_pick)
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
                    if let Some((_host, pid, cid)) =
                        pick_active_card(&cards, &archived_cards, *card_pick)
                    {
                        let visible = world
                            .client_merged_snapshot()?
                            .cards
                            .iter()
                            .any(|c| c.id == cid && c.active());
                        if visible {
                            let _ = InvariantOracle::assert_card_complete_and_readable(
                                world, &cid, &pid,
                            )?;
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
                            .filter(|(h, _, cid)| h == &wf_host && !archived_cards.contains(cid))
                            .collect();
                        if !wf_cards.is_empty() {
                            let (_, pid, cid) = wf_cards[*card_pick % wf_cards.len()];
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
                            world.hit("chaos.execution_report");
                        }
                    }
                }
                ChaosOp::ViewerPresence { card_pick, attach } => {
                    if let Some((_host, pid, cid)) =
                        pick_active_card(&cards, &archived_cards, *card_pick)
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
                            5 => {
                                world.github.set_read_after_write_lag(1);
                                world.hit("chaos.github_raw_lag");
                            }
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
                                world.github.set_rate_limit_budget(Some(1), reset_secs);
                                world.hit("chaos.github_rate_limit_budget");
                            }
                            _ => {
                                // Ensure transient host-offline GitHub faults recover on clean ops.
                                world.github.set_host_offline(&gh_host, false);
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
                                herdr.buffer_next_events_for_reorder(2);
                                if let Some(addr) = addrs.first() {
                                    herdr.set_status(addr, "working", Some(u64::from(*tag)));
                                    herdr.set_status(addr, "idle", Some(u64::from(*tag) + 1));
                                } else {
                                    herdr.flush_reordered_events();
                                }
                                world.hit("chaos.herdr_reorder_events");
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
                ChaosOp::NetworkFault { remote_idx, kind } => {
                    if !remotes.is_empty() {
                        let target = &remotes[*remote_idx % remotes.len()];
                        match kind {
                            0 => {
                                world.partition("ui-client", target);
                                partitioned_remotes.insert(target.clone());
                                world.hit("chaos.net_partition");
                            }
                            1 => {
                                world.partition_oneway(target, "ui-client");
                                partitioned_remotes.insert(target.clone());
                                world.hit("chaos.net_oneway");
                            }
                            2 => {
                                world.repair("ui-client", target);
                                partitioned_remotes.remove(target);
                            }
                            3 => {
                                world.hold("ui-client", target);
                                world.advance(Duration::from_millis(60))?;
                                world.release("ui-client", target);
                            }
                            _ => {
                                world.set_link_fail_rate("ui-client", target, 0.05);
                                world.advance(Duration::from_millis(80))?;
                                world.set_link_fail_rate(
                                    "ui-client",
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
                ChaosOp::BounceClient => {
                    world.crash_client()?;
                    world.advance(Duration::from_millis(120))?;
                    world.bounce_client()?;
                    world.hit("chaos.client_bounce");
                }
                ChaosOp::AdvanceTime { secs } => {
                    if *secs >= 60 {
                        // Step in bounded chunks so background idle reclaimer ticks cleanly.
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
            }

            // Advance a small virtual tick and check step invariants after EVERY SINGLE OP!
            world.advance(Duration::from_millis(80))?;
            oracle.check_step_invariants(world)?;
        }

        // Restore dynamic clock jumps/drift before healing & quiescence.
        for handle in world.servers.values() {
            handle.clock.reset_adjustments();
        }
        // Heal all faults & quiesce.
        world.heal_and_quiesce(25_000)?;

        // Resolve ONLY genuine `SyncItemState::Conflict` items (cycling through ALL SyncAction
        // variants: Retry, UseMine, UseEdited, Discard!) plus the specific expected offline-edit
        // vs server-archive race where the server genuinely archived the card while an offline
        // mutation was queued. NEVER discard any other `SyncItemState::Failed`!
        let mut retry_used = false;
        for round in 0..8usize {
            let sync_items = world.client_sync_items();
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
                    let raw_cid = canban_types::split_qualified_host_id(&i.card_id)
                        .map(|(_, r)| r)
                        .unwrap_or(i.card_id.as_str());
                    world
                        .server(srv_name)
                        .and_then(|s| s.snapshot().ok())
                        .and_then(|snap| snap.cards.into_iter().find(|c| c.id == raw_cid))
                        .is_some_and(|c| !c.active())
                })
                .cloned()
                .collect();
            if conflicts.is_empty() && expected_archived_fails.is_empty() {
                break;
            }
            for item in expected_archived_fails {
                world.client_sync_action(&item.host_id, &item.mutation_id, SyncAction::Discard)?;
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
                world.client_sync_action(&item.host_id, &item.mutation_id, action)?;
            }
            world.heal_and_quiesce(20_000)?;
        }

        // Explicitly assert remote global-env convergence when global env was saved on Desktop!
        if profile.desktop_local && env_secret_saved_count > 0 {
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
        // divergence, SQLite integrity error, post-bounce stuck state, or secret leak!).
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

fn pick_active_card(
    cards: &[(String, String, String)],
    archived: &BTreeSet<String>,
    pick: usize,
) -> Option<(String, String, String)> {
    let active: Vec<_> = cards
        .iter()
        .filter(|(_, _, cid)| !archived.contains(cid))
        .collect();
    if active.is_empty() {
        return None;
    }
    Some(active[pick % active.len()].clone())
}

fn lookup_card_rev(world: &SimWorld, qualified_cid: &str) -> u64 {
    world
        .client_merged_snapshot()
        .ok()
        .and_then(|snap| {
            snap.cards
                .iter()
                .find(|c| c.id == qualified_cid)
                .map(|c| c.revision)
        })
        .unwrap_or(1)
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
