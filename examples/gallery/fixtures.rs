use canban_types::{CardEventSummary, Event, ReviewPreview, Snapshot};
use serde_json::json;

pub fn apply_fixture_events(s: &mut Snapshot, events: &[Event]) {
    let mut by_card: std::collections::BTreeMap<String, Vec<&Event>> =
        std::collections::BTreeMap::new();
    for ev in events {
        if ev.card_id.is_empty() {
            continue;
        }
        by_card.entry(ev.card_id.clone()).or_default().push(ev);
    }
    for (cid, evs) in by_card {
        let last_event_seq = evs.iter().map(|e| e.sequence).max().unwrap_or(0);
        let reply_count = evs
            .iter()
            .filter(|e| {
                matches!(
                    e.kind.as_str(),
                    "comment.created"
                        | "comment.replied"
                        | "github.comment"
                        | "github.pr.commented"
                        | "execution.reported"
                )
            })
            .count() as u32;
        s.card_summaries.insert(
            cid,
            CardEventSummary {
                last_event_seq,
                reply_count,
            },
        );
    }
    s.reminders = canban_notifications::reminders_from_events(s, events);
    s.sound_event_ids = canban_notifications::sound_event_ids_from_events(s, events);
}

pub fn seed_ui_timelines(ui: canban_ui::presenter::Ui, events: &[Event]) {
    let mut by_card: std::collections::BTreeMap<String, Vec<Event>> =
        std::collections::BTreeMap::new();
    for ev in events {
        if ev.card_id.is_empty() {
            continue;
        }
        by_card
            .entry(ev.card_id.clone())
            .or_default()
            .push(ev.clone());
    }
    ui.card_timelines.update(|timelines| {
        for (cid, mut evs) in by_card {
            evs.sort_by_key(|e| e.sequence);
            let oldest_seq = evs.first().map(|e| e.sequence);
            let highwater = evs.last().map(|e| e.sequence).unwrap_or(0);
            timelines.insert(
                cid,
                canban_ui::presenter::CardTimelineState {
                    events: evs,
                    oldest_seq,
                    highwater,
                    has_more_before: false,
                    loading: false,
                    loading_older: false,
                    initialized: true,
                },
            );
        }
    });
}

pub fn timeline() -> Event {
    Event {id:"gallery-event".into(),project_id:"gallery-project".into(),card_id:"gallery-card".into(),sequence:1,kind:"comment.created".into(),actor:"user".into(),message:"A **shared component**, rendered from deterministic fixture data.\n\n中文输入、选择、复制与长文本。".into(),data:json!({}),created_at:canban_types::now()}
}

pub fn timeline_connected() -> [Event; 5] {
    let base = canban_types::now();
    let mk = |seq: u64, kind: &str, actor: &str, msg: &str, mins_ago: u64| Event {
        id: format!("gallery-tl-{seq}"),
        project_id: "gallery-project".into(),
        card_id: "gallery-card".into(),
        sequence: seq,
        kind: kind.into(),
        actor: actor.into(),
        message: msg.into(),
        data: json!({}),
        created_at: base.saturating_sub(mins_ago * 60_000),
    };
    [
        mk(
            1,
            "comment.created",
            "user",
            "Connect narrative replies and key milestones along one vertical spine.",
            45,
        ),
        mk(2, "card.stage_changed", "user", "backlog → develop", 40),
        mk(3, "review.merged", "system", "Auto-merged into main", 12),
        mk(
            4,
            "comment.replied",
            "agent:developer",
            "Shipped with collapsible intermediate status updates · 中文与长文本对齐正常。",
            5,
        ),
        mk(
            5,
            "comment.replied",
            "agent:reviewer",
            "Verified colored provider avatars across Codex, Antigravity, Claude, and Gemini.",
            2,
        ),
    ]
}

pub fn archive() -> canban_types::Snapshot {
    use canban_types::*;
    let mut s = workspace();
    s.cards[0].archived_at = Some(now().saturating_sub(86_400_000));
    s.cards[1].archived_at = Some(now().saturating_sub(3_600_000));
    s.cards[1].title = "原生终端恢复 · preserve session history".into();
    s.cards[1].attention = None;
    s.cards[1].human_gate = None;
    s.cards[1].delivery = Some(Delivery {
        target_branch: "main".into(),
        commit: "91ab162".into(),
        request_key: "gallery-merge".into(),
        merged_at: now().saturating_sub(3_600_000),
    });
    for (id, name) in [
        ("archive-tools", "Tools"),
        ("archive-empty", "No archived Cards"),
        ("archive-deleted", "Legacy removed only (hidden)"),
    ] {
        let mut project = s.projects[0].clone();
        project.id = id.into();
        project.name = name.into();
        s.projects.push(project);
    }
    let mut card = s.cards[0].clone();
    card.id = "archive-tools-card".into();
    card.project_id = "archive-tools".into();
    card.title = "Fix archive search and keyboard focus".into();
    card.number = 42;
    s.cards.push(card);
    let mut card = s.cards[2].clone();
    card.id = "archive-active-card".into();
    card.project_id = "archive-empty".into();
    s.cards.push(card);
    let mut card = s.cards[2].clone();
    card.id = "archive-deleted-card".into();
    card.project_id = "archive-deleted".into();
    card.removed_at = Some(now());
    // Retain legacy removed data to verify it never appears in Archive.
    card.archived_at = Some(now());
    card.title = "Legacy removed work (hidden)".into();
    s.cards.push(card);
    for (card, profile, provider) in [
        ("card-1", "developer", "codex"),
        ("card-2", "reviewer", "claude"),
        ("archive-tools-card", "developer", "codex"),
    ] {
        let project = s
            .cards
            .iter()
            .find(|c| c.id == card)
            .unwrap()
            .project_id
            .clone();
        s.conversations.push(Conversation {
            id: Conversation::key(card, profile),
            project_id: project,
            card_id: card.into(),
            profile_id: profile.into(),
            provider: provider.into(),
            provider_session_id: None,
            created_at: now(),
        });
    }
    s
}

pub fn review() -> ReviewPreview {
    ReviewPreview {
        card_id: "card-1".into(),
        revision: 1,
        target_branch: "main".into(),
        target_head: "0123456789".into(),
        card_head: "abcdef0123".into(),
        source_branch: "main".into(),
        worktree: "/example/Card".into(),
        files: vec!["src/example.rs".into(), "src/review.rs".into()],
        patch: "diff --git a/src/example.rs b/src/example.rs\n--- a/src/example.rs\n+++ b/src/example.rs\n@@ -1 +1,2 @@\n-// old\n+// Reviewed · 中文\n+pub fn ready() {}\ndiff --git a/src/review.rs b/src/review.rs\n--- a/src/review.rs\n+++ b/src/review.rs\n@@ -1 +1 @@\n-const MERGED: bool = false;\n+const MERGED: bool = true;".into(),
        truncated: false,
        dirty: true,
        conflicts: vec![],
        token: "gallery-token".into(),
        merge_allowed: true,
        reason: None,
        warning: None,
    }
}

pub fn git_refs() -> serde_json::Value {
    json!({
        "cwd": "/example/project",
        "current_branch": "feat/diff-workbench",
        "default_branch": "main",
        "head_sha": "c207c08a91fe001234567890abcdef0123456789",
        "upstream": "origin/feat/diff-workbench",
        "ahead": 2,
        "behind": 1,
        "has_remote": true,
        "remote_name": "origin",
        "sync_remote": "origin",
        "has_sync_remote": true,
        "dirty": true,
        "dirty_count": 3,
        "staged_count": 1,
        "unstaged_count": 2,
        "last_commit_subject": "ui: add unified Diff & Review workbench",
        "last_commit_body": "Supports standalone Git compare, branch commit log, and working tree commit controls.",
        "checkouts": [
            {
                "kind": "project",
                "label": "Canban workspace (main)",
                "path": "/example/project",
                "branch": "main"
            },
            {
                "kind": "card",
                "card_id": "card-1",
                "card_number": 1,
                "label": "#1 Improve search and preserve the user's context",
                "path": "/preview/worktrees/card-1",
                "branch": "canban/gallery/1"
            },
            {
                "kind": "worktree",
                "worktree_id": "wt-gallery-1",
                "label": "feat/diff-workbench (worktree)",
                "path": "/preview/worktrees/wt-diff",
                "branch": "feat/diff-workbench"
            }
        ],
        "status_files": [
            {
                "path": "crates/workspaces/src/git_client.rs",
                "orig_path": null,
                "index_code": "A",
                "worktree_code": " ",
                "staged": true,
                "unstaged": false,
                "untracked": false,
                "conflicted": false
            },
            {
                "path": "crates/ui/src/review/sidebar.rs",
                "orig_path": null,
                "index_code": " ",
                "worktree_code": "M",
                "staged": false,
                "unstaged": true,
                "untracked": false,
                "conflicted": false
            },
            {
                "path": "crates/ui/src/review/git_ops.rs",
                "orig_path": null,
                "index_code": "?",
                "worktree_code": "?",
                "staged": false,
                "unstaged": false,
                "untracked": true,
                "conflicted": false
            }
        ],
        "branches": [
            {
                "name": "feat/diff-workbench",
                "kind": "local",
                "sha": "c207c08a91fe001234567890abcdef0123456789",
                "short_sha": "c207c08",
                "relative_time": "2 minutes ago",
                "subject": "ui: add unified Diff & Review workbench",
                "is_current": true,
                "is_default": false,
                "ahead": 2,
                "behind": 1,
                "gone": false,
            },
            {
                "name": "main",
                "kind": "local",
                "sha": "1cfcce8111222333444555666777888999000aaa",
                "short_sha": "1cfcce8",
                "relative_time": "2 hours ago",
                "subject": "ui: apply borderless SectionGroup layout",
                "is_current": false,
                "is_default": true,
                "ahead": 0,
                "behind": 0,
                "gone": false,
            },
            {
                "name": "canban/gallery/1",
                "kind": "local",
                "sha": "abcdef0123456789abcdef0123456789abcdef01",
                "short_sha": "abcdef0",
                "relative_time": "yesterday",
                "subject": "feat(search): preserve user context",
                "is_current": false,
                "is_default": false,
                "ahead": 1,
                "behind": 0,
                "gone": false,
            },
            {
                "name": "origin/feat/remote-pr",
                "kind": "remote",
                "sha": "99887766554433221100ffeeddccbbaa99887766",
                "short_sha": "9988776",
                "relative_time": "3 days ago",
                "subject": "fix(github): cache PR review threads",
                "is_current": false,
                "is_default": false,
                "ahead": 0,
                "behind": 0,
                "gone": false,
            }
        ],
        "tags": [
            {
                "name": "v0.3.0",
                "short_sha": "d93a6a2",
                "relative_time": "5 days ago",
                "subject": "Canban v0.3.0",
            }
        ],
        "commits": [
            {
                "sha": "c207c08a91fe001234567890abcdef0123456789",
                "short_sha": "c207c08",
                "relative_time": "2m ago",
                "author": "JJy",
                "subject": "ui: add unified Diff & Review workbench",
            },
            {
                "sha": "1cfcce8111222333444555666777888999000aaa",
                "short_sha": "1cfcce8",
                "relative_time": "2h ago",
                "author": "JJy",
                "subject": "ui: apply borderless SectionGroup layout across Settings",
            },
            {
                "sha": "d93a6a2999888777666555444333222111000fff",
                "short_sha": "d93a6a2",
                "relative_time": "1d ago",
                "author": "JJy",
                "subject": "feat(github): unify auth UX around gh CLI",
            }
        ]
    })
}

pub fn git_commit_compare() -> serde_json::Value {
    json!({
        "base_ref": "c207c08~1",
        "compare_ref": "c207c08a91fe001234567890abcdef0123456789",
        "base_sha": "1cfcce8111222333444555666777888999000aaa",
        "compare_sha": "c207c08a91fe001234567890abcdef0123456789",
        "three_dot": false,
        "total_additions": 14,
        "total_deletions": 3,
        "truncated": false,
        "cwd": "/example/project",
        "commits": [
            {
                "sha": "c207c08a91fe001234567890abcdef0123456789",
                "short_sha": "c207c08",
                "relative_time": "2m ago",
                "author": "JJy",
                "subject": "Merge branch 'feat/diff-workbench' into main",
            }
        ],
        "commit_detail": {
            "sha": "c207c08a91fe001234567890abcdef0123456789",
            "short_sha": "c207c08",
            "author": "JJy",
            "email": "jjyruby@gmail.com",
            "date_iso": "2026-11-14T16:45:00+08:00",
            "relative_time": "2 minutes ago",
            "subject": "feat(review): add branch commit log, single-commit diff & Git sync",
            "body": "- Browse paginated commit history per branch with instant SHA/subject filtering\n- Inspect individual commits with full message body and merge-parent switching\n- Stage, unstage, commit/amend, fetch, fast-forward pull, and push directly in Diff & Review",
            "parent_index": 0,
            "parents": [
                {
                    "index": 0,
                    "sha": "1cfcce8111222333444555666777888999000aaa",
                    "short_sha": "1cfcce8",
                    "subject": "ui: apply borderless SectionGroup layout"
                },
                {
                    "index": 1,
                    "sha": "d93a6a2999888777666555444333222111000fff",
                    "short_sha": "d93a6a2",
                    "subject": "feat(github): unify auth UX around gh CLI"
                }
            ]
        },
        "files": git_compare()["files"].clone(),
    })
}

pub fn git_workdir_compare() -> serde_json::Value {
    let refs = git_refs();
    json!({
        "base_ref": "HEAD",
        "compare_ref": "UNCOMMITTED",
        "base_sha": "c207c08a91fe001234567890abcdef0123456789",
        "compare_sha": "c207c08a91fe001234567890abcdef0123456789",
        "three_dot": false,
        "total_additions": 26,
        "total_deletions": 4,
        "truncated": false,
        "cwd": "/example/project",
        "commits": [],
        "status_files": refs["status_files"].clone(),
        "files": [
            {
                "filename": "crates/workspaces/src/git_client.rs",
                "section": "staged",
                "status": "added",
                "additions": 15,
                "deletions": 0,
                "patch": "@@ -0,0 +1,6 @@\n+impl Ops {\n+    pub fn git_pull(&self, project: &Project, dir: &Path) -> Result<Value> {\n+        // Safe fast-forward only (`--ff-only`)\n+        self.checked(dir, &[\"pull\", \"--ff-only\"])\n+    }\n+}"
            },
            {
                "filename": "crates/ui/src/review/sidebar.rs",
                "section": "unstaged",
                "status": "modified",
                "additions": 8,
                "deletions": 4,
                "patch": "@@ -35,6 +35,10 @@ pub fn render_top_bar(ui: Ui) -> impl IntoElement {\n-    .child(project_picker)\n+    .child(project_picker)\n+    .child(git_ops::render_checkout_picker(ui))\n+    .child(git_ops::render_sync_controls(ui))"
            },
            {
                "filename": "crates/ui/src/review/git_ops.rs",
                "section": "unstaged",
                "status": "added",
                "additions": 3,
                "deletions": 0,
                "patch": "@@ -0,0 +1,3 @@\n+pub fn run_commit(ui: Ui, and_push: bool) {\n+    // Dispatches git.commit with optional stage_all & push\n+}"
            }
        ]
    })
}

pub fn git_file_list_showcase() -> (serde_json::Value, serde_json::Value) {
    let mut refs = git_refs();
    let long_path = "crates/ui/src/review/components/extremely_long_diff_card_header_filename_for_hover_tooltip_verification.rs";
    let status_files = json!([
        {
            "path": "crates/ui/src/review/new_helper.rs",
            "orig_path": null,
            "index_code": "A",
            "worktree_code": " ",
            "staged": true,
            "unstaged": false,
            "untracked": false,
            "conflicted": false
        },
        {
            "path": "crates/ui/src/review/mod.rs",
            "orig_path": null,
            "index_code": "M",
            "worktree_code": "M",
            "staged": true,
            "unstaged": true,
            "untracked": false,
            "conflicted": false
        },
        {
            "path": "crates/ui/src/review/renamed_placeholder.rs",
            "orig_path": "crates/ui/src/review/old_placeholder.rs",
            "index_code": "R",
            "worktree_code": " ",
            "staged": true,
            "unstaged": false,
            "untracked": false,
            "conflicted": false
        },
        {
            "path": "crates/kanban/src/mod.rs",
            "orig_path": null,
            "index_code": " ",
            "worktree_code": "M",
            "staged": false,
            "unstaged": true,
            "untracked": false,
            "conflicted": false
        },
        {
            "path": long_path,
            "orig_path": null,
            "index_code": "?",
            "worktree_code": "?",
            "staged": false,
            "unstaged": false,
            "untracked": true,
            "conflicted": false
        }
    ]);
    refs["dirty_count"] = json!(5);
    refs["staged_count"] = json!(3);
    refs["unstaged_count"] = json!(3);
    refs["status_files"] = status_files.clone();

    let compare = json!({
        "base_ref": "HEAD",
        "compare_ref": "UNCOMMITTED",
        "base_sha": "c207c08a91fe001234567890abcdef0123456789",
        "compare_sha": "c207c08a91fe001234567890abcdef0123456789",
        "three_dot": false,
        "total_additions": 187,
        "total_deletions": 52,
        "truncated": false,
        "cwd": "/example/project",
        "commits": [],
        "status_files": status_files,
        "files": [
            {
                "filename": "crates/ui/src/review/new_helper.rs",
                "section": "staged",
                "status": "added",
                "additions": 45,
                "deletions": 0,
                "patch": "@@ -0,0 +1,2 @@\n+pub fn helper() {}\n+pub fn helper_two() {}"
            },
            {
                "filename": "crates/ui/src/review/mod.rs",
                "section": "staged",
                "status": "modified",
                "additions": 18,
                "deletions": 6,
                "patch": "@@ -1,2 +1,2 @@\n-old staged\n+new staged"
            },
            {
                "filename": "crates/ui/src/review/renamed_placeholder.rs",
                "previous_filename": "crates/ui/src/review/old_placeholder.rs",
                "section": "staged",
                "status": "renamed",
                "additions": 0,
                "deletions": 0,
                "patch": ""
            },
            {
                "filename": "crates/ui/src/review/mod.rs",
                "section": "unstaged",
                "status": "modified",
                "additions": 4,
                "deletions": 2,
                "patch": "@@ -1,2 +1,2 @@\n-new staged\n+working tree edit"
            },
            {
                "filename": "crates/kanban/src/mod.rs",
                "section": "unstaged",
                "status": "modified",
                "additions": 0,
                "deletions": 14,
                "patch": "@@ -1,2 +1 @@\n-removed line"
            },
            {
                "filename": long_path,
                "section": "unstaged",
                "status": "added",
                "additions": 120,
                "deletions": 30,
                "patch": "@@ -1,2 +1,2 @@\n-legacy long line\n+updated long path line"
            }
        ]
    });
    (refs, compare)
}

pub fn git_compare() -> serde_json::Value {
    json!({
        "base_ref": "main",
        "compare_ref": "feat/diff-workbench",
        "base_sha": "1cfcce8111222333444555666777888999000aaa",
        "compare_sha": "c207c08a91fe001234567890abcdef0123456789",
        "three_dot": true,
        "total_additions": 14,
        "total_deletions": 3,
        "truncated": false,
        "cwd": "/example/project",
        "commits": [
            {
                "sha": "c207c08a91fe001234567890abcdef0123456789",
                "short_sha": "c207c08",
                "relative_time": "2m ago",
                "author": "JJy",
                "subject": "ui: add unified Diff & Review workbench",
            }
        ],
        "files": [
            {
                "filename": "crates/ui/src/rail.rs",
                "status": "modified",
                "additions": 9,
                "deletions": 2,
                "patch": "@@ -49,4 +49,11 @@ pub fn sidebar(ui: Ui) -> impl IntoElement {\n     let rail = rail.child(icon_action(\n         Icon::Terminal,\n         \"Terminals\".into(),\n-        move || ui.page.get() == \"terminals\",\n-        || 0,\n+        move || ui.page.get() == \"terminals\",\n+        || 0,\n         move |_| ui.navigate(\"terminals\"),\n+    ))\n+    .child(icon_action(\n+        Icon::Compare,\n+        \"Diff & Review\".into(),\n+        move || ui.page.get() == \"review\",\n+        move || ui.review_state.pending_comments.with(Vec::len),\n+        move |_| crate::review::open_compare(ui),\n     ));"
            },
            {
                "filename": "crates/workspaces/src/compare.rs",
                "status": "added",
                "additions": 5,
                "deletions": 1,
                "patch": "@@ -0,0 +1,5 @@\n+pub fn git_compare(project: &Project, base: &str, compare: &str) -> Result<Value> {\n+    // Supports arbitrary branch, commit, and Working tree comparison without a Card\n+    Ops::local().git_compare(project, base, compare, None, true)\n+}"
            }
        ]
    })
}

pub fn git_context_expand_compare() -> serde_json::Value {
    let mut lines: Vec<String> = (1..=140)
        .map(|n| format!("    // Context line {n:03}: preserves exact scroll anchor on expand"))
        .collect();
    lines[29] = "    let mode = ui.review_state.mode.get();".into();
    lines[30] = "    let enabled = mode == \"git\" && !ui.narrow.get();".into();
    lines[31] = "    let theme = ui.theme.get();".into();
    lines[41] = "    let summary = ui.review_state.submit_summary.get();".into();
    lines[42] = "    let ready = enabled && !summary.is_empty();".into();
    lines[43] = "    if ready { dispatch_review(ui); }".into();
    lines[104] = "    let viewport = DiffViewport { scroll, content };".into();
    lines[105] = "    st.viewport.replace(Some(viewport));".into();
    lines[106] = "    render_workbench_body(ui, viewport)".into();

    let patch = concat!(
        "@@ -30,3 +30,3 @@ fn top_section(ui: Ui) {\n",
        "     let mode = ui.review_state.mode.get();\n",
        "-    let enabled = mode == \"git\";\n",
        "+    let enabled = mode == \"git\" && !ui.narrow.get();\n",
        "     let theme = ui.theme.get();\n",
        "@@ -42,3 +42,3 @@ fn small_gap_section(ui: Ui) {\n",
        "     let summary = ui.review_state.submit_summary.get();\n",
        "-    let ready = enabled;\n",
        "+    let ready = enabled && !summary.is_empty();\n",
        "     if ready { dispatch_review(ui); }\n",
        "@@ -105,3 +105,3 @@ pub fn workbench(ui: Ui) {\n",
        "     let viewport = DiffViewport { scroll, content };\n",
        "-    st.viewport.replace(None);\n",
        "+    st.viewport.replace(Some(viewport));\n",
        "     render_workbench_body(ui, viewport)",
    );

    json!({
        "base_ref": "main",
        "compare_ref": "feat/diff-context-expand",
        "base_sha": "1cfcce8111222333444555666777888999000aaa",
        "compare_sha": "d409f12a91fe001234567890abcdef0123456789",
        "three_dot": true,
        "total_additions": 3,
        "total_deletions": 3,
        "truncated": false,
        "cwd": "/example/project",
        "commits": [],
        "files": [
            {
                "filename": "crates/ui/src/review/diff.rs",
                "status": "modified",
                "additions": 3,
                "deletions": 3,
                "patch": patch,
                "preview_lines": lines,
            }
        ]
    })
}

pub const TERMINAL: &str = concat!(
    "\x1b[1;38;2;110;210;255m╭─ Canban Native Terminal · libghostty-vt ──────────────────────╮\x1b[0m\r\n",
    "\x1b[1;38;2;110;210;255m│\x1b[0m Styles : \x1b[1mBold\x1b[0m · \x1b[3mItalic\x1b[0m · \x1b[2mFaint\x1b[0m · \x1b[9mStrike\x1b[0m · \x1b[4:3;58;2;255;105;97mCurly diagnostic\x1b[0m     \x1b[1;38;2;110;210;255m│\x1b[0m\r\n",
    "\x1b[1;38;2;110;210;255m│\x1b[0m CJK    : 世界你好 · café · Grid [中文对齐] · Blocks ░▒▓█▀▄▌▐  \x1b[1;38;2;110;210;255m│\x1b[0m\r\n",
    "\x1b[1;38;2;110;210;255m│\x1b[0m Braille: ⠁⠃⠇⠏⠟⠿⣿ · Links: https://canban.dev (\x1b]8;;https://ghostty.org\x07OSC-8 link\x1b]8;;\x07)     \x1b[1;38;2;110;210;255m│\x1b[0m\r\n",
    "\x1b[1;38;2;110;210;255m╰───────────────────────────────────────────────────────────────╯\x1b[0m\r\n",
    "\x1b[?2026h\x1b[32m❯\x1b[0m Select text, Cmd-hover links, or type 中文 smoothly in Terminal.\x1b[?2026l\r\n",
);

pub fn workspace() -> canban_types::Snapshot {
    use canban_types::*;
    let mut s = Snapshot::default();
    s.projects.push(Project {
        github: None,
        detected_github: Some(DetectedGitHub {
            repository: "jjyr/canban".into(),
            remote: "origin".into(),
        }),
        github_disconnected: false,
        delivery_mode: Default::default(),
        agent_overrides: std::collections::BTreeMap::from([(
            "developer".into(),
            ProjectAgentOverride {
                agent: Some("agy".into()),
                model: Some("gemini-4-carbon".into()),
                effort: Some("high".into()),
                orchestrator: None,
            },
        )]),
        tags: vec![
            "search".into(),
            "ux".into(),
            "agent".into(),
            "terminal".into(),
            "bug".into(),
        ],
        id: "gallery-project".into(),
        name: "Canban workspace".into(),
        directory: "/example/project".into(),
        workflow: Workflow::default(),
        target_branch: Some("main".into()),
        created_at: now(),
    });
    for (i, title, stage, attention) in [
        (
            1,
            "Improve search and preserve the user's context",
            "review",
            None,
        ),
        (
            2,
            "Handle a missing credential",
            "develop",
            Some("Which environment should the Agent use?"),
        ),
        (3, "Explore a smaller navigation layout", "backlog", None),
        (
            4,
            "Auto-merged today · dashed border + quick Archive",
            "review",
            None,
        ),
    ] {
        let delivery = (i == 4).then(|| Delivery {
            target_branch: "main".into(),
            commit: "fcf9d2d".into(),
            request_key: "gallery-merged-today".into(),
            merged_at: now(),
        });
        let resolution = (i == 4).then(|| CardResolution {
            resolved_at: now(),
            kind: "merged".into(),
            reason: None,
            actor: "system".into(),
        });
        let tag_suffix = match i {
            1 => "\n\nTags: #search #ux #agent #bug",
            2 => "\n\nTags: #agent #terminal",
            3 => "\n\nTags: #ux",
            _ => "",
        };
        let pull_request = match i {
            1 => Some(canban_types::GitHubPr {
                review_summary: "0 approvals".into(),
                repository: "jjyr/canban".into(),
                repository_id: 1,
                number: 148,
                url: "https://github.com/jjyr/canban/pull/148".into(),
                state: "open".into(),
                head: "abcdef1234567890".into(),
                head_branch: format!("canban/gallery/{i}"),
                base_branch: "main".into(),
                merged: false,
                checks: "running".into(),
                checks_summary: Some(canban_types::GitHubPrChecks {
                    state: "running".into(),
                    head_sha: "abcdef1234567890".into(),
                    url: Some("https://github.com/jjyr/canban/pull/148/checks".into()),
                    total: 4,
                    passed: 2,
                    failed: 0,
                    running: 2,
                    failed_checks: vec![],
                }),
                workspace: canban_types::WorkspaceBinding {
                    directory: "/preview".into(),
                    path: format!("/preview/worktrees/card-{i}"),
                    branch: Some(format!("canban/gallery/{i}")),
                    base_commit: Some("abcdef0123".into()),
                    target_branch: Some("main".into()),
                },
            }),
            2 => Some(canban_types::GitHubPr {
                review_summary: "1 approval".into(),
                repository: "jjyr/canban".into(),
                repository_id: 1,
                number: 147,
                url: "https://github.com/jjyr/canban/pull/147".into(),
                state: "open".into(),
                head: "bcdef12345678901".into(),
                head_branch: format!("canban/gallery/{i}"),
                base_branch: "main".into(),
                merged: false,
                checks: "failure".into(),
                checks_summary: Some(canban_types::GitHubPrChecks {
                    state: "failure".into(),
                    head_sha: "bcdef12345678901".into(),
                    url: Some("https://github.com/jjyr/canban/pull/147/checks".into()),
                    total: 4,
                    passed: 3,
                    failed: 1,
                    running: 0,
                    failed_checks: vec![canban_types::GitHubPrCheck {
                        name: "clippy".into(),
                        status: "completed".into(),
                        conclusion: Some("failure".into()),
                        url: Some("https://github.com/jjyr/canban/actions/runs/1/job/2".into()),
                    }],
                }),
                workspace: canban_types::WorkspaceBinding {
                    directory: "/preview".into(),
                    path: format!("/preview/worktrees/card-{i}"),
                    branch: Some(format!("canban/gallery/{i}")),
                    base_commit: Some("abcdef0123".into()),
                    target_branch: Some("main".into()),
                },
            }),
            3 => Some(canban_types::GitHubPr {
                review_summary: "1 approval".into(),
                repository: "jjyr/canban".into(),
                repository_id: 1,
                number: 146,
                url: "https://github.com/jjyr/canban/pull/146".into(),
                state: "open".into(),
                head: "cdef123456789012".into(),
                head_branch: format!("canban/gallery/{i}"),
                base_branch: "main".into(),
                merged: false,
                checks: "success".into(),
                checks_summary: Some(canban_types::GitHubPrChecks {
                    state: "success".into(),
                    head_sha: "cdef123456789012".into(),
                    url: Some("https://github.com/jjyr/canban/pull/146/checks".into()),
                    total: 4,
                    passed: 4,
                    failed: 0,
                    running: 0,
                    failed_checks: vec![],
                }),
                workspace: canban_types::WorkspaceBinding {
                    directory: "/preview".into(),
                    path: format!("/preview/worktrees/card-{i}"),
                    branch: Some(format!("canban/gallery/{i}")),
                    base_commit: Some("abcdef0123".into()),
                    target_branch: Some("main".into()),
                },
            }),
            _ => None,
        };
        s.cards.push(Card{schedule_source:None,source_trust:Some("local".into()),hold_auto_merge:false,github_links:vec![],created_by:"Gallery".into(),content:Default::default(),removed_at:None,pull_request,pull_request_history:Vec::new(),delivery_mode:None,workspace:None,workspace_history:Vec::new(),id:format!("card-{i}"),project_id:"gallery-project".into(),number:i,board_order:None,title:title.into(),body:format!("## Goal\nMake progress easy to understand.\n\nKeep long descriptions readable, preserve drafts and provide clear next actions.\n\n中文与 English content uses the same document component.{tag_suffix}"),context_notes:Some(vec!["note-1".into()]),stage:stage.into(),revision:1,attention:attention.map(str::to_owned),archived_at:None,archive:None,archive_error:None,worktree:Some(format!("/preview/worktrees/card-{i}")),branch:Some(format!("canban/gallery/{i}")),base_commit:Some("abcdef0123".into()),human_gate:attention.map(|reason|HumanGate{reason:reason.into(),terminal_id:None,next_stage:None}),delivery,resolution,created_at:now()});
    }
    s.cards.push(Card {
        schedule_source: None,
        source_trust: Some("local".into()),
        hold_auto_merge: false,
        github_links: vec![],
        created_by: "Gallery".into(),
        content: Default::default(),
        removed_at: None,
        pull_request: None,
        pull_request_history: Vec::new(),
        delivery_mode: None,
        workspace: None,
        workspace_history: Vec::new(),
        id: "card-resolved-manual".into(),
        project_id: "gallery-project".into(),
        number: 6,
        board_order: None,
        title: "Resolved manually · dashed border + Reopen".into(),
        body: "Closed as duplicate without creating a branch.".into(),
        context_notes: None,
        stage: "backlog".into(),
        revision: 1,
        attention: None,
        archived_at: None,
        archive: None,
        archive_error: None,
        worktree: None,
        branch: None,
        base_commit: None,
        human_gate: None,
        delivery: None,
        resolution: Some(CardResolution {
            resolved_at: now(),
            kind: "manual".into(),
            reason: Some("Duplicate of #1".into()),
            actor: "user".into(),
        }),
        created_at: now(),
    });
    s.notes.push(Note{id:"note-1".into(),project_id:"gallery-project".into(),title:"Search ideas.md".into(),body:"# Search experience\n\nFind a Card without remembering its number.\n\nSee [Architecture](note:note-2).".into(),revision:1,created_at:now(),updated_at:now(),links:vec!["note-2".into()],card_ids:vec!["card-1".into()]});
    s.notes.push(Note {
        id: "note-2".into(),
        project_id: "gallery-project".into(),
        title: "Architecture.md".into(),
        body: "Shared decisions live here. Incoming links appear as backlinks.".into(),
        revision: 1,
        created_at: now(),
        updated_at: now(),
        links: vec![],
        card_ids: vec![],
    });
    let mut event = timeline();
    event.card_id = "card-1".into();
    event.created_at = now();
    apply_fixture_events(&mut s, &[event]);
    s.note_events.push(NoteEvent {
        id: "note-event".into(),
        project_id: "gallery-project".into(),
        note_id: "note-1".into(),
        sequence: 1,
        kind: "note.updated".into(),
        actor: "@developer".into(),
        message: "Updated search constraints while handling Card #1".into(),
        card_id: Some("card-1".into()),
        revision: 1,
        data: json!({}),
        created_at: now(),
    });
    let default_doc = canban_workflow_engine::source::default_document(
        "delivery:gallery-project",
        WorkspaceSettings::default(),
    );
    let default_yaml = canban_workflow_engine::source::encode(&default_doc).unwrap();
    let default_def = default_doc.compile().unwrap();
    s.workflow_revisions.push(WorkflowRevision {
        host_id: String::new(),
        source_yaml: Some(default_yaml),
        id: "delivery:gallery-project:1".into(),
        project_id: "gallery-project".into(),
        workflow_id: "delivery:gallery-project".into(),
        revision: 1,
        definition: default_def,
        created_at: now(),
    });
    s.workflow_sources.push(WorkflowSource {
        host_id: String::new(),
        id: "gallery-project:development.yaml".into(),
        project_id: "gallery-project".into(),
        filename: "development.yaml".into(),
        workflow_id: "delivery:gallery-project".into(),
        revision: 1,
        hash: "gallery-default".into(),
        revision_id: "delivery:gallery-project:1".into(),
        enabled: true,
        columns: default_doc.columns,
        rules: default_doc.on,
    });
    s
}

fn push_agent_terminal(s: &mut canban_types::Snapshot, run: canban_types::AgentRun) -> String {
    let tid = canban_types::Conversation::key(&run.card_id, &run.profile.id);
    if let Some(existing) = s.terminals.iter_mut().find(|t| t.id == tid) {
        existing.agent = Some(run);
        return tid;
    }
    let number = s.terminals.iter().map(|t| t.number).max().unwrap_or(0) + 1;
    let origin = Some(run.card_id.clone());
    let created_at = run.updated_at;
    s.terminals.push(canban_types::TerminalEntry {
        id: tid.clone(),
        conversation_id: Some(tid.clone()),
        number,
        name: None,
        origin_project_id: None,
        origin_card_id: origin,
        agent_engine: None,
        runtime: None,
        agent: Some(run),
        created_at,
    });
    tid
}

pub fn workspace_with_card_terminals() -> canban_types::Snapshot {
    use canban_types::*;
    let mut s = workspace();
    let dev_profile = agent_config()
        .profiles
        .into_iter()
        .find(|p| p.id == "developer")
        .unwrap();
    let mut run = canban_scheduler::agent_run(
        "gallery-project",
        "card-2",
        1,
        "develop",
        "stage_entry",
        dev_profile,
        "Implement credential validation.".into(),
        None,
    );
    run.state = "running".into();
    run.core_id = Some("gallery".into());
    run.terminal = Some("card-2-dev-pty".into());
    run.cwd = "/preview/worktrees/card-2".into();
    run.input_path = "input.md".into();
    run.observed = Some(json!({"state": "live", "status": {"state": "working"}}));
    run.provider_session_id = Some("session-card-2".into());
    run.connection_state = "connected".into();
    run.lifecycle.observed_at = Some(u64::MAX);
    push_agent_terminal(&mut s, run);
    s.terminals.push(TerminalEntry {
        id: "card-2-shell-1".into(),
        conversation_id: None,
        number: 2,
        name: Some("Terminal 1".into()),
        origin_project_id: Some("gallery-project".into()),
        origin_card_id: Some("card-2".into()),
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            core_id: Some("gallery".into()),
            terminal: Some("card-2-shell-pty".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: now(),
    });
    s.terminals.push(TerminalEntry {
        id: "card-2-agy-1".into(),
        conversation_id: None,
        number: 3,
        name: Some("Antigravity 1".into()),
        origin_project_id: Some("gallery-project".into()),
        origin_card_id: Some("card-2".into()),
        agent_engine: Some("agy".into()),
        runtime: Some(TerminalRuntime {
            core_id: Some("gallery".into()),
            terminal: Some("card-2-agy-pty".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: now(),
    });
    s
}

/// Three overflowing columns for independent scrolling and overlay thumb regression checks.
pub fn overflowing_board() -> canban_types::Snapshot {
    let mut s = workspace();
    let base = s.cards[2].clone();
    s.cards.clear();
    for (stage_index, stage) in ["backlog", "develop", "review"].iter().enumerate() {
        for i in 1..=24 {
            let mut card = base.clone();
            card.number = (stage_index * 24 + i) as u64;
            card.id = format!("overflow-{}", card.number);
            card.stage = (*stage).into();
            card.title = format!("Card {} · Preserve context while scrolling", card.number);
            s.cards.push(card);
        }
    }
    s
}

/// Lift regression: uneven rows make the bottom Card's missing transform obvious.
/// Drag #4 by its body or grip; it must stay under the grab point at lift/release.
pub fn bottom_card_drag() -> canban_types::Snapshot {
    let mut s = overflowing_board();
    s.cards.retain(|c| c.number <= 4);
    s.cards[1].title = "A taller Card with a deliberately long title that wraps across several lines before the next row".into();
    s.cards[3].title = "Drag this bottom Card without jumping to the top".into();
    s
}

/// Deep Review column (36 Cards, mixed heights, tags, branches, and merged state)
/// exceeding 3x viewport scroll height to guard against content-culling reclaim blanking
/// and row coordinate/reconciliation overlap.
pub fn deep_review_board() -> canban_types::Snapshot {
    let mut s = workspace();
    let base = s.cards[2].clone();
    s.cards.clear();
    for (i, stage) in [(1u64, "backlog"), (2, "backlog"), (3, "develop")] {
        let mut card = base.clone();
        card.number = i;
        card.id = format!("deep-seed-{i}");
        card.stage = stage.into();
        card.title = format!("Card #{i} · Active work in {stage}");
        card.human_gate = None;
        card.attention = None;
        s.cards.push(card);
    }
    for idx in 1..=36u64 {
        let n = 10 + idx;
        let mut card = base.clone();
        card.number = n;
        card.id = format!("deep-review-{idx}");
        card.stage = "review".into();
        card.branch = Some(format!("canban-0afd/{n}"));
        card.title = if idx % 3 == 0 {
            format!(
                "Review #{idx} · Multi-line title checking variable row height and deep scroll culling stability"
            )
        } else {
            format!("Review #{idx} · Verify bottom Card content stays visible")
        };
        card.body = if idx % 2 == 0 {
            format!("Excerpt for Review #{idx} with #search and #ux tags.")
        } else {
            String::new()
        };
        if idx > 12 {
            card.human_gate = None;
            card.attention = None;
            card.delivery = Some(canban_types::Delivery {
                target_branch: "main".into(),
                commit: format!("deadbeef{idx:02x}"),
                request_key: format!("deep-req-{idx}"),
                merged_at: 1_700_000_000_000 + idx,
            });
        }
        s.cards.push(card);
    }
    s
}

/// Board-only project switching: human gates, unbadged work and quiet projects,
/// long Unicode names and overflow. Uses the production Overview classification.
pub fn project_activity() -> canban_types::Snapshot {
    use canban_types::*;
    let mut s = workspace();
    for (i, name) in [
        "Fiber",
        "Pot",
        "Shear",
        "Tools",
        "Research · 项目上下文与非常长的名字",
        "Empty project",
        "Docs",
        "Web",
        "Sandbox",
        "API",
        "Design",
        "Lab",
    ]
    .iter()
    .enumerate()
    {
        let mut project = s.projects[0].clone();
        project.id = format!("activity-{i}");
        project.name = (*name).into();
        project.created_at = i as u64 + 1;
        let mut card = s.cards[2].clone();
        card.id = format!("activity-card-{i}");
        card.project_id = project.id.clone();
        card.title = format!("{name} task");
        card.human_gate = None;
        card.attention = None;
        if i < 4 {
            let state = ["completed", "blocked", "starting", "queued"][i];
            let mut attempt = canban_scheduler::agent_run(
                &project.id,
                &card.id,
                1,
                "develop",
                "stage_entry",
                Config::default().profiles[0].clone(),
                "Fixture".into(),
                None,
            );
            attempt.state = state.into();
            let tid = push_agent_terminal(&mut s, attempt);
            if i < 2 {
                card.human_gate = Some(HumanGate {
                    reason: if i == 0 {
                        "Review the result"
                    } else {
                        "Answer the Agent"
                    }
                    .into(),
                    terminal_id: Some(tid),
                    next_stage: None,
                });
            }
        }
        if i < 5 {
            let mut event = timeline();
            event.id = format!("activity-event-{i}");
            event.project_id = project.id.clone();
            event.card_id = card.id.clone();
            event.created_at = now() - i as u64 * 1000;
            s.cards.push(card);
            apply_fixture_events(&mut s, &[event]);
        }
        s.projects.push(project);
    }
    let mut another = s
        .cards
        .iter()
        .find(|c| c.project_id == "activity-1")
        .unwrap()
        .clone();
    another.id = "activity-second-input".into();
    another.number = 4;
    another.title = "A separate human decision".into();
    another.human_gate = None;
    another.attention = Some("Choose the next step".into());
    s.cards.push(another);
    s
}

/// All Overview groups, independent reasons on one row, multiple Projects and long context.
pub fn overview() -> canban_types::Snapshot {
    use canban_types::*;
    let mut s = workspace();
    let mut second = s.projects[0].clone();
    second.id = "second-project".into();
    second.name = "Tools".into();
    s.projects.push(second);
    let base = s.cards[0].clone();
    s.cards.clear();
    for (i, state) in [
        "blocked", "review", "working", "queued", "idle", "closed", "unknown", "exited",
    ]
    .iter()
    .enumerate()
    {
        let mut card = base.clone();
        card.id = format!("overview-card-{i}");
        card.number = i as u64 + 1;
        card.title = format!(
            "{} · {}",
            [
                "Approve repository access",
                "Inspect saved delivery",
                "Preserve keyboard focus",
                "Build the next iteration",
                "Discuss the API",
                "Resume previous investigation",
                "Recover disconnected Agent",
                "Recover Agent that exited before reporting"
            ][i],
            state
        );
        card.project_id = s.projects[i % 2].id.clone();
        card.stage = if *state == "review" {
            "review"
        } else {
            "develop"
        }
        .into();
        card.human_gate = None;
        let profile = Config::default().profiles[0].clone();
        let run_tid = Conversation::key(&card.id, &profile.id);
        let mut run = canban_scheduler::agent_run(
            &card.project_id,
            &card.id,
            1,
            &card.stage,
            "stage_entry",
            profile,
            "Inspect the Card context and implement a small, reviewable change.\n\nKeep the user's existing work and explain any decision that needs attention.".into(),
            None,
        );
        run.state = if *state == "queued" {
            "queued"
        } else if *state == "review" {
            "completed"
        } else if *state == "exited" {
            "failed"
        } else {
            "running"
        }
        .into();
        if *state == "review" {
            card.human_gate = Some(HumanGate {
                reason: "Agent report ready; human review required".into(),
                terminal_id: Some(run_tid.clone()),
                next_stage: None,
            });
            s.reports.push(Report {
                id: format!("overview-report-{i}"),
                terminal_id: Some(run_tid),
                workflow_run_id: None,
                actor: "agent:developer".into(),
                project_id: card.project_id.clone(),
                card_id: card.id.clone(),
                state: "completed".into(),
                message: "Completed changes and verified all workspace tests".into(),
                path: None,
                request_key: format!("key-{i}"),
                fingerprint: format!("fp-{i}"),
                created_at: now() - 60_000,
            });
        }
        if *state != "queued" {
            run.core_id = Some("gallery-core".into());
            run.terminal = Some(format!("t{}", i + 1));
            run.cwd = "/example/card-worktree".into();
            run.input_path = "input.md".into();
            run.updated_at = now() - 180_000;
            run.provider_session_id = Some(format!("session-{i}"));
            run.connection_state = if *state == "unknown" {
                "unavailable"
            } else {
                "connected"
            }
            .into();
            run.observed = Some(json!({
                "state": if matches!(*state, "closed" | "exited") { "finished" } else { "live" },
                "status": {
                    "state": if *state == "review" { "done" } else { state },
                    "kind": "permission",
                    "title": "Allow repository command?",
                    "msg": "Agent requests permission to run git status in the Card worktree."
                }
            }));
            if *state == "exited" {
                run.observed = Some(json!({"state":"finished","exit_code":1,"status":null}));
                run.output_path = Some("saved-output.txt".into());
                run.close_state = "closed".into();
                run.error = Some("Agent exited without reporting an outcome".into());
            }
            // A Gallery fixture has no polling runtime; keep its declared status fresh.
            run.lifecycle.observed_at = Some(u64::MAX);
            if *state == "blocked" {
                run.lifecycle.waiting = Some(RuntimeWait {
                    id: "gallery-wait".into(),
                    kind: "permission".into(),
                    title: "Allow repository command?".into(),
                    message: "git status in this worktree".into(),
                    since: now() - 120_000,
                });
            }
            if *state == "idle" {
                run.lifecycle.idle_since = Some(now() - 240_000);
            }
        }
        push_agent_terminal(&mut s, run);
        s.cards.push(card);
    }
    let default_prof = Config::default().profiles[0].id.clone();
    let card0_tid = Conversation::key(&s.cards[0].id, &default_prof);
    let card2_tid = Conversation::key(&s.cards[2].id, &default_prof);
    // Multi-reason and long-title row exercises compact layout and both direct actions.
    s.cards[0].title =
        "修复支付重试逻辑 · preserve the complete task identity when the window is narrow".into();
    s.cards[0].human_gate = Some(HumanGate {
        reason: "Decide whether to raise the retry limit".into(),
        terminal_id: Some(card0_tid.clone()),
        next_stage: None,
    });
    if let Some(a) = s
        .terminals
        .iter_mut()
        .find(|t| t.id == card2_tid)
        .and_then(|t| t.agent.as_mut())
    {
        a.observed = Some(
            json!({"state":"live","status":{"state":"working","msg":"Adding keyboard focus regression coverage"}}),
        );
    }
    // A second live profile makes Card-level navigation require an explicit choice.
    let mut attempt = s
        .terminals
        .iter()
        .find(|t| t.id == card2_tid)
        .and_then(|t| t.agent.clone())
        .unwrap();
    attempt.profile.id = "reviewer".into();
    attempt.terminal = Some("reviewer-pty".into());
    push_agent_terminal(&mut s, attempt);
    let mut overview_events = Vec::new();
    for (index, (card_index, tid)) in [(1, (0, &card0_tid)), (2, (2, &card2_tid))] {
        let mut event = timeline();
        event.id = format!("overview-unread-{index}");
        event.sequence = index;
        event.card_id = s.cards[card_index].id.clone();
        event.project_id = s.cards[card_index].project_id.clone();
        event.kind = "comment.replied".into();
        event.actor = "agent:codex".into();
        event.message = "Added regression coverage; please check the new boundary case.".into();
        event.data = json!({"terminal_id": tid});
        overview_events.push(event);
    }
    s.cards[2].body=(0..20).map(|i|format!("### Context {i}\n\nLong task context tests direct navigation to an unread reply.\n\n")).collect();
    let mut stage_done = timeline();
    stage_done.id = "overview-unread-stage".into();
    stage_done.sequence = 10;
    stage_done.card_id = s.cards[3].id.clone();
    stage_done.project_id = s.cards[3].project_id.clone();
    stage_done.kind = "execution.reported".into();
    stage_done.actor = "agent:developer".into();
    stage_done.message = "completed: Unit tests passed; queued for next stage".into();
    stage_done.data = json!({"report_id": "overview-stage-rep", "state": "completed"});
    overview_events.push(stage_done);
    apply_fixture_events(&mut s, &overview_events);

    let mut merge_card = base.clone();
    merge_card.id = "overview-merge-conflict".into();
    merge_card.number = 9;
    merge_card.title = "Refactor event bus".into();
    merge_card.stage = "review".into();
    merge_card.human_gate = Some(HumanGate {
        reason: "Merge conflict with main; auto-merge paused".into(),
        terminal_id: None,
        next_stage: None,
    });
    s.cards.push(merge_card);

    let mut recovery = base;
    recovery.id = "workspace-recovery".into();
    recovery.title = "Reconnect the execution machine".into();
    recovery.human_gate = None;
    recovery.attention = Some("Execution machine unavailable; reconnect before retrying".into());
    s.cards.push(recovery);

    let number = s.terminals.iter().map(|t| t.number).max().unwrap_or(0) + 1;
    s.terminals.push(TerminalEntry {
        id: "overview-manual-agent".into(),
        conversation_id: None,
        number,
        name: Some("Terminal 2".into()),
        origin_project_id: Some("second-project".into()),
        origin_card_id: None,
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            core_id: Some("gallery-core".into()),
            terminal: Some("manual-pty-2".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("working".into()),
            agent_turn: Some(1),
            observed_agent: Some("claude".into()),
            waiting: None,
        }),
        agent: None,
        created_at: now() - 60_000,
    });
    s.terminal_contexts.push(TerminalContext {
        terminal_id: "overview-manual-agent".into(),
        core_id: "gallery-core".into(),
        terminal: "manual-pty-2".into(),
        cwd: Some("/preview/tools".into()),
        directory_root: Some("/preview/tools".into()),
        project_id: Some("second-project".into()),
        card_id: None,
        worktree_id: None,
        branch: Some("main".into()),
        dirty: false,
        additions: 0,
        deletions: 0,
        ahead: 0,
        behind: 0,
    });
    s
}

/// Quick Open coverage: cross-Project duplicates, Unicode, long names, Agent
/// states and enough results to exercise keyboard-driven list scrolling.
pub fn search() -> canban_types::Snapshot {
    let mut s = overview();
    let base = s.cards[0].clone();
    for (id, project) in [("same-a", "gallery-project"), ("same-b", "second-project")] {
        let mut c = base.clone();
        c.id = id.into();
        c.project_id = project.into();
        c.title = "Search keyboard navigation".into();
        c.number = 42;
        c.human_gate = None;
        c.attention = None;
        s.cards.push(c);
    }
    let note = s.notes[0].clone();
    let mut other = note.clone();
    other.id = "search-note".into();
    other.project_id = "second-project".into();
    s.notes.push(other);
    let mut chinese = note;
    chinese.id = "unicode-note".into();
    chinese.title = "中文架构决策.md".into();
    s.notes.push(chinese);
    for i in 0..70 {
        let mut c = base.clone();
        c.id = format!("search-card-{i}");
        c.number = 100 + i;
        c.title = format!("Search fixture {i:02} · a long Card title for candidate layout checks");
        c.human_gate = None;
        c.attention = None;
        s.cards.push(c);
    }
    let mut archived = base.clone();
    archived.id = "search-archived".into();
    archived.number = 900;
    archived.title = "Archived fixture · hidden from Quick Open".into();
    archived.archived_at = Some(canban_types::now());
    archived.human_gate = None;
    archived.attention = None;
    s.cards.push(archived);
    let mut merged = base;
    merged.id = "search-merged".into();
    merged.number = 901;
    merged.title = "Merged fixture · hidden from Quick Open".into();
    merged.human_gate = None;
    merged.attention = None;
    merged.delivery = Some(canban_types::Delivery {
        target_branch: "main".into(),
        commit: "abc1234".into(),
        request_key: "gallery-search-merged".into(),
        merged_at: canban_types::now(),
    });
    s.cards.push(merged);
    s
}

/// Terminal navigation adds two profiles on one Card and closed archived history.
pub fn terminal_workspace() -> canban_types::Snapshot {
    let mut s = overview();
    s.projects[0].name = "Canban workspace with a long Project name".into();
    let mut reviewer = s
        .terminals
        .iter()
        .find_map(|t| t.agent.as_ref().filter(|r| r.card_id == "overview-card-2"))
        .unwrap()
        .clone();
    reviewer.profile.id = "reviewer".into();
    reviewer.profile.preset = Some("claude".into());
    reviewer.stage = "review".into();
    reviewer.state = "queued".into();
    reviewer.core_id = None;
    reviewer.terminal = None;
    reviewer.cwd.clear();
    reviewer.input_path.clear();
    reviewer.observed = None;
    reviewer.provider_session_id = None;
    reviewer.connection_state.clear();
    reviewer.updated_at += 1;
    push_agent_terminal(&mut s, reviewer);
    let mut card = s
        .cards
        .iter()
        .find(|c| c.id == "overview-card-5")
        .unwrap()
        .clone();
    card.id = "terminal-archived".into();
    card.title = "Archived investigation".into();
    card.archived_at = Some(canban_types::now());
    let mut run = s
        .terminals
        .iter()
        .find_map(|t| t.agent.as_ref().filter(|a| a.card_id == "overview-card-5"))
        .unwrap()
        .clone();
    run.card_id = card.id.clone();
    s.cards.push(card);
    push_agent_terminal(&mut s, run);
    if let Some(card) = s.cards.iter_mut().find(|c| c.id == "overview-card-2") {
        card.title="修复跨项目调度中的上下文关联并验证长标题不会挤掉阶段与操作 / Preserve the full Agent context across recovery".into();
    }
    s.terminals = canban_ui::terminals::entries(&s)
        .into_iter()
        .map(|e| e.saved)
        .collect();
    let number = s.terminals.iter().map(|t| t.number).max().unwrap_or(0) + 1;
    s.terminals.push(canban_types::TerminalEntry {
        id: "gallery-manual".into(),
        conversation_id: None,
        number,
        name: Some("Terminal 1".into()),
        origin_project_id: None,
        origin_card_id: None,
        agent_engine: None,
        runtime: Some(canban_types::TerminalRuntime {
            core_id: Some("gallery".into()),
            terminal: Some("shell".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: 0,
    });
    let mut remote = s.terminals.last().unwrap().clone();
    remote.id = "gallery-remote-shell".into();
    remote.number += 2;
    remote.name = Some("Remote shell with a long custom name for layout checks".into());
    let runtime = remote.runtime.as_mut().unwrap();
    runtime.terminal = Some("gallery-remote/shell".into());
    runtime.connection = "unavailable".into();
    let mut linked = s.terminals.last().unwrap().clone();
    linked.id = "gallery-linked-shell".into();
    linked.number += 1;
    linked.name = Some("Terminal 1".into());
    linked.runtime.as_mut().unwrap().terminal = Some("linked-shell".into());
    s.terminals.push(linked);
    s.terminals.push(remote);
    s.entity_hosts
        .insert("gallery-remote-shell".into(), "gallery-remote".into());
    s.entity_hosts
        .insert("overview-card-2".into(), "gallery-remote".into());
    s.host_summaries.insert(
        "gallery-remote".into(),
        canban_types::HostSummary {
            id: "gallery-remote".into(),
            name: "Build machine".into(),
            is_local: false,
            transport_label: "SSH".into(),
            endpoint_label: "ssh-host".into(),
            ssh_target: Some("ssh-host".into()),
            server_id: None,
            connection: canban_types::HostConnectionState::Connected {
                rtt_ms: 14,
                api_version: canban_updater::DESKTOP_API,
                version: env!("CARGO_PKG_VERSION").into(),
                platform: "linux-x86_64".into(),
            },
            active_agents: 1,
            max_agents: 4,
            project_count: 1,
            reconnect_count: 0,
            last_synced_at: None,
            pending_mutations: 0,
            sync_conflicts: 0,
        },
    );
    for t in &mut s.terminals {
        if let Some(attempt) = t.agent.as_mut()
            && attempt.card_id == "overview-card-2"
            && attempt.terminal.is_some()
        {
            attempt.directory = Some("/remote/workspaces/canban".into());
            attempt.terminal = Some("gallery-remote/t3".into());
            s.entity_hosts.insert(t.id.clone(), "gallery-remote".into());
        }
    }
    let card = s.cards.iter().find(|c| c.archived_at.is_none()).unwrap();
    let card_project_id = card.project_id.clone();
    let card_id = card.id.clone();
    s.terminal_contexts.push(canban_types::TerminalContext {
        directory_root: None,
        terminal_id: "gallery-linked-shell".into(),
        core_id: "gallery".into(),
        terminal: "linked-shell".into(),
        cwd: Some("/tmp/gallery/worktree".into()),
        project_id: Some(card_project_id),
        card_id: Some(card_id),
        worktree_id: None,
        branch: Some(
            "canban/very-long-worktree-and-branch-name/context-navigation-acceptance-42".into(),
        ),
        dirty: true,
        additions: 48,
        deletions: 12,
        ahead: 2,
        behind: 0,
    });
    let proj_id = s.projects[0].id.clone();
    s.terminals.push(canban_types::TerminalEntry {
        id: "gallery-project-root-shell".into(),
        conversation_id: None,
        number: number + 3,
        name: Some("Terminal 2".into()),
        origin_project_id: Some(proj_id.clone()),
        origin_card_id: None,
        agent_engine: None,
        runtime: Some(canban_types::TerminalRuntime {
            core_id: Some("gallery".into()),
            terminal: Some("root-shell".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("working".into()),
            agent_turn: Some(1),
            observed_agent: Some("agy".into()),
            waiting: None,
        }),
        agent: None,
        created_at: 0,
    });
    s.terminal_contexts.push(canban_types::TerminalContext {
        directory_root: None,
        terminal_id: "gallery-project-root-shell".into(),
        core_id: "gallery".into(),
        terminal: "root-shell".into(),
        cwd: Some(s.projects[0].directory.clone()),
        project_id: Some(proj_id.clone()),
        card_id: None,
        worktree_id: None,
        branch: Some("main".into()),
        dirty: false,
        additions: 0,
        deletions: 0,
        ahead: 0,
        behind: 1,
    });
    s.worktrees.push(canban_types::ManagedWorktree {
        id: "gallery-wt-spike".into(),
        project_id: proj_id.clone(),
        branch: "feat/standalone-spike".into(),
        base_branch: "main".into(),
        base_commit: None,
        path: "/tmp/gallery/worktrees/standalone-spike".into(),
        directory: "/tmp/gallery/worktrees/standalone-spike".into(),
        created_at: canban_types::now(),
    });
    s.worktrees.push(canban_types::ManagedWorktree {
        id: "gallery-wt-retained".into(),
        project_id: proj_id.clone(),
        branch: "chore/retained-experiment".into(),
        base_branch: "main".into(),
        base_commit: None,
        path: "/tmp/gallery/worktrees/retained-experiment".into(),
        directory: "/tmp/gallery/worktrees/retained-experiment".into(),
        created_at: canban_types::now(),
    });
    s.terminals.push(canban_types::TerminalEntry {
        id: "gallery-wt-shell".into(),
        conversation_id: None,
        number: number + 4,
        name: Some("feat/standalone-spike".into()),
        origin_project_id: Some(proj_id.clone()),
        origin_card_id: None,
        agent_engine: None,
        runtime: Some(canban_types::TerminalRuntime {
            core_id: Some("gallery".into()),
            terminal: Some("wt-shell".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("done".into()),
            agent_turn: Some(2),
            observed_agent: Some("codex".into()),
            waiting: None,
        }),
        agent: None,
        created_at: 0,
    });
    s.terminal_contexts.push(canban_types::TerminalContext {
        directory_root: None,
        terminal_id: "gallery-wt-shell".into(),
        core_id: "gallery".into(),
        terminal: "wt-shell".into(),
        cwd: Some("/tmp/gallery/worktrees/standalone-spike".into()),
        project_id: Some(proj_id.clone()),
        card_id: None,
        worktree_id: Some("gallery-wt-spike".into()),
        branch: Some("feat/standalone-spike".into()),
        dirty: true,
        additions: 14,
        deletions: 3,
        ahead: 3,
        behind: 1,
    });
    s.terminals.push(canban_types::TerminalEntry {
        id: "gallery-wt-shell-2".into(),
        conversation_id: None,
        number: number + 5,
        name: Some("Terminal 3".into()),
        origin_project_id: Some(proj_id.clone()),
        origin_card_id: None,
        agent_engine: None,
        runtime: Some(canban_types::TerminalRuntime {
            core_id: Some("gallery".into()),
            terminal: Some("wt-shell-2".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("blocked".into()),
            agent_turn: Some(1),
            observed_agent: Some("claude".into()),
            waiting: Some(canban_types::RuntimeWait {
                id: "gallery-wt-wait".into(),
                kind: "permission".into(),
                title: "Allow command?".into(),
                message: "cargo test --workspace".into(),
                since: canban_types::now().saturating_sub(2 * 60 * 1000),
            }),
        }),
        agent: None,
        created_at: 0,
    });
    s.terminals.push(canban_types::TerminalEntry {
        id: "gallery-ephemeral-agent".into(),
        conversation_id: None,
        number: number + 6,
        name: Some("Antigravity 1".into()),
        origin_project_id: Some(proj_id.clone()),
        origin_card_id: None,
        agent_engine: Some("agy".into()),
        runtime: Some(canban_types::TerminalRuntime {
            core_id: Some("gallery".into()),
            terminal: Some("ephemeral-agy".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("idle".into()),
            agent_turn: Some(1),
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: 0,
    });
    s.terminal_contexts.push(canban_types::TerminalContext {
        directory_root: None,
        terminal_id: "gallery-ephemeral-agent".into(),
        core_id: "gallery".into(),
        terminal: "ephemeral-agy".into(),
        cwd: Some(s.projects[0].directory.clone()),
        project_id: Some(proj_id.clone()),
        card_id: None,
        worktree_id: None,
        branch: Some("main".into()),
        dirty: false,
        additions: 0,
        deletions: 0,
        ahead: 0,
        behind: 1,
    });
    s.terminal_contexts.push(canban_types::TerminalContext {
        directory_root: None,
        terminal_id: "gallery-wt-shell-2".into(),
        core_id: "gallery".into(),
        terminal: "wt-shell-2".into(),
        cwd: Some("/tmp/gallery/worktrees/standalone-spike".into()),
        project_id: Some(proj_id),
        card_id: None,
        worktree_id: Some("gallery-wt-spike".into()),
        branch: Some("feat/standalone-spike".into()),
        dirty: true,
        additions: 14,
        deletions: 3,
        ahead: 3,
        behind: 1,
    });
    s.projects.push(canban_types::Project {
        id: "gallery-empty-project".into(),
        name: "Docs & Website".into(),
        directory: "/tmp/gallery/docs-site".into(),
        github: None,
        detected_github: None,
        github_disconnected: false,
        delivery_mode: Default::default(),
        agent_overrides: Default::default(),
        tags: Default::default(),
        workflow: canban_types::Workflow::default(),
        target_branch: Some("main".into()),
        created_at: 0,
    });
    s
}

/// Terminals workspace with many tabs in the active project-root scope to verify header tab scrolling.
pub fn terminal_many_tabs() -> canban_types::Snapshot {
    let mut s = terminal_workspace();
    let proj_id = s.projects[0].id.clone();
    let proj_dir = s.projects[0].directory.clone();
    let next_num = s.terminals.iter().map(|t| t.number).max().unwrap_or(0) + 1;
    for idx in 1..=12u64 {
        let id = format!("gallery-overflow-shell-{idx}");
        let pty = format!("overflow-shell-{idx}");
        s.terminals.push(canban_types::TerminalEntry {
            id: id.clone(),
            conversation_id: None,
            number: next_num + idx,
            name: Some(format!("Shell {idx:02} · build-check")),
            origin_project_id: Some(proj_id.clone()),
            origin_card_id: None,
            agent_engine: None,
            runtime: Some(canban_types::TerminalRuntime {
                core_id: Some("gallery".into()),
                terminal: Some(pty.clone()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
            created_at: idx,
        });
        s.terminal_contexts.push(canban_types::TerminalContext {
            directory_root: None,
            terminal_id: id,
            core_id: "gallery".into(),
            terminal: pty,
            cwd: Some(proj_dir.clone()),
            project_id: Some(proj_id.clone()),
            card_id: None,
            worktree_id: None,
            branch: Some("main".into()),
            dirty: true,
            additions: 18,
            deletions: 4,
            ahead: 1,
            behind: 0,
        });
    }
    s
}

/// Board density: quiet, queued, executing and human-gated Cards share one renderer.
pub fn board_cards() -> canban_types::Snapshot {
    use canban_types::*;
    let mut s = workspace();
    for c in &mut s.cards {
        c.branch = None;
        c.worktree = None;
    }
    s.cards[0].human_gate = Some(HumanGate {
        reason: "Review the report".into(),
        terminal_id: None,
        next_stage: None,
    });
    s.cards[1].attention = None;
    // Running Card: status chips, branch and the pulsing Agent share one footer.
    s.cards[1].branch = Some("canban/gallery/2".into());
    let profile = Config::default()
        .profiles
        .into_iter()
        .find(|p| p.id == "developer")
        .unwrap();
    let mut running_attempt = canban_scheduler::agent_run(
        "gallery-project",
        "card-2",
        1,
        "develop",
        "stage_entry",
        profile.clone(),
        "Work on the Card".into(),
        None,
    );
    let base_now = now();
    running_attempt.state = "running".into();
    running_attempt.started_at = Some(base_now.saturating_sub(70 * 60 * 1000 + 30_000));
    running_attempt.updated_at = base_now.saturating_sub(70 * 60 * 1000 + 30_000);
    running_attempt.core_id = Some("gallery".into());
    running_attempt.terminal = Some("gallery-card-2-pty".into());
    running_attempt.cwd = "/preview/worktrees/card-2".into();
    running_attempt.input_path = "input.md".into();
    running_attempt.observed = Some(json!({"state": "live", "status": {"state": "working"}}));
    running_attempt.provider_session_id = Some("session-gallery-card-2".into());
    running_attempt.connection_state = "connected".into();
    running_attempt.lifecycle.observed_at = Some(u64::MAX);
    running_attempt.workflow_run_id = Some("gallery-card-2-run".into());
    running_attempt.workflow_step = Some("agent:develop:2".into());
    let card_2_tid = push_agent_terminal(&mut s, running_attempt);
    s.workflow_runs.push(
        serde_json::from_value(json!({
            "id": "gallery-card-2-run",
            "project_id": "gallery-project",
            "card_id": "card-2",
            "workflow_id": "delivery",
            "definition_revision": 1,
            "definition": {
                "id": "delivery",
                "name": "Development",
                "entries": {"backlog": "agent:plan", "develop": "script:check", "review": "deliver:review"},
                "steps": {
                    "agent:plan": {
                        "type": "agent",
                        "profile": "planner",
                        "instruction": "Plan",
                        "next": "script:check"
                    },
                    "script:check": {
                        "type": "script",
                        "run": "cargo test --workspace",
                        "shell": "bash",
                        "on_failure": "agent:develop",
                        "next": "agent:develop"
                    },
                    "agent:develop": {
                        "type": "agent",
                        "profile": "developer",
                        "instruction": "Implement",
                        "next": "deliver:review"
                    },
                    "deliver:review": {
                        "type": "deliver",
                        "sync_upstream": true,
                        "max_conflict_attempts": 3,
                        "next": "__end"
                    },
                    "__end": {"type": "complete"}
                }
            },
            "state": "waiting_agent",
            "step": "agent:develop",
            "visit": 3,
            "terminal_id": card_2_tid,
            "trigger_event_id": null,
            "input_digest": "gallery",
            "inputs": {"sync_upstream": true},
            "outputs": {
                "agent:plan": {"state": "completed"},
                "script:check": {"outcome": "success", "exit_code": 0, "path": "projects/gallery-project/cards/card-2/scripts/script-check-v2.log", "summary": "Script `script:check` succeeded"}
            },
            "history": [
                {"step": "agent:plan", "visit": 1, "event": "entered", "at": base_now.saturating_sub(74 * 60 * 1000 + 30_000)},
                {"step": "agent:plan", "visit": 1, "event": "completed", "at": base_now.saturating_sub(72 * 60 * 1000 + 30_000)},
                {"step": "script:check", "visit": 2, "event": "entered", "at": base_now.saturating_sub(72 * 60 * 1000 + 30_000)},
                {"step": "script:check", "visit": 2, "event": "completed", "at": base_now.saturating_sub(70 * 60 * 1000 + 30_000)},
                {"step": "agent:develop", "visit": 3, "event": "entered", "at": base_now.saturating_sub(70 * 60 * 1000 + 30_000)}
            ],
            "error": null,
            "created_at": base_now.saturating_sub(74 * 60 * 1000 + 30_000),
            "updated_at": base_now.saturating_sub(70 * 60 * 1000 + 30_000)
        }))
        .unwrap(),
    );
    s.terminals.push(TerminalEntry {
        id: "gallery-card-1-cli-agent".into(),
        conversation_id: None,
        number: 9,
        name: Some("Antigravity 1".into()),
        origin_project_id: Some("gallery-project".into()),
        origin_card_id: Some("card-1".into()),
        agent_engine: Some("agy".into()),
        runtime: Some(TerminalRuntime {
            core_id: Some("gallery".into()),
            terminal: Some("gallery-card-1-agy".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("idle".into()),
            agent_turn: Some(1),
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: base_now.saturating_sub(8 * 60 * 1000 + 30_000),
    });
    let mut queued = s.cards[2].clone();
    queued.id = "queued-card".into();
    queued.number = 5;
    queued.title = "Waiting for a free execution slot".into();
    queued.stage = "develop".into();
    let attempt = canban_scheduler::agent_run(
        "gallery-project",
        &queued.id,
        1,
        "develop",
        "stage_entry",
        profile,
        "Wait".into(),
        None,
    );
    push_agent_terminal(&mut s, attempt);
    s.cards.push(queued);
    let mut merging = s.cards[2].clone();
    merging.id = "merging-card".into();
    merging.number = 6;
    merging.title = "Queue merge into main · syncing upstream".into();
    merging.stage = "review".into();
    merging.branch = Some("canban/gallery/6".into());
    s.workflow_runs.push(
        serde_json::from_value(json!({
            "id": "gallery-merging-run",
            "project_id": "gallery-project",
            "card_id": "merging-card",
            "workflow_id": "delivery",
            "definition_revision": 1,
            "definition": {
                "id": "delivery",
                "name": "Development",
                "entries": {"review": "deliver:review"},
                "steps": {
                    "deliver:review": {
                        "type": "deliver",
                        "sync_upstream": true,
                        "max_conflict_attempts": 3,
                        "next": "__end"
                    },
                    "__end": {"type": "complete"}
                }
            },
            "state": "waiting_delivery",
            "step": "deliver:review",
            "visit": 1,
            "terminal_id": null,
            "trigger_event_id": null,
            "input_digest": "gallery",
            "inputs": {"sync_upstream": true, "queued_at": 100},
            "outputs": {},
            "history": [],
            "error": null,
            "created_at": 0,
            "updated_at": 0
        }))
        .unwrap(),
    );
    s.cards.push(merging);
    let mut conflict = s.cards[2].clone();
    conflict.id = "conflict-card".into();
    conflict.number = 8;
    conflict.title = "Resolve merge conflict with main via @merger".into();
    conflict.stage = "review".into();
    conflict.branch = Some("canban/gallery/8".into());
    let merger_profile = Config::default()
        .profiles
        .into_iter()
        .find(|p| p.id == "merger")
        .unwrap();
    let mut merger_attempt = canban_scheduler::agent_run(
        "gallery-project",
        &conflict.id,
        1,
        "review",
        "workflow",
        merger_profile,
        "Merge conflict to resolve before delivery".into(),
        None,
    );
    merger_attempt.state = "working".into();
    merger_attempt.started_at = Some(base_now.saturating_sub(3 * 60 * 1000 + 30_000));
    merger_attempt.updated_at = base_now.saturating_sub(3 * 60 * 1000 + 30_000);
    merger_attempt.workflow_run_id = Some("gallery-conflict-run".into());
    merger_attempt.workflow_step = Some("deliver:review:2".into());
    let merger_terminal_id = push_agent_terminal(&mut s, merger_attempt);
    s.workflow_runs.push(
        serde_json::from_value(json!({
            "id": "gallery-conflict-run",
            "project_id": "gallery-project",
            "card_id": "conflict-card",
            "workflow_id": "delivery",
            "definition_revision": 1,
            "definition": {
                "id": "delivery",
                "name": "Development",
                "entries": {"review": "deliver:review"},
                "steps": {
                    "agent:develop": {
                        "type": "agent",
                        "profile": "developer",
                        "instruction": "Implement",
                        "next": "deliver:review"
                    },
                    "deliver:review": {
                        "type": "deliver",
                        "sync_upstream": true,
                        "max_conflict_attempts": 3,
                        "next": "__end"
                    },
                    "__end": {"type": "complete"}
                }
            },
            "state": "waiting_agent",
            "step": "deliver:review",
            "visit": 2,
            "terminal_id": merger_terminal_id,
            "trigger_event_id": null,
            "input_digest": "gallery",
            "inputs": {"sync_upstream": true, "conflict_attempts": 1, "queued_at": 200},
            "outputs": {"agent:develop": {"state": "completed"}},
            "history": [
                {"step": "agent:develop", "visit": 1, "event": "entered", "at": base_now.saturating_sub(31 * 60 * 1000 + 30_000)},
                {"step": "agent:develop", "visit": 1, "event": "completed", "at": base_now.saturating_sub(3 * 60 * 1000 + 30_000)},
                {"step": "deliver:review", "visit": 2, "decision": "merge_conflict", "at": base_now.saturating_sub(3 * 60 * 1000 + 30_000)}
            ],
            "error": null,
            "created_at": base_now.saturating_sub(31 * 60 * 1000 + 30_000),
            "updated_at": base_now.saturating_sub(3 * 60 * 1000 + 30_000)
        }))
        .unwrap(),
    );
    s.cards.push(conflict);
    let mut queued_behind = s.cards[2].clone();
    queued_behind.id = "queued-merge-card".into();
    queued_behind.number = 9;
    queued_behind.title = "Queued behind active delivery · FIFO position #3".into();
    queued_behind.stage = "review".into();
    queued_behind.branch = Some("canban/gallery/9".into());
    s.workflow_runs.push(
        serde_json::from_value(json!({
            "id": "gallery-queued-merge-run",
            "project_id": "gallery-project",
            "card_id": "queued-merge-card",
            "workflow_id": "delivery",
            "definition_revision": 1,
            "definition": {
                "id": "delivery",
                "name": "Development",
                "entries": {"review": "deliver:review"},
                "steps": {
                    "deliver:review": {
                        "type": "deliver",
                        "strategy": "squash",
                        "sync_upstream": true,
                        "max_conflict_attempts": 3,
                        "next": "__end"
                    },
                    "__end": {"type": "complete"}
                }
            },
            "state": "waiting_delivery",
            "step": "deliver:review",
            "visit": 1,
            "terminal_id": null,
            "trigger_event_id": null,
            "input_digest": "gallery",
            "inputs": {"strategy": "squash", "sync_upstream": true, "queued_at": 300},
            "outputs": {},
            "history": [],
            "error": null,
            "created_at": 0,
            "updated_at": 0
        }))
        .unwrap(),
    );
    s.cards.push(queued_behind);
    let mut long = s.cards[2].clone();
    long.id = "long-card".into();
    long.number = 7;
    long.title="Keep a long Card title readable without repeating the stage, routing configuration or empty execution state".into();
    long.body = String::new();
    s.cards.push(long);
    let mut board_events = Vec::new();
    for (seq, card_id, kind, actor, message) in [
        (
            2,
            "card-1",
            "comment.replied",
            "agent:agy",
            "Proposed ASCII layout options for Board Card reply counts.",
        ),
        (
            3,
            "card-1",
            "comment.created",
            "user",
            "Go with Option A in the footer row; hide when 0.",
        ),
        (
            4,
            "card-2",
            "comment.created",
            "user",
            "@developer Check edge cases around echoed reports.",
        ),
        (
            5,
            "card-2",
            "comment.replied",
            "agent:developer",
            "Deduplicating echoed reports so Board matches Timeline.",
        ),
        (
            6,
            "card-3",
            "comment.created",
            "user",
            "Keep the collapsed rail at 56pt so Board columns stay roomy.",
        ),
    ] {
        board_events.push(Event {
            id: format!("board-reply-{seq}"),
            project_id: "gallery-project".into(),
            card_id: card_id.into(),
            sequence: seq,
            kind: kind.into(),
            actor: actor.into(),
            message: message.into(),
            data: json!({}),
            created_at: base_now.saturating_sub((10 - seq) * 60_000),
        });
    }
    apply_fixture_events(&mut s, &board_events);
    s
}

/// The production Card reader under long conversational history and background activity.
pub fn conversation() -> (canban_types::Snapshot, Vec<Event>) {
    let mut s = workspace();
    s.cards[0].body = "## Goal\nKeep the Card conversation readable while task context stays available.\n\n- [x] Full Markdown description\n- [ ] Keep selection and undo through background updates\n\n> Description, comments and events share one scroll owner.\n\n| Surface | Behavior |\n| --- | --- |\n| Editor | Write / Preview |\n| Agent | Type @ to choose |\n\n```rust\nlet context = card.history();\n```\n\n[Canban](https://github.com/jjyr/canban) · **Readable** and _editable_.\n".into();
    let base = canban_types::now();
    let mut seq = 1_u64;
    let mut events = Vec::new();
    let mut push = |kind: &str, actor: &str, msg: String, mins_ago: u64| {
        events.push(Event {
            id: format!("conversation-{seq}"),
            project_id: "gallery-project".into(),
            card_id: "card-1".into(),
            sequence: seq,
            kind: kind.into(),
            actor: actor.into(),
            message: msg,
            data: json!({}),
            created_at: base.saturating_sub(mins_ago * 60_000),
        });
        seq += 1;
    };

    // Filtered bookkeeping noise (never crowds the Timeline).
    push("card.created", "user", "Card created".into(), 120);
    push("card.updated", "user", "Card updated".into(), 118);
    push(
        "comment.created",
        "user",
        "请把时间线连起来，隐藏创建/更新噪音，并支持连续小状态折叠。".into(),
        90,
    );
    push("card.stage_changed", "user", "backlog → develop".into(), 88);
    push("execution.queued", "system", "Queued @developer".into(), 88);
    push(
        "execution.started",
        "system",
        "@developer started".into(),
        87,
    );
    push(
        "execution.reported",
        "agent:developer",
        "Implemented the connected vertical spine and collapsible milestone groups.\n\n- Filtered `card.created`, `card.updated` and heartbeat noise\n- Grouped adjacent transitions under a `+N earlier` pill".into(),
        60,
    );
    // Consecutive milestone run: collapses to the latest row (`Auto-merged into main · +3 earlier`).
    push(
        "card.stage_changed",
        "system",
        "develop → review".into(),
        58,
    );
    push(
        "github.pr_published",
        "system",
        "Opened PR #42 · Optimize Card timeline".into(),
        57,
    );
    push(
        "review.merge_conflict",
        "system",
        "Merge conflict with main; Agent resolved it (attempt 1/3)".into(),
        52,
    );
    push(
        "review.merged",
        "system",
        "Auto-merged into main and synced to origin/main".into(),
        48,
    );

    for i in 1..=20 {
        let agent_actor = match (i / 2) % 4 {
            1 => "agent:developer",
            2 => "agent:reviewer",
            3 => "agent:claude",
            _ => "agent:gemini",
        };
        push(
            if i == 7 || i == 15 {
                "execution.started"
            } else if i % 2 == 0 {
                "comment.replied"
            } else {
                "comment.created"
            },
            if i % 2 == 0 { agent_actor } else { "user" },
            if i == 7 || i == 15 {
                "Agent execution started".into()
            } else {
                format!(
                    "Message {i}: **conversation context**\n\n{}",
                    "Keep this reading position while new activity arrives. ".repeat(4)
                )
            },
            45_u64.saturating_sub(i * 2),
        );
    }
    if let Some(first_comment) = events
        .iter_mut()
        .find(|e| e.card_id == "card-1" && e.kind == "comment.created")
    {
        first_comment.data = json!({
            "body": first_comment.message,
            "revision": 2,
            "edited_at": base.saturating_sub(89 * 60_000),
        });
    }
    apply_fixture_events(&mut s, &events);
    (s, events)
}

/// Production GitHub surfaces with deterministic paginated and versioned fixtures.
pub fn github_browser(ui: canban_ui::presenter::Ui, pr: bool) {
    use canban_types::*;
    let mut snapshot = workspace();
    snapshot.github = GitHubAccount {
        state: "disconnected".into(),
        ..Default::default()
    };
    snapshot.projects[0].github = Some(GitHubSource {
        repository: "fixture/repo".into(),
        repository_id: 1,
        generation: "gallery".into(),
        intake: GitHubIntake::All,
        pr_intake: GitHubPrIntake::All,
        authors: vec![],
        labels: vec![],
        include_drafts: false,
        paused: false,
        synced_at: Some(now()),
        error: None,
    });
    snapshot.cards[0].content = CardContent::GitHubPr {
        source: GitHubIssueRef {
            repository: "fixture/repo".into(),
            repository_id: 1,
            issue_id: 102,
            number: 2,
            url: "https://github.com/fixture/repo/pull/2".into(),
        },
    };
    snapshot.cards[0].title = "Review the native diff".into();
    let pr_ref = GitHubReference {
        repository: "fixture/repo".into(),
        kind: "pr".into(),
        number: Some(2),
        sha: None,
        url: "https://github.com/fixture/repo/pull/2".into(),
    };
    let issue_ref = GitHubReference {
        repository: "fixture/repo".into(),
        kind: "issue".into(),
        number: Some(18),
        sha: None,
        url: "https://github.com/fixture/repo/issues/18".into(),
    };
    ui.snapshot.set(snapshot);
    ui.github
        .gh_status
        .set(json!({"state":"connected","login":"jjy","account_matches":true}));
    ui.github.cache_bytes.set(248_832);
    ui.github
        .tab
        .set(if pr { "prs" } else { "notifications" }.into());
    ui.github
        .state
        .set(if pr { "open" } else { "inbox" }.into());
    ui.github
        .relation
        .set(if pr { "review_requested" } else { "all" }.into());
    ui.github.list.set(json!({
        "items": [
            {
                "reference": pr_ref,
                "item": {
                    "id": "901",
                    "unread": !pr,
                    "reason": "review_requested",
                    "title": "Review the native diff",
                    "subject": {"title": "Review the native diff", "type": "PullRequest"},
                    "repository": {"full_name": "fixture/repo"},
                    "state": "open",
                    "html_url": "https://github.com/fixture/repo/pull/2",
                    "user": {"login": "octocat"},
                    "updated_at": "2026-11-12T08:15:00Z",
                    "labels": [{"name": "ui"}, {"name": "review"}],
                    "body": "### Summary\n\nUpgrades the **GitHub detail reader** and structured inline diff viewer.\n\n- Clickable `#1` Card references\n- Direct jump to Board Card"
                }
            },
            {
                "reference": issue_ref,
                "item": {
                    "id": "902",
                    "unread": !pr,
                    "reason": "mention",
                    "title": "Borderless section group for Overview & Settings",
                    "subject": {"title": "Borderless section group for Overview & Settings", "type": "Issue"},
                    "repository": {"full_name": "fixture/repo"},
                    "state": "open",
                    "html_url": "https://github.com/fixture/repo/issues/18",
                    "user": {"login": "designer"},
                    "updated_at": "2026-11-11T19:30:00Z",
                    "labels": [{"name": "design"}],
                    "body": "Use a clean **title + subtle bottom divider** without outer card borders so hierarchy stays crisp across Overview, GitHub, and Settings."
                }
            },
            {
                "reference": serde_json::Value::Null,
                "item": {
                    "id": "903",
                    "unread": false,
                    "reason": "subscribed",
                    "subject": {"title": "v0.9.0-rc1 desktop release published", "type": "Release"},
                    "repository": {"full_name": "fixture/repo"},
                    "updated_at": "2026-11-10T10:00:00Z"
                }
            }
        ],
        "page": 1,
        "has_next": true,
        "validated_at": now()
    }));
    if pr {
        ui.github.reference.set(Some(pr_ref));
        ui.github.section.set("files".into());
        ui.github.overview.set(json!({
            "number": 2,
            "title": "Review the native diff",
            "state": "open",
            "user": {"login": "octocat"},
            "updated_at": "2026-11-12T08:15:00Z",
            "labels": [{"name": "ui"}, {"name": "review"}],
            "body": "### Summary\n\nUpgrades the **GitHub detail reader** and structured inline diff viewer.",
            "head": {"sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "ref": "feature/github-ux"},
            "base": {"sha": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "ref": "main"}
        }));
        ui.github.detail.set(json!({
            "data": [
                {
                    "filename": "crates/ui-kit/src/section_group.rs",
                    "status": "added",
                    "additions": 4,
                    "deletions": 0,
                    "patch": "@@ -0,0 +1,4 @@\n+pub fn section_group(title: impl IntoText) -> SectionGroup {\n+    SectionGroup::new(title)\n+}\n+// Borderless section header + divider"
                },
                {
                    "filename": "crates/ui/src/github/workspace.rs",
                    "status": "modified",
                    "additions": 2,
                    "deletions": 2,
                    "patch": "@@ -231,3 +231,3 @@\n-action(\"Refresh\", Variant::Secondary, move |_| {})\n+chrome_button_disabled(Icon::Refresh, \"Refresh GitHub\", ...)\n context_line_stays_aligned();"
                }
            ],
            "page": 1,
            "has_next": false
        }));
    } else {
        ui.github.reference.set(None);
    }
    ui.navigate("github");
}

/// Same generic viewer with planning/approval and a conditional branch.
pub fn workflow() -> canban_types::WorkflowRun {
    serde_json::from_value(json!({
        "id":"gallery-run","project_id":"gallery","card_id":"gallery-card","workflow_id":"plan-delivery","definition_revision":2,
        "definition":{"id":"plan-delivery","name":"Plan and develop","entries":{"new_issue":"plan"},"steps":{
            "plan":{"type":"agent","profile":"planner","instruction":"Read and design","next":"approve"},
            "approve":{"type":"approval","title":"Approve plan v1","next":"choose","rework":"plan"},
            "choose":{"type":"condition","pointer":"/inputs/implement","equals":true,"then":"develop","otherwise":"done"},
            "develop":{"type":"move","column":"develop","next":"done"},"done":{"type":"complete"}}},
        "state":"waiting_approval","step":"approve","visit":2,"terminal_id":null,"trigger_event_id":null,
        "input_digest":"fixture","inputs":{"implement":true},"outputs":{"plan":{"message":"Plan v1: preserve existing runtime and add a regression.","report_id":"plan-v1"}},
        "history":[],"error":null,"created_at":0,"updated_at":0
    })).unwrap()
}

/// Active column rules across all columns (including Backlog next to `+` and custom `+N`
/// multi-workflow tooltips) over real Card text.
pub fn automation_board() -> canban_types::Snapshot {
    use canban_types::*;
    let mut snapshot = workspace();
    snapshot.workflow_sources.clear();
    snapshot.workflow_revisions.clear();
    let mut primary = canban_workflow_engine::source::default_document(
        "delivery:gallery-project",
        WorkspaceSettings::default(),
    );
    canban_workflow_engine::source::set_backlog_options(&mut primary, true, true);
    canban_workflow_engine::source::set_backlog_admission(&mut primary, true, Some(2));
    let primary_yaml = canban_workflow_engine::source::encode(&primary).unwrap();
    snapshot.workflow_revisions.push(WorkflowRevision {
        host_id: String::new(),
        source_yaml: Some(primary_yaml),
        id: "delivery:gallery-project:1".into(),
        project_id: "gallery-project".into(),
        workflow_id: "delivery:gallery-project".into(),
        revision: 1,
        definition: primary.compile().unwrap(),
        created_at: now(),
    });
    snapshot.workflow_sources.push(WorkflowSource {
        host_id: String::new(),
        id: "gallery-project:development.yaml".into(),
        project_id: "gallery-project".into(),
        filename: "development.yaml".into(),
        workflow_id: "delivery:gallery-project".into(),
        revision: 1,
        hash: "gallery".into(),
        revision_id: "delivery:gallery-project:1".into(),
        enabled: true,
        columns: primary.columns.clone(),
        rules: primary.on.clone(),
    });

    let raw = include_str!("../workflows/plan-and-develop.yaml");
    let document = canban_workflow_engine::source::parse(raw).unwrap();
    let workflow_id = "gallery-automation-extra".to_owned();
    let revision_id = format!("{workflow_id}:1");
    let mut definition = document.compile().unwrap();
    definition.id = workflow_id.clone();
    definition.name = "Extra Develop helper".into();
    snapshot.workflow_revisions.push(WorkflowRevision {
        host_id: String::new(),
        source_yaml: Some(raw.into()),
        id: revision_id.clone(),
        project_id: "gallery-project".into(),
        workflow_id: workflow_id.clone(),
        revision: 1,
        definition,
        created_at: now(),
    });
    snapshot.workflow_sources.push(WorkflowSource {
        host_id: String::new(),
        id: workflow_id.clone(),
        project_id: "gallery-project".into(),
        filename: "extra-develop.yaml".into(),
        workflow_id,
        revision: 1,
        hash: "gallery".into(),
        revision_id,
        enabled: true,
        columns: document.columns,
        rules: document.on,
    });
    snapshot
}

pub fn agent_config() -> canban_types::Config {
    use canban_types::*;
    let mut cfg = Config {
        onboarded: true,
        default_agent: "developer".into(),
        ..Default::default()
    };
    // Enable Antigravity (`agy`) alongside Codex and keep Claude disabled so Gallery
    // demonstrates enabled-agent gating + custom identities (`@qa`) + dual-role Orchestrator mode
    // (`@merger` uses `orchestrator` with Planner `Claude Opus 5.5 (high)` and Worker
    // `Gemini 4 Hydrogen (medium)` while `@reviewer` uses single-model `gemini-4-carbon` / `medium`).
    cfg.profiles.push(Profile {
        id: "agy".into(),
        enabled: true,
        preset: Some("agy".into()),
        model: Some("gemini-4-carbon".into()),
        effort: Some("medium".into()),
        ..Default::default()
    });
    if let Some(claude) = cfg.profiles.iter_mut().find(|p| p.id == "claude") {
        claude.enabled = false;
    }
    if let Some(rev) = cfg.profiles.iter_mut().find(|p| p.id == "reviewer") {
        rev.preset = Some("agy".into());
        rev.model = Some("gemini-4-carbon".into());
        rev.effort = Some("medium".into());
    }
    if let Some(merger) = cfg.profiles.iter_mut().find(|p| p.id == "merger") {
        merger.preset = Some("agy".into());
        merger.model = Some("orchestrator".into());
        merger.effort = None;
        merger.orchestrator = Some(OrchestratorConfig {
            planner_model: Some("claude-opus-5-5".into()),
            planner_effort: Some("high".into()),
            worker_model: Some("gemini-4-hydrogen".into()),
            worker_effort: Some("medium".into()),
        });
    }
    cfg.profiles.push(Profile {
        id: "qa".into(),
        enabled: true,
        identity: true,
        preset: Some("codex".into()),
        model: Some("gpt-6.1-sol".into()),
        effort: Some("high".into()),
        role_instruction: "Run end-to-end acceptance checks and verify edge cases.".into(),
        ..Default::default()
    });
    let tmp = std::env::temp_dir().join("canban-gallery-profiles-fixture");
    canban_profiles::resolve(cfg.clone(), &tmp).unwrap_or(cfg)
}

pub fn agent_catalog() -> serde_json::Value {
    let installed_bins = ["codex", "claude", "agy", "gemini"];
    let enabled_ids = ["codex", "agy"];
    let agents: Vec<serde_json::Value> = canban_profiles::builtin_catalog()
        .into_iter()
        .map(|entry| {
            let inst = installed_bins.contains(&entry.id.as_str());
            let en = enabled_ids.contains(&entry.id.as_str());
            json!({
                "id": entry.id,
                "display_name": entry.display_name,
                "binary": entry.binary,
                "default_argv": entry.argv,
                "kind": "builtin",
                "resumable": entry.resumable,
                "installed": inst,
                "path": if inst { json!(format!("/opt/homebrew/bin/{}", entry.binary)) } else { serde_json::Value::Null },
                "configured": inst,
                "enabled": en,
                "models": entry.default_models,
                "supported_efforts": entry.supported_efforts,
                "supports_model": !entry.model_args.is_empty(),
                "supports_effort": !entry.effort_args.is_empty(),
                "supports_orchestrator": entry.supports_orchestrator,
            })
        })
        .collect();
    json!({
        "machine": "local",
        "onboarded": true,
        "default_agent": "developer",
        "agents": agents,
        "herdr_templates": ["codex", "claude", "agy", "gemini", "opencode"],
        "raw_profiles": [
            { "id": "agy", "preset": "agy", "model": "gemini-4-carbon", "effort": "medium" },
            { "id": "reviewer", "identity": true, "preset": "agy" },
            {
                "id": "merger",
                "identity": true,
                "preset": "agy",
                "model": "orchestrator",
                "orchestrator": {
                    "planner_model": "claude-opus-5-5",
                    "planner_effort": "high",
                    "worker_model": "gemini-4-hydrogen",
                    "worker_effort": "medium"
                }
            },
            { "id": "qa", "identity": true, "preset": "codex", "model": "gpt-6.1-sol", "effort": "high" }
        ],
    })
}

pub fn paired_devices_preview() -> canban_ui::presenter::PairedDevicesPreview {
    use canban_types::*;
    let t = now();
    canban_ui::presenter::PairedDevicesPreview {
        devices: vec![
            PairedDevice {
                id: "dev-macbook".into(),
                name: "Work MacBook Pro".into(),
                created_at: t.saturating_sub(3_600_000),
                last_seen_at: Some(t.saturating_sub(90_000)),
                auth_kind: String::new(),
            },
            PairedDevice {
                id: "dev-iphone".into(),
                name: "iPhone 17".into(),
                created_at: t.saturating_sub(300_000),
                last_seen_at: None,
                auth_kind: String::new(),
            },
        ],
        has_legacy_token: true,
        tcp_enabled: true,
        listening: true,
        listen_addrs: vec!["100.84.0.2:7680".into()],
        trust_tailscale_self: true,
        generated_uri: Some("canban://100.84.0.2:7680#pair_demo9f8e7d6c5b4a3210".into()),
        generated_expires_at: Some(t + 240_000),
    }
}

pub fn discovery_catalog() -> canban_types::DiscoveryCatalog {
    use canban_types::*;
    DiscoveryCatalog {
        tailscale_available: true,
        tailscale_self_ip: Some("100.84.0.1".into()),
        tailscale_self_host: Some("macbook-pro".into()),
        tailscale_error: None,
        tailscale_peers: vec![
            TailscalePeer {
                hostname: "home-linux".into(),
                dns_name: "home-linux.tailnet.ts.net".into(),
                ips: vec!["100.84.0.2".into()],
                os: "linux".into(),
                online: true,
            },
            TailscalePeer {
                hostname: "gpu-workstation".into(),
                dns_name: "gpu-workstation.tailnet.ts.net".into(),
                ips: vec!["100.84.0.9".into()],
                os: "linux".into(),
                online: false,
            },
        ],
        ssh_hosts: vec![
            SshHostEntry {
                alias: "home-linux".into(),
                hostname: "100.84.0.2".into(),
                user: Some("builder".into()),
                port: None,
            },
            SshHostEntry {
                alias: "prod-bastion".into(),
                hostname: "203.0.113.42".into(),
                user: Some("ubuntu".into()),
                port: Some(2222),
            },
        ],
    }
}

pub fn multi_host_workspace(
    offline_remote: bool,
) -> (canban_types::Snapshot, Vec<canban_types::HostSummary>) {
    use canban_types::*;
    let mut local = workspace();
    let mut local_fiber = local.projects[0].clone();
    local_fiber.id = "fiber".into();
    local_fiber.name = "fiber".into();
    local_fiber.directory = "/Users/jjy/Workspace/fiber".into();
    local.projects.push(local_fiber);
    local.register_host_entities("local");

    let mut remote = Snapshot::default();
    let mut remote_proj = local.projects[0].clone();
    remote_proj.id = "remote-linux-core".into();
    remote_proj.name = "Core Service".into();
    remote_proj.directory = "/home/builder/workspaces/core-service".into();
    remote.projects.push(remote_proj);

    let mut remote_fiber = local.projects[0].clone();
    remote_fiber.id = "fiber".into();
    remote_fiber.name = "fiber".into();
    remote_fiber.directory = "/home/builder/workspaces/fiber".into();
    remote.projects.push(remote_fiber);

    let mut remote_card = local.cards[0].clone();
    remote_card.id = "remote-card-1".into();
    remote_card.project_id = "remote-linux-core".into();
    remote_card.number = 12;
    remote_card.title = "Autonomous overnight workflow execution on Home Linux".into();
    remote.cards.push(remote_card);

    let mut remote_fiber_card = local.cards[0].clone();
    remote_fiber_card.id = "fiber-card-1".into();
    remote_fiber_card.project_id = "fiber".into();
    remote_fiber_card.number = 1;
    remote_fiber_card.title = "Cross-host fiber benchmark".into();
    remote.cards.push(remote_fiber_card);

    remote.schedule_states.push(ScheduleState {
        host_id: String::new(),
        id: "schedule:__global:global:worktree-gc:schedule".into(),
        project_id: "__global".into(),
        workflow_id: "global:worktree-gc".into(),
        workflow_name: "Worktree GC".into(),
        filename: "worktree-gc.yaml".into(),
        rule_id: "schedule".into(),
        cron: "0 3 * * *".into(),
        timezone: None,
        overlap: ScheduleOverlap::Skip,
        creates_card: false,
        starts_with_script: true,
        target_column: None,
        profile_id: None,
        enabled: true,
        last_scheduled_at: None,
        next_run_at: Some(now().saturating_add(3_600_000)),
        last_run_id: None,
        run_count: 0,
        queued_triggers: 0,
        updated_at: now(),
    });

    let local_summary = HostSummary {
        id: "local".into(),
        name: canban_ui::platform::LOCAL_HOST_NAME.into(),
        is_local: true,
        transport_label: "Local socket".into(),
        endpoint_label: "~/.canban/app.sock".into(),
        ssh_target: None,
        server_id: Some("srv-local-mac".into()),
        connection: HostConnectionState::Connected {
            rtt_ms: 0,
            api_version: canban_updater::DESKTOP_API,
            version: "20261115-1".into(),
            platform: "macos-aarch64".into(),
        },
        active_agents: 1,
        max_agents: 8,
        project_count: local.projects.len(),
        reconnect_count: 0,
        last_synced_at: Some(now()),
        pending_mutations: 0,
        sync_conflicts: 0,
    };

    let remote_summary = HostSummary {
        id: "home-linux".into(),
        name: "Home Linux".into(),
        is_local: false,
        transport_label: "Tailscale / TCP".into(),
        endpoint_label: "100.84.0.2:7680".into(),
        ssh_target: Some("builder@100.84.0.2".into()),
        server_id: Some("srv-home-linux".into()),
        connection: if offline_remote {
            HostConnectionState::Offline {
                error: "Tailscale tunnel asleep while laptop was closed".into(),
                cached_at: Some(now().saturating_sub(300_000)),
            }
        } else {
            HostConnectionState::Connected {
                rtt_ms: 9,
                api_version: canban_updater::DESKTOP_API,
                version: "20261115-1".into(),
                platform: "linux-x86_64".into(),
            }
        },
        active_agents: 2,
        max_agents: 12,
        project_count: remote.projects.len(),
        reconnect_count: if offline_remote { 2 } else { 0 },
        last_synced_at: Some(now().saturating_sub(if offline_remote { 300_000 } else { 800 })),
        pending_mutations: 0,
        sync_conflicts: 0,
    };

    local
        .host_summaries
        .insert("local".into(), local_summary.clone());
    local.merge_remote_snapshot(remote, remote_summary.clone());

    (local, vec![local_summary, remote_summary])
}

pub fn sync_preview_items() -> Vec<canban_types::SyncItemView> {
    use std::collections::BTreeMap;
    let mut mine = BTreeMap::new();
    mine.insert(
        "title".into(),
        "Offline edited title · keep local workflow changes".into(),
    );
    let mut server = BTreeMap::new();
    server.insert(
        "title".into(),
        "Autonomous overnight workflow execution on Home Linux".into(),
    );
    vec![
        canban_types::SyncItemView {
            host_id: "home-linux".into(),
            host_name: "Home Linux".into(),
            mutation_id: "gallery-sync-fail-1".into(),
            card_id: "remote-card-move".into(),
            card_title: "Drafted offline".into(),
            kind: "Move Card".into(),
            state: canban_types::SyncItemState::Failed,
            message: "Target stage `qa` was removed on Home Linux".into(),
            conflict_fields: Vec::new(),
            mine: BTreeMap::new(),
            server: BTreeMap::new(),
        },
        canban_types::SyncItemView {
            host_id: "home-linux".into(),
            host_name: "Home Linux".into(),
            mutation_id: "gallery-sync-conflict-1".into(),
            card_id: "remote-card-1".into(),
            card_title: "Autonomous overnight workflow execution on Home Linux".into(),
            kind: "Edit Card".into(),
            state: canban_types::SyncItemState::Conflict,
            message: "Conflict on title".into(),
            conflict_fields: vec!["title".into()],
            mine,
            server,
        },
    ]
}

pub const INSIGHTS_NOW: u64 = 1_794_900_600_000;

pub fn insights() -> (canban_types::Snapshot, Vec<Event>) {
    use canban_types::*;
    let base_now = INSIGHTS_NOW;
    let day_ms: u64 = 86_400_000;
    let hour_ms: u64 = 3_600_000;
    let min_ms: u64 = 60_000;

    let mut s = workspace();
    let mut events = Vec::new();
    s.projects[0].name = "Canban".into();
    let mut second_proj = s.projects[0].clone();
    second_proj.id = "herdr-core".into();
    second_proj.name = "Herdr Core".into();
    second_proj.directory = "/Users/demo/Workspace/herdr".into();
    s.projects.push(second_proj);

    let titles: &[(&str, bool)] = &[
        (
            "任务完成情况统计页面 · Insights throughput & scheduling charts",
            false,
        ),
        ("Unified Diff & Git Compare searchable ref popovers", true),
        ("Multi-host Tailscale snapshot merge & offline cache", false),
        (
            "Preserve Board stage scroll across Card split transitions",
            false,
        ),
        ("Herdr PTY stream reconnect & wide-glyph clipping fix", true),
        ("Workflow Studio visual column automation badges", false),
        ("GitHub PR inline review thread sync & batch submit", true),
        ("SQLite WAL checkpoint diagnostics & server log tail", false),
        ("Markdown rope editor CJK + Menlo caret alignment", false),
        ("FIFO merge queue conflict recovery with @merger", false),
    ];

    let mut seq = 500u64;
    let mut card_num = 10u64;

    // Populate a full year across both Projects for cadence and date navigation.
    for days_ago in (0u64..365).rev() {
        let pattern = (days_ago * 7 + 3) % 11;
        // Occasional quiet weekend-like days
        if pattern == 0 || pattern == 5 {
            continue;
        }
        let cards_today = match pattern {
            1 | 2 | 6 => 1usize,
            3 | 7 | 9 => 2usize,
            _ => 3usize,
        };
        let day_anchor = base_now.saturating_sub(days_ago * day_ms + 6 * hour_ms);

        for slot in 0..cards_today {
            card_num += 1;
            let (base_title, is_pr) = titles[(card_num as usize) % titles.len()];
            let pid = if (card_num + slot as u64).is_multiple_of(3) {
                "herdr-core"
            } else {
                "gallery-project"
            };
            let cid = format!("insights-card-{card_num}");
            let created_at = day_anchor.saturating_sub(((slot as u64) + 2) * 3 * hour_ms);
            let work_start = created_at + 45 * min_ms;

            let mut card = s.cards[0].clone();
            card.id = cid.clone();
            card.project_id = pid.into();
            card.number = card_num;
            card.title = base_title.into();
            card.created_at = created_at;
            card.attention = None;
            card.human_gate = None;
            if !is_pr {
                card.pull_request = None;
            }

            // Keep a couple of unmerged archived cards so Gallery verifies merge-only counting
            let unmerged_archived = card_num == 24 || card_num == 51;
            // Keep a couple of recent cards active (unmerged)
            let still_active = days_ago <= 1 && slot == cards_today - 1;

            if unmerged_archived {
                card.stage = "backlog".into();
                card.archived_at = Some(day_anchor + (slot as u64) * hour_ms);
                card.delivery = None;
            } else if still_active {
                card.stage = "develop".into();
                card.archived_at = None;
                card.delivery = None;
            } else {
                let lead_hours = if days_ago <= 4 {
                    (card_num % 2) + 1
                } else {
                    ((card_num % 7) + 2) * 2
                };
                let merged_at =
                    (work_start + lead_hours * hour_ms).min(base_now.saturating_sub(60_000));
                card.stage = "review".into();
                card.archived_at = Some(merged_at);
                card.delivery = Some(Delivery {
                    target_branch: "main".into(),
                    commit: format!("{:07x}", card_num * 9973),
                    request_key: if is_pr {
                        format!("github:jjyr/canban#{card_num}")
                    } else {
                        format!("merge:{cid}")
                    },
                    merged_at,
                });
            }
            s.cards.push(card);

            seq += 1;
            events.push(Event {
                id: format!("ins-ev-{seq}"),
                project_id: pid.into(),
                card_id: cid.clone(),
                sequence: seq,
                kind: "card.stage_changed".into(),
                actor: "user".into(),
                message: "backlog → develop".into(),
                data: json!({"from": "backlog", "to": "develop"}),
                created_at: work_start,
            });

            if unmerged_archived {
                continue;
            }

            if !still_active {
                let merged_at = s
                    .cards
                    .last()
                    .and_then(|c| c.delivery.as_ref().map(|d| d.merged_at))
                    .unwrap_or(work_start + 2 * hour_ms);
                let review_at = (work_start + (35 + (card_num % 4) * 12) * min_ms)
                    .min(merged_at.saturating_sub(15 * min_ms));
                let mq_queued_at = merged_at.saturating_sub(((card_num % 5) + 2) * min_ms);

                seq += 1;
                events.push(Event {
                    id: format!("ins-ev-{seq}"),
                    project_id: pid.into(),
                    card_id: cid.clone(),
                    sequence: seq,
                    kind: "card.stage_changed".into(),
                    actor: "system".into(),
                    message: "develop → review".into(),
                    data: json!({"from": "develop", "to": "review"}),
                    created_at: review_at,
                });

                if !is_pr {
                    seq += 1;
                    events.push(Event {
                        id: format!("ins-ev-{seq}"),
                        project_id: pid.into(),
                        card_id: cid.clone(),
                        sequence: seq,
                        kind: "review.merged".into(),
                        actor: "system".into(),
                        message: "Merged batch into main".into(),
                        data: json!({
                            "commit": format!("{:07x}", card_num * 9973),
                            "target_branch": "main",
                            "batch_id": format!("batch-{days_ago}-{}", slot / 2),
                            "batch_size": if slot.is_multiple_of(2) { 2 } else { 1 },
                            "queued_at": mq_queued_at,
                        }),
                        created_at: merged_at,
                    });
                }

                if card_num.is_multiple_of(3) {
                    let comment_ts = review_at + 3 * min_ms;
                    let reply_ts = (comment_ts + ((card_num % 4) + 1) * 90_000).min(merged_at);
                    let comment_ev_id = format!("ins-ev-{}", seq + 1);
                    seq += 1;
                    events.push(Event {
                        id: comment_ev_id.clone(),
                        project_id: pid.into(),
                        card_id: cid.clone(),
                        sequence: seq,
                        kind: "comment.created".into(),
                        actor: "user".into(),
                        message: "@reviewer please double-check edge cases".into(),
                        data: json!({"body": "@reviewer please double-check edge cases"}),
                        created_at: comment_ts,
                    });
                    seq += 1;
                    events.push(Event {
                        id: format!("ins-ev-{seq}"),
                        project_id: pid.into(),
                        card_id: cid.clone(),
                        sequence: seq,
                        kind: "comment.replied".into(),
                        actor: "agent:reviewer".into(),
                        message: "Checked and verified.".into(),
                        data: json!({"reply_to": comment_ev_id}),
                        created_at: reply_ts,
                    });
                }
            }

            // Schedule 1-2 Agent runs on this card
            let profiles_for_card: &[(&str, &str, &str)] = match card_num % 5 {
                0 => &[
                    ("planner", "workflow", "completed"),
                    ("developer", "workflow", "completed"),
                ],
                1 => &[
                    ("developer", "workflow", "completed"),
                    ("reviewer", "comment:rev", "completed"),
                ],
                2 => &[("developer", "comment:ask", "blocked")],
                3 => &[(
                    "developer",
                    "interactive",
                    if days_ago < 20 { "failed" } else { "completed" },
                )],
                _ => &[(
                    "developer",
                    "workflow",
                    if still_active { "running" } else { "completed" },
                )],
            };

            for (r_idx, &(prof, trig, outcome)) in profiles_for_card.iter().enumerate() {
                let run_num = (r_idx + 1) as u32;
                let q_at = day_anchor + (slot as u64) * 35 * min_ms + (r_idx as u64) * 25 * min_ms;
                let q_wait = ((card_num % 4) + 1) * 8_000;
                let s_at = q_at + q_wait;
                let tid = format!("{cid}:{prof}");
                let has_wf = trig == "workflow";

                seq += 1;
                events.push(Event {
                    id: format!("ins-ev-{seq}"),
                    project_id: pid.into(),
                    card_id: cid.clone(),
                    sequence: seq,
                    kind: "execution.queued".into(),
                    actor: if trig.starts_with("comment:") { "user" } else { "system" }.into(),
                    message: format!("Queued @{prof}"),
                    data: json!({
                        "terminal_id": tid,
                        "run_number": run_num,
                        "trigger": trig,
                        "origin": if trig.starts_with("comment:") { "comment" } else if trig == "interactive" { "manual" } else { "workflow" },
                        "workflow_run_id": has_wf.then(|| format!("wf-{cid}")),
                    }),
                    created_at: q_at,
                });

                seq += 1;
                events.push(Event {
                    id: format!("ins-ev-{seq}"),
                    project_id: pid.into(),
                    card_id: cid.clone(),
                    sequence: seq,
                    kind: "execution.started".into(),
                    actor: "system".into(),
                    message: format!("Started @{prof}"),
                    data: json!({
                        "terminal_id": tid,
                        "run_number": run_num,
                        "trigger": trig,
                        "workflow_run_id": has_wf.then(|| format!("wf-{cid}")),
                    }),
                    created_at: s_at,
                });

                if outcome == "failed" {
                    seq += 1;
                    events.push(Event {
                        id: format!("ins-ev-{seq}"),
                        project_id: pid.into(),
                        card_id: cid.clone(),
                        sequence: seq,
                        kind: "execution.failed".into(),
                        actor: "system".into(),
                        message: "Agent process exited unexpectedly".into(),
                        data: json!({
                            "terminal_id": tid,
                            "run_number": run_num,
                        }),
                        created_at: s_at + 12 * min_ms,
                    });
                } else if outcome != "running" {
                    seq += 1;
                    events.push(Event {
                        id: format!("ins-ev-{seq}"),
                        project_id: pid.into(),
                        card_id: cid.clone(),
                        sequence: seq,
                        kind: "execution.reported".into(),
                        actor: format!("agent:{prof}"),
                        message: format!("Reported {outcome}"),
                        data: json!({
                            "state": outcome,
                            "workflow_run_id": has_wf.then(|| format!("wf-{cid}")),
                        }),
                        created_at: s_at + 18 * min_ms,
                    });
                }
            }
        }
    }

    // Include two @merger conflict-resolution batch runs in the past two weeks
    for (b_idx, d_ago) in [(1u64, 2u64), (2u64, 9u64)] {
        let ts = base_now.saturating_sub(d_ago * day_ms + 3 * hour_ms);
        s.merge_batches.push(MergeBatch {
            id: format!("merge-batch:ins-{b_idx}"),
            project_id: "gallery-project".into(),
            profile_id: "merger".into(),
            provider_session_id: None,
            target_branch: "main".into(),
            target_head: "a1b2c3d".into(),
            strategy: Default::default(),
            sync_upstream: true,
            test_command: Some("cargo nextest run".into()),
            worktree: "/preview/worktrees/merge-batch".into(),
            state: "completed".into(),
            failure_kind: Some("conflict".into()),
            conflict_card_number: Some(card_num),
            conflict_files: vec!["crates/ui/src/rail.rs".into()],
            test_output: None,
            items: Vec::new(),
            prompt: "Resolve conflict in crates/ui/src/rail.rs".into(),
            error: None,
            queued_at: Some(ts),
            started_at: Some(ts + min_ms),
            created_at: ts,
            updated_at: ts + 6 * min_ms,
        });
    }

    // Add a few bulk initial-sync GitHub issues so Gallery demonstrates organic vs sync separation
    for sync_idx in 1..=12u64 {
        card_num += 1;
        let cid = format!("insights-sync-{sync_idx}");
        let sync_ts = base_now.saturating_sub(2 * hour_ms);
        let mut card = s.cards[0].clone();
        card.id = cid.clone();
        card.project_id = "herdr-core".into();
        card.number = card_num;
        card.title = format!("Synced upstream issue #{sync_idx}");
        card.stage = "backlog".into();
        card.created_at = sync_ts;
        card.archived_at = None;
        card.delivery = None;
        card.attention = None;
        card.human_gate = None;
        s.cards.push(card);
        seq += 1;
        events.push(Event {
            id: format!("ins-ev-{seq}"),
            project_id: "herdr-core".into(),
            card_id: cid,
            sequence: seq,
            kind: "card.created".into(),
            actor: "github".into(),
            message: "Card created".into(),
            data: serde_json::json!({
                "initial_sync": true,
                "origin": "github",
                "reason": "automatic_intake",
                "source_kind": "github.issue",
            }),
            created_at: sync_ts,
        });
    }

    apply_fixture_events(&mut s, &events);
    (s, events)
}

pub fn insights_activity() -> canban_ui::insights::model::ActivityInput {
    use canban_types::{
        ActivityBucket, ActivityDetail, ActivityKind, ActivitySegment, AgentBucketStat,
        AgentWorkData, DwellBucketTotals, DwellData, DwellLane, ProjectActivityTotals,
    };
    use canban_ui::insights::model::{ActivityInput, Comparison, RangeWindow, activity_fetch_plan};
    use std::collections::BTreeMap;

    let base_now = INSIGHTS_NOW;
    let plan = activity_fetch_plan(
        RangeWindow::Days90,
        base_now,
        &chrono::Local,
        None,
        Comparison::Previous,
    );
    let min_ms: u64 = 60_000;
    let (snap, _) = insights();
    let card_gallery = snap
        .cards
        .iter()
        .rfind(|c| c.project_id == "gallery-project" && !c.id.starts_with("insights-sync-"))
        .map(|c| c.id.clone())
        .unwrap_or_else(|| "card-1".into());
    let card_herdr = snap
        .cards
        .iter()
        .rfind(|c| c.project_id == "herdr-core" && !c.id.starts_with("insights-sync-"))
        .map(|c| c.id.clone())
        .unwrap_or_else(|| "card-2".into());

    let today_start = plan
        .range_boundaries
        .get(plan.range_boundaries.len().saturating_sub(2))
        .copied()
        .unwrap_or_else(|| base_now.saturating_sub(8 * 3_600_000));
    let span = base_now.saturating_sub(today_start).max(1);
    let seg_bounds = |frac_start: f64, frac_end: f64| -> (u64, u64) {
        let a = today_start + ((span as f64) * frac_start) as u64;
        let b = today_start + ((span as f64) * frac_end) as u64;
        (a, b.max(a + 60_000).min(base_now))
    };

    let (dc1_a, dc1_b) = seg_bounds(0.18, 0.34);
    let (dc2_a, dc2_b) = seg_bounds(0.54, 0.72);
    let (dt1_a, dt1_b) = seg_bounds(0.36, 0.50);
    let (do1_a, do1_b) = seg_bounds(0.75, 0.84);
    let (ag1_a, ag1_b) = seg_bounds(0.20, 0.44);
    let (ag2_a, ag2_b) = seg_bounds(0.46, 0.68);
    let (ag3_a, ag3_b) = seg_bounds(0.76, 1.00);

    let period_segments = vec![
        ActivitySegment {
            id: Some(1),
            kind: ActivityKind::Dwell,
            start_ms: dc1_a,
            end_ms: dc1_b,
            project_id: Some("gallery-project".into()),
            card_id: Some(card_gallery.clone()),
            terminal_id: None,
            open: false,
            data: ActivityDetail::Dwell(DwellData {
                lane: DwellLane::Card,
                page: "card".into(),
            }),
        },
        ActivitySegment {
            id: Some(2),
            kind: ActivityKind::Dwell,
            start_ms: dt1_a,
            end_ms: dt1_b,
            project_id: Some("herdr-core".into()),
            card_id: Some(card_herdr.clone()),
            terminal_id: Some("shell-herdr-dev".into()),
            open: false,
            data: ActivityDetail::Dwell(DwellData {
                lane: DwellLane::Terminal,
                page: "terminals".into(),
            }),
        },
        ActivitySegment {
            id: Some(3),
            kind: ActivityKind::Dwell,
            start_ms: dc2_a,
            end_ms: dc2_b,
            project_id: Some("gallery-project".into()),
            card_id: Some(card_gallery.clone()),
            terminal_id: None,
            open: false,
            data: ActivityDetail::Dwell(DwellData {
                lane: DwellLane::Card,
                page: "card".into(),
            }),
        },
        ActivitySegment {
            id: Some(4),
            kind: ActivityKind::Dwell,
            start_ms: do1_a,
            end_ms: do1_b,
            project_id: None,
            card_id: None,
            terminal_id: None,
            open: false,
            data: ActivityDetail::Dwell(DwellData {
                lane: DwellLane::Other,
                page: "board".into(),
            }),
        },
        ActivitySegment {
            id: Some(5),
            kind: ActivityKind::Agent,
            start_ms: ag1_a,
            end_ms: ag1_b,
            project_id: Some("gallery-project".into()),
            card_id: Some(card_gallery.clone()),
            terminal_id: Some(format!("{card_gallery}:developer")),
            open: false,
            data: ActivityDetail::Agent(AgentWorkData {
                profile_id: "developer".into(),
                provider: "codex".into(),
                model: "gpt-5.4".into(),
                engine: Some("codex".into()),
                stage: Some("develop".into()),
                run_number: Some(1),
                trigger: Some("workflow".into()),
                manual: false,
            }),
        },
        ActivitySegment {
            id: Some(6),
            kind: ActivityKind::Agent,
            start_ms: ag2_a,
            end_ms: ag2_b,
            project_id: Some("herdr-core".into()),
            card_id: Some(card_herdr.clone()),
            terminal_id: Some(format!("{card_herdr}:reviewer")),
            open: false,
            data: ActivityDetail::Agent(AgentWorkData {
                profile_id: "reviewer".into(),
                provider: "claude".into(),
                model: "claude-sonnet-4-6".into(),
                engine: Some("claude".into()),
                stage: Some("review".into()),
                run_number: Some(1),
                trigger: Some("comment:rev".into()),
                manual: false,
            }),
        },
        ActivitySegment {
            id: Some(7),
            kind: ActivityKind::Agent,
            start_ms: ag3_a,
            end_ms: ag3_b,
            project_id: Some("gallery-project".into()),
            card_id: Some(card_gallery.clone()),
            terminal_id: Some(format!("{card_gallery}:planner")),
            open: true,
            data: ActivityDetail::Agent(AgentWorkData {
                profile_id: "planner".into(),
                provider: "codex".into(),
                model: "gpt-5.4".into(),
                engine: Some("codex".into()),
                stage: Some("backlog".into()),
                run_number: Some(1),
                trigger: Some("workflow".into()),
                manual: false,
            }),
        },
    ];

    let day_ms: u64 = 86_400_000;
    let yesterday_start = plan
        .annual_boundaries
        .get(plan.annual_boundaries.len().saturating_sub(3))
        .copied()
        .unwrap_or_else(|| today_start.saturating_sub(day_ms));
    let shift_to_yesterday =
        |ts: u64| yesterday_start.saturating_add(ts.saturating_sub(today_start));

    let baseline_segments = vec![
        ActivitySegment {
            id: Some(8),
            kind: ActivityKind::Dwell,
            start_ms: shift_to_yesterday(dc1_a),
            end_ms: shift_to_yesterday(dc1_b),
            project_id: Some("gallery-project".into()),
            card_id: Some(card_gallery.clone()),
            terminal_id: None,
            open: false,
            data: ActivityDetail::Dwell(DwellData {
                lane: DwellLane::Card,
                page: "card".into(),
            }),
        },
        ActivitySegment {
            id: Some(9),
            kind: ActivityKind::Agent,
            start_ms: shift_to_yesterday(ag1_a),
            end_ms: shift_to_yesterday(ag1_b),
            project_id: Some("gallery-project".into()),
            card_id: Some(card_gallery),
            terminal_id: None,
            open: false,
            data: ActivityDetail::Agent(AgentWorkData {
                profile_id: "developer".into(),
                provider: "codex".into(),
                model: "gpt-5.4".into(),
                engine: Some("codex".into()),
                stage: Some("develop".into()),
                run_number: Some(1),
                trigger: Some("workflow".into()),
                manual: false,
            }),
        },
    ];

    let aggregate_bucket =
        |start_ms: u64, end_ms: u64, segments: &[ActivitySegment]| -> ActivityBucket {
            let mut dwell = DwellBucketTotals::default();
            let mut agent_ms = 0u64;
            let mut by_project: BTreeMap<String, ProjectActivityTotals> = BTreeMap::new();
            let mut by_agent_map: BTreeMap<(String, String, String), u64> = BTreeMap::new();

            for seg in segments {
                let overlap = seg
                    .end_ms
                    .min(end_ms)
                    .saturating_sub(seg.start_ms.max(start_ms));
                if overlap == 0 {
                    continue;
                }
                let pid = seg.project_id.as_deref().filter(|s| !s.is_empty());
                match (&seg.kind, &seg.data) {
                    (ActivityKind::Dwell, ActivityDetail::Dwell(d)) => {
                        match d.lane {
                            DwellLane::Card => dwell.card += overlap,
                            DwellLane::Terminal => dwell.terminal += overlap,
                            DwellLane::Other => dwell.other += overlap,
                        }
                        if let Some(pid) = pid {
                            by_project.entry(pid.to_owned()).or_default().dwell_ms += overlap;
                        }
                    }
                    (ActivityKind::Agent, ActivityDetail::Agent(a)) => {
                        agent_ms += overlap;
                        if let Some(pid) = pid {
                            by_project.entry(pid.to_owned()).or_default().agent_ms += overlap;
                        }
                        *by_agent_map
                            .entry((a.profile_id.clone(), a.provider.clone(), a.model.clone()))
                            .or_default() += overlap;
                    }
                    _ => {}
                }
            }

            let mut by_agent: Vec<AgentBucketStat> = by_agent_map
                .into_iter()
                .map(|((profile_id, provider, model), ms)| AgentBucketStat {
                    profile_id,
                    provider,
                    model,
                    ms,
                })
                .collect();
            by_agent.sort_by(|a, b| {
                b.ms.cmp(&a.ms)
                    .then_with(|| a.profile_id.cmp(&b.profile_id))
                    .then_with(|| a.provider.cmp(&b.provider))
                    .then_with(|| a.model.cmp(&b.model))
            });

            ActivityBucket {
                start_ms,
                end_ms,
                dwell,
                agent_ms,
                by_project,
                by_agent,
            }
        };

    let make_buckets = |boundaries: &[u64]| -> Vec<ActivityBucket> {
        let len = boundaries.len().saturating_sub(1);
        boundaries
            .windows(2)
            .enumerate()
            .map(|(i, w)| {
                let rev_idx = len.saturating_sub(1 + i) as u64;
                let pattern = (rev_idx * 7 + 3) % 11;
                if pattern == 0 || pattern == 5 {
                    return ActivityBucket {
                        start_ms: w[0],
                        end_ms: w[1],
                        ..Default::default()
                    };
                }
                let card = (25 + (rev_idx % 4) * 12) * min_ms;
                let terminal = (15 + (rev_idx % 3) * 10) * min_ms;
                let other = (6 + (rev_idx % 2) * 5) * min_ms;
                let dev_ms = (32 + (rev_idx % 5) * 14) * min_ms;
                let rev_ms = (14 + (rev_idx % 3) * 8) * min_ms;
                let plan_ms = if rev_idx.is_multiple_of(2) {
                    12 * min_ms
                } else {
                    0
                };
                let agent_ms = dev_ms + rev_ms + plan_ms;
                let g_dwell = card + terminal * 2 / 5;
                let h_dwell = terminal - terminal * 2 / 5;
                let g_agent = dev_ms + plan_ms;
                let h_agent = rev_ms;
                let mut by_agent = vec![
                    AgentBucketStat {
                        profile_id: "developer".into(),
                        provider: "codex".into(),
                        model: "gpt-5.4".into(),
                        ms: dev_ms,
                    },
                    AgentBucketStat {
                        profile_id: "reviewer".into(),
                        provider: "claude".into(),
                        model: "claude-sonnet-4-6".into(),
                        ms: rev_ms,
                    },
                ];
                if plan_ms > 0 {
                    by_agent.push(AgentBucketStat {
                        profile_id: "planner".into(),
                        provider: "codex".into(),
                        model: "gpt-5.4".into(),
                        ms: plan_ms,
                    });
                }
                ActivityBucket {
                    start_ms: w[0],
                    end_ms: w[1],
                    dwell: DwellBucketTotals {
                        card,
                        terminal,
                        other,
                    },
                    agent_ms,
                    by_project: BTreeMap::from([
                        (
                            "gallery-project".into(),
                            ProjectActivityTotals {
                                dwell_ms: g_dwell,
                                agent_ms: g_agent,
                            },
                        ),
                        (
                            "herdr-core".into(),
                            ProjectActivityTotals {
                                dwell_ms: h_dwell,
                                agent_ms: h_agent,
                            },
                        ),
                    ]),
                    by_agent,
                }
            })
            .collect()
    };

    let mut annual_buckets = make_buckets(&plan.annual_boundaries);
    let len = annual_buckets.len();
    if len >= 2 {
        let y_b = &annual_buckets[len - 2];
        annual_buckets[len - 2] = aggregate_bucket(y_b.start_ms, y_b.end_ms, &baseline_segments);
    }
    if len >= 1 {
        let t_b = &annual_buckets[len - 1];
        annual_buckets[len - 1] = aggregate_bucket(t_b.start_ms, t_b.end_ms, &period_segments);
    }
    ActivityInput {
        period_segments,
        baseline_segments,
        range_buckets: annual_buckets.clone(),
        baseline_buckets: annual_buckets.clone(),
        annual_buckets,
        host_totals: std::collections::BTreeMap::new(),
        loaded: true,
    }
}

pub fn schedules() -> (canban_types::Snapshot, canban_ui::workflows::Draft) {
    use canban_types::*;
    let mut s = workspace();
    let now_ms = now();
    let project_id = "gallery-project".to_owned();

    let default_raw =
        canban_workflow_engine::source::encode(&canban_workflow_engine::source::default_document(
            "delivery:gallery-project",
            WorkspaceSettings::default(),
        ))
        .unwrap();

    let audit_yaml = r#"schema: 8
id: dependency-audit:gallery-project
name: Nightly Dependency Audit
enabled: true
on:
  schedule:
    event: schedule
    start_at: agent:audit
    cron: "0 2 * * *"
    timezone: Asia/Shanghai
    overlap: skip
    enabled: true
steps:
  - id: agent:audit
    name: Security & Outdated Audit
    uses: agent
    with:
      profile: developer
      instruction: |
        Run cargo audit and check Cargo.lock for security advisories or outdated workspace crates, then report a concise summary.
    next: __end
  - id: __end
    uses: complete
"#
    .to_owned();

    let qa_yaml = r#"schema: 8
id: nightly-qa:gallery-project
name: Weekday QA Smoke Card
enabled: true
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  schedule:
    event: schedule
    start_at: agent:qa
    cron: "30 3 * * 1-5"
    timezone: Local
    overlap: queue
    card:
      column: develop
      title: "Weekday QA Smoke · {{date}} (#{{run_number}})"
      body: "Automated weekday acceptance suite and UI regression check."
    enabled: true
steps:
  - id: agent:qa
    name: Run Acceptance Suite
    uses: agent
    with:
      profile: qa
      instruction: |
        Execute mise run test and verify UI regression scenarios on the scheduled Card branch.
    next: move:review
  - id: move:review
    uses: card.move
    with:
      column: review
    next: __end
  - id: __end
    uses: complete
"#
    .to_owned();

    let cleanup_yaml = r#"schema: 8
id: weekly-cleanup:gallery-project
name: Weekly Worktree Prune
enabled: true
on:
  schedule:
    event: schedule
    start_at: agent:prune
    cron: "0 10 * * 1"
    timezone: UTC
    enabled: false
steps:
  - id: agent:prune
    uses: agent
    with:
      profile: developer
      instruction: Inspect stale local caches and summarize disk usage.
    next: __end
  - id: __end
    uses: complete
"#
    .to_owned();

    let sid_audit = "schedule:gallery-project:dependency-audit:gallery-project:schedule".to_owned();
    let sid_qa = "schedule:gallery-project:nightly-qa:gallery-project:schedule".to_owned();
    let sid_cleanup = "schedule:gallery-project:weekly-cleanup:gallery-project:schedule".to_owned();

    let next_audit = cron::parse_schedule("0 2 * * *", Some("Asia/Shanghai"))
        .ok()
        .and_then(|(sc, tz)| sc.next_after_ms(now_ms, tz));
    let next_qa = cron::parse_schedule("30 3 * * 1-5", None)
        .ok()
        .and_then(|(sc, tz)| sc.next_after_ms(now_ms, tz));

    let sid_global_gc = format!(
        "schedule:{}:global:worktree-gc:schedule",
        canban_types::GLOBAL_PROJECT_ID
    );
    let global_gc_yaml = "schema: 9
id: global:worktree-gc
name: Worktree GC (7d)
scope: global
on:
  schedule:
    - cron: '0 3 * * *'
      timezone: Local
      overlap: skip
steps:
  - id: gc
    run: |
      \"$CANBAN_CLI\" worktree gc --merged-before 7d --fail-on-dirty --env-dirty DIRTY_WORKTREES
    on_failure: ask_agent
    next: done
  - id: ask_agent
    uses: agent
    with:
      profile: developer
      instruction: Inspect dirty worktrees and ask the user whether to clean or keep them.
    next: done
  - id: done
    uses: complete
";
    let global_gc_doc = canban_workflow_engine::source::parse(global_gc_yaml).unwrap();
    let global_gc_def = global_gc_doc.compile().unwrap();
    s.workflow_sources.push(WorkflowSource {
        host_id: String::new(),
        id: format!("{}:worktree-gc.yaml", canban_types::GLOBAL_PROJECT_ID),
        project_id: canban_types::GLOBAL_PROJECT_ID.into(),
        filename: "worktree-gc.yaml".into(),
        workflow_id: global_gc_def.id.clone(),
        revision: 1,
        hash: "global:worktree-gc:1".into(),
        revision_id: "global:worktree-gc:1".into(),
        enabled: true,
        columns: global_gc_def.columns.clone(),
        rules: global_gc_doc.on.clone(),
    });
    s.workflow_revisions.push(WorkflowRevision {
        host_id: String::new(),
        source_yaml: Some(global_gc_yaml.into()),
        id: "global:worktree-gc:1".into(),
        project_id: canban_types::GLOBAL_PROJECT_ID.into(),
        workflow_id: global_gc_def.id.clone(),
        revision: 1,
        definition: global_gc_def,
        created_at: now_ms,
    });

    s.schedule_states = vec![
        ScheduleState {
            host_id: String::new(),
            id: sid_global_gc.clone(),
            project_id: canban_types::GLOBAL_PROJECT_ID.into(),
            workflow_id: "global:worktree-gc".into(),
            workflow_name: "Worktree GC (7d)".into(),
            filename: "worktree-gc.yaml".into(),
            rule_id: "schedule".into(),
            cron: "0 3 * * *".into(),
            timezone: Some("Local".into()),
            overlap: ScheduleOverlap::Skip,
            creates_card: false,
            starts_with_script: true,
            target_column: None,
            profile_id: Some("developer".into()),
            enabled: true,
            last_scheduled_at: Some(now_ms.saturating_sub(90_000)),
            next_run_at: next_audit,
            last_run_id: Some("scheduled:global:gc12".into()),
            run_count: 12,
            queued_triggers: 0,
            updated_at: now_ms,
        },
        ScheduleState {
            host_id: String::new(),
            id: sid_audit.clone(),
            project_id: project_id.clone(),
            workflow_id: "dependency-audit:gallery-project".into(),
            workflow_name: "Nightly Dependency Audit".into(),
            filename: "dependency-audit.yaml".into(),
            rule_id: "schedule".into(),
            cron: "0 2 * * *".into(),
            timezone: Some("Asia/Shanghai".into()),
            overlap: ScheduleOverlap::Skip,
            creates_card: false,
            starts_with_script: false,
            target_column: None,
            profile_id: Some("developer".into()),
            enabled: true,
            last_scheduled_at: Some(now_ms.saturating_sub(180_000)),
            next_run_at: next_audit,
            last_run_id: Some("scheduled:gallery-project:run42".into()),
            run_count: 42,
            queued_triggers: 0,
            updated_at: now_ms,
        },
        ScheduleState {
            host_id: String::new(),
            id: sid_qa.clone(),
            project_id: project_id.clone(),
            workflow_id: "nightly-qa:gallery-project".into(),
            workflow_name: "Weekday QA Smoke Card".into(),
            filename: "nightly-qa.yaml".into(),
            rule_id: "schedule".into(),
            cron: "30 3 * * 1-5".into(),
            timezone: None,
            overlap: ScheduleOverlap::Queue,
            creates_card: true,
            starts_with_script: false,
            target_column: Some("develop".into()),
            profile_id: None,
            enabled: true,
            last_scheduled_at: Some(now_ms.saturating_sub(3_600_000)),
            next_run_at: next_qa,
            last_run_id: Some("scheduled:gallery-project:qa14".into()),
            run_count: 14,
            queued_triggers: 0,
            updated_at: now_ms,
        },
        ScheduleState {
            host_id: String::new(),
            id: sid_cleanup,
            project_id: project_id.clone(),
            workflow_id: "weekly-cleanup:gallery-project".into(),
            workflow_name: "Weekly Worktree Prune".into(),
            filename: "weekly-cleanup.yaml".into(),
            rule_id: "schedule".into(),
            cron: "0 10 * * 1".into(),
            timezone: Some("UTC".into()),
            overlap: ScheduleOverlap::Skip,
            creates_card: false,
            starts_with_script: false,
            target_column: None,
            profile_id: Some("developer".into()),
            enabled: false,
            last_scheduled_at: None,
            next_run_at: None,
            last_run_id: None,
            run_count: 0,
            queued_triggers: 0,
            updated_at: now_ms,
        },
    ];

    // Scheduled Card on the Board with `schedule_source` provenance.
    s.cards.push(Card {
        schedule_source: Some(ScheduleSourceRef {
            schedule_id: sid_qa.clone(),
            workflow_id: "nightly-qa:gallery-project".into(),
            workflow_name: "Weekday QA Smoke Card".into(),
            filename: "nightly-qa.yaml".into(),
            rule_id: "schedule".into(),
            cron: "30 3 * * 1-5".into(),
            run_id: "scheduled:gallery-project:qa14".into(),
            run_number: 14,
            scheduled_at: now_ms.saturating_sub(3_600_000),
        }),
        source_trust: Some("local".into()),
        hold_auto_merge: false,
        github_links: Vec::new(),
        created_by: "schedule:Weekday QA Smoke Card".into(),
        content: Default::default(),
        removed_at: None,
        pull_request: None,
        pull_request_history: Vec::new(),
        delivery_mode: None,
        workspace: None,
        workspace_history: Vec::new(),
        id: "card-5".into(),
        project_id: project_id.clone(),
        number: 5,
        title: "Weekday QA Smoke · 2026-11-15 (#14)".into(),
        body: "Automated weekday acceptance suite and UI regression check.".into(),
        board_order: None,
        context_notes: None,
        stage: "review".into(),
        revision: 2,
        attention: None,
        archived_at: None,
        archive: None,
        archive_error: None,
        worktree: Some("/preview/worktrees/card-5".into()),
        branch: Some("canban/gallery/5".into()),
        base_commit: Some("abcdef0123".into()),
        human_gate: None,
        delivery: None,
        resolution: None,
        created_at: now_ms.saturating_sub(3_600_000),
    });

    // Live Cardless Terminal Agent entry (`scheduled:gallery-project:run42`).
    s.terminals.push(TerminalEntry {
        id: "scheduled:gallery-project:run42".into(),
        conversation_id: None,
        number: 9,
        name: Some("Scheduled · Nightly Dependency Audit #42 (@developer)".into()),
        origin_project_id: Some(project_id.clone()),
        origin_card_id: None,
        agent_engine: Some("agy".into()),
        runtime: Some(TerminalRuntime {
            core_id: Some("local".into()),
            terminal: Some("pty-sched-42".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("working".into()),
            agent_turn: Some(1),
            observed_agent: Some("agy".into()),
            waiting: None,
        }),
        agent: None,
        created_at: now_ms.saturating_sub(180_000),
    });

    s.terminals.push(TerminalEntry {
        id: "scheduled:global:gc12".into(),
        conversation_id: None,
        number: 13,
        name: Some("Worktree GC (7d) #12".into()),
        origin_project_id: Some(canban_types::GLOBAL_PROJECT_ID.into()),
        origin_card_id: None,
        agent_engine: Some("agy".into()),
        runtime: Some(TerminalRuntime {
            core_id: Some("local".into()),
            terminal: Some("pty-sched-gc12".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("blocked".into()),
            agent_turn: Some(1),
            observed_agent: Some("agy".into()),
            waiting: Some(RuntimeWait {
                id: "wait-gc-12".into(),
                kind: "question".into(),
                title: "Dirty worktree found".into(),
                message: "Worktree feat/legacy has 2 uncommitted files. Force remove or keep?"
                    .into(),
                since: now_ms.saturating_sub(60_000),
            }),
        }),
        agent: None,
        created_at: now_ms.saturating_sub(90_000),
    });

    s.scheduled_runs = vec![
        ScheduledRun {
            host_id: String::new(),
            id: "scheduled:global:gc12".into(),
            schedule_id: sid_global_gc,
            project_id: canban_types::GLOBAL_PROJECT_ID.into(),
            workflow_id: "global:worktree-gc".into(),
            workflow_name: "Worktree GC (7d)".into(),
            filename: "worktree-gc.yaml".into(),
            rule_id: "schedule".into(),
            cron: "0 3 * * *".into(),
            number: 12,
            trigger_kind: "cron".into(),
            creates_card: false,
            card_id: None,
            card_number: None,
            workflow_run_id: None,
            start_step: Some("gc".into()),
            current_step: Some("ask_agent".into()),
            profile_id: Some("developer".into()),
            provider_session_id: None,
            terminal_id: Some("scheduled:global:gc12".into()),
            cwd: "/Users/example/.canban".into(),
            state: "blocked".into(),
            summary: Some(
                "Removed 3 clean merged worktrees; 1 dirty worktree (`feat/legacy`) needs decision in Terminal."
                    .into(),
            ),
            report_path: None,
            output_path: Some("global/schedules/gc12/gc.log".into()),
            error: None,
            steps: vec![
                ScheduledStepRecord {
                    step_id: "gc".into(),
                    kind: "script".into(),
                    state: "failed".into(),
                    profile_id: None,
                    exit_code: Some(1),
                    duration_ms: Some(420),
                    log_path: Some("global/schedules/gc12/gc.log".into()),
                    summary: Some("1 dirty worktree skipped".into()),
                },
                ScheduledStepRecord {
                    step_id: "ask_agent".into(),
                    kind: "agent".into(),
                    state: "blocked".into(),
                    profile_id: Some("developer".into()),
                    exit_code: None,
                    duration_ms: None,
                    log_path: None,
                    summary: Some("Waiting for decision in Terminal".into()),
                },
            ],
            scheduled_at: now_ms.saturating_sub(90_000),
            started_at: Some(now_ms.saturating_sub(90_000)),
            completed_at: None,
            close_requested_at: None,
            updated_at: now_ms.saturating_sub(30_000),
        },
        ScheduledRun {
            host_id: String::new(),
            id: "scheduled:gallery-project:run42".into(),
            schedule_id: sid_audit.clone(),
            project_id: project_id.clone(),
            workflow_id: "dependency-audit:gallery-project".into(),
            workflow_name: "Nightly Dependency Audit".into(),
            filename: "dependency-audit.yaml".into(),
            rule_id: "schedule".into(),
            cron: "0 2 * * *".into(),
            number: 42,
            trigger_kind: "manual".into(),
            creates_card: false,
            card_id: None,
            card_number: None,
            workflow_run_id: None,
            start_step: Some("agent:audit".into()),
            current_step: Some("agent:audit".into()),
            profile_id: Some("developer".into()),
            provider_session_id: None,
            terminal_id: Some("scheduled:gallery-project:run42".into()),
            cwd: "/example/project".into(),
            state: "running".into(),
            summary: Some("Auditing Cargo.lock advisories in live Terminal Agent…".into()),
            report_path: None,
            output_path: None,
            error: None,
            steps: Vec::new(),
            scheduled_at: now_ms.saturating_sub(180_000),
            started_at: Some(now_ms.saturating_sub(180_000)),
            completed_at: None,
            close_requested_at: None,
            updated_at: now_ms,
        },
        ScheduledRun {
            host_id: String::new(),
            id: "scheduled:gallery-project:run41".into(),
            schedule_id: sid_audit.clone(),
            project_id: project_id.clone(),
            workflow_id: "dependency-audit:gallery-project".into(),
            workflow_name: "Nightly Dependency Audit".into(),
            filename: "dependency-audit.yaml".into(),
            rule_id: "schedule".into(),
            cron: "0 2 * * *".into(),
            number: 41,
            trigger_kind: "cron".into(),
            creates_card: false,
            card_id: None,
            card_number: None,
            workflow_run_id: None,
            start_step: Some("agent:audit".into()),
            current_step: Some("agent:audit".into()),
            profile_id: Some("developer".into()),
            provider_session_id: None,
            terminal_id: None,
            cwd: "/example/project".into(),
            state: "completed".into(),
            summary: Some(
                "0 security advisories across 312 crates; Terminal Agent automatically reclaimed after reporting."
                    .into(),
            ),
            report_path: Some("projects/gallery-project/schedules/run41/report.md".into()),
            output_path: None,
            error: None,
            steps: Vec::new(),
            scheduled_at: now_ms.saturating_sub(86_400_000),
            started_at: Some(now_ms.saturating_sub(86_400_000)),
            completed_at: Some(now_ms.saturating_sub(86_350_000)),
            close_requested_at: Some(now_ms.saturating_sub(86_350_000)),
            updated_at: now_ms.saturating_sub(86_350_000),
        },
        ScheduledRun {
            host_id: String::new(),
            id: "scheduled:gallery-project:run40".into(),
            schedule_id: sid_audit,
            project_id: project_id.clone(),
            workflow_id: "dependency-audit:gallery-project".into(),
            workflow_name: "Nightly Dependency Audit".into(),
            filename: "dependency-audit.yaml".into(),
            rule_id: "schedule".into(),
            cron: "0 2 * * *".into(),
            number: 40,
            trigger_kind: "cron".into(),
            creates_card: false,
            card_id: None,
            card_number: None,
            workflow_run_id: None,
            start_step: None,
            current_step: None,
            profile_id: None,
            provider_session_id: None,
            terminal_id: None,
            cwd: "/example/project".into(),
            state: "skipped".into(),
            summary: Some("Skipped overlap tick while run #39 was still running".into()),
            report_path: None,
            output_path: None,
            error: None,
            steps: Vec::new(),
            scheduled_at: now_ms.saturating_sub(172_800_000),
            started_at: None,
            completed_at: Some(now_ms.saturating_sub(172_800_000)),
            close_requested_at: None,
            updated_at: now_ms.saturating_sub(172_800_000),
        },
        ScheduledRun {
            host_id: String::new(),
            id: "scheduled:gallery-project:qa14".into(),
            schedule_id: sid_qa,
            project_id,
            workflow_id: "nightly-qa:gallery-project".into(),
            workflow_name: "Weekday QA Smoke Card".into(),
            filename: "nightly-qa.yaml".into(),
            rule_id: "schedule".into(),
            cron: "30 3 * * 1-5".into(),
            number: 14,
            trigger_kind: "cron".into(),
            creates_card: true,
            card_id: Some("card-5".into()),
            card_number: Some(5),
            workflow_run_id: Some("wfrun-qa-14".into()),
            start_step: Some("agent:qa".into()),
            current_step: None,
            profile_id: Some("qa".into()),
            provider_session_id: None,
            terminal_id: None,
            cwd: "/preview/worktrees/card-5".into(),
            state: "completed".into(),
            summary: Some("All 192 workspace tests and acceptance suites passed on Card #5.".into()),
            report_path: None,
            output_path: None,
            error: None,
            steps: Vec::new(),
            scheduled_at: now_ms.saturating_sub(3_600_000),
            started_at: Some(now_ms.saturating_sub(3_600_000)),
            completed_at: Some(now_ms.saturating_sub(3_200_000)),
            close_requested_at: None,
            updated_at: now_ms.saturating_sub(3_200_000),
        },
    ];

    let mut draft = canban_ui::workflows::Draft {
        base: json!({"revision": "gallery", "sources": [
            {"filename": "dependency-audit.yaml", "source": audit_yaml, "active": {"revision": 1, "workflow_id": "dependency-audit:gallery-project"}},
            {"filename": "development.yaml", "source": default_raw, "active": {"revision": 1, "workflow_id": "delivery:gallery-project"}},
            {"filename": "nightly-qa.yaml", "source": qa_yaml, "active": {"revision": 1, "workflow_id": "nightly-qa:gallery-project"}},
            {"filename": "weekly-cleanup.yaml", "source": cleanup_yaml, "active": {"revision": 1, "workflow_id": "weekly-cleanup:gallery-project"}}
        ]}),
        ..Default::default()
    };
    draft
        .sources
        .insert("dependency-audit.yaml".into(), audit_yaml);
    draft.sources.insert("development.yaml".into(), default_raw);
    draft.sources.insert("nightly-qa.yaml".into(), qa_yaml);
    draft
        .sources
        .insert("weekly-cleanup.yaml".into(), cleanup_yaml);
    draft.selected = "dependency-audit.yaml".into();

    (s, draft)
}

/// Mixed-source Board filter regression fixture; no network or runtime.
pub fn board_filters() -> canban_types::Snapshot {
    use canban_types::*;
    let mut s = workspace();
    s.github = GitHubAccount {
        state: "connected".into(),
        login: Some("alice".into()),
        ..Default::default()
    };
    while s.cards.len() < 5 {
        let mut card = s.cards[0].clone();
        card.id = format!("card-{}", s.cards.len() + 1);
        card.number = (s.cards.len() + 1) as u64;
        s.cards.push(card);
    }
    s.cards.truncate(5);
    for (i, c) in s.cards.iter_mut().enumerate() {
        c.delivery = None;
        c.attention = None;
        c.human_gate = None;
        c.stage = "backlog".into();
        c.created_by = if i == 1 { "bob" } else { "alice" }.into();
        c.title = [
            "My local card",
            "Bob's local card",
            "My GitHub PR",
            "Bob's PR imported by me",
            "PR with unavailable details",
        ][i]
            .into();
        if i >= 2 {
            let source = GitHubIssueRef {
                repository: "fixture/repo".into(),
                repository_id: 1,
                issue_id: i as u64,
                number: i as u64,
                url: format!("https://github.com/fixture/repo/pull/{i}"),
            };
            c.content = CardContent::GitHubPr {
                source: source.clone(),
            };
            if i != 4 {
                s.github_issues.push(GitHubIssueSnapshot {
                    id: format!("source-{i}"),
                    project_id: c.project_id.clone(),
                    source,
                    title: c.title.clone(),
                    body: String::new(),
                    state: "open".into(),
                    author_id: i as u64,
                    author_login: if i == 2 { "alice" } else { "bob" }.into(),
                    labels: vec![],
                    assignees: vec![],
                    updated_at: String::new(),
                    synced_at: 0,
                    pr_head_sha: None,
                    pr_checks: None,
                });
            }
            if i == 2 {
                s.review_drafts.push(canban_types::ReviewDraft {
                    card_id: c.id.clone(),
                    repository: Some("fixture/repo".into()),
                    pr_number: Some(2),
                    compare_sha: Some("8f3a1c2".into()),
                    summary: "Found one concurrency hazard and a missing boundary check.".into(),
                    verdict: "request_changes".into(),
                    comments: vec![canban_types::ReviewDraftComment {
                        id: "draft-1".into(),
                        path: "crates/ui/src/rail.rs".into(),
                        section: String::new(),
                        line: 56,
                        side: "RIGHT".into(),
                        snippet: "move |_| crate::review::open_compare(ui)".into(),
                        body: "Guard against stale project selection before opening Diff.".into(),
                        author: Some("@reviewer".into()),
                        edited_by_user: false,
                    }],
                    author: Some("@reviewer".into()),
                    updated_at: 1_763_000_000_000,
                });
            }
        }
    }
    s
}

/// Production Board regression: scaled Issue/PR ownership, optional read-only
/// exported Snapshot. No AppServer, provider requests or real Card writes.
pub fn large_board() -> canban_types::Snapshot {
    let mut s = if let Some(path) = std::env::var_os("CANBAN_GALLERY_SNAPSHOT") {
        serde_json::from_slice::<canban_types::Snapshot>(
            &std::fs::read(path).expect("Gallery snapshot file"),
        )
        .expect("Gallery snapshot JSON")
    } else {
        board_filters()
    };
    if let Ok(count) = std::env::var("CANBAN_GALLERY_CARDS") {
        let count: usize = count.parse().expect("Gallery Card count");
        let originals = s.cards.clone();
        let sources = s.github_issues.clone();
        s.cards.clear();
        s.github_issues.clear();
        for i in 0..count {
            let mut c = originals[i % originals.len()].clone();
            let original_source = c.github_issue().cloned();
            c.id = format!("large-{i}");
            c.number = i as u64 + 1;
            c.board_order = Some(i as i64 * 1024);
            c.stage = "backlog".into();
            c.archived_at = None;
            c.removed_at = None;
            c.delivery = None;
            c.pull_request = None;
            if let canban_types::CardContent::GitHubIssue { source }
            | canban_types::CardContent::GitHubPr { source } = &mut c.content
            {
                source.issue_id = i as u64 + 1;
                source.number = i as u64 + 1;
                if let Some(mut cached) = sources
                    .iter()
                    .find(|v| {
                        original_source.as_ref().is_some_and(|r| {
                            r.issue_id == v.source.issue_id
                                && r.repository_id == v.source.repository_id
                        })
                    })
                    .cloned()
                {
                    cached.id = format!("large-source-{i}");
                    cached.source = source.clone();
                    s.github_issues.push(cached);
                }
            }
            s.cards.push(c);
        }
    }
    s
}

/// Terminal workspace fixture contrasting managed tabs (paused, reclaiming soon, auto-reclaiming,
/// configuration, and cardless scheduled) against manual CLI and plain shell terminals.
pub fn terminal_managed_reclaim() -> canban_types::Snapshot {
    use canban_types::*;
    let mut s = workspace();
    let cfg = agent_config();
    let dev_profile = cfg
        .profiles
        .iter()
        .find(|p| p.id == "developer")
        .unwrap()
        .clone();
    let rev_profile = cfg
        .profiles
        .iter()
        .find(|p| p.id == "reviewer")
        .cloned()
        .unwrap_or_else(|| dev_profile.clone());

    let mut paused_run = canban_scheduler::agent_run(
        "gallery-project",
        "card-2",
        1,
        "develop",
        "stage_entry",
        dev_profile.clone(),
        "Implement managed terminal distinction.".into(),
        None,
    );
    paused_run.state = "completed".into();
    paused_run.core_id = Some("gallery".into());
    paused_run.terminal = Some("managed-paused-pty".into());
    paused_run.cwd = "/preview/worktrees/card-2".into();
    paused_run.observed = Some(json!({"state": "live", "turn": 2, "status": {"state": "done"}}));
    paused_run.connection_state = "connected".into();
    paused_run.completed_at = Some(now().saturating_sub(45_000));
    paused_run.lifecycle.idle_since = Some(now().saturating_sub(45_000));
    paused_run.lifecycle.observed_at = Some(u64::MAX);
    let paused_id = push_agent_terminal(&mut s, paused_run);

    let mut soon_run = canban_scheduler::agent_run(
        "gallery-project",
        "card-3",
        2,
        "review",
        "stage_entry",
        rev_profile,
        "Review terminal lifecycle changes.".into(),
        None,
    );
    soon_run.state = "completed".into();
    soon_run.core_id = Some("gallery".into());
    soon_run.terminal = Some("managed-soon-pty".into());
    soon_run.cwd = "/preview/worktrees/card-2".into();
    soon_run.observed = Some(json!({"state": "live", "turn": 3, "status": {"state": "done"}}));
    soon_run.connection_state = "connected".into();
    soon_run.completed_at = Some(now().saturating_sub(22_000));
    soon_run.lifecycle.idle_since = Some(now().saturating_sub(22_000));
    soon_run.lifecycle.observed_at = Some(u64::MAX);
    let soon_id = push_agent_terminal(&mut s, soon_run);

    let mut closing_run = canban_scheduler::agent_run(
        "gallery-project",
        "card-1",
        3,
        "plan",
        "stage_entry",
        dev_profile,
        "Planning draft complete.".into(),
        None,
    );
    closing_run.state = "completed".into();
    closing_run.core_id = Some("gallery".into());
    closing_run.terminal = Some("managed-reclaiming-pty".into());
    closing_run.cwd = "/preview/worktrees/card-2".into();
    closing_run.close_state = "closing".into();
    closing_run.close_requested_at = Some(now());
    closing_run.lifecycle.close_reason = "idle".into();
    closing_run.observed = Some(json!({"state": "live", "turn": 1, "status": {"state": "done"}}));
    closing_run.connection_state = "connected".into();
    closing_run.lifecycle.observed_at = Some(u64::MAX);
    let closing_id = push_agent_terminal(&mut s, closing_run);

    s.terminals.push(TerminalEntry {
        id: "configuration:gallery-project:default:developer".into(),
        conversation_id: None,
        number: 10,
        name: Some("Workflow Setup".into()),
        origin_project_id: Some("gallery-project".into()),
        origin_card_id: None,
        agent_engine: Some("codex".into()),
        runtime: Some(TerminalRuntime {
            core_id: Some("gallery".into()),
            terminal: Some("managed-config-pty".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("idle".into()),
            agent_turn: Some(1),
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: now(),
    });

    let sched_id = "scheduled:gallery-project:run42".to_string();
    s.terminals.push(TerminalEntry {
        id: sched_id.clone(),
        conversation_id: None,
        number: 11,
        name: Some("Nightly Audit #42".into()),
        origin_project_id: Some("gallery-project".into()),
        origin_card_id: None,
        agent_engine: Some("codex".into()),
        runtime: Some(TerminalRuntime {
            core_id: Some("gallery".into()),
            terminal: Some("managed-sched-pty".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("done".into()),
            agent_turn: Some(2),
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: now(),
    });
    s.scheduled_runs.push(ScheduledRun {
        host_id: String::new(),
        id: sched_id.clone(),
        schedule_id: "sched-audit".into(),
        project_id: "gallery-project".into(),
        workflow_id: "dependency-audit:gallery-project".into(),
        workflow_name: "Nightly Dependency Audit".into(),
        filename: "dependency-audit.yaml".into(),
        rule_id: "schedule".into(),
        cron: "0 2 * * *".into(),
        number: 42,
        trigger_kind: "manual".into(),
        creates_card: false,
        card_id: None,
        card_number: None,
        workflow_run_id: None,
        start_step: Some("audit".into()),
        current_step: Some("audit".into()),
        profile_id: Some("developer".into()),
        provider_session_id: None,
        terminal_id: Some(sched_id.clone()),
        cwd: "/preview/worktrees/card-2".into(),
        state: "completed".into(),
        summary: Some("Audit completed; interactive follow-up cooled down.".into()),
        report_path: None,
        output_path: None,
        error: None,
        steps: Vec::new(),
        scheduled_at: now().saturating_sub(180_000),
        started_at: Some(now().saturating_sub(180_000)),
        completed_at: Some(now().saturating_sub(60_000)),
        close_requested_at: None,
        updated_at: now(),
    });

    s.terminals.push(TerminalEntry {
        id: "manual-agy-1".into(),
        conversation_id: None,
        number: 12,
        name: Some("Antigravity 1".into()),
        origin_project_id: Some("gallery-project".into()),
        origin_card_id: Some("card-2".into()),
        agent_engine: Some("agy".into()),
        runtime: Some(TerminalRuntime {
            core_id: Some("gallery".into()),
            terminal: Some("manual-agy-pty".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("idle".into()),
            agent_turn: Some(1),
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: now(),
    });
    s.terminals.push(TerminalEntry {
        id: "manual-shell-1".into(),
        conversation_id: None,
        number: 13,
        name: Some("Terminal 1".into()),
        origin_project_id: Some("gallery-project".into()),
        origin_card_id: Some("card-2".into()),
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            core_id: Some("gallery".into()),
            terminal: Some("manual-shell-pty".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
            observed_agent: None,
            waiting: None,
        }),
        agent: None,
        created_at: now(),
    });

    s.terminal_contexts = vec![
        paused_id.as_str(),
        soon_id.as_str(),
        closing_id.as_str(),
        "manual-agy-1",
        "manual-shell-1",
    ]
    .into_iter()
    .map(|tid| TerminalContext {
        terminal_id: tid.into(),
        cwd: Some("/preview/worktrees/card-2".into()),
        directory_root: Some("/preview/worktrees/card-2".into()),
        branch: Some("canban/card-2".into()),
        project_id: Some("gallery-project".into()),
        card_id: Some("card-2".into()),
        ..Default::default()
    })
    .collect();

    s.reclaim_status = vec![
        TerminalReclaimStatus {
            terminal_id: paused_id,
            paused: true,
            reclaiming_soon: false,
            auto_reclaiming: false,
            interactive_followup: false,
            deadline_at: None,
        },
        TerminalReclaimStatus {
            terminal_id: soon_id,
            paused: false,
            reclaiming_soon: true,
            auto_reclaiming: false,
            interactive_followup: false,
            deadline_at: Some(now().saturating_add(8_000)),
        },
        TerminalReclaimStatus {
            terminal_id: closing_id,
            paused: false,
            reclaiming_soon: false,
            auto_reclaiming: true,
            interactive_followup: false,
            deadline_at: None,
        },
        TerminalReclaimStatus {
            terminal_id: sched_id,
            paused: false,
            reclaiming_soon: false,
            auto_reclaiming: false,
            interactive_followup: true,
            deadline_at: Some(now().saturating_add(15 * 60_000)),
        },
    ];
    s
}

pub fn env_meta_map() -> std::collections::BTreeMap<String, canban_types::EnvEntryMeta> {
    use canban_types::EnvEntryMeta;
    std::collections::BTreeMap::from([
        (
            "RUST_LOG".into(),
            EnvEntryMeta::Plain {
                value: "debug".into(),
            },
        ),
        (
            "OPENAI_API_KEY".into(),
            EnvEntryMeta::Secret {
                configured: true,
                hint: "••••9a2f".into(),
            },
        ),
        (
            "MISSING_REMOTE_TOKEN".into(),
            EnvEntryMeta::Secret {
                configured: false,
                hint: String::new(),
            },
        ),
    ])
}
#[cfg(test)]
mod tests {
    use super::*;
    use canban_ui::insights::model::{
        HeatmapMetric, RangeWindow, ViewOptions, compute_dashboard, time_totals,
    };

    #[test]
    fn gallery_insights_activity_has_1d_7d_and_heatmap_parity() {
        let (snap, events) = insights();
        let act = insights_activity();
        assert!(act.loaded);
        assert_eq!(act.annual_buckets.len(), 365);
        assert_eq!(act.range_buckets, act.annual_buckets);
        assert_eq!(act.baseline_buckets, act.annual_buckets);

        let totals_1d = time_totals(RangeWindow::Today, &act);
        let today_bucket = act.annual_buckets.last().unwrap();
        let yesterday_bucket = &act.annual_buckets[act.annual_buckets.len() - 2];

        assert_eq!(today_bucket.dwell.card, totals_1d.dwell_card_ms);
        assert_eq!(today_bucket.dwell.terminal, totals_1d.dwell_terminal_ms);
        assert_eq!(today_bucket.dwell.other, totals_1d.dwell_other_ms);
        assert_eq!(today_bucket.dwell.total(), totals_1d.dwell_total_ms);
        assert_eq!(today_bucket.agent_ms, totals_1d.agent_total_ms);

        assert_eq!(
            yesterday_bucket.dwell.total(),
            totals_1d.prev_dwell_total_ms
        );
        assert_eq!(yesterday_bucket.agent_ms, totals_1d.prev_agent_total_ms);

        let d1 = compute_dashboard(
            &snap,
            &events,
            &act,
            None,
            &chrono::Local,
            &[],
            ViewOptions::with_now(INSIGHTS_NOW),
        );
        let mut opts_7d = ViewOptions::with_now(INSIGHTS_NOW);
        opts_7d.range = RangeWindow::Days7;
        let d7 = compute_dashboard(&snap, &events, &act, None, &chrono::Local, &[], opts_7d);

        let row_today = d7.day_rows.last().unwrap();
        let row_yesterday = &d7.day_rows[d7.day_rows.len() - 2];
        assert!(row_today.is_today);
        assert_eq!(row_today.dwell_card_ms, d1.time_totals.dwell_card_ms);
        assert_eq!(
            row_today.dwell_terminal_ms,
            d1.time_totals.dwell_terminal_ms
        );
        assert_eq!(row_today.dwell_other_ms, d1.time_totals.dwell_other_ms);
        assert_eq!(row_today.dwell_total_ms(), d1.time_totals.dwell_total_ms);
        assert_eq!(row_today.agent_ms, d1.time_totals.agent_total_ms);

        assert_eq!(
            row_yesterday.dwell_total_ms(),
            d1.time_totals.prev_dwell_total_ms
        );
        assert_eq!(row_yesterday.agent_ms, d1.time_totals.prev_agent_total_ms);

        let mut opts_dwell_hm = ViewOptions::with_now(INSIGHTS_NOW);
        opts_dwell_hm.heatmap_metric = HeatmapMetric::Dwell;
        let d_hm = compute_dashboard(
            &snap,
            &events,
            &act,
            None,
            &chrono::Local,
            &[],
            opts_dwell_hm,
        );
        let cells: Vec<_> = d_hm
            .heatmap_weeks
            .iter()
            .flat_map(|w| w.cells.iter())
            .filter(|c| !c.is_future)
            .collect();
        let today_cell = cells.iter().find(|c| c.is_today).unwrap();
        let yest_cell = cells
            .iter()
            .find(|c| c.date_key == row_yesterday.date_key)
            .unwrap();
        assert_eq!(today_cell.dwell_ms, d1.time_totals.dwell_total_ms);
        assert_eq!(today_cell.agent_ms, d1.time_totals.agent_total_ms);
        assert_eq!(yest_cell.dwell_ms, d1.time_totals.prev_dwell_total_ms);
        assert_eq!(yest_cell.agent_ms, d1.time_totals.prev_agent_total_ms);
    }
}
