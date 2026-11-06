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
        Self { name: "Development".into(), stages: vec![
        Stage { id:"backlog".into(),name:"Backlog".into(),execute:false,instruction:String::new(),on_completed:None },
        Stage { id:"develop".into(),name:"Develop".into(),execute:true,instruction:"Implement the card task in the supplied working directory. Explain your changes and validation in your report.".into(),on_completed:Some("review".into()) },
        Stage { id:"review".into(),name:"Review".into(),execute:true,instruction:"Review the work for this card. Inspect the code and earlier results, then write an ordinary Markdown review report. Report findings and any remaining concerns. Do not implement unrelated changes.".into(),on_completed:None },
    ] }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub id: String,
    pub argv: Vec<String>,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "one")]
    pub capacity: usize,
}
fn one() -> usize {
    1
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub hc: String,
    pub hc_session: String,
    #[serde(default)]
    pub hc_env: BTreeMap<String, String>,
    pub default_agent: String,
    pub capacity: usize,
    pub profiles: Vec<Profile>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            hc: "hc".into(),
            hc_session: "default".into(),
            hc_env: BTreeMap::new(),
            default_agent: "codex".into(),
            capacity: 2,
            profiles: vec![
                Profile {
                    id: "codex".into(),
                    argv: vec![
                        "codex",
                        "exec",
                        "--sandbox",
                        "workspace-write",
                        "--skip-git-repo-check",
                        "--add-dir",
                        "{home}",
                        "{prompt}",
                    ]
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                    template: None,
                    tags: vec!["codex".into()],
                    capacity: 1,
                },
                Profile {
                    id: "claude".into(),
                    argv: vec!["claude", "-p", "{prompt}"]
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                    template: None,
                    tags: vec!["claude".into()],
                    capacity: 1,
                },
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
}
impl Attempt {
    pub fn key(request: &str, number: u32) -> String {
        format!("{request}:{number}")
    }
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
