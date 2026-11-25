//! Deterministic in-memory GitHub REST API simulator implementing [`canban_github::HttpTransport`].
use anyhow::Result;
use canban_github::{HttpRequest, HttpResponse, HttpTransport};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Default)]
pub struct GitHubStats {
    pub total_requests: u64,
    pub responses_200: u64,
    pub responses_304: u64,
    pub responses_error: u64,
    pub requests_by_prefix: BTreeMap<String, u64>,
}

#[derive(Clone, Debug)]
pub struct SimIssue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub author: String,
    pub labels: Vec<String>,
    pub updated_at: String,
    pub is_pr: bool,
    pub draft: bool,
    pub merged: bool,
    pub head_sha: String,
    pub head_ref: String,
    pub base_ref: String,
    pub check_conclusion: Option<String>,
}

#[derive(Clone, Debug)]
struct RepoState {
    id: u64,
    full_name: String,
    default_branch: String,
    collaborators: Vec<String>,
    issues: BTreeMap<u64, SimIssue>,
    comments: Vec<Value>,
    version: u64,
}

struct State {
    repos: BTreeMap<String, RepoState>,
    stats: GitHubStats,
    injected_errors: VecDeque<(u16, Vec<(String, String)>)>,
    /// One-shot stale responses keyed by path prefix (to simulate slow/out-of-order responses).
    stale_overrides: BTreeMap<String, Value>,
}

/// Deterministic in-memory GitHub REST API with ETag/304 support, exact counters, and fault injection.
pub struct SimGitHub {
    state: Mutex<State>,
}

impl SimGitHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                repos: BTreeMap::new(),
                stats: GitHubStats::default(),
                injected_errors: VecDeque::new(),
                stale_overrides: BTreeMap::new(),
            }),
        })
    }

    pub fn add_repo(
        &self,
        full_name: impl Into<String>,
        id: u64,
        default_branch: impl Into<String>,
    ) {
        let full_name = full_name.into();
        let mut st = self.state.lock().unwrap();
        st.repos.insert(
            full_name.clone(),
            RepoState {
                id,
                full_name,
                default_branch: default_branch.into(),
                collaborators: vec!["alice".into(), "bot".into()],
                issues: BTreeMap::new(),
                comments: Vec::new(),
                version: 1,
            },
        );
    }

    pub fn upsert_issue(&self, repo: &str, issue: SimIssue) {
        let mut st = self.state.lock().unwrap();
        if let Some(r) = st.repos.get_mut(repo) {
            r.issues.insert(issue.number, issue);
            r.version += 1;
        }
    }

    pub fn inject_error(&self, status: u16, headers: Vec<(String, String)>) {
        self.state
            .lock()
            .unwrap()
            .injected_errors
            .push_back((status, headers));
    }

    pub fn inject_stale_response_once(&self, path_contains: impl Into<String>, body: Value) {
        self.state
            .lock()
            .unwrap()
            .stale_overrides
            .insert(path_contains.into(), body);
    }

    pub fn stats(&self) -> GitHubStats {
        self.state.lock().unwrap().stats.clone()
    }

    pub fn reset_stats(&self) {
        self.state.lock().unwrap().stats = GitHubStats::default();
    }

    fn issue_json(repo: &str, i: &SimIssue) -> Value {
        let mut v = json!({
            "id": 10_000 + i.number,
            "node_id": format!("I_sim_{}", i.number),
            "number": i.number,
            "title": i.title,
            "body": i.body,
            "state": i.state,
            "draft": i.draft,
            "html_url": format!("https://github.com/{repo}/{}/{}", if i.is_pr { "pull" } else { "issues" }, i.number),
            "user": { "id": 1, "login": i.author },
            "labels": i.labels.iter().map(|l| json!({ "name": l })).collect::<Vec<_>>(),
            "assignees": [],
            "comments": 0,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": i.updated_at,
        });
        if i.is_pr {
            v["pull_request"] = json!({
                "url": format!("https://api.github.com/repos/{repo}/pulls/{}", i.number),
            });
        }
        v
    }

    fn pr_json(repo: &str, i: &SimIssue) -> Value {
        json!({
            "id": 20_000 + i.number,
            "node_id": format!("PR_sim_{}", i.number),
            "number": i.number,
            "title": i.title,
            "body": i.body,
            "state": i.state,
            "draft": i.draft,
            "merged": i.merged,
            "mergeable_state": "clean",
            "html_url": format!("https://github.com/{repo}/pull/{}", i.number),
            "user": { "id": 1, "login": i.author },
            "labels": i.labels.iter().map(|l| json!({ "name": l })).collect::<Vec<_>>(),
            "assignees": [],
            "requested_reviewers": [],
            "head": {
                "ref": i.head_ref,
                "sha": i.head_sha,
                "repo": { "full_name": repo },
            },
            "base": {
                "ref": i.base_ref,
                "sha": "base000",
                "repo": { "full_name": repo },
            },
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": i.updated_at,
        })
    }

    fn parse_query_page(path: &str) -> u64 {
        let Some((_, q)) = path.split_once('?') else {
            return 1;
        };
        for part in q.split('&') {
            if let Some(val) = part.strip_prefix("page=")
                && let Ok(n) = val.parse::<u64>()
            {
                return n;
            }
        }
        1
    }

    fn path_without_query(path: &str) -> &str {
        path.split('?').next().unwrap_or(path)
    }
}

impl HttpTransport for SimGitHub {
    fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
        let mut st = self.state.lock().unwrap();
        st.stats.total_requests += 1;
        let base_path = Self::path_without_query(&req.path).to_owned();
        *st.stats
            .requests_by_prefix
            .entry(base_path.clone())
            .or_default() += 1;

        if let Some((status, headers)) = st.injected_errors.pop_front() {
            st.stats.responses_error += 1;
            return Ok(HttpResponse {
                status,
                headers,
                body: serde_json::to_vec(&json!({"message": format!("Injected HTTP {status}")}))?,
            });
        }

        if let Some(key) = st
            .stale_overrides
            .keys()
            .find(|k| req.path.contains(k.as_str()))
            .cloned()
            && let Some(stale_val) = st.stale_overrides.remove(&key)
        {
            st.stats.responses_200 += 1;
            return Ok(HttpResponse {
                status: 200,
                headers: vec![("etag".into(), "\"stale\"".into())],
                body: serde_json::to_vec(&stale_val)?,
            });
        }

        let if_none_match = req
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("if-none-match"))
            .map(|(_, v)| v.as_str());

        if base_path == "/user" {
            st.stats.responses_200 += 1;
            return Ok(HttpResponse {
                status: 200,
                headers: vec![("etag".into(), "\"user-v1\"".into())],
                body: serde_json::to_vec(&json!({ "id": 1, "login": "bot" }))?,
            });
        }

        let Some(rest) = base_path.strip_prefix("/repos/") else {
            st.stats.responses_error += 1;
            return Ok(HttpResponse {
                status: 404,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            });
        };
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() < 2 {
            st.stats.responses_error += 1;
            return Ok(HttpResponse {
                status: 404,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            });
        }
        let repo_name = format!("{}/{}", parts[0], parts[1]);
        let Some(repo) = st.repos.get(&repo_name).cloned() else {
            st.stats.responses_error += 1;
            return Ok(HttpResponse {
                status: 404,
                headers: Vec::new(),
                body: b"{\"message\":\"Not Found\"}".to_vec(),
            });
        };

        let etag = format!("\"v{}:{}\"", repo.version, req.path);
        if req.method == "GET" && if_none_match == Some(etag.as_str()) {
            st.stats.responses_304 += 1;
            return Ok(HttpResponse {
                status: 304,
                headers: vec![("etag".into(), etag)],
                body: Vec::new(),
            });
        }

        let page = Self::parse_query_page(&req.path);
        let tail = &parts[2..];
        let payload: Value = match tail {
            [] => json!({
                "id": repo.id,
                "full_name": repo.full_name,
                "default_branch": repo.default_branch,
                "html_url": format!("https://github.com/{}", repo.full_name),
                "private": false,
            }),
            ["collaborators"] => {
                if page > 1 {
                    json!([])
                } else {
                    json!(
                        repo.collaborators
                            .iter()
                            .enumerate()
                            .map(|(idx, login)| json!({"id": idx + 1, "login": login}))
                            .collect::<Vec<_>>()
                    )
                }
            }
            ["issues"] => {
                if page > 1 {
                    json!([])
                } else {
                    json!(
                        repo.issues
                            .values()
                            .map(|i| Self::issue_json(&repo.full_name, i))
                            .collect::<Vec<_>>()
                    )
                }
            }
            ["issues", "comments"] => {
                if page > 1 {
                    json!([])
                } else {
                    json!(repo.comments)
                }
            }
            ["issues", num_str] => {
                let num: u64 = num_str.parse().unwrap_or(0);
                if let Some(i) = repo.issues.get(&num) {
                    Self::issue_json(&repo.full_name, i)
                } else {
                    st.stats.responses_error += 1;
                    return Ok(HttpResponse {
                        status: 404,
                        headers: Vec::new(),
                        body: b"{}".to_vec(),
                    });
                }
            }
            ["pulls"] => {
                if page > 1 {
                    json!([])
                } else {
                    json!(
                        repo.issues
                            .values()
                            .filter(|i| i.is_pr)
                            .map(|i| Self::pr_json(&repo.full_name, i))
                            .collect::<Vec<_>>()
                    )
                }
            }
            ["pulls", num_str] => {
                let num: u64 = num_str.parse().unwrap_or(0);
                if let Some(i) = repo.issues.get(&num).filter(|i| i.is_pr) {
                    Self::pr_json(&repo.full_name, i)
                } else {
                    st.stats.responses_error += 1;
                    return Ok(HttpResponse {
                        status: 404,
                        headers: Vec::new(),
                        body: b"{}".to_vec(),
                    });
                }
            }
            ["pulls", _num_str, "reviews"]
            | ["pulls", _num_str, "comments"]
            | ["issues", _num_str, "comments"] => {
                json!([])
            }
            ["commits", sha, "check-runs"] => {
                let conclusion = repo
                    .issues
                    .values()
                    .find(|i| i.head_sha == *sha)
                    .and_then(|i| i.check_conclusion.clone());
                let runs = match conclusion {
                    Some(c) => vec![json!({
                        "id": 1,
                        "name": "ci",
                        "status": if c == "pending" { "in_progress" } else { "completed" },
                        "conclusion": if c == "pending" { Value::Null } else { json!(c) },
                    })],
                    None => Vec::new(),
                };
                json!({ "total_count": runs.len(), "check_runs": runs })
            }
            ["commits", _sha, "status"] => json!({ "state": "success", "statuses": [] }),
            _ => json!({}),
        };

        st.stats.responses_200 += 1;
        Ok(HttpResponse {
            status: 200,
            headers: vec![("etag".into(), etag)],
            body: serde_json::to_vec(&payload)?,
        })
    }
}
