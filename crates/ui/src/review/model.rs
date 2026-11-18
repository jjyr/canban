use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::sync::Arc;

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

#[derive(Clone, Copy)]
pub struct DiffViewport {
    pub scroll: ElementRef,
    pub content: ElementRef,
}

#[derive(Clone, Copy)]
pub struct State {
    pub sidebar_open: Signal<bool>,
    pub compare_open: Signal<bool>,
    pub collapsed_files: Signal<std::collections::BTreeSet<String>>,
    pub viewport: Stored<Option<DiffViewport>>,
    pub file_targets: Stored<std::collections::BTreeMap<String, ElementRef>>,
    pub card_drawer_open: Signal<bool>,
    pub commits_open: Signal<bool>,
    /// "git" (ad-hoc Git comparison), "card_local" (Card worktree/branch), or "github_pr" (remote PR)
    pub mode: Signal<String>,
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
    pub log_ticket: Signal<u64>,
    /// Saved `(mode, base_ref, compare_ref)` when drilling into a commit from `Commits in range`.
    pub range_backup: Signal<Option<(String, String, String)>>,
    pub commit_parent_idx: Signal<usize>,
    pub commit_summary: Signal<String>,
    pub commit_description: Signal<String>,
    pub commit_amend: Signal<bool>,
    pub discard_confirm: Signal<Option<Vec<String>>>,
    pub loaded_draft_key: Stored<(String, u64)>,
    pub draft_anchor_sha: Signal<Option<String>>,
    pub draft_drift_dismissed: Signal<bool>,
    pub agent_review_open: Signal<bool>,
    pub agent_review_profile: Signal<String>,
    pub agent_review_focus: Signal<String>,
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
            viewport: stored(None),
            file_targets: stored(Default::default()),
            card_drawer_open: signal(false),
            commits_open: signal(false),
            mode: signal("git".into()),
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
            log_ticket: signal(0),
            range_backup: signal(None),
            commit_parent_idx: signal(0),
            commit_summary: signal(String::new()),
            commit_description: signal(String::new()),
            commit_amend: signal(false),
            discard_confirm: signal(None),
            loaded_draft_key: stored((String::new(), 0)),
            draft_anchor_sha: signal(None),
            draft_drift_dismissed: signal(false),
            agent_review_open: signal(false),
            agent_review_profile: signal(String::new()),
            agent_review_focus: signal(String::new()),
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

pub fn parse_unified_hunk_lines(patch: &str) -> Vec<DiffLine> {
    let mut out = Vec::new();
    let mut old_ln = 0u64;
    let mut new_ln = 0u64;
    for raw in patch.lines() {
        if raw.starts_with("diff --git ")
            || raw.starts_with("index ")
            || raw.starts_with("--- ")
            || raw.starts_with("+++ ")
            || raw.starts_with("new file mode ")
            || raw.starts_with("deleted file mode ")
            || raw.starts_with("similarity index ")
            || raw.starts_with("rename from ")
            || raw.starts_with("rename to ")
        {
            continue;
        }
        if let Some(after) = raw.strip_prefix("@@") {
            if let Some((header, _)) = after.split_once("@@") {
                for part in header.split_whitespace() {
                    if let Some(old_spec) = part.strip_prefix('-') {
                        let start = old_spec.split(',').next().unwrap_or("1");
                        old_ln = start.parse::<u64>().unwrap_or(1);
                    } else if let Some(new_spec) = part.strip_prefix('+') {
                        let start = new_spec.split(',').next().unwrap_or("1");
                        new_ln = start.parse::<u64>().unwrap_or(1);
                    }
                }
            }
            out.push(DiffLine {
                kind: '@',
                old_line: None,
                new_line: None,
                text: raw.to_owned(),
            });
            continue;
        }
        if raw.starts_with('\\') {
            out.push(DiffLine {
                kind: '\\',
                old_line: None,
                new_line: None,
                text: raw.to_owned(),
            });
            continue;
        }
        if let Some(added) = raw.strip_prefix('+') {
            let ln = new_ln.max(1);
            out.push(DiffLine {
                kind: '+',
                old_line: None,
                new_line: Some(ln),
                text: added.to_owned(),
            });
            new_ln = ln + 1;
        } else if let Some(removed) = raw.strip_prefix('-') {
            let ln = old_ln.max(1);
            out.push(DiffLine {
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
            out.push(DiffLine {
                kind: ' ',
                old_line: Some(o),
                new_line: Some(n),
                text: content.to_owned(),
            });
            old_ln = o + 1;
            new_ln = n + 1;
        }
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
