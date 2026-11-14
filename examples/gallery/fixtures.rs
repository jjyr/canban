use canban_types::{Event, ReviewPreview};
use serde_json::json;

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
        "compare_ref": "WORKDIR",
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
                "status": "added",
                "additions": 15,
                "deletions": 0,
                "patch": "@@ -0,0 +1,6 @@\n+impl Ops {\n+    pub fn git_pull(&self, project: &Project, dir: &Path) -> Result<Value> {\n+        // Safe fast-forward only (`--ff-only`)\n+        self.checked(dir, &[\"pull\", \"--ff-only\"])\n+    }\n+}"
            },
            {
                "filename": "crates/ui/src/review/sidebar.rs",
                "status": "modified",
                "additions": 8,
                "deletions": 4,
                "patch": "@@ -35,6 +35,10 @@ pub fn render_top_bar(ui: Ui) -> impl IntoElement {\n-    .child(project_picker)\n+    .child(project_picker)\n+    .child(git_ops::render_checkout_picker(ui))\n+    .child(git_ops::render_sync_controls(ui))"
            },
            {
                "filename": "crates/ui/src/review/git_ops.rs",
                "status": "added",
                "additions": 3,
                "deletions": 0,
                "patch": "@@ -0,0 +1,3 @@\n+pub fn run_commit(ui: Ui, and_push: bool) {\n+    // Dispatches git.commit with optional stage_all & push\n+}"
            }
        ]
    })
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

pub const TERMINAL: &str = concat!(
    "\x1b[1;38;2;110;210;255m╭─ Canban Native Terminal · libghostty-vt ──────────────────────╮\x1b[0m\r\n",
    "\x1b[1;38;2;110;210;255m│\x1b[0m Styles : \x1b[1mBold\x1b[0m · \x1b[3mItalic\x1b[0m · \x1b[2mFaint\x1b[0m · \x1b[9mStrike\x1b[0m · \x1b[4:3;58;2;255;105;97mCurly diagnostic\x1b[0m     \x1b[1;38;2;110;210;255m│\x1b[0m\r\n",
    "\x1b[1;38;2;110;210;255m│\x1b[0m CJK    : 世界你好 · café · Grid [中文对齐] · Blocks ░▒▓█▀▄▌▐  \x1b[1;38;2;110;210;255m│\x1b[0m\r\n",
    "\x1b[1;38;2;110;210;255m│\x1b[0m Braille: ⠁⠃⠇⠏⠟⠿⣿ · Links: https://canban.dev (\x1b]8;;https://ghostty.org\x07OSC-8 link\x1b]8;;\x07)     \x1b[1;38;2;110;210;255m│\x1b[0m\r\n",
    "\x1b[1;38;2;110;210;255m╰───────────────────────────────────────────────────────────────╯\x1b[0m\r\n",
    "\x1b[32m❯\x1b[0m Select text, Cmd-hover links, or change Terminal font in Settings.\r\n",
);

pub fn workspace() -> canban_types::Snapshot {
    use canban_types::*;
    let mut s = Snapshot::default();
    s.projects.push(Project {
        github: None,
        delivery_mode: Default::default(),
        agent_overrides: std::collections::BTreeMap::from([(
            "developer".into(),
            ProjectAgentOverride {
                agent: Some("agy".into()),
                model: Some("gemini-4-carbon-high".into()),
                effort: Some("high".into()),
            },
        )]),
        tags: vec![
            "search".into(),
            "ux".into(),
            "agent".into(),
            "terminal".into(),
            "bug".into(),
        ],
        location: None,
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
        let tag_suffix = match i {
            1 => "\n\nTags: #search #ux #agent #bug",
            2 => "\n\nTags: #agent #terminal",
            3 => "\n\nTags: #ux",
            _ => "",
        };
        s.cards.push(Card{source_trust:Some("local".into()),hold_auto_merge:false,github_links:vec![],created_by:"Gallery".into(),content:Default::default(),removed_at:None,pull_request:None,pull_request_history:Vec::new(),delivery_mode:None,workspace:None,workspace_history:Vec::new(),id:format!("card-{i}"),project_id:"gallery-project".into(),number:i,board_order:None,title:title.into(),body:format!("## Goal\nMake progress easy to understand.\n\nKeep long descriptions readable, preserve drafts and provide clear next actions.\n\n中文与 English content uses the same document component.{tag_suffix}"),context_notes:Some(vec!["note-1".into()]),stage:stage.into(),revision:1,attention:attention.map(str::to_owned),archived_at:None,archive:None,archive_error:None,worktree:Some(format!("/preview/worktrees/card-{i}")),branch:Some(format!("canban/gallery/{i}")),base_commit:Some("abcdef0123".into()),human_gate:attention.map(|reason|HumanGate{reason:reason.into(),terminal_id:None,next_stage:None}),delivery,created_at:now()});
    }
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
    s.events.push(event);
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
        source_yaml: Some(default_yaml),
        id: "delivery:gallery-project:1".into(),
        project_id: "gallery-project".into(),
        workflow_id: "delivery:gallery-project".into(),
        revision: 1,
        definition: default_def,
        created_at: now(),
    });
    s.workflow_sources.push(WorkflowSource {
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
        manual_number: None,
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
        name: None,
        manual_number: Some(1),
        origin_card_id: Some("card-2".into()),
        agent_engine: None,
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("gallery".into()),
            terminal: Some("card-2-shell-pty".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
        }),
        agent: None,
        created_at: now(),
    });
    s.terminals.push(TerminalEntry {
        id: "card-2-agy-1".into(),
        conversation_id: None,
        number: 3,
        name: None,
        manual_number: Some(2),
        origin_card_id: Some("card-2".into()),
        agent_engine: Some("agy".into()),
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("gallery".into()),
            terminal: Some("card-2-agy-pty".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
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
            s.events.push(event);
            s.cards.push(card);
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
        s.events.push(event);
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
    s.events.push(stage_done);

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
        name: None,
        manual_number: Some(1),
        origin_card_id: None,
        agent_engine: None,
        runtime: Some(canban_types::TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("gallery".into()),
            terminal: Some("shell".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
        }),
        agent: None,
        created_at: 0,
    });
    let mut remote = s.terminals.last().unwrap().clone();
    remote.id = "gallery-remote-shell".into();
    remote.number += 2;
    remote.manual_number = Some(3);
    remote.name = Some("Remote shell with a long custom name for layout checks".into());
    let runtime = remote.runtime.as_mut().unwrap();
    runtime.machine = "gallery-remote".into();
    runtime.terminal = Some("gallery-remote/shell".into());
    runtime.connection = "unavailable".into();
    let mut linked = s.terminals.last().unwrap().clone();
    linked.id = "gallery-linked-shell".into();
    linked.number += 1;
    linked.manual_number = Some(2);
    linked.runtime.as_mut().unwrap().terminal = Some("linked-shell".into());
    s.terminals.push(linked);
    s.terminals.push(remote);
    for attempt in s
        .terminals
        .iter_mut()
        .filter_map(|t| t.agent.as_mut())
        .filter(|a| a.card_id == "overview-card-2" && a.terminal.is_some())
    {
        attempt.location = Some(canban_types::WorkLocation {
            machine: "gallery-remote".into(),
            directory: "/remote/workspaces/canban".into(),
        });
        attempt.terminal = Some("gallery-remote/t3".into());
    }
    let card = s.cards.iter().find(|c| c.archived_at.is_none()).unwrap();
    let card_project_id = card.project_id.clone();
    let card_id = card.id.clone();
    s.terminal_contexts.push(canban_types::TerminalContext {
        machine: "local".into(),
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
        ahead: 2,
        behind: 0,
    });
    let proj_id = s.projects[0].id.clone();
    s.terminals.push(canban_types::TerminalEntry {
        id: "gallery-project-root-shell".into(),
        conversation_id: None,
        number: number + 3,
        name: None,
        manual_number: Some(5),
        origin_card_id: None,
        agent_engine: None,
        runtime: Some(canban_types::TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("gallery".into()),
            terminal: Some("root-shell".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
        }),
        agent: None,
        created_at: 0,
    });
    s.terminal_contexts.push(canban_types::TerminalContext {
        machine: "local".into(),
        terminal_id: "gallery-project-root-shell".into(),
        core_id: "gallery".into(),
        terminal: "root-shell".into(),
        cwd: Some(s.projects[0].directory.clone()),
        project_id: Some(proj_id.clone()),
        card_id: None,
        worktree_id: None,
        branch: Some("main".into()),
        dirty: false,
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
        location: canban_types::WorkLocation {
            machine: "local".into(),
            directory: "/tmp/gallery/worktrees/standalone-spike".into(),
        },
        created_at: canban_types::now(),
    });
    s.worktrees.push(canban_types::ManagedWorktree {
        id: "gallery-wt-retained".into(),
        project_id: proj_id.clone(),
        branch: "chore/retained-experiment".into(),
        base_branch: "main".into(),
        base_commit: None,
        path: "/tmp/gallery/worktrees/retained-experiment".into(),
        location: canban_types::WorkLocation {
            machine: "local".into(),
            directory: "/tmp/gallery/worktrees/retained-experiment".into(),
        },
        created_at: canban_types::now(),
    });
    s.terminals.push(canban_types::TerminalEntry {
        id: "gallery-wt-shell".into(),
        conversation_id: None,
        number: number + 4,
        name: Some("feat/standalone-spike".into()),
        manual_number: Some(4),
        origin_card_id: None,
        agent_engine: None,
        runtime: Some(canban_types::TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("gallery".into()),
            terminal: Some("wt-shell".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: None,
            agent_turn: None,
        }),
        agent: None,
        created_at: 0,
    });
    s.terminal_contexts.push(canban_types::TerminalContext {
        machine: "local".into(),
        terminal_id: "gallery-wt-shell".into(),
        core_id: "gallery".into(),
        terminal: "wt-shell".into(),
        cwd: Some("/tmp/gallery/worktrees/standalone-spike".into()),
        project_id: Some(proj_id),
        card_id: None,
        worktree_id: Some("gallery-wt-spike".into()),
        branch: Some("feat/standalone-spike".into()),
        dirty: true,
        ahead: 3,
        behind: 1,
    });
    s.projects.push(canban_types::Project {
        id: "gallery-empty-project".into(),
        name: "Docs & Website".into(),
        directory: "/tmp/gallery/docs-site".into(),
        location: None,
        github: None,
        delivery_mode: Default::default(),
        agent_overrides: Default::default(),
        tags: Default::default(),
        workflow: canban_types::Workflow::default(),
        target_branch: Some("main".into()),
        created_at: 0,
    });
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
        name: None,
        manual_number: Some(1),
        origin_card_id: Some("card-1".into()),
        agent_engine: Some("agy".into()),
        runtime: Some(TerminalRuntime {
            machine: "local".into(),
            owner_core_id: None,
            core_id: Some("gallery".into()),
            terminal: Some("gallery-card-1-agy".into()),
            state: "running".into(),
            connection: "connected".into(),
            error: None,
            agent_status: Some("idle".into()),
            agent_turn: Some(1),
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
        s.events.push(Event {
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
    s
}

/// The production Card reader under long conversational history and background activity.
pub fn conversation() -> canban_types::Snapshot {
    let mut s = workspace();
    s.cards[0].body = "## Goal\nKeep the Card conversation readable while task context stays available.\n\n- [x] Full Markdown description\n- [ ] Keep selection and undo through background updates\n\n> Description, comments and events share one scroll owner.\n\n| Surface | Behavior |\n| --- | --- |\n| Editor | Write / Preview |\n| Agent | Type @ to choose |\n\n```rust\nlet context = card.history();\n```\n\n[Canban](https://github.com/jjyr/canban) · **Readable** and _editable_.\n".into();
    let base = canban_types::now();
    let mut seq = 1_u64;
    s.events.retain(|e| e.card_id != "card-1");
    let mut push = |kind: &str, actor: &str, msg: String, mins_ago: u64| {
        s.events.push(Event {
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
    if let Some(first_comment) = s
        .events
        .iter_mut()
        .find(|e| e.card_id == "card-1" && e.kind == "comment.created")
    {
        first_comment.data = json!({
            "body": first_comment.message,
            "revision": 2,
            "edited_at": base.saturating_sub(89 * 60_000),
        });
    }
    s
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
        source_yaml: Some(primary_yaml),
        id: "delivery:gallery-project:1".into(),
        project_id: "gallery-project".into(),
        workflow_id: "delivery:gallery-project".into(),
        revision: 1,
        definition: primary.compile().unwrap(),
        created_at: now(),
    });
    snapshot.workflow_sources.push(WorkflowSource {
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
        source_yaml: Some(raw.into()),
        id: revision_id.clone(),
        project_id: "gallery-project".into(),
        workflow_id: workflow_id.clone(),
        revision: 1,
        definition,
        created_at: now(),
    });
    snapshot.workflow_sources.push(WorkflowSource {
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
    // demonstrates enabled-agent gating + custom identities (`@qa`) + per-model effort gating
    // (`@merger` uses `gemini-pro-orchestrator` which hides `--effort` while `@reviewer` uses
    // `gemini-4-carbon-medium` with `medium` effort).
    cfg.profiles.push(Profile {
        id: "agy".into(),
        enabled: true,
        preset: Some("agy".into()),
        model: Some("gemini-4-carbon-medium".into()),
        effort: Some("medium".into()),
        ..Default::default()
    });
    if let Some(claude) = cfg.profiles.iter_mut().find(|p| p.id == "claude") {
        claude.enabled = false;
    }
    if let Some(rev) = cfg.profiles.iter_mut().find(|p| p.id == "reviewer") {
        rev.preset = Some("agy".into());
        rev.model = Some("gemini-4-carbon-medium".into());
        rev.effort = Some("medium".into());
    }
    if let Some(merger) = cfg.profiles.iter_mut().find(|p| p.id == "merger") {
        merger.preset = Some("agy".into());
        merger.model = Some("gemini-pro-orchestrator".into());
        merger.effort = Some("medium".into());
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
            { "id": "agy", "preset": "agy", "model": "gemini-4-carbon-medium", "effort": "medium" },
            { "id": "reviewer", "identity": true, "preset": "agy" },
            { "id": "merger", "identity": true, "preset": "agy", "model": "gemini-pro-orchestrator" },
            { "id": "qa", "identity": true, "preset": "codex", "model": "gpt-6.1-sol", "effort": "high" }
        ],
    })
}
