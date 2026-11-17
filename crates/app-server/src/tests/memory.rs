use super::*;

#[test]
fn unchanged_state_and_watch_do_not_deserialize_history() {
    let (_dir, app, _project, _card) = setup();
    let initial = app.call("app.state", json!({})).unwrap();
    let unchanged = app
        .call("app.state", json!({"after":initial["revision"]}))
        .unwrap();
    assert_eq!(unchanged["changed"], false);
    assert!(unchanged.get("snapshot").is_none());
    // If either cheap path reads event bodies, this deliberately invalid history fails parsing.
    app.store
        .lock()
        .unwrap()
        .db()
        .conn
        .execute("UPDATE events SET data='invalid-json'", [])
        .unwrap();
    let watch = app
        .call("app.watch", json!({"after":initial["revision"]}))
        .unwrap();
    assert_eq!(watch["changed"], true);
    assert!(watch.get("snapshot").is_none());
    let unchanged = app
        .call("app.state", json!({"after":watch["revision"],"wait_ms":1}))
        .unwrap();
    assert_eq!(unchanged["changed"], false);
    assert!(unchanged.get("snapshot").is_none());
}

#[test]
fn state_cursor_covers_worker_writes_config_freshness_and_server_restart() {
    let (dir, app, _project, card) = setup();
    let mut previous = app.call("app.watch", json!({})).unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut card: Card = db.get(&card.id)?;
            card.body = "internal worker committed".into();
            db.save(&card)
        })
        .unwrap();
    let next = app
        .call("app.state", json!({"after":previous["revision"]}))
        .unwrap();
    assert_eq!(next["changed"], true);
    assert_eq!(
        next["snapshot"]["cards"][0]["body"],
        "internal worker committed"
    );
    previous = next;
    app.config.write().unwrap().scheduler.max_agents += 1;
    app.emit("configuration", None);
    let next = app
        .call("app.state", json!({"after":previous["revision"]}))
        .unwrap();
    assert_ne!(next["revision"], previous["revision"]);
    assert_eq!(
        next["config"]["scheduler"]["max_agents"],
        app.config().scheduler.max_agents
    );
    previous = next;
    app.inventory.lock().unwrap().last_at = now();
    let next = app
        .call("app.watch", json!({"after":previous["revision"]}))
        .unwrap();
    assert_eq!(next["changed"], true);
    app.emit("github", None);
    let github = app
        .call("app.watch", json!({"after":next["revision"]}))
        .unwrap();
    assert_ne!(github["github_revision"], next["github_revision"]);
    drop(app);
    let reopened = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
    let restarted = reopened
        .call("app.state", json!({"after":github["revision"]}))
        .unwrap();
    assert_eq!(restarted["changed"], true);
    assert_eq!(restarted["server_id"], github["server_id"]);
    assert_ne!(restarted["revision"], github["revision"]);
}

#[test]
fn waiting_watch_observes_commits_without_emit_and_coalesces_updates() {
    let (_dir, app, _project, card) = setup();
    let cursor = app.call("app.watch", json!({})).unwrap()["revision"].clone();
    let watcher = app.clone();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let handle = std::thread::spawn(move || {
        ready_tx.send(()).unwrap();
        watcher
            .call("app.watch", json!({"after":cursor,"wait_ms":2000}))
            .unwrap()
    });
    ready_rx.recv().unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut c: Card = db.get(&card.id)?;
            c.body = "changed twice".into();
            db.save(&c)?;
            c.title = "latest title".into();
            db.save(&c)
        })
        .unwrap();
    let update = handle.join().unwrap();
    assert_eq!(update["changed"], true);
    let full = app.call("app.state", json!({})).unwrap();
    assert_eq!(full["snapshot"]["cards"][0]["title"], "latest title");
    assert_eq!(full["snapshot"]["cards"][0]["body"], "changed twice");
}

#[test]
fn timeline_pages_ignore_other_card_bodies_and_preserve_sequence_order() {
    let (_dir, app, project, card) = setup();
    let other: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({
                "project_id":project.id,"title":"Unrelated history"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    for n in 0..5 {
        app.call(
            "comment.add",
            json!({"card_id":card.id,"body":format!("message {n}")}),
        )
        .unwrap();
    }
    app.store
        .lock()
        .unwrap()
        .db()
        .conn
        .execute(
            "UPDATE events SET data='invalid-json' WHERE card_id=?1",
            [&other.id],
        )
        .unwrap();
    let mut after = 0;
    let mut seen = Vec::new();
    loop {
        let page = app
            .call(
                "timeline.list",
                json!({"card_id":card.id,"after_sequence":after,"limit":2}),
            )
            .unwrap();
        let events: Vec<Event> = serde_json::from_value(page["events"].clone()).unwrap();
        assert!(events.len() <= 2);
        for event in &events {
            assert_eq!(event.card_id, card.id);
            assert!(event.sequence > after);
            after = event.sequence;
            seen.push(event.id.clone());
        }
        if page["next_cursor"].is_null() {
            break;
        }
        assert_eq!(page["next_cursor"], after);
    }
    assert_eq!(seen.len(), 6); // creation plus five comments
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 6);
}

#[test]
fn transient_merge_marker_removal_invalidates_without_a_database_write() {
    let (_dir, app, _project, card) = setup();
    let guard = app.track_merging(&card.id);
    let before = app.call("app.watch", json!({})).unwrap();
    assert_eq!(app.snapshot().unwrap().merging_cards, vec![card.id.clone()]);
    drop(guard);
    let after = app
        .call("app.state", json!({"after":before["revision"]}))
        .unwrap();
    assert_eq!(after["changed"], true);
    assert!(
        after["snapshot"]["merging_cards"].is_null()
            || after["snapshot"]["merging_cards"]
                .as_array()
                .unwrap()
                .is_empty()
    );
}
