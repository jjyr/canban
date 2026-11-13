use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Stage {
    pub id: String,
    pub name: String,
    pub execute: bool,
    #[serde(default)]
    pub profile: Option<String>,
    pub instruction: String,
    pub on_completed: Option<String>,
    #[serde(default)]
    pub require_human_review: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Workflow {
    #[serde(default)]
    pub workspace: WorkspaceSettings,
    pub name: String,
    pub stages: Vec<Stage>,
}
/// Missing fields preserve existing local-only templates.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceSettings {
    #[serde(default = "yes")]
    pub create_worktree: bool,
    #[serde(default)]
    pub base_branch: Option<String>,
    #[serde(default)]
    pub sync_base: bool,
    #[serde(default = "origin")]
    pub remote: String,
}
fn yes() -> bool {
    true
}
fn origin() -> String {
    "origin".into()
}
impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            create_worktree: true,
            base_branch: None,
            sync_base: false,
            remote: origin(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HumanGate {
    pub reason: String,
    pub request_id: Option<String>,
    pub next_stage: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceInfo {
    pub path: Option<String>,
    pub branch: Option<String>,
    pub target_branch: Option<String>,
    pub base_commit: Option<String>,
    pub state: String,
    pub dirty: bool,
}
impl Default for Workflow {
    fn default() -> Self {
        Self {
            workspace: WorkspaceSettings { sync_base: true, ..Default::default() },
            name: "Development".into(),
            stages: vec![
                Stage {
                    id: "backlog".into(), name: "Backlog".into(), execute: false,
                    profile: None, instruction: String::new(), on_completed: None, require_human_review: false,
                },
                Stage {
                    id: "develop".into(), name: "Develop".into(), execute: true,
                    profile: Some("developer".into()),
                    instruction: "Implement the card task in the supplied working directory. Explain your changes and validation in your report.".into(),
                    on_completed: Some("review".into()), require_human_review: false,
                },
                Stage {
                    id: "review".into(), name: "Review".into(), execute: false,
                    profile: None,
                    instruction: String::new(),
                    on_completed: None, require_human_review: false,
                },
            ],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExitAction {
    #[default]
    Manual,
    Input {
        text: String,
    },
}
fn is_true(v: &bool) -> bool {
    *v
}
fn is_false(v: &bool) -> bool {
    !*v
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub identity: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default)]
    pub role_instruction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_argv: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<ExitAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_instruction: Option<String>,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: None,
            enabled: true,
            identity: false,
            preset: None,
            model: None,
            effort: None,
            role_instruction: String::new(),
            argv: None,
            resume_argv: None,
            exit: None,
            session_instruction: None,
        }
    }
}
impl Profile {
    pub fn is_identity(&self) -> bool {
        self.identity
            || !self.role_instruction.trim().is_empty()
            || matches!(self.id.as_str(), "planner" | "developer" | "reviewer")
    }
    pub fn command(&self) -> &[String] {
        self.argv.as_deref().unwrap_or_default()
    }
    pub fn resume_command(&self) -> &[String] {
        self.resume_argv.as_deref().unwrap_or_default()
    }
    /// A diagnostic/session namespace only; never selects runtime behavior.
    pub fn command_name(&self) -> String {
        self.command()
            .first()
            .and_then(|s| std::path::Path::new(s).file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("command")
            .to_owned()
    }
}
pub fn default_idle_minutes() -> u64 {
    15
}
pub fn default_max_agents() -> usize {
    8
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SchedulerSettings {
    /// Zero disables idle reclamation.
    #[serde(default = "default_idle_minutes")]
    pub idle_minutes: u64,
    #[serde(default = "default_max_agents")]
    pub max_agents: usize,
}
impl Default for SchedulerSettings {
    fn default() -> Self {
        Self {
            max_agents: default_max_agents(),
            idle_minutes: default_idle_minutes(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default = "local_machine")]
    pub default_machine: String,
    /// Client-only relay to AppServer; never used by its own Core adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_endpoint: Option<String>,
    #[serde(default)]
    pub scheduler: SchedulerSettings,
    pub hc: String,
    pub hc_session: String,
    #[serde(default)]
    pub hc_env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub onboarded: bool,
    pub default_agent: String,
    pub profiles: Vec<Profile>,
}
impl Default for Config {
    fn default() -> Self {
        let profile = |id: &str, preset: &str, identity: bool, role: &str| Profile {
            id: id.into(),
            identity,
            preset: Some(preset.into()),
            role_instruction: role.into(),
            ..Default::default()
        };
        Self {
            default_machine: "local".into(),
            relay_endpoint: None,
            scheduler: SchedulerSettings::default(),
            hc: "hc".into(),
            hc_session: "default".into(),
            hc_env: BTreeMap::new(),
            onboarded: false,
            default_agent: "developer".into(),
            profiles: vec![
                profile("codex", "codex", false, ""),
                profile("claude", "claude", false, ""),
                profile(
                    "developer",
                    "codex",
                    true,
                    "You are the Card's development Agent. Implement the agreed task, preserve relevant project conventions, and explain your changes and validation.",
                ),
                profile(
                    "reviewer",
                    "codex",
                    true,
                    "You are the Card's independent review Agent. Inspect the actual changes critically, identify correctness issues and missing validation, and report findings. Do not implement changes unless explicitly asked.",
                ),
                profile(
                    "planner",
                    "codex",
                    true,
                    "You are the Card's planning Agent. Analyze the task and relevant repository files in read-only mode, identify risks, and write a concise implementation plan without modifying source files.",
                ),
            ],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Project {
    #[serde(default)]
    pub github: Option<GitHubSource>,
    #[serde(default)]
    pub delivery_mode: DeliveryMode,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_overrides: BTreeMap<String, String>,
    pub id: String,
    pub name: String,
    pub directory: String,
    /// Current execution default. `directory` remains a compatibility mirror.
    #[serde(default)]
    pub location: Option<WorkLocation>,
    pub workflow: Workflow,
    #[serde(default)]
    pub target_branch: Option<String>,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkLocation {
    /// `local` means AppServer's Core; remote values are owning Core IDs.
    #[serde(default = "local_machine")]
    pub machine: String,
    pub directory: String,
}
pub fn local_machine() -> String {
    "local".into()
}
impl Project {
    pub fn work_location(&self) -> WorkLocation {
        self.location.clone().unwrap_or_else(|| WorkLocation {
            machine: local_machine(),
            directory: self.directory.clone(),
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceBinding {
    pub location: WorkLocation,
    pub path: String,
    pub branch: Option<String>,
    pub base_commit: Option<String>,
    pub target_branch: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Card {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_trust: Option<String>,
    #[serde(default)]
    pub hold_auto_merge: bool,
    #[serde(default)]
    pub github_links: Vec<GitHubReference>,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub content: CardContent,
    #[serde(default)]
    pub removed_at: Option<u64>,
    #[serde(default)]
    pub pull_request: Option<GitHubPr>,
    #[serde(default)]
    pub pull_request_history: Vec<GitHubPr>,
    #[serde(default)]
    pub delivery_mode: Option<DeliveryMode>,
    #[serde(default)]
    pub workspace: Option<WorkspaceBinding>,
    #[serde(default)]
    pub workspace_history: Vec<WorkspaceBinding>,
    pub id: String,
    pub project_id: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    /// Sparse presentation rank within the current stage; old Cards use number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_order: Option<i64>,
    #[serde(default)]
    pub context_notes: Option<Vec<String>>,
    pub stage: String,
    pub revision: u64,
    pub attention: Option<String>,
    pub archived_at: Option<u64>,
    pub archive: Option<String>,
    pub archive_error: Option<String>,
    pub worktree: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub base_commit: Option<String>,
    #[serde(default)]
    pub human_gate: Option<HumanGate>,
    #[serde(default)]
    pub delivery: Option<Delivery>,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Delivery {
    pub target_branch: String,
    pub commit: String,
    pub request_key: String,
    pub merged_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReviewPreview {
    pub card_id: String,
    pub revision: u64,
    pub target_branch: String,
    pub target_head: String,
    pub card_head: String,
    pub source_branch: String,
    pub worktree: String,
    pub files: Vec<String>,
    pub patch: String,
    pub truncated: bool,
    pub dirty: bool,
    pub conflicts: Vec<String>,
    pub token: String,
    pub merge_allowed: bool,
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: String,
    pub project_id: String,
    pub card_id: String,
    pub sequence: u64,
    pub kind: String,
    pub actor: String,
    pub message: String,
    pub request_id: Option<String>,
    pub data: Value,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Request {
    #[serde(default)]
    pub workflow_run_id: Option<String>,
    #[serde(default)]
    pub workflow_step: Option<String>,
    #[serde(default)]
    pub location: Option<WorkLocation>,
    #[serde(default)]
    pub resume_from: Option<String>,
    pub id: String,
    pub project_id: String,
    pub card_id: String,
    pub trigger: String,
    pub stage: String,
    pub profile: Profile,
    pub instruction: String,
    pub on_completed: Option<String>,
    #[serde(default)]
    pub require_human_review: bool,
    pub state: String,
    pub attempt: u32,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Attempt {
    #[serde(default)]
    pub runtime_home: Option<String>,
    #[serde(default)]
    pub workspace: Option<WorkspaceBinding>,
    #[serde(default)]
    pub location: Option<WorkLocation>,
    #[serde(default)]
    pub owner_core_id: Option<String>,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    pub id: String,
    pub request_id: String,
    pub project_id: String,
    pub card_id: String,
    pub number: u32,
    pub state: String,
    pub core_id: Option<String>,
    pub terminal: Option<String>,
    pub cwd: String,
    pub input_path: String,
    pub output_path: Option<String>,
    pub observed: Option<Value>,
    pub error: Option<String>,
    pub created_at: u64,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub provider_session_id: Option<String>,
    #[serde(default)]
    pub close_state: String,
    #[serde(default)]
    pub turn_ended_at: Option<u64>,
    #[serde(default)]
    pub response_path: Option<String>,
    #[serde(default)]
    pub completed_at: Option<u64>,
    #[serde(default)]
    pub completion_turn: Option<u64>,
    #[serde(default)]
    pub close_requested_at: Option<u64>,
    #[serde(default)]
    pub connection_state: String,
}
/// Durable policy facts; runtime input waits never become workflow approvals.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Lifecycle {
    pub observed_at: Option<u64>,
    pub idle_since: Option<u64>,
    pub last_interaction_at: Option<u64>,
    pub waiting: Option<RuntimeWait>,
    #[serde(default)]
    pub close_reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RuntimeWait {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub message: String,
    pub since: u64,
}
impl Attempt {
    pub fn finished(&self) -> bool {
        self.state == "continued"
            || self.connection_state == "missing"
            || self.terminal.is_none() && self.state == "failed"
            || self
                .observed
                .as_ref()
                .is_some_and(|v| v["state"] == "finished")
    }
    /// True when the terminal exited or Herdr reports the Agent has stopped its turn.
    pub fn agent_stopped(&self) -> bool {
        if self.finished() {
            return true;
        }
        if self.connection_state != "connected" {
            return false;
        }
        match self
            .observed
            .as_ref()
            .and_then(|v| v["status"]["state"].as_str())
        {
            Some("idle" | "done") => true,
            Some(_) => false,
            None => self.state == "completed",
        }
    }
    /// A stale or disconnected status must not release scheduling capacity.
    pub fn runtime_status(&self) -> &str {
        if self.finished() {
            return "closed";
        }
        if self.connection_state != "connected"
            || self
                .lifecycle
                .observed_at
                .is_none_or(|t| now().saturating_sub(t) > 30_000)
        {
            return "unknown";
        }
        self.observed
            .as_ref()
            .and_then(|v| v["status"]["state"].as_str())
            .unwrap_or("unknown")
    }
    pub fn key(request: &str, number: u32) -> String {
        format!("{request}:{number}")
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Conversation {
    pub id: String,
    pub project_id: String,
    pub card_id: String,
    pub profile_id: String,
    pub provider: String,
    pub provider_session_id: Option<String>,
    pub last_request_id: Option<String>,
    pub created_at: u64,
}
impl Conversation {
    pub fn key(card: &str, profile: &str) -> String {
        format!("{card}:{profile}")
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Question {
    pub id: String,
    pub project_id: String,
    pub card_id: String,
    pub profile_id: String,
    pub comment_id: String,
    pub body: String,
    pub request_id: String,
    pub attempt: u32,
    pub state: String,
    pub error: Option<String>,
    pub reply_path: Option<String>,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Report {
    pub id: String,
    pub request_id: String,
    pub project_id: String,
    pub card_id: String,
    pub attempt: u32,
    pub state: String,
    pub message: String,
    pub path: Option<String>,
    pub request_key: String,
    pub fingerprint: String,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Note {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub body: String,
    pub revision: u64,
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub card_ids: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NoteRevision {
    pub id: String,
    pub project_id: String,
    pub note_id: String,
    pub revision: u64,
    pub title: String,
    pub body: String,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NoteEvent {
    pub id: String,
    pub project_id: String,
    pub note_id: String,
    pub sequence: u64,
    pub kind: String,
    pub actor: String,
    pub message: String,
    pub card_id: Option<String>,
    pub request_id: Option<String>,
    pub attempt: Option<u32>,
    pub revision: u64,
    pub data: Value,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Notice {
    pub id: String,
    pub project_id: String,
    pub card_id: String,
    pub message: String,
    pub read: bool,
    pub created_at: u64,
}
/// A user's read-through position, independent of Card revisions and gates.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CardRead {
    pub id: String,
    pub project_id: String,
    pub card_id: String,
    pub sequence: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    pub id: String,
    pub project_id: String,
    pub card_id: String,
    pub name: String,
    pub media_type: String,
    pub path: String,
    pub digest: String,
    pub bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct Snapshot {
    #[serde(default)]
    pub workflow_runs: Vec<WorkflowRun>,
    #[serde(default)]
    pub workflow_sources: Vec<WorkflowSource>,
    #[serde(default)]
    pub event_subscriptions: Vec<EventSubscription>,
    #[serde(default)]
    pub configuration_sessions: Vec<ConfigurationSession>,
    #[serde(default)]
    pub workflow_revisions: Vec<WorkflowRevision>,
    #[serde(default)]
    pub card_reads: Vec<CardRead>,
    #[serde(default)]
    pub github: GitHubAccount,
    #[serde(default)]
    pub github_issues: Vec<GitHubIssueSnapshot>,
    #[serde(default)]
    pub terminals: Vec<TerminalEntry>,
    /// Ephemeral Herdr observations, never persisted as Terminal ownership.
    #[serde(default)]
    pub terminal_contexts: Vec<TerminalContext>,
    pub projects: Vec<Project>,
    pub cards: Vec<Card>,
    pub events: Vec<Event>,
    pub requests: Vec<Request>,
    pub attempts: Vec<Attempt>,
    pub reports: Vec<Report>,
    pub notes: Vec<Note>,
    pub notices: Vec<Notice>,
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub conversations: Vec<Conversation>,
    #[serde(default)]
    pub questions: Vec<Question>,
    #[serde(default)]
    pub note_events: Vec<NoteEvent>,
}
/// Durable navigation identity. Agent runtime bindings remain on Attempts.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerminalEntry {
    pub id: String,
    pub conversation_id: Option<String>,
    pub number: u64,
    pub name: Option<String>,
    /// Separate durable sequence for independently opened Terminal labels.
    pub manual_number: Option<u64>,
    /// Optional Card association when opened directly from a Card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_card_id: Option<String>,
    /// Optional interactive CLI engine identifier for ephemeral Agent terminals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_engine: Option<String>,
    /// Independent terminals own a binding; Agent bindings remain on Attempts.
    pub runtime: Option<TerminalRuntime>,
    pub created_at: u64,
}
impl TerminalEntry {
    pub fn title(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            if self.conversation_id.is_none() {
                let n = self.manual_number.unwrap_or(self.number);
                if let Some(engine) = &self.agent_engine {
                    format!("{} {n}", friendly_agent_engine(engine))
                } else {
                    format!("Terminal {n}")
                }
            } else {
                self.number.to_string()
            }
        })
    }
    /// Decode v4 records/receipts without rewriting historical operation data.
    pub fn from_json(mut value: Value) -> serde_json::Result<Self> {
        if let Some(mut source) = value.as_object_mut().and_then(|o| o.remove("source")) {
            let agent = source["kind"] == "agent";
            value["conversation_id"] = if agent {
                value["id"].clone()
            } else {
                Value::Null
            };
            if agent {
                value["runtime"] = Value::Null;
            } else {
                if let Some(fields) = source.as_object_mut() {
                    fields.remove("kind");
                    fields.remove("cwd");
                }
                value["runtime"] = source;
            }
        }
        serde_json::from_value(value)
    }
}
fn friendly_agent_engine(engine: &str) -> String {
    let clean = engine.strip_prefix("herdr:").unwrap_or(engine);
    match clean {
        "codex" => "Codex".into(),
        "claude" => "Claude Code".into(),
        "agy" | "antigravity" => "Antigravity".into(),
        "gemini" => "Gemini CLI".into(),
        "opencode" => "OpenCode".into(),
        "cursor" | "cursor-agent" => "Cursor Agent".into(),
        "pi" => "Pi".into(),
        "omp" => "Omp".into(),
        "copilot" => "GitHub Copilot".into(),
        "droid" => "Droid".into(),
        "amp" => "Amp".into(),
        "kimi" => "Kimi".into(),
        "kiro" | "kiro-cli" => "Kiro".into(),
        "qwen" => "Qwen Code".into(),
        "devin" => "Devin".into(),
        "cline" => "Cline".into(),
        "grok" => "Grok".into(),
        "hermes" => "Hermes".into(),
        "kilo" => "Kilo".into(),
        "qodercli" | "qoder" => "Qoder".into(),
        "letta" => "Letta".into(),
        "mastracode" | "mastra" => "Mastra Code".into(),
        "maki" => "Maki".into(),
        "muse" => "Muse".into(),
        other => other.into(),
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerminalRuntime {
    #[serde(default = "local_machine")]
    pub machine: String,
    #[serde(default)]
    pub owner_core_id: Option<String>,
    pub core_id: Option<String>,
    pub terminal: Option<String>,
    pub state: String,
    pub connection: String,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerminalContext {
    #[serde(default = "local_machine")]
    pub machine: String,
    pub terminal_id: String,
    pub core_id: String,
    pub terminal: String,
    pub cwd: Option<String>,
    pub project_id: Option<String>,
    pub card_id: Option<String>,
    pub branch: Option<String>,
}
#[derive(Clone, Debug)]
pub enum AppEvent {
    Changed {
        topic: &'static str,
        card_id: Option<String>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Command {
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    #[default]
    Local,
    #[serde(rename = "github_pr")]
    GitHubPr,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GitHubIntake {
    #[default]
    All,
    Collaborators,
    Off,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GitHubSource {
    #[serde(default)]
    pub pr_intake: GitHubPrIntake,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub include_drafts: bool,
    #[serde(default)]
    pub paused: bool,

    pub repository: String,
    pub repository_id: u64,
    pub generation: String,
    pub intake: GitHubIntake,
    pub synced_at: Option<u64>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GitHubPrIntake {
    #[default]
    Off,
    All,
    Collaborators,
    ReviewRequested,
    Assigned,
    Authored,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubReference {
    pub repository: String,
    pub kind: String,
    pub number: Option<u64>,
    pub sha: Option<String>,
    pub url: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GitHubIssueRef {
    pub repository: String,
    pub repository_id: u64,
    pub issue_id: u64,
    pub number: u64,
    pub url: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CardContent {
    #[default]
    Local,
    #[serde(rename = "github_issue")]
    GitHubIssue { source: GitHubIssueRef },
    #[serde(rename = "github_pr")]
    GitHubPr { source: GitHubIssueRef },
}
impl Card {
    pub fn board_rank(&self) -> i64 {
        self.board_order.unwrap_or_else(|| {
            i64::try_from(self.number)
                .unwrap_or(i64::MAX / 1024)
                .saturating_mul(1024)
        })
    }
    pub fn board_cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.board_rank()
            .cmp(&other.board_rank())
            .then_with(|| self.number.cmp(&other.number))
            .then_with(|| self.id.cmp(&other.id))
    }
    pub fn github_issue(&self) -> Option<&GitHubIssueRef> {
        match &self.content {
            CardContent::GitHubIssue { source } | CardContent::GitHubPr { source } => Some(source),
            CardContent::Local => None,
        }
    }
    pub fn is_github_pr(&self) -> bool {
        matches!(self.content, CardContent::GitHubPr { .. })
    }
    pub fn source_kind(&self) -> &'static str {
        if self.is_github_pr() { "pr" } else { "issue" }
    }
    pub fn trust_level(&self) -> &str {
        if let Some(trust) = self.source_trust.as_deref() {
            let trimmed = trust.trim();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        match &self.content {
            CardContent::Local => "local",
            CardContent::GitHubIssue { .. } => "collaborator",
            CardContent::GitHubPr { .. } => "external_pr",
        }
    }
    pub fn active(&self) -> bool {
        self.removed_at.is_none() && self.archived_at.is_none()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GitHubIssueSnapshot {
    #[serde(default)]
    pub assignees: Vec<String>,
    pub id: String,
    pub project_id: String,
    pub source: GitHubIssueRef,
    pub title: String,
    pub body: String,
    pub state: String,
    pub author_id: u64,
    pub author_login: String,
    pub labels: Vec<String>,
    pub updated_at: String,
    pub synced_at: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct GitHubAccount {
    #[serde(default)]
    pub installation_url: Option<String>,
    pub state: String,
    pub login: Option<String>,
    pub user_code: Option<String>,
    pub verification_uri: Option<String>,
    pub expires_at: Option<u64>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GitHubPr {
    #[serde(default)]
    pub review_summary: String,
    pub repository: String,
    pub repository_id: u64,
    pub number: u64,
    pub url: String,
    pub state: String,
    pub head: String,
    pub head_branch: String,
    pub base_branch: String,
    pub merged: bool,
    pub checks: String,
    pub workspace: WorkspaceBinding,
}

mod workflow;
pub use workflow::*;
