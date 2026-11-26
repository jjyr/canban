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
    let evs = migrated.events().unwrap();
    assert!(evs.iter().any(|e| e.message == "LOCAL_HISTORY"));
    assert!(!evs.iter().any(|e| e.message == "REMOTE_HISTORY"));
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
        .call("project.detect_github", json!({ "directory": source }))
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

    // Meanwhile, concurrent GitHub remote actions serialize via `github_action_gate`
    // with bounded waiting rather than failing immediately.
    let gh_guard = app
        .lock_github_action_gate(std::time::Duration::from_millis(50))
        .unwrap();
    let busy_err = app
        .lock_github_action_gate(std::time::Duration::from_millis(20))
        .err()
        .unwrap()
        .to_string();
    assert!(
        busy_err.contains("Another GitHub action is pending"),
        "{busy_err}"
    );
    drop(gh_guard);
    // Once released, a queued caller acquires immediately.
    let _gh_guard2 = app
        .lock_github_action_gate(std::time::Duration::from_millis(50))
        .unwrap();
}

#[test]
fn host_terminal_env_mode_save_and_refresh_rpc() {
    let dir = tempfile::tempdir().unwrap();
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("canban")).unwrap();
    assert_eq!(app.config().terminal_env_mode, HostTerminalEnvMode::Inherit);

    let saved: Config = serde_json::from_value(
        app.call(
            "config.agents.save",
            json!({"op": "set_terminal_env_mode", "terminal_env_mode": "minimal"}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(saved.terminal_env_mode, HostTerminalEnvMode::Minimal);
    assert_eq!(app.config().terminal_env_mode, HostTerminalEnvMode::Minimal);

    let restored: Config = serde_json::from_value(
        app.call(
            "config.agents.save",
            json!({"op": "set_terminal_env_mode", "terminal_env_mode": "inherit"}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(restored.terminal_env_mode, HostTerminalEnvMode::Inherit);

    let refreshed = app.call("host.env.refresh", json!({})).unwrap();
    assert_eq!(refreshed["ok"], true);
    assert!(refreshed["shell"].is_string());
    assert!(refreshed["var_count"].is_u64());
    assert!(refreshed["full_env_captured"].is_boolean());
}

#[test]
fn github_observation_canonicalizes_labels_and_head_across_api_and_compact_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let app = AppServer::open(dir.path().join("home"), PathBuf::from("canban")).unwrap();

    // 1. Observe a PR using raw GitHub REST API JSON (rich label objects + nested head.repo).
    let rest_pr = json!({
        "id": 9001,
        "number": 1754,
        "title": "Bump crate",
        "body": null,
        "state": "open",
        "draft": false,
        "merged": false,
        "html_url": "https://github.com/acme/widgets/pull/1754",
        "updated_at": "2026-11-16T01:55:27Z",
        "user": {"id": 49699333, "login": "dependabot[bot]"},
        "pull_request": {"url": "https://api.github.com/repos/acme/widgets/pulls/1754"},
        "labels": [
            {"id": 2, "name": "rust", "color": "ededed"},
            {"id": 1, "name": "dependencies", "color": "0366d6"}
        ],
        "assignees": [{"id": 7, "login": "Alice"}],
        "requested_reviewers": [{"id": 8, "login": "Bob"}],
        "head": {
            "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "ref": "dependabot/cargo/foo",
            "repo": {"pushed_at": "2026-11-16T01:00:00Z", "open_issues_count": 10}
        }
    });
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            crate::events::github::observe_batch(
                db,
                "acme/widgets",
                &[rest_pr],
                Some("pr"),
                app.now_ms(),
            )
        })
        .unwrap();

    // 2. Re-observe the exact same PR via compact models::Issue shape (only `{name}`, different order,
    //    `body: ""`) AND via a second REST fetch with mutated volatile `head.repo` counters.
    let compact_issue = json!({
        "id": 9001,
        "number": 1754,
        "title": "Bump crate",
        "body": "",
        "state": "open",
        "html_url": "https://github.com/acme/widgets/pull/1754",
        "updated_at": "2026-11-16T01:55:27Z",
        "user": {"id": 49699333, "login": "dependabot[bot]"},
        "pull_request": {},
        "labels": [{"name": "dependencies"}, {"name": "rust"}],
        "assignees": ["alice"],
        "requested_reviewers": ["bob"],
        "head": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    });
    let rest_pr_volatile_repo = json!({
        "id": 9001,
        "number": 1754,
        "title": "Bump crate",
        "body": null,
        "state": "open",
        "draft": false,
        "merged": false,
        "html_url": "https://github.com/acme/widgets/pull/1754",
        "updated_at": "2026-11-16T01:55:27Z",
        "user": {"id": 49699333, "login": "dependabot[bot]"},
        "pull_request": {"url": "https://api.github.com/repos/acme/widgets/pulls/1754"},
        "labels": [{"id": 1, "name": "dependencies", "color": "0366d6"}, {"id": 2, "name": "rust", "color": "ededed"}],
        "assignees": [{"id": 7, "login": "alice"}],
        "requested_reviewers": [{"id": 8, "login": "bob"}],
        "head": {
            "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "ref": "dependabot/cargo/foo",
            "repo": {"pushed_at": "2026-11-19T21:00:00Z", "open_issues_count": 99}
        }
    });
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            crate::events::github::observe_batch(
                db,
                "acme/widgets",
                &[compact_issue],
                Some("pr"),
                app.now_ms(),
            )?;
            crate::events::github::observe_batch(
                db,
                "acme/widgets",
                &[rest_pr_volatile_repo],
                Some("pr"),
                app.now_ms(),
            )
        })
        .unwrap();

    let evs = app.events().unwrap();
    let spurious: Vec<_> = evs
        .iter()
        .filter(|e| e.kind != "github.pr.discovered")
        .map(|e| e.kind.as_str())
        .collect();
    assert!(
        spurious.is_empty(),
        "alternating compact/REST shapes must not emit spurious events: {spurious:?}"
    );

    // 3. Real label and head SHA changes must still emit labels_changed and head_changed.
    let changed_pr = json!({
        "id": 9001,
        "number": 1754,
        "title": "Bump crate",
        "body": null,
        "state": "open",
        "draft": false,
        "merged": false,
        "html_url": "https://github.com/acme/widgets/pull/1754",
        "updated_at": "2026-11-16T02:00:00Z",
        "user": {"id": 49699333, "login": "dependabot[bot]"},
        "pull_request": {"url": "https://api.github.com/repos/acme/widgets/pulls/1754"},
        "labels": [{"id": 1, "name": "dependencies"}, {"id": 3, "name": "automerge"}],
        "head": {"sha": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
    });
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            crate::events::github::observe_batch(
                db,
                "acme/widgets",
                &[changed_pr],
                Some("pr"),
                app.now_ms(),
            )
        })
        .unwrap();

    let evs = app.events().unwrap();
    let kinds: Vec<_> = evs.iter().map(|e| e.kind.as_str()).collect();
    assert!(kinds.contains(&"github.pr.labels_changed"), "{kinds:?}");
    assert!(kinds.contains(&"github.pr.head_changed"), "{kinds:?}");
}
