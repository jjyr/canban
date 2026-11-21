//! Exercise the real presenter and socket client against delayed/failing refreshes.
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

static DONE: AtomicBool = AtomicBool::new(false);
static CATALOG_REPLAYED: AtomicBool = AtomicBool::new(false);

#[test]
fn submissions_finish_before_refresh_and_refresh_failure_does_not_fail_the_write() {
    let dir = std::env::temp_dir().join(format!("cb-submit-{}", id()));
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("app.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let refresh_entered = Arc::new(AtomicBool::new(false));
    let release_refresh = Arc::new(AtomicBool::new(false));
    let catalog_entered = Arc::new(AtomicBool::new(false));
    let release_catalog = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let creates = Arc::new(AtomicUsize::new(0));
    let comments = Arc::new(AtomicUsize::new(0));
    let config_saves = Arc::new(AtomicUsize::new(0));
    let utility_saves = Arc::new(AtomicUsize::new(0));
    let card = json!({"id":"c1","project_id":"p1","number":1,"title":"First","body":"","stage":"backlog","revision":1,"created_at":1});
    let server = std::thread::spawn({
        let (
            stopped,
            entered,
            release,
            cat_entered,
            cat_release,
            reads,
            creates,
            comments,
            config_saves,
            utility_saves,
        ) = (
            stopped.clone(),
            refresh_entered.clone(),
            release_refresh.clone(),
            catalog_entered.clone(),
            release_catalog.clone(),
            reads.clone(),
            creates.clone(),
            comments.clone(),
            config_saves.clone(),
            utility_saves.clone(),
        );
        move || {
            while !stopped.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                let (
                    stopped,
                    entered,
                    release,
                    cat_entered,
                    cat_release,
                    reads,
                    creates,
                    comments,
                    config_saves,
                    utility_saves,
                    card,
                ) = (
                    stopped.clone(),
                    entered.clone(),
                    release.clone(),
                    cat_entered.clone(),
                    cat_release.clone(),
                    reads.clone(),
                    creates.clone(),
                    comments.clone(),
                    config_saves.clone(),
                    utility_saves.clone(),
                    card.clone(),
                );
                std::thread::spawn(move || {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_millis(100)))
                        .unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    while !stopped.load(Ordering::SeqCst) {
                        match reader.read_line(&mut line) {
                            Ok(0) => break,
                            Ok(_) => {}
                            Err(_) => continue,
                        }
                        let req: Value = serde_json::from_str(&line).unwrap();
                        line.clear();
                        let result = match req["method"].as_str().unwrap() {
                            "app.ping" => {
                                json!({"build":canban_updater::Build::current(),"api_version":canban_updater::DESKTOP_API})
                            }
                            "app.state" => {
                                if reads.fetch_add(1, Ordering::SeqCst) > 0 {
                                    entered.store(true, Ordering::SeqCst);
                                    while !release.load(Ordering::SeqCst)
                                        && !stopped.load(Ordering::SeqCst)
                                    {
                                        std::thread::sleep(Duration::from_millis(2));
                                    }
                                    let _ = writeln!(
                                        stream,
                                        "{}",
                                        json!({"jsonrpc":"2.0","id":req["id"],"error":{"code":-32000,"message":"Refresh unavailable"}})
                                    );
                                    continue;
                                }
                                json!({"home":"/tmp/fixture","config":Config::default(),"snapshot":Snapshot::default()})
                            }
                            "agent.catalog" => {
                                cat_entered.store(true, Ordering::SeqCst);
                                while !cat_release.load(Ordering::SeqCst)
                                    && !stopped.load(Ordering::SeqCst)
                                {
                                    std::thread::sleep(Duration::from_millis(2));
                                }
                                json!({
                                    "machine": "local",
                                    "agents": [{"id":"codex","enabled":true,"configured":true,"installed":true}],
                                    "raw_profiles": [
                                        {"id":"codex","enabled":true,"identity":false,"preset":"codex","model":"gpt-5.4"},
                                        {"id":"developer","enabled":true,"identity":true,"preset":"codex"}
                                    ]
                                })
                            }
                            "config.agents.save" => {
                                config_saves.fetch_add(1, Ordering::SeqCst);
                                let mut cfg = Config::default();
                                if let Some(dev) =
                                    cfg.profiles.iter_mut().find(|p| p.id == "developer")
                                {
                                    dev.model = Some("gpt-5.4-mini".into());
                                } else {
                                    cfg.profiles.push(Profile {
                                        id: "developer".into(),
                                        enabled: true,
                                        identity: true,
                                        preset: Some("codex".into()),
                                        model: Some("gpt-5.4-mini".into()),
                                        ..Default::default()
                                    });
                                }
                                serde_json::to_value(cfg).unwrap()
                            }
                            "utilities.configure" => {
                                utility_saves.fetch_add(1, Ordering::SeqCst);
                                json!({"awake":false,"preferences":{"theme":"dracula"}})
                            }
                            "card.create" => {
                                creates.fetch_add(1, Ordering::SeqCst);
                                card.clone()
                            }
                            "card.move" => {
                                let mut moved = card.clone();
                                moved["stage"] = req["params"]["stage"].clone();
                                moved["revision"] = json!(2);
                                moved
                            }
                            "card.hold_auto_merge" => {
                                let mut held = card.clone();
                                held["stage"] = json!("develop");
                                held["hold_auto_merge"] = req["params"]["hold"].clone();
                                held["revision"] = json!(3);
                                held
                            }
                            "comment.add" => {
                                comments.fetch_add(1, Ordering::SeqCst);
                                json!({"id":"ev1","project_id":"p1","card_id":"c1","sequence":1,"kind":"comment.created","actor":"user","message":"Sent","data":{"body":"Sent"},"created_at":1})
                            }
                            other => panic!("Unexpected RPC {other}"),
                        };
                        let _ = writeln!(
                            stream,
                            "{}",
                            json!({"jsonrpc":"2.0","id":req["id"],"result":result})
                        );
                    }
                });
            }
        }
    });
    let client = canban_app_server::AppClient::connect_ssh_socket(socket.clone()).unwrap();
    assert!(
        crate::backend::set(Arc::new(crate::backend::LocalBackend::from_client(client))).is_ok()
    );

    #[component]
    fn ProbeRoot() -> impl IntoElement {
        let ui = Ui::new();
        effect(move || {
            let replayed = ui.agent_catalog.with(|c| {
                c["raw_profiles"].as_array().is_some_and(|arr| {
                    arr.iter()
                        .any(|p| p["id"] == "developer" && p["model"] == "gpt-5.4-mini")
                })
            });
            if replayed {
                CATALOG_REPLAYED.store(true, Ordering::SeqCst);
            }
        });
        ui.project.set("p1".into());
        let origin = ui.location();
        ui.history.update(|h| h.visit(origin.clone()));
        ui.page.set("new-card".into());
        ui.history.update(|h| h.visit(ui.location()));

        // Start a slow `agent.catalog` load, then save a model change while it is in flight.
        crate::agents_setup::refresh_catalog(ui);
        crate::agents_setup::dispatch_agents_save(
            ui,
            json!({"op":"set_profile_options","id":"developer","model":"gpt-5.4-mini"}),
            move |cfg| {
                assert_eq!(
                    cfg.profiles
                        .iter()
                        .find(|p| p.id == "developer")
                        .and_then(|p| p.model.as_deref()),
                    Some("gpt-5.4-mini")
                );
            },
        );
        // Optimistic update took effect synchronously before RPC completed.
        assert_eq!(
            ui.all_profiles.with_untracked(|ps| ps
                .iter()
                .find(|p| p.id == "developer")
                .and_then(|p| p.model.clone())),
            Some("gpt-5.4-mini".into())
        );

        crate::utilities::configure_preferences(ui, json!({"theme":"dracula"}));
        assert_eq!(
            ui.utility
                .with_untracked(|v| v["preferences"]["theme"].clone()),
            json!("dracula")
        );

        dispatch_complete(
            ui,
            "card.create",
            json!({"project_id":"p1","title":"First"}),
            move |result| {
                assert!(result.is_ok());
                assert!(!ui.is_method_pending("card.create"));
                assert_eq!(ui.snapshot.with_untracked(|s| s.cards.len()), 1);
                assert_eq!(ui.page.get_untracked(), "card");
                assert_eq!(ui.card.get_untracked(), "c1");
                assert_eq!(ui.history.with(|h| h.entries.len()), 2);
                assert_eq!(ui.history.with(|h| h.entries[0].clone()), origin);
                assert_eq!(ui.history.with(|h| h.entries[1].clone()), ui.location());
                move_card(ui, "c1", "develop");
                assert_eq!(
                    ui.snapshot.with_untracked(|s| s.cards[0].stage.clone()),
                    "develop"
                );
                hold_auto_merge(ui, "c1", 2, true);
                assert!(ui.snapshot.with_untracked(|s| s.cards[0].hold_auto_merge));
                // A second write must be admitted while the previous refresh is stalled.
                dispatch_complete(
                    ui,
                    "comment.add",
                    json!({"card_id":"c1","body":"Sent","mentions":[]}),
                    move |result| {
                        assert!(result.is_ok());
                        assert!(!ui.is_method_pending("comment.add"));
                        assert_eq!(ui.card_timeline_events("c1").len(), 1);
                        // A late create receipt must not replace a different draft.
                        ui.page.set("new-card".into());
                        ui.new_draft.set("new-card:submitted".into());
                        dispatch_complete(
                            ui,
                            "card.create",
                            json!({"project_id":"p1","title":"Second"}),
                            move |result| {
                                assert!(result.is_ok());
                                assert_eq!(ui.page.get_untracked(), "new-card");
                                assert_eq!(ui.new_draft.get_untracked(), "new-card:other");
                                DONE.store(true, Ordering::SeqCst);
                            },
                        );
                        ui.new_draft.set("new-card:other".into());
                    },
                );
            },
        );
        view()
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_time()
        .build()
        .unwrap();
    let mut core = nsh_core::Core::new(
        nsh_core::ApproxTextEngine,
        nsh_core::geometry::size(px(400.), px(300.)),
    );
    core.set_task_runtime(runtime.handle().clone());
    core.mount_root(&nsh_core::element::Root::of::<ProbeRoot>());
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline
        && !(DONE.load(Ordering::SeqCst)
            && refresh_entered.load(Ordering::SeqCst)
            && catalog_entered.load(Ordering::SeqCst))
    {
        core.run_component_tasks();
        core.commit();
        std::thread::sleep(Duration::from_millis(2));
    }
    let finished_before_refresh = DONE.load(Ordering::SeqCst);
    // Neither `config.agents.save` nor `utilities.configure` nor `card.move` waited on `app.state`.
    assert_eq!(config_saves.load(Ordering::SeqCst), 1);
    assert_eq!(utility_saves.load(Ordering::SeqCst), 1);
    release_catalog.store(true, Ordering::SeqCst);
    release_refresh.store(true, Ordering::SeqCst);
    // Drive both the failed refresh and the coalesced follow-up through the UI.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline
        && (reads.load(Ordering::SeqCst) < 3 || !CATALOG_REPLAYED.load(Ordering::SeqCst))
    {
        core.run_component_tasks();
        core.commit();
        std::thread::sleep(Duration::from_millis(2));
    }
    stopped.store(true, Ordering::SeqCst);
    server.join().unwrap();
    assert!(
        CATALOG_REPLAYED.load(Ordering::SeqCst),
        "stale agent.catalog response must replay concurrent config.agents.save edits"
    );
    assert!(
        finished_before_refresh,
        "write callbacks waited for the blocked snapshot"
    );
    assert_eq!(creates.load(Ordering::SeqCst), 2);
    assert_eq!(comments.load(Ordering::SeqCst), 1);
    assert!(
        reads.load(Ordering::SeqCst) >= 3,
        "refresh requested during an in-flight read was lost"
    );
    drop(core);
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ui_projection_hydrates_real_app_server_card_and_comment_writes_for_overlay() {
    struct TempDir(std::path::PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    let dir = TempDir(std::env::temp_dir().join(format!("cb-ui-proj-{}", id())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let app = canban_app_server::AppServer::open(
        dir.0.join("home"),
        std::path::PathBuf::from("/bin/echo"),
    )
    .unwrap();
    let project: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "ProjectionTest",
                "directory": dir.0.join("repo"),
            }),
        )
        .unwrap(),
    )
    .unwrap();

    // Explicit `fields` must not collide with `view = "full"`.
    let explicit_fields =
        crate::with_ui_projection("comment.update", json!({"fields": ["body"], "body": "x"}));
    assert!(explicit_fields.get("view").is_none());

    let mut writes = submission::CommittedWrites::default();
    let call_projected = |method: &str, params: Value| -> Value {
        app.call(method, crate::with_ui_projection(method, params))
            .unwrap()
    };

    // 1. `card.create`
    let created = call_projected(
        "card.create",
        json!({
            "project_id": project.id,
            "title": "Initial title",
            "body": "Initial body",
        }),
    );
    let card_id = created["id"].as_str().unwrap().to_owned();
    let rev1 = created["revision"].as_u64().unwrap();
    writes.record("card.create", &created, "local");

    let mut stale = Snapshot::default();
    writes.reconcile(&mut stale);
    assert_eq!(stale.cards.len(), 1);
    assert_eq!(stale.cards[0].id, card_id);
    assert_eq!(stale.cards[0].stage, "backlog");
    assert_eq!(stale.cards[0].title, "Initial title");
    assert_eq!(stale.cards[0].body, "Initial body");

    // Capture a stale snapshot at rev1 before subsequent mutations.
    let stale_card_snap = Snapshot {
        projects: vec![project.clone()],
        cards: stale.cards.clone(),
        ..Default::default()
    };

    // 2. `card.move`
    let moved = call_projected(
        "card.move",
        json!({
            "card_id": card_id,
            "revision": rev1,
            "stage": "develop",
        }),
    );
    let rev2 = moved["revision"].as_u64().unwrap();
    assert!(rev2 > rev1);
    writes.record("card.move", &moved, "local");
    let mut overlayed = stale_card_snap.clone();
    writes.reconcile(&mut overlayed);
    assert_eq!(overlayed.cards[0].stage, "develop");
    assert_eq!(overlayed.cards[0].title, "Initial title");

    // 3. `card.hold_auto_merge`
    let held = call_projected(
        "card.hold_auto_merge",
        json!({
            "card_id": card_id,
            "revision": rev2,
            "hold": true,
        }),
    );
    let rev3 = held["revision"].as_u64().unwrap();
    assert!(rev3 > rev2);
    writes.record("card.hold_auto_merge", &held, "local");
    let mut overlayed = stale_card_snap.clone();
    writes.reconcile(&mut overlayed);
    assert_eq!(overlayed.cards[0].stage, "develop");
    assert!(overlayed.cards[0].hold_auto_merge);

    // 4. `card.update` (returns `title` and `body` for `drafts::save`)
    let updated_card = call_projected(
        "card.update",
        json!({
            "card_id": card_id,
            "revision": rev3,
            "title": "Renamed title",
            "body": "Edited markdown body",
        }),
    );
    assert_eq!(updated_card["title"], json!("Renamed title"));
    assert_eq!(updated_card["body"], json!("Edited markdown body"));

    // 5. `comment.add`
    let added_comment = call_projected(
        "comment.add",
        json!({
            "card_id": card_id,
            "body": "First comment",
            "mentions": [],
        }),
    );
    let comment_id = added_comment["id"].as_str().unwrap().to_owned();
    writes.record("comment.add", &added_comment, "local");
    let mut overlayed_events = Vec::new();
    writes.reconcile_card_events(&card_id, &overlayed_events);
    writes.overlay_card_events(&card_id, &mut overlayed_events);
    assert_eq!(overlayed_events.len(), 1);
    assert_eq!(overlayed_events[0].id, comment_id);
    assert_eq!(overlayed_events[0].message, "First comment");

    // 6. `comment.update` (without `fields: ["body"]`, hydrates both `val["body"]` and `CommittedWrites`)
    let stale_with_comment = overlayed_events.clone();
    let updated_comment = call_projected(
        "comment.update",
        json!({
            "card_id": card_id,
            "comment_id": comment_id,
            "body": "Updated comment body",
            "mentions": [],
        }),
    );
    assert_eq!(updated_comment["body"], json!("Updated comment body"));
    writes.record("comment.update", &updated_comment, "local");
    let mut overlayed_events = stale_with_comment;
    writes.reconcile_card_events(&card_id, &overlayed_events);
    writes.overlay_card_events(&card_id, &mut overlayed_events);
    assert_eq!(overlayed_events.len(), 1);
    assert_eq!(overlayed_events[0].message, "Updated comment body");
    assert_eq!(overlayed_events[0].data["revision"], json!(2));
}
