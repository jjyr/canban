use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct User {
    pub id: u64,
    pub login: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Repository {
    pub id: u64,
    pub full_name: String,
    pub default_branch: String,
    pub html_url: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Label {
    pub name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Issue {
    pub id: u64,
    pub node_id: String,
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String,
    pub html_url: String,
    pub user: User,
    #[serde(default)]
    pub labels: Vec<Label>,
    pub updated_at: String,
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Comment {
    pub id: u64,
    pub body: String,
    pub user: User,
    pub html_url: String,
    pub updated_at: String,
}
