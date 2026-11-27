use super::*;

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
            trust_tailscale_self: true,
        }))
        .unwrap();

    let client = AppClient::connect_home(home.clone(), Path::new("/bin/echo")).unwrap();
    let (mut push_rx, ready_rx) = client.subscribe_topics_ready(&["update"]);
    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("app.subscribe handshake should complete");

    let _op_lock = canban_updater::lock(&home, "operation.lock").unwrap();
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
        app.now_ms(),
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
        app.now_ms(),
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

#[test]
fn stale_downloading_update_recovers_on_open_update_state_and_step() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join("updates/staging/leftover")).unwrap();
    std::fs::write(
        home.join("updates/staging/leftover/download.tar.zst"),
        b"partial",
    )
    .unwrap();
    let avail = canban_updater::feed::Available {
        manifest: canban_updater::feed::Manifest {
            version: "20990101-1".into(),
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
            platform: canban_updater::Build::current().platform,
            api: canban_updater::DESKTOP_API,
            schema: canban_storage::migrations::CURRENT,
            rollback_floor: 8,
            asset: format!(
                "Canban-{}.tar.zst",
                canban_updater::Build::current().platform
            ),
            size: 1024,
            sha256: "a".repeat(64),
        },
        url: "https://example.invalid/update.tar.zst".into(),
        notes: String::new(),
    };
    canban_updater::save_state(
        &home,
        &canban_updater::State {
            phase: "downloading".into(),
            available: Some(avail.clone()),
            progress: 16,
            error: None,
            checked_at: 100,
            last_check_started_at: 100,
        },
    )
    .unwrap();

    // 1. AppServer::open recovers stale "downloading" state immediately.
    let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
    let after_open = canban_updater::load_state(&home);
    assert_eq!(after_open.phase, "error");
    assert_eq!(after_open.progress, 0);
    assert_eq!(after_open.available, Some(avail.clone()));
    assert!(!home.join("updates/staging/leftover").exists());

    // 2. `update.state` recovers stale "downloading" state on demand.
    canban_updater::save_state(
        &home,
        &canban_updater::State {
            phase: "downloading".into(),
            available: Some(avail.clone()),
            progress: 55,
            ..Default::default()
        },
    )
    .unwrap();
    let rpc_state = app.call("update.state", json!({})).unwrap();
    assert_eq!(rpc_state["state"]["phase"], "error");
    assert_eq!(rpc_state["state"]["progress"], 0);
    assert_eq!(
        rpc_state["state"]["available"]["manifest"]["version"],
        "20990101-1"
    );

    // 3. `update_check_step` recovers stale "downloading" even when `allow_auto_check == false`
    //    and `updates_disabled == true`, emits a single UpdateChanged, and returns 1s sleep (no 500ms spin).
    let mut push_rx = app.subscribe_push();
    canban_updater::save_state(
        &home,
        &canban_updater::State {
            phase: "downloading".into(),
            available: Some(avail),
            progress: 77,
            ..Default::default()
        },
    )
    .unwrap();
    let (next_payload, _, sleep_dur) = crate::updates::update_check_step(
        &Arc::downgrade(&app),
        &home,
        &app.github,
        true,
        false,
        None,
        app.now_ms(),
    )
    .expect("step must recover stale downloading state");
    assert_eq!(next_payload, None);
    assert_eq!(sleep_dur, Duration::from_secs(1));
    assert_eq!(canban_updater::load_state(&home).phase, "error");
    match push_rx.try_recv().expect("expected UpdateChanged push") {
        crate::state::PushEvent::UpdateChanged { state, .. } => {
            assert_eq!(state["state"]["phase"], "error");
            assert_eq!(state["state"]["progress"], 0);
        }
        other => panic!("unexpected PushEvent: {other:?}"),
    }
    assert!(
        push_rx.try_recv().is_err(),
        "step must emit only one UpdateChanged event"
    );
}
