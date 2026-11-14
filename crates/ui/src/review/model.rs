use nsh_core::prelude::*;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingInlineComment {
    pub id: String,
    pub path: String,
    pub line: u64,
    pub side: String, // "LEFT" | "RIGHT"
    pub snippet: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineTarget {
    pub path: String,
    pub line: u64,
    pub side: String,
    pub snippet: String,
}

#[derive(Clone, Copy)]
pub struct State {
    pub sidebar_open: Signal<bool>,
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
    pub refs_data: Signal<Value>,
    pub compare_data: Signal<Value>,
    pub refs_loading: Signal<bool>,
    pub compare_loading: Signal<bool>,
    pub error: Signal<String>,
    pub refs_ticket: Signal<u64>,
    pub compare_ticket: Signal<u64>,
    pub custom_ref_open: Signal<bool>,
    pub pending_comments: Signal<Vec<PendingInlineComment>>,
    pub active_line: Signal<Option<LineTarget>>,
    pub line_draft: Signal<String>,
    pub editing_remote_comment: Signal<Option<u64>>,
    pub remote_edit_draft: Signal<String>,
    pub remote_edit_error: Signal<String>,
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
            card_drawer_open: signal(false),
            commits_open: signal(false),
            mode: signal("git".into()),
            base_ref: signal(String::new()),
            compare_ref: signal(String::new()),
            three_dot: signal(true),
            pr_target: signal(None),
            selected_file: signal(String::new()),
            refs_data: signal(json!({})),
            compare_data: signal(json!({})),
            refs_loading: signal(false),
            compare_loading: signal(false),
            error: signal(String::new()),
            refs_ticket: signal(0),
            compare_ticket: signal(0),
            custom_ref_open: signal(false),
            pending_comments: signal(Vec::new()),
            active_line: signal(None),
            line_draft: signal(String::new()),
            editing_remote_comment: signal(None),
            remote_edit_draft: signal(String::new()),
            remote_edit_error: signal(String::new()),
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
            out.push_str(&format!(
                "\n{}. **`{}:{}`**{}\n",
                idx + 1,
                item.path,
                item.line,
                side_note
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
