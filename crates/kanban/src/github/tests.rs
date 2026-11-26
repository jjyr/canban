use super::*;
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
