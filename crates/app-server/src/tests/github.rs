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
        let ok = std::process::Command::new("git")
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
            std::process::Command::new("git")
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

    // Holding `review_gate` (as `pump_deliveries` or `git.commit` does) must never reject
    // `github.remote.*` or `github.notification.*` with "Another GitHub action is pending".
    let _delivery_guard = app.review_gate.lock().unwrap();
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
            crate::sessions::queue_interactive_mentions(
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
    assert_eq!(runs[0].trigger, ExecutionTrigger::Interactive);
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
