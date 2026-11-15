//! Shared declarative workflow and durable subscription contracts.
use crate::{Card, Event, Snapshot, WorkspaceSettings};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowColumn {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDefinition {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<WorkflowColumn>,
    pub entries: BTreeMap<String, String>,
    pub steps: BTreeMap<String, WorkflowStep>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub workspace: WorkspaceSettings,
    #[serde(default = "default_limit")]
    pub max_steps: u32,
}
fn default_limit() -> u32 {
    100
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    #[default]
    Rebase,
    Squash,
    Merge,
}

impl MergeStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rebase => "rebase",
            Self::Squash => "squash",
            Self::Merge => "merge",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Rebase => "Rebase",
            Self::Squash => "Squash",
            Self::Merge => "Merge commit",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "rebase" => Some(Self::Rebase),
            "squash" => Some(Self::Squash),
            "merge" => Some(Self::Merge),
            _ => None,
        }
    }
}

pub fn is_default_merge_strategy(v: &MergeStrategy) -> bool {
    *v == MergeStrategy::Rebase
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowStep {
    Agent {
        profile: Option<String>,
        instruction: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
        next: Option<String>,
    },
    Script {
        run: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_directory: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_minutes: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
        #[serde(default, skip_serializing_if = "is_false")]
        continue_on_error: bool,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_failure: Option<String>,
    },
    Move {
        column: String,
        next: Option<String>,
    },
    Approval {
        title: String,
        next: Option<String>,
        rework: Option<String>,
    },
    Deliver {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_branch: Option<String>,
        #[serde(default, skip_serializing_if = "is_default_merge_strategy")]
        strategy: MergeStrategy,
        #[serde(default = "default_sync_upstream", skip_serializing_if = "is_true")]
        sync_upstream: bool,
        #[serde(
            default = "default_merge_batch_size",
            skip_serializing_if = "is_default_merge_batch_size"
        )]
        batch_size: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        test_command: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rework: Option<String>,
        /// Automatic merge-conflict hand-offs to `@merger` (or `profile`) per run before
        /// pausing for a human. `0` disables automatic conflict resolution.
        #[serde(
            default = "default_max_conflict_attempts",
            skip_serializing_if = "is_default_max_conflict_attempts"
        )]
        max_conflict_attempts: u32,
    },
    Condition {
        pointer: String,
        equals: Value,
        then: String,
        otherwise: String,
    },
    Notify {
        message: String,
        next: Option<String>,
    },
    /// Capacity gate: the run waits (`waiting_capacity`) until the scheduler admits it.
    /// Admission follows Board order within the Card's column.
    Admit {
        /// Wait for a free global automatic Agent slot (`scheduler.max_agents`).
        #[serde(default = "default_sync_upstream", skip_serializing_if = "is_true")]
        agent_slot: bool,
        /// At most this many Cards that passed this gate may remain in the gate's column
        /// (for example: keep at most K planned Cards waiting in Backlog).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_ready: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<String>,
    },
    Complete,
}
impl WorkflowStep {
    pub fn next(&self) -> Option<&str> {
        match self {
            Self::Agent { next, .. }
            | Self::Script { next, .. }
            | Self::Move { next, .. }
            | Self::Approval { next, .. }
            | Self::Deliver { next, .. }
            | Self::Admit { next, .. }
            | Self::Notify { next, .. } => next.as_deref(),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkflowRevision {
    #[serde(default)]
    pub source_yaml: Option<String>,
    pub id: String,
    pub project_id: String,
    pub workflow_id: String,
    pub revision: u64,
    pub definition: WorkflowDefinition,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkflowRun {
    pub id: String,
    pub project_id: String,
    pub card_id: String,
    pub workflow_id: String,
    pub definition_revision: u64,
    pub definition: WorkflowDefinition,
    pub state: String,
    pub step: Option<String>,
    pub visit: u32,
    #[serde(default)]
    pub terminal_id: Option<String>,
    pub trigger_event_id: Option<String>,
    pub input_digest: String,
    pub inputs: Value,
    pub outputs: BTreeMap<String, Value>,
    pub history: Vec<Value>,
    pub error: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}
impl WorkflowRun {
    pub fn active(&self) -> bool {
        !matches!(
            self.state.as_str(),
            "completed" | "cancelled" | "superseded"
        )
    }

    /// True when the run's current step is a `Script` step.
    pub fn step_is_script(&self) -> bool {
        self.step.as_ref().is_some_and(|k| {
            matches!(
                self.definition.steps.get(k),
                Some(WorkflowStep::Script { .. })
            )
        })
    }

    /// Resolves a `workflow_step` reference (possibly suffixed with `:{visit}`) to a step ID.
    pub fn resolve_step_id<'a>(&'a self, step_ref: Option<&'a str>) -> Option<&'a str> {
        let Some(raw) = step_ref else {
            return self.step.as_deref();
        };
        if self.definition.steps.contains_key(raw) {
            return Some(raw);
        }
        if let Some((base, visit)) = raw.rsplit_once(':')
            && !visit.is_empty()
            && visit.chars().all(|c| c.is_ascii_digit())
            && self.definition.steps.contains_key(base)
        {
            return Some(base);
        }
        self.step.as_deref()
    }

    /// Custom environment layers for `step_ref`: workflow `env` < `$CANBAN_ENV` < step `env`.
    pub fn custom_env_for_step(&self, step_ref: Option<&str>) -> BTreeMap<String, String> {
        let mut env = self.definition.env.clone();
        if let Some(obj) = self.inputs["env"].as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    env.insert(k.clone(), s.to_owned());
                }
            }
        }
        if let Some(step_id) = self.resolve_step_id(step_ref) {
            match self.definition.steps.get(step_id) {
                Some(WorkflowStep::Agent { env: step_env, .. })
                | Some(WorkflowStep::Script { env: step_env, .. }) => {
                    for (k, v) in step_env {
                        env.insert(k.clone(), v.clone());
                    }
                }
                _ => {}
            }
        }
        env
    }

    /// True when the run's current step is a `Deliver` (Queue Merge) step.
    pub fn step_is_deliver(&self) -> bool {
        self.step.as_ref().is_some_and(|k| {
            matches!(
                self.definition.steps.get(k),
                Some(WorkflowStep::Deliver { .. })
            )
        })
    }

    /// Monotonic queue ticket timestamp recorded when the run entered `Deliver`.
    pub fn merge_queued_at(&self) -> u64 {
        self.inputs["queued_at"].as_u64().unwrap_or(self.created_at)
    }

    /// Transient main-checkout wait reason while holding the head of a Project's merge queue.
    pub fn queue_pause_reason(&self) -> Option<&str> {
        self.inputs["queue_pause_reason"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    /// True when this run occupies a slot in its Project's active FIFO merge queue:
    /// either waiting to merge (`waiting_delivery`) or actively resolving a merge conflict
    /// at the head of the queue (`waiting_agent` / `running` at `Deliver`).
    pub fn in_merge_queue(&self, card: &Card) -> bool {
        card.active()
            && !card.merged()
            && !card.hold_auto_merge
            && self.step_is_deliver()
            && matches!(
                self.state.as_str(),
                "waiting_delivery" | "waiting_batch" | "waiting_agent" | "running"
            )
    }
}

/// Orders active merge-queue runs for `project_id` in strict FIFO order by
/// `(queued_at, storage_index)`.
pub fn project_merge_queue<'a>(
    workflow_runs: &'a [WorkflowRun],
    cards: &[Card],
    project_id: &str,
) -> Vec<&'a WorkflowRun> {
    let cards_by_id: BTreeMap<&str, &Card> = cards.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut entries: Vec<(usize, &'a WorkflowRun)> = workflow_runs
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            r.project_id == project_id
                && cards_by_id
                    .get(r.card_id.as_str())
                    .is_some_and(|c| r.in_merge_queue(c))
        })
        .collect();
    entries.sort_by_key(|(idx, r)| (r.merge_queued_at(), *idx));
    entries.into_iter().map(|(_, r)| r).collect()
}

impl Snapshot {
    /// Active FIFO merge queue for `project_id`.
    pub fn merge_queue(&self, project_id: &str) -> Vec<&WorkflowRun> {
        project_merge_queue(&self.workflow_runs, &self.cards, project_id)
    }

    /// Returns `(1_based_position, total_in_queue, head_run)` when `card_id` is in its Project's active merge queue.
    pub fn merge_queue_status(&self, card_id: &str) -> Option<(usize, usize, &WorkflowRun)> {
        let card = self.cards.iter().find(|c| c.id == card_id)?;
        let queue = self.merge_queue(&card.project_id);
        let head = *queue.first()?;
        let pos = queue.iter().position(|r| r.card_id == card_id)?;
        Some((pos + 1, queue.len(), head))
    }

    /// Returns the currently active [`MergeBatch`] for `project_id`, if any.
    pub fn active_merge_batch_for_project(&self, project_id: &str) -> Option<&MergeBatch> {
        self.merge_batches
            .iter()
            .find(|b| b.project_id == project_id && b.active())
    }

    /// Returns `(batch, 1_based_non_evicted_index, total_non_evicted)` when `card_id` is actively
    /// participating in an active [`MergeBatch`].
    pub fn active_merge_batch_for_card(
        &self,
        card_id: &str,
    ) -> Option<(&MergeBatch, usize, usize)> {
        for batch in self.merge_batches.iter().filter(|b| b.active()) {
            let live_items: Vec<&MergeBatchItem> =
                batch.items.iter().filter(|item| !item.evicted).collect();
            if let Some(pos) = live_items.iter().position(|item| item.card_id == card_id) {
                return Some((batch, pos + 1, live_items.len()));
            }
        }
        None
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EventSelector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub include_related: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub include_initial: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubscriptionTarget {
    Consumer,
    Workflow {
        workflow_id: String,
        entry: String,
        card_id: Option<String>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EventSubscription {
    pub id: String,
    pub revision: u64,
    pub selector: EventSelector,
    pub target: SubscriptionTarget,
    pub enabled: bool,
    pub cursor: u64,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EventDelivery {
    #[serde(default)]
    pub definition_revision: Option<u64>,
    pub id: String,
    pub subscription_id: String,
    pub subscription_revision: u64,
    pub event_id: String,
    pub offset: u64,
    pub selector: EventSelector,
    pub target: SubscriptionTarget,
    pub state: String,
    pub attempts: u32,
    pub next_attempt_at: u64,
    pub error: Option<String>,
    pub workflow_run_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct JournalEvent {
    pub offset: u64,
    #[serde(flatten)]
    pub event: Event,
}
/// Active SQL source reference. Original YAML lives on immutable revisions.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkflowSource {
    pub id: String,
    pub project_id: String,
    pub filename: String,
    pub workflow_id: String,
    pub revision: u64,
    pub hash: String,
    pub revision_id: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<WorkflowColumn>,
    pub rules: BTreeMap<String, WorkflowRule>,
}
/// One YAML file saved inside a reusable `WorkflowTemplate` collection.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowTemplateFile {
    pub filename: String,
    pub source: String,
}
/// Global reusable workflow source collection (for example `id = "default"` for new Projects).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowTemplate {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_project_name: Option<String>,
    pub sources: Vec<WorkflowTemplateFile>,
    pub updated_at: u64,
}
pub const DEFAULT_WORKFLOW_TEMPLATE_ID: &str = "default";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRule {
    pub event: String,
    pub start_at: String,
    #[serde(default, rename = "match")]
    pub selector: EventSelector,
    #[serde(default = "enabled_rule")]
    pub enabled: bool,
}
fn enabled_rule() -> bool {
    true
}
/// A Project configuration conversation has no business Card or assignment.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConfigurationSession {
    pub id: String,
    pub project_id: String,
    pub profile_id: String,
    pub provider_session_id: Option<String>,
    pub generation: u64,
    pub state: String,
    pub prompt: String,
    pub error: Option<String>,
}

/// One Card participating in an in-flight or historical merge batch.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MergeBatchItem {
    pub card_id: String,
    pub card_number: u64,
    pub workflow_run_id: String,
    pub branch: String,
    pub worktree: String,
    pub base_head: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrated_commit: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub evicted: bool,
}

/// Persistent record for a batch merge-queue execution (including any `@merger` resolution session).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MergeBatch {
    pub id: String,
    pub project_id: String,
    pub profile_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    pub target_branch: String,
    pub target_head: String,
    #[serde(default)]
    pub strategy: MergeStrategy,
    #[serde(default = "default_sync_upstream")]
    pub sync_upstream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_command: Option<String>,
    pub worktree: String,
    /// One of: "integrating", "testing", "queued", "running", "completed", "blocked", "failed", "cancelled".
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_card_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_output: Option<String>,
    #[serde(default)]
    pub items: Vec<MergeBatchItem>,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl MergeBatch {
    pub fn active(&self) -> bool {
        matches!(
            self.state.as_str(),
            "integrating" | "testing" | "queued" | "running"
        )
    }

    pub fn short_id(&self) -> &str {
        self.id.rsplit(':').next().unwrap_or(&self.id)
    }
}

fn default_sync_upstream() -> bool {
    true
}

/// Default maximum Cards combined into one batch on a `deliver` step.
pub const DEFAULT_MERGE_BATCH_SIZE: u32 = 5;
fn default_merge_batch_size() -> u32 {
    DEFAULT_MERGE_BATCH_SIZE
}
fn is_default_merge_batch_size(v: &u32) -> bool {
    *v == DEFAULT_MERGE_BATCH_SIZE
}

/// Default automatic merge-conflict rework attempts for a `deliver` step.
pub const DEFAULT_MAX_CONFLICT_ATTEMPTS: u32 = 3;
fn default_max_conflict_attempts() -> u32 {
    DEFAULT_MAX_CONFLICT_ATTEMPTS
}
fn is_default_max_conflict_attempts(v: &u32) -> bool {
    *v == DEFAULT_MAX_CONFLICT_ATTEMPTS
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_false(v: &bool) -> bool {
    !*v
}
