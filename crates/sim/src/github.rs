//! Deterministic in-memory GitHub REST API simulator implementing [`canban_github::HttpTransport`].
use anyhow::{Result, anyhow};
use canban_github::{HttpRequest, HttpResponse, HttpTransport};
use canban_types::Clock;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
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

/// Optional extended PR metadata stored alongside [`SimIssue`] to preserve backwards compatibility.
#[derive(Clone, Debug, Default)]
pub struct SimPrMeta {
    pub head_repo_id: Option<u64>,
    pub head_user_login: Option<String>,
    pub head_label: Option<String>,
    pub merged_at: Option<String>,
    pub merge_commit_sha: Option<String>,
    pub mergeable_state: Option<String>,
    pub assignees: Vec<String>,
    pub requested_reviewers: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SimCheckRun {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SimReview {
    pub id: u64,
    pub author: String,
    pub state: String,
    pub body: String,
    pub commit_id: String,
    pub submitted_at: String,
}

#[derive(Clone, Debug)]
pub struct SimComment {
    pub id: u64,
    pub issue_number: u64,
    pub author: String,
    pub body: String,
    pub updated_at: String,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub side: Option<String>,
    pub in_reply_to_id: Option<u64>,
}

/// Injectable GitHub HTTP, rate-limit, or transport-layer fault.
#[derive(Clone, Debug)]
pub enum GitHubFault {
    Http {
        status: u16,
        headers: Vec<(String, String)>,
        body: Option<String>,
    },
    /// Primary rate limit (`403` with `x-ratelimit-remaining: 0` and `x-ratelimit-reset`).
    PrimaryRateLimit { reset_epoch_secs: u64 },
    /// Secondary/abuse rate limit (`403` or `429` with `retry-after`, no `x-ratelimit-remaining: 0`).
    SecondaryRateLimit { status: u16, retry_after_secs: u64 },
    /// Transport-level network/IO failure (`Err(io::Error)` rather than HTTP status).
    IoError {
        kind: io::ErrorKind,
        message: String,
    },
}

#[derive(Clone, Debug)]
struct RepoState {
    id: u64,
    full_name: String,
    default_branch: String,
    collaborators: Vec<String>,
    issues: BTreeMap<u64, SimIssue>,
    pr_meta: BTreeMap<u64, SimPrMeta>,
    issue_comments: BTreeMap<u64, Vec<SimComment>>,
    pr_reviews: BTreeMap<u64, Vec<SimReview>>,
    pr_review_comments: BTreeMap<u64, Vec<SimComment>>,
    check_runs: BTreeMap<String, Vec<SimCheckRun>>,
    commit_statuses: BTreeMap<String, Value>,
    branch_heads: BTreeMap<String, String>,
    next_comment_id: u64,
    next_review_id: u64,
    version: u64,
    resource_versions: BTreeMap<String, u64>,
}

impl RepoState {
    fn bump_resource(&mut self, keys: &[&str]) {
        self.version += 1;
        for &k in keys {
            let entry = self.resource_versions.entry(k.to_owned()).or_insert(1);
            *entry += 1;
        }
    }

    fn resource_version(&self, key: &str) -> u64 {
        self.resource_versions.get(key).copied().unwrap_or(1)
    }

    fn owner(&self) -> &str {
        self.full_name.split('/').next().unwrap_or("owner")
    }
}

#[derive(Clone, Debug)]
struct StaleReadSnapshot {
    remaining_reads: usize,
    snapshot: RepoState,
}

#[derive(Clone, Debug, Default)]
struct HostFaultState {
    offline: bool,
    injected_faults: VecDeque<GitHubFault>,
    rate_limit_remaining: Option<u64>,
    rate_limit_reset_secs: u64,
}

struct State {
    login: String,
    repos: BTreeMap<String, RepoState>,
    notifications: Vec<Value>,
    stats: GitHubStats,
    injected_faults: VecDeque<GitHubFault>,
    /// One-shot faults matched by `(HTTP method, path substring)`.
    targeted_faults: Vec<(String, String, GitHubFault)>,
    rate_limit_remaining: Option<u64>,
    rate_limit_reset_secs: u64,
    /// One-shot stale responses keyed by path substring.
    stale_overrides: BTreeMap<String, Value>,
    /// Number of subsequent GET reads per repo that observe pre-write state after a write.
    read_after_write_lag: usize,
    pending_stale_reads: BTreeMap<String, StaleReadSnapshot>,
    /// Per-host network/fault isolation state (for `SimGitHub::for_host`).
    host_faults: BTreeMap<String, HostFaultState>,
}

/// Deterministic unique positive `user.id` derived purely from a GitHub login.
///
/// Guarantees collaborators (`alice`, `bot`) and untrusted external authors (`mallory`)
/// never collide on `user.id`, matching real GitHub collaborator filtering by numeric ID.
pub fn user_id_for_login(login: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in login.trim().to_ascii_lowercase().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    100 + (h & 0x000f_ffff_ffff_ffff)
}

/// Deterministic in-memory GitHub REST API with per-resource ETag/304, pagination,
/// full write mutations, rate-limit/IO fault injection, and per-host isolation handles.
pub struct SimGitHub {
    clock: Option<Arc<dyn Clock>>,
    state: Mutex<State>,
}

/// Per-host wrapper around a shared [`SimGitHub`] allowing host-scoped offline/rate-limit faults.
pub struct SimGitHubHost {
    host_id: String,
    shared: Arc<SimGitHub>,
}

impl SimGitHubHost {
    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    pub fn shared(&self) -> &Arc<SimGitHub> {
        &self.shared
    }

    pub fn set_offline(&self, offline: bool) {
        self.shared.set_host_offline(&self.host_id, offline);
    }

    pub fn inject_fault(&self, fault: GitHubFault) {
        self.shared.inject_host_fault(&self.host_id, fault);
    }
}

impl HttpTransport for SimGitHubHost {
    fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
        self.shared.send_for_host(Some(&self.host_id), req)
    }
}

impl SimGitHub {
    pub fn new() -> Arc<Self> {
        Self::new_inner(None)
    }

    /// Creates a [`SimGitHub`] bound to a virtual [`Clock`] so primary rate-limit budgets
    /// automatically replenish once virtual `now_secs >= reset_epoch_secs`.
    pub fn with_clock(clock: Arc<dyn Clock>) -> Arc<Self> {
        Self::new_inner(Some(clock))
    }

    fn new_inner(clock: Option<Arc<dyn Clock>>) -> Arc<Self> {
        Arc::new(Self {
            clock,
            state: Mutex::new(State {
                login: "bot".into(),
                repos: BTreeMap::new(),
                notifications: Vec::new(),
                stats: GitHubStats::default(),
                injected_faults: VecDeque::new(),
                targeted_faults: Vec::new(),
                rate_limit_remaining: None,
                rate_limit_reset_secs: 1_700_003_600,
                stale_overrides: BTreeMap::new(),
                read_after_write_lag: 0,
                pending_stale_reads: BTreeMap::new(),
                host_faults: BTreeMap::new(),
            }),
        })
    }

    /// Creates a per-host [`HttpTransport`] handle backed by this shared [`SimGitHub`].
    pub fn for_host(self: &Arc<Self>, host_id: impl Into<String>) -> Arc<SimGitHubHost> {
        Arc::new(SimGitHubHost {
            host_id: host_id.into(),
            shared: Arc::clone(self),
        })
    }

    /// Returns the simulated authenticated GitHub login (served by `GET /user`).
    pub fn login(&self) -> String {
        self.state.lock().unwrap().login.clone()
    }

    /// Updates the simulated authenticated GitHub login (default: `"bot"`).
    pub fn set_login(&self, login: impl Into<String>) {
        self.state.lock().unwrap().login = login.into();
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
                pr_meta: BTreeMap::new(),
                issue_comments: BTreeMap::new(),
                pr_reviews: BTreeMap::new(),
                pr_review_comments: BTreeMap::new(),
                check_runs: BTreeMap::new(),
                commit_statuses: BTreeMap::new(),
                branch_heads: BTreeMap::new(),
                next_comment_id: 1_000,
                next_review_id: 5_000,
                version: 1,
                resource_versions: BTreeMap::new(),
            },
        );
    }

    pub fn set_collaborators(&self, repo: &str, collaborators: Vec<String>) {
        let mut st = self.state.lock().unwrap();
        if let Some(r) = st.repos.get_mut(repo) {
            r.collaborators = collaborators;
            r.bump_resource(&["collaborators"]);
        }
    }

    pub fn add_collaborator(&self, repo: &str, login: impl Into<String>) {
        let login = login.into();
        let mut st = self.state.lock().unwrap();
        if let Some(r) = st.repos.get_mut(repo)
            && !r
                .collaborators
                .iter()
                .any(|c| c.eq_ignore_ascii_case(&login))
        {
            r.collaborators.push(login);
            r.bump_resource(&["collaborators"]);
        }
    }

    pub fn upsert_issue(&self, repo: &str, issue: SimIssue) {
        let mut st = self.state.lock().unwrap();
        if let Some(r) = st.repos.get_mut(repo) {
            let num = issue.number;
            let is_pr = issue.is_pr;
            let sha = issue.head_sha.clone();
            r.issues.insert(num, issue);
            let issue_key = format!("issue:{num}");
            let pr_key = format!("pr:{num}");
            let checks_key = format!("checks:{sha}");
            if is_pr {
                r.bump_resource(&["issues", &issue_key, "pulls", &pr_key, &checks_key]);
            } else {
                r.bump_resource(&["issues", &issue_key]);
            }
        }
    }

    pub fn set_pr_meta(&self, repo: &str, number: u64, meta: SimPrMeta) {
        let mut st = self.state.lock().unwrap();
        if let Some(r) = st.repos.get_mut(repo) {
            r.pr_meta.insert(number, meta);
            let pr_key = format!("pr:{number}");
            let issue_key = format!("issue:{number}");
            r.bump_resource(&["pulls", &pr_key, "issues", &issue_key]);
        }
    }

    pub fn set_check_runs(&self, repo: &str, sha: impl Into<String>, runs: Vec<SimCheckRun>) {
        let sha = sha.into();
        let mut st = self.state.lock().unwrap();
        if let Some(r) = st.repos.get_mut(repo) {
            let key = format!("checks:{sha}");
            r.check_runs.insert(sha, runs);
            r.bump_resource(&[&key]);
        }
    }

    pub fn set_commit_status(&self, repo: &str, sha: impl Into<String>, status: Value) {
        let sha = sha.into();
        let mut st = self.state.lock().unwrap();
        if let Some(r) = st.repos.get_mut(repo) {
            let key = format!("status:{sha}");
            r.commit_statuses.insert(sha, status);
            r.bump_resource(&[&key]);
        }
    }

    /// Records the pushed tip SHA for `branch` on `repo` so `POST /pulls` resolves `head.sha`
    /// identically to real GitHub after a `git push`.
    pub fn set_branch_head(&self, repo: &str, branch: impl Into<String>, sha: impl Into<String>) {
        let mut st = self.state.lock().unwrap();
        if let Some(r) = st.repos.get_mut(repo) {
            r.branch_heads.insert(branch.into(), sha.into());
        }
    }

    /// Merges an existing open PR directly on simulated GitHub (out-of-band / browser merge),
    /// returning the generated merge commit SHA so callers can verify remote-merge reconciliation.
    pub fn merge_pr_remote(&self, repo: &str, number: u64) -> Option<String> {
        let mut st = self.state.lock().unwrap();
        let r = st.repos.get_mut(repo)?;
        let next_ver = r.version + 1;
        let pr = r.issues.get_mut(&number).filter(|i| i.is_pr)?;
        let merge_sha = format!("merge-{number}-{}", pr.head_sha);
        pr.merged = true;
        pr.state = "closed".into();
        pr.updated_at = format!("2026-06-01T02:{:02}:00Z", next_ver % 60);
        let merged_at = pr.updated_at.clone();
        let meta = r.pr_meta.entry(number).or_default();
        meta.merged_at = Some(merged_at);
        meta.merge_commit_sha = Some(merge_sha.clone());
        let pr_key = format!("pr:{number}");
        let issue_key = format!("issue:{number}");
        r.bump_resource(&["pulls", &pr_key, "issues", &issue_key]);
        Some(merge_sha)
    }

    /// Injects a one-shot fault that triggers on the next request matching `method` and
    /// `path_contains` without disturbing unrelated endpoints.
    pub fn inject_fault_on(
        &self,
        method: impl Into<String>,
        path_contains: impl Into<String>,
        fault: GitHubFault,
    ) {
        self.state.lock().unwrap().targeted_faults.push((
            method.into(),
            path_contains.into(),
            fault,
        ));
    }

    pub fn add_issue_comment(
        &self,
        repo: &str,
        number: u64,
        author: impl Into<String>,
        body: impl Into<String>,
        updated_at: impl Into<String>,
    ) -> Option<u64> {
        let mut st = self.state.lock().unwrap();
        let r = st.repos.get_mut(repo)?;
        let id = r.next_comment_id;
        r.next_comment_id += 1;
        r.issue_comments
            .entry(number)
            .or_default()
            .push(SimComment {
                id,
                issue_number: number,
                author: author.into(),
                body: body.into(),
                updated_at: updated_at.into(),
                path: None,
                line: None,
                side: None,
                in_reply_to_id: None,
            });
        let key = format!("issue_comments:{number}");
        r.bump_resource(&["comments", &key]);
        Some(id)
    }

    pub fn add_pr_review(&self, repo: &str, number: u64, mut review: SimReview) -> Option<u64> {
        let mut st = self.state.lock().unwrap();
        let r = st.repos.get_mut(repo)?;
        let id = if review.id == 0 {
            let next = r.next_review_id;
            r.next_review_id += 1;
            next
        } else {
            r.next_review_id = r.next_review_id.max(review.id + 1);
            review.id
        };
        review.id = id;
        r.pr_reviews.entry(number).or_default().push(review);
        let key = format!("pr_reviews:{number}");
        r.bump_resource(&[&key]);
        Some(id)
    }

    pub fn add_pr_review_comment(
        &self,
        repo: &str,
        number: u64,
        mut comment: SimComment,
    ) -> Option<u64> {
        let mut st = self.state.lock().unwrap();
        let r = st.repos.get_mut(repo)?;
        let id = if comment.id == 0 {
            let next = r.next_comment_id;
            r.next_comment_id += 1;
            next
        } else {
            r.next_comment_id = r.next_comment_id.max(comment.id + 1);
            comment.id
        };
        comment.id = id;
        comment.issue_number = number;
        r.pr_review_comments
            .entry(number)
            .or_default()
            .push(comment);
        let key = format!("pr_comments:{number}");
        r.bump_resource(&[&key]);
        Some(id)
    }

    /// Configures read-after-write lag: after each write mutation on a repo, the next
    /// `stale_reads` GET requests to that repo serve the pre-write snapshot.
    pub fn set_read_after_write_lag(&self, stale_reads: usize) {
        self.state.lock().unwrap().read_after_write_lag = stale_reads;
    }

    pub fn inject_error(&self, status: u16, headers: Vec<(String, String)>) {
        self.inject_fault(GitHubFault::Http {
            status,
            headers,
            body: None,
        });
    }

    pub fn inject_fault(&self, fault: GitHubFault) {
        self.state.lock().unwrap().injected_faults.push_back(fault);
    }

    pub fn inject_primary_rate_limit(&self, reset_epoch_secs: u64) {
        self.inject_fault(GitHubFault::PrimaryRateLimit { reset_epoch_secs });
    }

    pub fn inject_secondary_rate_limit(&self, status: u16, retry_after_secs: u64) {
        self.inject_fault(GitHubFault::SecondaryRateLimit {
            status,
            retry_after_secs,
        });
    }

    pub fn inject_io_error(&self, kind: io::ErrorKind, message: impl Into<String>) {
        self.inject_fault(GitHubFault::IoError {
            kind,
            message: message.into(),
        });
    }

    /// Configures a global remaining primary rate-limit budget (when exhausted, returns 403
    /// with `x-ratelimit-remaining: 0` and `x-ratelimit-reset`). Pass `None` to disable.
    pub fn set_rate_limit_budget(&self, remaining: Option<u64>, reset_epoch_secs: u64) {
        let mut st = self.state.lock().unwrap();
        st.rate_limit_remaining = remaining;
        st.rate_limit_reset_secs = reset_epoch_secs;
    }

    pub fn set_host_offline(&self, host_id: &str, offline: bool) {
        let mut st = self.state.lock().unwrap();
        st.host_faults
            .entry(host_id.to_owned())
            .or_default()
            .offline = offline;
    }

    pub fn inject_host_fault(&self, host_id: &str, fault: GitHubFault) {
        let mut st = self.state.lock().unwrap();
        st.host_faults
            .entry(host_id.to_owned())
            .or_default()
            .injected_faults
            .push_back(fault);
    }

    pub fn set_host_rate_limit_budget(
        &self,
        host_id: &str,
        remaining: Option<u64>,
        reset_epoch_secs: u64,
    ) {
        let mut st = self.state.lock().unwrap();
        let entry = st.host_faults.entry(host_id.to_owned()).or_default();
        entry.rate_limit_remaining = remaining;
        entry.rate_limit_reset_secs = reset_epoch_secs;
    }

    pub fn inject_stale_response_once(&self, path_contains: impl Into<String>, body: Value) {
        self.state
            .lock()
            .unwrap()
            .stale_overrides
            .insert(path_contains.into(), body);
    }

    /// Clears all global and per-host injected faults, rate-limit budgets, stale overrides,
    /// and pending read-after-write lag snapshots.
    pub fn clear_injected_faults(&self) {
        let mut st = self.state.lock().unwrap();
        st.injected_faults.clear();
        st.targeted_faults.clear();
        st.rate_limit_remaining = None;
        st.stale_overrides.clear();
        st.read_after_write_lag = 0;
        st.pending_stale_reads.clear();
        for hf in st.host_faults.values_mut() {
            hf.offline = false;
            hf.injected_faults.clear();
            hf.rate_limit_remaining = None;
        }
    }

    pub fn stats(&self) -> GitHubStats {
        self.state.lock().unwrap().stats.clone()
    }

    pub fn reset_stats(&self) {
        self.state.lock().unwrap().stats = GitHubStats::default();
    }

    fn user_json(login: &str) -> Value {
        json!({
            "id": user_id_for_login(login),
            "login": login,
        })
    }

    fn issue_json(repo: &RepoState, i: &SimIssue) -> Value {
        let comment_count = repo.issue_comments.get(&i.number).map_or(0, Vec::len);
        let assignees = repo
            .pr_meta
            .get(&i.number)
            .map(|m| {
                m.assignees
                    .iter()
                    .map(|l| Self::user_json(l))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut v = json!({
            "id": 10_000 + i.number,
            "node_id": format!("I_sim_{}", i.number),
            "number": i.number,
            "title": i.title,
            "body": i.body,
            "state": i.state,
            "draft": i.draft,
            "html_url": format!(
                "https://github.com/{}/{}/{}",
                repo.full_name,
                if i.is_pr { "pull" } else { "issues" },
                i.number
            ),
            "user": Self::user_json(&i.author),
            "labels": i.labels.iter().map(|l| json!({ "name": l })).collect::<Vec<_>>(),
            "assignees": assignees,
            "comments": comment_count,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": i.updated_at,
        });
        if i.is_pr {
            v["pull_request"] = json!({
                "url": format!("https://api.github.com/repos/{}/pulls/{}", repo.full_name, i.number),
            });
        }
        v
    }

    fn pr_json(repo: &RepoState, i: &SimIssue) -> Value {
        let meta = repo.pr_meta.get(&i.number);
        let repo_owner = repo.owner();
        let head_owner = meta
            .and_then(|m| m.head_user_login.as_deref())
            .unwrap_or(repo_owner);
        let head_repo_id = meta.and_then(|m| m.head_repo_id).unwrap_or(repo.id);
        let head_label = meta
            .and_then(|m| m.head_label.clone())
            .unwrap_or_else(|| format!("{head_owner}:{}", i.head_ref));
        let merged_at = if i.merged {
            meta.and_then(|m| m.merged_at.clone())
                .or_else(|| Some("2026-06-01T02:00:00Z".into()))
        } else {
            None
        };
        let merge_commit_sha = if i.merged {
            meta.and_then(|m| m.merge_commit_sha.clone())
                .or_else(|| Some(format!("merge-{}-{}", i.number, i.head_sha)))
        } else {
            None
        };
        let mergeable_state = meta
            .and_then(|m| m.mergeable_state.as_deref())
            .unwrap_or("clean");
        let assignees = meta
            .map(|m| {
                m.assignees
                    .iter()
                    .map(|l| Self::user_json(l))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let requested_reviewers = meta
            .map(|m| {
                m.requested_reviewers
                    .iter()
                    .map(|l| Self::user_json(l))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        json!({
            "id": 20_000 + i.number,
            "node_id": format!("PR_sim_{}", i.number),
            "number": i.number,
            "title": i.title,
            "body": i.body,
            "state": i.state,
            "draft": i.draft,
            "merged": i.merged,
            "merged_at": merged_at,
            "merge_commit_sha": merge_commit_sha,
            "mergeable_state": mergeable_state,
            "html_url": format!("https://github.com/{}/pull/{}", repo.full_name, i.number),
            "user": Self::user_json(&i.author),
            "labels": i.labels.iter().map(|l| json!({ "name": l })).collect::<Vec<_>>(),
            "assignees": assignees,
            "requested_reviewers": requested_reviewers,
            "head": {
                "ref": i.head_ref,
                "sha": i.head_sha,
                "label": head_label,
                "user": Self::user_json(head_owner),
                "repo": {
                    "id": head_repo_id,
                    "full_name": format!("{head_owner}/{}", repo.full_name.split('/').nth(1).unwrap_or("repo")),
                },
            },
            "base": {
                "ref": i.base_ref,
                "sha": "base000",
                "label": format!("{repo_owner}:{}", i.base_ref),
                "user": Self::user_json(repo_owner),
                "repo": {
                    "id": repo.id,
                    "full_name": repo.full_name,
                },
            },
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": i.updated_at,
        })
    }

    fn issue_comment_json(repo_name: &str, c: &SimComment) -> Value {
        json!({
            "id": c.id,
            "body": c.body,
            "user": Self::user_json(&c.author),
            "html_url": format!("https://github.com/{repo_name}/issues/{}#issuecomment-{}", c.issue_number, c.id),
            "issue_url": format!("https://api.github.com/repos/{repo_name}/issues/{}", c.issue_number),
            "created_at": c.updated_at,
            "updated_at": c.updated_at,
        })
    }

    fn pr_review_json(repo_name: &str, number: u64, r: &SimReview) -> Value {
        json!({
            "id": r.id,
            "user": Self::user_json(&r.author),
            "body": r.body,
            "state": r.state,
            "commit_id": r.commit_id,
            "html_url": format!("https://github.com/{repo_name}/pull/{number}#pullrequestreview-{}", r.id),
            "submitted_at": r.submitted_at,
        })
    }

    fn pr_review_comment_json(repo_name: &str, c: &SimComment) -> Value {
        let mut v = json!({
            "id": c.id,
            "body": c.body,
            "user": Self::user_json(&c.author),
            "path": c.path.as_deref().unwrap_or("README.md"),
            "line": c.line.unwrap_or(1),
            "side": c.side.as_deref().unwrap_or("RIGHT"),
            "html_url": format!("https://github.com/{repo_name}/pull/{}#discussion_r{}", c.issue_number, c.id),
            "pull_request_url": format!("https://api.github.com/repos/{repo_name}/pulls/{}", c.issue_number),
            "created_at": c.updated_at,
            "updated_at": c.updated_at,
        });
        if let Some(parent) = c.in_reply_to_id {
            v["in_reply_to_id"] = json!(parent);
        }
        v
    }

    fn parse_query(path: &str) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let Some((_, q)) = path.split_once('?') else {
            return out;
        };
        for part in q.split('&') {
            if part.is_empty() {
                continue;
            }
            if let Some((k, v)) = part.split_once('=') {
                out.insert(Self::percent_decode(k), Self::percent_decode(v));
            } else {
                out.insert(Self::percent_decode(part), String::new());
            }
        }
        out
    }

    fn percent_decode(input: &str) -> String {
        let bytes = input.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => {
                    out.push(b' ');
                    i += 1;
                }
                b'%' if i + 2 < bytes.len() => {
                    let hi = (bytes[i + 1] as char).to_digit(16);
                    let lo = (bytes[i + 2] as char).to_digit(16);
                    if let (Some(h), Some(l)) = (hi, lo) {
                        out.push(((h << 4) | l) as u8);
                        i += 3;
                    } else {
                        out.push(b'%');
                        i += 1;
                    }
                }
                b => {
                    out.push(b);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn path_without_query(path: &str) -> &str {
        path.split('?').next().unwrap_or(path)
    }

    fn build_page_url(base_path: &str, query: &BTreeMap<String, String>, page: usize) -> String {
        let mut params = query.clone();
        params.insert("page".into(), page.to_string());
        let qs = params
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        format!("https://api.github.com{base_path}?{qs}")
    }

    fn paginate(
        items: Vec<Value>,
        base_path: &str,
        query: &BTreeMap<String, String>,
    ) -> (Value, Option<String>) {
        let page = query
            .get("page")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .max(1);
        let per_page = query
            .get("per_page")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(30)
            .clamp(1, 100);
        let total = items.len();
        let start = (page - 1).saturating_mul(per_page);
        let slice: Vec<Value> = if start >= total {
            Vec::new()
        } else {
            let end = (start + per_page).min(total);
            items[start..end].to_vec()
        };
        let last_page = total.div_ceil(per_page).max(1);
        let link = if start.saturating_add(per_page) < total {
            let next_url = Self::build_page_url(base_path, query, page + 1);
            let last_url = Self::build_page_url(base_path, query, last_page);
            Some(format!(
                "<{next_url}>; rel=\"next\", <{last_url}>; rel=\"last\""
            ))
        } else {
            None
        };
        (Value::Array(slice), link)
    }

    fn last_modified_for_version(version: u64) -> String {
        let secs = version % 60;
        let mins = (version / 60) % 60;
        let hours = (version / 3600) % 24;
        format!("Thu, 01 Jan 1970 {hours:02}:{mins:02}:{secs:02} GMT")
    }

    fn apply_fault(stats: &mut GitHubStats, fault: GitHubFault) -> Result<HttpResponse> {
        stats.responses_error += 1;
        match fault {
            GitHubFault::Http {
                status,
                headers,
                body,
            } => {
                crate::trace::record_coverage_hit("github.fault.http");
                let raw = match body {
                    Some(b) => b.into_bytes(),
                    None => {
                        serde_json::to_vec(&json!({"message": format!("Injected HTTP {status}")}))?
                    }
                };
                Ok(HttpResponse {
                    status,
                    headers,
                    body: raw,
                })
            }
            GitHubFault::PrimaryRateLimit { reset_epoch_secs } => {
                crate::trace::record_coverage_hit("github.fault.primary_rate_limit");
                Ok(HttpResponse {
                    status: 403,
                    headers: vec![
                        ("x-ratelimit-remaining".into(), "0".into()),
                        ("x-ratelimit-reset".into(), reset_epoch_secs.to_string()),
                    ],
                    body: serde_json::to_vec(&json!({
                        "message": "API rate limit exceeded",
                    }))?,
                })
            }
            GitHubFault::SecondaryRateLimit {
                status,
                retry_after_secs,
            } => {
                crate::trace::record_coverage_hit("github.fault.secondary_rate_limit");
                Ok(HttpResponse {
                    status,
                    headers: vec![("retry-after".into(), retry_after_secs.to_string())],
                    body: serde_json::to_vec(&json!({
                        "message": "You have exceeded a secondary rate limit",
                    }))?,
                })
            }
            GitHubFault::IoError { kind, message } => {
                crate::trace::record_coverage_hit("github.fault.io_error");
                Err(anyhow!(io::Error::new(kind, message)))
            }
        }
    }

    fn not_found(stats: &mut GitHubStats) -> HttpResponse {
        stats.responses_error += 1;
        HttpResponse {
            status: 404,
            headers: Vec::new(),
            body: b"{\"message\":\"Not Found\"}".to_vec(),
        }
    }

    fn record_write_lag(st: &mut State, repo_name: &str) {
        if st.read_after_write_lag > 0
            && let Some(prev) = st.repos.get(repo_name).cloned()
        {
            st.pending_stale_reads.insert(
                repo_name.to_owned(),
                StaleReadSnapshot {
                    remaining_reads: st.read_after_write_lag,
                    snapshot: prev,
                },
            );
        }
    }

    fn send_for_host(&self, host_id: Option<&str>, req: HttpRequest) -> Result<HttpResponse> {
        crate::trace::record_coverage_hit("github.request");
        let mut st = self.state.lock().unwrap();
        st.stats.total_requests += 1;
        let base_path = Self::path_without_query(&req.path).to_owned();
        *st.stats
            .requests_by_prefix
            .entry(base_path.clone())
            .or_default() += 1;

        let now_secs = self.clock.as_ref().map(|c| c.now_ms() / 1000);

        // 1. Per-host offline / fault / rate-limit budget checks.
        if let Some(hid) = host_id
            && let Some(hf) = st.host_faults.get_mut(hid)
        {
            if hf.offline {
                st.stats.responses_error += 1;
                crate::trace::record_coverage_hit("github.fault.host_offline");
                return Err(anyhow!(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    format!("Simulated GitHub offline for host '{hid}'"),
                )));
            }
            if let Some(fault) = hf.injected_faults.pop_front() {
                return Self::apply_fault(&mut st.stats, fault);
            }
            if let Some(rem) = hf.rate_limit_remaining {
                if now_secs.is_some_and(|now| now >= hf.rate_limit_reset_secs) {
                    hf.rate_limit_remaining = None;
                    crate::trace::record_coverage_hit("github.rate_limit_budget_reset");
                } else if rem == 0 {
                    let reset = hf.rate_limit_reset_secs;
                    return Self::apply_fault(
                        &mut st.stats,
                        GitHubFault::PrimaryRateLimit {
                            reset_epoch_secs: reset,
                        },
                    );
                } else {
                    hf.rate_limit_remaining = Some(rem - 1);
                }
            }
        }

        // 2. Global injected faults, targeted method/path faults, and primary rate-limit budget.
        if let Some(fault) = st.injected_faults.pop_front() {
            return Self::apply_fault(&mut st.stats, fault);
        }
        if let Some(pos) = st.targeted_faults.iter().position(|(m, pat, _)| {
            m.eq_ignore_ascii_case(&req.method) && req.path.contains(pat.as_str())
        }) {
            let (_, _, fault) = st.targeted_faults.remove(pos);
            return Self::apply_fault(&mut st.stats, fault);
        }
        if let Some(rem) = st.rate_limit_remaining {
            if now_secs.is_some_and(|now| now >= st.rate_limit_reset_secs) {
                st.rate_limit_remaining = None;
                crate::trace::record_coverage_hit("github.rate_limit_budget_reset");
            } else if rem == 0 {
                let reset = st.rate_limit_reset_secs;
                return Self::apply_fault(
                    &mut st.stats,
                    GitHubFault::PrimaryRateLimit {
                        reset_epoch_secs: reset,
                    },
                );
            } else {
                st.rate_limit_remaining = Some(rem - 1);
            }
        }

        // 3. One-shot stale response overrides (GET only).
        if req.method == "GET"
            && let Some(key) = st
                .stale_overrides
                .keys()
                .find(|k| req.path.contains(k.as_str()))
                .cloned()
            && let Some(stale_val) = st.stale_overrides.remove(&key)
        {
            st.stats.responses_200 += 1;
            crate::trace::record_coverage_hit("github.stale_response_served");
            return Ok(HttpResponse {
                status: 200,
                headers: vec![("etag".into(), "\"stale\"".into())],
                body: serde_json::to_vec(&stale_val)?,
            });
        }

        let query = Self::parse_query(&req.path);

        // 4. Non-repo endpoints.
        if req.method == "GET" && base_path == "/user" {
            st.stats.responses_200 += 1;
            let login = st.login.clone();
            return Ok(HttpResponse {
                status: 200,
                headers: vec![("etag".into(), format!("\"user:{login}\""))],
                body: serde_json::to_vec(&Self::user_json(&login))?,
            });
        }
        if req.method == "GET" && base_path == "/notifications" {
            let (payload, link) = Self::paginate(st.notifications.clone(), &base_path, &query);
            st.stats.responses_200 += 1;
            let mut headers = vec![("etag".into(), "\"notifications:v1\"".into())];
            if let Some(l) = link {
                headers.push(("link".into(), l));
            }
            return Ok(HttpResponse {
                status: 200,
                headers,
                body: serde_json::to_vec(&payload)?,
            });
        }
        if req.method == "GET" && base_path == "/search/issues" {
            let q = query.get("q").map(String::as_str).unwrap_or("");
            let mut items = Vec::new();
            for repo in st.repos.values() {
                if q.contains("repo:") && !q.contains(&format!("repo:{}", repo.full_name)) {
                    continue;
                }
                for issue in repo.issues.values() {
                    if q.contains("is:issue") && issue.is_pr {
                        continue;
                    }
                    if q.contains("is:pr") && !issue.is_pr {
                        continue;
                    }
                    items.push(Self::issue_json(repo, issue));
                }
            }
            let total_count = items.len();
            let (page_items, link) = Self::paginate(items, &base_path, &query);
            st.stats.responses_200 += 1;
            let mut headers = Vec::new();
            if let Some(l) = link {
                headers.push(("link".into(), l));
            }
            return Ok(HttpResponse {
                status: 200,
                headers,
                body: serde_json::to_vec(&json!({
                    "total_count": total_count,
                    "incomplete_results": false,
                    "items": page_items,
                }))?,
            });
        }

        // 5. Repo-scoped endpoints (`/repos/{owner}/{repo}/...`).
        let Some(rest) = base_path.strip_prefix("/repos/") else {
            return Ok(Self::not_found(&mut st.stats));
        };
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() < 2 {
            return Ok(Self::not_found(&mut st.stats));
        }
        let repo_name = format!("{}/{}", parts[0], parts[1]);
        if !st.repos.contains_key(&repo_name) {
            return Ok(Self::not_found(&mut st.stats));
        }
        let tail = &parts[2..];

        // 6. Write HTTP methods (POST / PUT / PATCH / DELETE).
        if req.method != "GET" {
            return self.handle_write(&mut st, &req, &repo_name, tail);
        }

        // 7. GET handling (optionally served from read-after-write lag snapshot).
        let repo = if let Some(lag) = st.pending_stale_reads.get_mut(&repo_name) {
            let snap = lag.snapshot.clone();
            lag.remaining_reads = lag.remaining_reads.saturating_sub(1);
            if lag.remaining_reads == 0 {
                st.pending_stale_reads.remove(&repo_name);
            }
            crate::trace::record_coverage_hit("github.read_after_write_lag");
            crate::trace::record_coverage_hit("github.read_after_write_lag_served");
            snap
        } else {
            st.repos[&repo_name].clone()
        };

        let resource_key = Self::resource_key_for_get(tail);
        let res_ver = repo.resource_version(&resource_key);
        let etag = format!("\"r{}:v{}:{}\"", repo.id, res_ver, req.path);
        let last_modified = Self::last_modified_for_version(res_ver);

        let if_none_match = req
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("if-none-match"))
            .map(|(_, v)| v.as_str());
        let if_modified_since = req
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("if-modified-since"))
            .map(|(_, v)| v.as_str());

        if if_none_match == Some(etag.as_str())
            || (if_none_match.is_none() && if_modified_since == Some(last_modified.as_str()))
        {
            st.stats.responses_304 += 1;
            crate::trace::record_coverage_hit("github.304");
            return Ok(HttpResponse {
                status: 304,
                headers: vec![
                    ("etag".into(), etag),
                    ("last-modified".into(), last_modified),
                ],
                body: Vec::new(),
            });
        }

        let (payload, link): (Value, Option<String>) = match tail {
            [] => (
                json!({
                    "id": repo.id,
                    "full_name": repo.full_name,
                    "default_branch": repo.default_branch,
                    "html_url": format!("https://github.com/{}", repo.full_name),
                    "private": false,
                }),
                None,
            ),
            ["collaborators"] => {
                let list: Vec<Value> = repo
                    .collaborators
                    .iter()
                    .map(|login| Self::user_json(login))
                    .collect();
                Self::paginate(list, &base_path, &query)
            }
            ["issues"] => {
                let state_filter = query.get("state").map(String::as_str).unwrap_or("open");
                let since = query.get("since").map(String::as_str);
                let mut filtered: Vec<&SimIssue> = repo
                    .issues
                    .values()
                    .filter(|i| state_filter == "all" || i.state.eq_ignore_ascii_case(state_filter))
                    .filter(|i| since.is_none_or(|s| i.updated_at.as_str() >= s))
                    .collect();
                Self::sort_issues(&mut filtered, &query);
                let list: Vec<Value> = filtered
                    .into_iter()
                    .map(|i| Self::issue_json(&repo, i))
                    .collect();
                Self::paginate(list, &base_path, &query)
            }
            ["issues", "comments"] => {
                let since = query.get("since").map(String::as_str);
                let mut comments: Vec<&SimComment> = repo
                    .issue_comments
                    .values()
                    .flatten()
                    .filter(|c| since.is_none_or(|s| c.updated_at.as_str() >= s))
                    .collect();
                comments.sort_by(|a, b| a.updated_at.cmp(&b.updated_at).then(a.id.cmp(&b.id)));
                let list: Vec<Value> = comments
                    .into_iter()
                    .map(|c| Self::issue_comment_json(&repo.full_name, c))
                    .collect();
                Self::paginate(list, &base_path, &query)
            }
            ["issues", num_str] => {
                let num: u64 = num_str.parse().unwrap_or(0);
                if let Some(i) = repo.issues.get(&num) {
                    (Self::issue_json(&repo, i), None)
                } else {
                    return Ok(Self::not_found(&mut st.stats));
                }
            }
            ["issues", num_str, "comments"] => {
                let num: u64 = num_str.parse().unwrap_or(0);
                if !repo.issues.contains_key(&num) {
                    return Ok(Self::not_found(&mut st.stats));
                }
                let list: Vec<Value> = repo
                    .issue_comments
                    .get(&num)
                    .into_iter()
                    .flatten()
                    .map(|c| Self::issue_comment_json(&repo.full_name, c))
                    .collect();
                Self::paginate(list, &base_path, &query)
            }
            ["pulls"] => {
                let state_filter = query.get("state").map(String::as_str).unwrap_or("open");
                let head_filter = query.get("head").map(String::as_str);
                let base_filter = query.get("base").map(String::as_str);
                let mut filtered: Vec<&SimIssue> = repo
                    .issues
                    .values()
                    .filter(|i| i.is_pr)
                    .filter(|i| state_filter == "all" || i.state.eq_ignore_ascii_case(state_filter))
                    .filter(|i| base_filter.is_none_or(|b| i.base_ref.eq_ignore_ascii_case(b)))
                    .filter(|i| {
                        head_filter.is_none_or(|h| {
                            let meta = repo.pr_meta.get(&i.number);
                            let owner = meta
                                .and_then(|m| m.head_user_login.as_deref())
                                .unwrap_or_else(|| repo.owner());
                            let label = meta
                                .and_then(|m| m.head_label.as_deref())
                                .map(str::to_owned)
                                .unwrap_or_else(|| format!("{owner}:{}", i.head_ref));
                            label.eq_ignore_ascii_case(h) || i.head_ref.eq_ignore_ascii_case(h)
                        })
                    })
                    .collect();
                Self::sort_issues(&mut filtered, &query);
                let list: Vec<Value> = filtered
                    .into_iter()
                    .map(|i| Self::pr_json(&repo, i))
                    .collect();
                Self::paginate(list, &base_path, &query)
            }
            ["pulls", num_str] => {
                let num: u64 = num_str.parse().unwrap_or(0);
                if let Some(i) = repo.issues.get(&num).filter(|i| i.is_pr) {
                    (Self::pr_json(&repo, i), None)
                } else {
                    return Ok(Self::not_found(&mut st.stats));
                }
            }
            ["pulls", num_str, "reviews"] => {
                let num: u64 = num_str.parse().unwrap_or(0);
                if !repo.issues.get(&num).is_some_and(|i| i.is_pr) {
                    return Ok(Self::not_found(&mut st.stats));
                }
                let list: Vec<Value> = repo
                    .pr_reviews
                    .get(&num)
                    .into_iter()
                    .flatten()
                    .map(|r| Self::pr_review_json(&repo.full_name, num, r))
                    .collect();
                Self::paginate(list, &base_path, &query)
            }
            ["pulls", num_str, "comments"] => {
                let num: u64 = num_str.parse().unwrap_or(0);
                if !repo.issues.get(&num).is_some_and(|i| i.is_pr) {
                    return Ok(Self::not_found(&mut st.stats));
                }
                let list: Vec<Value> = repo
                    .pr_review_comments
                    .get(&num)
                    .into_iter()
                    .flatten()
                    .map(|c| Self::pr_review_comment_json(&repo.full_name, c))
                    .collect();
                Self::paginate(list, &base_path, &query)
            }
            ["commits", sha, "check-runs"] => {
                let runs: Vec<Value> = if let Some(explicit) = repo.check_runs.get(*sha) {
                    explicit
                        .iter()
                        .map(|r| {
                            json!({
                                "id": r.id,
                                "name": r.name,
                                "status": r.status,
                                "conclusion": r.conclusion,
                            })
                        })
                        .collect()
                } else {
                    let conclusion = repo
                        .issues
                        .values()
                        .find(|i| i.head_sha == *sha)
                        .and_then(|i| i.check_conclusion.clone());
                    match conclusion {
                        Some(c) => vec![json!({
                            "id": 1,
                            "name": "ci",
                            "status": if c == "pending" { "in_progress" } else { "completed" },
                            "conclusion": if c == "pending" { Value::Null } else { json!(c) },
                        })],
                        None => Vec::new(),
                    }
                };
                (
                    json!({ "total_count": runs.len(), "check_runs": runs }),
                    None,
                )
            }
            ["commits", sha, "status"] => {
                let status = repo
                    .commit_statuses
                    .get(*sha)
                    .cloned()
                    .unwrap_or_else(|| json!({ "state": "success", "statuses": [] }));
                (status, None)
            }
            _ => {
                return Ok(Self::not_found(&mut st.stats));
            }
        };

        st.stats.responses_200 += 1;
        let mut headers = vec![
            ("etag".into(), etag),
            ("last-modified".into(), last_modified),
        ];
        if let Some(l) = link {
            headers.push(("link".into(), l));
        }
        Ok(HttpResponse {
            status: 200,
            headers,
            body: serde_json::to_vec(&payload)?,
        })
    }

    fn resource_key_for_get(tail: &[&str]) -> String {
        match tail {
            [] => "repo".into(),
            ["collaborators"] => "collaborators".into(),
            ["issues"] => "issues".into(),
            ["issues", "comments"] => "comments".into(),
            ["issues", n] => format!("issue:{n}"),
            ["issues", n, "comments"] => format!("issue_comments:{n}"),
            ["pulls"] => "pulls".into(),
            ["pulls", n] => format!("pr:{n}"),
            ["pulls", n, "reviews"] => format!("pr_reviews:{n}"),
            ["pulls", n, "comments"] => format!("pr_comments:{n}"),
            ["commits", sha, "check-runs"] => format!("checks:{sha}"),
            ["commits", sha, "status"] => format!("status:{sha}"),
            _ => "other".into(),
        }
    }

    fn sort_issues(list: &mut [&SimIssue], query: &BTreeMap<String, String>) {
        let sort = query.get("sort").map(String::as_str).unwrap_or("created");
        let desc = query
            .get("direction")
            .map(|d| d.eq_ignore_ascii_case("desc"))
            .unwrap_or(true);
        list.sort_by(|a, b| {
            let ord = match sort {
                "updated" => a
                    .updated_at
                    .cmp(&b.updated_at)
                    .then(a.number.cmp(&b.number)),
                _ => a.number.cmp(&b.number),
            };
            if desc { ord.reverse() } else { ord }
        });
    }

    fn handle_write(
        &self,
        st: &mut State,
        req: &HttpRequest,
        repo_name: &str,
        tail: &[&str],
    ) -> Result<HttpResponse> {
        crate::trace::record_coverage_hit("github.write");
        let login = st.login.clone();
        let body = req.body.clone().unwrap_or_else(|| json!({}));

        match (req.method.as_str(), tail) {
            ("POST", ["pulls"]) => {
                Self::record_write_lag(st, repo_name);
                let repo = st.repos.get_mut(repo_name).unwrap();
                let next_num = repo.issues.keys().next_back().copied().unwrap_or(0) + 1;
                let head_raw = body["head"].as_str().unwrap_or("feature");
                let (head_owner, head_ref) = match head_raw.split_once(':') {
                    Some((o, r)) => (o.to_owned(), r.to_owned()),
                    None => (repo.owner().to_owned(), head_raw.to_owned()),
                };
                let base_ref = body["base"]
                    .as_str()
                    .unwrap_or(&repo.default_branch)
                    .to_owned();
                let title = body["title"].as_str().unwrap_or("Untitled PR").to_owned();
                let pr_body = body["body"].as_str().unwrap_or("").to_owned();
                let draft = body["draft"].as_bool().unwrap_or(false);
                let head_sha = body["head_sha"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| repo.branch_heads.get(&head_ref).cloned())
                    .unwrap_or_else(|| format!("sha-pr-{next_num}-v1"));
                let issue = SimIssue {
                    number: next_num,
                    title,
                    body: pr_body,
                    state: "open".into(),
                    author: login,
                    labels: Vec::new(),
                    updated_at: format!("2026-06-01T00:{:02}:00Z", (repo.version + 1) % 60),
                    is_pr: true,
                    draft,
                    merged: false,
                    head_sha,
                    head_ref: head_ref.clone(),
                    base_ref,
                    check_conclusion: None,
                };
                repo.pr_meta.insert(
                    next_num,
                    SimPrMeta {
                        head_user_login: Some(head_owner.clone()),
                        head_label: Some(format!("{head_owner}:{head_ref}")),
                        ..SimPrMeta::default()
                    },
                );
                repo.issues.insert(next_num, issue);
                let pr_key = format!("pr:{next_num}");
                let issue_key = format!("issue:{next_num}");
                repo.bump_resource(&["pulls", &pr_key, "issues", &issue_key]);
                let payload = Self::pr_json(repo, &repo.issues[&next_num]);
                st.stats.responses_200 += 1;
                Ok(HttpResponse {
                    status: 201,
                    headers: Vec::new(),
                    body: serde_json::to_vec(&payload)?,
                })
            }
            ("PUT", ["pulls", num_str, "merge"]) => {
                let num: u64 = num_str.parse().unwrap_or(0);
                let expected_sha = body["sha"].as_str();
                {
                    let repo = &st.repos[repo_name];
                    let Some(pr) = repo.issues.get(&num).filter(|i| i.is_pr) else {
                        return Ok(Self::not_found(&mut st.stats));
                    };
                    if pr.merged || pr.state != "open" {
                        st.stats.responses_error += 1;
                        return Ok(HttpResponse {
                            status: 405,
                            headers: Vec::new(),
                            body: serde_json::to_vec(&json!({
                                "message": "Pull Request is not mergeable",
                            }))?,
                        });
                    }
                    if let Some(sha) = expected_sha
                        && !sha.is_empty()
                        && sha != pr.head_sha
                    {
                        st.stats.responses_error += 1;
                        return Ok(HttpResponse {
                            status: 409,
                            headers: Vec::new(),
                            body: serde_json::to_vec(&json!({
                                "message": "Head branch was modified. Review and try the merge again.",
                            }))?,
                        });
                    }
                }
                Self::record_write_lag(st, repo_name);
                let repo = st.repos.get_mut(repo_name).unwrap();
                let next_ver = repo.version + 1;
                let pr = repo.issues.get_mut(&num).unwrap();
                let merge_sha = format!("merge-{num}-{}", pr.head_sha);
                pr.merged = true;
                pr.state = "closed".into();
                pr.updated_at = format!("2026-06-01T01:{:02}:00Z", next_ver % 60);
                let meta = repo.pr_meta.entry(num).or_default();
                meta.merged_at = Some(pr.updated_at.clone());
                meta.merge_commit_sha = Some(merge_sha.clone());
                let pr_key = format!("pr:{num}");
                let issue_key = format!("issue:{num}");
                repo.bump_resource(&["pulls", &pr_key, "issues", &issue_key]);
                st.stats.responses_200 += 1;
                Ok(HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: serde_json::to_vec(&json!({
                        "sha": merge_sha,
                        "merged": true,
                        "message": "Pull Request successfully merged",
                    }))?,
                })
            }
            ("PATCH", ["issues", num_str] | ["pulls", num_str]) => {
                let num: u64 = num_str.parse().unwrap_or(0);
                if !st.repos[repo_name].issues.contains_key(&num) {
                    return Ok(Self::not_found(&mut st.stats));
                }
                Self::record_write_lag(st, repo_name);
                let repo = st.repos.get_mut(repo_name).unwrap();
                let next_ver = repo.version + 1;
                let issue = repo.issues.get_mut(&num).unwrap();
                if let Some(t) = body["title"].as_str() {
                    issue.title = t.to_owned();
                }
                if let Some(b) = body["body"].as_str() {
                    issue.body = b.to_owned();
                }
                if let Some(s) = body["state"].as_str() {
                    issue.state = s.to_owned();
                }
                if let Some(labels) = body["labels"].as_array() {
                    issue.labels = labels
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect();
                }
                issue.updated_at = format!("2026-06-01T01:{:02}:00Z", next_ver % 60);
                let is_pr = issue.is_pr;
                let pr_key = format!("pr:{num}");
                let issue_key = format!("issue:{num}");
                if is_pr {
                    repo.bump_resource(&["pulls", &pr_key, "issues", &issue_key]);
                } else {
                    repo.bump_resource(&["issues", &issue_key]);
                }
                let payload = if tail[0] == "pulls" && is_pr {
                    Self::pr_json(repo, &repo.issues[&num])
                } else {
                    Self::issue_json(repo, &repo.issues[&num])
                };
                st.stats.responses_200 += 1;
                Ok(HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: serde_json::to_vec(&payload)?,
                })
            }
            ("POST", ["issues", num_str, "comments"]) => {
                let num: u64 = num_str.parse().unwrap_or(0);
                if !st.repos[repo_name].issues.contains_key(&num) {
                    return Ok(Self::not_found(&mut st.stats));
                }
                Self::record_write_lag(st, repo_name);
                let repo = st.repos.get_mut(repo_name).unwrap();
                let id = repo.next_comment_id;
                repo.next_comment_id += 1;
                let comment = SimComment {
                    id,
                    issue_number: num,
                    author: login,
                    body: body["body"].as_str().unwrap_or("").to_owned(),
                    updated_at: format!("2026-06-01T01:{:02}:00Z", (repo.version + 1) % 60),
                    path: None,
                    line: None,
                    side: None,
                    in_reply_to_id: None,
                };
                let payload = Self::issue_comment_json(&repo.full_name, &comment);
                repo.issue_comments.entry(num).or_default().push(comment);
                let key = format!("issue_comments:{num}");
                let issue_key = format!("issue:{num}");
                repo.bump_resource(&["comments", &key, "issues", &issue_key]);
                st.stats.responses_200 += 1;
                Ok(HttpResponse {
                    status: 201,
                    headers: Vec::new(),
                    body: serde_json::to_vec(&payload)?,
                })
            }
            ("PATCH", ["issues", "comments", id_str]) => {
                let id: u64 = id_str.parse().unwrap_or(0);
                let new_body = body["body"].as_str().unwrap_or("").to_owned();
                let exists = st.repos[repo_name]
                    .issue_comments
                    .values()
                    .flatten()
                    .any(|c| c.id == id);
                if !exists {
                    return Ok(Self::not_found(&mut st.stats));
                }
                Self::record_write_lag(st, repo_name);
                let repo = st.repos.get_mut(repo_name).unwrap();
                let next_ver = repo.version + 1;
                let mut updated = None;
                let mut issue_num = 0;
                for (num, list) in &mut repo.issue_comments {
                    if let Some(c) = list.iter_mut().find(|c| c.id == id) {
                        c.body = new_body.clone();
                        c.updated_at = format!("2026-06-01T01:{:02}:00Z", next_ver % 60);
                        issue_num = *num;
                        updated = Some(Self::issue_comment_json(&repo.full_name, c));
                        break;
                    }
                }
                let key = format!("issue_comments:{issue_num}");
                repo.bump_resource(&["comments", &key]);
                st.stats.responses_200 += 1;
                Ok(HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: serde_json::to_vec(&updated.unwrap())?,
                })
            }
            ("DELETE", ["issues", "comments", id_str]) => {
                let id: u64 = id_str.parse().unwrap_or(0);
                let exists = st.repos[repo_name]
                    .issue_comments
                    .values()
                    .flatten()
                    .any(|c| c.id == id);
                if !exists {
                    return Ok(Self::not_found(&mut st.stats));
                }
                Self::record_write_lag(st, repo_name);
                let repo = st.repos.get_mut(repo_name).unwrap();
                let mut issue_num = 0;
                for (num, list) in &mut repo.issue_comments {
                    let before = list.len();
                    list.retain(|c| c.id != id);
                    if list.len() != before {
                        issue_num = *num;
                        break;
                    }
                }
                let key = format!("issue_comments:{issue_num}");
                repo.bump_resource(&["comments", &key]);
                st.stats.responses_200 += 1;
                Ok(HttpResponse {
                    status: 204,
                    headers: Vec::new(),
                    body: Vec::new(),
                })
            }
            ("POST", ["pulls", num_str, "reviews"]) => {
                let num: u64 = num_str.parse().unwrap_or(0);
                if !st.repos[repo_name]
                    .issues
                    .get(&num)
                    .is_some_and(|i| i.is_pr)
                {
                    return Ok(Self::not_found(&mut st.stats));
                }
                Self::record_write_lag(st, repo_name);
                let repo = st.repos.get_mut(repo_name).unwrap();
                let id = repo.next_review_id;
                repo.next_review_id += 1;
                let event = body["event"].as_str().unwrap_or("COMMENT");
                let review_state = match event {
                    "APPROVE" => "APPROVED",
                    "REQUEST_CHANGES" => "CHANGES_REQUESTED",
                    _ => "COMMENTED",
                };
                let commit_id = body["commit_id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| repo.issues[&num].head_sha.clone());
                let submitted_at = format!("2026-06-01T01:{:02}:00Z", (repo.version + 1) % 60);
                let review = SimReview {
                    id,
                    author: login.clone(),
                    state: review_state.into(),
                    body: body["body"].as_str().unwrap_or("").to_owned(),
                    commit_id,
                    submitted_at: submitted_at.clone(),
                };
                let payload = Self::pr_review_json(&repo.full_name, num, &review);
                repo.pr_reviews.entry(num).or_default().push(review);
                if let Some(comments) = body["comments"].as_array() {
                    for c in comments {
                        let cid = repo.next_comment_id;
                        repo.next_comment_id += 1;
                        repo.pr_review_comments
                            .entry(num)
                            .or_default()
                            .push(SimComment {
                                id: cid,
                                issue_number: num,
                                author: login.clone(),
                                body: c["body"].as_str().unwrap_or("").to_owned(),
                                updated_at: submitted_at.clone(),
                                path: c["path"].as_str().map(str::to_owned),
                                line: c["line"].as_u64(),
                                side: c["side"].as_str().map(str::to_owned),
                                in_reply_to_id: None,
                            });
                    }
                }
                let rkey = format!("pr_reviews:{num}");
                let ckey = format!("pr_comments:{num}");
                repo.bump_resource(&[&rkey, &ckey]);
                st.stats.responses_200 += 1;
                Ok(HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: serde_json::to_vec(&payload)?,
                })
            }
            ("PUT", ["pulls", num_str, "reviews", id_str]) => {
                let num: u64 = num_str.parse().unwrap_or(0);
                let id: u64 = id_str.parse().unwrap_or(0);
                let exists = st.repos[repo_name]
                    .pr_reviews
                    .get(&num)
                    .is_some_and(|list| list.iter().any(|r| r.id == id));
                if !exists {
                    return Ok(Self::not_found(&mut st.stats));
                }
                Self::record_write_lag(st, repo_name);
                let repo = st.repos.get_mut(repo_name).unwrap();
                let new_body = body["body"].as_str().unwrap_or("").to_owned();
                let list = repo.pr_reviews.get_mut(&num).unwrap();
                let rev = list.iter_mut().find(|r| r.id == id).unwrap();
                rev.body = new_body;
                let payload = Self::pr_review_json(&repo.full_name, num, rev);
                let rkey = format!("pr_reviews:{num}");
                repo.bump_resource(&[&rkey]);
                st.stats.responses_200 += 1;
                Ok(HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: serde_json::to_vec(&payload)?,
                })
            }
            ("POST", ["pulls", num_str, "comments", parent_id_str, "replies"]) => {
                let num: u64 = num_str.parse().unwrap_or(0);
                let parent_id: u64 = parent_id_str.parse().unwrap_or(0);
                if !st.repos[repo_name]
                    .issues
                    .get(&num)
                    .is_some_and(|i| i.is_pr)
                {
                    return Ok(Self::not_found(&mut st.stats));
                }
                Self::record_write_lag(st, repo_name);
                let repo = st.repos.get_mut(repo_name).unwrap();
                let id = repo.next_comment_id;
                repo.next_comment_id += 1;
                let reply = SimComment {
                    id,
                    issue_number: num,
                    author: login,
                    body: body["body"].as_str().unwrap_or("").to_owned(),
                    updated_at: format!("2026-06-01T01:{:02}:00Z", (repo.version + 1) % 60),
                    path: None,
                    line: None,
                    side: None,
                    in_reply_to_id: Some(parent_id),
                };
                let payload = Self::pr_review_comment_json(&repo.full_name, &reply);
                repo.pr_review_comments.entry(num).or_default().push(reply);
                let ckey = format!("pr_comments:{num}");
                repo.bump_resource(&[&ckey]);
                st.stats.responses_200 += 1;
                Ok(HttpResponse {
                    status: 201,
                    headers: Vec::new(),
                    body: serde_json::to_vec(&payload)?,
                })
            }
            ("PATCH", ["pulls", "comments", id_str]) => {
                let id: u64 = id_str.parse().unwrap_or(0);
                let new_body = body["body"].as_str().unwrap_or("").to_owned();
                let exists = st.repos[repo_name]
                    .pr_review_comments
                    .values()
                    .flatten()
                    .any(|c| c.id == id);
                if !exists {
                    return Ok(Self::not_found(&mut st.stats));
                }
                Self::record_write_lag(st, repo_name);
                let repo = st.repos.get_mut(repo_name).unwrap();
                let next_ver = repo.version + 1;
                let mut updated = None;
                let mut pr_num = 0;
                for (num, list) in &mut repo.pr_review_comments {
                    if let Some(c) = list.iter_mut().find(|c| c.id == id) {
                        c.body = new_body.clone();
                        c.updated_at = format!("2026-06-01T01:{:02}:00Z", next_ver % 60);
                        pr_num = *num;
                        updated = Some(Self::pr_review_comment_json(&repo.full_name, c));
                        break;
                    }
                }
                let ckey = format!("pr_comments:{pr_num}");
                repo.bump_resource(&[&ckey]);
                st.stats.responses_200 += 1;
                Ok(HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: serde_json::to_vec(&updated.unwrap())?,
                })
            }
            ("DELETE", ["pulls", "comments", id_str]) => {
                let id: u64 = id_str.parse().unwrap_or(0);
                let exists = st.repos[repo_name]
                    .pr_review_comments
                    .values()
                    .flatten()
                    .any(|c| c.id == id);
                if !exists {
                    return Ok(Self::not_found(&mut st.stats));
                }
                Self::record_write_lag(st, repo_name);
                let repo = st.repos.get_mut(repo_name).unwrap();
                let mut pr_num = 0;
                for (num, list) in &mut repo.pr_review_comments {
                    let before = list.len();
                    list.retain(|c| c.id != id);
                    if list.len() != before {
                        pr_num = *num;
                        break;
                    }
                }
                let ckey = format!("pr_comments:{pr_num}");
                repo.bump_resource(&[&ckey]);
                st.stats.responses_200 += 1;
                Ok(HttpResponse {
                    status: 204,
                    headers: Vec::new(),
                    body: Vec::new(),
                })
            }
            _ => Ok(Self::not_found(&mut st.stats)),
        }
    }
}

impl HttpTransport for SimGitHub {
    fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
        self.send_for_host(None, req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: &str, path: &str, body: Option<Value>) -> HttpRequest {
        HttpRequest {
            method: method.into(),
            path: path.into(),
            headers: Vec::new(),
            body,
        }
    }

    fn sample_issue(number: u64, author: &str, is_pr: bool) -> SimIssue {
        SimIssue {
            number,
            title: format!("Item #{number}"),
            body: "body".into(),
            state: "open".into(),
            author: author.into(),
            labels: Vec::new(),
            updated_at: format!("2026-06-01T00:{:02}:00Z", number % 60),
            is_pr,
            draft: false,
            merged: false,
            head_sha: format!("sha-{number}"),
            head_ref: format!("feat/{number}"),
            base_ref: "main".into(),
            check_conclusion: Some("success".into()),
        }
    }

    #[test]
    fn collaborator_user_ids_are_unique_per_login() {
        let gh = SimGitHub::new();
        gh.add_repo("acme/app", 42, "main");
        gh.upsert_issue("acme/app", sample_issue(1, "alice", false));
        gh.upsert_issue("acme/app", sample_issue(2, "mallory", true));

        let collabs: Value = serde_json::from_slice(
            &gh.send(req("GET", "/repos/acme/app/collaborators", None))
                .unwrap()
                .body,
        )
        .unwrap();
        let collab_ids: Vec<u64> = collabs
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["id"].as_u64().unwrap())
            .collect();

        let alice_issue: Value = serde_json::from_slice(
            &gh.send(req("GET", "/repos/acme/app/issues/1", None))
                .unwrap()
                .body,
        )
        .unwrap();
        let mallory_pr: Value = serde_json::from_slice(
            &gh.send(req("GET", "/repos/acme/app/pulls/2", None))
                .unwrap()
                .body,
        )
        .unwrap();

        let alice_id = alice_issue["user"]["id"].as_u64().unwrap();
        let mallory_id = mallory_pr["user"]["id"].as_u64().unwrap();
        assert!(collab_ids.contains(&alice_id));
        assert!(
            !collab_ids.contains(&mallory_id),
            "untrusted author must never collide with collaborator user.id"
        );
        assert_ne!(alice_id, mallory_id);

        // PR schema includes head.repo.id, head.user.login, head.label, base.repo.id
        assert_eq!(mallory_pr["head"]["repo"]["id"], json!(42));
        assert_eq!(mallory_pr["head"]["user"]["login"], json!("acme"));
        assert_eq!(mallory_pr["head"]["label"], json!("acme:feat/2"));
        assert_eq!(mallory_pr["base"]["repo"]["id"], json!(42));
    }

    #[test]
    fn pagination_link_headers_and_query_filtering() {
        let gh = SimGitHub::new();
        gh.add_repo("acme/app", 7, "main");
        for n in 1..=5 {
            gh.upsert_issue("acme/app", sample_issue(n, "alice", true));
        }
        let mut closed = sample_issue(6, "alice", true);
        closed.state = "closed".into();
        closed.base_ref = "release".into();
        gh.upsert_issue("acme/app", closed);

        // Page 1 of 2 with per_page=2 -> 2 items + Link rel="next"
        let p1 = gh
            .send(req(
                "GET",
                "/repos/acme/app/pulls?state=open&per_page=2&page=1",
                None,
            ))
            .unwrap();
        assert_eq!(p1.status, 200);
        let p1_arr: Vec<Value> = serde_json::from_slice(&p1.body).unwrap();
        assert_eq!(p1_arr.len(), 2);
        let link1 = p1.header("link").expect("page 1 must include Link header");
        assert!(link1.contains("rel=\"next\""));
        assert!(link1.contains("page=2"));

        // Page 3 of 3 -> 1 item and no rel="next"
        let p3 = gh
            .send(req(
                "GET",
                "/repos/acme/app/pulls?state=open&per_page=2&page=3",
                None,
            ))
            .unwrap();
        let p3_arr: Vec<Value> = serde_json::from_slice(&p3.body).unwrap();
        assert_eq!(p3_arr.len(), 1);
        assert!(p3.header("link").is_none());

        // Filter by head (percent-encoded owner:branch) and base
        let by_head = gh
            .send(req(
                "GET",
                "/repos/acme/app/pulls?state=all&head=acme%3Afeat%2F6&base=release",
                None,
            ))
            .unwrap();
        let by_head_arr: Vec<Value> = serde_json::from_slice(&by_head.body).unwrap();
        assert_eq!(by_head_arr.len(), 1);
        assert_eq!(by_head_arr[0]["number"], json!(6));

        // Filter issues by since
        let since = gh
            .send(req(
                "GET",
                "/repos/acme/app/issues?state=all&since=2026-06-01T00:05:00Z",
                None,
            ))
            .unwrap();
        let since_arr: Vec<Value> = serde_json::from_slice(&since.body).unwrap();
        assert_eq!(since_arr.len(), 2); // #5 and #6
    }

    #[test]
    fn write_methods_mutate_state_enforce_sha_precondition_and_unknown_routes_404() {
        let gh = SimGitHub::new();
        gh.add_repo("acme/app", 9, "main");
        gh.set_login("reviewer");

        // Unknown route returns 404 (never 200 {})
        let unknown = gh
            .send(req("GET", "/repos/acme/app/does-not-exist", None))
            .unwrap();
        assert_eq!(unknown.status, 404);

        // POST /pulls creates PR #1
        let created = gh
            .send(req(
                "POST",
                "/repos/acme/app/pulls",
                Some(json!({
                    "head": "acme:feat/new",
                    "base": "main",
                    "title": "New Feature",
                    "body": "Hello",
                })),
            ))
            .unwrap();
        assert_eq!(created.status, 201);
        let pr: Value = serde_json::from_slice(&created.body).unwrap();
        assert_eq!(pr["number"], json!(1));
        assert_eq!(pr["user"]["login"], json!("reviewer"));
        let sha = pr["head"]["sha"].as_str().unwrap().to_owned();

        // POST comment -> PATCH -> DELETE
        let c_post = gh
            .send(req(
                "POST",
                "/repos/acme/app/issues/1/comments",
                Some(json!({ "body": "initial" })),
            ))
            .unwrap();
        assert_eq!(c_post.status, 201);
        let cid = serde_json::from_slice::<Value>(&c_post.body).unwrap()["id"]
            .as_u64()
            .unwrap();
        let c_patch = gh
            .send(req(
                "PATCH",
                &format!("/repos/acme/app/issues/comments/{cid}"),
                Some(json!({ "body": "edited" })),
            ))
            .unwrap();
        assert_eq!(c_patch.status, 200);
        let c_del = gh
            .send(req(
                "DELETE",
                &format!("/repos/acme/app/issues/comments/{cid}"),
                None,
            ))
            .unwrap();
        assert_eq!(c_del.status, 204);

        // POST review
        let rev = gh
            .send(req(
                "POST",
                "/repos/acme/app/pulls/1/reviews",
                Some(json!({ "event": "APPROVE", "body": "LGTM", "commit_id": sha })),
            ))
            .unwrap();
        assert_eq!(rev.status, 200);
        let rev_list: Vec<Value> = serde_json::from_slice(
            &gh.send(req("GET", "/repos/acme/app/pulls/1/reviews", None))
                .unwrap()
                .body,
        )
        .unwrap();
        assert_eq!(rev_list.len(), 1);
        assert_eq!(rev_list[0]["state"], json!("APPROVED"));

        // PUT /pulls/1/merge with wrong SHA returns 409 Conflict
        let mismatch = gh
            .send(req(
                "PUT",
                "/repos/acme/app/pulls/1/merge",
                Some(json!({ "sha": "stale-sha" })),
            ))
            .unwrap();
        assert_eq!(mismatch.status, 409);

        // PUT /pulls/1/merge with matching SHA merges and populates merged_at & merge_commit_sha
        let merged = gh
            .send(req(
                "PUT",
                "/repos/acme/app/pulls/1/merge",
                Some(json!({ "sha": sha })),
            ))
            .unwrap();
        assert_eq!(merged.status, 200);
        let fetched: Value = serde_json::from_slice(
            &gh.send(req("GET", "/repos/acme/app/pulls/1", None))
                .unwrap()
                .body,
        )
        .unwrap();
        assert_eq!(fetched["merged"], json!(true));
        assert!(fetched["merged_at"].is_string());
        assert!(fetched["merge_commit_sha"].is_string());
    }

    #[test]
    fn per_resource_etags_read_after_write_lag_rate_limits_io_errors_and_host_isolation() {
        let gh = SimGitHub::new();
        gh.add_repo("acme/app", 1, "main");
        gh.upsert_issue("acme/app", sample_issue(1, "alice", false));

        // Fetch collaborators + issue 1 ETags
        let c1 = gh
            .send(req("GET", "/repos/acme/app/collaborators", None))
            .unwrap();
        let c_etag = c1.header("etag").unwrap().to_owned();
        let i1 = gh
            .send(req("GET", "/repos/acme/app/issues/1", None))
            .unwrap();
        let i_etag = i1.header("etag").unwrap().to_owned();

        // Mutating issue #2 must NOT invalidate collaborators or issue #1 ETags!
        gh.upsert_issue("acme/app", sample_issue(2, "alice", false));
        let c_check = gh
            .send(HttpRequest {
                method: "GET".into(),
                path: "/repos/acme/app/collaborators".into(),
                headers: vec![("If-None-Match".into(), c_etag)],
                body: None,
            })
            .unwrap();
        assert_eq!(c_check.status, 304);
        let i1_check = gh
            .send(HttpRequest {
                method: "GET".into(),
                path: "/repos/acme/app/issues/1".into(),
                headers: vec![("If-None-Match".into(), i_etag)],
                body: None,
            })
            .unwrap();
        assert_eq!(i1_check.status, 304);

        // Read-after-write lag serves pre-write state once before showing the new title.
        gh.set_read_after_write_lag(1);
        gh.send(req(
            "PATCH",
            "/repos/acme/app/issues/1",
            Some(json!({ "title": "Updated Title" })),
        ))
        .unwrap();
        let stale: Value = serde_json::from_slice(
            &gh.send(req("GET", "/repos/acme/app/issues/1", None))
                .unwrap()
                .body,
        )
        .unwrap();
        assert_eq!(stale["title"], json!("Item #1"));
        let fresh: Value = serde_json::from_slice(
            &gh.send(req("GET", "/repos/acme/app/issues/1", None))
                .unwrap()
                .body,
        )
        .unwrap();
        assert_eq!(fresh["title"], json!("Updated Title"));

        // Primary rate limit, secondary rate limit, and IO transport error
        gh.inject_primary_rate_limit(1_700_005_000);
        let r_pri = gh.send(req("GET", "/user", None)).unwrap();
        assert_eq!(r_pri.status, 403);
        assert_eq!(r_pri.header("x-ratelimit-remaining"), Some("0"));
        assert_eq!(r_pri.header("x-ratelimit-reset"), Some("1700005000"));

        gh.inject_secondary_rate_limit(429, 60);
        let r_sec = gh.send(req("GET", "/user", None)).unwrap();
        assert_eq!(r_sec.status, 429);
        assert_eq!(r_sec.header("retry-after"), Some("60"));

        gh.inject_io_error(io::ErrorKind::TimedOut, "simulated timeout");
        assert!(gh.send(req("GET", "/user", None)).is_err());

        // Per-host isolation: remote-a offline while remote-b succeeds.
        let host_a = gh.for_host("remote-a");
        let host_b = gh.for_host("remote-b");
        host_a.set_offline(true);
        assert!(host_a.send(req("GET", "/user", None)).is_err());
        assert_eq!(host_b.send(req("GET", "/user", None)).unwrap().status, 200);

        gh.clear_injected_faults();
        assert_eq!(host_a.send(req("GET", "/user", None)).unwrap().status, 200);
    }

    #[test]
    fn rate_limit_budgets_auto_replenish_at_virtual_reset_and_raw_lag_hit_requires_write() {
        use std::sync::atomic::{AtomicU64, Ordering};

        struct FakeClock(AtomicU64);
        impl Clock for FakeClock {
            fn now_ms(&self) -> u64 {
                self.0.load(Ordering::SeqCst)
            }
        }

        let clock = Arc::new(FakeClock(AtomicU64::new(1_700_000_000_000)));
        let gh = SimGitHub::with_clock(clock.clone());
        gh.add_repo("acme/app", 1, "main");
        gh.upsert_issue("acme/app", sample_issue(1, "alice", false));

        // Setting read-after-write lag without a write must NOT hit `github.read_after_write_lag_served`.
        crate::trace::reset_thread_coverage();
        gh.set_read_after_write_lag(1);
        assert_eq!(
            gh.send(req("GET", "/repos/acme/app/issues/1", None))
                .unwrap()
                .status,
            200
        );
        let cov_no_write = crate::trace::take_thread_coverage();
        assert_eq!(cov_no_write.count("github.read_after_write_lag_served"), 0);

        // After an actual write, the next GET serves the pre-write snapshot and records the hit.
        gh.send(req(
            "PATCH",
            "/repos/acme/app/issues/1",
            Some(json!({ "title": "After Write" })),
        ))
        .unwrap();
        let stale: Value = serde_json::from_slice(
            &gh.send(req("GET", "/repos/acme/app/issues/1", None))
                .unwrap()
                .body,
        )
        .unwrap();
        assert_eq!(stale["title"], json!("Item #1"));
        let cov_after_write = crate::trace::take_thread_coverage();
        assert_eq!(
            cov_after_write.count("github.read_after_write_lag_served"),
            1
        );

        // Global rate-limit budget: 1 allowed, then 403 until virtual clock reaches reset_secs.
        let reset_secs = 1_700_000_005;
        gh.set_rate_limit_budget(Some(1), reset_secs);
        assert_eq!(gh.send(req("GET", "/user", None)).unwrap().status, 200);
        let limited = gh.send(req("GET", "/user", None)).unwrap();
        assert_eq!(limited.status, 403);
        assert_eq!(limited.header("x-ratelimit-remaining"), Some("0"));
        assert_eq!(limited.header("x-ratelimit-reset"), Some("1700000005"));

        // Advance virtual clock past reset_secs -> budget auto-clears and requests succeed again.
        clock.0.store(reset_secs * 1000, Ordering::SeqCst);
        assert_eq!(gh.send(req("GET", "/user", None)).unwrap().status, 200);
        assert_eq!(gh.send(req("GET", "/user", None)).unwrap().status, 200);

        // Per-host rate-limit budget also auto-replenishes once virtual clock >= reset_secs.
        let host_a = gh.for_host("remote-a");
        let host_reset = reset_secs + 10;
        gh.set_host_rate_limit_budget("remote-a", Some(1), host_reset);
        assert_eq!(host_a.send(req("GET", "/user", None)).unwrap().status, 200);
        assert_eq!(host_a.send(req("GET", "/user", None)).unwrap().status, 403);
        clock.0.store(host_reset * 1000, Ordering::SeqCst);
        assert_eq!(host_a.send(req("GET", "/user", None)).unwrap().status, 200);
        assert_eq!(host_a.send(req("GET", "/user", None)).unwrap().status, 200);
    }

    #[test]
    fn branch_head_remote_merge_and_targeted_fault_injection() {
        let gh = SimGitHub::new();
        gh.add_repo("acme/app", 77, "main");
        let pushed_sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        gh.set_branch_head("acme/app", "feat/pr-1", pushed_sha);

        // POST /pulls without explicit head_sha resolves pushed branch head SHA.
        let created = gh
            .send(req(
                "POST",
                "/repos/acme/app/pulls",
                Some(json!({
                    "head": "acme:feat/pr-1",
                    "base": "main",
                    "title": "PR with pushed SHA",
                    "body": "body",
                })),
            ))
            .unwrap();
        assert_eq!(created.status, 201);
        let pr: Value = serde_json::from_slice(&created.body).unwrap();
        assert_eq!(pr["head"]["sha"], json!(pushed_sha));
        let num = pr["number"].as_u64().unwrap();

        // Targeted fault triggers ONLY on matching method + path, leaving GETs untouched.
        gh.inject_fault_on(
            "PUT",
            "/merge",
            GitHubFault::Http {
                status: 409,
                headers: Vec::new(),
                body: Some(r#"{"message":"conflict"}"#.into()),
            },
        );
        assert_eq!(
            gh.send(req("GET", &format!("/repos/acme/app/pulls/{num}"), None))
                .unwrap()
                .status,
            200
        );
        assert_eq!(
            gh.send(req(
                "PUT",
                &format!("/repos/acme/app/pulls/{num}/merge"),
                Some(json!({ "sha": pushed_sha })),
            ))
            .unwrap()
            .status,
            409
        );

        // Out-of-band remote merge transitions PR to merged+closed with merge_commit_sha.
        let merge_sha = gh.merge_pr_remote("acme/app", num).unwrap();
        let after: Value = serde_json::from_slice(
            &gh.send(req("GET", &format!("/repos/acme/app/pulls/{num}"), None))
                .unwrap()
                .body,
        )
        .unwrap();
        assert_eq!(after["merged"], json!(true));
        assert_eq!(after["state"], json!("closed"));
        assert_eq!(after["merge_commit_sha"], json!(merge_sha));
    }
}
