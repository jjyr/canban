use super::*;
use canban_herdr::{BackendSubscription, HerdrBackend};
use canban_types::{Clock, DeterministicIdGen};
use std::sync::atomic::{AtomicU64, Ordering};

struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    fn new(start_ms: u64) -> (Self, Arc<AtomicU64>) {
        let v = Arc::new(AtomicU64::new(start_ms));
        (Self(Arc::clone(&v)), v)
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct TokioPausedClock {
    base_ms: u64,
    origin: tokio::time::Instant,
}

impl Clock for TokioPausedClock {
    fn now_ms(&self) -> u64 {
        self.base_ms
            .saturating_add(self.origin.elapsed().as_millis() as u64)
    }
}

struct StubHerdrBackend {
    core: String,
    terminals: Mutex<Vec<Value>>,
    offline: AtomicBool,
    _hold_senders: Mutex<Vec<std::sync::mpsc::Sender<Result<Value>>>>,
}

impl StubHerdrBackend {
    fn online(core: &str, terminals: Vec<Value>) -> Self {
        Self {
            core: core.to_owned(),
            terminals: Mutex::new(terminals),
            offline: AtomicBool::new(false),
            _hold_senders: Mutex::new(Vec::new()),
        }
    }

    fn offline() -> Self {
        Self {
            core: String::new(),
            terminals: Mutex::new(Vec::new()),
            offline: AtomicBool::new(true),
            _hold_senders: Mutex::new(Vec::new()),
        }
    }
}

impl HerdrBackend for StubHerdrBackend {
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        if self.offline.load(Ordering::SeqCst) {
            bail!("Herdr Core offline");
        }
        match method {
            "status" => Ok(json!({"core": {"id": self.core}, "version": "sim"})),
            "terminal.list" => Ok(json!({"terminals": self.terminals.lock().unwrap().clone()})),
            "terminal.get" => {
                let addr = params["terminal"].as_str().unwrap_or("");
                self.terminals
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|t| t["address"].as_str() == Some(addr))
                    .cloned()
                    .context("terminal not found")
            }
            "terminal.launch" => {
                let mut list = self.terminals.lock().unwrap();
                let addr = format!("t-launched-{}", list.len() + 1);
                let term = json!({
                    "address": addr,
                    "state": "live",
                    "status": {"state": "working"},
                    "cwd": params["cwd"],
                });
                list.push(term.clone());
                Ok(term)
            }
            _ => Ok(json!({"ok": true})),
        }
    }

    fn subscribe(&self, _wake: Arc<dyn Fn() + Send + Sync>) -> Result<BackendSubscription> {
        if self.offline.load(Ordering::SeqCst) {
            bail!("Herdr Core offline");
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self._hold_senders.lock().unwrap().push(tx);
        Ok(BackendSubscription {
            core: self.core.clone(),
            snapshot: json!({
                "seq": 1,
                "terminals": self.terminals.lock().unwrap().clone(),
            }),
            receiver: rx,
        })
    }

    fn activity(&self, _core: &str, _terminal: &str) -> canban_herdr::activity::Activity {
        Default::default()
    }
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some("tests") {
                continue;
            }
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            if path.file_name().and_then(|n| n.to_str()) == Some("tests.rs") {
                continue;
            }
            out.push(path);
        }
    }
}

/// Strips `#[cfg(test)]` items (modules/functions/impls) and `//` line comments so we scan only
/// compiled production code lines.
fn production_lines(source: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut skipping_test_item = false;
    let mut test_brace_depth: i32 = 0;
    let mut seen_open_brace = false;
    let mut in_block_comment = false;

    for (idx, raw_line) in source.lines().enumerate() {
        let mut line = String::new();
        let bytes = raw_line.as_bytes();
        let mut i = 0;
        let mut in_string = false;
        while i < bytes.len() {
            if in_block_comment {
                if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    in_block_comment = false;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            if !in_string && i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
                break;
            }
            if !in_string && i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                in_block_comment = true;
                i += 2;
                continue;
            }
            if bytes[i] == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
                in_string = !in_string;
            }
            line.push(bytes[i] as char);
            i += 1;
        }
        let trimmed = line.trim();
        if !skipping_test_item && trimmed.starts_with("#[cfg(test)]") {
            skipping_test_item = true;
            test_brace_depth = 0;
            seen_open_brace = false;
            // Handle single-line `#[cfg(test)] mod tests;`
            if trimmed.ends_with(';') {
                skipping_test_item = false;
            }
            continue;
        }
        if skipping_test_item {
            if !seen_open_brace && trimmed.ends_with(';') {
                skipping_test_item = false;
                continue;
            }
            for ch in trimmed.chars() {
                if ch == '{' {
                    seen_open_brace = true;
                    test_brace_depth += 1;
                } else if ch == '}' {
                    test_brace_depth -= 1;
                }
            }
            if seen_open_brace && test_brace_depth <= 0 {
                skipping_test_item = false;
                seen_open_brace = false;
            }
            continue;
        }
        out.push((idx + 1, line));
    }
    out
}

fn contains_bare_now_call(line: &str) -> bool {
    let bytes = line.as_bytes();
    let needle = b"now()";
    let mut start = 0;
    while let Some(pos) = line[start..].find("now()") {
        let abs = start + pos;
        let prev = abs.checked_sub(1).map(|i| bytes[i]);
        let is_bare = match prev {
            None => true,
            Some(b) => !(b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b':'),
        };
        if is_bare {
            return true;
        }
        start = abs + needle.len();
    }
    false
}

#[test]
fn production_sources_have_no_stray_wall_clock_calls() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut files = Vec::new();
    collect_rs_files(&workspace_root.join("crates/app-server/src"), &mut files);
    collect_rs_files(&workspace_root.join("crates/storage/src"), &mut files);
    collect_rs_files(&workspace_root.join("crates/context/src"), &mut files);
    assert!(
        !files.is_empty(),
        "expected production source files under app-server, storage, and context"
    );

    let mut violations = Vec::new();
    for file in files {
        let rel = file
            .strip_prefix(workspace_root)
            .unwrap_or(&file)
            .display()
            .to_string();
        let src = std::fs::read_to_string(&file).unwrap();
        for (line_no, line) in production_lines(&src) {
            if line.contains("canban_types::now") {
                violations.push(format!(
                    "{rel}:{line_no}: canban_types::now -> {}",
                    line.trim()
                ));
            }
            if contains_bare_now_call(&line) {
                violations.push(format!("{rel}:{line_no}: bare now() -> {}", line.trim()));
            }
            if line.contains("unwrap_or_else(now)") {
                violations.push(format!(
                    "{rel}:{line_no}: unwrap_or_else(now) -> {}",
                    line.trim()
                ));
            }
            if line.contains("SystemTime::now") {
                violations.push(format!(
                    "{rel}:{line_no}: SystemTime::now -> {}",
                    line.trim()
                ));
            }
            if line.contains("'localtime'") {
                violations.push(format!(
                    "{rel}:{line_no}: SQLite 'localtime' -> {}",
                    line.trim()
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "Found stray production wall-clock calls:\n{}",
        violations.join("\n")
    );
}

fn git_in(dir: &Path, args: &[&str]) {
    let out = canban_exec::raw_command("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Canban Sim")
        .env("GIT_AUTHOR_EMAIL", "sim@canban.local")
        .env("GIT_COMMITTER_NAME", "Canban Sim")
        .env("GIT_COMMITTER_EMAIL", "sim@canban.local")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn app_server_with_2023_clock_obeys_virtual_time_across_scheduler_workflow_delivery_and_close() {
    const T_2023: u64 = 1_700_000_000_000; // 2023-11-14T22:13:20Z

    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_in(&repo, &["init", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "init\n").unwrap();
    git_in(&repo, &["add", "README.md"]);
    git_in(&repo, &["commit", "-m", "initial"]);

    let (clock, clock_ms) = ManualClock::new(T_2023);
    let backend = Arc::new(StubHerdrBackend::online(
        "core-2023",
        vec![json!({
            "address": "t-working-1",
            "state": "live",
            "status": {"state": "working"},
            "cwd": repo.to_string_lossy(),
        })],
    ));
    let env = ServerRuntimeEnv {
        clock: Arc::new(clock),
        id_gen: DeterministicIdGen::arc(2023),
        herdr_backend: Some(backend.clone()),
        fs_durability: FsDurability::NoSync,
        sqlite_synchronous_off: true,
        disable_os_watchers: true,
        inline_workers: true,
        ..Default::default()
    };
    let app = AppServer::open_with_env(dir.path().join("home-a"), PathBuf::from("/bin/echo"), env)
        .unwrap();

    // 1. Deterministic IDs & virtual 2023 timestamps on open + auth stores.
    let (clock_b, _) = ManualClock::new(T_2023);
    let app_b = AppServer::open_with_env(
        dir.path().join("home-b"),
        PathBuf::from("/bin/echo"),
        ServerRuntimeEnv {
            clock: Arc::new(clock_b),
            id_gen: DeterministicIdGen::arc(2023),
            fs_durability: FsDurability::NoSync,
            sqlite_synchronous_off: true,
            disable_os_watchers: true,
            inline_workers: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(app.server_id(), app_b.server_id());
    assert_eq!(app.state_instance, app_b.state_instance);

    let (dev_rec, _tok) = app.issue_device_token("sim-mac").unwrap();
    assert_eq!(dev_rec.created_at, T_2023);
    let (code_a, expires_at) = app.create_pairing_code(300).unwrap();
    assert_eq!(
        expires_at,
        T_2023 + 300_000,
        "pairing expiry must derive from 2023 clock"
    );
    let (_dev_b, _) = app_b.issue_device_token("sim-mac").unwrap();
    let (code_b, _) = app_b.create_pairing_code(300).unwrap();
    assert_eq!(
        code_a, code_b,
        "pairing codes must be deterministic under injected IdGen"
    );

    // Configure max_agents = 1 and a minimal workflow with develop -> review -> deliver(sync_upstream:false) -> done.
    app.config.write().unwrap().scheduler.max_agents = 1;
    let workflow_yaml = format!(
        r#"schema: {}
id: development
name: Development
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
  - id: done
    name: Done
on:
  enter_develop:
    event: card.column_entered
    match:
      column: develop
    start_at: work
steps:
  - id: work
    uses: agent
    with:
      profile: developer
    next: to_review
  - id: to_review
    uses: card.move
    with:
      column: review
    next: ship
  - id: ship
    uses: deliver
    with:
      sync_upstream: false
      batch_size: 1
    next: to_done
  - id: to_done
    uses: card.move
    with:
      column: done
    next: finish
  - id: finish
    uses: complete
"#,
        canban_workflow_engine::migrations::CURRENT
    );

    let project: Project = serde_json::from_value(
        app.call(
            "project.create",
            json!({
                "name": "ClockProj",
                "directory": repo.to_string_lossy(),
                "workflow_yaml": workflow_yaml,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(project.created_at, T_2023);

    let card1: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": project.id, "title": "Card 1"}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(card1.created_at, T_2023);

    let card2: Card = serde_json::from_value(
        app.call(
            "card.create",
            json!({"view": "full", "project_id": project.id, "title": "Card 2"}),
        )
        .unwrap(),
    )
    .unwrap();

    // 2. Workflow `updated_at` and scheduler slot release under 2023 virtual clock.
    app.call(
        "card.move",
        json!({"card_id": card1.id, "revision": card1.revision, "stage": "develop"}),
    )
    .unwrap();
    app.call(
        "card.move",
        json!({"card_id": card2.id, "revision": card2.revision, "stage": "develop"}),
    )
    .unwrap();

    let snap = app.snapshot().unwrap();
    let run1 = snap
        .workflow_runs
        .iter()
        .find(|r| r.card_id.as_deref() == Some(card1.id.as_str()))
        .expect("workflow run for card 1");
    assert_eq!(run1.created_at, T_2023);
    assert_eq!(run1.updated_at, T_2023);

    let term1 = snap
        .terminals
        .iter()
        .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == card1.id))
        .unwrap()
        .clone();
    let term2 = snap
        .terminals
        .iter()
        .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == card2.id))
        .unwrap()
        .clone();
    assert_eq!(term1.created_at, T_2023);

    // Bind term1 as live & working at T_2023.
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut t: TerminalEntry = db.get(&term1.id)?;
            let a = t.agent.as_mut().unwrap();
            a.state = "running".into();
            a.number = 1;
            a.core_id = Some("core-2023".into());
            a.terminal = Some("t-working-1".into());
            a.connection_state = "connected".into();
            a.started_at = Some(T_2023);
            a.observed = Some(json!({"state": "live", "status": {"state": "working"}}));
            a.lifecycle.observed_at = Some(T_2023);
            db.save(&t)
        })
        .unwrap();

    let snap = app.snapshot().unwrap();
    assert_eq!(canban_scheduler::active_count_at(&snap, T_2023), 1);
    assert!(canban_scheduler::next_at(&snap, 1, T_2023).is_none());

    // Advance virtual clock by 5s and observe term1 becoming idle in 2023.
    let t_idle = T_2023 + 5_000;
    clock_ms.store(t_idle, Ordering::SeqCst);
    *backend.terminals.lock().unwrap() = vec![json!({
        "address": "t-working-1",
        "state": "live",
        "status": {"state": "idle"},
        "cwd": repo.to_string_lossy(),
    })];
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut t: TerminalEntry = db.get(&term1.id)?;
            let a = t.agent.as_mut().unwrap();
            a.observed = Some(json!({"state": "live", "status": {"state": "idle"}}));
            a.lifecycle.observed_at = Some(t_idle);
            a.lifecycle.idle_since = Some(t_idle);
            db.save(&t)
        })
        .unwrap();

    // Slot must release immediately under 2023 clock (would fail if compared against 2026 wall time).
    let snap = app.snapshot().unwrap();
    assert_eq!(canban_scheduler::active_count_at(&snap, t_idle), 0);
    assert_eq!(
        canban_scheduler::next_at(&snap, 1, t_idle).map(|e| e.id),
        Some(term2.id.clone())
    );

    // 3. Delivery settle timer (`DELIVERY_AGENT_SETTLE_MS` = 180s) obeys 2023 virtual clock.
    // Prepare a real git worktree + branch commit for Card 1 so Queue Merge succeeds once settled.
    let wt1 = dir.path().join("wt-card1");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "feat/card1",
            wt1.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(wt1.join("change.txt"), "hello 2023\n").unwrap();
    git_in(&wt1, &["add", "change.txt"]);
    git_in(&wt1, &["commit", "-m", "feat: card 1"]);
    let base_head = String::from_utf8(
        canban_exec::raw_command("git")
            .args(["rev-parse", "main"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();

    let t_report = T_2023 + 10_000;
    clock_ms.store(t_report, Ordering::SeqCst);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            let mut c: Card = db.get(&card1.id)?;
            c.worktree = Some(wt1.to_string_lossy().into_owned());
            c.branch = Some("feat/card1".into());
            c.base_commit = Some(base_head.clone());
            c.workspace = Some(WorkspaceBinding {
                directory: repo.to_string_lossy().into_owned(),
                path: wt1.to_string_lossy().into_owned(),
                branch: Some("feat/card1".into()),
                base_commit: Some(base_head.clone()),
                target_branch: Some("main".into()),
            });
            db.save(&c)?;
            let mut t: TerminalEntry = db.get(&term1.id)?;
            let a = t.agent.as_mut().unwrap();
            // Keep terminal connected & working while reporting completed to trigger settle window.
            a.observed = Some(json!({"state": "live", "status": {"state": "working"}}));
            a.lifecycle.observed_at = Some(t_report);
            db.save(&t)
        })
        .unwrap();

    app.call(
        "execution.report",
        json!({
            "project_id": project.id,
            "card": card1.number,
            "state": "completed",
            "message": "ready to merge",
            "request_key": "rep-2023-1",
        }),
    )
    .unwrap();

    let snap = app.snapshot().unwrap();
    let run1 = snap.workflow_runs.iter().find(|r| r.id == run1.id).unwrap();
    assert_eq!(run1.state, "waiting_delivery");
    assert_eq!(run1.updated_at, t_report);

    // At +179s (< 180s settle grace), pump_deliveries MUST wait.
    clock_ms.store(t_report + 179_000, Ordering::SeqCst);
    assert!(
        !app.pump_deliveries().unwrap(),
        "delivery must wait while agent is still within 180s settle window"
    );
    let run1_waiting = app
        .snapshot()
        .unwrap()
        .workflow_runs
        .into_iter()
        .find(|r| r.id == run1.id)
        .unwrap();
    assert_eq!(run1_waiting.state, "waiting_delivery");

    // At +181s (> 180s settle grace), pump_deliveries MUST proceed and record 2023 updated_at.
    let t_settled = t_report + 181_000;
    clock_ms.store(t_settled, Ordering::SeqCst);
    assert!(
        app.pump_deliveries().unwrap(),
        "delivery must proceed once 180s virtual settle window expires"
    );
    let snap = app.snapshot().unwrap();
    let run1_done = snap.workflow_runs.iter().find(|r| r.id == run1.id).unwrap();
    assert_eq!(run1_done.state, "completed");
    assert_eq!(run1_done.updated_at, t_settled);
    assert_eq!(
        snap.cards.iter().find(|c| c.id == card1.id).unwrap().stage,
        "done"
    );

    // 4. 30s `close_requested_at` timeout in `AppServer::tick()` obeys 2023 virtual clock.
    let t_close = t_settled + 5_000;
    clock_ms.store(t_close, Ordering::SeqCst);
    app.store
        .lock()
        .unwrap()
        .write(|db| {
            // Mark term2 cancelled so tick() doesn't try to launch term2 during the close-timeout check.
            let mut t2: TerminalEntry = db.get(&term2.id)?;
            t2.agent.as_mut().unwrap().state = "cancelled".into();
            db.save(&t2)?;
            let mut t1: TerminalEntry = db.get(&term1.id)?;
            let a = t1.agent.as_mut().unwrap();
            a.close_state = "closing".into();
            a.close_requested_at = Some(t_close);
            a.lifecycle.close_reason = "completed".into();
            db.save(&t1)
        })
        .unwrap();

    // At +29s (<= 30s), close_state must still be "closing".
    clock_ms.store(t_close + 29_000, Ordering::SeqCst);
    app.tick().unwrap();
    let t1_mid = app
        .snapshot()
        .unwrap()
        .terminals
        .into_iter()
        .find(|t| t.id == term1.id)
        .unwrap();
    assert_eq!(
        t1_mid.agent.as_ref().unwrap().close_state,
        "closing",
        "close_requested_at must not time out at +29s"
    );

    // At +31s (> 30s), close_state must transition to "error" and record a 2023 gate timestamp.
    let t_timeout = t_close + 31_000;
    clock_ms.store(t_timeout, Ordering::SeqCst);
    app.tick().unwrap();
    let snap = app.snapshot().unwrap();
    let t1_err = snap.terminals.iter().find(|t| t.id == term1.id).unwrap();
    assert_eq!(t1_err.agent.as_ref().unwrap().close_state, "error");
    let c1_after = snap.cards.iter().find(|c| c.id == card1.id).unwrap();
    assert!(
        c1_after.human_gate.is_some(),
        "close timeout must raise a human gate"
    );
    assert!(
        snap.notices
            .iter()
            .any(|n| n.card_id == card1.id && n.created_at == t_timeout),
        "attention notice created by close timeout must use 2023 virtual time ({t_timeout}), notices={:?}",
        snap.notices
    );
}

#[test]
fn runtime_and_delivery_workers_park_cleanly_without_livelock() {
    // Scenario A: max_agents = 1, 1 live working terminal + 1 queued terminal.
    // Scenario B: Herdr offline + 1 queued terminal.
    // Neither scenario may cause `runtime_worker_loop <-> delivery_worker_loop` ping-pong spin.
    for herdr_offline in [false, true] {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap();

        rt.block_on(async move {
            let dir = tempfile::tempdir().unwrap();
            let repo = dir.path().join("repo");
            std::fs::create_dir_all(&repo).unwrap();

            let backend: Arc<dyn HerdrBackend> = if herdr_offline {
                Arc::new(StubHerdrBackend::offline())
            } else {
                Arc::new(StubHerdrBackend::online(
                    "core-live",
                    vec![json!({
                        "address": "t-busy",
                        "state": "live",
                        "status": {"state": "working"},
                        "cwd": repo.to_string_lossy(),
                    })],
                ))
            };
            let clock = Arc::new(TokioPausedClock {
                base_ms: 1_700_000_000_000,
                origin: tokio::time::Instant::now(),
            });
            let app = AppServer::open_with_env(
                dir.path().join("home"),
                PathBuf::from("/bin/echo"),
                ServerRuntimeEnv {
                    clock,
                    id_gen: DeterministicIdGen::arc(99),
                    herdr_backend: Some(backend),
                    fs_durability: FsDurability::NoSync,
                    sqlite_synchronous_off: true,
                    disable_os_watchers: true,
                    inline_workers: true,
                    ..Default::default()
                },
            )
            .unwrap();
            app.config.write().unwrap().scheduler.max_agents = 1;

            let project: Project = serde_json::from_value(
                app.call(
                    "project.create",
                    json!({"name": "SpinTest", "directory": repo.to_string_lossy()}),
                )
                .unwrap(),
            )
            .unwrap();
            let c1: Card = serde_json::from_value(
                app.call(
                    "card.create",
                    json!({"view": "full", "project_id": project.id, "title": "Busy"}),
                )
                .unwrap(),
            )
            .unwrap();
            app.call(
                "card.move",
                json!({"card_id": c1.id, "revision": c1.revision, "stage": "develop"}),
            )
            .unwrap();

            if !herdr_offline {
                let c2: Card = serde_json::from_value(
                    app.call(
                        "card.create",
                        json!({"view": "full", "project_id": project.id, "title": "Queued"}),
                    )
                    .unwrap(),
                )
                .unwrap();
                app.call(
                    "card.move",
                    json!({"card_id": c2.id, "revision": c2.revision, "stage": "develop"}),
                )
                .unwrap();

                let term1 = app
                    .snapshot()
                    .unwrap()
                    .terminals
                    .into_iter()
                    .find(|t| t.agent.as_ref().is_some_and(|a| a.card_id == c1.id))
                    .unwrap();
                let now_ms = app.now_ms();
                app.store
                    .lock()
                    .unwrap()
                    .write(|db| {
                        let mut t: TerminalEntry = db.get(&term1.id)?;
                        let a = t.agent.as_mut().unwrap();
                        a.state = "running".into();
                        a.number = 1;
                        a.core_id = Some("core-live".into());
                        a.terminal = Some("t-busy".into());
                        a.connection_state = "connected".into();
                        a.started_at = Some(now_ms);
                        a.observed = Some(json!({"state": "live", "status": {"state": "working"}}));
                        a.lifecycle.observed_at = Some(now_ms);
                        db.save(&t)
                    })
                    .unwrap();
            }

            let running = app
                .start_in_runtime(tokio::runtime::Handle::current())
                .unwrap();

            // Let initial startup ticks settle, then advance virtual time by 500ms and 3s.
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            let ticks_after_start = app.runtime_worker_ticks.load(Ordering::SeqCst);
            let delivery_after_start = app.delivery_worker_passes.load(Ordering::SeqCst);
            assert!(
                ticks_after_start <= 6 && delivery_after_start <= 6,
                "startup should settle cleanly (herdr_offline={herdr_offline}): runtime_ticks={ticks_after_start}, delivery_passes={delivery_after_start}"
            );

            tokio::time::advance(Duration::from_millis(500)).await;
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            let ticks_500 = app.runtime_worker_ticks.load(Ordering::SeqCst);
            let delivery_500 = app.delivery_worker_passes.load(Ordering::SeqCst);
            assert!(
                ticks_500 <= ticks_after_start + 2,
                "runtime_worker_loop must sleep cleanly over 500ms (herdr_offline={herdr_offline}): before={ticks_after_start}, after={ticks_500}"
            );
            assert_eq!(
                delivery_500, delivery_after_start,
                "delivery_worker_loop must stay parked over 500ms (herdr_offline={herdr_offline})"
            );

            // Advance another 3 seconds: even when Herdr is offline (1s -> 2s exponential backoff),
            // runtime ticks stay strictly bounded and delivery_worker_loop never gets ping-ponged.
            tokio::time::advance(Duration::from_secs(3)).await;
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            let ticks_3500 = app.runtime_worker_ticks.load(Ordering::SeqCst);
            let delivery_3500 = app.delivery_worker_passes.load(Ordering::SeqCst);
            assert!(
                ticks_3500 <= ticks_after_start + 6,
                "runtime_worker_loop must not spin over 3.5s (herdr_offline={herdr_offline}): start={ticks_after_start}, after={ticks_3500}"
            );
            assert_eq!(
                delivery_3500, delivery_after_start,
                "delivery_worker_loop must not be woken on idle/no-progress runtime retries (herdr_offline={herdr_offline})"
            );

            running.set_abrupt_crash(true);
            drop(running);
        });
    }
}
