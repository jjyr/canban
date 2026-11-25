use super::*;
use canban_github::{HttpRequest, HttpResponse, HttpTransport};
use canban_herdr::{BackendSubscription, HerdrBackend};
use canban_types::{
    Clock, DeterministicIdGen, FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE,
    FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB, FP_HERDR_AFTER_SEND_BEFORE_DB,
    FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB, FailpointAction, Failpoints,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

struct FixedClock(AtomicU64);

impl FixedClock {
    fn new(ms: u64) -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(ms)))
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct MockHerdr {
    core_id: String,
    next_id: AtomicUsize,
    terminals: Mutex<BTreeMap<String, Value>>,
    run_calls: AtomicUsize,
    send_calls: AtomicUsize,
    kill_calls: Mutex<Vec<String>>,
    fail_kill: AtomicBool,
    fail_send: AtomicBool,
}

impl MockHerdr {
    fn new(core_id: &str) -> Arc<Self> {
        Arc::new(Self {
            core_id: core_id.to_owned(),
            next_id: AtomicUsize::new(1),
            terminals: Mutex::new(BTreeMap::new()),
            run_calls: AtomicUsize::new(0),
            send_calls: AtomicUsize::new(0),
            kill_calls: Mutex::new(Vec::new()),
            fail_kill: AtomicBool::new(false),
            fail_send: AtomicBool::new(false),
        })
    }

    fn live_addresses(&self) -> Vec<String> {
        self.terminals
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, v)| v["state"] != "finished")
            .map(|(k, _)| k.clone())
            .collect()
    }

    fn seed_terminal(&self, address: &str, cwd: &str, labels: Value) {
        self.terminals.lock().unwrap().insert(
            address.to_owned(),
            json!({
                "address": address,
                "state": "live",
                "cwd": cwd,
                "status": { "state": "idle" },
                "labels": labels,
            }),
        );
    }
}

impl HerdrBackend for MockHerdr {
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        match method {
            "server.status" => Ok(json!({"core": {"id": self.core_id}})),
            "terminal.run" => {
                self.run_calls.fetch_add(1, Ordering::SeqCst);
                let n = self.next_id.fetch_add(1, Ordering::SeqCst);
                let addr = format!("sim-term-{n}");
                let rec = json!({
                    "address": addr,
                    "state": "live",
                    "cwd": params["cwd"],
                    "labels": params["labels"],
                    "status": { "state": "working" },
                });
                self.terminals
                    .lock()
                    .unwrap()
                    .insert(addr.clone(), rec.clone());
                Ok(rec)
            }
            "terminal.list" => {
                let list: Vec<Value> = self.terminals.lock().unwrap().values().cloned().collect();
                Ok(json!({ "terminals": list }))
            }
            "terminal.get" => {
                let addr = params["terminal"].as_str().unwrap_or("");
                self.terminals
                    .lock()
                    .unwrap()
                    .get(addr)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("terminal not found: {addr}"))
            }
            "terminal.send" => {
                self.send_calls.fetch_add(1, Ordering::SeqCst);
                if self.fail_send.load(Ordering::SeqCst) {
                    anyhow::bail!("simulated terminal.send transport error");
                }
                Ok(json!({ "ok": true }))
            }
            "terminal.kill" => {
                let addr = params["terminal"].as_str().unwrap_or("").to_owned();
                if self.fail_kill.load(Ordering::SeqCst) {
                    anyhow::bail!("simulated terminal.kill transport error for {addr}");
                }
                self.kill_calls.lock().unwrap().push(addr.clone());
                if let Some(t) = self.terminals.lock().unwrap().get_mut(&addr) {
                    t["state"] = json!("finished");
                }
                Ok(json!({ "ok": true }))
            }
            "terminal.read" => Ok(json!({ "output": "", "text": "" })),
            other => anyhow::bail!("unexpected MockHerdr call: {other}"),
        }
    }

    fn subscribe(&self, _wake: Arc<dyn Fn() + Send + Sync>) -> Result<BackendSubscription> {
        anyhow::bail!("unused in synchronous failpoint unit tests")
    }

    fn activity(&self, _core: &str, _terminal: &str) -> canban_herdr::activity::Activity {
        Default::default()
    }
}

#[test]
fn failpoint_herdr_after_terminal_run_marks_agent_unknown_and_closes_orphan_pty() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let herdr = MockHerdr::new("core-fp-1");
    let fp = Failpoints::shared();
    let env = ServerRuntimeEnv {
        clock: FixedClock::new(1_700_000_000_000),
        id_gen: DeterministicIdGen::arc(11),
        herdr_backend: Some(herdr.clone()),
        inline_workers: true,
        fs_durability: FsDurability::NoSync,
        sqlite_synchronous_off: true,
        disable_os_watchers: true,
        failpoints: Some(fp.clone()),
        ..Default::default()
    };
    let app = AppServer::open_with_env(home, PathBuf::from("/bin/echo"), env).unwrap();

    // Seed an executable stage and a fast local profile so `launch()` uses `MockHerdr`.
    app.config.write().unwrap().profiles = vec![Profile {
        id: "developer".into(),
        enabled: true,
        argv: Some(vec!["/bin/echo".into(), "{prompt}".into()]),
        ..Default::default()
    }];
    app.config.write().unwrap().default_agent = "developer".into();

    let project: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "FP1",
                "directory": repo,
                "workspace": {"create_worktree": false, "sync_base": false},
            }),
        )
        .unwrap(),
    )
    .unwrap();

    let card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": project.id, "title": "Crash after run"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"view": "full", "card_id": card.id, "revision": card.revision, "stage": "develop"}),
    )
    .unwrap();

    // Arm Crash Window #1: fail right after `terminal.run` succeeds, before SQLite commits receipt.
    fp.arm(
        FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB,
        FailpointAction::FailOnce("simulated crash after terminal.run".into()),
    );

    app.tick().unwrap();

    // 1. Herdr DID spawn the PTY, and the failpoint fired exactly once.
    assert_eq!(herdr.run_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fp.stats_for(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB).fired, 1);

    // 2. SQLite did NOT commit the terminal binding; run is marked `"unknown"` with attention.
    let snap = app.snapshot().unwrap();
    let entry = snap
        .terminals
        .iter()
        .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == card.id))
        .expect("agent terminal entry present");
    let run = entry.agent.as_ref().unwrap();
    assert_eq!(run.state, "unknown");
    assert!(run.terminal.is_none());
    assert!(!run.finished());
    let live_card = snap.cards.iter().find(|c| c.id == card.id).unwrap();
    assert!(live_card.attention.is_some());
    assert!(live_card.human_gate.is_some());

    // 3. Subsequent ticks MUST NOT auto-replay a duplicate PTY.
    app.tick().unwrap();
    assert_eq!(herdr.run_calls.load(Ordering::SeqCst), 1);

    // 4. Closing the unknown terminal discovers and kills the orphan PTY in Herdr via label.
    app.call(
        "terminal.close",
        json!({"terminal_id": entry.id, "request_key": "close-orphan-1"}),
    )
    .unwrap();
    assert_eq!(
        herdr.kill_calls.lock().unwrap().as_slice(),
        &["sim-term-1".to_owned()]
    );
}

#[test]
fn failpoint_herdr_after_terminal_run_on_create_terminal_recovers_via_orphan_close() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let herdr = MockHerdr::new("core-fp-term");
    let fp = Failpoints::shared();
    let env = ServerRuntimeEnv {
        clock: FixedClock::new(1_700_000_000_000),
        id_gen: DeterministicIdGen::arc(12),
        herdr_backend: Some(herdr.clone()),
        inline_workers: true,
        fs_durability: FsDurability::NoSync,
        sqlite_synchronous_off: true,
        disable_os_watchers: true,
        failpoints: Some(fp.clone()),
        ..Default::default()
    };
    let app = AppServer::open_with_env(home, PathBuf::from("/bin/echo"), env).unwrap();

    fp.arm(
        FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB,
        FailpointAction::FailOnce("simulated crash after shell launch".into()),
    );
    let created: TerminalEntry = serde_json::from_value(
        app.call(
            "terminal.create",
            json!({"cwd": repo, "request_key": "term-create-fp"}),
        )
        .unwrap(),
    )
    .unwrap();

    assert_eq!(herdr.run_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fp.stats_for(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB).fired, 1);
    let rt = created.runtime.as_ref().unwrap();
    assert_eq!(rt.state, "unknown");
    assert!(rt.terminal.is_none());

    app.call(
        "terminal.close",
        json!({"terminal_id": created.id, "request_key": "term-close-orphan"}),
    )
    .unwrap();
    assert_eq!(
        herdr.kill_calls.lock().unwrap().as_slice(),
        &["sim-term-1".to_owned()]
    );
}

#[test]
fn failpoint_herdr_after_send_marks_question_delivery_unknown_and_never_resends() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let herdr = MockHerdr::new("core-fp-send");
    herdr.seed_terminal("t-live-1", &repo.to_string_lossy(), json!({}));
    let fp = Failpoints::shared();
    let clock = FixedClock::new(1_700_000_000_000);
    let env = ServerRuntimeEnv {
        clock: clock.clone(),
        id_gen: DeterministicIdGen::arc(22),
        herdr_backend: Some(herdr.clone()),
        inline_workers: true,
        fs_durability: FsDurability::NoSync,
        sqlite_synchronous_off: true,
        disable_os_watchers: true,
        failpoints: Some(fp.clone()),
        ..Default::default()
    };
    let app = AppServer::open_with_env(home, PathBuf::from("/bin/echo"), env).unwrap();

    let project: Project = serde_json::from_value(
        app.call("project.create", json!({"name": "FP2", "directory": repo}))
            .unwrap(),
    )
    .unwrap();
    let card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": project.id, "title": "Mention delivery"}),
        )
        .unwrap(),
    )
    .unwrap();
    let opened = app
        .call(
            "agent.open",
            json!({"card_id": card.id, "profile_id": "developer"}),
        )
        .unwrap();
    let tid = opened["terminal_id"].as_str().unwrap().to_owned();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut entry: TerminalEntry = db.get(&tid)?;
            let run = entry.agent.as_mut().unwrap();
            run.state = "running".into();
            run.core_id = Some("core-fp-send".into());
            run.terminal = Some("t-live-1".into());
            run.connection_state = "connected".into();
            run.lifecycle.observed_at = Some(clock.now_ms());
            db.save(&entry)
        })
        .unwrap();

    app.call(
        "comment.add",
        json!({"view": "full", "card_id": card.id, "body": "@developer check status"}),
    )
    .unwrap();

    // Arm Crash Window #2: fail right after `terminal.send` succeeds, before `"delivered"` commit.
    fp.arm(
        FP_HERDR_AFTER_SEND_BEFORE_DB,
        FailpointAction::FailOnce("simulated crash after terminal.send".into()),
    );
    app.deliver_questions(&app.herdr_client()).unwrap();

    assert_eq!(herdr.send_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fp.stats_for(FP_HERDR_AFTER_SEND_BEFORE_DB).fired, 1);

    let snap = app.snapshot().unwrap();
    assert_eq!(snap.questions.len(), 1);
    let qid = snap.questions[0].id.clone();
    assert_eq!(snap.questions[0].state, "delivery_unknown");
    assert!(
        snap.cards
            .iter()
            .find(|c| c.id == card.id)
            .unwrap()
            .attention
            .is_some()
    );

    // Retrying delivery MUST NOT blindly resend an already-delivered message.
    app.deliver_questions(&app.herdr_client()).unwrap();
    assert_eq!(herdr.send_calls.load(Ordering::SeqCst), 1);

    // Agent reply reconciles the question cleanly.
    let reply = app
        .call(
            "agent.reply",
            json!({"question_id": qid, "body": "All green", "request_key": "fp2-reply"}),
        )
        .unwrap();
    assert_eq!(reply["accepted"], true);
    let final_snap = app.snapshot().unwrap();
    assert_eq!(final_snap.questions[0].state, "replied");
}

#[test]
fn failpoint_github_after_pr_create_and_merge_reconcile_cleanly_via_refresh() {
    struct FakeGitRunner {
        repo_git_dir: PathBuf,
        card_wt: PathBuf,
        branch: String,
        head_sha: String,
        target_head: String,
    }

    impl FakeGitRunner {
        fn ok(stdout: &str) -> ExecOutput {
            ExecOutput {
                status_code: 0,
                stdout: stdout.as_bytes().to_vec(),
                stderr: Vec::new(),
            }
        }

        fn err(code: i32, stderr: &str) -> ExecOutput {
            ExecOutput {
                status_code: code,
                stdout: Vec::new(),
                stderr: stderr.as_bytes().to_vec(),
            }
        }
    }

    impl CommandRunner for FakeGitRunner {
        fn is_system(&self) -> bool {
            false
        }

        fn run(&self, spec: &CommandSpec) -> Result<ExecOutput> {
            let mut i = 0;
            let mut cwd = spec.cwd.clone().unwrap_or_default();
            while i < spec.args.len() {
                match spec.args[i].as_str() {
                    "--no-optional-locks" => i += 1,
                    "-C" => {
                        cwd = PathBuf::from(spec.args.get(i + 1).cloned().unwrap_or_default());
                        i += 2;
                    }
                    _ => break,
                }
            }
            let sub = &spec.args[i..];
            let is_card = cwd == self.card_wt;
            let out = match sub.first().map(String::as_str) {
                Some("remote") => {
                    if sub.get(1).map(String::as_str) == Some("get-url") {
                        Self::ok("https://github.com/acme/app.git\n")
                    } else {
                        Self::ok("origin\n")
                    }
                }
                Some("check-ref-format") => Self::ok(""),
                Some("symbolic-ref") => {
                    if is_card {
                        Self::ok(&format!("{}\n", self.branch))
                    } else {
                        Self::ok("main\n")
                    }
                }
                Some("rev-parse") => {
                    if sub.iter().any(|a| a == "--is-inside-work-tree") {
                        Self::ok("true\n")
                    } else if sub.iter().any(|a| a == "--git-common-dir") {
                        Self::ok(&format!("{}\n", self.repo_git_dir.display()))
                    } else if sub
                        .iter()
                        .any(|a| a == "MERGE_HEAD" || a == "CHERRY_PICK_HEAD")
                    {
                        Self::err(128, "none")
                    } else if sub.iter().any(|a| a == "--git-path") {
                        Self::ok(&format!("{}\n", cwd.join(".git/none").display()))
                    } else if sub.iter().any(|a| a == "refs/heads/main^{commit}") {
                        Self::ok(&format!("{}\n", self.target_head))
                    } else if sub.iter().any(|a| a == "HEAD") {
                        if is_card {
                            Self::ok(&format!("{}\n", self.head_sha))
                        } else {
                            Self::ok(&format!("{}\n", self.target_head))
                        }
                    } else {
                        Self::ok(&format!("{}\n", self.head_sha))
                    }
                }
                Some("status") | Some("ls-files") | Some("show-ref") | Some("worktree") => {
                    Self::ok("")
                }
                Some("diff") => {
                    if sub.iter().any(|a| a == "--diff-filter=U") {
                        Self::ok("")
                    } else if sub.iter().any(|a| a == "--name-only") {
                        Self::ok("a.txt\0")
                    } else {
                        Self::ok("diff --git a/a.txt b/a.txt\n+hello\n")
                    }
                }
                Some("rev-list") => Self::ok("1\n"),
                Some("merge-base") => Self::ok(&format!("{}\n", self.target_head)),
                Some("push") | Some("rebase") | Some("fetch") | Some("checkout") => Self::ok(""),
                _ => Self::ok(""),
            };
            Ok(out)
        }

        fn find_executable(&self, _name: &str) -> Option<PathBuf> {
            None
        }

        fn shell_env(&self) -> canban_exec::shell_env::ShellEnvSnapshot {
            canban_exec::shell_env::ShellEnvSnapshot::default()
        }

        fn hostname(&self) -> String {
            "fp3-sim-host".into()
        }
    }

    struct MockGitHub {
        pr_head: String,
        pr_created: AtomicBool,
        pr_merged: AtomicBool,
        create_calls: AtomicUsize,
        merge_calls: AtomicUsize,
    }

    impl MockGitHub {
        fn json_ok(val: Value) -> HttpResponse {
            HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: serde_json::to_vec(&val).unwrap(),
            }
        }

        fn pr_payload(&self) -> Value {
            let merged = self.pr_merged.load(Ordering::SeqCst);
            json!({
                "number": 77,
                "html_url": "https://github.com/acme/app/pull/77",
                "state": if merged { "closed" } else { "open" },
                "merged": merged,
                "draft": false,
                "head": { "sha": self.pr_head, "ref": "proj/1" },
                "base": { "ref": "main", "repo": { "id": 777 } },
                "merge_commit_sha": if merged { json!("8888888888888888888888888888888888888888") } else { Value::Null },
            })
        }
    }

    impl HttpTransport for MockGitHub {
        fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
            match (req.method.as_str(), req.path.as_str()) {
                ("GET", "/repos/acme/app") => Ok(Self::json_ok(json!({
                    "id": 777,
                    "full_name": "acme/app",
                    "default_branch": "main",
                    "html_url": "https://github.com/acme/app",
                }))),
                ("GET", p) if p.starts_with("/repos/acme/app/pulls?") => {
                    if self.pr_created.load(Ordering::SeqCst) {
                        Ok(Self::json_ok(json!([self.pr_payload()])))
                    } else {
                        Ok(Self::json_ok(json!([])))
                    }
                }
                ("POST", "/repos/acme/app/pulls") => {
                    self.create_calls.fetch_add(1, Ordering::SeqCst);
                    self.pr_created.store(true, Ordering::SeqCst);
                    Ok(Self::json_ok(self.pr_payload()))
                }
                ("GET", "/repos/acme/app/pulls/77") => Ok(Self::json_ok(self.pr_payload())),
                ("PUT", "/repos/acme/app/pulls/77/merge") => {
                    self.merge_calls.fetch_add(1, Ordering::SeqCst);
                    self.pr_merged.store(true, Ordering::SeqCst);
                    Ok(Self::json_ok(json!({
                        "merged": true,
                        "sha": "8888888888888888888888888888888888888888",
                    })))
                }
                ("GET", p) if p.contains("/check-runs") => {
                    Ok(Self::json_ok(json!({"check_runs": []})))
                }
                ("GET", p) if p.contains("/status") => {
                    Ok(Self::json_ok(json!({"state": "success", "statuses": []})))
                }
                ("GET", p) if p.contains("/reviews") || p.contains("/comments") => {
                    Ok(Self::json_ok(json!([])))
                }
                other => anyhow::bail!("unexpected MockGitHub request: {other:?}"),
            }
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let repo = dir.path().join("repo");
    let card_wt = home.join("worktrees/proj/1");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&card_wt).unwrap();

    let head_sha = "2222222222222222222222222222222222222222".to_owned();
    let gh = Arc::new(MockGitHub {
        pr_head: head_sha.clone(),
        pr_created: AtomicBool::new(false),
        pr_merged: AtomicBool::new(false),
        create_calls: AtomicUsize::new(0),
        merge_calls: AtomicUsize::new(0),
    });
    let fp = Failpoints::shared();
    let env = ServerRuntimeEnv {
        clock: FixedClock::new(1_700_000_500_000),
        id_gen: DeterministicIdGen::arc(33),
        github_transport: Some(gh.clone()),
        exec_runner: Arc::new(FakeGitRunner {
            repo_git_dir: repo.join(".git"),
            card_wt: card_wt.clone(),
            branch: "proj/1".into(),
            head_sha: head_sha.clone(),
            target_head: "1111111111111111111111111111111111111111".into(),
        }),
        inline_workers: true,
        fs_durability: FsDurability::NoSync,
        sqlite_synchronous_off: true,
        disable_os_watchers: true,
        failpoints: Some(fp.clone()),
        ..Default::default()
    };
    let app = AppServer::open_with_env(home, PathBuf::from("/bin/echo"), env).unwrap();

    let project: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({"name": "FP3", "directory": repo, "target_branch": "main"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.call(
        "project.github",
        json!({"project_id": project.id, "repository": "acme/app", "intake": "off"}),
    )
    .unwrap();
    let card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": project.id, "title": "PR Crash Window"}),
        )
        .unwrap(),
    )
    .unwrap();
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut c: Card = db.get(&card.id)?;
            c.worktree = Some(card_wt.to_string_lossy().into_owned());
            c.branch = Some("proj/1".into());
            c.workspace = Some(WorkspaceBinding {
                directory: project.directory.clone(),
                path: card_wt.to_string_lossy().into_owned(),
                branch: Some("proj/1".into()),
                base_commit: Some("1111111111111111111111111111111111111111".into()),
                target_branch: Some("main".into()),
            });
            db.save(&c)
        })
        .unwrap();

    // Part A: Crash after GitHub PR creation, before SQLite commit.
    let preview = app
        .call("review.preview", json!({"card_id": card.id}))
        .unwrap();
    let token = preview["token"].as_str().unwrap().to_owned();
    fp.arm(
        FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB,
        FailpointAction::FailOnce("crash after create_pr".into()),
    );
    let publish_res = app
        .call(
            "github.pr.publish",
            json!({
                "card_id": card.id,
                "revision": card.revision,
                "token": token,
                "request_key": "fp3-pub-1",
            }),
        )
        .unwrap();
    assert_eq!(publish_res["state"], "unknown");
    assert_eq!(gh.create_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fp.stats_for(FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB).fired, 1);

    let after_pub_crash = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.id == card.id)
        .unwrap();
    assert_eq!(after_pub_crash.pull_request.as_ref().unwrap().number, 0);
    assert_eq!(
        after_pub_crash.pull_request.as_ref().unwrap().state,
        "unknown"
    );
    assert!(after_pub_crash.attention.is_some());

    // Reconcile via github.pr.refresh: discovers PR #77 without duplicate creation & clears stale attention.
    let refreshed = app
        .call("github.pr.refresh", json!({"card_id": card.id}))
        .unwrap();
    assert_eq!(refreshed["number"], 77);
    assert_eq!(refreshed["state"], "open");
    assert_eq!(gh.create_calls.load(Ordering::SeqCst), 1);
    let after_pub_refresh = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.id == card.id)
        .unwrap();
    assert_eq!(after_pub_refresh.pull_request.as_ref().unwrap().number, 77);
    assert!(after_pub_refresh.attention.is_none());
    assert!(after_pub_refresh.human_gate.is_none());

    // Part B: Crash after GitHub PR merge, before SQLite commit.
    fp.arm(
        FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB,
        FailpointAction::FailOnce("crash after merge_pr".into()),
    );
    let merge_res = app
        .call(
            "github.pr.merge",
            json!({
                "card_id": card.id,
                "revision": after_pub_refresh.revision,
                "head": head_sha,
                "request_key": "fp3-merge-1",
            }),
        )
        .unwrap();
    assert_eq!(merge_res["state"], "unknown");
    assert_eq!(gh.merge_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fp.stats_for(FP_GITHUB_AFTER_PR_WRITE_BEFORE_DB).fired, 2);

    let after_merge_crash = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.id == card.id)
        .unwrap();
    assert!(after_merge_crash.delivery.is_none());
    assert!(after_merge_crash.attention.is_some());

    // Reconcile via github.pr.refresh: discovers merged PR #77 and completes delivery cleanly.
    let refreshed_merged = app
        .call("github.pr.refresh", json!({"card_id": card.id}))
        .unwrap();
    assert_eq!(refreshed_merged["merged"], true);
    assert_eq!(gh.merge_calls.load(Ordering::SeqCst), 1);

    let final_card = app
        .snapshot()
        .unwrap()
        .cards
        .into_iter()
        .find(|c| c.id == card.id)
        .unwrap();
    assert!(final_card.delivery.is_some());
    assert_eq!(
        final_card.delivery.as_ref().unwrap().commit,
        "8888888888888888888888888888888888888888"
    );
    assert!(final_card.resolved());
    assert!(final_card.attention.is_none());
    assert!(final_card.human_gate.is_none());
}

fn git_cmd(dir: &Path, args: &[&str]) -> String {
    let out = canban_exec::raw_command("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Canban Test")
        .env("GIT_AUTHOR_EMAIL", "test@canban.local")
        .env("GIT_COMMITTER_NAME", "Canban Test")
        .env("GIT_COMMITTER_EMAIL", "test@canban.local")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn enable_auto_batch_delivery(app: &AppServer, project: &Project) {
    let col = app
        .call("workflow.sources", json!({"project_id": project.id}))
        .unwrap();
    let raw = col["sources"][0]["source"].as_str().unwrap();
    let mut doc = canban_workflow_engine::source::parse(raw).unwrap();
    canban_workflow_engine::stages::normalize(&mut doc, &[]);
    canban_workflow_engine::source::set_review_manual(&mut doc, false);
    canban_workflow_engine::source::set_sync_upstream(&mut doc, true);
    app.call(
        "workflow.apply",
        json!({
            "project_id": project.id,
            "revision": col["revision"],
            "sources": [{
                "filename": "development.yaml",
                "source": canban_workflow_engine::source::encode(&doc).unwrap(),
            }],
        }),
    )
    .unwrap();
}

#[test]
fn failpoint_git_after_push_before_batch_finalize_recovers_idempotently_and_cleans_startup_state() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote.git");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&remote).unwrap();

    git_cmd(&remote, &["init", "--bare", "-b", "main"]);
    git_cmd(&source, &["init", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Canban Test"),
        ("user.email", "test@canban.local"),
        ("core.hooksPath", "/dev/null"),
        ("commit.gpgsign", "false"),
    ] {
        git_cmd(&source, &["config", k, v]);
    }
    std::fs::write(source.join("README.md"), "base\n").unwrap();
    git_cmd(&source, &["add", "."]);
    git_cmd(&source, &["commit", "-m", "initial"]);
    git_cmd(
        &source,
        &["remote", "add", "origin", &remote.to_string_lossy()],
    );
    git_cmd(&source, &["push", "-u", "origin", "main"]);

    let fp = Failpoints::shared();
    let make_env = |clock_ms: u64| ServerRuntimeEnv {
        clock: FixedClock::new(clock_ms),
        id_gen: DeterministicIdGen::arc(44),
        inline_workers: true,
        fs_durability: FsDurability::NoSync,
        sqlite_synchronous_off: true,
        disable_os_watchers: true,
        failpoints: Some(fp.clone()),
        ..Default::default()
    };
    let app = AppServer::open_with_env(
        home.clone(),
        PathBuf::from("/bin/echo"),
        make_env(1_700_000_000_000),
    )
    .unwrap();

    let project: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "FP4",
                "directory": source,
                "target_branch": "main",
                "workspace": {"create_worktree": true, "sync_base": false},
            }),
        )
        .unwrap(),
    )
    .unwrap();
    enable_auto_batch_delivery(&app, &project);

    // Hold auto-delivery with an untracked file in the main checkout while both Cards
    // enter `waiting_delivery`, so `pump_deliveries()` integrates them together as a batch.
    std::fs::write(source.join("wip.local"), "hold queue\n").unwrap();

    // Create two clean Cards with separate worktrees and advance both to waiting_delivery.
    let mut cards = Vec::new();
    for i in 1..=2 {
        let c: Card = serde_json::from_value(
            app.call(
                "card.create",
                json!({"view": "full", "project_id": project.id, "title": format!("Batch card {i}")}),
            )
            .unwrap(),
        )
        .unwrap();
        let moved: Card = serde_json::from_value(
            app.call(
                "card.move",
                json!({"view": "full", "card_id": c.id, "revision": c.revision, "stage": "develop"}),
            )
            .unwrap(),
        )
        .unwrap();
        let wt = app.ops().prepare(&home, &project, &moved).unwrap();
        let (branch, base_commit) = app.ops().workspace_head_meta(&wt, &moved);
        std::fs::write(wt.join(format!("file_{i}.txt")), format!("content {i}\n")).unwrap();
        git_cmd(&wt, &["add", "."]);
        git_cmd(&wt, &["commit", "-m", &format!("Add file {i}")]);
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut cur: Card = db.get(&moved.id)?;
                cur.worktree = Some(wt.to_string_lossy().into_owned());
                cur.branch = branch.clone();
                cur.base_commit = base_commit.clone();
                cur.workspace = Some(WorkspaceBinding {
                    directory: project.directory.clone(),
                    path: wt.to_string_lossy().into_owned(),
                    branch,
                    base_commit,
                    target_branch: Some("main".into()),
                });
                db.save(&cur)
            })
            .unwrap();
        let term = app
            .snapshot()
            .unwrap()
            .terminals
            .into_iter()
            .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == moved.id))
            .unwrap();
        admit(&app, &term);
        app.call(
            "execution.report",
            json!({
                "project_id": project.id,
                "card": moved.number,
                "state": "completed",
                "message": "Ready",
                "request_key": format!("fp4-rep-{i}"),
            }),
        )
        .unwrap();
        // Mark terminal finished so settle grace does not hold delivery.
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut entry: TerminalEntry = db.get(&term.id)?;
                if let Some(a) = &mut entry.agent {
                    a.observed = Some(json!({"state": "finished"}));
                }
                db.save(&entry)
            })
            .unwrap();
        cards.push(moved);
    }

    // Release the main checkout hold so both queued Cards integrate in one batch.
    std::fs::remove_file(source.join("wip.local")).unwrap();

    // Arm Crash Window #4: fail right after `git push` succeeds, before batch finalization in SQLite.
    fp.arm(
        FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE,
        FailpointAction::FailOnce("simulated crash after git push".into()),
    );
    let err = app.pump_deliveries().unwrap_err();
    assert!(
        format!("{err:#}").contains(FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE),
        "expected failpoint error, got: {err:#}"
    );
    assert_eq!(
        fp.stats_for(FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE).fired,
        1
    );

    // 1. Git push DID succeed: remote `main` already has both files.
    let pushed_remote_head = git_cmd(&remote, &["rev-parse", "refs/heads/main"]);
    let local_head = git_cmd(&source, &["rev-parse", "HEAD"]);
    assert_eq!(pushed_remote_head, local_head);
    assert!(source.join("file_1.txt").exists());
    assert!(source.join("file_2.txt").exists());

    // 2. SQLite did NOT finalize cards yet, AND no wedged `"integrating"` batch row remains.
    let mid_snap = app.snapshot().unwrap();
    assert!(mid_snap.merge_batches.is_empty());
    for c in &cards {
        let live = mid_snap.cards.iter().find(|x| x.id == c.id).unwrap();
        assert!(live.delivery.is_none());
    }

    // 3. Also prove startup recovery cleans up an interrupted `"integrating"` row left by an abrupt crash.
    let stale_batch = canban_types::MergeBatch {
        id: format!("merge-queue:{}:stale001", project.id),
        project_id: project.id.clone(),
        profile_id: "merger".into(),
        provider_session_id: None,
        target_branch: "main".into(),
        target_head: pushed_remote_head.clone(),
        strategy: MergeStrategy::Rebase,
        sync_upstream: true,
        test_command: None,
        worktree: String::new(),
        state: "integrating".into(),
        failure_kind: None,
        conflict_card_number: None,
        conflict_files: Vec::new(),
        test_output: None,
        items: Vec::new(),
        prompt: String::new(),
        error: None,
        queued_at: None,
        started_at: None,
        created_at: 1_700_000_000_000,
        updated_at: 1_700_000_000_000,
    };
    app.store
        .lock()
        .unwrap()
        .write(|db| db.save(&stale_batch))
        .unwrap();
    drop(app);

    let reopened = AppServer::open_with_env(
        home,
        PathBuf::from("/bin/echo"),
        make_env(1_700_000_001_000),
    )
    .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _running = reopened.start_in_runtime(rt.handle().clone()).unwrap();
    assert!(
        reopened.snapshot().unwrap().merge_batches.is_empty(),
        "startup recovery must clear interrupted integrating/testing batch rows"
    );

    // 4. Retrying delivery after restart converges idempotently without duplicate commits!
    assert!(reopened.pump_deliveries().unwrap());
    let final_snap = reopened.snapshot().unwrap();
    assert!(final_snap.merge_batches.is_empty());
    for c in &cards {
        let live = final_snap.cards.iter().find(|x| x.id == c.id).unwrap();
        assert!(
            live.delivery.is_some(),
            "Card #{} must finalize cleanly on retry after post-push crash",
            c.number
        );
        assert!(live.resolved());
    }
    assert_eq!(
        git_cmd(&remote, &["rev-parse", "refs/heads/main"]),
        pushed_remote_head,
        "idempotent recovery after post-push crash must not add duplicate commits"
    );
}

#[test]
fn failpoint_batch_report_after_push_retries_idempotently_when_target_already_fast_forwarded() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote.git");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&remote).unwrap();

    git_cmd(&remote, &["init", "--bare", "-b", "main"]);
    git_cmd(&source, &["init", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Canban Test"),
        ("user.email", "test@canban.local"),
        ("core.hooksPath", "/dev/null"),
        ("commit.gpgsign", "false"),
    ] {
        git_cmd(&source, &["config", k, v]);
    }
    std::fs::write(source.join("shared.txt"), "base\n").unwrap();
    git_cmd(&source, &["add", "."]);
    git_cmd(&source, &["commit", "-m", "initial"]);
    git_cmd(
        &source,
        &["remote", "add", "origin", &remote.to_string_lossy()],
    );
    git_cmd(&source, &["push", "-u", "origin", "main"]);

    let fp = Failpoints::shared();
    let herdr = MockHerdr::new("core-batch-report");
    let env = ServerRuntimeEnv {
        clock: FixedClock::new(1_700_000_000_000),
        id_gen: DeterministicIdGen::arc(45),
        herdr_backend: Some(herdr.clone()),
        inline_workers: true,
        fs_durability: FsDurability::NoSync,
        sqlite_synchronous_off: true,
        disable_os_watchers: true,
        failpoints: Some(fp.clone()),
        ..Default::default()
    };
    let app = AppServer::open_with_env(home.clone(), PathBuf::from("/bin/echo"), env).unwrap();

    let project: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "FP4-Report",
                "directory": source,
                "target_branch": "main",
                "workspace": {"create_worktree": true, "sync_base": false},
            }),
        )
        .unwrap(),
    )
    .unwrap();
    enable_auto_batch_delivery(&app, &project);

    // Hold auto-delivery while both conflicting Cards enter `waiting_delivery` together.
    std::fs::write(source.join("wip.local"), "hold queue\n").unwrap();

    // Create two conflicting Cards editing `shared.txt` so batch delivery hands off to @merger.
    let mut cards = Vec::new();
    for (i, line) in [(1, "card-1-edit\n"), (2, "card-2-edit\n")] {
        let c: Card = serde_json::from_value(
            app.call(
                "card.create",
                json!({"view": "full", "project_id": project.id, "title": format!("Conflict card {i}")}),
            )
            .unwrap(),
        )
        .unwrap();
        let moved: Card = serde_json::from_value(
            app.call(
                "card.move",
                json!({"view": "full", "card_id": c.id, "revision": c.revision, "stage": "develop"}),
            )
            .unwrap(),
        )
        .unwrap();
        let wt = app.ops().prepare(&home, &project, &moved).unwrap();
        let (branch, base_commit) = app.ops().workspace_head_meta(&wt, &moved);
        std::fs::write(wt.join("shared.txt"), line).unwrap();
        git_cmd(&wt, &["add", "."]);
        git_cmd(&wt, &["commit", "-m", &format!("Edit shared {i}")]);
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut cur: Card = db.get(&moved.id)?;
                cur.worktree = Some(wt.to_string_lossy().into_owned());
                cur.branch = branch.clone();
                cur.base_commit = base_commit.clone();
                cur.workspace = Some(WorkspaceBinding {
                    directory: project.directory.clone(),
                    path: wt.to_string_lossy().into_owned(),
                    branch,
                    base_commit,
                    target_branch: Some("main".into()),
                });
                db.save(&cur)
            })
            .unwrap();
        let term = app
            .snapshot()
            .unwrap()
            .terminals
            .into_iter()
            .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == moved.id))
            .unwrap();
        admit(&app, &term);
        app.call(
            "execution.report",
            json!({
                "project_id": project.id,
                "card": moved.number,
                "state": "completed",
                "message": "Ready",
                "request_key": format!("fp4-conf-rep-{i}"),
            }),
        )
        .unwrap();
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut entry: TerminalEntry = db.get(&term.id)?;
                if let Some(a) = &mut entry.agent {
                    a.observed = Some(json!({"state": "finished"}));
                }
                db.save(&entry)
            })
            .unwrap();
        cards.push(moved);
    }

    // Release hold and pump delivery -> conflict materializes in _merge_queue and launches @merger.
    std::fs::remove_file(source.join("wip.local")).unwrap();
    app.pump_deliveries().unwrap();
    let snap = app.snapshot().unwrap();
    assert_eq!(snap.merge_batches.len(), 1);
    let batch = &snap.merge_batches[0];
    assert_eq!(batch.state, "running");

    // Simulate @merger resolving conflict cleanly in `_merge_queue`.
    let bwt = PathBuf::from(&batch.worktree);
    std::fs::write(bwt.join("shared.txt"), "card-1-edit\ncard-2-edit\n").unwrap();
    git_cmd(&bwt, &["add", "shared.txt"]);
    git_cmd(&bwt, &["commit", "--no-edit"]);
    let resolved_head = git_cmd(&bwt, &["rev-parse", "HEAD"]);

    // Arm Crash Window #4 during `batch.report(state="completed")`.
    fp.arm(
        FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE,
        FailpointAction::FailOnce("crash after batch.report git push".into()),
    );
    let err = app
        .call(
            "batch.report",
            json!({
                "batch_id": batch.id,
                "state": "completed",
                "message": "Resolved conflict",
                "request_key": "fp4-batch-report-1",
            }),
        )
        .unwrap_err();
    assert!(format!("{err:#}").contains(FP_GIT_AFTER_PUSH_BEFORE_BATCH_FINALIZE));
    assert_eq!(
        git_cmd(&remote, &["rev-parse", "refs/heads/main"]),
        resolved_head
    );
    assert_eq!(git_cmd(&source, &["rev-parse", "HEAD"]), resolved_head);
    // Batch not finalized in SQLite yet.
    assert_eq!(app.snapshot().unwrap().merge_batches[0].state, "running");

    // Retry exact same `batch.report`: succeeds idempotently even though main is already at `resolved_head`!
    let report = app
        .call(
            "batch.report",
            json!({
                "batch_id": batch.id,
                "state": "completed",
                "message": "Resolved conflict",
                "request_key": "fp4-batch-report-1",
            }),
        )
        .unwrap();
    assert_eq!(report["accepted"], true);
    assert_eq!(report["delivered"], 2);
    let final_snap = app.snapshot().unwrap();
    assert_eq!(final_snap.merge_batches[0].state, "completed");
    for c in &cards {
        let live = final_snap.cards.iter().find(|x| x.id == c.id).unwrap();
        assert!(live.delivery.is_some());
        assert!(live.resolved());
    }
    assert_eq!(
        git_cmd(&remote, &["rev-parse", "refs/heads/main"]),
        resolved_head
    );
}

#[test]
fn serve_local_stream_handles_framed_json_rpc_over_generic_async_io() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let env = ServerRuntimeEnv {
        clock: FixedClock::new(1_700_000_000_000),
        id_gen: DeterministicIdGen::arc(55),
        inline_workers: true,
        fs_durability: FsDurability::NoSync,
        sqlite_synchronous_off: true,
        disable_os_watchers: true,
        ..Default::default()
    };
    let app = AppServer::open_with_env(home, PathBuf::from("/bin/echo"), env).unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let srv_task = tokio::spawn(Arc::clone(&app).serve_local_stream(server_io));

        let (read_half, mut write_half) = tokio::io::split(client_io);
        let mut reader = BufReader::new(read_half);

        let req1 = json!({"jsonrpc": "2.0", "id": 1, "method": "app.ping", "params": {}});
        write_half
            .write_all(format!("{req1}\n").as_bytes())
            .await
            .unwrap();
        write_half.flush().await.unwrap();

        let mut line1 = String::new();
        reader.read_line(&mut line1).await.unwrap();
        let resp1: Value = serde_json::from_str(&line1).unwrap();
        assert_eq!(resp1["id"], 1);
        assert_eq!(resp1["result"]["server_id"], app.server_id());
        assert_eq!(resp1["result"]["api_version"], app.api_version());

        let req2 = json!({"jsonrpc": "2.0", "id": 2, "method": "project.list", "params": {}});
        write_half
            .write_all(format!("{req2}\n").as_bytes())
            .await
            .unwrap();
        write_half.flush().await.unwrap();

        let mut line2 = String::new();
        reader.read_line(&mut line2).await.unwrap();
        let resp2: Value = serde_json::from_str(&line2).unwrap();
        assert_eq!(resp2["id"], 2);
        assert_eq!(resp2["result"], json!([]));

        drop(write_half);
        drop(reader);
        srv_task.await.unwrap().unwrap();
    });
}

#[test]
fn failpoint_pump_cardless_agents_after_terminal_run_kills_orphan_pty_and_retries_kill_on_outage() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let herdr = MockHerdr::new("core-cardless-fp1");
    let fp = Failpoints::shared();
    let clock = FixedClock::new(1_700_000_000_000);
    let env = ServerRuntimeEnv {
        clock: clock.clone(),
        id_gen: DeterministicIdGen::arc(61),
        herdr_backend: Some(herdr.clone()),
        inline_workers: true,
        fs_durability: FsDurability::NoSync,
        sqlite_synchronous_off: true,
        disable_os_watchers: true,
        failpoints: Some(fp.clone()),
        ..Default::default()
    };
    let app = AppServer::open_with_env(home, PathBuf::from("/bin/echo"), env).unwrap();
    app.observer_running.store(true, Ordering::SeqCst);
    app.config.write().unwrap().profiles = vec![Profile {
        id: "developer".into(),
        enabled: true,
        argv: Some(vec!["/bin/echo".into(), "{prompt}".into()]),
        ..Default::default()
    }];
    app.config.write().unwrap().default_agent = "developer".into();

    let project: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "CardlessFP1",
                "directory": repo,
                "workspace": {"create_worktree": false, "sync_base": false},
            }),
        )
        .unwrap(),
    )
    .unwrap();

    let cardless_yaml = r#"schema: 12
id: cardless:agent-fp1
name: Cardless Agent FP1
on:
  triage_event:
    event: github.issue.opened
    start_at: triage
    cardless: true
steps:
  - id: triage
    uses: agent
    with:
      profile: developer
      instruction: Triage the issue
    next: done
  - id: done
    uses: complete
"#;
    let flow = canban_workflow_engine::source::parse(cardless_yaml)
        .unwrap()
        .compile()
        .unwrap();

    // Part A: FP #1 fires after `terminal.run` while Herdr is reachable -> orphan PTY is killed immediately.
    let run1 = app
        .store
        .lock()
        .unwrap()
        .write(|db| {
            canban_workflow_engine::runtime::start_cardless_at(
                db,
                &project.id,
                flow.clone(),
                1,
                "triage_event",
                None,
                clock.now_ms(),
            )
        })
        .unwrap();
    assert_eq!(run1.state, "waiting_agent");

    fp.arm(
        FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB,
        FailpointAction::FailOnce("simulated crash after cardless terminal.run".into()),
    );
    app.pump_cardless_agents().unwrap();

    assert_eq!(herdr.run_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fp.stats_for(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB).fired, 1);
    assert_eq!(
        herdr.kill_calls.lock().unwrap().as_slice(),
        &["sim-term-1".to_owned()]
    );
    assert!(
        herdr.live_addresses().is_empty(),
        "must not leak any live orphan PTY after cardless FP #1: {:?}",
        herdr.live_addresses()
    );
    let snap1 = app.snapshot().unwrap();
    let live_run1 = snap1
        .workflow_runs
        .iter()
        .find(|r| r.id == run1.id)
        .unwrap();
    assert_eq!(live_run1.state, "failed");
    assert!(live_run1.error.is_some());
    assert!(live_run1.inputs.get("agent_terminal").is_none());

    // Subsequent pump must not relaunch a duplicate PTY.
    app.pump_cardless_agents().unwrap();
    assert_eq!(herdr.run_calls.load(Ordering::SeqCst), 1);

    // Part B: FP #1 fires AND `terminal.kill` transiently fails -> tracks `orphan_kill_pending`,
    // refuses duplicate launch while Herdr is failing, then kills & fails cleanly once Herdr recovers.
    let run2 = app
        .store
        .lock()
        .unwrap()
        .write(|db| {
            canban_workflow_engine::runtime::start_cardless_at(
                db,
                &project.id,
                flow,
                1,
                "triage_event",
                None,
                clock.now_ms(),
            )
        })
        .unwrap();
    herdr.fail_kill.store(true, Ordering::SeqCst);
    fp.arm(
        FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB,
        FailpointAction::FailOnce("simulated crash + kill outage".into()),
    );
    app.pump_cardless_agents().unwrap();

    assert_eq!(herdr.run_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fp.stats_for(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB).fired, 2);
    let mid_snap = app.snapshot().unwrap();
    let pending_run2 = mid_snap
        .workflow_runs
        .iter()
        .find(|r| r.id == run2.id)
        .unwrap();
    assert_eq!(pending_run2.state, "waiting_agent");
    assert_eq!(
        pending_run2.inputs["agent_terminal"]["address"],
        json!("sim-term-2")
    );
    assert_eq!(
        pending_run2.inputs["agent_terminal"]["orphan_kill_pending"],
        json!(true)
    );

    // Retrying while kill still fails must NOT launch a third PTY.
    app.pump_cardless_agents().unwrap();
    assert_eq!(herdr.run_calls.load(Ordering::SeqCst), 2);

    // Once Herdr recovers, the next pump terminates `sim-term-2` and marks the run failed.
    herdr.fail_kill.store(false, Ordering::SeqCst);
    app.pump_cardless_agents().unwrap();
    assert_eq!(herdr.run_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        herdr.kill_calls.lock().unwrap().as_slice(),
        &["sim-term-1".to_owned(), "sim-term-2".to_owned()]
    );
    assert!(herdr.live_addresses().is_empty());
    let final_snap = app.snapshot().unwrap();
    let done_run2 = final_snap
        .workflow_runs
        .iter()
        .find(|r| r.id == run2.id)
        .unwrap();
    assert_eq!(done_run2.state, "failed");
    assert!(done_run2.inputs.get("agent_terminal").is_none());
}

#[test]
fn failpoint_pump_scripts_herdr_after_terminal_run_kills_orphan_pty_and_retries_kill_on_outage() {
    struct NoSpawnRunner;
    impl CommandRunner for NoSpawnRunner {
        fn run(&self, spec: &CommandSpec) -> Result<ExecOutput> {
            anyhow::bail!("unexpected OS spawn: {} {:?}", spec.program, spec.args);
        }
        fn find_executable(&self, _name: &str) -> Option<PathBuf> {
            None
        }
        fn shell_env(&self) -> canban_exec::shell_env::ShellEnvSnapshot {
            canban_exec::shell_env::ShellEnvSnapshot {
                shell: "/bin/sh".into(),
                vars: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
                path_entries: vec![PathBuf::from("/usr/bin"), PathBuf::from("/bin")],
                full_env_captured: true,
            }
        }
        fn hostname(&self) -> String {
            "script-fp1-host".into()
        }
        fn is_system(&self) -> bool {
            false
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let herdr = MockHerdr::new("core-script-fp1");
    let fp = Failpoints::shared();
    let clock = FixedClock::new(1_700_000_000_000);
    let env = ServerRuntimeEnv {
        clock: clock.clone(),
        id_gen: DeterministicIdGen::arc(62),
        herdr_backend: Some(herdr.clone()),
        exec_runner: Arc::new(NoSpawnRunner),
        inline_workers: true,
        fs_durability: FsDurability::NoSync,
        sqlite_synchronous_off: true,
        disable_os_watchers: true,
        failpoints: Some(fp.clone()),
        script_execution_mode: ScriptExecutionMode::Herdr,
        ..Default::default()
    };
    let app = AppServer::open_with_env(home, PathBuf::from("/bin/echo"), env).unwrap();

    let wf_yaml = r#"schema: 12
id: herdr-script-fp1
name: Herdr Script FP1
workspace:
  create_worktree: false
  sync_base: false
columns:
  - id: backlog
    name: Backlog
on:
  start:
    event: manual
    start_at: check
steps:
  - id: check
    stage: backlog
    uses: script
    with:
      run: echo hi
      timeout_seconds: 30
    next: done
  - id: done
    stage: backlog
    uses: complete
"#;
    let project: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "ScriptFP1",
                "directory": repo,
                "workflow_yaml": wf_yaml,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let card: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": project.id, "title": "Script FP1 card"}),
        )
        .unwrap(),
    )
    .unwrap();

    // Part A: FP #1 fires after `terminal.run` in `pump_scripts_herdr` -> kills orphan PTY immediately
    // and pauses the Card workflow for human approval with attention & human_gate.
    fp.arm(
        FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB,
        FailpointAction::FailOnce("simulated crash after script terminal.run".into()),
    );
    app.call(
        "workflow.start",
        json!({ "workflow_id": "herdr-script-fp1", "card_id": card.id, "entry": "start" }),
    )
    .unwrap();

    assert_eq!(herdr.run_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fp.stats_for(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB).fired, 1);
    assert_eq!(
        herdr.kill_calls.lock().unwrap().as_slice(),
        &["sim-term-1".to_owned()]
    );
    assert!(
        herdr.live_addresses().is_empty(),
        "must not leak any live orphan script PTY after FP #1: {:?}",
        herdr.live_addresses()
    );
    let snap1 = app.snapshot().unwrap();
    let run1 = snap1
        .workflow_runs
        .iter()
        .find(|r| r.card_id.as_deref() == Some(card.id.as_str()))
        .unwrap();
    assert_eq!(run1.state, "waiting_approval");
    assert!(run1.inputs.get("script_terminal").is_none());
    let live_card = snap1.cards.iter().find(|c| c.id == card.id).unwrap();
    assert!(live_card.attention.is_some());
    assert!(live_card.human_gate.is_some());

    // Subsequent pump must not launch a duplicate PTY.
    assert!(!app.pump_scripts().unwrap());
    assert_eq!(herdr.run_calls.load(Ordering::SeqCst), 1);

    // Part B: Cardless script hits FP #1 while `terminal.kill` transiently fails -> tracks
    // `orphan_kill_pending`, avoids duplicate launches, then kills & fails cleanly once Herdr recovers.
    let cardless_script_yaml = r#"schema: 12
id: cardless:script-fp1
name: Cardless Script FP1
on:
  hook:
    event: github.issue.opened
    start_at: check
    cardless: true
steps:
  - id: check
    uses: script
    with:
      run: echo cardless
      timeout_seconds: 30
    next: done
  - id: done
    uses: complete
"#;
    let cardless_flow = canban_workflow_engine::source::parse(cardless_script_yaml)
        .unwrap()
        .compile()
        .unwrap();
    let run2 = app
        .store
        .lock()
        .unwrap()
        .write(|db| {
            canban_workflow_engine::runtime::start_cardless_at(
                db,
                &project.id,
                cardless_flow,
                1,
                "hook",
                None,
                clock.now_ms(),
            )
        })
        .unwrap();
    assert_eq!(run2.state, "waiting_script");

    herdr.fail_kill.store(true, Ordering::SeqCst);
    fp.arm(
        FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB,
        FailpointAction::FailOnce("simulated crash + kill outage for script".into()),
    );
    assert!(app.pump_scripts().unwrap());

    assert_eq!(herdr.run_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fp.stats_for(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB).fired, 2);
    let mid_snap = app.snapshot().unwrap();
    let pending_run2 = mid_snap
        .workflow_runs
        .iter()
        .find(|r| r.id == run2.id)
        .unwrap();
    assert_eq!(pending_run2.state, "waiting_script");
    assert_eq!(
        pending_run2.inputs["script_terminal"]["address"],
        json!("sim-term-2")
    );
    assert_eq!(
        pending_run2.inputs["script_terminal"]["orphan_kill_pending"],
        json!(true)
    );

    // Retrying while kill still fails must NOT launch a third PTY.
    assert!(app.pump_scripts().unwrap());
    assert_eq!(herdr.run_calls.load(Ordering::SeqCst), 2);

    // Once Herdr recovers, next pump terminates `sim-term-2` and marks the cardless run failed.
    herdr.fail_kill.store(false, Ordering::SeqCst);
    assert!(!app.pump_scripts().unwrap());
    assert_eq!(herdr.run_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        herdr.kill_calls.lock().unwrap().as_slice(),
        &["sim-term-1".to_owned(), "sim-term-2".to_owned()]
    );
    assert!(herdr.live_addresses().is_empty());
    let final_snap = app.snapshot().unwrap();
    let done_run2 = final_snap
        .workflow_runs
        .iter()
        .find(|r| r.id == run2.id)
        .unwrap();
    assert_eq!(done_run2.state, "failed");
    assert!(done_run2.inputs.get("script_terminal").is_none());
}

#[test]
fn continue_assignment_question_delivery_ordering_on_send_error_and_failpoint() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let herdr = MockHerdr::new("core-continue-fp2");
    let fp = Failpoints::shared();
    let clock = FixedClock::new(1_700_000_000_000);
    let env = ServerRuntimeEnv {
        clock: clock.clone(),
        id_gen: DeterministicIdGen::arc(63),
        herdr_backend: Some(herdr.clone()),
        inline_workers: true,
        fs_durability: FsDurability::NoSync,
        sqlite_synchronous_off: true,
        disable_os_watchers: true,
        failpoints: Some(fp.clone()),
        ..Default::default()
    };
    let app = AppServer::open_with_env(home, PathBuf::from("/bin/echo"), env).unwrap();

    let project: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "ContinueOrdering",
                "directory": repo,
                "workspace": {"create_worktree": false, "sync_base": false},
            }),
        )
        .unwrap(),
    )
    .unwrap();

    let setup_retained_with_question = |title: &str, herdr_addr: &str, body: &str| {
        herdr.seed_terminal(herdr_addr, &repo.to_string_lossy(), json!({}));
        let card: Card = serde_json::from_value(
            app.call(
                "card.create",
                json!({"view": "full", "project_id": project.id, "title": title}),
            )
            .unwrap(),
        )
        .unwrap();
        let opened = app
            .call(
                "agent.open",
                json!({"card_id": card.id, "profile_id": "developer"}),
            )
            .unwrap();
        let tid = opened["terminal_id"].as_str().unwrap().to_owned();
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut entry: TerminalEntry = db.get(&tid)?;
                let run = entry.agent.as_mut().unwrap();
                run.state = "completed".into();
                run.core_id = Some("core-continue-fp2".into());
                run.terminal = Some(herdr_addr.into());
                run.cwd = repo.to_string_lossy().into_owned();
                run.connection_state = "connected".into();
                run.observed = Some(json!({"state": "live", "status": {"state": "idle"}}));
                run.lifecycle.observed_at = Some(clock.now_ms());
                let profile = run.profile.clone();
                run.pending = Some(QueuedAssignment {
                    stage: card.stage.clone(),
                    trigger: ExecutionTrigger::Interactive,
                    profile,
                    instruction: "Follow up on the pending question".into(),
                    on_completed: None,
                    require_human_review: false,
                    workflow_run_id: None,
                    workflow_step: None,
                    queued_at: clock.now_ms(),
                });
                db.save(&entry)
            })
            .unwrap();
        app.call(
            "comment.add",
            json!({"view": "full", "card_id": card.id, "body": body}),
        )
        .unwrap();
        let snap = app.snapshot().unwrap();
        let entry = snap
            .terminals
            .iter()
            .find(|t| t.id == tid)
            .cloned()
            .unwrap();
        let qid = snap
            .questions
            .iter()
            .find(|q| q.card_id == card.id)
            .map(|q| q.id.clone())
            .expect("pending question created");
        // Ensure the pending question is bound to this retained terminal before continue_assignment.
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut q: Question = db.get(&qid)?;
                q.terminal_id = Some(tid.clone());
                q.state = "pending".into();
                db.save(&q)
            })
            .unwrap();
        (card.id, tid, qid, entry)
    };

    // Case A: `terminal.send` itself errors -> pending Question must become `delivery_unknown` (never `delivered`).
    let (card_a, tid_a, qid_a, entry_a) =
        setup_retained_with_question("Send error", "t-continue-a", "@developer handle case A");
    herdr.fail_send.store(true, Ordering::SeqCst);
    let snap_before_a = app.snapshot().unwrap();
    let err_a = app
        .continue_assignment(&entry_a, &snap_before_a)
        .unwrap_err();
    assert!(format!("{err_a:#}").contains("simulated terminal.send transport error"));
    assert_eq!(herdr.send_calls.load(Ordering::SeqCst), 1);

    let snap_a = app.snapshot().unwrap();
    let q_a = snap_a.questions.iter().find(|q| q.id == qid_a).unwrap();
    assert_eq!(q_a.state, "delivery_unknown");
    assert!(q_a.error.is_some());
    let run_a = snap_a
        .terminals
        .iter()
        .find(|t| t.id == tid_a)
        .unwrap()
        .agent
        .as_ref()
        .unwrap();
    assert_eq!(run_a.state, "unknown");
    let live_card_a = snap_a.cards.iter().find(|c| c.id == card_a).unwrap();
    assert!(live_card_a.attention.is_some());
    assert!(live_card_a.human_gate.is_some());

    // `deliver_questions` must NOT blindly resend the `delivery_unknown` question.
    herdr.fail_send.store(false, Ordering::SeqCst);
    app.deliver_questions(&app.herdr_client()).unwrap();
    assert_eq!(herdr.send_calls.load(Ordering::SeqCst), 1);

    // Case B: `terminal.send` succeeds, then FP #2 fires before post-send DB commit -> `delivery_unknown` (never `delivered`).
    let (card_b, tid_b, qid_b, entry_b) =
        setup_retained_with_question("FP2 after send", "t-continue-b", "@developer handle case B");
    fp.arm(
        FP_HERDR_AFTER_SEND_BEFORE_DB,
        FailpointAction::FailOnce("simulated crash after continue terminal.send".into()),
    );
    let snap_before_b = app.snapshot().unwrap();
    let err_b = app
        .continue_assignment(&entry_b, &snap_before_b)
        .unwrap_err();
    assert!(format!("{err_b:#}").contains(FP_HERDR_AFTER_SEND_BEFORE_DB));
    assert_eq!(herdr.send_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fp.stats_for(FP_HERDR_AFTER_SEND_BEFORE_DB).fired, 1);

    let snap_b = app.snapshot().unwrap();
    let q_b = snap_b.questions.iter().find(|q| q.id == qid_b).unwrap();
    assert_eq!(q_b.state, "delivery_unknown");
    assert!(q_b.error.is_some());
    let run_b = snap_b
        .terminals
        .iter()
        .find(|t| t.id == tid_b)
        .unwrap()
        .agent
        .as_ref()
        .unwrap();
    assert_eq!(run_b.state, "unknown");
    let live_card_b = snap_b.cards.iter().find(|c| c.id == card_b).unwrap();
    assert!(live_card_b.attention.is_some());
    assert!(live_card_b.human_gate.is_some());

    // Retrying `deliver_questions` must not resend; explicit `agent.reply` reconciles cleanly to `replied`.
    app.deliver_questions(&app.herdr_client()).unwrap();
    assert_eq!(herdr.send_calls.load(Ordering::SeqCst), 2);
    let reply_b = app
        .call(
            "agent.reply",
            json!({"question_id": qid_b, "body": "Handled B", "request_key": "continue-fp2-reply"}),
        )
        .unwrap();
    assert_eq!(reply_b["accepted"], true);
    assert_eq!(
        app.snapshot()
            .unwrap()
            .questions
            .iter()
            .find(|q| q.id == qid_b)
            .unwrap()
            .state,
        "replied"
    );

    // Case C: Happy path -> Question commits `delivered` only after `terminal.send` succeeds.
    let (_card_c, tid_c, qid_c, entry_c) =
        setup_retained_with_question("Happy continue", "t-continue-c", "@developer handle case C");
    let snap_before_c = app.snapshot().unwrap();
    app.continue_assignment(&entry_c, &snap_before_c).unwrap();
    assert_eq!(herdr.send_calls.load(Ordering::SeqCst), 3);

    let snap_c = app.snapshot().unwrap();
    let q_c = snap_c.questions.iter().find(|q| q.id == qid_c).unwrap();
    assert_eq!(q_c.state, "delivered");
    let run_c = snap_c
        .terminals
        .iter()
        .find(|t| t.id == tid_c)
        .unwrap()
        .agent
        .as_ref()
        .unwrap();
    assert_eq!(run_c.state, "running");
}
