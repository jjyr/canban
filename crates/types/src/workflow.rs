//! Shared declarative workflow and durable subscription contracts.
use crate::{Event, WorkspaceSettings};
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
    #[serde(default)]
    pub workspace: WorkspaceSettings,
    #[serde(default = "default_limit")]
    pub max_steps: u32,
}
fn default_limit() -> u32 {
    100
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowStep {
    Agent {
        profile: Option<String>,
        instruction: String,
        next: Option<String>,
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
        target_branch: Option<String>,
        #[serde(default = "default_sync_upstream", skip_serializing_if = "is_true")]
        sync_upstream: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rework: Option<String>,
        /// Automatic merge-conflict hand-offs to `rework` per run before pausing for a human.
        /// `0` disables automatic conflict rework.
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
    pub request_id: Option<String>,
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

fn default_sync_upstream() -> bool {
    true
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
