use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::sync::Arc;

pub const MAX_SCOPED_REVIEW_DRAFTS: usize = 64;

/// Isolation key for in-progress diff review drafts (`pending_comments`, summary, verdict, anchor).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReviewDraftScope {
    /// Standalone Git compare/client workbench scoped by host, project, and physical checkout path.
    /// An empty `cwd` denotes the Project's main directory.
    Checkout {
        host: String,
        project_id: String,
        cwd: String,
    },
    /// Card-bound review (local card worktree or Card-linked GitHub PR).
    Card {
        host: String,
        project_id: String,
        card_id: String,
    },
    /// GitHub PR opened directly from the GitHub browser without an associated Card.
    StandalonePr { repository: String, number: u64 },
}

impl ReviewDraftScope {
    pub fn is_foreign_project(&self, host: &str, project_id: &str) -> bool {
        match self {
            Self::Checkout {
                host: h,
                project_id: p,
                ..
            }
            | Self::Card {
                host: h,
                project_id: p,
                ..
            } => h != host || p != project_id,
            Self::StandalonePr { .. } => false,
        }
    }

    pub fn matches_project(&self, host: &str, project_id: &str) -> bool {
        match self {
            Self::Checkout {
                host: h,
                project_id: p,
                ..
            }
            | Self::Card {
                host: h,
                project_id: p,
                ..
            } => h == host && p == project_id,
            Self::StandalonePr { .. } => false,
        }
    }

    pub fn matches_card(&self, card_id: &str) -> bool {
        matches!(self, Self::Card { card_id: cid, .. } if cid == card_id)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DraftScopeInput<'a> {
    pub host: &'a str,
    pub project_id: &'a str,
    pub project_dir: Option<&'a str>,
    pub mode: &'a str,
    pub review_card_id: &'a str,
    pub ui_card_id: &'a str,
    pub card_host_project: Option<(&'a str, &'a str)>,
    pub checkout_cwd: &'a str,
    pub pr_repository: Option<&'a str>,
    pub pr_number: Option<u64>,
}

fn trim_checkout_path(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed.len() > 1 {
        trimmed.trim_end_matches('/')
    } else {
        trimmed
    }
}

pub fn normalize_checkout_path(cwd: &str, project_dir: Option<&str>) -> String {
    let norm_cwd = trim_checkout_path(cwd);
    if norm_cwd.is_empty() {
        return String::new();
    }
    if let Some(dir) = project_dir
        .map(trim_checkout_path)
        .filter(|d| !d.is_empty())
        && norm_cwd == dir
    {
        return String::new();
    }
    norm_cwd.to_owned()
}

pub fn resolve_draft_scope(input: DraftScopeInput<'_>) -> ReviewDraftScope {
    let card_id = if !input.review_card_id.is_empty() {
        input.review_card_id
    } else if input.mode != "git" && !input.ui_card_id.is_empty() {
        input.ui_card_id
    } else {
        ""
    };
    if !card_id.is_empty() {
        let (h, p) = input
            .card_host_project
            .unwrap_or((input.host, input.project_id));
        return ReviewDraftScope::Card {
            host: h.to_owned(),
            project_id: p.to_owned(),
            card_id: card_id.to_owned(),
        };
    }
    if input.mode == "github_pr"
        && let (Some(repo), Some(num)) = (
            input.pr_repository.map(str::trim).filter(|s| !s.is_empty()),
            input.pr_number.filter(|&n| n > 0),
        )
    {
        return ReviewDraftScope::StandalonePr {
            repository: repo.to_owned(),
            number: num,
        };
    }
    ReviewDraftScope::Checkout {
        host: input.host.to_owned(),
        project_id: input.project_id.to_owned(),
        cwd: normalize_checkout_path(input.checkout_cwd, input.project_dir),
    }
}

pub fn draft_scope_from_read_context(
    read_context: &str,
    project_dir: Option<&str>,
) -> Option<ReviewDraftScope> {
    let parsed: Value = serde_json::from_str(read_context).ok()?;
    let arr = parsed.as_array()?;
    let host = arr.first()?.as_str()?;
    let project_id = arr.get(1)?.as_str()?;
    let kind = arr.get(2)?.as_str()?;
    let cwd = arr.get(3)?.as_str()?;
    if kind != "checkout" {
        return None;
    }
    Some(ReviewDraftScope::Checkout {
        host: host.to_owned(),
        project_id: project_id.to_owned(),
        cwd: normalize_checkout_path(cwd, project_dir),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewDraftSnapshot {
    pub pending_comments: Vec<PendingInlineComment>,
    pub submit_summary: String,
    pub submit_gh_event: String,
    pub draft_anchor_sha: Option<String>,
    pub draft_drift_dismissed: bool,
    pub loaded_draft_key: (String, u64),
}

impl Default for ReviewDraftSnapshot {
    fn default() -> Self {
        Self {
            pending_comments: Vec::new(),
            submit_summary: String::new(),
            submit_gh_event: "COMMENT".into(),
            draft_anchor_sha: None,
            draft_drift_dismissed: false,
            loaded_draft_key: (String::new(), 0),
        }
    }
}

impl ReviewDraftSnapshot {
    pub fn is_empty(&self) -> bool {
        self.pending_comments.is_empty()
            && self.submit_summary.trim().is_empty()
            && self.submit_gh_event == "COMMENT"
            && self.draft_anchor_sha.is_none()
            && !self.draft_drift_dismissed
            && self.loaded_draft_key.1 == 0
    }

    pub fn from_server_draft(draft: &canban_types::ReviewDraft) -> Self {
        let pending_comments = draft
            .comments
            .iter()
            .map(|c| PendingInlineComment {
                id: c.id.clone(),
                path: c.path.clone(),
                section: c.section.clone(),
                line: c.line,
                side: c.side.clone(),
                snippet: c.snippet.clone(),
                body: c.body.clone(),
                author: c.author.clone(),
                edited_by_user: c.edited_by_user,
            })
            .collect();
        let submit_gh_event = match draft.verdict.as_str() {
            "approve" => "APPROVE",
            "request_changes" => "REQUEST_CHANGES",
            _ => "COMMENT",
        }
        .to_owned();
        Self {
            pending_comments,
            submit_summary: draft.summary.clone(),
            submit_gh_event,
            draft_anchor_sha: draft.compare_sha.clone(),
            draft_drift_dismissed: false,
            loaded_draft_key: (draft.card_id.clone(), draft.updated_at),
        }
    }
}

/// Bounded per-scope storage for in-progress diff review drafts across Projects, worktrees, and Cards/PRs.
#[derive(Clone, Debug)]
pub struct ReviewDraftStore {
    entries: crate::cache::LruCache<ReviewDraftScope, ReviewDraftSnapshot>,
}

impl Default for ReviewDraftStore {
    fn default() -> Self {
        Self::new(MAX_SCOPED_REVIEW_DRAFTS)
    }
}

impl ReviewDraftStore {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: crate::cache::LruCache::new(capacity),
        }
    }

    pub fn save(&mut self, scope: ReviewDraftScope, snapshot: ReviewDraftSnapshot) {
        if snapshot.is_empty() {
            self.entries.retain(|k, _| k != &scope);
        } else {
            self.entries.insert(scope, snapshot);
        }
    }

    pub fn load(&mut self, scope: &ReviewDraftScope) -> ReviewDraftSnapshot {
        self.entries.get(scope).cloned().unwrap_or_default()
    }

    pub fn peek(&self, scope: &ReviewDraftScope) -> Option<&ReviewDraftSnapshot> {
        self.entries.peek(scope)
    }

    pub fn pending_count(&self, scope: &ReviewDraftScope) -> usize {
        self.entries
            .peek(scope)
            .map_or(0, |d| d.pending_comments.len())
    }

    pub fn clear_scope(&mut self, scope: &ReviewDraftScope) {
        self.entries.retain(|k, _| k != scope);
    }

    pub fn clear_card(&mut self, card_id: &str) {
        self.entries.retain(|k, _| !k.matches_card(card_id));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HydrateAction {
    Unchanged,
    Apply(ReviewDraftSnapshot),
    ClearAnchorOnly,
}

pub fn plan_server_draft_hydration(
    active_scope: Option<&ReviewDraftScope>,
    current_loaded_key: &(String, u64),
    card_id: &str,
    server_draft: Option<&canban_types::ReviewDraft>,
    force: bool,
) -> HydrateAction {
    if let Some(draft) = server_draft {
        let key = (card_id.to_owned(), draft.updated_at);
        if !force && current_loaded_key == &key {
            return HydrateAction::Unchanged;
        }
        return HydrateAction::Apply(ReviewDraftSnapshot::from_server_draft(draft));
    }
    let server_draft_removed = current_loaded_key.0 == card_id && current_loaded_key.1 > 0;
    let foreign_scope = active_scope.is_some_and(|s| !s.matches_card(card_id));
    let foreign_loaded = !current_loaded_key.0.is_empty() && current_loaded_key.0 != card_id;
    if force || server_draft_removed {
        if server_draft_removed || foreign_scope || foreign_loaded {
            HydrateAction::Apply(ReviewDraftSnapshot {
                loaded_draft_key: (card_id.to_owned(), 0),
                ..Default::default()
            })
        } else {
            HydrateAction::ClearAnchorOnly
        }
    } else {
        HydrateAction::Unchanged
    }
}

/// Switches the active draft scope, saving any prior scope's draft and loading the target scope's
/// saved bucket. Returns `true` when an actual switch between two distinct scopes occurred.
pub fn switch_draft_scope_pure(
    active_scope: &mut Option<ReviewDraftScope>,
    current: &mut ReviewDraftSnapshot,
    store: &mut ReviewDraftStore,
    inferred_prev: Option<ReviewDraftScope>,
    next: ReviewDraftScope,
) -> bool {
    if active_scope.as_ref() == Some(&next) {
        return false;
    }
    let prev = active_scope
        .take()
        .or_else(|| inferred_prev.filter(|s| s != &next));
    if let Some(old) = prev {
        store.save(old, std::mem::take(current));
        *current = store.load(&next);
        *active_scope = Some(next);
        true
    } else {
        if current.is_empty() {
            *current = store.load(&next);
        }
        *active_scope = Some(next);
        false
    }
}

pub fn rail_pending_count_pure(
    active_scope: Option<&ReviewDraftScope>,
    active_pending_len: usize,
    store: &ReviewDraftStore,
    current_host: &str,
    current_project_id: &str,
    current_page: &str,
) -> usize {
    if let Some(scope) = active_scope {
        let matches_active = match scope {
            ReviewDraftScope::Checkout { .. } | ReviewDraftScope::Card { .. } => {
                scope.matches_project(current_host, current_project_id)
            }
            ReviewDraftScope::StandalonePr { .. } => current_page == "review",
        };
        if matches_active {
            active_pending_len
        } else {
            store.pending_count(&ReviewDraftScope::Checkout {
                host: current_host.to_owned(),
                project_id: current_project_id.to_owned(),
                cwd: String::new(),
            })
        }
    } else {
        active_pending_len
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingInlineComment {
    pub id: String,
    pub path: String,
    pub section: String,
    pub line: u64,
    pub side: String, // "LEFT" | "RIGHT"
    pub snippet: String,
    pub body: String,
    pub author: Option<String>,
    pub edited_by_user: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineTarget {
    pub path: String,
    pub section: String,
    pub line: u64,
    pub side: String,
    pub snippet: String,
    pub editing_author: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GapExpansion {
    pub from_top: u64,
    pub from_bottom: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnchorEdge {
    HunkStart,
    HunkEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullRefreshTarget {
    Commits,
    Files,
    Diff,
}

#[derive(Clone, Copy)]
pub struct DiffViewport {
    pub scroll: ElementRef,
    pub content: ElementRef,
    pub bottom_space: Signal<f32>,
}

#[derive(Clone, Copy)]
pub struct State {
    pub sidebar_open: Signal<bool>,
    pub compare_open: Signal<bool>,
    pub collapsed_files: Signal<std::collections::BTreeSet<String>>,
    pub auto_collapsed_seen: Stored<std::collections::BTreeSet<String>>,
    pub viewport: Stored<Option<DiffViewport>>,
    pub file_targets: Stored<std::collections::BTreeMap<String, ElementRef>>,
    pub card_drawer_open: Signal<bool>,
    pub commits_open: Signal<bool>,
    /// "git" (ad-hoc Git comparison), "card_local" (Card worktree/branch), or "github_pr" (remote PR)
    pub mode: Signal<String>,
    pub review_card_id: Signal<String>,
    pub base_ref: Signal<String>,
    pub compare_ref: Signal<String>,
    pub three_dot: Signal<bool>,
    /// Optional GitHub PR target (`{ "repository": "owner/repo", "number": 42 }`)
    pub pr_target: Signal<Option<Value>>,
    pub selected_file: Signal<String>,
    pub summary_revision: Signal<u64>,
    pub pending_file: Stored<String>,
    pub read_cache: Stored<Arc<super::cache::ReadCache>>,
    pub read_context: Stored<String>,
    pub refs_request: Stored<String>,
    pub log_request: Stored<String>,
    pub compare_request: Stored<String>,
    pub prepared_lines: Signal<Arc<super::cache::PreparedLines>>,
    pub prepared_hunks: Signal<Arc<super::cache::PreparedHunks>>,
    pub visible_file_limit: Signal<usize>,
    pub refs_data: Signal<Value>,
    pub compare_data: Signal<Value>,
    pub refs_loading: Signal<bool>,
    pub compare_loading: Signal<bool>,
    pub refresh_pending: Signal<bool>,
    pub error: Signal<String>,
    pub refs_ticket: Signal<u64>,
    pub compare_ticket: Signal<u64>,
    pub pending_comments: Signal<Vec<PendingInlineComment>>,
    pub active_line: Signal<Option<LineTarget>>,
    pub line_draft: Signal<String>,
    pub editing_remote_comment: Signal<Option<u64>>,
    pub remote_edit_draft: Signal<String>,
    pub remote_edit_error: Signal<String>,
    pub deleting_remote_comment: Signal<Option<u64>>,
    pub remote_delete_error: Signal<String>,
    pub submit_open: Signal<bool>,
    pub submit_summary: Signal<String>,
    pub submit_action: Signal<String>,
    pub submit_profile: Signal<String>,
    pub submit_gh_event: Signal<String>,
    pub submit_move_develop: Signal<bool>,
    pub submit_create_worktree: Signal<bool>,
    pub submit_new_card_title: Signal<String>,
    pub delivery_customize: Signal<bool>,
    pub delivery_mode: Signal<String>,
    pub delivery_strategy: Signal<String>,
    /// Active checkout path override for Git client operations (empty = Project main directory).
    pub checkout_cwd: Signal<String>,
    /// Selected branch or ref in the left sidebar `Commits` explorer.
    pub log_branch: Signal<String>,
    pub log_query: Signal<String>,
    pub card_query: Signal<String>,
    pub log_commits: Signal<Vec<Value>>,
    pub log_has_more: Signal<bool>,
    pub log_loading: Signal<bool>,
    pub log_refreshing: Signal<bool>,
    pub files_refreshing: Signal<bool>,
    pub diff_refreshing: Signal<bool>,
    pub log_refresh_hold_ready: Signal<bool>,
    pub files_refresh_hold_ready: Signal<bool>,
    pub diff_refresh_hold_ready: Signal<bool>,
    pub log_refresh_ticket: Signal<u64>,
    pub files_refresh_ticket: Signal<u64>,
    pub diff_refresh_ticket: Signal<u64>,
    pub log_ticket: Signal<u64>,
    /// Saved `(mode, base_ref, compare_ref)` when drilling into a commit from `Commits in range`.
    pub range_backup: Signal<Option<(String, String, String)>>,
    pub commit_parent_idx: Signal<usize>,
    pub commit_summary: Signal<String>,
    pub commit_description: Signal<String>,
    pub commit_amend: Signal<bool>,
    pub discard_confirm: Signal<Option<Vec<String>>>,
    pub active_draft_scope: Signal<Option<ReviewDraftScope>>,
    pub draft_store: Stored<ReviewDraftStore>,
    pub draft_store_revision: Signal<u64>,
    pub loaded_draft_key: Stored<(String, u64)>,
    pub draft_anchor_sha: Signal<Option<String>>,
    pub draft_drift_dismissed: Signal<bool>,
    pub expanded_gaps: Signal<std::collections::BTreeMap<(String, usize), GapExpansion>>,
    pub file_lines_cache: Stored<super::cache::FileLinesCache>,
    pub file_lines_loading: Signal<std::collections::BTreeSet<String>>,
    pub file_lines_errors: Signal<std::collections::BTreeMap<String, String>>,
    pub file_lines_revision: Signal<u64>,
    pub anchor_targets: Stored<std::collections::BTreeMap<(String, usize, AnchorEdge), ElementRef>>,
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub fn new() -> Self {
        Self {
            sidebar_open: signal(true),
            compare_open: signal(false),
            collapsed_files: signal(Default::default()),
            auto_collapsed_seen: stored(Default::default()),
            viewport: stored(None),
            file_targets: stored(Default::default()),
            card_drawer_open: signal(false),
            commits_open: signal(false),
            mode: signal("git".into()),
            review_card_id: signal(String::new()),
            base_ref: signal(String::new()),
            compare_ref: signal(String::new()),
            three_dot: signal(true),
            pr_target: signal(None),
            selected_file: signal(String::new()),
            summary_revision: signal(0),
            pending_file: stored(String::new()),
            read_cache: stored(Arc::new(super::cache::ReadCache::default())),
            read_context: stored(String::new()),
            refs_request: stored(String::new()),
            log_request: stored(String::new()),
            compare_request: stored(String::new()),
            prepared_lines: signal(Arc::default()),
            prepared_hunks: signal(Arc::default()),
            visible_file_limit: signal(4),
            refs_data: signal(json!({})),
            compare_data: signal(json!({})),
            refs_loading: signal(false),
            compare_loading: signal(false),
            refresh_pending: signal(false),
            error: signal(String::new()),
            refs_ticket: signal(0),
            compare_ticket: signal(0),
            pending_comments: signal(Vec::new()),
            active_line: signal(None),
            line_draft: signal(String::new()),
            editing_remote_comment: signal(None),
            remote_edit_draft: signal(String::new()),
            remote_edit_error: signal(String::new()),
            deleting_remote_comment: signal(None),
            remote_delete_error: signal(String::new()),
            submit_open: signal(false),
            submit_summary: signal(String::new()),
            submit_action: signal("terminal".into()),
            submit_profile: signal(String::new()),
            submit_gh_event: signal("COMMENT".into()),
            submit_move_develop: signal(true),
            submit_create_worktree: signal(false),
            submit_new_card_title: signal(String::new()),
            delivery_customize: signal(false),
            delivery_mode: signal("local".into()),
            delivery_strategy: signal("rebase".into()),
            checkout_cwd: signal(String::new()),
            log_branch: signal(String::new()),
            log_query: signal(String::new()),
            card_query: signal(String::new()),
            log_commits: signal(Vec::new()),
            log_has_more: signal(false),
            log_loading: signal(false),
            log_refreshing: signal(false),
            files_refreshing: signal(false),
            diff_refreshing: signal(false),
            log_refresh_hold_ready: signal(false),
            files_refresh_hold_ready: signal(false),
            diff_refresh_hold_ready: signal(false),
            log_refresh_ticket: signal(0),
            files_refresh_ticket: signal(0),
            diff_refresh_ticket: signal(0),
            log_ticket: signal(0),
            range_backup: signal(None),
            commit_parent_idx: signal(0),
            commit_summary: signal(String::new()),
            commit_description: signal(String::new()),
            commit_amend: signal(false),
            discard_confirm: signal(None),
            active_draft_scope: signal(None),
            draft_store: stored(ReviewDraftStore::default()),
            draft_store_revision: signal(0),
            loaded_draft_key: stored((String::new(), 0)),
            draft_anchor_sha: signal(None),
            draft_drift_dismissed: signal(false),
            expanded_gaps: signal(Default::default()),
            file_lines_cache: stored(super::cache::FileLinesCache::default()),
            file_lines_loading: signal(Default::default()),
            file_lines_errors: signal(Default::default()),
            file_lines_revision: signal(0),
            anchor_targets: stored(Default::default()),
        }
    }

    pub fn pull_slot(self, target: PullRefreshTarget) -> (Signal<bool>, Signal<bool>, Signal<u64>) {
        match target {
            PullRefreshTarget::Commits => (
                self.log_refreshing,
                self.log_refresh_hold_ready,
                self.log_refresh_ticket,
            ),
            PullRefreshTarget::Files => (
                self.files_refreshing,
                self.files_refresh_hold_ready,
                self.files_refresh_ticket,
            ),
            PullRefreshTarget::Diff => (
                self.diff_refreshing,
                self.diff_refresh_hold_ready,
                self.diff_refresh_ticket,
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: char, // '@', '+', '-', ' ', '\\'
    pub old_line: Option<u64>,
    pub new_line: Option<u64>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub old_start: u64,
    pub old_count: u64,
    pub new_start: u64,
    pub new_count: u64,
    pub context: String,
    pub lines: Vec<DiffLine>,
}

impl DiffHunk {
    pub fn entry_old(&self) -> u64 {
        if self.old_count > 0 {
            self.old_start
        } else {
            self.old_start + 1
        }
    }

    pub fn entry_new(&self) -> u64 {
        if self.new_count > 0 {
            self.new_start
        } else {
            self.new_start + 1
        }
    }

    pub fn exit_old(&self) -> u64 {
        if self.old_count > 0 {
            self.old_start + self.old_count
        } else {
            self.old_start + 1
        }
    }

    pub fn exit_new(&self) -> u64 {
        if self.new_count > 0 {
            self.new_start + self.new_count
        } else {
            self.new_start + 1
        }
    }

    pub fn ends_with_no_newline(&self) -> bool {
        self.lines.last().is_some_and(|l| l.kind == '\\')
    }

    pub fn formatted_header(&self, expanded_above: u64) -> String {
        if expanded_above == 0 {
            return self.header.clone();
        }
        let adj_old_start = self.entry_old().saturating_sub(expanded_above);
        let adj_old_count = self.old_count + expanded_above;
        let adj_new_start = self.entry_new().saturating_sub(expanded_above);
        let adj_new_count = self.new_count + expanded_above;
        if self.context.is_empty() {
            format!("@@ -{adj_old_start},{adj_old_count} +{adj_new_start},{adj_new_count} @@")
        } else {
            format!(
                "@@ -{adj_old_start},{adj_old_count} +{adj_new_start},{adj_new_count} @@ {}",
                self.context
            )
        }
    }
}

fn parse_hunk_range(spec: &str) -> (u64, u64) {
    if let Some((start, count)) = spec.split_once(',') {
        (start.parse().unwrap_or(1), count.parse().unwrap_or(1))
    } else {
        (spec.parse().unwrap_or(1), 1)
    }
}

fn parse_hunk_header(raw: &str) -> Option<(u64, u64, u64, u64, String)> {
    let after = raw.strip_prefix("@@")?;
    let (header, context) = after.split_once("@@")?;
    let mut old_start = 1u64;
    let mut old_count = 1u64;
    let mut new_start = 1u64;
    let mut new_count = 1u64;
    for part in header.split_whitespace() {
        if let Some(old_spec) = part.strip_prefix('-') {
            (old_start, old_count) = parse_hunk_range(old_spec);
        } else if let Some(new_spec) = part.strip_prefix('+') {
            (new_start, new_count) = parse_hunk_range(new_spec);
        }
    }
    Some((
        old_start,
        old_count,
        new_start,
        new_count,
        context.trim().to_owned(),
    ))
}

fn is_diff_metadata_line(raw: &str) -> bool {
    raw.starts_with("diff --git ")
        || raw.starts_with("index ")
        || raw.starts_with("--- ")
        || raw.starts_with("+++ ")
        || raw.starts_with("new file mode ")
        || raw.starts_with("deleted file mode ")
        || raw.starts_with("similarity index ")
        || raw.starts_with("rename from ")
        || raw.starts_with("rename to ")
        || raw.starts_with("old mode ")
        || raw.starts_with("new mode ")
}

pub fn parse_unified_hunks(patch: &str) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    let mut current: Option<DiffHunk> = None;
    let mut old_ln = 0u64;
    let mut new_ln = 0u64;

    for raw in patch.lines() {
        if is_diff_metadata_line(raw) {
            continue;
        }
        if let Some((os, oc, ns, nc, ctx)) = parse_hunk_header(raw) {
            if let Some(prev) = current.take() {
                hunks.push(prev);
            }
            old_ln = os;
            new_ln = ns;
            current = Some(DiffHunk {
                header: raw.to_owned(),
                old_start: os,
                old_count: oc,
                new_start: ns,
                new_count: nc,
                context: ctx,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            continue;
        };
        if raw.starts_with('\\') {
            hunk.lines.push(DiffLine {
                kind: '\\',
                old_line: None,
                new_line: None,
                text: raw.to_owned(),
            });
        } else if let Some(added) = raw.strip_prefix('+') {
            let ln = new_ln.max(1);
            hunk.lines.push(DiffLine {
                kind: '+',
                old_line: None,
                new_line: Some(ln),
                text: added.to_owned(),
            });
            new_ln = ln + 1;
        } else if let Some(removed) = raw.strip_prefix('-') {
            let ln = old_ln.max(1);
            hunk.lines.push(DiffLine {
                kind: '-',
                old_line: Some(ln),
                new_line: None,
                text: removed.to_owned(),
            });
            old_ln = ln + 1;
        } else {
            let content = raw.strip_prefix(' ').unwrap_or(raw);
            let o = old_ln.max(1);
            let n = new_ln.max(1);
            hunk.lines.push(DiffLine {
                kind: ' ',
                old_line: Some(o),
                new_line: Some(n),
                text: content.to_owned(),
            });
            old_ln = o + 1;
            new_ln = n + 1;
        }
    }
    if let Some(prev) = current.take() {
        hunks.push(prev);
    }
    hunks
}

pub fn parse_unified_hunk_lines(patch: &str) -> Vec<DiffLine> {
    let mut out = Vec::new();
    for hunk in parse_unified_hunks(patch) {
        out.push(DiffLine {
            kind: '@',
            old_line: None,
            new_line: None,
            text: hunk.header,
        });
        out.extend(hunk.lines);
    }
    out
}

pub fn format_pending_markdown(
    context_label: &str,
    summary: &str,
    comments: &[PendingInlineComment],
) -> String {
    let mut out = format!("### Code Review ({context_label})\n");
    if !summary.trim().is_empty() {
        out.push('\n');
        out.push_str(summary.trim());
        out.push('\n');
    }
    if !comments.is_empty() {
        out.push_str("\n#### Inline Comments\n");
        for (idx, item) in comments.iter().enumerate() {
            let side_note = if item.side == "LEFT" { " (old)" } else { "" };
            let source_note = if item.section.is_empty() {
                String::new()
            } else {
                format!(" ({})", item.section)
            };
            out.push_str(&format!(
                "\n{}. **`{}:{}`**{side_note}{source_note}\n",
                idx + 1,
                item.path,
                item.line
            ));
            if !item.snippet.trim().is_empty() {
                out.push_str(&format!("   > `{}`\n", item.snippet.trim()));
            }
            if !item.body.trim().is_empty() {
                for line in item.body.trim().lines() {
                    out.push_str(&format!("   {line}\n"));
                }
            }
        }
    }
    out
}

/// The same path may have independent index and working-directory patches.
pub fn file_key(file: &Value) -> String {
    diff_key(
        file["section"].as_str().unwrap_or(""),
        file["filename"].as_str().unwrap_or(""),
    )
}
pub fn diff_key(section: &str, path: &str) -> String {
    if section.is_empty() {
        path.to_owned()
    } else {
        format!("{section}\0{path}")
    }
}

/// Extreme diffs above this line count start collapsed by default (GitHub-style),
/// while expanding renders the complete patch without mid-file truncation.
pub const LARGE_DIFF_AUTO_FOLD_LINES: usize = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoFoldReason {
    Generated,
    LargeDiff(usize),
}

pub fn is_generated_or_lock_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let base = lower.rsplit('/').next().unwrap_or(&lower);
    if matches!(
        base,
        "cargo.lock"
            | "package-lock.json"
            | "npm-shrinkwrap.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
            | "gemfile.lock"
            | "poetry.lock"
            | "uv.lock"
            | "pipfile.lock"
            | "composer.lock"
            | "go.sum"
            | "podfile.lock"
            | "packages.lock.json"
            | "flake.lock"
            | "mix.lock"
            | "pubspec.lock"
            | "gradle.lockfile"
    ) {
        return true;
    }
    lower.ends_with(".min.js")
        || lower.ends_with(".min.mjs")
        || lower.ends_with(".min.css")
        || lower.ends_with(".js.map")
        || lower.ends_with(".css.map")
        || lower.ends_with(".pb.go")
        || lower.ends_with(".pb.rs")
        || lower.ends_with(".g.dart")
        || lower.ends_with(".freezed.dart")
        || lower.ends_with(".snap")
}

pub fn auto_fold_reason(file: &Value, hunks: Option<&[DiffHunk]>) -> Option<AutoFoldReason> {
    let path = file["filename"].as_str().unwrap_or("");
    if is_generated_or_lock_path(path) {
        return Some(AutoFoldReason::Generated);
    }
    let line_count = if let Some(hs) = hunks {
        hs.iter().map(|h| h.lines.len() + 1).sum()
    } else {
        let patch = file["patch"].as_str().unwrap_or("");
        let stat_sum = (file["additions"].as_u64().unwrap_or(0)
            + file["deletions"].as_u64().unwrap_or(0)) as usize;
        if stat_sum > LARGE_DIFF_AUTO_FOLD_LINES {
            stat_sum
        } else {
            patch.bytes().filter(|&b| b == b'\n').count() + usize::from(!patch.is_empty())
        }
    };
    (line_count > LARGE_DIFF_AUTO_FOLD_LINES).then_some(AutoFoldReason::LargeDiff(line_count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unified_hunks_handles_multiple_hunks_and_dynamic_headers() {
        let patch = concat!(
            "@@ -10,3 +10,4 @@ fn top_fn() {\n",
            " ctx1\n",
            "+added\n",
            " ctx2\n",
            " ctx3\n",
            "@@ -50,2 +51,2 @@ fn second_fn() {\n",
            "-old\n",
            "+new\n",
            " tail\n",
        );
        let hunks = parse_unified_hunks(patch);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].entry_old(), 10);
        assert_eq!(hunks[0].entry_new(), 10);
        assert_eq!(hunks[0].exit_old(), 13);
        assert_eq!(hunks[0].exit_new(), 14);
        assert_eq!(hunks[1].entry_old(), 50);
        assert_eq!(hunks[1].entry_new(), 51);
        assert_eq!(
            hunks[1].formatted_header(20),
            "@@ -30,22 +31,22 @@ fn second_fn() {"
        );
        // Gap between hunk 0 and hunk 1 has identical old/new length (37 lines).
        assert_eq!(
            hunks[1].entry_old() - hunks[0].exit_old(),
            hunks[1].entry_new() - hunks[0].exit_new()
        );

        let pure_insert = parse_unified_hunks("@@ -10,0 +11,2 @@\n+a\n+b");
        assert_eq!(pure_insert[0].entry_old(), 11);
        assert_eq!(pure_insert[0].entry_new(), 11);
        assert_eq!(pure_insert[0].exit_old(), 11);
        assert_eq!(pure_insert[0].exit_new(), 13);

        let single_line = parse_unified_hunks("@@ -1 +1 @@\n-a\n+b");
        assert_eq!(single_line[0].old_count, 1);
        assert_eq!(single_line[0].new_count, 1);
        assert_eq!(single_line[0].exit_old(), 2);
        assert_eq!(single_line[0].exit_new(), 2);
    }

    #[test]
    fn auto_fold_detects_lockfiles_generated_assets_and_extreme_diffs() {
        assert_eq!(
            auto_fold_reason(
                &json!({"filename": "Cargo.lock", "patch": "@@ -1 +1 @@\n+a"}),
                None
            ),
            Some(AutoFoldReason::Generated)
        );
        assert_eq!(
            auto_fold_reason(
                &json!({"filename": "web/dist/app.min.js", "patch": "@@ -1 +1 @@\n+a"}),
                None
            ),
            Some(AutoFoldReason::Generated)
        );
        assert_eq!(
            auto_fold_reason(
                &json!({"filename": "src/feature.rs", "additions": 1200, "deletions": 0, "patch": "@@ -0,0 +1,2 @@\n+a\n+b"}),
                None
            ),
            None
        );
        assert!(matches!(
            auto_fold_reason(
                &json!({"filename": "src/huge.rs", "additions": 5001, "deletions": 0, "patch": "@@ -0,0 +1,2 @@\n+a\n+b"}),
                None
            ),
            Some(AutoFoldReason::LargeDiff(_))
        ));
    }
}
