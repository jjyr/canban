use super::*;
fn github_setup() -> (tempfile::TempDir, Arc<AppServer>, Project) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join("credentials")).unwrap();
    std::fs::write(home.join("credentials/github.json"),serde_json::to_vec(&json!({"client_id":canban_github::auth::CLIENT_ID,"login":"fixture","access_token":"fixture-not-a-real-token","refresh_token":null,"expires_at":null})).unwrap()).unwrap();
    let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    let p: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name":"GitHub","directory":dir.path().join("source")}),
        )
        .unwrap(),
    )
    .unwrap();
    (dir, app, p)
}
#[test]
fn github_pages_survive_restart_without_creating_cards_or_mirroring_bodies() {
    let (_dir, app, p) = github_setup();
    let client = app.github.cached_client().unwrap();
    let path = |page| {
        format!(
            "/search/issues?q={}&sort=updated&order=desc&per_page=30&page={page}",
            canban_github::segment("is:issue is:open ")
        )
    };
    for page in [1, 2] {
        client.seed_response(&path(page),&canban_github::cache::Response{data:json!({"items":[{"number":page,"title":"REMOTE_BODY_SENTINEL","html_url":format!("https://github.com/owner/repo/issues/{page}")}]}),link:(page==1).then(||"<https://api.github.com/x?page=2>; rel=\"next\"".into()),etag:None,modified:None,poll_interval:None,validated_at:1,fetched_at:0,cached:false});
    }
    let q = |page| json!({"kind":"issues","relation":"all","state":"open","query":"","page":page});
    let first = app.call("github.browse", q(1)).unwrap();
    assert_eq!(first["items"][0]["item"]["number"], 1);
    assert_eq!(first["has_next"], true);
    let second = app.call("github.browse", q(2)).unwrap();
    assert_eq!(second["items"][0]["item"]["number"], 2);
    assert_eq!(second["has_next"], false);
    assert!(app.snapshot().unwrap().cards.is_empty());
    let texts:Vec<String>=app.store.lock().unwrap().db().conn.prepare("SELECT data FROM projects UNION ALL SELECT data FROM cards UNION ALL SELECT data FROM events").unwrap().query_map([],|r|r.get(0)).unwrap().map(Result::unwrap).collect();
    assert!(!texts.join("").contains("REMOTE_BODY_SENTINEL"));
    let home = app.home.clone();
    drop(app);
    let restarted = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    assert_eq!(
        restarted.call("github.browse", q(1)).unwrap()["items"],
        first["items"]
    );
    assert_eq!(restarted.snapshot().unwrap().projects[0].id, p.id);
}
#[test]
fn references_keep_local_identity_and_require_current_revision() {
    let (_d, app, p, c) = setup();
    let saved:Card=serde_json::from_value(app.call("github.reference.add",json!({"card_id":c.id,"revision":c.revision,"url":"https://github.com/a/b/pull/12/files"})).unwrap()).unwrap();
    assert_eq!(saved.content, CardContent::Local);
    assert_eq!(saved.github_links[0].url, "https://github.com/a/b/pull/12");
    assert!(
        app.call(
            "github.reference.add",
            json!({"card_id":c.id,"revision":c.revision,"url":"https://github.com/a/b/issues/2"})
        )
        .is_err()
    );
    assert!(
        app.snapshot()
            .unwrap()
            .terminals
            .iter()
            .all(|t| t.agent.is_none())
    );
    assert_eq!(app.snapshot().unwrap().projects[0].id, p.id);
    let removed: Card = serde_json::from_value(
        app.call(
            "github.reference.remove",
            json!({"card_id":c.id,"revision":saved.revision,"reference":saved.github_links[0]}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(removed.github_links.is_empty());
}
#[test]
fn legacy_remote_mirror_moves_to_disposable_cache_without_losing_local_history() {
    let (_d, app, p, c) = setup();
    let home = app.home.clone();
    let local = canban_timeline::event(
        &p.id,
        &c.id,
        "comment.added",
        "user",
        "LOCAL_HISTORY",
        json!({}),
    );
    let remote = canban_timeline::event(
        &p.id,
        &c.id,
        "github.comment",
        "github:someone",
        "REMOTE_HISTORY",
        json!({"external_revision":"old"}),
    );
    app.store.lock().unwrap().write(|db|{db.append(local)?;let mut legacy=remote;legacy.sequence=db.counter(&format!("timeline:{}",c.id))?;db.conn.execute("INSERT INTO events(id,project_id,card_id,state,number,data) VALUES(?1,?2,?3,'',?4,?5)",params![legacy.id,legacy.project_id,legacy.card_id,legacy.sequence,serde_json::to_string(&legacy)?])?;Ok(())}).unwrap();
    drop(app);
    let db = rusqlite::Connection::open(home.join("canban.sqlite")).unwrap();
    db.execute_batch("PRAGMA user_version=8;").unwrap();
    drop(db);
    let migrated = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let s = migrated.snapshot().unwrap();
    assert!(s.events.iter().any(|e| e.message == "LOCAL_HISTORY"));
    assert!(!s.events.iter().any(|e| e.message == "REMOTE_HISTORY"));
    assert_eq!(s.cards[0].id, c.id);
    assert!(
        std::fs::read_to_string(home.join("cache/github/legacy/v8.json"))
            .unwrap()
            .contains("REMOTE_HISTORY")
    );
}

#[test]
fn migration_sanitizes_source_titles_in_events_and_receipts_but_retains_local_instructions() {
    let (_d, app, p, mut card) = setup();
    let home = app.home.clone();
    card.content = CardContent::GitHubIssue {
        source: GitHubIssueRef {
            repository: "owner/repo".into(),
            repository_id: 1,
            issue_id: 101,
            number: 1,
            url: "https://github.com/owner/repo/issues/1".into(),
        },
    };
    card.title = "LEGACY_REMOTE_TITLE".into();
    card.body = "LOCAL_INSTRUCTIONS".into();
    app.store.lock().unwrap().write(|db|{
        let data=serde_json::to_string(&card)?;
        db.conn.execute("UPDATE cards SET data=?2 WHERE id=?1",params![card.id,data])?;
        db.conn.execute("UPDATE events SET data=json_set(data,'$.data.title','LEGACY_REMOTE_TITLE') WHERE card_id=?1 AND json_extract(data,'$.kind')='card.created'",[&card.id])?;
        db.conn.execute("INSERT INTO operations(id,fingerprint,result) VALUES('legacy-card','fixture',?1)",[data])?;
        db.conn.execute_batch("PRAGMA user_version=8;")?;Ok(())
    }).unwrap();
    drop(app);
    let migrated = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let store = migrated.store.lock().unwrap();
    let db = store.db();
    let saved: Card = db.get(&card.id).unwrap();
    assert_eq!(saved.title, "owner/repo#1");
    assert_eq!(saved.body, "LOCAL_INSTRUCTIONS");
    assert_eq!(saved.project_id, p.id);
    assert!(
        db.list::<Event>()
            .unwrap()
            .iter()
            .all(|e| !e.data.to_string().contains("LEGACY_REMOTE_TITLE"))
    );
    let receipt: String = db
        .conn
        .query_row(
            "SELECT result FROM operations WHERE id='legacy-card'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!receipt.contains("LEGACY_REMOTE_TITLE"));
    assert!(receipt.contains("LOCAL_INSTRUCTIONS"));
    assert!(
        std::fs::read_to_string(home.join("cache/github/legacy/v8.json"))
            .unwrap()
            .contains("LEGACY_REMOTE_TITLE")
    );
}

#[test]
fn project_auto_detects_github_remote_links_with_intake_off_and_respects_manual_disconnect() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join("credentials")).unwrap();
    std::fs::write(
        home.join("credentials/github.json"),
        serde_json::to_vec(&json!({
            "client_id": canban_github::auth::CLIENT_ID,
            "login": "fixture",
            "access_token": "fixture-not-a-real-token",
            "refresh_token": null,
            "expires_at": null
        }))
        .unwrap(),
    )
    .unwrap();

    let source = dir.path().join("repo-source");
    std::fs::create_dir_all(&source).unwrap();
    for args in [
        &["init", "-b", "main"][..],
        &["remote", "add", "origin", "git@github.com:jjyr/canban.git"][..],
    ] {
        let ok = canban_exec::raw_command("git")
            .arg("-C")
            .arg(&source)
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
    app.github.cached_client().unwrap().seed(
        "/repos/jjyr/canban",
        json!({
            "id": 777,
            "full_name": "jjyr/canban",
            "default_branch": "main",
            "html_url": "https://github.com/jjyr/canban"
        }),
    );

    let probe = app
        .call(
            "project.detect_github",
            json!({ "machine": "local", "directory": source }),
        )
        .unwrap();
    assert_eq!(probe["detected"]["repository"], "jjyr/canban");
    assert_eq!(probe["detected"]["remote"], "origin");

    // 1. Default creation auto-links with Intake Off + PR Intake Off.
    let created: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({ "name": "Canban", "directory": source }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        created.detected_github,
        Some(DetectedGitHub {
            repository: "jjyr/canban".into(),
            remote: "origin".into(),
        })
    );
    assert!(!created.github_disconnected);
    let gh = created.github.as_ref().expect("auto-linked GitHubSource");
    assert_eq!(gh.repository, "jjyr/canban");
    assert_eq!(gh.repository_id, 777);
    assert_eq!(gh.intake, GitHubIntake::Off);
    assert_eq!(gh.pr_intake, GitHubPrIntake::Off);

    // 2. Manual disconnect sets `github_disconnected = true` and prevents background auto-relink.
    let disconnected: Project = serde_json::from_value(
        app.call(
            "github.source.disconnect",
            json!({ "project_id": created.id }),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(disconnected.github.is_none());
    assert!(disconnected.github_disconnected);
    assert_eq!(
        disconnected
            .detected_github
            .as_ref()
            .map(|d| d.repository.as_str()),
        Some("jjyr/canban")
    );
    app.reconcile_project_github_bindings();
    let after_reconcile: Project = app.store.lock().unwrap().db().get(&created.id).unwrap();
    assert!(after_reconcile.github.is_none());
    assert!(after_reconcile.github_disconnected);

    // 3. Existing legacy unlinked project (`github: None, github_disconnected: false`) auto-links on reconcile.
    let legacy_source = dir.path().join("legacy-source");
    std::fs::create_dir_all(&legacy_source).unwrap();
    for args in [
        &["init", "-b", "main"][..],
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/jjyr/canban.git",
        ][..],
    ] {
        assert!(
            canban_exec::raw_command("git")
                .arg("-C")
                .arg(&legacy_source)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let opted_out: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "Opted Out",
                "directory": legacy_source,
                "github_auto_link": false
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(opted_out.github.is_none());
    assert!(opted_out.github_disconnected);
    // Simulate a pre-upgrade row where `github_disconnected` and `detected_github` were absent.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut p: Project = db.get(&opted_out.id)?;
            p.github_disconnected = false;
            p.detected_github = None;
            db.save(&p)
        })
        .unwrap();
    app.reconcile_project_github_bindings();
    let upgraded: Project = app.store.lock().unwrap().db().get(&opted_out.id).unwrap();
    let upgraded_gh = upgraded
        .github
        .as_ref()
        .expect("legacy project auto-linked on reconcile");
    assert_eq!(upgraded_gh.repository, "jjyr/canban");
    assert_eq!(upgraded_gh.intake, GitHubIntake::Off);
    assert_eq!(upgraded_gh.pr_intake, GitHubPrIntake::Off);
}

#[test]
fn delivery_review_gate_does_not_block_github_remote_actions() {
    let (_dir, app, _p) = github_setup();

    // Holding the main checkout gate (as `pump_deliveries` or `git.commit` does) must never reject
    // `github.remote.*` or `github.notification.*` with "Another GitHub action is pending".
    let _delivery_guard = app.lock_checkout(&_p, "test delivery");
    let err = app
        .call(
            "github.remote.comment",
            json!({
                "reference": {
                    "repository": "fixture/repo",
                    "kind": "pr",
                    "number": 2,
                    "url": "https://github.com/fixture/repo/pull/2"
                },
                "body": "",
                "request_key": "no-false-pending"
            }),
        )
        .unwrap_err()
        .to_string();
    assert_eq!(err, "Comment is empty");

    // Meanwhile, concurrent GitHub remote actions still serialize via `github_action_gate`.
    let _gh_guard = app.github_action_gate.lock().unwrap();
    let busy_err = app
        .call(
            "github.remote.comment",
            json!({
                "reference": {
                    "repository": "fixture/repo",
                    "kind": "pr",
                    "number": 2,
                    "url": "https://github.com/fixture/repo/pull/2"
                },
                "body": "hello",
                "request_key": "serialized-gh-action"
            }),
        )
        .unwrap_err()
        .to_string();
    assert_eq!(busy_err, "Another GitHub action is pending");
}

#[test]
fn github_issue_upsert_triggers_only_from_local_instructions_not_remote_issue_body() {
    let (_d, app, p, _c) = setup();
    let config = app.config();
    let issue: canban_github::models::Issue = serde_json::from_value(json!({
        "id": 4201,
        "node_id": "I_kwDO42",
        "number": 42,
        "title": "Remote @reviewer mention should not trigger",
        "body": "Remote body mentions @planner",
        "state": "open",
        "html_url": "https://github.com/jjyr/canban/issues/42",
        "user": {"id": 1, "login": "octocat"},
        "labels": [],
        "assignees": [],
        "updated_at": "2026-11-01T00:00:00Z"
    }))
    .unwrap();
    let mut project = p;
    project.github = Some(
        serde_json::from_value(json!({
            "repository": "jjyr/canban",
            "repository_id": 777,
            "generation": "g1",
            "intake": "off"
        }))
        .unwrap(),
    );

    let card_id = app
        .store
        .lock()
        .unwrap()
        .write(|db| {
            db.save(&project)?;
            let instructions = "@developer Investigate locally";
            let mut card = canban_kanban::github::upsert(
                db,
                &project,
                canban_kanban::github::Change {
                    issue,
                    comments: Vec::new(),
                },
                true,
                instructions,
            )?;
            crate::sessions::queue_card_mentions(
                db,
                &config,
                &mut card,
                &project,
                &json!({}),
                &[instructions],
            )?;
            Ok(card.id)
        })
        .unwrap();

    let snap = app.snapshot().unwrap();
    let runs: Vec<_> = snap
        .terminals
        .iter()
        .filter_map(|t| t.agent.as_ref().filter(|a| a.card_id == card_id))
        .collect();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].profile.id, "developer");
    assert!(runs[0].trigger.is_comment());
    assert!(
        runs[0]
            .instruction
            .contains("You were mentioned (@developer) on this Card.")
    );
    assert_eq!(
        snap.questions
            .iter()
            .filter(|q| q.card_id == card_id && q.profile_id == "developer")
            .count(),
        1
    );
}

#[test]
fn review_github_pr_file_decodes_contents_and_blob_fallback() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let (_dir, app, _p) = github_setup();
    let client = app.github.cached_client().unwrap();

    let encoded_small = STANDARD.encode("alpha\nbeta\ngamma\n");
    client.seed_response(
        "/repos/owner/repo/contents/src/main.rs?ref=1234567890abcdef1234567890abcdef12345678",
        &canban_github::cache::Response {
            data: json!({
                "encoding": "base64",
                "content": format!("{encoded_small}\n"),
            }),
            link: None,
            etag: None,
            modified: None,
            poll_interval: None,
            validated_at: 1,
            fetched_at: 0,
            cached: false,
        },
    );
    let resp = app
        .call(
            "review.github_pr_file",
            json!({
                "repository": "owner/repo",
                "sha": "1234567890abcdef1234567890abcdef12345678",
                "path": "src/main.rs",
            }),
        )
        .unwrap();
    assert_eq!(resp["total_lines"], 3);
    assert_eq!(resp["lines"], json!(["alpha", "beta", "gamma"]));

    // When /contents omits base64 content (large file), fall back to /git/blobs/<sha>.
    let blob_sha = "abcdef1234567890abcdef1234567890abcdef12";
    client.seed_response(
        "/repos/owner/repo/contents/src/big.rs?ref=1234567890abcdef1234567890abcdef12345678",
        &canban_github::cache::Response {
            data: json!({
                "encoding": "none",
                "sha": blob_sha,
            }),
            link: None,
            etag: None,
            modified: None,
            poll_interval: None,
            validated_at: 1,
            fetched_at: 0,
            cached: false,
        },
    );
    let encoded_big = STANDARD.encode("row1\nrow2\n");
    client.seed_response(
        &format!("/repos/owner/repo/git/blobs/{blob_sha}"),
        &canban_github::cache::Response {
            data: json!({
                "encoding": "base64",
                "content": encoded_big,
            }),
            link: None,
            etag: None,
            modified: None,
            poll_interval: None,
            validated_at: 1,
            fetched_at: 0,
            cached: false,
        },
    );
    let blob_resp = app
        .call(
            "review.github_pr_file",
            json!({
                "repository": "owner/repo",
                "sha": "1234567890abcdef1234567890abcdef12345678",
                "path": "src/big.rs",
            }),
        )
        .unwrap();
    assert_eq!(blob_resp["total_lines"], 2);
    assert_eq!(blob_resp["lines"], json!(["row1", "row2"]));
}

#[test]
fn github_pr_delivery_is_isolated_from_main_checkout_and_other_cards() {
    let (_dir, app, p) = github_setup();
    let c1: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "PR Card 1"}),
        )
        .unwrap(),
    )
    .unwrap();
    let c2: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full","project_id": p.id, "title": "PR Card 2"}),
        )
        .unwrap(),
    )
    .unwrap();

    // 1. Holding the main checkout gate must not reject `github.pr.publish` or `github.pr.merge`
    // with a delivery lock error.
    let _checkout_guard = app.lock_checkout(&p, "Code delivery for GitHub (main)");
    let pub_err = app
        .call(
            "github.pr.publish",
            json!({"card_id": c1.id, "revision": c1.revision, "request_key": "pub-while-checkout-locked"}),
        )
        .unwrap_err()
        .to_string();
    assert_eq!(pub_err, "Connect this Project to GitHub first");

    let merge_err = app
        .call(
            "github.pr.merge",
            json!({"card_id": c1.id, "revision": c1.revision, "head": "abc", "request_key": "merge-while-checkout-locked"}),
        )
        .unwrap_err()
        .to_string();
    assert!(
        !merge_err.contains("Another delivery"),
        "main checkout lock must not reject github.pr.merge: {merge_err}"
    );

    // 2. Per-card delivery gate rejects concurrent publish on `c1` while leaving `c2` unblocked.
    let _c1_guard = app
        .try_lock_card_delivery(&c1.id, "GitHub PR publish")
        .unwrap();
    let c1_busy = app
        .call(
            "github.pr.publish",
            json!({"card_id": c1.id, "revision": c1.revision, "request_key": "pub-c1-concurrent"}),
        )
        .unwrap_err()
        .to_string();
    assert_eq!(
        c1_busy,
        "Another delivery for this Card is in progress; retry after it completes"
    );

    let c2_err = app
        .call(
            "github.pr.publish",
            json!({"card_id": c2.id, "revision": c2.revision, "request_key": "pub-c2-unblocked"}),
        )
        .unwrap_err()
        .to_string();
    assert_eq!(c2_err, "Connect this Project to GitHub first");
}

#[test]
fn branch_pr_discovery_tracks_fast_fail_ci_dedup_rerun_and_triggers_workflow() {
    let (_dir, app, mut p) = github_setup();
    let client = app.github.cached_client().unwrap();
    let seed = |path: &str, data: Value| {
        client.seed_response(
            path,
            &canban_github::cache::Response {
                data,
                link: None,
                etag: None,
                modified: None,
                poll_interval: None,
                validated_at: 1,
                fetched_at: u64::MAX,
                cached: false,
            },
        );
    };
    seed(
        "/repos/owner/repo",
        json!({
            "id": 42,
            "full_name": "owner/repo",
            "default_branch": "main",
            "html_url": "https://github.com/owner/repo"
        }),
    );

    p.github = Some(GitHubSource {
        repository: "owner/repo".into(),
        repository_id: 42,
        generation: "gen-1".into(),
        intake: GitHubIntake::Off,
        pr_intake: GitHubPrIntake::Off,
        authors: vec![],
        labels: vec![],
        include_drafts: false,
        paused: false,
        synced_at: None,
        error: None,
    });
    app.store.lock().unwrap().write(|db| db.save(&p)).unwrap();

    let sources = app
        .call("workflow.sources", json!({"project_id": p.id}))
        .unwrap();
    let wf_yaml = format!(
        r#"schema: {}
id: ci-watcher
name: CI Watcher
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  ci_fix:
    event: github.pr.ci_failed
    start_at: notify_ci_fail
steps:
  - id: notify_ci_fail
    uses: notify
    with:
      message: "CI failure noticed"
  - id: done
    uses: complete
"#,
        canban_workflow_engine::migrations::CURRENT
    );
    app.call(
        "workflow.apply",
        json!({
            "project_id": p.id,
            "revision": sources["revision"],
            "sources": [{"filename": "development.yaml", "source": wf_yaml}],
        }),
    )
    .unwrap();

    let mut card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": p.id, "title": "Track PR CI"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            card.branch = Some("canban-0afd/143".into());
            card.revision += 1;
            db.save(&card)
        })
        .unwrap();

    let sha = "1234567890abcdef1234567890abcdef12345678";
    seed(
        "/repos/owner/repo/pulls?state=open&sort=updated&direction=desc&per_page=100",
        json!([{
            "number": 148,
            "state": "open",
            "html_url": "https://github.com/owner/repo/pull/148",
            "head": {"ref": "canban-0afd/143", "sha": sha, "label": "owner:canban-0afd/143", "repo": {"id": 42}, "user": {"login": "fixture"}},
            "base": {"ref": "main"}
        }]),
    );
    seed(
        "/repos/owner/repo/pulls/148",
        json!({
            "number": 148,
            "state": "open",
            "merged": false,
            "html_url": "https://github.com/owner/repo/pull/148",
            "head": {"ref": "canban-0afd/143", "sha": sha, "label": "owner:canban-0afd/143", "repo": {"id": 42}, "user": {"login": "fixture"}},
            "base": {"ref": "main"}
        }),
    );
    seed(
        "/repos/owner/repo/pulls/148/reviews?per_page=100&page=1",
        json!([]),
    );
    seed(
        "/repos/owner/repo/issues/148/comments?per_page=100&page=1",
        json!([]),
    );
    seed(
        "/repos/owner/repo/pulls/148/comments?per_page=100&page=1",
        json!([]),
    );
    seed(
        &format!("/repos/owner/repo/commits/{sha}/status"),
        json!({"total_count": 0, "statuses": []}),
    );
    // Option B fast-fail: clippy failed while test is still in_progress.
    seed(
        &format!("/repos/owner/repo/commits/{sha}/check-runs?per_page=100&page=1"),
        json!({
            "check_runs": [
                {"name": "clippy", "status": "completed", "conclusion": "failure", "html_url": "https://github.com/owner/repo/actions/runs/1/job/9"},
                {"name": "test", "status": "in_progress"}
            ]
        }),
    );

    app.github_discover_branch_prs(&p).unwrap();

    let snap1 = app.snapshot().unwrap();
    let card1 = snap1.cards.iter().find(|c| c.id == card.id).unwrap();
    let pr1 = card1.pull_request.as_ref().expect("PR linked from branch");
    assert_eq!(pr1.number, 148);
    assert_eq!(pr1.checks, "failure");
    let summary1 = pr1.checks_summary.as_ref().unwrap();
    assert_eq!(summary1.failed, 1);
    assert_eq!(summary1.running, 1);
    assert_eq!(summary1.failed_checks[0].name, "clippy");

    let failed_events: Vec<_> = snap1
        .events
        .iter()
        .filter(|e| e.card_id == card.id && e.kind == "github.pr.ci_failed")
        .collect();
    assert_eq!(failed_events.len(), 1);
    assert!(
        snap1
            .events
            .iter()
            .any(|e| e.card_id == card.id && e.kind == "github.pr_linked")
    );
    let wf_run = snap1
        .workflow_runs
        .iter()
        .find(|r| r.card_id == card.id && r.workflow_id == "ci-watcher")
        .expect("CI failed workflow must trigger");
    assert_eq!(
        wf_run.inputs["trigger_event"]["kind"],
        "github.pr.ci_failed"
    );
    assert_eq!(wf_run.inputs["trigger_event"]["data"]["number"], 148);
    // Automated CI workflow took over, clearing passive attention banner.
    assert!(card1.attention.is_none());

    // Repeated refresh on the same SHA & outcome must not emit a duplicate failure event.
    app.call("github.pr.refresh", json!({"card_id": card.id}))
        .unwrap();
    let snap2 = app.snapshot().unwrap();
    assert_eq!(
        snap2
            .events
            .iter()
            .filter(|e| e.card_id == card.id && e.kind == "github.pr.ci_failed")
            .count(),
        1
    );

    // Re-run on GitHub transitions to running, then to success -> emits `github.pr.ci_succeeded`.
    seed(
        &format!("/repos/owner/repo/commits/{sha}/check-runs?per_page=100&page=1"),
        json!({
            "check_runs": [
                {"name": "clippy", "status": "in_progress"},
                {"name": "test", "status": "in_progress"}
            ]
        }),
    );
    app.call("github.pr.refresh", json!({"card_id": card.id}))
        .unwrap();
    let snap_running = app.snapshot().unwrap();
    assert_eq!(
        snap_running
            .cards
            .iter()
            .find(|c| c.id == card.id)
            .unwrap()
            .pull_request
            .as_ref()
            .unwrap()
            .checks,
        "running"
    );

    seed(
        &format!("/repos/owner/repo/commits/{sha}/check-runs?per_page=100&page=1"),
        json!({
            "check_runs": [
                {"name": "clippy", "status": "completed", "conclusion": "success"},
                {"name": "test", "status": "completed", "conclusion": "success"}
            ]
        }),
    );
    app.call("github.pr.refresh", json!({"card_id": card.id}))
        .unwrap();
    let snap3 = app.snapshot().unwrap();
    let pr3 = snap3
        .cards
        .iter()
        .find(|c| c.id == card.id)
        .unwrap()
        .pull_request
        .as_ref()
        .unwrap();
    assert_eq!(pr3.checks, "success");
    assert_eq!(
        snap3
            .events
            .iter()
            .filter(|e| e.card_id == card.id && e.kind == "github.pr.ci_succeeded")
            .count(),
        1
    );

    // Remote CI summaries stay out of durable SQLite `cards` JSON.
    let raw_card: String = app
        .store
        .lock()
        .unwrap()
        .db()
        .conn
        .query_row("SELECT data FROM cards WHERE id=?1", [&card.id], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(!raw_card.contains("checks_summary"));
}

#[test]
fn pr_activity_updates_record_commits_comments_reviews_and_hydrate_ephemeral_bodies() {
    let (_dir, app, mut project) = github_setup();
    project.github = Some(GitHubSource {
        repository: "owner/repo".into(),
        repository_id: 99,
        generation: "gen-1".into(),
        intake: GitHubIntake::Off,
        pr_intake: GitHubPrIntake::Off,
        authors: vec![],
        labels: vec![],
        include_drafts: false,
        paused: false,
        synced_at: None,
        error: None,
    });
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&project))
        .unwrap();

    let mut card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": project.id, "title": "Track PR activity"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            card.branch = Some("feat/pr-updates".into());
            card.revision += 1;
            db.save(&card)
        })
        .unwrap();

    let client = app.github.cached_client().unwrap();
    let seed = |path: &str, data: Value| {
        client.seed_response(
            path,
            &canban_github::cache::Response {
                data,
                link: None,
                etag: None,
                modified: None,
                poll_interval: None,
                validated_at: 1,
                fetched_at: u64::MAX,
                cached: false,
            },
        );
    };

    let sha1 = "1111111111111111111111111111111111111111";
    let sha2 = "2222222222222222222222222222222222222222";
    let pr1_json = json!({
        "number": 149,
        "state": "open",
        "draft": false,
        "html_url": "https://github.com/owner/repo/pull/149",
        "updated_at": "2026-11-19T00:00:00Z",
        "head": {"ref": "feat/pr-updates", "sha": sha1, "label": "owner:feat/pr-updates", "repo": {"id": 99}, "user": {"login": "fixture"}},
        "base": {"ref": "main", "repo": {"id": 99}}
    });

    seed(
        "/repos/owner/repo",
        json!({
            "id": 99,
            "full_name": "owner/repo",
            "default_branch": "main",
            "html_url": "https://github.com/owner/repo"
        }),
    );
    seed(
        "/repos/owner/repo/pulls?state=open&sort=updated&direction=desc&per_page=100",
        json!([pr1_json.clone()]),
    );
    seed("/repos/owner/repo/pulls/149", pr1_json.clone());
    seed(
        &format!(
            "/repos/owner/repo/pulls?state=open&head={}&per_page=10",
            canban_github::segment("owner:feat/pr-updates")
        ),
        json!([pr1_json]),
    );
    seed(
        &format!("/repos/owner/repo/commits/{sha1}/check-runs?per_page=100&page=1"),
        json!({"check_runs": [{"name": "test", "status": "completed", "conclusion": "success"}]}),
    );
    seed(
        &format!("/repos/owner/repo/commits/{sha1}/status"),
        json!({"state": "success", "statuses": []}),
    );
    seed(
        "/repos/owner/repo/issues/149/comments?per_page=100&page=1",
        json!([]),
    );
    seed(
        "/repos/owner/repo/pulls/149/comments?per_page=100&page=1",
        json!([]),
    );
    seed(
        "/repos/owner/repo/pulls/149/reviews?per_page=100&page=1",
        json!([]),
    );

    // First refresh baselines existing PR state & head SHA.
    app.call("github.pr.refresh", json!({"card_id": card.id}))
        .unwrap();

    // Push new commits, add external comment + inline review comment + CHANGES_REQUESTED review + own comment.
    let pr2_json = json!({
        "number": 149,
        "state": "open",
        "draft": false,
        "html_url": "https://github.com/owner/repo/pull/149",
        "updated_at": "2026-11-19T01:00:00Z",
        "head": {"ref": "feat/pr-updates", "sha": sha2, "repo": {"id": 99}},
        "base": {"ref": "main", "repo": {"id": 99}}
    });
    seed("/repos/owner/repo/pulls/149", pr2_json);
    seed(
        &format!("/repos/owner/repo/commits/{sha2}/check-runs?per_page=100&page=1"),
        json!({"check_runs": [{"name": "test", "status": "completed", "conclusion": "success"}]}),
    );
    seed(
        &format!("/repos/owner/repo/commits/{sha2}/status"),
        json!({"state": "success", "statuses": []}),
    );
    seed(
        &format!("/repos/owner/repo/compare/{sha1}...{sha2}"),
        json!({
            "html_url": format!("https://github.com/owner/repo/compare/{sha1}...{sha2}"),
            "commits": [
                {"sha": sha2, "commit": {"message": "Fix edge case"}, "author": {"login": "alice"}}
            ]
        }),
    );
    seed(
        "/repos/owner/repo/issues/149/comments?per_page=100&page=1",
        json!([
            {
                "id": 9001,
                "user": {"login": "alice"},
                "body": "Remote PR discussion body should stay ephemeral",
                "html_url": "https://github.com/owner/repo/pull/149#issuecomment-9001"
            }
        ]),
    );
    seed(
        "/repos/owner/repo/pulls/149/comments?per_page=100&page=1",
        json!([
            {
                "id": 9002,
                "user": {"login": "bob"},
                "path": "crates/ui/src/overview.rs",
                "line": 42,
                "body": "Inline review comment body",
                "html_url": "https://github.com/owner/repo/pull/149#discussion_r9002"
            }
        ]),
    );
    seed(
        "/repos/owner/repo/pulls/149/reviews?per_page=100&page=1",
        json!([
            {
                "id": 9003,
                "user": {"login": "bob"},
                "state": "CHANGES_REQUESTED",
                "body": "Please address inline note",
                "html_url": "https://github.com/owner/repo/pull/149#pullrequestreview-9003"
            }
        ]),
    );

    app.call("github.pr.refresh", json!({"card_id": card.id}))
        .unwrap();

    let snap = app.snapshot().unwrap();
    let updated_card = snap.cards.iter().find(|c| c.id == card.id).unwrap();
    assert!(
        updated_card
            .attention
            .as_deref()
            .is_some_and(|a| a.starts_with("Changes requested on PR #149"))
    );

    let updated_ev = snap
        .events
        .iter()
        .find(|e| e.card_id == card.id && e.kind == "github.pr.updated")
        .unwrap();
    assert_eq!(
        updated_ev.data["commits"][0].as_str(),
        Some("2222222 · Fix edge case")
    );

    let issue_cmt = snap
        .events
        .iter()
        .find(|e| {
            e.card_id == card.id
                && e.kind == "github.pr.commented"
                && e.data["kind"] == "issue_comment"
        })
        .unwrap();
    assert_eq!(
        issue_cmt.data["projected_body"].as_str(),
        Some("Remote PR discussion body should stay ephemeral")
    );

    let inline_cmt = snap
        .events
        .iter()
        .find(|e| {
            e.card_id == card.id
                && e.kind == "github.pr.commented"
                && e.data["kind"] == "review_comment"
        })
        .unwrap();
    assert_eq!(
        inline_cmt.data["projected_body"].as_str(),
        Some("Inline review comment body")
    );
    assert_eq!(inline_cmt.data["line"].as_u64(), Some(42));

    let review_ev = snap
        .events
        .iter()
        .find(|e| e.card_id == card.id && e.kind == "github.pr.reviewed")
        .unwrap();
    assert_eq!(review_ev.data["state"].as_str(), Some("CHANGES_REQUESTED"));
    assert_eq!(
        review_ev.data["projected_body"].as_str(),
        Some("Please address inline note")
    );

    // Durable SQLite `events` JSON must NOT mirror remote comment/review prose bodies.
    let raw_events: Vec<String> = {
        let store = app.store.lock().unwrap();
        let mut stmt = store
            .db()
            .conn
            .prepare("SELECT data FROM events WHERE card_id=?1")
            .unwrap();
        stmt.query_map([&card.id], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    for raw in raw_events {
        assert!(!raw.contains("Remote PR discussion body should stay ephemeral"));
        assert!(!raw.contains("Inline review comment body"));
        assert!(!raw.contains("Please address inline note"));
        assert!(!raw.contains("projected_body"));
    }
}
