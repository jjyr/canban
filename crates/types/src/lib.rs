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
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Workflow {
    pub name: String,
    pub stages: Vec<Stage>,
}
impl Default for Workflow {
    fn default() -> Self {
        Self {
            name: "Development".into(),
            stages: vec![
                Stage {
                    id: "backlog".into(), name: "Backlog".into(), execute: false,
                    profile: None, instruction: String::new(), on_completed: None,
                },
                Stage {
                    id: "develop".into(), name: "Develop".into(), execute: true,
                    profile: Some("developer".into()),
                    instruction: "Implement the card task in the supplied working directory. Explain your changes and validation in your report.".into(),
                    on_completed: Some("review".into()),
                },
                Stage {
                    id: "review".into(), name: "Review".into(), execute: true,
                    profile: Some("reviewer".into()),
                    instruction: "Review the work for this card. Inspect the code and earlier results, then write an ordinary Markdown review report. Report findings and any remaining concerns. Do not implement unrelated changes.".into(),
                    on_completed: None,
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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Profile {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
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
    #[serde(default)]
    pub tags: Vec<String>,
}
impl Profile {
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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub hc: String,
    pub hc_session: String,
    #[serde(default)]
    pub hc_env: BTreeMap<String, String>,
    pub default_agent: String,
    pub profiles: Vec<Profile>,
}
impl Default for Config {
    fn default() -> Self {
        let profile = |id: &str, preset: &str, role: &str| Profile {
            id: id.into(),
            preset: Some(preset.into()),
            role_instruction: role.into(),
            tags: vec![id.into()],
            ..Default::default()
        };
        Self {
            hc: "hc".into(),
            hc_session: "default".into(),
            hc_env: BTreeMap::new(),
            default_agent: "developer".into(),
            profiles: vec![
                profile("codex", "codex", ""),
                profile("claude", "claude", ""),
                profile(
                    "developer",
                    "codex",
                    "You are the Card's development Agent. Implement the agreed task, preserve relevant project conventions, and explain your changes and validation.",
                ),
                profile(
                    "reviewer",
                    "codex",
                    "You are the Card's independent review Agent. Inspect the actual changes critically, identify correctness issues and missing validation, and report findings. Do not implement changes unless explicitly asked.",
                ),
            ],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub directory: String,
    pub workflow: Workflow,
    #[serde(default)]
    pub target_branch: Option<String>,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Card {
    pub id: String,
    pub project_id: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
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
    pub id: String,
    pub project_id: String,
    pub card_id: String,
    pub trigger: String,
    pub stage: String,
    pub profile: Profile,
    pub instruction: String,
    pub on_completed: Option<String>,
    pub state: String,
    pub attempt: u32,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Attempt {
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
impl Attempt {
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
