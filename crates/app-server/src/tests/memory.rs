use super::*;

#[test]
fn unchanged_state_and_cursor_checks_do_not_deserialize_history() {
    let (_dir, app, _project, _card) = setup();
    let initial = app.call("app.state", json!({})).unwrap();
    let unchanged = app
        .call("app.state", json!({"after":initial["revision"]}))
        .unwrap();
    assert_eq!(unchanged["changed"], false);
    assert!(unchanged.get("snapshot").is_none());
    assert!(app.call("app.watch", json!({})).is_err());

    // If state_cursor or unchanged app.state reads event bodies, invalid history fails parsing.
    app.store
        .lock()
        .unwrap()
        .db()
        .conn
        .execute("UPDATE events SET data='invalid-json'", [])
        .unwrap();
    let cursor = app.state_cursor();
    assert_ne!(cursor, initial["revision"]);
    let unchanged = app.call("app.state", json!({"after":cursor})).unwrap();
    assert_eq!(unchanged["changed"], false);
    assert!(unchanged.get("snapshot").is_none());
}

#[test]
fn state_cursor_covers_worker_writes_config_freshness_and_server_restart() {
    let (dir, app, _project, card) = setup();
    let mut previous = app.call("app.state", json!({})).unwrap();
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
    let cursor_changed = app
        .call("app.state", json!({"after":previous["revision"]}))
        .unwrap();
    assert_eq!(cursor_changed["changed"], true);
    app.emit("github", None);
    let github = app
        .call("app.state", json!({"after":cursor_changed["revision"]}))
        .unwrap();
    assert_ne!(github["github_revision"], cursor_changed["github_revision"]);
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
fn push_subscription_observes_worker_commits_without_emit_side_channels_and_heartbeats() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    app.config.write().unwrap().hc = "/usr/bin/false".into();
    let p: Project = serde_json::from_value(
        app.call("project.create", json!({"name":"P","directory":"/tmp"}))
            .unwrap(),
    )
    .unwrap();
    let card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"project_id":p.id,"title":"T","view":"full"}),
        )
        .unwrap(),
    )
    .unwrap();
    let _running = app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let addr = app.bound_tcp_addrs().into_iter().next().unwrap();
    let (_, token) = app.issue_device_token("memory-test-1").unwrap();

    // 1. Handshake and push over TCP with all side channels + fast heartbeat.
    let mut tcp = std::net::TcpStream::connect(&addr).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    writeln!(
        tcp,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "app.subscribe",
            "auth": token,
            "params": {
                "topics": ["snapshot", "update", "utilities", "usage", "herdr"],
                "heartbeat_ms": 60,
            }
        })
    )
    .unwrap();
    tcp.flush().unwrap();
    let mut tcp_reader = BufReader::new(tcp.try_clone().unwrap());
    let mut line = String::new();
    tcp_reader.read_line(&mut line).unwrap();
    let handshake: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(handshake["id"], 1);
    assert_eq!(handshake["result"]["subscribed"], true);
    assert_eq!(handshake["result"]["changed"], true);
    assert_eq!(handshake["result"]["api_version"], API_VERSION);
    let initial_rev = handshake["result"]["revision"].clone();

    let next_matching =
        |reader: &mut BufReader<std::net::TcpStream>, pred: &dyn Fn(&Value) -> bool| -> Value {
            for _ in 0..24 {
                let mut buf = String::new();
                let n = reader.read_line(&mut buf).unwrap();
                assert!(n > 0, "unexpected EOF waiting for push frame");
                let v: Value = serde_json::from_str(&buf).unwrap();
                if v["method"] != "heartbeat" && pred(&v) {
                    return v;
                }
            }
            panic!("did not observe expected push frame within bound");
        };

    // Direct Store::write (two writes in one transaction) without emit pushes a single coalesced event.
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
    let ev = next_matching(&mut tcp_reader, &|v| {
        v["method"] == "event" && v["params"]["topic"] == "application"
    });
    assert_ne!(ev["params"]["revision"], initial_rev);

    // Explicit emit preserves topic and card_id.
    app.emit("review", Some(card.id.clone()));
    let ev = next_matching(&mut tcp_reader, &|v| {
        v["method"] == "event" && v["params"]["topic"] == "review"
    });
    assert_eq!(ev["params"]["card_id"], card.id);

    // Utilities configure pushes both desktop snapshot invalidation and utilities.changed payload.
    app.call("utilities.configure", json!({"theme":"nord"}))
        .unwrap();
    let mut saw_desktop = false;
    let mut saw_utilities = false;
    for _ in 0..16 {
        let msg = next_matching(&mut tcp_reader, &|v| {
            (v["method"] == "event" && v["params"]["topic"] == "desktop")
                || v["method"] == "utilities.changed"
        });
        if msg["method"] == "event" && msg["params"]["topic"] == "desktop" {
            saw_desktop = true;
        } else if msg["method"] == "utilities.changed"
            && msg["params"]["state"]["preferences"]["theme"] == "nord"
        {
            saw_utilities = true;
        }
        if saw_desktop && saw_utilities {
            break;
        }
    }
    assert!(saw_desktop && saw_utilities);

    // Side-channel emitters push update.changed and herdr.install directly.
    app.emit_update();
    let upd = next_matching(&mut tcp_reader, &|v| v["method"] == "update.changed");
    assert!(upd["params"]["state"]["build"].is_object());

    app.emit_herdr_install();
    let inst = next_matching(&mut tcp_reader, &|v| v["method"] == "herdr.install");
    assert_eq!(inst["params"]["state"]["phase"], "idle");

    // Idle heartbeat arrives within expected cadence.
    let mut saw_hb = false;
    for _ in 0..10 {
        let mut buf = String::new();
        tcp_reader.read_line(&mut buf).unwrap();
        let msg: Value = serde_json::from_str(&buf).unwrap();
        if msg["method"] == "heartbeat" {
            assert_eq!(msg["params"]["server_id"], app.server_id());
            saw_hb = true;
            break;
        }
    }
    assert!(saw_hb, "expected periodic heartbeat frame");

    // Closing subscriber cleans up TCP connection count cleanly.
    drop(tcp_reader);
    drop(tcp);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && app.active_tcp_count() > 0 {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.active_tcp_count(), 0);
}

#[test]
fn push_subscription_handles_auth_second_frame_lagged_resync_and_client_subscribe() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    app.config.write().unwrap().hc = "/usr/bin/false".into();
    let p: Project = serde_json::from_value(
        app.call("project.create", json!({"name":"P","directory":"/tmp"}))
            .unwrap(),
    )
    .unwrap();
    let running = app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();
    let addr = app.bound_tcp_addrs().into_iter().next().unwrap();
    let (_, token) = app.issue_device_token("memory-test-1").unwrap();

    // 1. Unauthenticated TCP subscribe is rejected with -32001.
    let mut bad = std::net::TcpStream::connect(&addr).unwrap();
    bad.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    writeln!(
        bad,
        "{}",
        json!({"jsonrpc":"2.0","id":1,"method":"app.subscribe","auth":"wrong-token","params":{}})
    )
    .unwrap();
    bad.flush().unwrap();
    let mut bad_line = String::new();
    BufReader::new(bad).read_line(&mut bad_line).unwrap();
    let bad_reply: Value = serde_json::from_str(&bad_line).unwrap();
    assert_eq!(bad_reply["error"]["code"], -32001);

    // 2. Sending app.subscribe as second frame after app.ping fails cleanly.
    let mut reuse = UnixStream::connect(app.endpoint()).unwrap();
    reuse
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(
        reuse,
        "{}",
        json!({"jsonrpc":"2.0","id":1,"method":"app.ping","params":{}})
    )
    .unwrap();
    writeln!(
        reuse,
        "{}",
        json!({"jsonrpc":"2.0","id":2,"method":"app.subscribe","params":{}})
    )
    .unwrap();
    reuse.flush().unwrap();
    let mut r_reader = BufReader::new(reuse);
    let mut r1 = String::new();
    let mut r2 = String::new();
    r_reader.read_line(&mut r1).unwrap();
    r_reader.read_line(&mut r2).unwrap();
    let reply1: Value = serde_json::from_str(&r1).unwrap();
    let reply2: Value = serde_json::from_str(&r2).unwrap();
    assert!(reply1.get("result").is_some());
    assert!(
        reply2["error"]["message"]
            .as_str()
            .unwrap()
            .contains("first frame")
    );

    // 3. Overflowing broadcast capacity surfaces resync.
    let mut lag_tcp = std::net::TcpStream::connect(&addr).unwrap();
    lag_tcp
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(
        lag_tcp,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "app.subscribe",
            "auth": token,
            "params": { "topics": ["snapshot"], "heartbeat_ms": 60_000 }
        })
    )
    .unwrap();
    lag_tcp.flush().unwrap();
    let mut lag_reader = BufReader::new(lag_tcp.try_clone().unwrap());
    let mut hs = String::new();
    lag_reader.read_line(&mut hs).unwrap();
    for _ in 0..320 {
        app.emit("application", None);
    }
    let mut lag_line = String::new();
    lag_reader.read_line(&mut lag_line).unwrap();
    let lag_msg: Value = serde_json::from_str(&lag_line).unwrap();
    assert_eq!(lag_msg["method"], "event");
    assert_eq!(lag_msg["params"]["topic"], "resync");
    assert!(lag_msg["params"]["lagged"].as_u64().unwrap() > 0);
    drop(lag_reader);
    drop(lag_tcp);

    // 4. AppClient::subscribe receives live push events over Unix socket and observes server shutdown cleanly.
    let client = AppClient::connect_home(home, Path::new("/bin/echo")).unwrap();
    let rx = client.subscribe();
    let first_ev = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(matches!(
        first_ev,
        AppEvent::Changed {
            topic: "resync",
            ..
        }
    ));
    let _ = client
        .call("card.create", json!({"project_id":p.id,"title":"Pushed"}))
        .unwrap();
    let pushed = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(matches!(pushed, AppEvent::Changed { .. }));
    drop(client);
    drop(running);
}

#[test]
fn timeline_pages_ignore_other_card_bodies_and_preserve_sequence_order() {
    let (_dir, app, project, card) = setup();
    let other: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view":"full",
                "project_id":project.id,"title":"Unrelated history"
            }),
        )
        .unwrap(),
    )
    .unwrap();
    for n in 0..5 {
        app.call(
            "comment.add",
            json!({"view":"full","card_id":card.id,"body":format!("message {n}")}),
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
                json!({"view":"full","card_id":card.id,"after_sequence":after,"limit":2}),
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
    let before = app.call("app.state", json!({})).unwrap();
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

#[test]
fn active_update_checks_push_update_changed_progress_without_ui_polling() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    app.config.write().unwrap().hc = "/usr/bin/false".into();
    let _running = app
        .start_with_listen(Some(ServerListenConfig {
            tailscale: false,
            port: 0,
            listen: Some("127.0.0.1:0".into()),
            allow_insecure_bind: false,
        }))
        .unwrap();

    let client = AppClient::connect_home(home.clone(), Path::new("/bin/echo")).unwrap();
    let mut push_rx = client.subscribe_topics(&["update"]);
    // Give subscribe stream a brief moment to complete handshake before writing updater state.
    std::thread::sleep(Duration::from_millis(60));

    let mut state = canban_updater::load_state(&home);
    state.phase = "downloading".into();
    state.progress = 35;
    canban_updater::save_state(&home, &state).unwrap();

    let first = push_rx
        .blocking_recv()
        .expect("expected initial update.changed push");
    match first {
        ClientPushEvent::UpdateChanged(v) => {
            assert_eq!(v["state"]["phase"], "downloading");
            assert_eq!(v["state"]["progress"], 35);
        }
        other => panic!("unexpected ClientPushEvent: {other:?}"),
    }

    state.progress = 85;
    canban_updater::save_state(&home, &state).unwrap();
    let second = push_rx
        .blocking_recv()
        .expect("expected progress update.changed push");
    match second {
        ClientPushEvent::UpdateChanged(v) => {
            assert_eq!(v["state"]["progress"], 85);
        }
        other => panic!("unexpected ClientPushEvent: {other:?}"),
    }
}

#[test]
fn interrupted_update_check_recovers_while_live_operation_keeps_pushing_progress() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let mut push_rx = app.subscribe_push();

    // 1. Stale interrupted "checking" state (no operation.lock held) transitions to "error" on step.
    let now_s = canban_updater::now();
    let mut state = canban_updater::load_state(&home);
    state.phase = "checking".into();
    state.checked_at = now_s;
    state.last_check_started_at = now_s;
    state.error = None;
    canban_updater::save_state(&home, &state).unwrap();

    let (next_payload, _, sleep_dur) = crate::updates::update_check_step(
        &Arc::downgrade(&app),
        &home,
        &app.github,
        false,
        true,
        None,
        std::time::Instant::now(),
    )
    .expect("step must return active state");
    assert_eq!(next_payload, None);
    assert_eq!(sleep_dur, Duration::from_secs(1));

    let recovered = canban_updater::load_state(&home);
    assert_eq!(recovered.phase, "error");
    assert!(
        recovered
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("interrupted"),
        "expected interrupted error message, got {:?}",
        recovered.error
    );
    match push_rx.try_recv().expect("expected UpdateChanged push") {
        crate::state::PushEvent::UpdateChanged { state, .. } => {
            assert_eq!(state["state"]["phase"], "error");
        }
        other => panic!("unexpected PushEvent: {other:?}"),
    }

    // 2. Live operation holding operation.lock keeps "downloading" phase and emits progress updates.
    let op_lock = canban_updater::lock(&home, "operation.lock").unwrap();
    let mut live_state = recovered;
    live_state.phase = "downloading".into();
    live_state.progress = 40;
    live_state.error = None;
    canban_updater::save_state(&home, &live_state).unwrap();

    let (next_payload, _, sleep_dur) = crate::updates::update_check_step(
        &Arc::downgrade(&app),
        &home,
        &app.github,
        false,
        true,
        None,
        std::time::Instant::now(),
    )
    .expect("step must continue while downloading");
    assert!(next_payload.is_some());
    assert_eq!(sleep_dur, Duration::from_millis(500));

    let untouched = canban_updater::load_state(&home);
    assert_eq!(untouched.phase, "downloading");
    assert_eq!(untouched.progress, 40);
    match push_rx
        .try_recv()
        .expect("expected live downloading UpdateChanged push")
    {
        crate::state::PushEvent::UpdateChanged { state, .. } => {
            assert_eq!(state["state"]["phase"], "downloading");
            assert_eq!(state["state"]["progress"], 40);
        }
        other => panic!("unexpected PushEvent: {other:?}"),
    }
    drop(op_lock);
}
