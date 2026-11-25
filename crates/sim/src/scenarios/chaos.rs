//! Swarm-Testing Randomized Chaos DST + Deterministic Shrinker (`chaos::shrink`):
//! - 6 focused deterministic Swarm Sub-Profiles (`ChaosSubProfile`: `DeliveryAndStorage`,
//!   `WorkflowAndSchedule`, `AuthMultiClientBlackhole`, `SessionsMentionReclaimClock`,
//!   `BundleGitHubPartition`, `FullMix`) selected deterministically from `seed` and preserved
//!   verbatim by `chaos::shrink`.
//! - 100% materialized `Vec<ChaosOp>` enabling exact replay & prefix+ddmin shrinking across
//!   core Kanban CRUD, workflows (`agent`/`script`/`deliver` + `workflow.apply` hot-updates),
//!   cron schedules (`skip`/`queue`/`allow`, carded & cardless), worktree + merge batches,
//!   `StorageFaultInjector` (`Busy`/`IoError`), `FaultyStream` byte-stream faults & >keepalive
//!   half-open blackhole stalls, 2-client concurrent races, `agent.*` / `@mention` / `card.adopt`
//!   / `viewer.presence`, `review.*` / `github.pr.*`, `draft.*`, `subscription.*`,
//!   `project.export` / `project.import`, live `device.revoke` + re-pair, secondary
//!   `project.archive`/`restore`, NTP clock steps/drift, and `FsDurability::Full` slices.
//! - Honest Armed-vs-Fired fault telemetry (`record_fault_armed` / `record_fault_fired`) with
//!   zero vacuous armed faults, split `chaos.delivery_batch_entered` vs `chaos.worktree_delivery_ops`,
//!   live-stream-gated `chaos.faulty_stream_*`, and a natural-quiesce verification slice before
//!   final forced heal and strict `InvariantOracle` checks.
use crate::{
    exec::{SimGitRepo, SimShellOutcome},
    github::SimIssue,
    herdr::RunFault,
    net::StreamFaultConfig,
    oracle::InvariantOracle,
    scenarios::finalize_digest,
    world::{
        FsDurability, PRIMARY_CLIENT, SIM_PORT, SimWorld, SimWorldConfig, StorageFaultKind,
        run_with_world,
    },
};
use anyhow::{Context, Result, bail, ensure};
use canban_herdr::HerdrBackend;
use canban_types::{
    EnvEntryMeta, FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE, FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB,
    FP_HERDR_AFTER_SEND_BEFORE_DB, FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB,
    FP_OUTBOX_MID_WRITE_TRUNCATE, FailpointAction, HostConnectionState, HostTransport, SavedHost,
    SyncAction, SyncItemState, unqualified_entity_id,
};
use rand::{Rng, SeedableRng, rngs::StdRng};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const SECONDARY_CLIENT: &str = "ui-client-2";
const NUM_CHAOS_OP_KINDS: usize = 39;

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
    next: ship
  - id: ship
    stage: review
    uses: deliver
    with:
      target_branch: main
      strategy: rebase
      sync_upstream: true
      batch_size: 2
      test_command: sim-chaos-test --workspace
      max_conflict_attempts: 2
    next: finish
    on_changes_requested: plan
  - id: finish
    stage: review
    uses: complete
"#;

const CHAOS_REVIEW_WORKTREE_YAML: &str = r#"schema: 12
id: chaos-review-flow
name: Chaos Review Worktree Flow
workspace:
  create_worktree: true
  sync_base: false
on:
  start:
    event: manual
    start_at: finish
steps:
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
  cron_allow:
    cron: "* * * * *"
    timezone: UTC
    overlap: allow
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

/// Focused Swarm Sub-Profile concentrating stress on specific subsystems while preserving
/// exact determinism for `(seed, ops)` replay and `chaos::shrink`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChaosSubProfile {
    DeliveryAndStorage,
    WorkflowAndSchedule,
    AuthMultiClientBlackhole,
    SessionsMentionReclaimClock,
    BundleGitHubPartition,
    FullMix,
}

impl ChaosSubProfile {
    pub const ALL: [ChaosSubProfile; 6] = [
        Self::DeliveryAndStorage,
        Self::WorkflowAndSchedule,
        Self::AuthMultiClientBlackhole,
        Self::SessionsMentionReclaimClock,
        Self::BundleGitHubPartition,
        Self::FullMix,
    ];

    pub fn from_seed(seed: u64) -> Self {
        match seed % 6 {
            0 => Self::DeliveryAndStorage,
            1 => Self::WorkflowAndSchedule,
            2 => Self::AuthMultiClientBlackhole,
            3 => Self::SessionsMentionReclaimClock,
            4 => Self::BundleGitHubPartition,
            _ => Self::FullMix,
        }
    }

    pub fn snake_name(self) -> &'static str {
        match self {
            Self::DeliveryAndStorage => "delivery_and_storage",
            Self::WorkflowAndSchedule => "workflow_and_schedule",
            Self::AuthMultiClientBlackhole => "auth_multi_client_blackhole",
            Self::SessionsMentionReclaimClock => "sessions_mention_reclaim_clock",
            Self::BundleGitHubPartition => "bundle_github_partition",
            Self::FullMix => "full_mix",
        }
    }
}

/// Per-seed Swarm-Testing profile randomizing topology, fault envelopes, and op weights.
#[derive(Clone, Debug)]
pub struct ChaosProfile {
    pub sub_profile: ChaosSubProfile,
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
    pub enable_half_open_blackhole: bool,
    pub enable_multi_client: bool,
    pub enable_schedules: bool,
    pub enable_worktree_delivery: bool,
    pub enable_auth_faults: bool,
    pub enable_project_archive: bool,
    pub enable_storage_faults: bool,
    pub enable_sessions: bool,
    pub enable_workflow_hot_update: bool,
    pub enable_review_pr: bool,
    pub enable_drafts: bool,
    pub enable_subscriptions: bool,
    pub enable_bundle_ops: bool,
    pub enable_dual_write_failpoints: bool,
    pub framed_local_transport: bool,
    pub full_durability: bool,
    pub weights: [u32; NUM_CHAOS_OP_KINDS],
}

impl ChaosProfile {
    pub fn from_seed(seed: u64) -> Self {
        let sub_profile = ChaosSubProfile::from_seed(seed);
        let mut rng = StdRng::seed_from_u64(seed ^ 0xA5A5_5A5A_C0DE_CAFE);

        let desktop_local = match sub_profile {
            ChaosSubProfile::AuthMultiClientBlackhole => rng.random_bool(0.70),
            ChaosSubProfile::SessionsMentionReclaimClock => rng.random_bool(0.75),
            ChaosSubProfile::BundleGitHubPartition => rng.random_bool(0.75),
            _ => rng.random_bool(0.65),
        };
        let two_remotes = match sub_profile {
            ChaosSubProfile::AuthMultiClientBlackhole | ChaosSubProfile::BundleGitHubPartition => {
                true
            }
            _ => !desktop_local || rng.random_bool(0.8),
        };
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
        let max_latency_ms = min_latency_ms + rng.random_range(10..=40);
        let base_fail_rate = match sub_profile {
            ChaosSubProfile::DeliveryAndStorage => 0.0,
            _ if rng.random_bool(0.25) => rng.random_range(0.01..=0.04),
            _ => 0.0,
        };
        let max_agents = match sub_profile {
            ChaosSubProfile::WorkflowAndSchedule => rng.random_range(1..=2),
            ChaosSubProfile::DeliveryAndStorage => rng.random_range(2..=4),
            _ => rng.random_range(1..=4),
        };

        // Deterministic full-durability slice (`FsDurability::Full` + `PRAGMA synchronous = FULL`)
        // on 1/8th of seeds so CI stays fast while verifying real fsync paths under chaos.
        let full_durability = seed.is_multiple_of(8);

        // Exercise framed local transport (`AppServer::serve_local_stream` over `turmoil::net`)
        // deterministically on half of desktop sub-profiles while preserving in-process coverage
        // on the other half.
        let framed_local_transport = desktop_local
            && matches!(
                sub_profile,
                ChaosSubProfile::AuthMultiClientBlackhole
                    | ChaosSubProfile::WorkflowAndSchedule
                    | ChaosSubProfile::FullMix
            );

        // Enable per-instance dual-write crash failpoints on focused fault & full-mix sub-profiles.
        let enable_dual_write_failpoints = matches!(
            sub_profile,
            ChaosSubProfile::DeliveryAndStorage
                | ChaosSubProfile::SessionsMentionReclaimClock
                | ChaosSubProfile::BundleGitHubPartition
                | ChaosSubProfile::AuthMultiClientBlackhole
                | ChaosSubProfile::FullMix
        );

        let (
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
            enable_half_open_blackhole,
            enable_multi_client,
            enable_schedules,
            enable_worktree_delivery,
            enable_auth_faults,
            enable_project_archive,
            enable_storage_faults,
            enable_sessions,
            enable_workflow_hot_update,
            enable_review_pr,
            enable_drafts,
            enable_subscriptions,
            enable_bundle_ops,
        ) = match sub_profile {
            ChaosSubProfile::DeliveryAndStorage => (
                false,                 // partitions off so delivery batches progress cleanly
                true,                  // server crash/bounce
                rng.random_bool(0.30), // occasional client bounce
                rng.random_bool(0.45), // herdr faults
                false,                 // github faults off
                true,                  // workflows ON
                false,                 // reclaim jumps off
                desktop_local && rng.random_bool(0.35),
                false,                 // clock faults off
                false,                 // colocate off
                false,                 // stream faults off
                false,                 // blackhole off
                false,                 // single client
                false,                 // schedules off
                true,                  // worktree + uses: deliver ON
                false,                 // auth faults off
                rng.random_bool(0.30), // archive/restore
                true,                  // storage Busy/IoError ON
                rng.random_bool(0.30), // sessions
                rng.random_bool(0.35), // workflow hot-update
                false,                 // review/PR off (deliver owns worktree merge)
                rng.random_bool(0.35), // drafts
                false,                 // subscriptions off
                false,                 // bundle ops off
            ),
            ChaosSubProfile::WorkflowAndSchedule => (
                rng.random_bool(0.40), // light partitions
                true,                  // crashes
                rng.random_bool(0.35), // client bounce
                true,                  // herdr faults
                rng.random_bool(0.35), // github faults
                true,                  // workflows ON
                true,                  // time jumps for cron ticks
                desktop_local && rng.random_bool(0.45),
                rng.random_bool(0.40), // clock faults
                rng.random_bool(0.50), // colocate
                false,                 // stream faults off
                false,                 // blackhole off
                rng.random_bool(0.30), // multi-client
                true,                  // cron schedules ON
                rng.random_bool(0.50), // sometimes delivery workflow
                false,                 // auth faults off
                rng.random_bool(0.30), // archive
                rng.random_bool(0.60), // storage faults
                rng.random_bool(0.40), // sessions
                true,                  // workflow.apply hot-updates ON
                false,                 // review/PR off
                rng.random_bool(0.30), // drafts
                rng.random_bool(0.50), // subscriptions
                false,                 // bundle ops off
            ),
            ChaosSubProfile::AuthMultiClientBlackhole => (
                true,                  // partitions ON
                rng.random_bool(0.55), // crashes
                true,                  // client bounce ON
                rng.random_bool(0.30), // light herdr faults
                rng.random_bool(0.35), // light github
                rng.random_bool(0.35), // light workflows
                false,                 // reclaim jumps off
                desktop_local,         // env sync ON when desktop
                rng.random_bool(0.40), // clock faults
                false,                 // colocate
                true,                  // stream faults ON
                true,                  // >keepalive half-open blackhole ON
                true,                  // 2-client concurrent races ON
                false,                 // schedules off
                false,                 // worktree delivery off
                true,                  // device.revoke + re-pair ON
                rng.random_bool(0.35), // archive
                rng.random_bool(0.40), // storage faults
                false,                 // sessions off
                false,                 // workflow hot-update off
                false,                 // review/PR off
                rng.random_bool(0.50), // drafts under multi-client
                false,                 // subscriptions off
                false,                 // bundle ops off
            ),
            ChaosSubProfile::SessionsMentionReclaimClock => (
                rng.random_bool(0.45), // partitions
                true,                  // server crash/bounce
                rng.random_bool(0.45), // client bounce
                true,                  // herdr faults ON
                rng.random_bool(0.35), // github
                rng.random_bool(0.55), // workflows
                true,                  // 65..95s reclaim jumps ON
                desktop_local,         // env sync ON (exercises clock jumps + LWW!)
                true,                  // NTP clock jumps/drift ON
                false,                 // colocate
                rng.random_bool(0.40), // stream faults
                false,                 // blackhole off
                rng.random_bool(0.40), // multi-client
                false,                 // schedules off
                false,                 // delivery off
                false,                 // auth off
                rng.random_bool(0.30), // archive
                rng.random_bool(0.55), // storage faults
                true,                  // agent.open/@mention/adopt/reply/interrupt/end ON
                false,                 // workflow hot-update off
                false,                 // review/PR off
                rng.random_bool(0.40), // drafts
                rng.random_bool(0.40), // subscriptions
                false,                 // bundle ops off
            ),
            ChaosSubProfile::BundleGitHubPartition => (
                true,                  // network partitions ON
                rng.random_bool(0.55), // crashes
                rng.random_bool(0.45), // client bounce
                rng.random_bool(0.35), // light herdr
                true,                  // GitHub sync + faults ON
                false,                 // avoid conflicting auto-delivery workflow on review cards
                false,                 // reclaim jumps off
                desktop_local && rng.random_bool(0.65),
                rng.random_bool(0.35), // clock faults
                true,                  // co-locate GitHub + SimGitRepo on remote-a for github.pr.*
                rng.random_bool(0.45), // stream faults
                false,                 // blackhole off
                rng.random_bool(0.45), // multi-client
                false,                 // schedules off
                false,                 // delivery off (review.merge / github.pr.* owns worktrees)
                false,                 // auth off
                true,                  // project.archive/restore ON
                rng.random_bool(0.45), // storage faults
                false,                 // sessions off
                false,                 // hot-update off
                true,                  // review.* + github.pr.* ON
                true,                  // draft.* ON
                true,                  // subscription.* ON
                true,                  // project.export / project.import ON
            ),
            ChaosSubProfile::FullMix => {
                let wf = rng.random_bool(0.90);
                let deliv = wf && rng.random_bool(0.60);
                let rev_pr = !deliv && rng.random_bool(0.75);
                (
                    rng.random_bool(0.85), // partitions
                    rng.random_bool(0.85), // crashes
                    rng.random_bool(0.80), // client bounce
                    rng.random_bool(0.85), // herdr faults
                    rng.random_bool(0.85), // github faults
                    wf,                    // workflows
                    rng.random_bool(0.75), // reclaim jumps
                    desktop_local && rng.random_bool(0.90),
                    rng.random_bool(0.80), // clock faults
                    rev_pr || rng.random_bool(0.60),
                    rng.random_bool(0.80), // stream faults
                    rng.random_bool(0.65), // half-open blackhole
                    rng.random_bool(0.65), // multi-client
                    rng.random_bool(0.65), // schedules
                    deliv,                 // worktree delivery
                    rng.random_bool(0.60), // auth faults
                    rng.random_bool(0.65), // project archive
                    rng.random_bool(0.85), // storage faults
                    rng.random_bool(0.75), // sessions
                    wf && rng.random_bool(0.65),
                    rev_pr,                // review/PR
                    rng.random_bool(0.75), // drafts
                    rng.random_bool(0.75), // subscriptions
                    rng.random_bool(0.65), // bundle ops
                )
            }
        };

        let mut weights = [0u32; NUM_CHAOS_OP_KINDS];
        for w in &mut weights {
            *w = rng.random_range(1..=3);
        }

        // Apply focused sub-profile weight boosts.
        match sub_profile {
            ChaosSubProfile::DeliveryAndStorage => {
                weights[0] += 6; // CreateCard
                weights[1] += 2; // UpdateCard
                weights[11] += 10; // StartWorkflow
                weights[12] += 6; // DecideWorkflow (approve precheck script_gate)
                weights[13] = 1; // StopWorkflow (keep rare so runs reach ship!)
                weights[14] += 12; // ReportExecution (biased completed)
                weights[20] += 8; // CrashAndBounceServer
                weights[22] += 3; // AdvanceTime
                weights[27] += 14; // WorktreeDeliveryOp (deliver + @merger + conflicts)
                weights[30] += 10; // StorageFault (Busy/IoError)
                weights[38] += 4; // DualWriteFailpoint
                weights[3] = 1; // ArchiveCard rare
            }
            ChaosSubProfile::WorkflowAndSchedule => {
                weights[0] += 4; // CreateCard
                weights[2] += 6; // MoveCard
                weights[3] += 4; // ArchiveCard
                weights[11] += 8; // StartWorkflow
                weights[12] += 6; // DecideWorkflow
                weights[13] += 4; // StopWorkflow
                weights[14] += 8; // ReportExecution
                weights[18] += 5; // HerdrFault
                weights[20] += 5; // CrashAndBounceServer
                weights[22] += 8; // AdvanceTime (cron boundaries)
                weights[26] += 12; // ScheduleOp
                weights[27] += 6; // WorktreeDeliveryOp (when enabled)
                weights[30] += 4; // StorageFault
                weights[33] += 8; // WorkflowHotUpdate (workflow.apply)
            }
            ChaosSubProfile::AuthMultiClientBlackhole => {
                weights[0] += 4; // CreateCard
                weights[1] += 4; // UpdateCard
                weights[4] += 3; // AddComment
                weights[10] += 4; // SaveGlobalEnv
                weights[19] += 8; // NetworkFault (partitions/oneway/hold/loss)
                weights[21] += 6; // BounceClient
                weights[24] += 10; // StreamFault (chunk/rst/eof/stall)
                weights[25] += 10; // MultiClientRace
                weights[28] += 10; // AuthRevokeRecover (invite/pair/revoke/re-pair)
                weights[31] += 8; // HalfOpenBlackhole (>keepalive)
                weights[38] += 4; // DualWriteFailpoint (FP#1/FP#5)
            }
            ChaosSubProfile::SessionsMentionReclaimClock => {
                weights[0] += 4; // CreateCard
                weights[4] += 4; // AddComment
                weights[8] += 4; // CreateShellTerminal
                weights[9] += 4; // EndShellTerminal
                weights[10] += 5; // SaveGlobalEnv (tests LWW under NTP jumps!)
                weights[15] += 8; // ViewerPresence
                weights[18] += 10; // HerdrFault (PTY crash/lose/overflow)
                weights[20] += 5; // CrashAndBounceServer
                weights[22] += 8; // AdvanceTime (65..95s reclaim jumps)
                weights[23] += 10; // ClockFault (forward/backward NTP steps & drift)
                weights[30] += 4; // StorageFault
                weights[32] += 14; // AgentSessionOp (agent.open/@mention/reply/interrupt/end/adopt)
                weights[38] += 5; // DualWriteFailpoint (FP#1/FP#2)
            }
            ChaosSubProfile::BundleGitHubPartition => {
                weights[0] += 4; // CreateCard
                weights[1] += 3; // UpdateCard
                weights[4] += 4; // AddComment (drives subscription deliveries)
                weights[17] += 10; // GitHubUpsertAndSync
                weights[19] += 8; // NetworkFault (partitions)
                weights[29] += 7; // ProjectArchiveRestore
                weights[34] += 12; // ReviewAndPrOp (review.* + github.pr.*)
                weights[35] += 8; // DraftOp (draft.*)
                weights[36] += 8; // SubscriptionOp (subscription.* + event.list)
                weights[37] += 8; // ProjectBundleOp (project.export / project.import)
                weights[38] += 4; // DualWriteFailpoint (FP#3/FP#5)
            }
            ChaosSubProfile::FullMix => {
                weights[0] += 3; // CreateCard
                weights[1] += 3; // UpdateCard
                weights[4] += 2; // AddComment
                weights[11] += 3; // StartWorkflow
                weights[14] += 3; // ReportExecution
                weights[17] += 2; // GitHubUpsertAndSync
                weights[19] += 3; // NetworkFault
                weights[20] += 3; // CrashAndBounceServer
                weights[24] += 4; // StreamFault
                weights[25] += 3; // MultiClientRace
                weights[26] += 3; // ScheduleOp
                weights[27] += 4; // WorktreeDeliveryOp
                weights[28] += 3; // AuthRevokeRecover
                weights[29] += 3; // ProjectArchiveRestore
                weights[30] += 3; // StorageFault
                weights[31] += 3; // HalfOpenBlackhole
                weights[32] += 3; // AgentSessionOp
                weights[33] += 2; // WorkflowHotUpdate
                weights[34] += 3; // ReviewAndPrOp
                weights[35] += 2; // DraftOp
                weights[36] += 2; // SubscriptionOp
                weights[37] += 2; // ProjectBundleOp
                weights[38] += 3; // DualWriteFailpoint
            }
        }

        // Zero out weights for disabled subsystems so op selection never wastes steps.
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
        if !enable_storage_faults {
            weights[30] = 0;
        }
        if !enable_half_open_blackhole {
            weights[31] = 0;
        }
        if !enable_sessions {
            weights[32] = 0;
        }
        if !enable_workflow_hot_update {
            weights[33] = 0;
        }
        if !enable_review_pr {
            weights[34] = 0;
        }
        if !enable_drafts {
            weights[35] = 0;
        }
        if !enable_subscriptions {
            weights[36] = 0;
        }
        if !enable_bundle_ops {
            weights[37] = 0;
        }
        if !enable_dual_write_failpoints {
            weights[38] = 0;
        }

        Self {
            sub_profile,
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
            enable_half_open_blackhole,
            enable_multi_client,
            enable_schedules,
            enable_worktree_delivery,
            enable_auth_faults,
            enable_project_archive,
            enable_storage_faults,
            enable_sessions,
            enable_workflow_hot_update,
            enable_review_pr,
            enable_drafts,
            enable_subscriptions,
            enable_bundle_ops,
            enable_dual_write_failpoints,
            framed_local_transport,
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
    /// SQLite `StorageFaultInjector` (`Busy` / `IoError`) armed immediately before a real write RPC
    /// followed by rollback verification and idempotent retry.
    StorageFault {
        host_idx: usize,
        is_io_error: bool,
        mutate_kind: u8,
        card_pick: usize,
        tag: u32,
    },
    /// Over-keepalive bidirectional half-open blackhole stall (`half_open_blackhole_for_ms` /
    /// `arm_half_open_blackhole`) advancing virtual time past `app.subscribe` keepalive timeout,
    /// then healing via `heal_host`.
    HalfOpenBlackhole {
        remote_idx: usize,
        duration_ms: u64,
        mutate: bool,
        card_pick: usize,
        tag: u32,
    },
    /// Interactive agent session & mention ops (`agent.open`, `agent.bind`, `@mention` comment,
    /// `agent.reply`, `agent.interrupt`, `agent.end`, `card.adopt`).
    AgentSessionOp {
        kind: u8,
        card_pick: usize,
        tag: u32,
    },
    /// Hot-update workflow YAML on `wf_host` via `workflow.sources` -> `workflow.apply`.
    WorkflowHotUpdate {
        tag: u32,
    },
    /// Review & GitHub PR delivery RPCs (`review.run_agent`, `review.preview`, `review.send_agent`,
    /// `review.merge`, `github.pr.publish`, `github.pr.refresh`, `github.pr.merge`).
    ReviewAndPrOp {
        kind: u8,
        card_pick: usize,
        tag: u32,
    },
    /// Drafts RPCs (`draft.save` create/update/conflict/tombstone + `draft.list`).
    DraftOp {
        kind: u8,
        host_idx: usize,
        draft_pick: usize,
        tag: u32,
    },
    /// Durable event journal & subscriptions (`subscription.save`, `subscription.list`,
    /// `event.list`, `subscription.deliveries`, `subscription.ack`, `subscription.retry`).
    SubscriptionOp {
        kind: u8,
        card_pick: usize,
        tag: u32,
    },
    /// Project bundle roundtrip (`project.export`, secret-leak inspection, `project.import`
    /// cross-host & duplicate rejection).
    ProjectBundleOp {
        kind: u8,
        tag: u32,
    },
    /// Per-instance dual-write crash-window failpoints (`world.failpoints` / `world.client_failpoints`)
    /// armed strictly after preconditions hold and paired immediately with their triggering op so
    /// zero vacuous armed faults occur.
    DualWriteFailpoint {
        kind: u8,
        host_idx: usize,
        card_pick: usize,
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
                approve: if profile.sub_profile == ChaosSubProfile::DeliveryAndStorage {
                    rng.random_bool(0.85)
                } else {
                    rng.random_bool(0.65)
                },
                tag,
            },
            13 => ChaosOp::StopWorkflow {
                run_pick: rng.random_range(0..32),
            },
            14 => {
                let state_idx = if profile.sub_profile == ChaosSubProfile::DeliveryAndStorage {
                    if rng.random_bool(0.75) {
                        1 // completed
                    } else {
                        rng.random_range(0..3)
                    }
                } else if profile.enable_worktree_delivery && rng.random_bool(0.55) {
                    1
                } else {
                    rng.random_range(0..3)
                };
                ChaosOp::ReportExecution {
                    term_pick: rng.random_range(0..32),
                    state_idx,
                    tag,
                }
            }
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
                let jump_prob = match profile.sub_profile {
                    ChaosSubProfile::SessionsMentionReclaimClock
                    | ChaosSubProfile::WorkflowAndSchedule => 0.55,
                    _ => 0.35,
                };
                let secs = if profile.enable_reclaim_jumps && rng.random_bool(jump_prob) {
                    rng.random_range(65..=95)
                } else {
                    rng.random_range(1..=6)
                };
                ChaosOp::AdvanceTime { secs }
            }
            23 => ChaosOp::ClockFault {
                host_idx: rng.random_range(0..16),
                kind: rng.random_range(0..2),
                delta_ms: rng.random_range(-6_000..=8_000),
                drift_ppm: rng.random_range(-5_000..=5_000),
            },
            24 => ChaosOp::StreamFault {
                remote_idx: rng.random_range(0..16),
                kind: rng.random_range(0..7),
                // Small byte budgets guarantee RST/EOF fire cleanly on a single RPC frame.
                byte_limit: rng.random_range(8..=36),
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
                enable: rng.random_bool(0.60),
                tag,
            },
            27 => ChaosOp::WorktreeDeliveryOp {
                kind: rng.random_range(0..6),
                card_pick: rng.random_range(0..64),
                tag,
            },
            28 => ChaosOp::AuthRevokeRecover {
                recover: rng.random_bool(0.55),
                tag,
            },
            29 => ChaosOp::ProjectArchiveRestore {
                archive: rng.random_bool(0.55),
                tag,
            },
            30 => ChaosOp::StorageFault {
                host_idx: rng.random_range(0..16),
                is_io_error: rng.random_bool(0.50),
                mutate_kind: rng.random_range(0..3),
                card_pick: rng.random_range(0..64),
                tag,
            },
            31 => ChaosOp::HalfOpenBlackhole {
                remote_idx: rng.random_range(0..16),
                // Exceeds the 45s `app.subscribe` heartbeat timeout (`3 * 15s`) so keepalive
                // timeout + disconnect + reconnect genuinely fires under virtual time!
                duration_ms: rng.random_range(48_000..=55_000),
                mutate: rng.random_bool(0.40),
                card_pick: rng.random_range(0..64),
                tag,
            },
            32 => ChaosOp::AgentSessionOp {
                kind: rng.random_range(0..6),
                card_pick: rng.random_range(0..64),
                tag,
            },
            33 => ChaosOp::WorkflowHotUpdate { tag },
            34 => ChaosOp::ReviewAndPrOp {
                kind: rng.random_range(0..6),
                card_pick: rng.random_range(0..64),
                tag,
            },
            35 => ChaosOp::DraftOp {
                kind: rng.random_range(0..4),
                host_idx: rng.random_range(0..16),
                draft_pick: rng.random_range(0..8),
                tag,
            },
            36 => ChaosOp::SubscriptionOp {
                kind: rng.random_range(0..5),
                card_pick: rng.random_range(0..64),
                tag,
            },
            37 => ChaosOp::ProjectBundleOp {
                kind: rng.random_range(0..3),
                tag,
            },
            _ => ChaosOp::DualWriteFailpoint {
                kind: rng.random_range(0..5),
                host_idx: rng.random_range(0..16),
                card_pick: rng.random_range(0..64),
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
                "draft_conflict",
                "sqlite_busy",
                "database is locked",
                "sqlite_ioerr",
                "disk i/o error",
                "failpoint",
                "outbox.mid_write_truncate",
                "herdr.after_terminal_run_before_db_commit",
                "herdr.after_send_before_db_commit",
                "github.after_pr_write_before_db_commit",
                "git.after_push_before_batch_finalize",
                "archived",
                "removed",
                "restore the card before",
                "terminal has no core binding",
                "terminal has no runtime binding",
                "terminal has no card agent run",
                "agent has no terminal",
                "original herdr core is unavailable",
                "caller terminal",
                "caller runtime",
                "caller directory",
                "already adopted",
                "not live",
                "provider conversation changed",
                "already replied",
                "end the previous agent",
                "not retryable",
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
                "already merged",
                "resolve the existing human decision",
                "human review must be approved",
                "resolve the existing pr delivery",
                "resolved cards do not",
                "review is stale",
                "merge unavailable",
                "no commits",
                "nothing to merge",
                "no worktree",
                "worktree",
                "pull request",
                "not linked",
                "has no branch",
                "only failed deliveries can be retried",
                "only pending",
                "not waiting",
                "not active",
                "workflow run is not",
                "batch is not",
                "batch test command",
                "live terminal",
                "unresolved",
                "cherry-pick",
                "schedule is disabled",
                "terminal finished",
                "finished",
                "rate limit",
                "403",
                "409",
                "422",
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
    cfg.framed_local_transport = profile.framed_local_transport;
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
        if profile.framed_local_transport {
            world.trace.mix_str("transport:local_framed");
        }
        world
            .trace
            .mix_str(&format!("sub_profile:{}", profile.sub_profile.snake_name()));
        world.hit(if profile.desktop_local {
            "chaos.profile.desktop"
        } else {
            "chaos.profile.mobile"
        });
        world.hit(&format!(
            "chaos.profile.{}",
            profile.sub_profile.snake_name()
        ));
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
        let needs_git_repo = profile.enable_worktree_delivery || profile.enable_review_pr;

        // Create one active project per host in deterministic order.
        let mut host_projects: Vec<(String, String)> = Vec::new();
        for host in &all_hosts {
            let dir = world.project_dir(&format!("chaos-{host}"))?;
            if host == &wf_host {
                wf_dir = dir.clone();
                if needs_git_repo {
                    let exec = world.exec_runner(&wf_host).unwrap();
                    exec.register_git_repo(&dir, SimGitRepo::default());
                    if profile.enable_worktree_delivery {
                        exec.set_script_default(
                            "chaos_precheck",
                            SimShellOutcome::ok("chaos precheck passed\n")
                                .with_output("verdict", "green"),
                        );
                        exec.set_shell_default(
                            "sim-chaos-test",
                            SimShellOutcome::ok("chaos batch tests passed\n"),
                        );
                        if seed.is_multiple_of(2)
                            && profile.sub_profile != ChaosSubProfile::DeliveryAndStorage
                        {
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
            }
            let mut create_params = json!({
                "host_id": host,
                "name": format!("Chaos {host}"),
                "directory": dir,
            });
            if host == &wf_host && needs_git_repo {
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

        // Install chaos workflow(s) (normal, worktree+script+deliver, or review-worktree,
        // plus optional cron schedule) on `remote-a`.
        let wf_pid = host_projects
            .iter()
            .find(|(h, _)| h == &wf_host)
            .map(|(_, p)| p.clone())
            .unwrap();
        let mut schedule_entries: Vec<(String, String)> = Vec::new(); // (schedule_id, rule_id)
        if profile.enable_workflows || profile.enable_schedules || profile.enable_review_pr {
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
            } else if profile.enable_review_pr {
                yaml_sources.push(json!({
                    "filename": "chaos-review-flow.yaml",
                    "source": CHAOS_REVIEW_WORKTREE_YAML,
                }));
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

        // Optional dedicated source project for `project.export` -> `project.import` roundtrips
        // so cross-host imported project IDs never disturb primary `host_projects` IDs.
        let bundle_src_host = if profile.desktop_local {
            "local".to_owned()
        } else {
            "remote-a".to_owned()
        };
        let bundle_dst_host: Option<String> =
            all_hosts.iter().find(|h| *h != &bundle_src_host).cloned();
        let mut bundle_src_pid: Option<String> = None;
        let mut bundle_exported_path: Option<String> = None;
        let mut bundle_imported_hosts: BTreeSet<String> = BTreeSet::new();
        if profile.enable_bundle_ops && bundle_dst_host.is_some() {
            let b_dir = world.project_dir("chaos-bundle-src")?;
            let bp = world.client_call(
                "project.create",
                json!({
                    "host_id": bundle_src_host,
                    "name": format!("Chaos Bundle Src {seed}"),
                    "directory": b_dir,
                }),
                None,
            )?;
            let b_pid = bp["id"].as_str().unwrap().to_owned();
            let bc = world.client_call(
                "card.create",
                json!({
                    "project_id": b_pid,
                    "title": format!("Bundle Card {seed}"),
                    "body": "exported across hosts",
                }),
                Some(&b_pid),
            )?;
            let b_cid = bc["id"].as_str().unwrap().to_owned();
            oracle.record_committed_card(
                server_key(&bundle_src_host),
                &b_cid,
                &b_pid,
                bc["revision"].as_u64().unwrap_or(1),
            );
            let _ = world.client_call(
                "note.save",
                json!({
                    "project_id": b_pid,
                    "title": "Bundle Note",
                    "body": format!("bundle-note-{seed}"),
                }),
                Some(&b_pid),
            )?;
            bundle_src_pid = Some(b_pid);
        }

        // Bind simulated GitHub repo — co-located on `remote-a` whenever `enable_review_pr` or
        // `colocate_workflow_and_github` is active (or single-remote topology).
        let gh_host = if profile.colocate_workflow_and_github
            || profile.enable_review_pr
            || remotes.len() == 1
        {
            "remote-a".to_owned()
        } else {
            "remote-b".to_owned()
        };
        let gh_pid = host_projects
            .iter()
            .find(|(h, _)| h == &gh_host)
            .map(|(_, p)| p.clone())
            .unwrap();
        let gh_repo = "acme/canban-sim";
        if profile.enable_github_faults || profile.enable_review_pr {
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
        let mut cards: Vec<TrackedCard> = Vec::new();
        let mut shell_terms: Vec<(String, String, String)> = Vec::new(); // (host, pid, qualified_tid)
        let mut tracked_wf_runs: Vec<(String, String, String)> = Vec::new(); // (host, pid, qualified_run_id)
        let mut archived_cards: BTreeSet<String> = BTreeSet::new();
        let mut seen_delivery_batches: BTreeSet<String> = BTreeSet::new();
        let mut draft_revs: BTreeMap<(String, String), u64> = BTreeMap::new();
        let mut tracked_subs: Vec<(String, String, String)> = Vec::new(); // (host, pid, sub_id)
        let mut queued_shell_test_fails: usize = 0;
        let mut published_pr_branches: BTreeSet<String> = BTreeSet::new();
        let mut gh_clock_high_water_ms: u64 =
            world.server(&gh_host).map(|s| s.now_ms()).unwrap_or(0);

        // Seed 1 initial card per host (2 on `wf_host` in delivery/review profiles) so early ops
        // always have targets without risking exhausting the last active card.
        for (idx, (host, pid)) in host_projects.iter().enumerate() {
            let seed_count = if host == &wf_host
                && (profile.enable_worktree_delivery || profile.enable_review_pr)
            {
                2
            } else {
                1
            };
            for sub in 0..seed_count {
                let c = world.client_call(
                    "card.create",
                    json!({
                        "project_id": pid,
                        "title": format!("Seed Card {idx}-{sub}"),
                        "body": "initial",
                    }),
                    Some(pid),
                )?;
                let cid = c["id"].as_str().unwrap().to_owned();
                oracle.observe_write_receipt(PRIMARY_CLIENT, host, &c)?;
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
                    let rkey = format!("chaos-card-{seed}-{tag}");
                    let card_title = format!("Chaos Card #{tag}");
                    if let Some(res) = check_expected_chaos_err(
                        "card.create",
                        world.client_call(
                            "card.create",
                            json!({
                                "project_id": pid,
                                "title": card_title,
                                "body": format!("seed-{seed}-tag-{tag}"),
                                "request_key": rkey,
                            }),
                            Some(pid),
                        ),
                    )? && let Some(cid) = res["id"].as_str()
                    {
                        let srv_name = server_key(host);
                        let raw_cid = strip_host_id(cid);
                        let offline_q = is_offline_queued(&res);
                        if !offline_q {
                            oracle.observe_write_receipt(PRIMARY_CLIENT, host, &res)?;
                        }
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
                        oracle.record_request_key_attempt_with_marker(
                            srv_name,
                            &rkey,
                            "card.create",
                            !offline_q && confirmed,
                            card_title,
                        );
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
                    if let Some((host, pid, cid)) =
                        pick_active_card(world, PRIMARY_CLIENT, &cards, &archived_cards, *card_pick)
                    {
                        let rev = lookup_card_rev(world, &cid);
                        if let Some(res) = check_expected_chaos_err(
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
                        )? && !is_offline_queued(&res)
                        {
                            oracle.observe_write_receipt(PRIMARY_CLIENT, &host, &res)?;
                        }
                    }
                }
                ChaosOp::MoveCard {
                    card_pick,
                    stage_idx,
                } => {
                    if let Some((host, pid, cid)) =
                        pick_active_card(world, PRIMARY_CLIENT, &cards, &archived_cards, *card_pick)
                    {
                        let rev = lookup_card_rev(world, &cid);
                        let stage = stages[*stage_idx % stages.len()];
                        if let Some(res) = check_expected_chaos_err(
                            "card.move",
                            world.client_call(
                                "card.move",
                                json!({ "card_id": cid, "revision": rev, "stage": stage }),
                                Some(&pid),
                            ),
                        )? && !is_offline_queued(&res)
                        {
                            oracle.observe_write_receipt(PRIMARY_CLIENT, &host, &res)?;
                        }
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
                        let host_active = cards
                            .iter()
                            .filter(|c| {
                                c.shared && c.host == host && !archived_cards.contains(&c.cid)
                            })
                            .count();
                        if host_active > 1 {
                            let rev = lookup_card_rev(world, &cid);
                            let srv_name = server_key(&host);
                            let raw_cid = strip_host_id(&cid);
                            let archive_res = check_expected_chaos_err(
                                "card.archive",
                                world.client_call(
                                    "card.archive",
                                    json!({ "card_id": cid, "revision": rev }),
                                    Some(&pid),
                                ),
                            )?;
                            if let Some(ref res) = archive_res
                                && !is_offline_queued(res)
                            {
                                oracle.observe_write_receipt(PRIMARY_CLIENT, &host, res)?;
                            }
                            let archived_ok = archive_res.is_some();
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
                                let _ = check_expected_chaos_err("open_card(revoked)", res)?;
                            } else {
                                let _ = res?;
                            }
                        }
                    }
                }
                ChaosOp::CreateShellTerminal { host_idx, tag } => {
                    let (host, pid) = &host_projects[*host_idx % host_projects.len()];
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
                        shell_terms.push((host.clone(), pid.clone(), tid));
                    }
                }
                ChaosOp::EndShellTerminal { term_pick } => {
                    if !shell_terms.is_empty() {
                        let idx = *term_pick % shell_terms.len();
                        let (_host, pid, tid) = shell_terms.remove(idx);
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
                        start_workflow_on_wf_host(
                            world,
                            &mut oracle,
                            &wf_host,
                            &cards,
                            &archived_cards,
                            &mut tracked_wf_runs,
                            *card_pick,
                            seed,
                            *tag,
                        )?;
                    }
                }
                ChaosOp::DecideWorkflow {
                    run_pick,
                    approve,
                    tag,
                } => {
                    let _ = decide_waiting_workflow(
                        world, &wf_host, &wf_pid, *run_pick, *approve, seed, *tag,
                    )?;
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
                    let states = ["working", "completed", "blocked"];
                    let state = states[*state_idx % states.len()];
                    let _ = report_running_execution(
                        world,
                        &wf_host,
                        profile.enable_worktree_delivery,
                        *term_pick,
                        state,
                        seed,
                        *tag,
                    )?;
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

                        // Clear stale host-offline / budget limits before deciding next fault so
                        // each armed fault is clean and observable.
                        world.github.set_host_offline(&gh_host, false);
                        world.github.set_host_rate_limit_budget(&gh_host, None, 0);
                        world.github.set_rate_limit_budget(None, 0);

                        let gh_srv_now = world.server(&gh_host).map(|s| s.now_ms()).unwrap_or(0);
                        let gh_clock_ready = gh_srv_now >= gh_clock_high_water_ms;
                        gh_clock_high_water_ms = gh_clock_high_water_ms.max(gh_srv_now);

                        let can_drive_sync = world.server(&gh_host).is_some()
                            && !partitioned_remotes.contains(&gh_host)
                            && !(auth_revoked && gh_host == auth_host)
                            && host_is_connected(world, PRIMARY_CLIENT, &gh_host)
                            && gh_clock_ready;

                        let mut armed_fault: Option<(&'static str, bool)> = None;
                        // (fault_name, expects_error_or_stat_err)
                        if can_drive_sync {
                            match fault_kind {
                                1 => {
                                    world.github.inject_error(500, vec![]);
                                    crate::trace::record_fault_armed("github.http_500");
                                    armed_fault = Some(("github.http_500", true));
                                }
                                2 => {
                                    world
                                        .github
                                        .inject_stale_response_once("/issues", json!([]));
                                    crate::trace::record_fault_armed("github.stale_response");
                                    armed_fault = Some(("github.stale_response", false));
                                }
                                3 => {
                                    world.github.inject_secondary_rate_limit(429, 10);
                                    crate::trace::record_fault_armed("github.secondary_rate_limit");
                                    armed_fault = Some(("github.secondary_rate_limit", true));
                                }
                                5 => {
                                    world.github.inject_stale_response_once("/pulls", json!([]));
                                    crate::trace::record_fault_armed("github.stale_response");
                                    armed_fault = Some(("github.stale_response", false));
                                }
                                6 => {
                                    world.github.inject_io_error(
                                        std::io::ErrorKind::ConnectionReset,
                                        "simulated github io reset",
                                    );
                                    crate::trace::record_fault_armed("github.io_error");
                                    armed_fault = Some(("github.io_error", true));
                                }
                                7 => {
                                    world.github.set_host_offline(&gh_host, true);
                                    crate::trace::record_fault_armed("github.host_offline");
                                    armed_fault = Some(("github.host_offline", true));
                                }
                                8 => {
                                    let reset_secs = (world.now_ms() / 1000).saturating_add(300);
                                    world.github.set_host_rate_limit_budget(
                                        &gh_host,
                                        Some(0),
                                        reset_secs,
                                    );
                                    crate::trace::record_fault_armed("github.rate_limit_budget");
                                    armed_fault = Some(("github.rate_limit_budget", true));
                                }
                                _ => {}
                            }
                        }

                        let stats_before = world.github.stats();
                        let sync_method = if armed_fault.is_some() {
                            "github.sync"
                        } else {
                            "github.sync.request"
                        };
                        let _sync_res = check_expected_chaos_err(
                            sync_method,
                            world.client_call(
                                sync_method,
                                json!({ "project_id": gh_pid }),
                                Some(&gh_pid),
                            ),
                        )?;
                        let mut stats_after = world.github.stats();
                        // If a transient Turmoil TCP drop prevented the RPC from reaching the
                        // server while a GitHub fault was armed, execute directly on `gh_host` so
                        // the armed GitHub fault genuinely fires in `SimGitHub`.
                        if let Some((_, expects_err)) = armed_fault {
                            let hit_already = if expects_err {
                                stats_after.responses_error > stats_before.responses_error
                            } else {
                                stats_after.total_requests > stats_before.total_requests
                            };
                            if !hit_already {
                                let _ = check_expected_chaos_err(
                                    "github.sync(server_fallback)",
                                    world.server_call(
                                        server_key(&gh_host),
                                        "github.sync",
                                        json!({ "project_id": strip_host_id(&gh_pid) }),
                                    ),
                                )?;
                                stats_after = world.github.stats();
                            }
                        }

                        if let Some((fault_name, expects_err)) = armed_fault {
                            let fired = if expects_err {
                                stats_after.responses_error > stats_before.responses_error
                            } else {
                                stats_after.total_requests > stats_before.total_requests
                            };
                            if fired {
                                crate::trace::record_fault_fired(fault_name);
                                match fault_name {
                                    "github.io_error" => world.hit("chaos.github_io_error"),
                                    "github.host_offline" => world.hit("chaos.github_host_offline"),
                                    "github.rate_limit_budget" => {
                                        world.hit("chaos.github_rate_limit_budget")
                                    }
                                    _ => {}
                                }
                            }
                            // Immediately clean up sticky per-host fault after firing so subsequent
                            // review/PR ops aren't starved unless explicitly faulted again.
                            world.github.set_host_offline(&gh_host, false);
                            world.github.set_host_rate_limit_budget(&gh_host, None, 0);
                            world.github.clear_injected_faults();
                        }
                        world.hit("chaos.github_sync");
                    }
                }
                ChaosOp::HerdrFault {
                    host_idx,
                    kind,
                    tag,
                } => {
                    let host = all_hosts[*host_idx % all_hosts.len()].clone();
                    let srv_name = server_key(&host).to_owned();
                    let host_pid = host_projects
                        .iter()
                        .find(|(h, _)| h == &host)
                        .map(|(_, p)| p.clone())
                        .unwrap();
                    if world.server(&srv_name).is_some()
                        && let Some(herdr) = world.herdr(&srv_name)
                    {
                        let addrs = herdr.terminal_addresses();
                        let live_addr = addrs
                            .iter()
                            .find(|a| {
                                herdr
                                    .terminal_record(a)
                                    .is_some_and(|r| r["state"].as_str() == Some("live"))
                            })
                            .cloned();
                        match kind {
                            0..=3 => {
                                if let Some(addr) = live_addr
                                    .as_deref()
                                    .or_else(|| addrs.first().map(String::as_str))
                                {
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
                                let now_off = !offline_herdr_hosts.contains(&host);
                                if now_off {
                                    offline_herdr_hosts.insert(host.clone());
                                    crate::trace::record_fault_armed("herdr.offline");
                                    herdr.set_offline(true);
                                    // Trigger a real Herdr-dependent RPC so offline error fires.
                                    let r = check_expected_chaos_err(
                                        "terminal.create(herdr_offline)",
                                        world.server_call(
                                            &srv_name,
                                            "terminal.create",
                                            json!({
                                                "project_id": strip_host_id(&host_pid),
                                                "name": format!("Offline Probe {tag}"),
                                            }),
                                        ),
                                    )?;
                                    world.advance(Duration::from_millis(120))?;
                                    if r.is_none() || herdr.subscriber_count() == 0 {
                                        crate::trace::record_fault_fired("herdr.offline");
                                        world.hit("chaos.herdr_offline_toggle");
                                    }
                                } else {
                                    offline_herdr_hosts.remove(&host);
                                    herdr.set_offline(false);
                                    world.hit("chaos.herdr_offline_toggle");
                                }
                            }
                            8 => {
                                if !offline_herdr_hosts.contains(&host)
                                    && herdr.subscriber_count() > 0
                                {
                                    let prev_core = herdr.core_id();
                                    crate::trace::record_fault_armed("herdr.restart_core");
                                    herdr.restart_core(format!("chaos-core-{host}-{seed}-{tag}"));
                                    let evicted = herdr.core_id() != prev_core
                                        && herdr.subscriber_count() == 0;
                                    world.advance(Duration::from_millis(250))?;
                                    if evicted {
                                        crate::trace::record_fault_fired("herdr.restart_core");
                                        world.hit("chaos.herdr_restart_core");
                                    }
                                }
                            }
                            9 => {
                                if !offline_herdr_hosts.contains(&host) {
                                    let fault = match tag % 3 {
                                        0 => RunFault::FailBeforeCreate(
                                            "simulated herdr run failure".into(),
                                        ),
                                        1 => RunFault::OrphanAfterCreate(
                                            "simulated herdr orphan timeout".into(),
                                        ),
                                        _ => RunFault::FirstMessageUnsent,
                                    };
                                    let terms_before = herdr.terminal_addresses().len();
                                    crate::trace::record_fault_armed("herdr.run_fault");
                                    herdr.inject_next_run_fault(fault.clone());
                                    // Immediately launch a shell terminal on this host so
                                    // `terminal.run` executes and consumes the armed RunFault!
                                    let res = check_expected_chaos_err(
                                        "terminal.create(run_fault)",
                                        world.server_call(
                                            &srv_name,
                                            "terminal.create",
                                            json!({
                                                "project_id": strip_host_id(&host_pid),
                                                "name": format!("RunFault Probe {tag}"),
                                            }),
                                        ),
                                    )?;
                                    world.advance(Duration::from_millis(200))?;
                                    let terms_after = herdr.terminal_addresses().len();
                                    let fired = match fault {
                                        RunFault::FailBeforeCreate(_) => {
                                            res.is_none() || terms_after == terms_before
                                        }
                                        RunFault::OrphanAfterCreate(_)
                                        | RunFault::FirstMessageUnsent => {
                                            res.is_none() || terms_after > terms_before
                                        }
                                    };
                                    if fired {
                                        crate::trace::record_fault_fired("herdr.run_fault");
                                        world.hit("chaos.herdr_run_fault");
                                    }
                                    herdr.clear_injected_faults();
                                }
                            }
                            10 => {
                                if !offline_herdr_hosts.contains(&host)
                                    && herdr.subscriber_count() > 0
                                    && let Some(addr) = live_addr.as_deref()
                                {
                                    crate::trace::record_fault_armed("herdr.event_drop");
                                    herdr.drop_next_events(1);
                                    // Emit a real event immediately so the drop fires now!
                                    herdr.set_status(addr, "working", Some(u64::from(*tag)));
                                    crate::trace::record_fault_fired("herdr.event_drop");
                                    world.hit("chaos.herdr_drop_events");
                                }
                            }
                            11 => {
                                if !offline_herdr_hosts.contains(&host)
                                    && herdr.subscriber_count() > 0
                                    && let Some(addr) = live_addr.as_deref()
                                {
                                    crate::trace::record_fault_armed("herdr.event_dup");
                                    herdr.duplicate_next_events(1);
                                    // Emit a real event immediately so duplication fires now!
                                    herdr.set_status(addr, "idle", Some(u64::from(*tag)));
                                    crate::trace::record_fault_fired("herdr.event_dup");
                                    world.hit("chaos.herdr_dup_events");
                                }
                            }
                            12 => {
                                if !offline_herdr_hosts.contains(&host)
                                    && herdr.subscriber_count() > 0
                                    && let Some(addr) = live_addr.as_deref()
                                {
                                    crate::trace::record_fault_armed("herdr.event_reorder");
                                    herdr.buffer_next_events_for_reorder(2);
                                    herdr.set_status(addr, "working", Some(u64::from(*tag)));
                                    herdr.set_status(addr, "idle", Some(u64::from(*tag) + 1));
                                    herdr.flush_reordered_events();
                                    crate::trace::record_fault_fired("herdr.event_reorder");
                                    world.hit("chaos.herdr_reorder_events");
                                }
                            }
                            13 => {
                                if !offline_herdr_hosts.contains(&host)
                                    && herdr.subscriber_count() > 0
                                    && let Some(addr) = live_addr.as_deref()
                                {
                                    let drops_before = herdr.overflow_drop_count();
                                    crate::trace::record_fault_armed("herdr.sub_overflow");
                                    herdr.simulate_subscriber_overflow(addr);
                                    if herdr.overflow_drop_count() > drops_before {
                                        crate::trace::record_fault_fired("herdr.sub_overflow");
                                        world.hit("chaos.herdr_sub_overflow");
                                    }
                                }
                            }
                            14 => {
                                // Only arm `hang_kill` when we have an owned shell terminal on
                                // this host whose live Herdr address we immediately invoke
                                // `terminal.end` on so `terminal.kill` genuinely fires!
                                if !offline_herdr_hosts.contains(&host)
                                    && let Some(pos) =
                                        shell_terms.iter().position(|(h, _, _)| h == &host)
                                {
                                    let (_, pid, q_tid) = shell_terms[pos].clone();
                                    let raw_tid = strip_host_id(&q_tid);
                                    let target_addr = world
                                        .server(&srv_name)
                                        .and_then(|s| s.snapshot().ok())
                                        .and_then(|snap| {
                                            snap.terminals
                                                .into_iter()
                                                .find(|t| t.id == raw_tid)
                                                .and_then(|t| t.runtime)
                                                .and_then(|r| r.terminal)
                                        });
                                    if let Some(addr) = target_addr {
                                        let kills_before = herdr.killed_terminals().len();
                                        crate::trace::record_fault_armed("herdr.hang_kill");
                                        herdr.set_hang_kill(&addr, true);
                                        let _ = check_expected_chaos_err(
                                            "terminal.end(hang_kill)",
                                            world.server_call(
                                                &srv_name,
                                                "terminal.end",
                                                json!({ "terminal_id": raw_tid }),
                                            ),
                                        )?;
                                        world.advance(Duration::from_millis(120))?;
                                        herdr.set_hang_kill(&addr, false);
                                        let _ = pid;
                                        if herdr.killed_terminals().len() > kills_before {
                                            crate::trace::record_fault_fired("herdr.hang_kill");
                                            world.hit("chaos.herdr_hang_kill");
                                            shell_terms.remove(pos);
                                        }
                                    }
                                }
                            }
                            _ => {
                                if !offline_herdr_hosts.contains(&host)
                                    && let Some(addr) = live_addr.as_deref()
                                    && herdr
                                        .terminal_record(addr)
                                        .is_some_and(|r| !r["pid"].is_null())
                                {
                                    herdr.clear_injected_faults();
                                    crate::trace::record_fault_armed("herdr.lose_terminal");
                                    herdr.lose_terminal(addr, "simulated_pty_lost");
                                    world.advance(Duration::from_millis(200))?;
                                    if herdr.terminal_record(addr).is_none_or(|r| {
                                        r["state"].as_str() != Some("live")
                                            || r["pid"].is_null()
                                            || r["reason"].as_str() == Some("simulated_pty_lost")
                                    }) {
                                        crate::trace::record_fault_fired("herdr.lose_terminal");
                                        world.hit("chaos.herdr_lose_terminal");
                                    }
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
                        if auth_revoked {
                            // A freshly restarted client legitimately performs 1 initial dial to
                            // re-discover AuthFailed before halting again.
                            oracle
                                .auth_failed_baselines
                                .remove(&(PRIMARY_CLIENT.to_owned(), auth_host.clone()));
                        }
                    }
                    world.hit("chaos.client_bounce");
                }
                ChaosOp::AdvanceTime { secs } => {
                    if *secs >= 60 {
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
                        let start = *remote_idx % remotes.len();
                        let live_target = (0..remotes.len()).find_map(|offset| {
                            let cand = &remotes[(start + offset) % remotes.len()];
                            let ok = !partitioned_remotes.contains(cand)
                                && !(auth_revoked && cand == &auth_host)
                                && world.server(cand).is_some()
                                && host_is_connected(world, PRIMARY_CLIENT, cand);
                            ok.then(|| cand.clone())
                        });
                        let target = live_target
                            .clone()
                            .unwrap_or_else(|| remotes[start].clone());
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
                        // Strictly gate stream faults & coverage hits on a live connected stream!
                        let can_drive_live = live_target.is_some();

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

                        if can_drive_live {
                            match kind % 7 {
                                0 | 6 => {
                                    // 1-byte framing chunking across live yamux + JSON-RPC frames.
                                    crate::trace::record_fault_armed("net.stream_chunk");
                                    world.stream_fault_policy().set_for_host(
                                        target.clone(),
                                        StreamFaultConfig {
                                            max_read_chunk: Some(1),
                                            ..StreamFaultConfig::default()
                                        },
                                    );
                                    let h = spawn_probe(world)?;
                                    let res = check_expected_chaos_err(
                                        "stream_fault.chunk",
                                        world.await_call(h, 60_000),
                                    )?;
                                    world.stream_fault_policy().clear_for_host(&target);
                                    if res.is_some() {
                                        crate::trace::record_fault_fired("net.stream_chunk");
                                        world.hit("chaos.faulty_stream_chunk");
                                    }
                                }
                                1 => {
                                    // Mid-stream TCP RST during write.
                                    crate::trace::record_fault_armed("net.stream_write_rst");
                                    world.stream_fault_policy().set_for_host(
                                        target.clone(),
                                        StreamFaultConfig {
                                            rst_after_write_bytes: Some(*byte_limit),
                                            ..StreamFaultConfig::default()
                                        },
                                    );
                                    let h = spawn_probe(world)?;
                                    let res = check_expected_chaos_err(
                                        "stream_fault.write_rst",
                                        world.await_call(h, 15_000),
                                    )?;
                                    let disconnected =
                                        !host_is_connected(world, PRIMARY_CLIENT, &target);
                                    world.stream_fault_policy().clear_for_host(&target);
                                    if res.is_none()
                                        || res.as_ref().is_some_and(is_offline_queued)
                                        || disconnected
                                    {
                                        crate::trace::record_fault_fired("net.stream_write_rst");
                                        world.hit("chaos.faulty_stream_rst");
                                    }
                                }
                                2 => {
                                    // Mid-stream TCP RST during read.
                                    crate::trace::record_fault_armed("net.stream_read_rst");
                                    world.stream_fault_policy().set_for_host(
                                        target.clone(),
                                        StreamFaultConfig {
                                            rst_after_read_bytes: Some(*byte_limit),
                                            ..StreamFaultConfig::default()
                                        },
                                    );
                                    let h = spawn_probe(world)?;
                                    let res = check_expected_chaos_err(
                                        "stream_fault.read_rst",
                                        world.await_call(h, 15_000),
                                    )?;
                                    let disconnected =
                                        !host_is_connected(world, PRIMARY_CLIENT, &target);
                                    world.stream_fault_policy().clear_for_host(&target);
                                    if res.is_none()
                                        || res.as_ref().is_some_and(is_offline_queued)
                                        || disconnected
                                    {
                                        crate::trace::record_fault_fired("net.stream_read_rst");
                                        world.hit("chaos.faulty_stream_rst");
                                    }
                                }
                                3 => {
                                    // Mid-stream clean 0-byte EOF truncation.
                                    crate::trace::record_fault_armed("net.stream_read_eof");
                                    world.stream_fault_policy().set_for_host(
                                        target.clone(),
                                        StreamFaultConfig {
                                            eof_after_read_bytes: Some(*byte_limit),
                                            ..StreamFaultConfig::default()
                                        },
                                    );
                                    let h = spawn_probe(world)?;
                                    let res = check_expected_chaos_err(
                                        "stream_fault.read_eof",
                                        world.await_call(h, 15_000),
                                    )?;
                                    let disconnected =
                                        !host_is_connected(world, PRIMARY_CLIENT, &target);
                                    world.stream_fault_policy().clear_for_host(&target);
                                    if res.is_none()
                                        || res.as_ref().is_some_and(is_offline_queued)
                                        || disconnected
                                    {
                                        crate::trace::record_fault_fired("net.stream_read_eof");
                                        world.hit("chaos.faulty_stream_rst");
                                        world.hit("chaos.faulty_stream_eof");
                                    }
                                }
                                4 => {
                                    // Transient half-open read blackhole stall -> clear to resume.
                                    crate::trace::record_fault_armed("net.stream_stall_reads");
                                    world.stream_fault_policy().set_for_host(
                                        target.clone(),
                                        StreamFaultConfig {
                                            stall_reads: true,
                                            ..StreamFaultConfig::default()
                                        },
                                    );
                                    let h = spawn_probe(world)?;
                                    world.advance(Duration::from_millis(120))?;
                                    let mut parked =
                                        world.stream_fault_policy().parked_waker_count() > 0;
                                    world.advance(Duration::from_millis(120))?;
                                    parked |= world.stream_fault_policy().parked_waker_count() > 0;
                                    world.stream_fault_policy().clear_for_host(&target);
                                    let res = check_expected_chaos_err(
                                        "stream_fault.stall_reads",
                                        world.await_call(h, 60_000),
                                    )?;
                                    if parked || res.is_none() {
                                        crate::trace::record_fault_fired("net.stream_stall_reads");
                                        world.hit("chaos.faulty_stream_stall");
                                    }
                                }
                                5 => {
                                    // Transient half-open write blackhole stall -> clear to resume.
                                    crate::trace::record_fault_armed("net.stream_stall_writes");
                                    world.stream_fault_policy().set_for_host(
                                        target.clone(),
                                        StreamFaultConfig {
                                            stall_writes: true,
                                            ..StreamFaultConfig::default()
                                        },
                                    );
                                    let h = spawn_probe(world)?;
                                    world.advance(Duration::from_millis(120))?;
                                    let mut parked =
                                        world.stream_fault_policy().parked_waker_count() > 0;
                                    world.advance(Duration::from_millis(120))?;
                                    parked |= world.stream_fault_policy().parked_waker_count() > 0;
                                    world.stream_fault_policy().clear_for_host(&target);
                                    let res = check_expected_chaos_err(
                                        "stream_fault.stall_writes",
                                        world.await_call(h, 60_000),
                                    )?;
                                    if parked || res.is_none() {
                                        crate::trace::record_fault_fired("net.stream_stall_writes");
                                        world.hit("chaos.faulty_stream_stall");
                                    }
                                }
                                _ => unreachable!(),
                            }
                        } else if kind % 7 == 6 {
                            world.stream_fault_policy().clear_for_host(&target);
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
                        let remote_active: Vec<(String, String, String)> = cards
                            .iter()
                            .filter(|c| {
                                c.shared && c.host != "local" && !archived_cards.contains(&c.cid)
                            })
                            .map(|c| (c.host.clone(), c.pid.clone(), c.cid.clone()))
                            .collect();

                        match kind % 5 {
                            0 => {
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
                                        if !is_offline_queued(&res) {
                                            oracle.observe_write_receipt(
                                                creator,
                                                &target_host,
                                                &res,
                                            )?;
                                        }
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
                                let (host, pid, cid) =
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
                                if let Some(res1) = check_expected_chaos_err(
                                    "race.c1.card.update",
                                    world.await_call(h1, 60_000),
                                )? && !is_offline_queued(&res1)
                                {
                                    oracle.observe_write_receipt(PRIMARY_CLIENT, &host, &res1)?;
                                }
                                if let Some(res2) = check_expected_chaos_err(
                                    "race.c2.card.update",
                                    world.await_call(h2, 60_000),
                                )? && !is_offline_queued(&res2)
                                {
                                    oracle.observe_write_receipt(SECONDARY_CLIENT, &host, &res2)?;
                                }
                                world.hit("chaos.multi_client_race");
                            }
                            2 if !remote_active.is_empty() => {
                                let (host, pid, cid) =
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
                                if let Some(res1) = check_expected_chaos_err(
                                    "race.c1.card.move",
                                    world.await_call(h1, 60_000),
                                )? && !is_offline_queued(&res1)
                                {
                                    oracle.observe_write_receipt(PRIMARY_CLIENT, &host, &res1)?;
                                }
                                if let Some(res2) = check_expected_chaos_err(
                                    "race.c2.card.move",
                                    world.await_call(h2, 60_000),
                                )? && !is_offline_queued(&res2)
                                {
                                    oracle.observe_write_receipt(SECONDARY_CLIENT, &host, &res2)?;
                                }
                                world.hit("chaos.multi_client_race");
                            }
                            3 if !remote_active.is_empty() => {
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
                        match kind % 6 {
                            0 => {
                                // Inject script or batch test failure to exercise rework & `@merger`.
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
                                        queued_shell_test_fails += 1;
                                    }
                                    world.hit("chaos.worktree_delivery_ops");
                                }
                            }
                            1 => {
                                // Inject a git merge conflict on the next `deliver` batch integration.
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
                                // via `batch.report`, or run a standalone worktree cycle.
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
                                        if seen_delivery_batches.insert(batch.id.clone()) {
                                            world.hit("chaos.delivery_batch_entered");
                                        }
                                        if let Some(exec) = world.exec_runner(&wf_host)
                                            && Path::new(&batch.worktree).is_dir()
                                        {
                                            let evict_card = batch
                                                .conflict_card_number
                                                .filter(|_| tag.is_multiple_of(2));
                                            let rkey = format!("chaos-batch-{seed}-{tag}");
                                            if let Some(cnum) = evict_card {
                                                let _ =
                                                    exec.abort_worktree_conflict(&batch.worktree);
                                                let params = json!({
                                                    "batch_id": batch.id,
                                                    "state": "completed",
                                                    "evict": [cnum],
                                                    "message": format!("chaos evict card {cnum}"),
                                                    "request_key": rkey,
                                                });
                                                let res = check_expected_chaos_err(
                                                    "batch.report",
                                                    world.server_call(
                                                        &wf_host,
                                                        "batch.report",
                                                        params,
                                                    ),
                                                )?;
                                                oracle.record_request_key_attempt(
                                                    &wf_host,
                                                    &rkey,
                                                    "batch.report",
                                                    res.is_some(),
                                                );
                                            } else {
                                                let _ = exec
                                                    .resolve_worktree_and_commit(&batch.worktree);
                                                let params = json!({
                                                    "batch_id": batch.id,
                                                    "state": "completed",
                                                    "message": format!("chaos resolve batch {tag}"),
                                                    "request_key": rkey,
                                                });
                                                let ran_test_cmds = exec
                                                    .recorded_commands()
                                                    .iter()
                                                    .filter(|c| {
                                                        c.args
                                                            .iter()
                                                            .any(|a| a.contains("sim-chaos-test"))
                                                    })
                                                    .count();
                                                let can_fire_fp4 = profile
                                                    .enable_dual_write_failpoints
                                                    && batch.sync_upstream
                                                    && ran_test_cmds >= queued_shell_test_fails
                                                    && exec.head_sha(&wf_dir).as_deref()
                                                        == Some(batch.target_head.as_str());
                                                if can_fire_fp4 {
                                                    let fp = world.failpoints(&wf_host);
                                                    let before_fired = fp
                                                        .stats_for(
                                                            FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE,
                                                        )
                                                        .fired;
                                                    fp.arm(
                                                        FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE,
                                                        FailpointAction::FailOnce(format!(
                                                            "chaos-fp4-{seed}-{tag}"
                                                        )),
                                                    );
                                                    let first = check_expected_chaos_err(
                                                        "batch.report(fp4)",
                                                        world.server_call(
                                                            &wf_host,
                                                            "batch.report",
                                                            params.clone(),
                                                        ),
                                                    )?;
                                                    fp.disarm(
                                                        FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE,
                                                    );
                                                    let fp4_fired = fp
                                                        .stats_for(
                                                            FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE,
                                                        )
                                                        .fired
                                                        > before_fired;
                                                    oracle.record_request_key_attempt(
                                                        &wf_host,
                                                        &rkey,
                                                        "batch.report",
                                                        first.is_some(),
                                                    );
                                                    if first.is_none() {
                                                        let retry = check_expected_chaos_err(
                                                            "batch.report(fp4_retry)",
                                                            world.server_call(
                                                                &wf_host,
                                                                "batch.report",
                                                                params,
                                                            ),
                                                        )?;
                                                        oracle.record_request_key_attempt(
                                                            &wf_host,
                                                            &rkey,
                                                            "batch.report",
                                                            retry.is_some(),
                                                        );
                                                    }
                                                    if fp4_fired {
                                                        world.hit("chaos.failpoint_git_after_push");
                                                    }
                                                } else {
                                                    let res = check_expected_chaos_err(
                                                        "batch.report",
                                                        world.server_call(
                                                            &wf_host,
                                                            "batch.report",
                                                            params,
                                                        ),
                                                    )?;
                                                    oracle.record_request_key_attempt(
                                                        &wf_host,
                                                        &rkey,
                                                        "batch.report",
                                                        res.is_some(),
                                                    );
                                                }
                                            };
                                        }
                                        world.hit("chaos.worktree_delivery_ops");
                                    } else if let Some(wt_res) = check_expected_chaos_err(
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
                                    )? && let Some(wt_id) =
                                        wt_res["worktree"]["id"].as_str()
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
                                        // Standalone worktree ops count ONLY as `worktree_delivery_ops`,
                                        // NEVER as `chaos.delivery_batch_entered`!
                                        world.hit("chaos.worktree_delivery_ops");
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
                            4 => {
                                // Toggle `card.hold_auto_merge` (releasing held cards so they enter delivery!).
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
                                    let raw_cid = strip_host_id(cid);
                                    let currently_held = world
                                        .server(&wf_host)
                                        .and_then(|s| s.snapshot().ok())
                                        .and_then(|snap| {
                                            snap.cards.into_iter().find(|c| c.id == raw_cid)
                                        })
                                        .is_some_and(|c| c.hold_auto_merge);
                                    let desired_hold = if currently_held {
                                        false
                                    } else {
                                        tag.is_multiple_of(3)
                                    };
                                    let rev = lookup_card_rev(world, cid);
                                    let _ = check_expected_chaos_err(
                                        "card.hold_auto_merge",
                                        world.client_call(
                                            "card.hold_auto_merge",
                                            json!({
                                                "card_id": cid,
                                                "revision": rev,
                                                "hold": desired_hold,
                                            }),
                                            Some(pid),
                                        ),
                                    )?;
                                    world.hit("chaos.worktree_delivery_ops");
                                }
                            }
                            _ => {
                                // Advance the delivery pipeline naturally: approve waiting gates,
                                // complete running worktree agents, or start a fresh delivery run!
                                let advanced = decide_waiting_workflow(
                                    world, &wf_host, &wf_pid, *card_pick, true, seed, *tag,
                                )? || report_running_execution(
                                    world,
                                    &wf_host,
                                    true,
                                    *card_pick,
                                    "completed",
                                    seed,
                                    *tag,
                                )?;
                                if !advanced {
                                    start_workflow_on_wf_host(
                                        world,
                                        &mut oracle,
                                        &wf_host,
                                        &cards,
                                        &archived_cards,
                                        &mut tracked_wf_runs,
                                        *card_pick,
                                        seed,
                                        *tag,
                                    )?;
                                }
                                world.hit("chaos.worktree_delivery_ops");
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
                                    if !partitioned_remotes.contains(&auth_host) {
                                        let _ = world.wait_until(5_000, |w| {
                                            Ok(w.client_host_summaries_on(PRIMARY_CLIENT)
                                                .iter()
                                                .any(|s| {
                                                    s.id == auth_host
                                                        && matches!(
                                                            s.connection,
                                                            HostConnectionState::AuthFailed { .. }
                                                        )
                                                }))
                                        });
                                    }
                                    oracle.check_auth_failed_zero_dials(world)?;
                                    let _ = check_expected_chaos_err(
                                        "project.list(revoked)",
                                        world.client_call(
                                            "project.list",
                                            json!({ "host_id": auth_host }),
                                            None,
                                        ),
                                    )?;
                                    oracle.check_auth_failed_zero_dials(world)?;
                                }
                            }
                        } else if *recover && !partitioned_remotes.contains(&auth_host) {
                            oracle.check_auth_failed_zero_dials(world)?;
                            world.stream_fault_policy().clear_for_host(&auth_host);
                            let repair_res = pair_host_on_primary_client(world, &auth_host)
                                .map(|dev_id| json!({ "device_id": dev_id }));
                            if let Some(val) = check_expected_chaos_err("auth.re_pair", repair_res)?
                                && let Some(new_dev_id) = val["device_id"].as_str()
                            {
                                auth_paired_device_id = Some(new_dev_id.to_owned());
                                auth_revoked = false;
                                oracle.check_auth_failed_zero_dials(world)?;
                                world.hit("chaos.auth_revoke_recover");
                            } else {
                                // Explicit user re-pair attempt may dial once before failing under
                                // transient packet loss; refresh the AuthFailed baseline so future
                                // steps strictly verify zero *background* dials.
                                oracle
                                    .auth_failed_baselines
                                    .remove(&(PRIMARY_CLIENT.to_owned(), auth_host.clone()));
                                oracle.check_auth_failed_zero_dials(world)?;
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
                        } else if let Some(res) = check_expected_chaos_err(
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
                                .and_then(|snap| snap.notes.into_iter().find(|n| n.id == raw_nid))
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
                ChaosOp::StorageFault {
                    host_idx,
                    is_io_error,
                    mutate_kind,
                    card_pick,
                    tag,
                } => {
                    if profile.enable_storage_faults {
                        let (host, pid) = host_projects[*host_idx % host_projects.len()].clone();
                        let srv_name = server_key(&host).to_owned();
                        if world.server(&srv_name).is_some()
                            && let Some(faults) = world.storage_faults(&srv_name).map(Arc::clone)
                        {
                            let fault_kind = if *is_io_error {
                                StorageFaultKind::IoError
                            } else {
                                StorageFaultKind::Busy
                            };
                            let fault_name = if *is_io_error {
                                "storage.io_error"
                            } else {
                                "storage.busy"
                            };
                            let raw_pid = strip_host_id(&pid);
                            let req_key = format!("chaos-sf-{seed}-{tag}");
                            let via_client = host == "local"
                                || (!partitioned_remotes.contains(&host)
                                    && !(auth_revoked && host == auth_host)
                                    && host_is_connected(world, PRIMARY_CLIENT, &host));

                            // Pick a concrete write RPC (card.create, comment.add, or draft.save)
                            // that executes immediately while the storage fault is armed!
                            let host_card = cards
                                .iter()
                                .filter(|c| {
                                    c.shared && c.host == host && !archived_cards.contains(&c.cid)
                                })
                                .nth(
                                    *card_pick
                                        % cards
                                            .iter()
                                            .filter(|c| {
                                                c.shared
                                                    && c.host == host
                                                    && !archived_cards.contains(&c.cid)
                                            })
                                            .count()
                                            .max(1),
                                )
                                .map(|c| c.cid.clone());

                            let before_triggers = faults.triggered_count();
                            faults.inject_next(fault_kind);
                            crate::trace::record_fault_armed(fault_name);

                            let sf_card_title = format!("SF Card #{tag}");
                            let (method, client_params, server_params, comment_meta) =
                                match mutate_kind % 3 {
                                    1 if host_card.is_some() => {
                                        let q_cid = host_card.unwrap();
                                        let raw_cid = strip_host_id(&q_cid);
                                        let body = format!("sf-comment-{seed}-{tag}");
                                        (
                                            "comment.add",
                                            json!({
                                                "card_id": q_cid,
                                                "body": body,
                                                "request_key": req_key,
                                            }),
                                            json!({
                                                "card_id": raw_cid,
                                                "body": body,
                                                "request_key": req_key,
                                            }),
                                            Some((raw_cid, body)),
                                        )
                                    }
                                    2 => {
                                        let draft_id = format!("new-card:chaos-sf-{seed}-{tag}");
                                        let data = json!({ "body": format!("sf-draft-{tag}") });
                                        (
                                            "draft.save",
                                            json!({
                                                "host_id": host,
                                                "id": draft_id,
                                                "revision": 0,
                                                "data": data,
                                            }),
                                            json!({
                                                "id": draft_id,
                                                "revision": 0,
                                                "data": data,
                                            }),
                                            None,
                                        )
                                    }
                                    _ => (
                                        "card.create",
                                        json!({
                                            "project_id": pid,
                                            "title": sf_card_title,
                                            "body": format!("sf-rollback-{seed}-{tag}"),
                                            "request_key": req_key,
                                        }),
                                        json!({
                                            "project_id": raw_pid,
                                            "title": sf_card_title,
                                            "body": format!("sf-rollback-{seed}-{tag}"),
                                            "request_key": req_key,
                                        }),
                                        None,
                                    ),
                                };

                            let first_res = if via_client {
                                check_expected_chaos_err(
                                    "storage_fault.write",
                                    world.client_call(method, client_params.clone(), Some(&pid)),
                                )?
                            } else {
                                check_expected_chaos_err(
                                    "storage_fault.server_write",
                                    world.server_call(&srv_name, method, server_params.clone()),
                                )?
                            };
                            // If a network hiccup prevented reaching SQLite, fallback directly
                            // to `server_call` while armed so the storage fault fires honestly!
                            if faults.triggered_count() == before_triggers {
                                let _ = check_expected_chaos_err(
                                    "storage_fault.server_fallback",
                                    world.server_call(&srv_name, method, server_params.clone()),
                                )?;
                            }
                            faults.clear();

                            let fired = faults.triggered_count() > before_triggers;
                            if fired {
                                crate::trace::record_fault_fired(fault_name);
                                world.hit(if *is_io_error {
                                    "chaos.storage_fault_io_error"
                                } else {
                                    "chaos.storage_fault_busy"
                                });
                                // Idempotent retry after clean SQLite rollback!
                                if method == "card.create" {
                                    oracle.record_request_key_attempt_with_marker(
                                        &srv_name,
                                        &req_key,
                                        "card.create",
                                        first_res.as_ref().is_some_and(|v| !is_offline_queued(v)),
                                        sf_card_title.clone(),
                                    );
                                } else if method == "comment.add"
                                    && let Some((_, ref body)) = comment_meta
                                {
                                    oracle.record_request_key_attempt_with_marker(
                                        &srv_name,
                                        &req_key,
                                        "comment.add",
                                        first_res.as_ref().is_some_and(|v| !is_offline_queued(v)),
                                        body.clone(),
                                    );
                                }
                                if first_res.is_none() {
                                    let retry_res = if via_client {
                                        check_expected_chaos_err(
                                            "storage_fault.retry",
                                            world.client_call(method, client_params, Some(&pid)),
                                        )?
                                    } else {
                                        check_expected_chaos_err(
                                            "storage_fault.server_retry",
                                            world.server_call(&srv_name, method, server_params),
                                        )?
                                    };
                                    if let Some(val) = retry_res {
                                        let acked = !is_offline_queued(&val);
                                        if method == "card.create"
                                            && let Some(cid) = val["id"].as_str()
                                        {
                                            if via_client && acked {
                                                oracle.observe_write_receipt(
                                                    PRIMARY_CLIENT,
                                                    &host,
                                                    &val,
                                                )?;
                                            }
                                            oracle.record_request_key_attempt_with_marker(
                                                &srv_name,
                                                &req_key,
                                                "card.create",
                                                acked,
                                                sf_card_title.clone(),
                                            );
                                            let raw_cid = strip_host_id(cid);
                                            if let Some(srv_card) = world
                                                .server(&srv_name)
                                                .and_then(|s| s.snapshot().ok())
                                                .and_then(|snap| {
                                                    snap.cards.into_iter().find(|c| c.id == raw_cid)
                                                })
                                            {
                                                oracle.record_committed_card(
                                                    &srv_name,
                                                    &srv_card.id,
                                                    &srv_card.project_id,
                                                    srv_card.revision,
                                                );
                                                let q_cid = if host == "local" {
                                                    srv_card.id
                                                } else {
                                                    canban_types::qualify_host_id(
                                                        &host,
                                                        &srv_card.id,
                                                    )
                                                };
                                                cards.push(TrackedCard {
                                                    host: host.clone(),
                                                    pid: pid.clone(),
                                                    cid: q_cid,
                                                    creator: PRIMARY_CLIENT,
                                                    shared: true,
                                                });
                                            }
                                        } else if method == "comment.add"
                                            && let Some((raw_cid, body)) = comment_meta
                                        {
                                            oracle.record_request_key_attempt_with_marker(
                                                &srv_name,
                                                &req_key,
                                                "comment.add",
                                                acked,
                                                body.clone(),
                                            );
                                            if world
                                                .server(&srv_name)
                                                .and_then(|s| s.events().ok())
                                                .is_some_and(|evs| {
                                                    evs.iter().any(|e| {
                                                        e.card_id == raw_cid
                                                            && e.kind == "comment.created"
                                                            && (e.data["body"].as_str()
                                                                == Some(body.as_str())
                                                                || e.message == body)
                                                    })
                                                })
                                            {
                                                oracle.record_committed_comment(
                                                    &srv_name, &raw_cid, body,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                ChaosOp::HalfOpenBlackhole {
                    remote_idx,
                    duration_ms,
                    mutate,
                    card_pick,
                    tag,
                } => {
                    if profile.enable_half_open_blackhole && !remotes.is_empty() {
                        let start = *remote_idx % remotes.len();
                        let live_target = (0..remotes.len()).find_map(|offset| {
                            let cand = &remotes[(start + offset) % remotes.len()];
                            let ok = !partitioned_remotes.contains(cand)
                                && !(auth_revoked && cand == &auth_host)
                                && world.server(cand).is_some()
                                && host_is_connected(world, PRIMARY_CLIENT, cand);
                            ok.then(|| cand.clone())
                        });
                        if let Some(target) = live_target {
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

                            crate::trace::record_fault_armed("net.half_open_blackhole");
                            world
                                .stream_fault_policy()
                                .arm_half_open_blackhole(&target, Some(*duration_ms));

                            let h = if *mutate && let Some(ref cid) = target_card {
                                world.client_spawn(
                                    "comment.add",
                                    json!({
                                        "card_id": cid,
                                        "body": format!("blackhole-comment-{tag}"),
                                    }),
                                    Some(&target_pid),
                                )?
                            } else {
                                world.client_spawn(
                                    "card.list",
                                    json!({ "project_id": target_pid, "limit": 5 }),
                                    Some(&target_pid),
                                )?
                            };

                            world.advance(Duration::from_millis(200))?;
                            let mut saw_parked =
                                world.stream_fault_policy().parked_waker_count() > 0;

                            // Advance virtual time in chunks past the 45s `app.subscribe`
                            // keepalive timeout until the client detects the half-open stall
                            // and transitions the host out of Connected (capped at 48s).
                            let mut elapsed_bh_ms = 200u64;
                            let mut saw_keepalive_timeout = false;
                            while elapsed_bh_ms < 48_000 {
                                world.advance(Duration::from_secs(2))?;
                                elapsed_bh_ms += 2_000;
                                saw_parked |= world.stream_fault_policy().parked_waker_count() > 0;
                                if !host_is_connected(world, PRIMARY_CLIENT, &target) {
                                    saw_keepalive_timeout = true;
                                    break;
                                }
                            }

                            world.stream_fault_policy().heal_host(&target);
                            let probe_res = check_expected_chaos_err(
                                "half_open_blackhole.probe",
                                world.await_call(h, 60_000),
                            )?;
                            if saw_parked || saw_keepalive_timeout || probe_res.is_none() {
                                crate::trace::record_fault_fired("net.half_open_blackhole");
                                world.hit("chaos.half_open_blackhole");
                                world.hit("chaos.faulty_stream_stall");
                            }
                            if saw_keepalive_timeout {
                                world.hit("chaos.keepalive_timeout_fired");
                            }
                        }
                    }
                }
                ChaosOp::AgentSessionOp {
                    kind,
                    card_pick,
                    tag,
                } => {
                    if profile.enable_sessions && world.server(&wf_host).is_some() {
                        let wf_cards: Vec<_> = cards
                            .iter()
                            .filter(|c| {
                                c.shared && c.host == wf_host && !archived_cards.contains(&c.cid)
                            })
                            .collect();
                        if !wf_cards.is_empty() {
                            let tc = wf_cards[*card_pick % wf_cards.len()];
                            let (pid, cid) = (tc.pid.clone(), tc.cid.clone());
                            let raw_cid = strip_host_id(&cid);
                            match kind % 6 {
                                0 => {
                                    // `agent.open` (+ optional `agent.bind` on fresh interactive session).
                                    if let Some(opened) = check_expected_chaos_err(
                                        "agent.open",
                                        world.client_call(
                                            "agent.open",
                                            json!({
                                                "card_id": cid,
                                                "profile_id": "codex",
                                                "request_key": format!("chaos-open-{seed}-{tag}"),
                                            }),
                                            Some(&pid),
                                        ),
                                    )? {
                                        world.hit("chaos.agent_open");
                                        world.hit("chaos.agent_session_ops");
                                        if let Some(tid) = opened["terminal_id"].as_str() {
                                            let q_tid = if tid.contains("::") {
                                                tid.to_owned()
                                            } else {
                                                canban_types::qualify_host_id(&wf_host, tid)
                                            };
                                            let _ = check_expected_chaos_err(
                                                "agent.bind",
                                                world.client_call(
                                                    "agent.bind",
                                                    json!({
                                                        "terminal_id": q_tid,
                                                        "session_id": format!("chaos-sess-{seed}-{tag}"),
                                                        "request_key": format!("chaos-bind-{seed}-{tag}"),
                                                    }),
                                                    Some(&pid),
                                                ),
                                            )?;
                                        }
                                    }
                                }
                                1 => {
                                    // `@mention` comment triggering `Question` creation + `terminal.send`!
                                    let body = format!("@codex chaos mention #{tag}");
                                    if check_expected_chaos_err(
                                        "comment.add(@mention)",
                                        world.client_call(
                                            "comment.add",
                                            json!({
                                                "card_id": cid,
                                                "body": body,
                                                "mentions": ["codex"],
                                                "request_key": format!("chaos-mention-{seed}-{tag}"),
                                            }),
                                            Some(&pid),
                                        ),
                                    )?
                                    .is_some()
                                    {
                                        if world
                                            .server(&wf_host)
                                            .and_then(|s| s.events().ok())
                                            .is_some_and(|evs| {
                                                evs.iter().any(|e| {
                                                    e.card_id == raw_cid
                                                        && e.kind == "comment.created"
                                                        && (e.data["body"].as_str()
                                                            == Some(body.as_str())
                                                            || e.message == body)
                                                })
                                            })
                                        {
                                            oracle.record_committed_comment(
                                                &wf_host, &raw_cid, body,
                                            );
                                        }
                                        world.hit("chaos.mention_comment");
                                        world.hit("chaos.agent_session_ops");
                                    }
                                }
                                2 => {
                                    // `agent.reply` to any open `Question` on `wf_host` (or create a mention first).
                                    let open_q = world
                                        .server(&wf_host)
                                        .and_then(|s| s.snapshot().ok())
                                        .and_then(|snap| {
                                            snap.questions.into_iter().find(|q| {
                                                q.state == "pending" || q.state == "delivered"
                                            })
                                        });
                                    if let Some(q) = open_q {
                                        let q_cid =
                                            canban_types::qualify_host_id(&wf_host, &q.card_id);
                                        if check_expected_chaos_err(
                                            "agent.reply",
                                            world.client_call(
                                                "agent.reply",
                                                json!({
                                                    "card_id": q_cid,
                                                    "question_id": q.id,
                                                    "body": format!("chaos reply #{tag}"),
                                                    "request_key": format!("chaos-reply-{seed}-{tag}"),
                                                }),
                                                Some(&wf_pid),
                                            ),
                                        )?
                                        .is_some()
                                        {
                                            world.hit("chaos.agent_reply");
                                            world.hit("chaos.agent_session_ops");
                                        }
                                    } else {
                                        let body = format!("@codex chaos follow-up #{tag}");
                                        if check_expected_chaos_err(
                                            "comment.add(@mention_fallback)",
                                            world.client_call(
                                                "comment.add",
                                                json!({
                                                    "card_id": cid,
                                                    "body": body,
                                                    "mentions": ["codex"],
                                                }),
                                                Some(&pid),
                                            ),
                                        )?
                                        .is_some()
                                        {
                                            world.hit("chaos.mention_comment");
                                            world.hit("chaos.agent_session_ops");
                                        }
                                    }
                                }
                                3 => {
                                    // `agent.interrupt` on a running agent terminal on `wf_host`.
                                    let running_tid = world
                                        .server(&wf_host)
                                        .and_then(|s| s.snapshot().ok())
                                        .and_then(|snap| {
                                            snap.terminals.into_iter().find(|t| {
                                                t.agent.as_ref().is_some_and(|a| {
                                                    a.state == "running" && a.terminal.is_some()
                                                })
                                            })
                                        })
                                        .map(|t| canban_types::qualify_host_id(&wf_host, &t.id));
                                    if let Some(q_tid) = running_tid {
                                        let _ = check_expected_chaos_err(
                                            "agent.interrupt",
                                            world.client_call(
                                                "agent.interrupt",
                                                json!({
                                                    "terminal_id": q_tid,
                                                    "request_key": format!("chaos-intr-{seed}-{tag}"),
                                                }),
                                                Some(&wf_pid),
                                            ),
                                        )?;
                                        world.hit("chaos.agent_session_ops");
                                    }
                                }
                                4 => {
                                    // `agent.end` on a running agent terminal (or `execution.retry` on a failed one).
                                    let snap =
                                        world.server(&wf_host).and_then(|s| s.snapshot().ok());
                                    if let Some(snap) = snap {
                                        if let Some(failed) = snap.terminals.iter().find(|t| {
                                            t.agent.as_ref().is_some_and(|a| {
                                                a.state == "failed"
                                                    && a.observed.as_ref().is_some_and(|v| {
                                                        matches!(
                                                            v["state"].as_str(),
                                                            Some("finished" | "missing")
                                                        )
                                                    })
                                            })
                                        }) {
                                            let q_tid =
                                                canban_types::qualify_host_id(&wf_host, &failed.id);
                                            let _ = check_expected_chaos_err(
                                                "execution.retry",
                                                world.client_call(
                                                    "execution.retry",
                                                    json!({
                                                        "terminal_id": q_tid,
                                                        "request_key": format!("chaos-retry-{seed}-{tag}"),
                                                    }),
                                                    Some(&wf_pid),
                                                ),
                                            )?;
                                            world.hit("chaos.agent_session_ops");
                                        } else if let Some(running) =
                                            snap.terminals.iter().find(|t| {
                                                t.agent.as_ref().is_some_and(|a| {
                                                    a.state == "running" && a.terminal.is_some()
                                                })
                                            })
                                        {
                                            let q_tid = canban_types::qualify_host_id(
                                                &wf_host,
                                                &running.id,
                                            );
                                            let _ = check_expected_chaos_err(
                                                "agent.end",
                                                world.client_call(
                                                    "agent.end",
                                                    json!({
                                                        "terminal_id": q_tid,
                                                        "request_key": format!("chaos-end-{seed}-{tag}"),
                                                    }),
                                                    Some(&wf_pid),
                                                ),
                                            )?;
                                            world.hit("chaos.agent_session_ops");
                                        }
                                    }
                                }
                                _ => {
                                    // `card.adopt`: launch a cardless agent terminal, then adopt it onto `cid`!
                                    if !offline_herdr_hosts.contains(&wf_host)
                                        && let Some(term_res) = check_expected_chaos_err(
                                            "terminal.create(adopt)",
                                            world.client_call(
                                                "terminal.create",
                                                json!({
                                                    "project_id": pid,
                                                    "agent": true,
                                                    "profile_id": "codex",
                                                    "cwd": wf_dir,
                                                    "name": format!("Adopt PTY {tag}"),
                                                }),
                                                Some(&pid),
                                            ),
                                        )?
                                        && let Some(tid) = term_res["id"].as_str()
                                    {
                                        let raw_tid = strip_host_id(tid);
                                        world.advance(Duration::from_millis(200))?;
                                        let runtime_addr = world
                                            .server(&wf_host)
                                            .and_then(|s| s.snapshot().ok())
                                            .and_then(|snap| {
                                                snap.terminals
                                                    .into_iter()
                                                    .find(|t| t.id == raw_tid)
                                                    .and_then(|t| t.runtime)
                                                    .and_then(|r| r.terminal)
                                            });
                                        if let Some(addr) = runtime_addr
                                            && let Some(adopted) = check_expected_chaos_err(
                                                "card.adopt",
                                                world.client_call(
                                                    "card.adopt",
                                                    json!({
                                                        "view": "full",
                                                        "card_id": cid,
                                                        "stage": "develop",
                                                        "profile_id": "codex",
                                                        "cwd": wf_dir,
                                                        "runtime_terminal": addr,
                                                        "runtime_session": "default",
                                                        "request_key": format!("chaos-adopt-{seed}-{tag}"),
                                                    }),
                                                    Some(&pid),
                                                ),
                                            )?
                                        {
                                            if let Some(wf_rid) =
                                                adopted["workflow_run"]["id"].as_str()
                                            {
                                                let raw_rid = strip_host_id(wf_rid);
                                                if let Some(srv_run) = world
                                                    .server(&wf_host)
                                                    .and_then(|s| s.snapshot().ok())
                                                    .and_then(|snap| {
                                                        snap.workflow_runs
                                                            .into_iter()
                                                            .find(|r| r.id == raw_rid)
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
                                            }
                                            world.hit("chaos.card_adopt");
                                            world.hit("chaos.agent_session_ops");
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                ChaosOp::WorkflowHotUpdate { tag } => {
                    if profile.enable_workflow_hot_update
                        && let Some(sources) = check_expected_chaos_err(
                            "workflow.sources",
                            world.client_call(
                                "workflow.sources",
                                json!({ "project_id": wf_pid }),
                                Some(&wf_pid),
                            ),
                        )?
                    {
                        let mut yaml_sources = Vec::new();
                        if profile.enable_workflows {
                            let base = if profile.enable_worktree_delivery {
                                CHAOS_DELIVERY_WORKFLOW_YAML
                            } else {
                                CHAOS_WORKFLOW_YAML
                            };
                            let updated = base.replace(
                                "Execute chaos step",
                                &format!("Execute chaos step rev-{tag}"),
                            );
                            yaml_sources.push(json!({
                                "filename": "chaos-flow.yaml",
                                "source": updated,
                            }));
                        }
                        if profile.enable_schedules {
                            let updated_cron = CHAOS_CRON_YAML.replace(
                                "Run scheduled chaos task",
                                &format!("Run scheduled chaos task rev-{tag}"),
                            );
                            yaml_sources.push(json!({
                                "filename": "chaos-cron.yaml",
                                "source": updated_cron,
                            }));
                        }
                        if !yaml_sources.is_empty()
                            && check_expected_chaos_err(
                                "workflow.apply",
                                world.client_call(
                                    "workflow.apply",
                                    json!({
                                        "project_id": wf_pid,
                                        "revision": sources["revision"],
                                        "sources": yaml_sources,
                                    }),
                                    Some(&wf_pid),
                                ),
                            )?
                            .is_some()
                        {
                            world.hit("chaos.workflow_hot_update");
                        }
                    }
                }
                ChaosOp::ReviewAndPrOp {
                    kind,
                    card_pick,
                    tag,
                } => {
                    if profile.enable_review_pr && world.server(&wf_host).is_some() {
                        let wf_cards: Vec<_> = cards
                            .iter()
                            .filter(|c| {
                                c.shared && c.host == wf_host && !archived_cards.contains(&c.cid)
                            })
                            .collect();
                        if !wf_cards.is_empty() {
                            let tc = wf_cards[*card_pick % wf_cards.len()];
                            let (pid, cid) = (tc.pid.clone(), tc.cid.clone());
                            let raw_cid = strip_host_id(&cid);
                            match kind % 6 {
                                0 => {
                                    // `review.run_agent` -> prepares card worktree & launches diff-review agent!
                                    if check_expected_chaos_err(
                                        "review.run_agent",
                                        world.client_call(
                                            "review.run_agent",
                                            json!({
                                                "card_id": cid,
                                                "profile_id": "developer",
                                                "focus": format!("chaos review focus #{tag}"),
                                            }),
                                            Some(&pid),
                                        ),
                                    )?
                                    .is_some()
                                    {
                                        world.advance(Duration::from_millis(160))?;
                                        // Commit a change inside the prepared worktree so preview/merge/PR have a diff.
                                        if let Some(wt) = world
                                            .server(&wf_host)
                                            .and_then(|s| s.snapshot().ok())
                                            .and_then(|snap| {
                                                snap.cards
                                                    .into_iter()
                                                    .find(|c| c.id == raw_cid)
                                                    .and_then(|c| c.worktree)
                                            })
                                            && Path::new(&wt).is_dir()
                                            && let Some(exec) = world.exec_runner(&wf_host)
                                        {
                                            let _ = exec.commit_in_worktree(
                                                &wt,
                                                &format!("feat(review): chaos commit {tag}"),
                                            );
                                            // Finish the review agent PTY cleanly on half the ops
                                            // so subsequent `review.merge` / `github.pr.publish` /
                                            // FP#3 ops see `merge_allowed: true`!
                                            if tag.is_multiple_of(2)
                                                && let Some(herdr) = world.herdr(&wf_host)
                                                && let Some(addr) = world
                                                    .server(&wf_host)
                                                    .and_then(|s| s.snapshot().ok())
                                                    .and_then(|snap| {
                                                        snap.terminals.into_iter().find_map(|t| {
                                                            let a = t.agent?;
                                                            if a.card_id == raw_cid && !a.finished()
                                                            {
                                                                a.terminal
                                                            } else {
                                                                None
                                                            }
                                                        })
                                                    })
                                            {
                                                herdr.finish_terminal(&addr, 0, "exited");
                                            }
                                        }
                                        world.hit("chaos.review_ops");
                                    }
                                }
                                1 => {
                                    // `review.preview` on any card with a worktree.
                                    if let Some(wt_card) =
                                        pick_worktree_card(world, &wf_host, &wf_cards, *card_pick)
                                    {
                                        let (w_pid, w_cid, w_path) = wt_card;
                                        if let Some(exec) = world.exec_runner(&wf_host)
                                            && Path::new(&w_path).is_dir()
                                        {
                                            let _ = exec.commit_in_worktree(
                                                &w_path,
                                                &format!("feat(preview): chaos {tag}"),
                                            );
                                        }
                                        if check_expected_chaos_err(
                                            "review.preview",
                                            world.client_call(
                                                "review.preview",
                                                json!({ "card_id": w_cid }),
                                                Some(&w_pid),
                                            ),
                                        )?
                                        .is_some()
                                        {
                                            world.hit("chaos.review_ops");
                                        }
                                    }
                                }
                                2 => {
                                    // `review.send_agent` (alternating `"card"` and `"terminal"` modes).
                                    if let Some((w_pid, w_cid, _)) =
                                        pick_worktree_card(world, &wf_host, &wf_cards, *card_pick)
                                    {
                                        let payload = if tag.is_multiple_of(2) {
                                            json!({
                                                "mode": "card",
                                                "card_id": w_cid,
                                                "profile_id": "developer",
                                                "move_to_develop": false,
                                                "context_label": format!("chaos review #{tag}"),
                                                "summary": format!("Chaos inline review #{tag}"),
                                                "comments": [{
                                                    "path": "src/change_1.rs",
                                                    "line": 1,
                                                    "side": "RIGHT",
                                                    "snippet": "// sim",
                                                    "body": format!("Check edge case #{tag}"),
                                                }],
                                                "request_key": format!("chaos-rsend-{seed}-{tag}"),
                                            })
                                        } else {
                                            json!({
                                                "mode": "terminal",
                                                "project_id": w_pid,
                                                "profile_id": "developer",
                                                "context_label": format!("chaos review term #{tag}"),
                                                "summary": format!("Chaos terminal review #{tag}"),
                                                "comments": [],
                                                "request_key": format!("chaos-rsend-{seed}-{tag}"),
                                            })
                                        };
                                        if check_expected_chaos_err(
                                            "review.send_agent",
                                            world.client_call(
                                                "review.send_agent",
                                                payload,
                                                Some(&w_pid),
                                            ),
                                        )?
                                        .is_some()
                                        {
                                            world.hit("chaos.review_ops");
                                        }
                                    }
                                }
                                3 => {
                                    // `review.merge` (only if >1 active card remains on `wf_host`).
                                    if wf_cards.len() > 1
                                        && let Some((w_pid, w_cid, w_path)) = pick_worktree_card(
                                            world, &wf_host, &wf_cards, *card_pick,
                                        )
                                    {
                                        if let Some(exec) = world.exec_runner(&wf_host)
                                            && Path::new(&w_path).is_dir()
                                        {
                                            let _ = exec.commit_in_worktree(
                                                &w_path,
                                                &format!("feat(merge): chaos {tag}"),
                                            );
                                        }
                                        if let Some(prev) = check_expected_chaos_err(
                                            "review.preview(for_merge)",
                                            world.client_call(
                                                "review.preview",
                                                json!({ "card_id": w_cid }),
                                                Some(&w_pid),
                                            ),
                                        )? && prev["merge_allowed"] == true
                                            && let Some(token) = prev["token"].as_str()
                                        {
                                            let rev = lookup_card_rev(world, &w_cid);
                                            if check_expected_chaos_err(
                                                "review.merge",
                                                world.client_call(
                                                    "review.merge",
                                                    json!({
                                                        "card_id": w_cid,
                                                        "revision": rev,
                                                        "token": token,
                                                        "request_key": format!("chaos-rmerge-{seed}-{tag}"),
                                                    }),
                                                    Some(&w_pid),
                                                ),
                                            )?
                                            .is_some()
                                            {
                                                let w_raw = strip_host_id(&w_cid);
                                                if world
                                                    .server(&wf_host)
                                                    .and_then(|s| s.snapshot().ok())
                                                    .and_then(|snap| {
                                                        snap.cards
                                                            .into_iter()
                                                            .find(|c| c.id == w_raw)
                                                    })
                                                    .is_some_and(|c| !c.active())
                                                {
                                                    archived_cards.insert(w_cid);
                                                }
                                                world.hit("chaos.review_ops");
                                            }
                                        }
                                    }
                                }
                                4 => {
                                    // `github.pr.publish` on a committed worktree card when GitHub is co-located.
                                    if gh_host == wf_host
                                        && let Some((w_pid, w_cid, w_path)) = pick_worktree_card(
                                            world, &wf_host, &wf_cards, *card_pick,
                                        )
                                    {
                                        let w_raw = strip_host_id(&w_cid);
                                        if let Some(exec) = world.exec_runner(&wf_host)
                                            && Path::new(&w_path).is_dir()
                                            && let Ok(head) = exec.commit_in_worktree(
                                                &w_path,
                                                &format!("feat(pr): chaos {tag}"),
                                            )
                                            && let Some(branch) = world
                                                .server(&wf_host)
                                                .and_then(|s| s.snapshot().ok())
                                                .and_then(|snap| {
                                                    snap.cards
                                                        .into_iter()
                                                        .find(|c| c.id == w_raw)
                                                        .and_then(|c| c.branch)
                                                })
                                        {
                                            world.github.set_branch_head(gh_repo, &branch, &head);
                                        }
                                        if let Some(prev) = check_expected_chaos_err(
                                            "review.preview(for_pr)",
                                            world.client_call(
                                                "review.preview",
                                                json!({ "card_id": w_cid }),
                                                Some(&w_pid),
                                            ),
                                        )? && prev["merge_allowed"] == true
                                            && let Some(token) = prev["token"].as_str()
                                        {
                                            let rev = lookup_card_rev(world, &w_cid);
                                            if check_expected_chaos_err(
                                                "github.pr.publish",
                                                world.client_call(
                                                    "github.pr.publish",
                                                    json!({
                                                        "card_id": w_cid,
                                                        "revision": rev,
                                                        "token": token,
                                                        "request_key": format!("chaos-prpub-{seed}-{tag}"),
                                                    }),
                                                    Some(&w_pid),
                                                ),
                                            )?
                                            .is_some()
                                            {
                                                if let Some(br) = world
                                                    .server(&wf_host)
                                                    .and_then(|s| s.snapshot().ok())
                                                    .and_then(|snap| {
                                                        snap.cards
                                                            .into_iter()
                                                            .find(|c| c.id == w_raw)
                                                            .and_then(|c| c.branch)
                                                    })
                                                {
                                                    published_pr_branches.insert(br);
                                                }
                                                world.hit("chaos.github_pr_ops");
                                            }
                                        }
                                    }
                                }
                                _ => {
                                    // `github.pr.refresh` (+ optional `github.pr.merge`) on any card with an open PR.
                                    if gh_host == wf_host
                                        && let Some(snap) =
                                            world.server(&wf_host).and_then(|s| s.snapshot().ok())
                                    {
                                        let pr_card = snap.cards.into_iter().find(|c| {
                                            c.active()
                                                && c.pull_request
                                                    .as_ref()
                                                    .is_some_and(|pr| !pr.merged)
                                        });
                                        if let Some(card) = pr_card {
                                            let q_cid =
                                                canban_types::qualify_host_id(&wf_host, &card.id);
                                            if check_expected_chaos_err(
                                                "github.pr.refresh",
                                                world.client_call(
                                                    "github.pr.refresh",
                                                    json!({ "card_id": q_cid }),
                                                    Some(&wf_pid),
                                                ),
                                            )?
                                            .is_some()
                                            {
                                                world.hit("chaos.github_pr_ops");
                                            }
                                            if tag.is_multiple_of(2)
                                                && wf_cards.len() > 1
                                                && let Some(head) = card
                                                    .pull_request
                                                    .as_ref()
                                                    .map(|pr| pr.head.clone())
                                                    .filter(|s| !s.is_empty())
                                            {
                                                let rev = lookup_card_rev(world, &q_cid);
                                                if check_expected_chaos_err(
                                                    "github.pr.merge",
                                                    world.client_call(
                                                        "github.pr.merge",
                                                        json!({
                                                            "card_id": q_cid,
                                                            "revision": rev,
                                                            "head": head,
                                                            "request_key": format!("chaos-prmerge-{seed}-{tag}"),
                                                        }),
                                                        Some(&wf_pid),
                                                    ),
                                                )?
                                                .is_some()
                                                {
                                                    if world
                                                        .server(&wf_host)
                                                        .and_then(|s| s.snapshot().ok())
                                                        .and_then(|s| {
                                                            s.cards
                                                                .into_iter()
                                                                .find(|c| c.id == card.id)
                                                        })
                                                        .is_some_and(|c| !c.active())
                                                    {
                                                        archived_cards.insert(q_cid);
                                                    }
                                                    world.hit("chaos.github_pr_ops");
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                ChaosOp::DraftOp {
                    kind,
                    host_idx,
                    draft_pick,
                    tag,
                } => {
                    if profile.enable_drafts {
                        let (host, pid) = host_projects[*host_idx % host_projects.len()].clone();
                        let draft_id = format!("new-card:chaos-{seed}-{}", *draft_pick % 3);
                        let key = (host.clone(), draft_id.clone());
                        let cur_rev = draft_revs.get(&key).copied().unwrap_or(0);
                        match kind % 4 {
                            0 | 1 => {
                                // Create or update draft at expected revision.
                                if let Some(saved) = check_expected_chaos_err(
                                    "draft.save",
                                    world.client_call(
                                        "draft.save",
                                        json!({
                                            "host_id": host,
                                            "id": draft_id,
                                            "revision": cur_rev,
                                            "data": {
                                                "title": format!("Chaos Draft {tag}"),
                                                "body": format!("draft-body-{seed}-{tag}"),
                                            },
                                        }),
                                        Some(&pid),
                                    ),
                                )? {
                                    if let Some(new_rev) = saved["revision"].as_u64() {
                                        draft_revs.insert(key, new_rev);
                                    }
                                    world.hit("chaos.draft_ops");
                                }
                            }
                            2 => {
                                // Intentionally test stale revision conflict (or tombstone if rev >= 2).
                                let (rev_to_send, data) = if cur_rev >= 2 && tag.is_multiple_of(2) {
                                    (cur_rev, Value::Null)
                                } else {
                                    (
                                        cur_rev.saturating_sub(1),
                                        json!({ "title": format!("Stale Draft {tag}") }),
                                    )
                                };
                                if let Some(saved) = check_expected_chaos_err(
                                    "draft.save(conflict_or_tombstone)",
                                    world.client_call(
                                        "draft.save",
                                        json!({
                                            "host_id": host,
                                            "id": draft_id,
                                            "revision": rev_to_send,
                                            "data": data,
                                        }),
                                        Some(&pid),
                                    ),
                                )? && let Some(new_rev) = saved["revision"].as_u64()
                                {
                                    draft_revs.insert(key, new_rev);
                                }
                                world.hit("chaos.draft_ops");
                            }
                            _ => {
                                // `draft.list`
                                if check_expected_chaos_err(
                                    "draft.list",
                                    world.client_call(
                                        "draft.list",
                                        json!({ "host_id": host }),
                                        Some(&pid),
                                    ),
                                )?
                                .is_some()
                                {
                                    world.hit("chaos.draft_ops");
                                }
                            }
                        }
                    }
                }
                ChaosOp::SubscriptionOp {
                    kind,
                    card_pick,
                    tag,
                } => {
                    if profile.enable_subscriptions {
                        match kind % 5 {
                            0 => {
                                // Register a durable consumer subscription on an active card.
                                if let Some((host, pid, cid)) = pick_active_card(
                                    world,
                                    PRIMARY_CLIENT,
                                    &cards,
                                    &archived_cards,
                                    *card_pick,
                                ) && tracked_subs.len() < 6
                                    && let Some(sub) = check_expected_chaos_err(
                                        "subscription.save",
                                        world.client_call(
                                            "subscription.save",
                                            json!({
                                                "host_id": host,
                                                "revision": 0,
                                                "enabled": true,
                                                "selector": {
                                                    "card_id": cid,
                                                    "types": ["comment.*", "card.*"],
                                                },
                                                "target": { "type": "consumer" },
                                            }),
                                            Some(&pid),
                                        ),
                                    )?
                                    && let Some(sid) = sub["id"].as_str()
                                {
                                    tracked_subs.push((host, pid, sid.to_owned()));
                                    world.hit("chaos.subscription_ops");
                                }
                            }
                            1 => {
                                // `subscription.list` on `wf_host`.
                                if check_expected_chaos_err(
                                    "subscription.list",
                                    world.client_call(
                                        "subscription.list",
                                        json!({ "host_id": wf_host }),
                                        Some(&wf_pid),
                                    ),
                                )?
                                .is_some()
                                {
                                    world.hit("chaos.subscription_ops");
                                }
                            }
                            2 => {
                                // `event.list` on `wf_host`.
                                if check_expected_chaos_err(
                                    "event.list",
                                    world.client_call(
                                        "event.list",
                                        json!({
                                            "host_id": wf_host,
                                            "after": 0,
                                            "limit": 50,
                                        }),
                                        Some(&wf_pid),
                                    ),
                                )?
                                .is_some()
                                {
                                    world.hit("chaos.subscription_ops");
                                }
                            }
                            3 => {
                                // `subscription.deliveries` + `subscription.ack` on pending deliveries.
                                if !tracked_subs.is_empty() {
                                    let (host, pid, sid) =
                                        tracked_subs[*card_pick % tracked_subs.len()].clone();
                                    if let Some(deliv) = check_expected_chaos_err(
                                        "subscription.deliveries",
                                        world.client_call(
                                            "subscription.deliveries",
                                            json!({ "host_id": host, "subscription_id": sid }),
                                            Some(&pid),
                                        ),
                                    )? {
                                        world.hit("chaos.subscription_ops");
                                        if let Some(pending_id) = deliv.as_array().and_then(|arr| {
                                            arr.iter()
                                                .find(|d| d["state"] == "pending")
                                                .and_then(|d| d["id"].as_str())
                                        }) {
                                            let _ = check_expected_chaos_err(
                                                "subscription.ack",
                                                world.client_call(
                                                    "subscription.ack",
                                                    json!({
                                                        "host_id": host,
                                                        "delivery_id": pending_id,
                                                    }),
                                                    Some(&pid),
                                                ),
                                            )?;
                                        }
                                    }
                                }
                            }
                            _ => {
                                // `subscription.retry` if any failed delivery exists, else emit a
                                // comment on a subscribed card to drive delivery generation.
                                if !tracked_subs.is_empty() {
                                    let (host, pid, sid) =
                                        tracked_subs[*card_pick % tracked_subs.len()].clone();
                                    if let Some(deliv) = check_expected_chaos_err(
                                        "subscription.deliveries(retry_scan)",
                                        world.client_call(
                                            "subscription.deliveries",
                                            json!({ "host_id": host, "subscription_id": sid }),
                                            Some(&pid),
                                        ),
                                    )? {
                                        if let Some(failed_id) = deliv.as_array().and_then(|arr| {
                                            arr.iter()
                                                .find(|d| d["state"] == "failed")
                                                .and_then(|d| d["id"].as_str())
                                        }) {
                                            let _ = check_expected_chaos_err(
                                                "subscription.retry",
                                                world.client_call(
                                                    "subscription.retry",
                                                    json!({
                                                        "host_id": host,
                                                        "delivery_id": failed_id,
                                                    }),
                                                    Some(&pid),
                                                ),
                                            )?;
                                        }
                                        world.hit("chaos.subscription_ops");
                                    }
                                } else if let Some((_host, pid, cid)) = pick_active_card(
                                    world,
                                    PRIMARY_CLIENT,
                                    &cards,
                                    &archived_cards,
                                    *card_pick,
                                ) {
                                    let _ = check_expected_chaos_err(
                                        "comment.add(sub_seed)",
                                        world.client_call(
                                            "comment.add",
                                            json!({
                                                "card_id": cid,
                                                "body": format!("sub-event-comment-{tag}"),
                                            }),
                                            Some(&pid),
                                        ),
                                    )?;
                                }
                            }
                        }
                    }
                }
                ChaosOp::ProjectBundleOp { kind, tag } => {
                    if profile.enable_bundle_ops
                        && let Some(ref b_pid) = bundle_src_pid
                        && let Some(ref dst_host) = bundle_dst_host
                    {
                        match kind % 3 {
                            0 => {
                                // `project.export` + verify bundle exists & zero leaked secrets.
                                let path = world.temp_file_path(&format!(
                                    "chaos-bundle-{seed}-{tag}.canban.tar.zst"
                                ))?;
                                let path_str = path.to_string_lossy().into_owned();
                                if let Some(exp) = check_expected_chaos_err(
                                    "project.export",
                                    world.client_call(
                                        "project.export",
                                        json!({
                                            "project_id": b_pid,
                                            "output": path_str,
                                            "include_archived": true,
                                            "overwrite": true,
                                        }),
                                        Some(b_pid),
                                    ),
                                )? && exp["exported"].as_bool() == Some(true)
                                    && path.exists()
                                {
                                    let raw_bytes = std::fs::read(&path)?;
                                    for secret in &oracle.tracked_secrets {
                                        ensure!(
                                            !raw_bytes
                                                .windows(secret.len())
                                                .any(|w| w == secret.as_bytes()),
                                            "SECURITY VIOLATION: tracked secret leaked into exported chaos bundle!"
                                        );
                                    }
                                    bundle_exported_path = Some(path_str);
                                    world.hit("chaos.bundle_export");
                                }
                            }
                            1 => {
                                // `project.import` onto distinct destination host (with `pause_github: true`).
                                if let Some(ref b_path) = bundle_exported_path
                                    && !bundle_imported_hosts.contains(dst_host)
                                {
                                    let imp_dir =
                                        world.project_dir(&format!("chaos-bundle-imp-{tag}"))?;
                                    if let Some(imp) = check_expected_chaos_err(
                                        "project.import",
                                        world.client_call(
                                            "project.import",
                                            json!({
                                                "host_id": dst_host,
                                                "file": b_path,
                                                "directory": imp_dir,
                                                "name": format!("Chaos Imported {seed}-{tag}"),
                                                "pause_github": true,
                                            }),
                                            None,
                                        ),
                                    )? && imp["imported"].as_bool() == Some(true)
                                    {
                                        bundle_imported_hosts.insert(dst_host.clone());
                                        world.hit("chaos.bundle_import");
                                    }
                                } else if bundle_exported_path.is_none() {
                                    // Export first if not yet exported.
                                    let path = world.temp_file_path(&format!(
                                        "chaos-bundle-init-{seed}-{tag}.canban.tar.zst"
                                    ))?;
                                    let path_str = path.to_string_lossy().into_owned();
                                    if let Some(exp) = check_expected_chaos_err(
                                        "project.export(auto)",
                                        world.client_call(
                                            "project.export",
                                            json!({
                                                "project_id": b_pid,
                                                "output": path_str,
                                                "include_archived": true,
                                                "overwrite": true,
                                            }),
                                            Some(b_pid),
                                        ),
                                    )? && exp["exported"].as_bool() == Some(true)
                                    {
                                        bundle_exported_path = Some(path_str);
                                        world.hit("chaos.bundle_export");
                                    }
                                }
                            }
                            _ => {
                                // Test duplicate-import rejection onto `bundle_src_host`!
                                if let Some(ref b_path) = bundle_exported_path {
                                    let dup_dir =
                                        world.project_dir(&format!("chaos-bundle-dup-{tag}"))?;
                                    let _ = check_expected_chaos_err(
                                        "project.import(duplicate)",
                                        world.client_call(
                                            "project.import",
                                            json!({
                                                "host_id": bundle_src_host,
                                                "file": b_path,
                                                "directory": dup_dir,
                                                "name": format!("Dup {tag}"),
                                                "pause_github": true,
                                            }),
                                            None,
                                        ),
                                    )?;
                                    world.hit("chaos.bundle_ops");
                                }
                            }
                        }
                    }
                }
                ChaosOp::DualWriteFailpoint {
                    kind,
                    host_idx,
                    card_pick,
                    tag,
                } => {
                    if profile.enable_dual_write_failpoints {
                        match kind % 5 {
                            0 => {
                                // FP #1: `FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB`
                                let (host, pid) = &host_projects[*host_idx % host_projects.len()];
                                let srv_name = server_key(host);
                                if world.server(srv_name).is_some()
                                    && !offline_herdr_hosts.contains(host)
                                {
                                    let fp = world.failpoints(srv_name);
                                    let before_fired =
                                        fp.stats_for(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB).fired;
                                    let raw_pid = strip_host_id(pid);
                                    fp.arm(
                                        FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB,
                                        FailpointAction::FailOnce(format!(
                                            "chaos-fp1-{seed}-{tag}"
                                        )),
                                    );
                                    let res = check_expected_chaos_err(
                                        "fp1.terminal.create",
                                        world.server_call(
                                            srv_name,
                                            "terminal.create",
                                            json!({
                                                "project_id": raw_pid,
                                                "name": format!("Chaos FP1 Shell {tag}"),
                                                "request_key": format!("chaos-fp1-term-{seed}-{tag}"),
                                            }),
                                        ),
                                    )?;
                                    fp.disarm(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB);
                                    let fp1_fired =
                                        fp.stats_for(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB).fired
                                            > before_fired;
                                    if let Some(val) = res
                                        && let Some(tid) = val["id"].as_str()
                                    {
                                        let _ = check_expected_chaos_err(
                                            "fp1.terminal.close",
                                            world.server_call(
                                                srv_name,
                                                "terminal.close",
                                                json!({
                                                    "terminal_id": tid,
                                                    "request_key": format!("chaos-fp1-close-{seed}-{tag}"),
                                                }),
                                            ),
                                        )?;
                                    }
                                    if fp1_fired {
                                        world.hit("chaos.failpoint_herdr_terminal_run");
                                    }
                                }
                            }
                            1 => {
                                // FP #2: `FP_HERDR_AFTER_SEND_BEFORE_DB`
                                if world.server(&wf_host).is_some()
                                    && !offline_herdr_hosts.contains(&wf_host)
                                    && let Some(herdr) = world.herdr(&wf_host)
                                {
                                    let cur_core_id = herdr.core_id();
                                    let target = world
                                        .server(&wf_host)
                                        .and_then(|s| s.snapshot().ok())
                                        .and_then(|snap| {
                                            snap.terminals.into_iter().find_map(|t| {
                                                let a = t.agent?;
                                                if a.state != "running"
                                                    || a.connection_state != "connected"
                                                    || a.core_id.as_deref()
                                                        != Some(cur_core_id.as_str())
                                                {
                                                    return None;
                                                }
                                                let addr = a.terminal?;
                                                let live = herdr
                                                    .call(
                                                        "terminal.get",
                                                        json!({ "terminal": addr }),
                                                    )
                                                    .ok()
                                                    .is_some_and(|v| v["state"] == "live");
                                                if !live {
                                                    return None;
                                                }
                                                let card_active = snap
                                                    .cards
                                                    .iter()
                                                    .any(|c| c.id == a.card_id && c.active());
                                                let has_open_q = snap.questions.iter().any(|q| {
                                                    q.card_id == a.card_id
                                                        && q.profile_id == a.profile.id
                                                        && matches!(
                                                            q.state.as_str(),
                                                            "pending" | "delivering"
                                                        )
                                                });
                                                if card_active && !has_open_q {
                                                    Some((a.card_id, a.profile.id))
                                                } else {
                                                    None
                                                }
                                            })
                                        });
                                    if let Some((raw_cid, prof_id)) = target {
                                        let fp = world.failpoints(&wf_host);
                                        let before_fired =
                                            fp.stats_for(FP_HERDR_AFTER_SEND_BEFORE_DB).fired;
                                        let fp2_key = format!("chaos-fp2-mention-{seed}-{tag}");
                                        let body = format!("@{prof_id} chaos fp2 mention #{tag}");
                                        fp.arm(
                                            FP_HERDR_AFTER_SEND_BEFORE_DB,
                                            FailpointAction::FailOnce(format!(
                                                "chaos-fp2-{seed}-{tag}"
                                            )),
                                        );
                                        let res = check_expected_chaos_err(
                                            "fp2.comment.add",
                                            world.server_call(
                                                &wf_host,
                                                "comment.add",
                                                json!({
                                                    "card_id": raw_cid,
                                                    "body": body,
                                                    "mentions": [prof_id],
                                                    "request_key": fp2_key,
                                                }),
                                            ),
                                        )?;
                                        let _ = world.wait_until(3_000, |w| {
                                            Ok(w.failpoints(&wf_host)
                                                .stats_for(FP_HERDR_AFTER_SEND_BEFORE_DB)
                                                .fired
                                                > before_fired)
                                        });
                                        fp.disarm(FP_HERDR_AFTER_SEND_BEFORE_DB);
                                        if res.is_some() {
                                            oracle.record_request_key_attempt_with_marker(
                                                &wf_host,
                                                &fp2_key,
                                                "comment.add",
                                                true,
                                                body.clone(),
                                            );
                                            oracle
                                                .record_committed_comment(&wf_host, &raw_cid, body);
                                        }
                                        if fp.stats_for(FP_HERDR_AFTER_SEND_BEFORE_DB).fired
                                            > before_fired
                                        {
                                            world.hit("chaos.failpoint_herdr_send");
                                        }
                                    }
                                }
                            }
                            2 => {
                                // FP #3: `FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB`
                                if profile.enable_review_pr
                                    && gh_host == wf_host
                                    && world.server(&wf_host).is_some()
                                {
                                    let raw_wf_pid = strip_host_id(&wf_pid);
                                    let candidate = world
                                        .server(&wf_host)
                                        .and_then(|s| s.snapshot().ok())
                                        .and_then(|snap| {
                                            let gh_ready = snap
                                                .projects
                                                .iter()
                                                .find(|p| p.id == raw_wf_pid)
                                                .and_then(|p| p.github.as_ref())
                                                .and_then(|g| g.synced_at)
                                                .is_some();
                                            if !gh_ready {
                                                return None;
                                            }
                                            let eligible: Vec<_> = snap
                                                .cards
                                                .iter()
                                                .filter(|c| {
                                                    let has_live_agent =
                                                        snap.terminals.iter().any(|t| {
                                                            t.agent.as_ref().is_some_and(|a| {
                                                                a.card_id == c.id && !a.finished()
                                                            })
                                                        });
                                                    c.active()
                                                        && !has_live_agent
                                                        && c.pull_request.is_none()
                                                        && c.worktree
                                                            .as_deref()
                                                            .is_some_and(|w| Path::new(w).is_dir())
                                                        && c.branch.as_deref().is_some_and(|b| {
                                                            !published_pr_branches.contains(b)
                                                        })
                                                })
                                                .collect();
                                            if eligible.is_empty() {
                                                return None;
                                            }
                                            let c = eligible[*card_pick % eligible.len()];
                                            Some((
                                                c.id.clone(),
                                                c.revision,
                                                c.worktree.clone().unwrap(),
                                                c.branch.clone().unwrap(),
                                            ))
                                        });
                                    if let Some((raw_cid, _rev, wt_path, branch)) = candidate
                                        && let Some(exec) = world.exec_runner(&wf_host)
                                    {
                                        world.github.clear_injected_faults();
                                        if let Ok(head) = exec.commit_in_worktree(
                                            &wt_path,
                                            &format!("feat(fp3): chaos pr {tag}"),
                                        ) {
                                            world.github.set_branch_head(gh_repo, &branch, &head);
                                            if let Some(prev) = check_expected_chaos_err(
                                                "fp3.review.preview",
                                                world.server_call(
                                                    &wf_host,
                                                    "review.preview",
                                                    json!({ "card_id": raw_cid }),
                                                ),
                                            )? && prev["merge_allowed"] == true
                                                && let Some(token) = prev["token"].as_str()
                                                && let Some(fresh_rev) = world
                                                    .server(&wf_host)
                                                    .and_then(|s| s.snapshot().ok())
                                                    .and_then(|snap| {
                                                        snap.cards
                                                            .into_iter()
                                                            .find(|c| c.id == raw_cid)
                                                            .map(|c| c.revision)
                                                    })
                                            {
                                                let fp = world.failpoints(&wf_host);
                                                let before_fired = fp
                                                    .stats_for(FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB)
                                                    .fired;
                                                fp.arm(
                                                    FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB,
                                                    FailpointAction::FailOnce(format!(
                                                        "chaos-fp3-{seed}-{tag}"
                                                    )),
                                                );
                                                let pub_res = check_expected_chaos_err(
                                                    "fp3.github.pr.publish",
                                                    world.server_call(
                                                        &wf_host,
                                                        "github.pr.publish",
                                                        json!({
                                                            "card_id": raw_cid,
                                                            "revision": fresh_rev,
                                                            "token": token,
                                                            "request_key": format!("chaos-fp3-pr-{seed}-{tag}"),
                                                        }),
                                                    ),
                                                )?;
                                                fp.disarm(FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB);
                                                let fp3_fired = fp
                                                    .stats_for(FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB)
                                                    .fired
                                                    > before_fired;
                                                if pub_res.is_some() {
                                                    published_pr_branches.insert(branch);
                                                    let _ = check_expected_chaos_err(
                                                        "fp3.github.pr.refresh",
                                                        world.server_call(
                                                            &wf_host,
                                                            "github.pr.refresh",
                                                            json!({ "card_id": raw_cid }),
                                                        ),
                                                    )?;
                                                }
                                                if fp3_fired {
                                                    world.hit("chaos.failpoint_github_pr_write");
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            3 => {
                                // FP #4: `FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE`
                                if profile.enable_worktree_delivery
                                    && let Some(srv) = world.server(&wf_host)
                                    && let Some(exec) = world.exec_runner(&wf_host)
                                {
                                    let snap = srv.snapshot()?;
                                    let batch = snap.merge_batches.into_iter().find(|b| {
                                        b.state == "running"
                                            && b.sync_upstream
                                            && Path::new(&b.worktree).is_dir()
                                    });
                                    let ran_test_cmds = exec
                                        .recorded_commands()
                                        .iter()
                                        .filter(|c| {
                                            c.args.iter().any(|a| a.contains("sim-chaos-test"))
                                        })
                                        .count();
                                    if let Some(batch) = batch
                                        && ran_test_cmds >= queued_shell_test_fails
                                        && exec.head_sha(&wf_dir).as_deref()
                                            == Some(batch.target_head.as_str())
                                    {
                                        oracle.record_committed_merge_batch(
                                            &wf_host,
                                            &batch.id,
                                            &batch.project_id,
                                        );
                                        let _ = exec.resolve_worktree_and_commit(&batch.worktree);
                                        let rkey = format!("chaos-fp4-batch-{seed}-{tag}");
                                        let params = json!({
                                            "batch_id": batch.id,
                                            "state": "completed",
                                            "message": format!("chaos fp4 resolve {tag}"),
                                            "request_key": rkey,
                                        });
                                        let fp = world.failpoints(&wf_host);
                                        let before_fired = fp
                                            .stats_for(FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE)
                                            .fired;
                                        fp.arm(
                                            FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE,
                                            FailpointAction::FailOnce(format!(
                                                "chaos-fp4-{seed}-{tag}"
                                            )),
                                        );
                                        let first = check_expected_chaos_err(
                                            "fp4.batch.report",
                                            world.server_call(
                                                &wf_host,
                                                "batch.report",
                                                params.clone(),
                                            ),
                                        )?;
                                        fp.disarm(FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE);
                                        let fp4_fired = fp
                                            .stats_for(FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE)
                                            .fired
                                            > before_fired;
                                        oracle.record_request_key_attempt(
                                            &wf_host,
                                            &rkey,
                                            "batch.report",
                                            first.is_some(),
                                        );
                                        if first.is_none() {
                                            let retry = check_expected_chaos_err(
                                                "fp4.batch.report.retry",
                                                world.server_call(&wf_host, "batch.report", params),
                                            )?;
                                            oracle.record_request_key_attempt(
                                                &wf_host,
                                                &rkey,
                                                "batch.report",
                                                retry.is_some(),
                                            );
                                        }
                                        if fp4_fired {
                                            world.hit("chaos.failpoint_git_after_push");
                                        }
                                    }
                                }
                            }
                            _ => {
                                // FP #5: `FP_OUTBOX_MID_WRITE_TRUNCATE`
                                // Only arm when target remote has 0 existing outbox items so
                                // quarantining the torn `.outbox.json` never drops unrelated items.
                                if !remotes.is_empty() {
                                    let candidate = remotes
                                        .iter()
                                        .rev()
                                        .find(|r| {
                                            !(auth_revoked && **r == auth_host)
                                                && world
                                                    .client_sync_items_on(PRIMARY_CLIENT)
                                                    .iter()
                                                    .all(|it| it.host_id != **r)
                                                && ((partitioned_remotes.contains(*r)
                                                    && !host_is_connected(
                                                        world,
                                                        PRIMARY_CLIENT,
                                                        r,
                                                    ))
                                                    || (profile.enable_partitions
                                                        && world.server(r).is_some()))
                                        })
                                        .cloned();
                                    if let Some(target) = candidate
                                        && let Some((_, target_pid)) = host_projects
                                            .iter()
                                            .find(|(h, _)| h == &target)
                                            .cloned()
                                    {
                                        let was_partitioned = partitioned_remotes.contains(&target);
                                        let mut crashed_for_fp5 = false;
                                        if !was_partitioned {
                                            world.crash_server(&target)?;
                                            crashed_for_fp5 = true;
                                            world.partition(PRIMARY_CLIENT, &target);
                                            let _ = world.wait_until(8_000, |w| {
                                                Ok(!host_is_connected(w, PRIMARY_CLIENT, &target))
                                            });
                                        }
                                        let still_disconnected = !host_is_connected(
                                            world,
                                            PRIMARY_CLIENT,
                                            &target,
                                        ) && !world
                                            .client_host_summaries_on(PRIMARY_CLIENT)
                                            .iter()
                                            .any(|s| {
                                                s.id == target
                                                    && matches!(
                                                        s.connection,
                                                        HostConnectionState::AuthFailed { .. }
                                                    )
                                            });
                                        if still_disconnected {
                                            let fp = world.client_failpoints(PRIMARY_CLIENT);
                                            let before_fired =
                                                fp.stats_for(FP_OUTBOX_MID_WRITE_TRUNCATE).fired;
                                            fp.arm(
                                                FP_OUTBOX_MID_WRITE_TRUNCATE,
                                                FailpointAction::FailOnce(format!(
                                                    "chaos-fp5-{seed}-{tag}"
                                                )),
                                            );
                                            let _ = check_expected_chaos_err(
                                                "fp5.outbox.card.create",
                                                world.client_call(
                                                    "card.create",
                                                    json!({
                                                        "project_id": target_pid,
                                                        "title": format!("Chaos FP5 Torn {tag}"),
                                                        "request_key": format!("chaos-fp5-{seed}-{tag}"),
                                                    }),
                                                    Some(&target_pid),
                                                ),
                                            )?;
                                            fp.disarm(FP_OUTBOX_MID_WRITE_TRUNCATE);
                                            let fp5_fired =
                                                fp.stats_for(FP_OUTBOX_MID_WRITE_TRUNCATE).fired
                                                    > before_fired;
                                            // Bounce primary client to quarantine torn `.outbox.json`.
                                            world.bounce_client()?;
                                            if auth_revoked {
                                                oracle.auth_failed_baselines.remove(&(
                                                    PRIMARY_CLIENT.to_owned(),
                                                    auth_host.clone(),
                                                ));
                                            }
                                            if fp5_fired {
                                                world.hit("chaos.failpoint_outbox_truncate");
                                            }
                                        }
                                        if !was_partitioned {
                                            world.repair(PRIMARY_CLIENT, &target);
                                        }
                                        if crashed_for_fp5 {
                                            world.bounce_server(&target)?;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Advance a small virtual tick, observe any real delivery batches, and check
            // step invariants after EVERY SINGLE OP!
            world.advance(Duration::from_millis(80))?;
            if profile.enable_worktree_delivery {
                observe_delivery_batches(
                    world,
                    &mut oracle,
                    &wf_host,
                    &mut seen_delivery_batches,
                    &mut archived_cards,
                );
            }
            if auth_revoked {
                oracle.check_auth_failed_zero_dials(world)?;
            }
            oracle.check_step_invariants(world)?;
        }

        // Clean pre-quiescence recovery:
        // 1. Repair links/stream/storage/failpoints/GitHub/Herdr/clock faults and ensure all
        //    servers/clients are running WITHOUT manually calling `reconnect` or
        //    `poll_remotes_once_async` yet!
        world.repair_all();
        world.github.clear_injected_faults();
        for h in world.servers.values() {
            h.storage_faults.clear();
            h.failpoints.clear();
            h.clock.reset_adjustments();
            h.herdr.clear_injected_faults();
            h.herdr.set_offline(false);
        }
        for c in world.clients.values() {
            c.failpoints.clear();
        }
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

        // 2. Disable cron schedules and flush any pending GitHub issue/PR cache updates before
        //    quiescence drain so background server timers do not mutate state mid-way between
        //    sequential multi-client `poll_remotes_once_async()` calls inside `heal_and_quiesce`.
        for (sid, _) in &schedule_entries {
            let _ = world.server_call(
                &wf_host,
                "schedule.toggle",
                json!({ "schedule_id": sid, "enabled": false }),
            )?;
        }
        if (profile.enable_github_faults || profile.enable_review_pr)
            && world.server(server_key(&gh_host)).is_some()
        {
            let _ = world.server_call(
                server_key(&gh_host),
                "github.sync",
                json!({ "project_id": strip_host_id(&gh_pid) }),
            );
        }

        // 3. Re-pair `auth_host` if still revoked so strict `Connected` + convergence checks run.
        if auth_revoked {
            oracle.check_auth_failed_zero_dials(world)?;
            let _new_dev_id = pair_host_on_primary_client(world, &auth_host)?;
            oracle.check_auth_failed_zero_dials(world)?;
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

        // 5. Natural Quiesce verification slice: advance virtual time WITHOUT calling manual
        //    `reconnect` / `clear_retry_backoff` / `poll_remotes_once_async` and verify
        //    unassisted reconnect + outbox drain convergence!
        world.hit("chaos.natural_quiesce_checked");
        world.quiesce_natural(120_000)?;
        world.hit("chaos.natural_quiesce_recovered");

        // Final forced heal & poll so Herdr observers and latest remote snapshots settle before
        // conflict resolution and strict quiescent oracle assertions.
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

        if profile.enable_worktree_delivery {
            observe_delivery_batches(
                world,
                &mut oracle,
                &wf_host,
                &mut seen_delivery_batches,
                &mut archived_cards,
            );
        }

        // Final strict quiescent invariant check (RYW, At-Most-Once, fails on ANY unexpected
        // `SyncItemState::Failed`, divergence on ANY client, SQLite integrity error, post-bounce
        // stuck state, or secret leak!).
        oracle.check_read_your_writes(world)?;
        oracle.check_at_most_once(world)?;
        oracle.check_quiescent_invariants(world)?;

        Ok(finalize_digest(world))
    })
}

fn is_offline_queued(val: &Value) -> bool {
    val.get("offline_queued").and_then(Value::as_bool) == Some(true)
}

fn host_is_connected(world: &SimWorld, client_id: &str, host_id: &str) -> bool {
    world
        .client_host_summaries_on(client_id)
        .iter()
        .find(|s| s.id == host_id)
        .is_some_and(|s| s.is_connected())
}

#[allow(clippy::too_many_arguments)]
fn start_workflow_on_wf_host(
    world: &mut SimWorld,
    oracle: &mut InvariantOracle,
    wf_host: &str,
    cards: &[TrackedCard],
    archived_cards: &BTreeSet<String>,
    tracked_wf_runs: &mut Vec<(String, String, String)>,
    card_pick: usize,
    seed: u64,
    tag: u32,
) -> Result<bool> {
    let wf_cards: Vec<_> = cards
        .iter()
        .filter(|c| c.shared && c.host == wf_host && !archived_cards.contains(&c.cid))
        .collect();
    if wf_cards.is_empty() {
        return Ok(false);
    }
    let tc = wf_cards[card_pick % wf_cards.len()];
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
            canban_types::qualify_host_id(wf_host, rid)
        };
        let raw_rid = strip_host_id(&q_rid);
        if let Some(srv_run) = world
            .server(wf_host)
            .and_then(|s| s.snapshot().ok())
            .and_then(|snap| snap.workflow_runs.into_iter().find(|r| r.id == raw_rid))
        {
            oracle.record_committed_workflow_run(
                wf_host,
                &srv_run.id,
                &srv_run.project_id,
                &srv_run.workflow_id,
                srv_run.visit,
            );
        }
        tracked_wf_runs.push((wf_host.to_owned(), pid.clone(), q_rid));
        world.hit("chaos.workflow_start");
        return Ok(true);
    }
    Ok(false)
}

fn decide_waiting_workflow(
    world: &mut SimWorld,
    wf_host: &str,
    wf_pid: &str,
    run_pick: usize,
    approve: bool,
    seed: u64,
    tag: u32,
) -> Result<bool> {
    if let Some(srv) = world.server(wf_host) {
        let snap = srv.snapshot()?;
        let mut waiting: Vec<_> = snap
            .workflow_runs
            .iter()
            .filter(|r| r.state == "waiting_approval")
            .collect();
        waiting.sort_by_key(|r| (r.visit, r.definition_revision));
        if !waiting.is_empty() {
            let wf = waiting[run_pick % waiting.len()];
            if let Some(raw_cid) = wf.card_id.as_deref()
                && let Some(card) = snap.cards.iter().find(|c| c.id == raw_cid)
            {
                let q_rid = canban_types::qualify_host_id(wf_host, &wf.id);
                let decision = if approve { "approve" } else { "rework" };
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
                        Some(wf_pid),
                    ),
                )?;
                world.hit("chaos.workflow_decide");
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn report_running_execution(
    world: &mut SimWorld,
    wf_host: &str,
    enable_worktree_delivery: bool,
    term_pick: usize,
    state: &str,
    seed: u64,
    tag: u32,
) -> Result<bool> {
    if let Some(srv) = world.server(wf_host) {
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
            let t = running[term_pick % running.len()];
            let mut finished_addr: Option<String> = None;
            if state == "completed"
                && enable_worktree_delivery
                && let Some(agent) = t.agent.as_ref()
            {
                if let Some(wt_path) = snap
                    .cards
                    .iter()
                    .find(|c| c.id == agent.card_id)
                    .and_then(|c| c.worktree.as_deref())
                    && Path::new(wt_path).is_dir()
                    && let Some(exec) = world.exec_runner(wf_host)
                {
                    let _ = exec.commit_in_worktree(wt_path, &format!("feat: chaos commit {tag}"));
                }
                finished_addr = agent.terminal.clone();
            }
            let _ = check_expected_chaos_err(
                "execution.report",
                world.server_call(
                    wf_host,
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
                && let Some(herdr) = world.herdr(wf_host)
            {
                herdr.finish_terminal(&addr, 0, "exited");
            }
            world.hit("chaos.execution_report");
            return Ok(true);
        }
    }
    Ok(false)
}

/// Observes real merge batch entries (`uses: deliver` -> `merge_batch` integration) on `wf_host`,
/// recording persistent `MergeBatch` durability in `oracle` and hitting `chaos.delivery_batch_entered`
/// strictly when a real merge batch enters integration (never on standalone worktree ops).
fn observe_delivery_batches(
    world: &SimWorld,
    oracle: &mut InvariantOracle,
    wf_host: &str,
    seen_delivery_batches: &mut BTreeSet<String>,
    archived_cards: &mut BTreeSet<String>,
) {
    let Some(srv) = world.server(wf_host) else {
        return;
    };
    if let Ok(snap) = srv.snapshot() {
        for batch in &snap.merge_batches {
            if matches!(
                batch.state.as_str(),
                "queued" | "running" | "completed" | "failed"
            ) {
                oracle.record_committed_merge_batch(wf_host, &batch.id, &batch.project_id);
                if seen_delivery_batches.insert(batch.id.clone()) {
                    world.hit("chaos.delivery_batch_entered");
                }
            }
        }
        for card in &snap.cards {
            if !card.active() {
                archived_cards.insert(canban_types::qualify_host_id(wf_host, &card.id));
            }
        }
    }
    if let Ok(events) = srv.events() {
        for ev in &events {
            if matches!(
                ev.kind.as_str(),
                "review.merged" | "review.batch_test_failed" | "review.merge_conflict"
            ) && let Some(bid) = ev.data["batch_id"].as_str()
                && seen_delivery_batches.insert(bid.to_owned())
            {
                world.hit("chaos.delivery_batch_entered");
            }
        }
    }
}

fn pick_worktree_card(
    world: &SimWorld,
    wf_host: &str,
    wf_cards: &[&TrackedCard],
    pick: usize,
) -> Option<(String, String, String)> {
    let snap = world.server(wf_host)?.snapshot().ok()?;
    let eligible: Vec<(String, String, String)> = wf_cards
        .iter()
        .filter_map(|tc| {
            let raw = unqualified_entity_id(&tc.cid);
            let card = snap.cards.iter().find(|c| c.id == raw && c.active())?;
            let wt = card.worktree.as_deref()?;
            if Path::new(wt).is_dir() {
                Some((tc.pid.clone(), tc.cid.clone(), wt.to_owned()))
            } else {
                None
            }
        })
        .collect();
    if eligible.is_empty() {
        return None;
    }
    Some(eligible[pick % eligible.len()].clone())
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

/// Extracts a stable error-class fingerprint prefix from a failure message so `shrink_ops`
/// preserves the original failure class instead of drifting onto an unrelated error when deleting
/// ops, while still matching when dynamic IDs, revisions, or counts shift across shrunk traces.
fn error_class_fingerprint(err: &str) -> String {
    fn strip_leading_brackets(mut s: &str) -> &str {
        loop {
            let trimmed = s.trim_start();
            if let Some(rest) = trimmed.strip_prefix('[')
                && let Some(end) = rest.find(']')
            {
                s = &rest[end + 1..];
            } else {
                return trimmed;
            }
        }
    }

    fn normalize_segment(seg: &str) -> String {
        let stripped = strip_leading_brackets(seg);
        let before_paren = stripped.split('(').next().unwrap_or(stripped).trim();
        let mut out = Vec::new();
        for tok in before_paren.split_whitespace() {
            if tok.starts_with('\'') && tok.ends_with('\'')
                || tok.starts_with('"') && tok.ends_with('"')
            {
                out.push("_");
            } else if tok.chars().any(|c| c.is_ascii_digit()) {
                out.push("#");
            } else {
                out.push(tok);
            }
        }
        out.join(" ")
    }

    let first_line = err.lines().next().unwrap_or(err).trim();
    let mut parts = first_line.split(": ");
    let first = parts.next().map(normalize_segment).unwrap_or_default();
    if let Some(second_raw) = parts.next() {
        let second = normalize_segment(second_raw);
        if !second.is_empty() {
            return format!("{first}: {second}");
        }
    }
    first
}

/// Generic deterministic shrinker (prefix binary search + bounded ddmin deletion).
/// Pins the original failure's error-class fingerprint so shrinking cannot drift onto a
/// different failure class.
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
    let target_fp = error_class_fingerprint(&best_err);
    let mut attempts = 0usize;

    // 1. Prefix binary search: find shortest failing prefix `best[..hi]` preserving failure class.
    if !best.is_empty() && attempts < max_attempts {
        let mut lo = 0usize;
        let mut hi = best.len();
        while lo < hi && attempts < max_attempts {
            let mid = lo + (hi - lo) / 2;
            attempts += 1;
            if let Some(err) = reproduces(&best[..mid])
                && error_class_fingerprint(&err) == target_fp
            {
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
            if let Some(err) = reproduces(&candidate)
                && error_class_fingerprint(&err) == target_fp
            {
                best = candidate;
                best_err = err;
                progressed = true;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_profiles_cycle_deterministically_and_preserve_shrink_profile() {
        let seen: BTreeSet<&str> = (0..6u64)
            .map(|s| ChaosProfile::from_seed(s).sub_profile.snake_name())
            .collect();
        assert_eq!(seen.len(), 6);
        for (idx, expected) in ChaosSubProfile::ALL.iter().enumerate() {
            let (prof, ops) = generate_ops(idx as u64, 15);
            assert_eq!(prof.sub_profile, *expected);
            assert_eq!(ops.len(), 15);
            // Replay of `generate_ops` yields identical profile and ops.
            let (prof2, ops2) = generate_ops(idx as u64, 15);
            assert_eq!(prof2.sub_profile, prof.sub_profile);
            assert_eq!(ops2, ops);
        }
    }

    #[test]
    fn delivery_and_storage_sub_profile_hits_real_delivery_batch_and_storage_faults() {
        crate::trace::reset_thread_coverage();
        // Seed 606 % 6 == 0 -> DeliveryAndStorage
        let d1 = run(606, 45).expect("DeliveryAndStorage seed 606 failed");
        let cov = crate::trace::take_thread_coverage();
        assert!(cov.count("chaos.profile.delivery_and_storage") > 0);
        assert!(
            cov.count("chaos.delivery_batch_entered") > 0,
            "expected chaos.delivery_batch_entered > 0, got:\n{}",
            cov.summary()
        );
        assert!(
            cov.count("exec.shell_executed") > 0,
            "expected exec.shell_executed > 0, got:\n{}",
            cov.summary()
        );
        assert!(
            cov.count("chaos.natural_quiesce_checked") > 0
                && cov.count("chaos.natural_quiesce_recovered") > 0
        );
        assert!(
            cov.vacuous_armed_faults().is_empty(),
            "vacuous armed faults detected: {:?}",
            cov.armed_vs_fired_report()
        );
        let d2 = run(606, 45).expect("DeliveryAndStorage seed 606 replay failed");
        assert_eq!(d1, d2, "DeliveryAndStorage must replay deterministically");
    }

    #[test]
    fn shrink_ops_pins_error_class_fingerprint_and_prevents_drift() {
        // Ops 0..10 fail with the target error iff op 3 and op 8 are present,
        // while any subset containing op 1 without op 3 fails with a DIFFERENT error class.
        let ops: Vec<usize> = (0..10).collect();
        let initial_err =
            "[primary][remote-a] RYW VIOLATION: Card c99 regressed to rev 2 < 5".to_owned();
        let (minimized, _attempts, final_err) = shrink_ops(ops, initial_err, 64, |cand| {
            if cand.contains(&3) && cand.contains(&8) {
                Some(format!(
                    "[primary][remote-b] RYW VIOLATION: Card c{} regressed to rev 1 < 3",
                    cand.len()
                ))
            } else if cand.contains(&1) {
                Some("[local-server] DURABILITY VIOLATION: committed Card c1 vanished!".into())
            } else {
                None
            }
        });
        assert_eq!(
            minimized,
            vec![3, 8],
            "shrink_ops must not drift onto the shorter unrelated DURABILITY VIOLATION on [1]"
        );
        assert!(
            final_err.contains("RYW VIOLATION"),
            "final error must retain pinned class, got: {final_err}"
        );
    }
}
