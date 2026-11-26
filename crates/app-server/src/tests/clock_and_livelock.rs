use super::*;
use canban_herdr::{BackendSubscription, HerdrBackend};
use canban_types::{Clock, DeterministicIdGen};
use std::sync::atomic::Ordering;

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
    let scanned_dirs = [
        "crates/app-server/src",
        "crates/storage/src",
        "crates/context/src",
        "crates/client/src",
        "crates/sync/src",
    ];
    let mut files = Vec::new();
    for rel_dir in scanned_dirs {
        let before = files.len();
        collect_rs_files(&workspace_root.join(rel_dir), &mut files);
        assert!(
            files.len() > before,
            "expected production source files under {rel_dir}"
        );
    }

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
