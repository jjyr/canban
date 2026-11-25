mod activity;
mod board_filter;
mod clock;
mod env;
mod failpoints;
mod id_gen;
pub use activity::*;
pub use board_filter::BoardFilter;
pub use clock::*;
pub use env::*;
pub use failpoints::*;
pub use id_gen::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_owned()
}
pub fn token_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
pub fn uuid_v4() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}
/// Validate a client-supplied entity identifier (`1..=64` chars of `[A-Za-z0-9_-]`).
pub fn is_valid_client_id(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 64
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
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
    #[serde(default)]
    pub terminal_id: Option<String>,
    pub next_stage: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceInfo {
    pub path: Option<String>,
    pub branch: Option<String>,
    pub target_branch: Option<String>,
    #[serde(default)]
    pub upstream: Option<String>,
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
                    instruction: "Evaluate the Card against the current codebase before editing:\n- Actionable: implement and validate the changes on the Card branch, commit deliverables, and report `--state completed`.\n- Already implemented, obsolete, or not applicable: make no unnecessary edits and report `--state backlog` explaining why.\n- Ambiguous or awaiting a human decision: report `--state blocked` with specific questions.".into(),
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
pub const ORCHESTRATOR_MODEL_ID: &str = "orchestrator";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrchestratorConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_effort: Option<String>,
}

fn clean_option_str(val: &Option<String>) -> Option<String> {
    val.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "inherit")
        .map(str::to_owned)
}

impl OrchestratorConfig {
    pub fn normalized(&self) -> Self {
        Self {
            planner_model: clean_option_str(&self.planner_model),
            planner_effort: clean_option_str(&self.planner_effort),
            worker_model: clean_option_str(&self.worker_model),
            worker_effort: clean_option_str(&self.worker_effort),
        }
    }

    pub fn is_empty(&self) -> bool {
        let n = self.normalized();
        n.planner_model.is_none()
            && n.planner_effort.is_none()
            && n.worker_model.is_none()
            && n.worker_effort.is_none()
    }

    pub fn into_non_empty(self) -> Option<Self> {
        let n = self.normalized();
        (!n.is_empty()).then_some(n)
    }

    /// Merge `self` (child override) over `base` (inherited parent).
    /// When `self` explicitly changes a role's model away from `base`'s model, do not
    /// inherit `base`'s effort for that role because effort tiers may differ across models.
    pub fn merge_fallback(&self, base: &Self) -> Self {
        let child = self.normalized();
        let base = base.normalized();
        let planner_changed = child.planner_model.is_some()
            && base.planner_model.is_some()
            && child.planner_model != base.planner_model;
        let worker_changed = child.worker_model.is_some()
            && base.worker_model.is_some()
            && child.worker_model != base.worker_model;
        Self {
            planner_model: child.planner_model.or(base.planner_model),
            planner_effort: if planner_changed {
                child.planner_effort
            } else {
                child.planner_effort.or(base.planner_effort)
            },
            worker_model: child.worker_model.or(base.worker_model),
            worker_effort: if worker_changed {
                child.worker_effort
            } else {
                child.worker_effort.or(base.worker_effort)
            },
        }
    }

    pub fn format_summary(orch: Option<&Self>) -> String {
        fn short_name(id: &str) -> &str {
            match id {
                "gemini-4-neon" => "Neon",
                "gemini-4-carbon" => "Carbon",
                "gemini-4-argon" => "Argon",
                "gemini-4-hydrogen" => "Hydrogen",
                "gemini-3.8-flash" => "Flash",
                "claude-opus-5-5" => "Opus 5.5",
                "claude-sonnet-5-5" => "Sonnet 5.5",
                other => other,
            }
        }
        fn fmt_role(model: Option<&str>, effort: Option<&str>) -> Option<String> {
            match (model, effort) {
                (Some(m), Some(e)) => Some(format!("{} ({e})", short_name(m))),
                (Some(m), None) => Some(short_name(m).to_owned()),
                (None, Some(e)) => Some(format!("Default ({e})")),
                (None, None) => None,
            }
        }
        let Some(n) = orch.map(Self::normalized) else {
            return "Orchestrator".to_owned();
        };
        let planner = fmt_role(n.planner_model.as_deref(), n.planner_effort.as_deref());
        let worker = fmt_role(n.worker_model.as_deref(), n.worker_effort.as_deref());
        match (planner, worker) {
            (Some(p), Some(w)) => format!("Orchestrator · {p} + {w}"),
            (Some(p), None) => format!("Orchestrator · {p}"),
            (None, Some(w)) => format!("Orchestrator · Default + {w}"),
            (None, None) => "Orchestrator".to_owned(),
        }
    }
}

pub fn legacy_agy_orchestrator_preset(model_id: &str) -> Option<OrchestratorConfig> {
    match model_id.trim().to_ascii_lowercase().as_str() {
        "gemini-pro-orchestrator-neon" => Some(OrchestratorConfig {
            planner_model: Some("gemini-4-neon".into()),
            planner_effort: Some("xhigh".into()),
            worker_model: Some("gemini-4-hydrogen".into()),
            worker_effort: Some("xhigh".into()),
        }),
        "gemini-pro-orchestrator" => Some(OrchestratorConfig {
            planner_model: Some("gemini-4-carbon".into()),
            planner_effort: Some("high".into()),
            worker_model: Some("gemini-4-hydrogen".into()),
            worker_effort: Some("high".into()),
        }),
        "gemini-pro-orchestrator-argon" => Some(OrchestratorConfig {
            planner_model: Some("gemini-4-argon".into()),
            planner_effort: Some("high".into()),
            worker_model: Some("gemini-4-hydrogen".into()),
            worker_effort: Some("high".into()),
        }),
        _ => None,
    }
}

pub fn is_orchestrator_model_id(model_id: &str) -> bool {
    let trimmed = model_id.trim();
    trimmed.eq_ignore_ascii_case(ORCHESTRATOR_MODEL_ID)
        || legacy_agy_orchestrator_preset(trimmed).is_some()
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator: Option<OrchestratorConfig>,
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
            orchestrator: None,
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
            || matches!(
                self.id.as_str(),
                "planner" | "developer" | "reviewer" | "merger"
            )
    }
    pub fn is_agy(&self) -> bool {
        if let Some(preset) = self.preset.as_deref() {
            return preset == "agy";
        }
        (!self.is_identity() && self.id == "agy") || self.command_name() == "agy"
    }
    pub fn is_orchestrator(&self) -> bool {
        self.is_agy() && self.model.as_deref().is_some_and(is_orchestrator_model_id)
    }
    pub fn effective_orchestrator(&self) -> Option<OrchestratorConfig> {
        if !self.is_orchestrator() {
            return None;
        }
        let explicit = self
            .orchestrator
            .as_ref()
            .map(OrchestratorConfig::normalized);
        let legacy = self
            .model
            .as_deref()
            .and_then(legacy_agy_orchestrator_preset);
        let resolved = match (explicit, legacy) {
            (Some(exp), Some(leg)) => exp.merge_fallback(&leg),
            (Some(exp), None) => exp,
            (None, Some(leg)) => leg,
            (None, None) => OrchestratorConfig::default(),
        };
        Some(resolved)
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
    /// Canonical provider name from preset, falling back to the launch executable basename.
    pub fn provider_name(&self) -> String {
        self.preset
            .clone()
            .filter(|p| !p.is_empty())
            .or_else(|| {
                self.command()
                    .first()
                    .and_then(|cmd| std::path::Path::new(cmd).file_name())
                    .map(|cmd| cmd.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Unknown".into())
    }
    /// Configured model name, defaulting to `"default"` when unset or blank.
    pub fn effective_model(&self) -> String {
        if self.is_orchestrator() {
            return OrchestratorConfig::format_summary(self.effective_orchestrator().as_ref());
        }
        self.model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .unwrap_or("default")
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

/// How unattended Agent and Script steps inherit the host user's login-shell environment.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostTerminalEnvMode {
    /// Inherit the host user's login-shell environment snapshot (default for trusted runs).
    #[default]
    Inherit,
    /// Only inject a minimal `PATH`-based environment (plus explicit `hc_env` / workflow `env`).
    Minimal,
}

impl HostTerminalEnvMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Minimal => "minimal",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Inherit => "Inherit user terminal environment",
            Self::Minimal => "Minimal (PATH only)",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "inherit" => Some(Self::Inherit),
            "minimal" => Some(Self::Minimal),
            _ => None,
        }
    }

    pub fn is_inherit(self) -> bool {
        self == Self::Inherit
    }
}

pub fn is_default_host_terminal_env_mode(v: &HostTerminalEnvMode) -> bool {
    *v == HostTerminalEnvMode::Inherit
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Config {
    /// Client-only relay to AppServer; never used by its own Core adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_endpoint: Option<String>,
    /// Client-only Bearer Token for authenticated TCP AppServer relays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_token: Option<String>,
    #[serde(default)]
    pub scheduler: SchedulerSettings,
    pub hc: String,
    pub hc_session: String,
    #[serde(default)]
    pub hc_env: BTreeMap<String, String>,
    /// Controls whether automated Agent/Script steps inherit the host user's login-shell
    /// environment (`inherit`, default) or use a minimal `PATH`-only environment (`minimal`).
    #[serde(default, skip_serializing_if = "is_default_host_terminal_env_mode")]
    pub terminal_env_mode: HostTerminalEnvMode,
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
            relay_endpoint: None,
            relay_token: None,
            scheduler: SchedulerSettings::default(),
            hc: "hc".into(),
            hc_session: "default".into(),
            hc_env: BTreeMap::new(),
            terminal_env_mode: HostTerminalEnvMode::default(),
            onboarded: false,
            default_agent: "developer".into(),
            profiles: vec![
                profile("codex", "codex", false, ""),
                profile("claude", "claude", false, ""),
                profile(
                    "developer",
                    "codex",
                    true,
                    include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../../prompts/profiles/role_developer.txt"
                    ))
                    .trim_end(),
                ),
                profile(
                    "reviewer",
                    "codex",
                    true,
                    include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../../prompts/profiles/role_reviewer.txt"
                    ))
                    .trim_end(),
                ),
                profile(
                    "planner",
                    "codex",
                    true,
                    include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../../prompts/profiles/role_planner.txt"
                    ))
                    .trim_end(),
                ),
                profile(
                    "merger",
                    "codex",
                    true,
                    include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../../prompts/profiles/role_merger.txt"
                    ))
                    .trim_end(),
                ),
            ],
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectAgentOverride {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub orchestrator: Option<OrchestratorConfig>,
}
impl ProjectAgentOverride {
    pub fn from_agent(agent: impl Into<String>) -> Self {
        let agent = agent.into();
        let agent = agent.trim();
        Self {
            agent: (!agent.is_empty()).then(|| agent.to_owned()),
            model: None,
            effort: None,
            orchestrator: None,
        }
    }
    pub fn normalized(&self) -> Self {
        Self {
            agent: clean_option_str(&self.agent),
            model: clean_option_str(&self.model),
            effort: clean_option_str(&self.effort),
            orchestrator: self
                .orchestrator
                .as_ref()
                .and_then(|o| o.normalized().into_non_empty()),
        }
    }
    pub fn is_empty(&self) -> bool {
        let n = self.normalized();
        n.agent.is_none() && n.model.is_none() && n.effort.is_none() && n.orchestrator.is_none()
    }
}
impl Serialize for ProjectAgentOverride {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let n = self.normalized();
        let mut count = 0;
        if n.agent.is_some() {
            count += 1;
        }
        if n.model.is_some() {
            count += 1;
        }
        if n.effort.is_some() {
            count += 1;
        }
        if n.orchestrator.is_some() {
            count += 1;
        }
        let mut map = serializer.serialize_map(Some(count))?;
        if let Some(agent) = &n.agent {
            map.serialize_entry("agent", agent)?;
        }
        if let Some(model) = &n.model {
            map.serialize_entry("model", model)?;
        }
        if let Some(effort) = &n.effort {
            map.serialize_entry("effort", effort)?;
        }
        if let Some(orchestrator) = &n.orchestrator {
            map.serialize_entry("orchestrator", orchestrator)?;
        }
        map.end()
    }
}
impl<'de> Deserialize<'de> for ProjectAgentOverride {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Agent(String),
            Full {
                #[serde(default)]
                agent: Option<String>,
                #[serde(default)]
                model: Option<String>,
                #[serde(default)]
                effort: Option<String>,
                #[serde(default)]
                orchestrator: Option<OrchestratorConfig>,
            },
        }
        match Raw::deserialize(deserializer)? {
            Raw::Agent(s) => Ok(Self::from_agent(s).normalized()),
            Raw::Full {
                agent,
                model,
                effort,
                orchestrator,
            } => Ok(Self {
                agent,
                model,
                effort,
                orchestrator,
            }
            .normalized()),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DetectedGitHub {
    pub repository: String,
    pub remote: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Project {
    #[serde(default)]
    pub github: Option<GitHubSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_github: Option<DetectedGitHub>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub github_disconnected: bool,
    #[serde(default)]
    pub delivery_mode: DeliveryMode,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_overrides: BTreeMap<String, ProjectAgentOverride>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub id: String,
    pub name: String,
    pub directory: String,
    pub workflow: Workflow,
    #[serde(default)]
    pub target_branch: Option<String>,
    pub created_at: u64,
}
pub const MAX_TAG_CHARS: usize = 48;
/// Normalize a `#tag` or `tag` token when it is a valid indexing tag name.
/// Pure ASCII numbers remain Card/Issue references (`#18`), never tags.
pub fn normalize_tag(raw: &str) -> Option<String> {
    let trimmed = raw.trim().strip_prefix('#').unwrap_or(raw.trim());
    if trimmed.is_empty() || trimmed.chars().count() > MAX_TAG_CHARS {
        return None;
    }
    if !trimmed
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(trimmed.to_owned())
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ManagedWorktree {
    pub id: String,
    pub project_id: String,
    pub branch: String,
    pub base_branch: String,
    #[serde(default)]
    pub base_commit: Option<String>,
    pub path: String,
    pub directory: String,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceBinding {
    pub directory: String,
    pub path: String,
    pub branch: Option<String>,
    pub base_commit: Option<String>,
    pub target_branch: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Card {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_source: Option<ScheduleSourceRef>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<CardResolution>,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CardResolution {
    pub resolved_at: u64,
    /// e.g. "merged" | "manual" | "agent"
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub actor: String,
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
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CardEventSummary {
    pub last_event_seq: u64,
    pub reply_count: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReminderEventMeta {
    pub kind: String,
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_label: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reminder {
    pub id: String,
    pub project_id: String,
    pub card_id: String,
    pub message: String,
    pub unread: bool,
    pub pending: bool,
    /// Card-local Timeline position. Zero denotes a state-only recovery item.
    pub sequence: u64,
    pub created_at: u64,
    #[serde(default)]
    pub audible_when_unread: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<ReminderEventMeta>,
}
impl AsRef<str> for Reminder {
    fn as_ref(&self) -> &str {
        &self.id
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: String,
    pub project_id: String,
    pub card_id: String,
    pub sequence: u64,
    pub kind: String,
    pub actor: String,
    // Read views normalize comment text to `body`; stored events still serialize `message`.
    #[serde(alias = "body")]
    pub message: String,
    pub data: Value,
    pub created_at: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ExecutionTrigger {
    #[default]
    Workflow,
    StageEntry,
    Interactive,
    Comment {
        comment_id: String,
    },
    Custom(String),
}
impl ExecutionTrigger {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "" | "workflow" => Self::Workflow,
            "stage_entry" => Self::StageEntry,
            "interactive" => Self::Interactive,
            _ if raw.starts_with("comment:") => Self::Comment {
                comment_id: raw["comment:".len()..].to_owned(),
            },
            other => Self::Custom(other.to_owned()),
        }
    }
    pub fn is_automatic(&self) -> bool {
        matches!(self, Self::Workflow | Self::StageEntry)
    }
    pub fn is_comment(&self) -> bool {
        matches!(self, Self::Comment { .. })
    }
    pub fn comment_id(&self) -> Option<&str> {
        match self {
            Self::Comment { comment_id } => Some(comment_id),
            _ => None,
        }
    }
}
impl std::fmt::Display for ExecutionTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Workflow => f.write_str("workflow"),
            Self::StageEntry => f.write_str("stage_entry"),
            Self::Interactive => f.write_str("interactive"),
            Self::Comment { comment_id } => write!(f, "comment:{comment_id}"),
            Self::Custom(raw) => f.write_str(raw),
        }
    }
}
impl From<&str> for ExecutionTrigger {
    fn from(raw: &str) -> Self {
        Self::parse(raw)
    }
}
impl From<String> for ExecutionTrigger {
    fn from(raw: String) -> Self {
        Self::parse(&raw)
    }
}
impl Serialize for ExecutionTrigger {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for ExecutionTrigger {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::parse(&s))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QueuedAssignment {
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub trigger: ExecutionTrigger,
    #[serde(default)]
    pub profile: Profile,
    #[serde(default)]
    pub instruction: String,
    #[serde(default)]
    pub on_completed: Option<String>,
    #[serde(default)]
    pub require_human_review: bool,
    #[serde(default)]
    pub workflow_run_id: Option<String>,
    #[serde(default)]
    pub workflow_step: Option<String>,
    #[serde(default)]
    pub queued_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentRun {
    #[serde(default)]
    pub workflow_run_id: Option<String>,
    #[serde(default)]
    pub workflow_step: Option<String>,
    #[serde(default)]
    pub resume_session: bool,
    #[serde(default)]
    pub trigger: ExecutionTrigger,
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub profile: Profile,
    #[serde(default)]
    pub instruction: String,
    #[serde(default)]
    pub on_completed: Option<String>,
    #[serde(default)]
    pub require_human_review: bool,
    #[serde(default)]
    pub workspace: Option<WorkspaceBinding>,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    pub project_id: String,
    pub card_id: String,
    #[serde(default)]
    pub card_number: u64,
    #[serde(default)]
    pub number: u32,
    pub state: String,
    #[serde(default)]
    pub core_id: Option<String>,
    #[serde(default)]
    pub terminal: Option<String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub input_path: String,
    #[serde(default)]
    pub output_path: Option<String>,
    #[serde(default)]
    pub observed: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub started_at: Option<u64>,
    #[serde(default)]
    pub updated_at: u64,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<QueuedAssignment>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub applied_env: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub applied_env_fingerprint: String,
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
    #[serde(default)]
    pub interactive_followup: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RuntimeWait {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub message: String,
    pub since: u64,
}
impl AgentRun {
    pub fn finished(&self) -> bool {
        matches!(self.state.as_str(), "cancelled" | "superseded")
            || self.connection_state == "missing"
            || self.terminal.is_none()
                && matches!(self.state.as_str(), "failed" | "completed" | "backlog")
            || self
                .observed
                .as_ref()
                .is_some_and(|v| v["state"] == "finished")
    }
    /// True when the terminal exited or Herdr reports the Agent is no longer running a turn.
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
            Some("idle" | "done" | "blocked" | "error") => true,
            Some(_) => false,
            None => matches!(
                self.state.as_str(),
                "completed" | "backlog" | "blocked" | "failed"
            ),
        }
    }
    /// A stale or disconnected status must not release scheduling capacity.
    pub fn runtime_status(&self) -> &str {
        self.runtime_status_at(now())
    }
    /// Pure clock-injected variant of [`Self::runtime_status`].
    pub fn runtime_status_at(&self, at: u64) -> &str {
        if self.finished() {
            return "closed";
        }
        if self.connection_state != "connected"
            || self
                .lifecycle
                .observed_at
                .is_none_or(|t| at.saturating_sub(t) > 30_000)
        {
            return "unknown";
        }
        self.observed
            .as_ref()
            .and_then(|v| v["status"]["state"].as_str())
            .unwrap_or("unknown")
    }
    /// Update `observed` with a Herdr terminal state and Agent status state.
    pub fn set_observed_status(&mut self, terminal_state: &str, status_state: &str) {
        self.observed = Some(serde_json::json!({
            "state": terminal_state,
            "status": { "state": status_state },
        }));
    }
    pub fn promote_pending(&mut self) -> Option<QueuedAssignment> {
        self.promote_pending_at(now())
    }
    /// Pure clock-injected variant of [`Self::promote_pending`].
    pub fn promote_pending_at(&mut self, now_ms: u64) -> Option<QueuedAssignment> {
        let pending = self.pending.take()?;
        let stopped = self.finished();
        self.workflow_run_id = pending.workflow_run_id.clone();
        self.workflow_step = pending.workflow_step.clone();
        self.trigger = pending.trigger.clone();
        self.stage = pending.stage.clone();
        self.profile = pending.profile.clone();
        self.instruction = pending.instruction.clone();
        self.on_completed = pending.on_completed.clone();
        self.require_human_review = pending.require_human_review;
        self.state = "queued".into();
        self.error = None;
        self.started_at = None;
        self.completed_at = None;
        self.completion_turn = None;
        self.turn_ended_at = None;
        self.close_requested_at = None;
        self.lifecycle.interactive_followup = false;
        if stopped {
            self.core_id = None;
            self.terminal = None;
            self.observed = None;
            self.output_path = None;
            self.response_path = None;
            self.close_state.clear();
            self.connection_state.clear();
            self.lifecycle = Lifecycle::default();
            self.applied_env.clear();
            self.applied_env_fingerprint.clear();
        }
        self.updated_at = pending.queued_at.max(now_ms);
        Some(pending)
    }
}

/// Merge-time caution about Agent work on a Card, or `None` when every Agent has
/// stopped its turn. Shared by AppServer previews and the live UI (which must
/// re-evaluate it as Herdr status changes; a preview snapshot goes stale once an
/// Agent that reported while finishing its turn becomes idle).
pub fn agent_activity_warning(card_id: &str, terminals: &[TerminalEntry]) -> Option<String> {
    let agents: Vec<&AgentRun> = terminals
        .iter()
        .filter_map(|t| t.agent.as_ref())
        .filter(|a| a.card_id == card_id)
        .collect();
    let live = agents
        .iter()
        .copied()
        .filter(|a| a.terminal.is_some() && !a.agent_stopped());
    let mut uncertain = false;
    for a in live {
        if a.connection_state == "connected" {
            return Some(
                "Card Agent is still running; merging now only includes current worktree changes"
                    .into(),
            );
        }
        uncertain = true;
    }
    if uncertain {
        return Some(
            "Card Agent status is unavailable (terminal disconnected); merging now only includes current worktree changes"
                .into(),
        );
    }
    let mut pending_labels = Vec::new();
    for a in agents {
        if a.terminal.is_none() && matches!(a.state.as_str(), "queued" | "starting" | "unknown") {
            pending_labels.push(format!("@{} {}", a.profile.id, a.state));
        } else if let Some(p) = &a.pending {
            pending_labels.push(format!("@{} queued", p.profile.id));
        }
    }
    if pending_labels.is_empty() {
        return None;
    }
    Some(format!(
        "Card still has pending Agent work ({})",
        pending_labels.join(", ")
    ))
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Conversation {
    pub id: String,
    pub project_id: String,
    pub card_id: String,
    pub profile_id: String,
    pub provider: String,
    pub provider_session_id: Option<String>,
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
    #[serde(default)]
    pub terminal_id: Option<String>,
    pub state: String,
    pub error: Option<String>,
    pub reply_path: Option<String>,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Report {
    pub id: String,
    #[serde(default)]
    pub terminal_id: Option<String>,
    #[serde(default)]
    pub workflow_run_id: Option<String>,
    #[serde(default)]
    pub actor: String,
    pub project_id: String,
    pub card_id: String,
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
fn default_review_side() -> String {
    "RIGHT".into()
}
fn default_review_verdict() -> String {
    "comment".into()
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewDraftComment {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub section: String,
    pub line: u64,
    #[serde(default = "default_review_side")]
    pub side: String,
    #[serde(default)]
    pub snippet: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default)]
    pub edited_by_user: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewDraft {
    pub card_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare_sha: Option<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default = "default_review_verdict")]
    pub verdict: String,
    #[serde(default)]
    pub comments: Vec<ReviewDraftComment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default)]
    pub updated_at: u64,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncMark {
    Pending,
    Conflict,
    Failed,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncItemState {
    Queued,
    InFlight,
    Conflict,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncItemView {
    pub host_id: String,
    pub host_name: String,
    pub mutation_id: String,
    pub card_id: String,
    pub card_title: String,
    pub kind: String,
    pub state: SyncItemState,
    pub message: String,
    #[serde(default)]
    pub conflict_fields: Vec<String>,
    #[serde(default)]
    pub mine: BTreeMap<String, String>,
    #[serde(default)]
    pub server: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SyncAction {
    Retry,
    Discard,
    UseMine,
    UseEdited { fields: BTreeMap<String, String> },
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct Snapshot {
    #[serde(default)]
    pub github_sync: BTreeMap<String, GitHubSyncStatus>,
    /// Client-side host ownership (`entity_id -> host_id`) populated when merging remote hosts.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entity_hosts: BTreeMap<String, String>,
    /// Client-side host connection summaries (`host_id -> HostSummary`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub host_summaries: BTreeMap<String, HostSummary>,
    #[serde(default)]
    pub workflow_runs: Vec<WorkflowRun>,
    #[serde(default)]
    pub workflow_sources: Vec<WorkflowSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_templates: Vec<WorkflowTemplate>,
    #[serde(default)]
    pub event_subscriptions: Vec<EventSubscription>,
    #[serde(default)]
    pub configuration_sessions: Vec<ConfigurationSession>,
    #[serde(default)]
    pub merge_batches: Vec<MergeBatch>,
    #[serde(default)]
    pub schedule_states: Vec<ScheduleState>,
    #[serde(default)]
    pub scheduled_runs: Vec<ScheduledRun>,
    #[serde(default)]
    pub workflow_revisions: Vec<WorkflowRevision>,
    #[serde(default)]
    pub card_reads: Vec<CardRead>,
    #[serde(default)]
    pub github: GitHubAccount,
    #[serde(default)]
    pub github_issues: Vec<GitHubIssueSnapshot>,
    #[serde(default)]
    pub worktrees: Vec<ManagedWorktree>,
    #[serde(default)]
    pub terminals: Vec<TerminalEntry>,
    /// Ephemeral Herdr observations, never persisted as Terminal ownership.
    #[serde(default)]
    pub terminal_contexts: Vec<TerminalContext>,
    /// Cards currently executing a merge or delivery step in AppServer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merging_cards: Vec<String>,
    pub projects: Vec<Project>,
    pub cards: Vec<Card>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub card_summaries: BTreeMap<String, CardEventSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reminders: Vec<Reminder>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sound_event_ids: Vec<String>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_drafts: Vec<ReviewDraft>,
    /// Bounded ephemeral auto-reclaim / presence-lease status per managed terminal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reclaim_status: Vec<TerminalReclaimStatus>,
    /// Client-only bounded sync status markers per Card ID.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending_sync: BTreeMap<String, SyncMark>,
    /// Global variables and secret metadata (never plaintext secrets), synced across hosts.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub global_env: BTreeMap<String, EnvEntryMeta>,
    /// Scoped Agent/Project environment metadata (`"project:<id>"`, `"agent:<id>"`, `"agent:<host>:<id>"`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scoped_env: BTreeMap<String, BTreeMap<String, EnvEntryMeta>>,
}

/// Bounded runtime-only projection returned by `runtime.state` for lightweight
/// `terminals` / `reclaim` refreshes without cloning or serializing full workspace history.
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct RuntimeState {
    #[serde(default)]
    pub terminals: Vec<TerminalEntry>,
    #[serde(default)]
    pub terminal_contexts: Vec<TerminalContext>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reclaim_status: Vec<TerminalReclaimStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merging_cards: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub terminal_hosts: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub host_active_agents: BTreeMap<String, usize>,
}
impl Snapshot {
    pub fn reclaim_status_for(&self, terminal_id: &str) -> Option<&TerminalReclaimStatus> {
        self.reclaim_status
            .iter()
            .find(|r| r.terminal_id == terminal_id)
    }

    pub fn project_env_key(project_id: &str) -> String {
        format!("project:{project_id}")
    }

    pub fn agent_env_key(host_id: &str, profile_id: &str) -> String {
        if host_id.is_empty() || host_id == "local" {
            format!("agent:{profile_id}")
        } else {
            format!("agent:{host_id}:{profile_id}")
        }
    }

    pub fn project_env(&self, project_id: &str) -> BTreeMap<String, EnvEntryMeta> {
        self.scoped_env
            .get(&Self::project_env_key(project_id))
            .cloned()
            .unwrap_or_default()
    }

    pub fn agent_env(&self, host_id: &str, profile_id: &str) -> BTreeMap<String, EnvEntryMeta> {
        self.scoped_env
            .get(&Self::agent_env_key(host_id, profile_id))
            .cloned()
            .unwrap_or_default()
    }
    pub fn terminal_project_id<'a>(&'a self, t: &'a TerminalEntry) -> Option<&'a str> {
        if let Some(a) = &t.agent {
            return Some(a.project_id.as_str());
        }
        if let Some(cfg) = self.configuration_sessions.iter().find(|c| c.id == t.id) {
            return Some(cfg.project_id.as_str());
        }
        if let Some(batch) = self.merge_batches.iter().find(|b| b.id == t.id) {
            return Some(batch.project_id.as_str());
        }
        if let Some(srun) = self
            .scheduled_runs
            .iter()
            .find(|r| r.terminal_id.as_deref() == Some(t.id.as_str()) || r.id == t.id)
        {
            return Some(srun.project_id.as_str());
        }
        if let Some(ctx) = self
            .terminal_contexts
            .iter()
            .find(|c| c.terminal_id == t.id)
            && let Some(pid) = &ctx.project_id
        {
            return Some(pid.as_str());
        }
        if let Some(pid) = &t.origin_project_id {
            return Some(pid.as_str());
        }
        if let Some(cid) = &t.origin_card_id
            && let Some(c) = self.cards.iter().find(|c| &c.id == cid)
        {
            return Some(c.project_id.as_str());
        }
        None
    }

    pub fn terminal_is_live(&self, t: &TerminalEntry) -> bool {
        if t.conversation_id.is_some() {
            return false;
        }
        let Some(r) = &t.runtime else {
            return false;
        };
        if r.state == "finished" {
            return false;
        }
        if let Some(cid) = self
            .terminal_contexts
            .iter()
            .find(|c| c.terminal_id == t.id)
            .and_then(|c| c.card_id.as_deref())
            .or(t.origin_card_id.as_deref())
            && let Some(card) = self.cards.iter().find(|c| c.id == cid)
            && !card.active()
        {
            return false;
        }
        true
    }

    /// Next automatic label `<prefix> <max_N + 1>` (or `<prefix> 1`) among live
    /// independent terminals in the same Project (`exclude_id` skips a terminal being renamed).
    pub fn next_terminal_label(
        &self,
        project_id: Option<&str>,
        agent_engine: Option<&str>,
        exclude_id: Option<&str>,
    ) -> String {
        let prefix = match agent_engine {
            Some(engine) => friendly_agent_engine(engine),
            None => "Terminal".into(),
        };
        let head = format!("{prefix} ");
        let max_n = self
            .terminals
            .iter()
            .filter(|t| exclude_id != Some(t.id.as_str()))
            .filter(|t| self.terminal_is_live(t))
            .filter(|t| self.terminal_project_id(t) == project_id)
            .filter_map(|t| {
                let title = t.title();
                let suffix = title.strip_prefix(&head)?;
                if suffix.is_empty()
                    || suffix.starts_with('0')
                    || !suffix.bytes().all(|b| b.is_ascii_digit())
                {
                    return None;
                }
                suffix.parse::<u64>().ok().filter(|&n| n > 0)
            })
            .max()
            .unwrap_or(0);
        format!("{prefix} {}", max_n + 1)
    }

    /// Deduplicated custom label `<base>` (when unused) or `<base> 2`, `<base> 3`, …
    /// among live terminals globally.
    pub fn next_unique_terminal_title(&self, base: &str) -> String {
        let base = base.trim();
        let head = format!("{base} ");
        let mut exact_used = false;
        let mut max_n = 1u64;
        for t in self.terminals.iter().filter(|t| self.terminal_is_live(t)) {
            let title = t.title();
            if title == base {
                exact_used = true;
            } else if let Some(suffix) = title.strip_prefix(&head)
                && !suffix.is_empty()
                && !suffix.starts_with('0')
                && suffix.bytes().all(|b| b.is_ascii_digit())
                && let Ok(n) = suffix.parse::<u64>()
                && n >= 2
            {
                max_n = max_n.max(n);
            }
        }
        if !exact_used && max_n == 1 {
            base.to_owned()
        } else {
            format!("{base} {}", max_n + 1)
        }
    }
}
/// Durable terminal and Card-role Agent execution record.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerminalEntry {
    pub id: String,
    pub conversation_id: Option<String>,
    pub number: u64,
    pub name: Option<String>,
    /// Optional Project association when opened within a Project scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_project_id: Option<String>,
    /// Optional Card association when opened directly from a Card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_card_id: Option<String>,
    /// Optional interactive CLI engine identifier for ephemeral Agent terminals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_engine: Option<String>,
    /// Independent terminals own `runtime`; Card-role Agent terminals own `agent`.
    pub runtime: Option<TerminalRuntime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentRun>,
    pub created_at: u64,
}
impl TerminalEntry {
    pub fn default_prefix(&self) -> String {
        if let Some(engine) = &self.agent_engine {
            friendly_agent_engine(engine)
        } else {
            "Terminal".into()
        }
    }
    pub fn title(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            if self.conversation_id.is_none() {
                format!("{} {}", self.default_prefix(), self.number)
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
pub fn friendly_agent_engine(engine: &str) -> String {
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
    pub core_id: Option<String>,
    pub terminal: Option<String>,
    pub state: String,
    pub connection: String,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<RuntimeWait>,
}

/// Unified Herdr runtime state across Card Role Agents, Ephemeral CLI Agents,
/// Configuration/Merge sessions, and interactive Shell terminals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HerdrStatus {
    /// Interactive shell without an active Agent process.
    PlainShell,
    /// Scheduled or queued before PTY launch.
    Queued,
    /// PTY or Agent process is launching / connecting.
    Starting,
    /// Agent is actively running a turn in Herdr (`working` / `running`).
    Working,
    /// Agent is blocked waiting on user input / permission (`blocked`).
    Blocked,
    /// Agent completed at least one turn or reported completion (`done`, or `idle` after turn).
    Done,
    /// Agent is sitting at prompt before running a turn (`idle` at turn 0).
    Idle,
    /// Agent reported an error or failed (`error` / `failed`).
    Error,
    /// PTY connection was lost or replaced (`unavailable` / `replaced`).
    Disconnected,
    /// Terminal is closing (`closing`).
    Closing,
    /// Terminal process has exited (`finished` / `closed`).
    Ended,
}

impl HerdrStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::PlainShell => "Open",
            Self::Queued => "Queued",
            Self::Starting => "Starting",
            Self::Working => "Working",
            Self::Blocked => "Blocked",
            Self::Done => "Done",
            Self::Idle => "Idle",
            Self::Error => "Error",
            Self::Disconnected => "Offline",
            Self::Closing => "Closing",
            Self::Ended => "Ended",
        }
    }

    pub fn is_working(self) -> bool {
        matches!(self, Self::Working | Self::Starting)
    }

    pub fn is_attention(self) -> bool {
        matches!(self, Self::Blocked | Self::Error | Self::Disconnected)
    }
}

/// Classification of Canban-managed terminals vs user-created manual terminals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedTerminalKind {
    Agent,
    Scheduled,
    MergeQueue,
    Configuration,
}

/// Bounded ephemeral idle-reclaim and presence-protection status for one managed terminal.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalReclaimStatus {
    pub terminal_id: String,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub reclaiming_soon: bool,
    #[serde(default)]
    pub auto_reclaiming: bool,
    #[serde(default)]
    pub interactive_followup: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_at: Option<u64>,
}

impl TerminalEntry {
    pub fn is_special_session(&self) -> bool {
        self.id.starts_with("configuration:")
            || self.id.starts_with("merge-queue:")
            || self.id.starts_with("scheduled:")
    }

    pub fn managed_kind(&self) -> Option<ManagedTerminalKind> {
        if self.agent.is_some() {
            Some(ManagedTerminalKind::Agent)
        } else if self.id.starts_with("scheduled:") {
            Some(ManagedTerminalKind::Scheduled)
        } else if self.id.starts_with("merge-queue:") {
            Some(ManagedTerminalKind::MergeQueue)
        } else if self.id.starts_with("configuration:") {
            Some(ManagedTerminalKind::Configuration)
        } else {
            None
        }
    }

    pub fn is_managed(&self) -> bool {
        self.managed_kind().is_some()
    }

    /// Active CLI / Role Agent engine identifier if an Agent is running in this terminal.
    pub fn active_agent_engine(&self) -> Option<&str> {
        if let Some(a) = &self.agent {
            if a.finished() {
                return None;
            }
            return a.profile.preset.as_deref().or(Some(a.profile.id.as_str()));
        }
        if let Some(engine) = self.agent_engine.as_deref() {
            return Some(engine);
        }
        let r = self.runtime.as_ref()?;
        if r.state == "finished" || r.connection != "connected" {
            return None;
        }
        r.observed_agent.as_deref()
    }

    /// True when this terminal hosts an active Agent (launched as an Agent, special
    /// Configuration/Merge session, or foreground CLI Agent detected in a manual shell).
    pub fn has_active_agent(&self) -> bool {
        if let Some(a) = &self.agent {
            return !a.finished();
        }
        let Some(r) = &self.runtime else {
            return false;
        };
        if r.state == "finished" {
            return false;
        }
        self.agent_engine.is_some()
            || self.is_special_session()
            || (r.connection == "connected"
                && (r.observed_agent.is_some()
                    || matches!(
                        r.agent_status.as_deref(),
                        Some("working" | "running" | "blocked" | "error")
                    )))
    }

    /// Unified live `RuntimeWait` across Card Role Agents and independent terminals.
    pub fn runtime_wait(&self) -> Option<&RuntimeWait> {
        if let Some(a) = &self.agent {
            if a.finished() {
                return None;
            }
            return a.lifecycle.waiting.as_ref();
        }
        let r = self.runtime.as_ref()?;
        if r.state == "finished" || r.connection != "connected" || !self.has_active_agent() {
            return None;
        }
        r.waiting.as_ref()
    }

    /// Unified real-time Herdr status across all terminal types.
    pub fn herdr_status(&self) -> HerdrStatus {
        if let Some(a) = &self.agent {
            if a.finished() {
                return HerdrStatus::Ended;
            }
            if a.close_state == "closing" {
                return HerdrStatus::Closing;
            }
            if a.terminal.is_none() {
                return match a.state.as_str() {
                    "queued" => HerdrStatus::Queued,
                    "starting" => HerdrStatus::Starting,
                    "failed" => HerdrStatus::Error,
                    "blocked" => HerdrStatus::Blocked,
                    "completed" | "backlog" => HerdrStatus::Ended,
                    _ => HerdrStatus::Starting,
                };
            }
            if a.connection_state == "connecting" {
                return HerdrStatus::Starting;
            }
            if !a.connection_state.is_empty() && a.connection_state != "connected" {
                return HerdrStatus::Disconnected;
            }
            let obs_state = a
                .observed
                .as_ref()
                .and_then(|v| v["status"]["state"].as_str());
            let turn = a
                .observed
                .as_ref()
                .and_then(|v| v["turn"].as_u64())
                .unwrap_or(0);
            return match obs_state {
                Some("working" | "running") => HerdrStatus::Working,
                Some("blocked") => HerdrStatus::Blocked,
                Some("error") => HerdrStatus::Error,
                Some("done") => HerdrStatus::Done,
                Some("idle") => {
                    if a.state == "failed" || a.close_state == "error" {
                        HerdrStatus::Error
                    } else if a.state == "blocked" {
                        HerdrStatus::Blocked
                    } else if turn > 0 || a.state == "completed" {
                        HerdrStatus::Done
                    } else {
                        HerdrStatus::Idle
                    }
                }
                Some(_) => HerdrStatus::Working,
                None => match a.state.as_str() {
                    "queued" => HerdrStatus::Queued,
                    "starting" => HerdrStatus::Starting,
                    "running" | "working" => HerdrStatus::Working,
                    "blocked" => HerdrStatus::Blocked,
                    "failed" => HerdrStatus::Error,
                    "completed" => HerdrStatus::Done,
                    _ => {
                        if a.close_state == "error" {
                            HerdrStatus::Error
                        } else {
                            HerdrStatus::Starting
                        }
                    }
                },
            };
        }

        let Some(r) = &self.runtime else {
            return HerdrStatus::Disconnected;
        };
        if r.state == "finished" {
            return HerdrStatus::Ended;
        }
        if r.state == "closing" {
            return HerdrStatus::Closing;
        }
        if matches!(r.state.as_str(), "queued") {
            return HerdrStatus::Queued;
        }
        if matches!(r.state.as_str(), "starting" | "checking") || r.connection == "connecting" {
            return HerdrStatus::Starting;
        }
        if r.connection != "connected" {
            return HerdrStatus::Disconnected;
        }
        if !self.has_active_agent() {
            return HerdrStatus::PlainShell;
        }
        match r.agent_status.as_deref() {
            Some("working" | "running") => HerdrStatus::Working,
            Some("blocked") => HerdrStatus::Blocked,
            Some("error") => HerdrStatus::Error,
            Some("done") => HerdrStatus::Done,
            Some("idle") => {
                if r.agent_turn.unwrap_or(0) > 0 {
                    HerdrStatus::Done
                } else {
                    HerdrStatus::Idle
                }
            }
            Some(_) => HerdrStatus::Working,
            None => HerdrStatus::Working,
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TerminalContext {
    pub terminal_id: String,
    pub core_id: String,
    pub terminal: String,
    pub cwd: Option<String>,
    /// Canonical worktree root, or canonical cwd outside Git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory_root: Option<String>,
    pub project_id: Option<String>,
    pub card_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    pub branch: Option<String>,
    #[serde(default)]
    pub dirty: bool,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
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
        self.resolved()
            .cmp(&other.resolved())
            .then_with(|| {
                if self.resolved() {
                    other.resolved_at().cmp(&self.resolved_at())
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| self.board_rank().cmp(&other.board_rank()))
            .then_with(|| self.number.cmp(&other.number))
            .then_with(|| self.id.cmp(&other.id))
    }
    pub fn merged(&self) -> bool {
        self.delivery.is_some() || self.pull_request.as_ref().is_some_and(|p| p.merged)
    }
    pub fn resolved(&self) -> bool {
        self.resolution.is_some() || self.merged()
    }
    pub fn resolved_at(&self) -> Option<u64> {
        self.resolution
            .as_ref()
            .map(|r| r.resolved_at)
            .filter(|&t| t > 0)
            .or_else(|| {
                self.delivery
                    .as_ref()
                    .map(|d| d.merged_at)
                    .filter(|&t| t > 0)
            })
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
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubPrCheck {
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubPrChecks {
    /// Aggregated CI state: `"running" | "success" | "failure" | "none" | "unknown"`.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub passed: u32,
    #[serde(default)]
    pub failed: u32,
    #[serde(default)]
    pub running: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_checks: Vec<GitHubPrCheck>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_checks: Option<GitHubPrChecks>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks_summary: Option<GitHubPrChecks>,
    pub workspace: WorkspaceBinding,
}

pub mod cron;
mod hosts;
pub use hosts::*;
mod workflow;
pub use workflow::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_terminal(
        card_id: &str,
        state: &str,
        connection: &str,
        observed: Option<Value>,
    ) -> TerminalEntry {
        serde_json::from_value(json!({
            "id": format!("{card_id}:developer"),
            "conversation_id": format!("{card_id}:developer"),
            "number": 1,
            "name": null,
            "runtime": null,
            "created_at": 1,
            "agent": {
                "project_id": "proj-1",
                "card_id": card_id,
                "number": 1,
                "state": state,
                "core_id": "core-1",
                "terminal": "t1",
                "cwd": "/tmp",
                "input_path": "in.md",
                "updated_at": 1,
                "connection_state": connection,
                "observed": observed,
            }
        }))
        .unwrap()
    }

    #[test]
    fn agent_activity_warning_clears_as_soon_as_live_snapshot_becomes_idle() {
        let mut terminals = vec![sample_terminal(
            "card-1",
            "completed",
            "connected",
            Some(json!({"state": "live", "status": {"state": "working"}})),
        )];
        // Right at `execution.report`, Herdr can still report `"working"` mid-turn.
        assert!(
            agent_activity_warning("card-1", &terminals)
                .unwrap()
                .contains("still running")
        );

        // Once Herdr transitions to any non-running turn state, live evaluation clears it.
        for stopped in ["idle", "done", "blocked", "error"] {
            terminals[0].agent.as_mut().unwrap().observed =
                Some(json!({"state": "live", "status": {"state": stopped}}));
            assert_eq!(agent_activity_warning("card-1", &terminals), None);
        }

        // Providers without deep status hooks also clear once the execution reported an outcome.
        for outcome in ["completed", "blocked", "failed"] {
            let a = terminals[0].agent.as_mut().unwrap();
            a.state = outcome.into();
            a.observed = Some(json!({"state": "live"}));
            assert_eq!(agent_activity_warning("card-1", &terminals), None);
        }
    }

    #[test]
    fn merged_cards_sink_below_unmerged_peers_in_board_order() {
        let mut merged: Card = serde_json::from_value(json!({
            "id": "card-1",
            "project_id": "proj",
            "number": 1,
            "board_order": 0,
            "title": "Merged earlier",
            "body": "",
            "stage": "review",
            "revision": 1,
            "created_at": 1,
            "delivery": {
                "target_branch": "main",
                "commit": "abc1234",
                "request_key": "k1",
                "merged_at": 100
            }
        }))
        .unwrap();
        let open: Card = serde_json::from_value(json!({
            "id": "card-2",
            "project_id": "proj",
            "number": 2,
            "board_order": 1024,
            "title": "Open peer",
            "body": "",
            "stage": "review",
            "revision": 1,
            "created_at": 2
        }))
        .unwrap();
        assert!(merged.merged());
        assert!(merged.resolved());
        assert!(!open.merged());
        assert!(!open.resolved());
        assert_eq!(open.board_cmp(&merged), std::cmp::Ordering::Less);
        merged.delivery = None;
        assert_eq!(merged.board_cmp(&open), std::cmp::Ordering::Less);
        merged.resolution = Some(CardResolution {
            resolved_at: 200,
            kind: "manual".into(),
            reason: Some("Already fixed".into()),
            actor: "user".into(),
        });
        assert!(!merged.merged());
        assert!(merged.resolved());
        assert_eq!(merged.resolved_at(), Some(200));
        assert_eq!(open.board_cmp(&merged), std::cmp::Ordering::Less);
    }

    #[test]
    fn resolved_cards_sort_by_resolved_at_descending_with_fallback_to_board_rank() {
        let make = |id: &str, number: u64, board_order: i64| -> Card {
            serde_json::from_value(json!({
                "id": id,
                "project_id": "proj",
                "number": number,
                "board_order": board_order,
                "title": id,
                "body": "",
                "stage": "review",
                "revision": 1,
                "created_at": number
            }))
            .unwrap()
        };
        let open_high_rank = make("open-high", 10, 10_000);
        let mut resolved_older = make("resolved-older", 1, 0);
        resolved_older.resolution = Some(CardResolution {
            resolved_at: 100,
            kind: "manual".into(),
            reason: None,
            actor: "user".into(),
        });
        let mut resolved_newer = make("resolved-newer", 5, 5_000);
        resolved_newer.resolution = Some(CardResolution {
            resolved_at: 300,
            kind: "manual".into(),
            reason: None,
            actor: "user".into(),
        });
        let mut merged_mid = make("merged-mid", 3, 2_000);
        merged_mid.delivery = Some(Delivery {
            target_branch: "main".into(),
            commit: "abc123".into(),
            request_key: "k1".into(),
            merged_at: 200,
        });
        let mut resolved_tie_a = make("resolved-tie-a", 2, 100);
        resolved_tie_a.resolution = Some(CardResolution {
            resolved_at: 100,
            kind: "manual".into(),
            reason: None,
            actor: "user".into(),
        });
        let mut merged_no_ts = make("merged-no-ts", 4, -500);
        merged_no_ts.resolution = Some(CardResolution {
            resolved_at: 0,
            kind: "manual".into(),
            reason: None,
            actor: "user".into(),
        });

        let mut cards = [
            merged_no_ts,
            resolved_tie_a,
            resolved_older,
            merged_mid,
            resolved_newer,
            open_high_rank,
        ];
        cards.sort_by(Card::board_cmp);
        let ids: Vec<_> = cards.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "open-high",
                "resolved-newer",
                "merged-mid",
                "resolved-older",
                "resolved-tie-a",
                "merged-no-ts",
            ]
        );
    }

    #[test]
    fn id_generators_produce_expected_formats() {
        let short = id();
        assert_eq!(short.len(), 8);
        assert!(
            short
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        );

        let token = token_id();
        assert_eq!(token.len(), 32);
        assert!(
            token
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        );

        let v4 = uuid_v4();
        assert_eq!(v4.len(), 36);
        assert_eq!(
            v4.split('-').map(str::len).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
    }

    #[test]
    fn runtime_status_at_and_promote_pending_at_use_explicit_clock_under_2023_epoch() {
        const EPOCH_2023_MS: u64 = 1_700_000_000_000;
        let mut run = AgentRun {
            workflow_run_id: None,
            workflow_step: None,
            resume_session: false,
            trigger: ExecutionTrigger::Workflow,
            stage: "develop".into(),
            profile: Profile::default(),
            instruction: "work".into(),
            on_completed: None,
            require_human_review: false,
            workspace: None,
            directory: None,
            lifecycle: Lifecycle {
                observed_at: Some(EPOCH_2023_MS),
                ..Default::default()
            },
            project_id: "p1".into(),
            card_id: "c1".into(),
            card_number: 1,
            number: 1,
            state: "running".into(),
            core_id: Some("core".into()),
            terminal: Some("term".into()),
            cwd: "/tmp".into(),
            input_path: String::new(),
            output_path: None,
            observed: Some(json!({"state": "running", "status": {"state": "idle"}})),
            error: None,
            started_at: Some(EPOCH_2023_MS),
            updated_at: EPOCH_2023_MS,
            provider_session_id: None,
            close_state: String::new(),
            turn_ended_at: None,
            response_path: None,
            completed_at: None,
            completion_turn: None,
            close_requested_at: None,
            connection_state: "connected".into(),
            pending: Some(QueuedAssignment {
                workflow_run_id: Some("wf1".into()),
                workflow_step: Some("review:1".into()),
                trigger: ExecutionTrigger::Workflow,
                stage: "review".into(),
                profile: Profile::default(),
                instruction: "review".into(),
                on_completed: None,
                require_human_review: false,
                queued_at: EPOCH_2023_MS + 1_000,
            }),
            applied_env: BTreeMap::new(),
            applied_env_fingerprint: String::new(),
        };
        assert_eq!(run.runtime_status_at(EPOCH_2023_MS + 30_000), "idle");
        assert_eq!(run.runtime_status_at(EPOCH_2023_MS + 30_001), "unknown");

        run.promote_pending_at(EPOCH_2023_MS + 5_000)
            .expect("pending assignment promoted");
        assert_eq!(run.updated_at, EPOCH_2023_MS + 5_000);
        assert_eq!(run.stage, "review");
        assert_eq!(run.state, "queued");
    }

    #[test]
    fn next_unique_terminal_title_deduplicates_live_entries() {
        let make_term = |id: &str, name: &str, state: &str| TerminalEntry {
            id: id.into(),
            conversation_id: None,
            number: 1,
            name: Some(name.into()),
            origin_project_id: None,
            origin_card_id: None,
            agent_engine: None,
            runtime: Some(TerminalRuntime {
                core_id: Some("c1".into()),
                terminal: Some("t1".into()),
                state: state.into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
            created_at: 1,
        };
        let mut snap = Snapshot::default();
        assert_eq!(
            snap.next_unique_terminal_title("Add machine"),
            "Add machine"
        );

        // Finished entries do not reserve the base title.
        snap.terminals
            .push(make_term("s0", "Add machine", "finished"));
        assert_eq!(
            snap.next_unique_terminal_title("Add machine"),
            "Add machine"
        );

        // Live `Add machine` advances to `Add machine 2`, then `Add machine 3`.
        snap.terminals
            .push(make_term("s1", "Add machine", "running"));
        assert_eq!(
            snap.next_unique_terminal_title("Add machine"),
            "Add machine 2"
        );
        snap.terminals
            .push(make_term("s2", "Add machine 2", "running"));
        assert_eq!(
            snap.next_unique_terminal_title("Add machine"),
            "Add machine 3"
        );
    }

    #[test]
    fn orchestrator_config_merging_legacy_presets_and_override_serde() {
        let base = OrchestratorConfig {
            planner_model: Some("gemini-4-neon".into()),
            planner_effort: Some("xhigh".into()),
            worker_model: Some("gemini-4-hydrogen".into()),
            worker_effort: Some("high".into()),
        };
        // Changing only worker_effort inherits planner and worker_model.
        let partial = OrchestratorConfig {
            worker_effort: Some("medium".into()),
            ..Default::default()
        };
        assert_eq!(
            partial.merge_fallback(&base),
            OrchestratorConfig {
                planner_model: Some("gemini-4-neon".into()),
                planner_effort: Some("xhigh".into()),
                worker_model: Some("gemini-4-hydrogen".into()),
                worker_effort: Some("medium".into()),
            }
        );

        // Changing planner_model does not leak base planner_effort (e.g. xhigh -> claude-opus-5-5).
        let model_switch = OrchestratorConfig {
            planner_model: Some("claude-opus-5-5".into()),
            ..Default::default()
        };
        assert_eq!(
            model_switch.merge_fallback(&base),
            OrchestratorConfig {
                planner_model: Some("claude-opus-5-5".into()),
                planner_effort: None,
                worker_model: Some("gemini-4-hydrogen".into()),
                worker_effort: Some("high".into()),
            }
        );

        let neon_legacy = legacy_agy_orchestrator_preset("gemini-pro-orchestrator-neon").unwrap();
        assert_eq!(neon_legacy.planner_model.as_deref(), Some("gemini-4-neon"));
        assert_eq!(neon_legacy.planner_effort.as_deref(), Some("xhigh"));
        assert_eq!(
            neon_legacy.worker_model.as_deref(),
            Some("gemini-4-hydrogen")
        );
        assert_eq!(neon_legacy.worker_effort.as_deref(), Some("xhigh"));
        assert!(is_orchestrator_model_id("orchestrator"));
        assert!(is_orchestrator_model_id("gemini-pro-orchestrator"));
        assert!(!is_orchestrator_model_id("gemini-4-neon"));

        let prof = Profile {
            id: "developer".into(),
            identity: true,
            preset: Some("agy".into()),
            model: Some("orchestrator".into()),
            orchestrator: Some(OrchestratorConfig {
                planner_model: Some("claude-opus-5-5".into()),
                planner_effort: Some("high".into()),
                worker_model: Some("gemini-4-hydrogen".into()),
                worker_effort: Some("medium".into()),
            }),
            ..Default::default()
        };
        assert!(prof.is_agy());
        assert!(prof.is_orchestrator());
        assert_eq!(
            prof.effective_model(),
            "Orchestrator · Opus 5.5 (high) + Hydrogen (medium)"
        );

        let ov = ProjectAgentOverride {
            agent: Some("agy".into()),
            model: Some("orchestrator".into()),
            effort: Some("inherit".into()),
            orchestrator: Some(OrchestratorConfig {
                planner_model: Some("  gemini-4-neon ".into()),
                planner_effort: Some("inherit".into()),
                worker_model: None,
                worker_effort: Some("low".into()),
            }),
        };
        let encoded = serde_json::to_value(&ov).unwrap();
        assert_eq!(
            encoded,
            json!({
                "agent": "agy",
                "model": "orchestrator",
                "orchestrator": {
                    "planner_model": "gemini-4-neon",
                    "worker_effort": "low"
                }
            })
        );
        let decoded: ProjectAgentOverride = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, ov.normalized());

        let legacy_ov: ProjectAgentOverride = serde_json::from_value(json!("agy")).unwrap();
        assert_eq!(legacy_ov, ProjectAgentOverride::from_agent("agy"));

        let blank_orch = ProjectAgentOverride {
            agent: None,
            model: None,
            effort: None,
            orchestrator: Some(OrchestratorConfig {
                planner_model: Some("   ".into()),
                planner_effort: Some("inherit".into()),
                worker_model: None,
                worker_effort: None,
            }),
        };
        assert!(blank_orch.is_empty());
    }
}

/// Ephemeral project worker status, delivered through the normal snapshot event stream.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct GitHubSyncStatus {
    pub generation: String,
    pub phase: String,
    pub issues_added: usize,
    pub prs_added: usize,
    pub error: Option<String>,
}
