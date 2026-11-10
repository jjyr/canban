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
        client.seed_response(&path(page),&canban_github::cache::Response{data:json!({"items":[{"number":page,"title":"REMOTE_BODY_SENTINEL","html_url":format!("https://github.com/owner/repo/issues/{page}")}]}),link:(page==1).then(||"<https://api.github.com/x?page=2>; rel=\"next\"".into()),etag:None,modified:None,poll_interval:None,validated_at:1,cached:false});
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
    assert!(app.snapshot().unwrap().requests.is_empty());
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
        None,
        json!({}),
    );
    let remote = canban_timeline::event(
        &p.id,
        &c.id,
        "github.comment",
        "github:someone",
        "REMOTE_HISTORY",
        None,
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
