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
    let reads = Arc::new(AtomicUsize::new(0));
    let creates = Arc::new(AtomicUsize::new(0));
    let comments = Arc::new(AtomicUsize::new(0));
    let card = json!({"id":"c1","project_id":"p1","number":1,"title":"First","body":"","stage":"backlog","revision":1,"created_at":1});
    let server = std::thread::spawn({
        let (stopped, entered, release, reads, creates, comments) = (
            stopped.clone(),
            refresh_entered.clone(),
            release_refresh.clone(),
            reads.clone(),
            creates.clone(),
            comments.clone(),
        );
        move || {
            while !stopped.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                let (stopped, entered, release, reads, creates, comments, card) = (
                    stopped.clone(),
                    entered.clone(),
                    release.clone(),
                    reads.clone(),
                    creates.clone(),
                    comments.clone(),
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
                            "card.create" => {
                                creates.fetch_add(1, Ordering::SeqCst);
                                card.clone()
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
    assert!(crate::APP.set(client).is_ok());

    #[component]
    fn ProbeRoot() -> impl IntoElement {
        let ui = Ui::new();
        ui.project.set("p1".into());
        let origin = ui.location();
        ui.history.update(|h| h.visit(origin.clone()));
        ui.page.set("new-card".into());
        ui.history.update(|h| h.visit(ui.location()));
        dispatch_complete(
            ui,
            "card.create",
            json!({"project_id":"p1","title":"First"}),
            move |result| {
                assert!(result.is_ok());
                assert!(!ui.pending.get_untracked());
                assert_eq!(ui.snapshot.with_untracked(|s| s.cards.len()), 1);
                assert_eq!(ui.page.get_untracked(), "card");
                assert_eq!(ui.card.get_untracked(), "c1");
                assert_eq!(ui.history.with(|h| h.entries.len()), 2);
                assert_eq!(ui.history.with(|h| h.entries[0].clone()), origin);
                assert_eq!(ui.history.with(|h| h.entries[1].clone()), ui.location());
                // A second write must be admitted while the previous refresh is stalled.
                dispatch_complete(
                    ui,
                    "comment.add",
                    json!({"card_id":"c1","body":"Sent","mentions":[]}),
                    move |result| {
                        assert!(result.is_ok());
                        assert!(!ui.pending.get_untracked());
                        assert_eq!(ui.snapshot.with_untracked(|s| s.events.len()), 1);
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
        && !(DONE.load(Ordering::SeqCst) && refresh_entered.load(Ordering::SeqCst))
    {
        core.run_component_tasks();
        core.commit();
        std::thread::sleep(Duration::from_millis(2));
    }
    let finished_before_refresh = DONE.load(Ordering::SeqCst);
    release_refresh.store(true, Ordering::SeqCst);
    // Drive both the failed refresh and the coalesced follow-up through the UI.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && reads.load(Ordering::SeqCst) < 3 {
        core.run_component_tasks();
        core.commit();
        std::thread::sleep(Duration::from_millis(2));
    }
    stopped.store(true, Ordering::SeqCst);
    server.join().unwrap();
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
