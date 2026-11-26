use super::*;
use canban_github::models::{Label, User};
use canban_storage::Store;
fn fixture() -> (tempfile::TempDir, Store, Project) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let mut project =
        crate::project("test", dir.path().to_str().unwrap(), Workflow::default()).unwrap();
    project.github = Some(GitHubSource {
        pr_intake: Default::default(),
        authors: Vec::new(),
        labels: Vec::new(),
        include_drafts: false,
        paused: false,
        repository: "owner/repo".into(),
        repository_id: 1,
        generation: "generation-a".into(),
        intake: GitHubIntake::All,
        synced_at: None,
        error: None,
    });
    store.write(|db| db.save(&project)).unwrap();
    (dir, store, project)
}
fn change(number: u64) -> Change {
    Change {
        issue: Issue {
            id: number + 100,
            node_id: format!("I_{number}"),
            number,
            title: format!("Issue {number}"),
            body: Some("Remote task description".into()),
            state: "open".into(),
            html_url: format!("https://github.com/owner/repo/issues/{number}"),
            user: User {
                id: 7,
                login: "author".into(),
            },
            labels: vec![Label {
                name: "codex".into(),
            }],
            updated_at: "2026-11-09T00:00:00Z".into(),
            pull_request: None,
        },
        comments: vec![Comment {
            id: number,
            body: "@codex remote text must never run an Agent".into(),
            user: User {
                id: 7,
                login: "author".into(),
            },
            html_url: "https://github.com/owner/repo/issues/1#issuecomment-1".into(),
            updated_at: "2026-11-09T00:00:00Z".into(),
        }],
    }
}
#[test]
fn local_removal_suppresses_sync_and_explicit_readd_restores_history() {
    let (_dir, mut store, p) = fixture();
    let mut c = store
        .write(|db| upsert(db, &p, change(1), true, "Keep context"))
        .unwrap();
    store.write(|db| remove(db, &mut c)).unwrap();
    store
        .write(|db| {
            apply(
                db,
                &p,
                "generation-a",
                Sync {
                    changes: vec![change(1)],
                },
            )
        })
        .unwrap();
    assert!(store.snapshot().unwrap().cards.is_empty());
    let restored = store
        .write(|db| upsert(db, &p, change(1), true, ""))
        .unwrap();
    assert_eq!(restored.id, c.id);
    assert_eq!(restored.number, c.number);
    assert_eq!(restored.body, "Keep context");
    assert_eq!(store.snapshot().unwrap().cards.len(), 1);
    assert!(
        store
            .db()
            .list::<Event>()
            .unwrap()
            .iter()
            .any(|e| e.kind == "card.removed")
    );
}
#[test]
fn remote_closure_keeps_local_stage_instructions_and_history() {
    let (_dir, mut store, p) = fixture();
    let mut c = store
        .write(|db| upsert(db, &p, change(1), true, "Local requirements"))
        .unwrap();
    c.stage = "review".into();
    store.write(|db| db.save(&c)).unwrap();
    let mut update = change(1);
    update.issue.state = "closed".into();
    update.issue.title = "Updated remotely".into();
    update.issue.updated_at = "2026-11-09T01:00:00Z".into();
    let updated = store.write(|db| upsert(db, &p, update, false, "")).unwrap();
    assert_eq!(updated.stage, "review");
    assert_eq!(updated.body, "Local requirements");
    assert_eq!(updated.title, "owner/repo#1");
    assert!(updated.archived_at.is_none());
}
#[test]
fn accepts_pull_request_as_source_without_copying_remote_content() {
    let (_dir, mut store, p) = fixture();
    let mut c = change(1);
    c.issue.pull_request = Some(json!({"url":"https://api.github.com/"}));
    let card = store.write(|db| upsert(db, &p, c, true, "")).unwrap();
    assert!(card.is_github_pr());
    assert_eq!(card.title, "owner/repo#1");
    assert!(store.snapshot().unwrap().github_issues.is_empty());
}
#[test]
fn summarize_checks_supports_fast_fail_running_passed_and_commit_statuses() {
    let pr_url = "https://github.com/owner/repo/pull/42";
    let sha = "abcdef1234567890abcdef1234567890abcdef12";

    let none = delivery::summarize_checks(pr_url, sha, &[], &json!({"total_count": 0}));
    assert_eq!(none.state, "none");
    assert_eq!(none.total, 0);

    let running = delivery::summarize_checks(
        pr_url,
        sha,
        &[
            json!({"name": "fmt", "status": "completed", "conclusion": "success", "html_url": "https://github.com/owner/repo/runs/1"}),
            json!({"name": "test", "status": "in_progress"}),
        ],
        &json!({}),
    );
    assert_eq!(running.state, "running");
    assert_eq!(
        (
            running.total,
            running.passed,
            running.failed,
            running.running
        ),
        (2, 1, 0, 1)
    );

    // Option B fast-fail: failure triggers immediately even while other checks are still running.
    let fast_fail = delivery::summarize_checks(
        pr_url,
        sha,
        &[
            json!({"name": "clippy", "status": "completed", "conclusion": "failure", "html_url": "https://github.com/owner/repo/runs/2"}),
            json!({"name": "test", "status": "in_progress"}),
        ],
        &json!({}),
    );
    assert_eq!(fast_fail.state, "failure");
    assert_eq!(
        (
            fast_fail.total,
            fast_fail.passed,
            fast_fail.failed,
            fast_fail.running
        ),
        (2, 0, 1, 1)
    );
    assert_eq!(fast_fail.failed_checks.len(), 1);
    assert_eq!(fast_fail.failed_checks[0].name, "clippy");
    assert_eq!(
        fast_fail.failed_checks[0].url.as_deref(),
        Some("https://github.com/owner/repo/runs/2")
    );
    assert_eq!(
        fast_fail.url.as_deref(),
        Some("https://github.com/owner/repo/pull/42/checks")
    );

    let passed = delivery::summarize_checks(
        pr_url,
        sha,
        &[
            json!({"name": "clippy", "status": "completed", "conclusion": "success"}),
            json!({"name": "docs", "status": "completed", "conclusion": "skipped"}),
        ],
        &json!({
            "statuses": [
                {"context": "ci/external", "state": "success", "target_url": "https://ci.example.com/1"}
            ]
        }),
    );
    assert_eq!(passed.state, "success");
    assert_eq!(
        (passed.total, passed.passed, passed.failed, passed.running),
        (3, 3, 0, 0)
    );
}

#[test]
fn branch_pr_matching_prefers_open_same_repo_respects_history_and_matches_merged_by_sha() {
    let prs = vec![
        json!({
            "number": 10,
            "state": "closed",
            "merged_at": "2026-11-01T00:00:00Z",
            "head": {"ref": "canban-0afd/143", "sha": "oldsha", "repo": {"id": 1}, "user": {"login": "alice"}}
        }),
        json!({
            "number": 11,
            "state": "open",
            "head": {"ref": "canban-0afd/143", "sha": "forksha", "repo": {"id": 999}, "user": {"login": "bob"}}
        }),
        json!({
            "number": 12,
            "state": "open",
            "head": {"ref": "canban-0afd/143", "sha": "mainsha", "repo": {"id": 1}, "user": {"login": "alice"}}
        }),
    ];
    let picked =
        pull_requests::match_branch_pr(&prs, 1, "canban-0afd/143", &[], None, None).unwrap();
    assert_eq!(picked["number"], 12);
    // Excluding #12 falls back to the unique remaining open fork PR #11 (when push_owner matches bob).
    let fork = pull_requests::match_branch_pr(&prs, 1, "canban-0afd/143", &[12], Some("bob"), None)
        .unwrap();
    assert_eq!(fork["number"], 11);
    // Closed/merged PR #10 is only matched when local_head_sha matches its head SHA.
    assert!(
        pull_requests::match_branch_pr(&prs, 1, "canban-0afd/143", &[11, 12], None, None).is_none()
    );
    let merged =
        pull_requests::match_branch_pr(&prs, 1, "canban-0afd/143", &[11, 12], None, Some("oldsha"))
            .unwrap();
    assert_eq!(merged["number"], 10);
}
