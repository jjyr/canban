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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub id: String,
    #[serde(default)]
    pub role_instruction: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub resume_argv: Vec<String>,
    pub argv: Vec<String>,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}
impl Profile {
    pub fn provider_kind(&self) -> &str {
        if !self.provider.is_empty() {
            return &self.provider;
        }
        match self
            .argv
            .first()
            .and_then(|a| std::path::Path::new(a).file_name())
            .and_then(|s| s.to_str())
        {
            Some("codex") => "codex",
            Some("claude") => "claude",
            _ => "command",
        }
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
        let mut config = Self {
            hc: "hc".into(),
            hc_session: "default".into(),
            hc_env: BTreeMap::new(),
            default_agent: "developer".into(),
            profiles: vec![
                Profile {
                    id: "codex".into(),
                    role_instruction: String::new(),
                    provider: "codex".into(),
                    resume_argv: Vec::new(),
                    argv: vec![
                        "codex",
                        "--no-daemon",
                        "--sandbox",
                        "workspace-write",
                        "--add-dir",
                        "{home}",
                        "{prompt}",
                    ]
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                    template: None,
                    tags: vec!["codex".into()],
                },
                Profile {
                    id: "claude".into(),
                    role_instruction: String::new(),
                    provider: "claude".into(),
                    resume_argv: Vec::new(),
                    argv: vec!["claude", "{prompt}"]
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                    template: None,
                    tags: vec!["claude".into()],
                },
            ],
        };
        let mut developer = config.profiles[0].clone();
        developer.id = "developer".into();
        developer.tags = vec!["developer".into()];
        developer.role_instruction = "You are the Card's development Agent. Implement the agreed task, preserve relevant project conventions, and explain your changes and validation.".into();
        let mut reviewer = config.profiles[0].clone();
        reviewer.id = "reviewer".into();
        reviewer.tags = vec!["reviewer".into()];
        reviewer.role_instruction = "You are the Card's independent review Agent. Inspect the actual changes critically, identify correctness issues and missing validation, and report findings. Do not implement changes unless explicitly asked.".into();
        config.profiles.extend([developer, reviewer]);
        config
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub directory: String,
    pub workflow: Workflow,
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
    pub created_at: u64,
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
