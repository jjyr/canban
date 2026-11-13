use canban_types::{Event, ReviewPreview};
use serde_json::json;

pub fn timeline() -> Event {
    Event {id:"gallery-event".into(),project_id:"gallery-project".into(),card_id:"gallery-card".into(),sequence:1,kind:"comment.created".into(),actor:"user".into(),message:"A **shared component**, rendered from deterministic fixture data.\n\n中文输入、选择、复制与长文本。".into(),request_id:None,data:json!({}),created_at:canban_types::now()}
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
            last_request_id: None,
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
        "current_branch": "feat/diff-workbench",
        "default_branch": "main",
        "head_sha": "c207c08a91fe001234567890abcdef0123456789",
        "dirty": true,
        "dirty_count": 3,
        "staged_count": 1,
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
        agent_overrides: Default::default(),
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
        s.cards.push(Card{source_trust:Some("local".into()),hold_auto_merge:false,github_links:vec![],created_by:"Gallery".into(),content:Default::default(),removed_at:None,pull_request:None,pull_request_history:Vec::new(),delivery_mode:None,workspace:None,workspace_history:Vec::new(),id:format!("card-{i}"),project_id:"gallery-project".into(),number:i,board_order:None,title:title.into(),body:format!("## Goal\nMake progress easy to understand.\n\nKeep long descriptions readable, preserve drafts and provide clear next actions.\n\n中文与 English content uses the same document component.{tag_suffix}"),context_notes:Some(vec!["note-1".into()]),stage:stage.into(),revision:1,attention:attention.map(str::to_owned),archived_at:None,archive:None,archive_error:None,worktree:Some(format!("/preview/worktrees/card-{i}")),branch:Some(format!("canban/gallery/{i}")),base_commit:Some("abcdef0123".into()),human_gate:attention.map(|reason|HumanGate{reason:reason.into(),request_id:None,next_stage:None}),delivery,created_at:now()});
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
        request_id: None,
        attempt: None,
        revision: 1,
        data: json!({}),
        created_at: now(),
    });
    s
}

pub fn workspace_with_card_terminals() -> canban_types::Snapshot {
    use canban_types::*;
    let mut s = workspace();
    let dev_profile = agent_config()
        .profiles
        .into_iter()
        .find(|p| p.id == "developer")
        .unwrap();
    let mut req = canban_scheduler::request(
        "gallery-project",
        "card-2",
        "develop",
        "stage_entry",
        dev_profile,
        "Implement credential validation.".into(),
        None,
    );
    req.id = "card-2-req".into();
    req.state = "running".into();
    req.attempt = 1;
    let mut run: Attempt = serde_json::from_value(json!({
        "id": Attempt::key(&req.id, 1),
        "request_id": req.id,
        "project_id": "gallery-project",
        "card_id": "card-2",
        "number": 1,
        "state": "running",
        "core_id": "gallery",
        "terminal": "card-2-dev-pty",
        "cwd": "/preview/worktrees/card-2",
        "input_path": "input.md",
        "output_path": null,
        "observed": {"state": "live", "status": {"state": "working"}},
        "error": null,
        "created_at": now(),
        "provider_session_id": "session-card-2",
        "connection_state": "connected"
    }))
    .unwrap();
    run.lifecycle.observed_at = Some(u64::MAX);
    s.requests.push(req);
    s.attempts.push(run);
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
            let mut request = canban_scheduler::request(
                &project.id,
                &card.id,
                "develop",
                "stage_entry",
                Config::default().profiles[0].clone(),
                "Fixture".into(),
                None,
            );
            request.state = state.into();
            if i < 2 {
                card.human_gate = Some(HumanGate {
                    reason: if i == 0 {
                        "Review the result"
                    } else {
                        "Answer the Agent"
                    }
                    .into(),
                    request_id: Some(request.id.clone()),
                    next_stage: None,
                });
            }
            s.requests.push(request);
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
        let mut req=canban_scheduler::request(&card.project_id,&card.id,&card.stage,"stage_entry",Config::default().profiles[0].clone(),"Inspect the Card context and implement a small, reviewable change.\n\nKeep the user's existing work and explain any decision that needs attention.".into(),None);
        req.id = format!("overview-request-{i}");
        req.attempt = 1;
        req.state = if *state == "queued" {
            "queued"
        } else if *state == "review" {
            "completed"
        } else {
            "running"
        }
        .into();
        if *state == "exited" {
            req.state = "failed".into();
        }
        if *state == "review" {
            card.human_gate = Some(HumanGate {
                reason: "Agent report ready; human review required".into(),
                request_id: Some(req.id.clone()),
                next_stage: None,
            });
        }
        if *state != "queued" {
            let mut run:Attempt=serde_json::from_value(json!({"id":Attempt::key(&req.id,1),"request_id":req.id,"project_id":card.project_id,"card_id":card.id,"number":1,"state":req.state,"core_id":"gallery-core","terminal":format!("t{}",i+1),"cwd":"/example/card-worktree","input_path":"input.md","output_path":null,"observed":{"state":if matches!(*state,"closed"|"exited") {"finished"} else {"live"},"status":{"state":if *state=="review" {"done"} else {state},"kind":"permission","title":"Allow repository command?","msg":"Agent requests permission to run git status in the Card worktree."}},"error":null,"created_at":now()-180_000,"provider_session_id":format!("session-{i}"),"connection_state":if *state=="unknown" {"unavailable"} else {"connected"}})).unwrap();
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
            s.attempts.push(run);
        }
        s.requests.push(req);
        s.cards.push(card);
    }
    // Multi-reason and long-title row exercises compact layout and both direct actions.
    s.cards[0].title =
        "修复支付重试逻辑 · preserve the complete task identity when the window is narrow".into();
    s.cards[0].human_gate = Some(HumanGate {
        reason: "Decide whether to raise the retry limit".into(),
        request_id: Some(s.requests[0].id.clone()),
        next_stage: None,
    });
    if let Some(a) = s
        .attempts
        .iter_mut()
        .find(|a| a.request_id == "overview-request-2")
    {
        a.observed = Some(
            json!({"state":"live","status":{"state":"working","msg":"Adding keyboard focus regression coverage"}}),
        );
    }
    // A second live profile makes Card-level navigation require an explicit choice.
    let mut req = s.requests[2].clone();
    req.id = "overview-reviewer".into();
    req.profile.id = "reviewer".into();
    let mut attempt = s
        .attempts
        .iter()
        .find(|a| a.request_id == "overview-request-2")
        .unwrap()
        .clone();
    attempt.id = "overview-reviewer-attempt".into();
    attempt.request_id = req.id.clone();
    attempt.terminal = Some("reviewer-pty".into());
    s.requests.push(req);
    s.attempts.push(attempt);
    for (index, card_index) in [(1, 0), (2, 2)] {
        let mut event = timeline();
        event.id = format!("overview-unread-{index}");
        event.sequence = index;
        event.card_id = s.cards[card_index].id.clone();
        event.project_id = s.cards[card_index].project_id.clone();
        event.kind = "comment.replied".into();
        event.actor = "agent:codex".into();
        event.message = "Added regression coverage; please check the new boundary case.".into();
        event.request_id = Some(s.requests[card_index].id.clone());
        s.events.push(event);
    }
    s.cards[2].body=(0..20).map(|i|format!("### Context {i}\n\nLong task context tests direct navigation to an unread reply.\n\n")).collect();
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
    s
}

/// Terminal navigation adds two profiles on one Card and closed archived history.
pub fn terminal_workspace() -> canban_types::Snapshot {
    let mut s = overview();
    s.projects[0].name = "Canban workspace with a long Project name".into();
    let mut reviewer = s
        .requests
        .iter()
        .find(|r| r.card_id == "overview-card-2")
        .unwrap()
        .clone();
    reviewer.id = "terminal-reviewer-queued".into();
    reviewer.profile.id = "reviewer".into();
    reviewer.profile.preset = Some("claude".into());
    reviewer.stage = "review".into();
    reviewer.state = "queued".into();
    reviewer.created_at += 1;
    s.requests.push(reviewer);
    let mut card = s
        .cards
        .iter()
        .find(|c| c.id == "overview-card-5")
        .unwrap()
        .clone();
    card.id = "terminal-archived".into();
    card.title = "Archived investigation".into();
    card.archived_at = Some(canban_types::now());
    let mut req = s
        .requests
        .iter()
        .find(|r| r.card_id == "overview-card-5")
        .unwrap()
        .clone();
    req.id = "terminal-archive-request".into();
    req.card_id = card.id.clone();
    let mut run = s
        .attempts
        .iter()
        .find(|a| a.request_id == "overview-request-5")
        .unwrap()
        .clone();
    run.id = "terminal-archive-run".into();
    run.request_id = req.id.clone();
    run.card_id = card.id.clone();
    s.cards.push(card);
    s.requests.push(req);
    s.attempts.push(run);
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
        .attempts
        .iter_mut()
        .filter(|a| a.card_id == "overview-card-2")
    {
        attempt.location = Some(canban_types::WorkLocation {
            machine: "gallery-remote".into(),
            directory: "/remote/workspaces/canban".into(),
        });
        attempt.terminal = Some("gallery-remote/t3".into());
    }
    let card = s.cards.iter().find(|c| c.archived_at.is_none()).unwrap();
    s.terminal_contexts.push(canban_types::TerminalContext {
        machine: "local".into(),
        terminal_id: "gallery-linked-shell".into(),
        core_id: "gallery".into(),
        terminal: "linked-shell".into(),
        cwd: Some("/tmp/gallery/worktree".into()),
        project_id: Some(card.project_id.clone()),
        card_id: Some(card.id.clone()),
        branch: Some(
            "canban/very-long-worktree-and-branch-name/context-navigation-acceptance-42".into(),
        ),
    });
    let proj_id = s.projects[0].id.clone();
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
        branch: Some("feat/standalone-spike".into()),
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
        request_id: None,
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
    let mut running = canban_scheduler::request(
        "gallery-project",
        "card-2",
        "develop",
        "stage_entry",
        profile.clone(),
        "Work on the Card".into(),
        None,
    );
    running.state = "working".into();
    running.attempt = 1;
    let running_id = running.id.clone();
    s.requests.push(running);
    let mut running_attempt: Attempt = serde_json::from_value(json!({
        "id": Attempt::key(&running_id, 1),
        "request_id": running_id,
        "project_id": "gallery-project",
        "card_id": "card-2",
        "number": 1,
        "state": "running",
        "core_id": "gallery",
        "terminal": "gallery-card-2-pty",
        "cwd": "/preview/worktrees/card-2",
        "input_path": "input.md",
        "output_path": null,
        "observed": {"state": "live", "status": {"state": "working"}},
        "error": null,
        "created_at": now(),
        "provider_session_id": "session-gallery-card-2",
        "connection_state": "connected"
    }))
    .unwrap();
    running_attempt.lifecycle.observed_at = Some(u64::MAX);
    s.attempts.push(running_attempt);
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
        created_at: now(),
    });
    let mut queued = s.cards[2].clone();
    queued.id = "queued-card".into();
    queued.number = 5;
    queued.title = "Waiting for a free execution slot".into();
    queued.stage = "develop".into();
    let request = canban_scheduler::request(
        "gallery-project",
        &queued.id,
        "develop",
        "stage_entry",
        profile,
        "Wait".into(),
        None,
    );
    s.requests.push(request);
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
            "request_id": null,
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
    let mut merger_req = canban_scheduler::request(
        "gallery-project",
        &conflict.id,
        "review",
        "workflow",
        merger_profile,
        "Merge conflict to resolve before delivery".into(),
        None,
    );
    merger_req.state = "working".into();
    merger_req.workflow_run_id = Some("gallery-conflict-run".into());
    merger_req.workflow_step = Some("deliver:review:2".into());
    let merger_req_id = merger_req.id.clone();
    s.requests.push(merger_req);
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
            "request_id": merger_req_id,
            "trigger_event_id": null,
            "input_digest": "gallery",
            "inputs": {"sync_upstream": true, "conflict_attempts": 1, "queued_at": 200},
            "outputs": {"agent:develop": {"state": "completed"}},
            "history": [{"step": "agent:develop", "visit": 1, "event": "completed"}],
            "error": null,
            "created_at": 0,
            "updated_at": 0
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
            "request_id": null,
            "trigger_event_id": null,
            "input_digest": "gallery",
            "inputs": {"sync_upstream": true, "queued_at": 300},
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
    s
}

/// The production Card reader under long conversational history and background activity.
pub fn conversation() -> canban_types::Snapshot {
    let mut s = workspace();
    s.cards[0].body = "## Goal\nKeep the Card conversation readable while task context stays available.\n\n- [x] Full Markdown description\n- [ ] Keep selection and undo through background updates\n\n> Description, comments and events share one scroll owner.\n\n| Surface | Behavior |\n| --- | --- |\n| Editor | Write / Preview |\n| Agent | Type @ to choose |\n\n```rust\nlet context = card.history();\n```\n\n[Canban](https://github.com/jjyr/canban) · **Readable** and _editable_.\n".into();
    for i in 1..=32 {
        s.events.push(Event {
            id: format!("conversation-{i}"),
            project_id: "gallery-project".into(),
            card_id: "card-1".into(),
            sequence: i + 1,
            kind: if i % 4 == 0 {
                "execution.started"
            } else {
                "comment.created"
            }
            .into(),
            actor: if i % 2 == 0 {
                "agent:developer"
            } else {
                "user"
            }
            .into(),
            message: if i % 4 == 0 {
                "Agent execution started".into()
            } else {
                format!(
                    "Message {i}: **conversation context**\n\n{}",
                    "Keep this reading position while new activity arrives. ".repeat(4)
                )
            },
            request_id: None,
            data: json!({}),
            created_at: canban_types::now(),
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
        "state":"waiting_approval","step":"approve","visit":2,"request_id":null,"trigger_event_id":null,
        "input_digest":"fixture","inputs":{"implement":true},"outputs":{"plan":{"message":"Plan v1: preserve existing runtime and add a regression.","report_id":"plan-v1"}},
        "history":[],"error":null,"created_at":0,"updated_at":0
    })).unwrap()
}

/// Active column rules over real Card text, including the first/last columns.
/// Exercises popover stacking, clipping, dismissal and overflow with production UI.
pub fn automation_board() -> canban_types::Snapshot {
    use canban_types::*;
    let mut snapshot = workspace();
    let raw = include_str!("../workflows/plan-and-develop.yaml");
    let mut document = canban_workflow_engine::source::parse(raw).unwrap();
    for column in ["backlog", "review"] {
        document.on.insert(
            column.into(),
            serde_json::from_value(json!({
                "event":"card.column_entered", "start_at":"plan", "match":{"column":column}
            }))
            .unwrap(),
        );
    }
    for index in 0..5 {
        let workflow_id = format!("gallery-automation-{index}");
        let revision_id = format!("{workflow_id}:1");
        let mut definition = document.compile().unwrap();
        definition.id = workflow_id.clone();
        definition.name = format!("Plan and deliver · automation {}", index + 1);
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
            filename: format!("automation-{index}.yaml"),
            workflow_id,
            revision: 1,
            hash: "gallery".into(),
            revision_id,
            enabled: true,
            columns: document.columns.clone(),
            rules: document.on.clone(),
        });
    }
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
    // demonstrates enabled-agent gating + custom identities (`@qa`).
    let mut agy = canban_profiles::builtin_catalog()
        .into_iter()
        .find(|e| e.id == "agy")
        .unwrap()
        .to_preset_profile();
    agy.enabled = true;
    cfg.profiles.push(agy);
    if let Some(claude) = cfg.profiles.iter_mut().find(|p| p.id == "claude") {
        claude.enabled = false;
    }
    if let Some(rev) = cfg.profiles.iter_mut().find(|p| p.id == "reviewer") {
        rev.preset = Some("agy".into());
    }
    if let Some(merger) = cfg.profiles.iter_mut().find(|p| p.id == "merger") {
        merger.preset = Some("agy".into());
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
    cfg
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
    })
}
