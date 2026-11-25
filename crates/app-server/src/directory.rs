//! Bounded, coalesced context discovery. Git IO never holds the product DB lock.
use super::*;
use canban_workspaces::context::WatchTarget;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Component,
};

const STATUS_DEBOUNCE_MS: u64 = 300;
const STATUS_MIN_INTERVAL_MS: u64 = 2_000;

/// Pure trailing-edge coalescer for Git status invalidations.
///
/// Debounces rapid bursts by [`STATUS_DEBOUNCE_MS`] without starving continuous writes
/// or dropping trailing updates inside the [`STATUS_MIN_INTERVAL_MS`] cooldown window.
#[derive(Clone, Debug, Default)]
struct StatusInvalidationCoalescer {
    last_flush: Option<u64>,
    pending_since: Option<u64>,
    due_at: Option<u64>,
}

impl StatusInvalidationCoalescer {
    fn record(&mut self, now_ms: u64) -> u64 {
        let first = *self.pending_since.get_or_insert(now_ms);
        let quiet_due = now_ms.saturating_add(STATUS_DEBOUNCE_MS);
        let max_due = first.saturating_add(STATUS_MIN_INTERVAL_MS);
        let rate_due = self
            .last_flush
            .map(|last| last.saturating_add(STATUS_MIN_INTERVAL_MS))
            .unwrap_or(now_ms);
        let due = quiet_due.min(max_due).max(rate_due);
        self.due_at = Some(due);
        due
    }

    fn record_immediate_flush(&mut self, now_ms: u64) {
        self.last_flush = Some(now_ms);
        self.pending_since = None;
        self.due_at = None;
    }

    fn next_deadline(&self) -> Option<u64> {
        self.due_at
    }

    fn poll_flush(&mut self, now_ms: u64) -> bool {
        if self.due_at.is_some_and(|due| now_ms >= due) {
            self.last_flush = Some(now_ms);
            self.pending_since = None;
            self.due_at = None;
            true
        } else {
            false
        }
    }

    fn reset(&mut self) {
        self.last_flush = None;
        self.pending_since = None;
        self.due_at = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InvalidationKind {
    Structural,
    Immediate,
    Coalesced,
}

impl InvalidationKind {
    fn upgrade(current: Option<Self>, next: Self) -> Option<Self> {
        match (current, next) {
            (Some(Self::Structural), _) | (_, Self::Structural) => Some(Self::Structural),
            (Some(Self::Immediate), _) | (_, Self::Immediate) => Some(Self::Immediate),
            (_, Self::Coalesced) => Some(Self::Coalesced),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct WatcherClassification {
    kind: Option<InvalidationKind>,
    new_watches: Vec<WatchTarget>,
}

fn is_ref_path(path: &Path) -> bool {
    let normals: Vec<&std::ffi::OsStr> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    if normals.iter().any(|name| *name == "reftable") {
        return true;
    }
    normals
        .windows(2)
        .any(|pair| pair[0] == "refs" && (pair[1] == "heads" || pair[1] == "remotes"))
}

fn classify_watcher_paths(paths: &[PathBuf]) -> WatcherClassification {
    let mut out = WatcherClassification::default();
    for p in paths {
        let name = p.file_name();
        if name.is_some_and(|n| n == ".git") {
            out.kind = InvalidationKind::upgrade(out.kind, InvalidationKind::Structural);
            if p.is_dir() {
                out.new_watches.push(WatchTarget {
                    path: p.clone(),
                    recursive: false,
                });
                let refs_dir = p.join("refs");
                if refs_dir.is_dir() {
                    out.new_watches.push(WatchTarget {
                        path: refs_dir.clone(),
                        recursive: false,
                    });
                    for sub in ["heads", "remotes"] {
                        let sub_dir = refs_dir.join(sub);
                        if sub_dir.is_dir() {
                            out.new_watches.push(WatchTarget {
                                path: sub_dir,
                                recursive: true,
                            });
                        }
                    }
                }
                let reftable_dir = p.join("reftable");
                if reftable_dir.is_dir() {
                    out.new_watches.push(WatchTarget {
                        path: reftable_dir,
                        recursive: false,
                    });
                }
            }
            continue;
        }
        if name.is_some_and(|n| n == "HEAD" || n == "HEAD.lock") {
            out.kind = InvalidationKind::upgrade(out.kind, InvalidationKind::Immediate);
            continue;
        }
        if is_ref_path(p)
            || name.is_some_and(|n| {
                n == "index"
                    || n == "index.lock"
                    || n == "FETCH_HEAD"
                    || n == "ORIG_HEAD"
                    || n == "packed-refs"
                    || n == "packed-refs.lock"
            })
        {
            out.kind = InvalidationKind::upgrade(out.kind, InvalidationKind::Coalesced);
        }
        if p.parent()
            .and_then(|par| par.file_name())
            .is_some_and(|n| n == "refs")
            && name.is_some_and(|n| n == "heads" || n == "remotes")
            && p.is_dir()
        {
            out.new_watches.push(WatchTarget {
                path: p.clone(),
                recursive: true,
            });
        } else if name.is_some_and(|n| n == "reftable") && p.is_dir() {
            out.new_watches.push(WatchTarget {
                path: p.clone(),
                recursive: false,
            });
        }
    }
    out
}

#[derive(Clone)]
struct Job {
    snapshot: Arc<Snapshot>,
    context: TerminalContext,
    signature: String,
    generation: u64,
}
type Key = String;
struct Cached {
    context: TerminalContext,
    signature: String,
    at: u64,
}
#[derive(Default)]
pub(super) struct ContextQueue {
    generation: u64,
    targets: BTreeMap<String, Job>,
    pending: BTreeMap<Key, Job>,
    running: bool,
    results: BTreeMap<Key, Cached>,
    cache: Arc<Mutex<canban_workspaces::context::ContextCache>>,
    mapping: Option<(String, Arc<Snapshot>)>,
    watcher: Option<RecommendedWatcher>,
    watched: BTreeSet<WatchTarget>,
    pending_watches: BTreeSet<WatchTarget>,
    coalescer: StatusInvalidationCoalescer,
}
impl ContextQueue {
    pub fn retain_targets(&mut self, wanted: &BTreeSet<String>) {
        self.targets.retain(|id, _| wanted.contains(id));
        let keys = self
            .targets
            .values()
            .filter_map(|t| t.context.cwd.clone())
            .collect::<BTreeSet<_>>();
        self.pending.retain(|key, _| keys.contains(key));
    }
    pub fn probes(&self) -> u64 {
        self.cache.lock().unwrap().probes
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len() + usize::from(self.running)
    }
    #[cfg(test)]
    pub(crate) fn has_watcher(&self) -> bool {
        self.watcher.is_some()
    }
    pub fn reset(&mut self) {
        self.generation += 1;
        self.targets.clear();
        self.pending.clear();
        self.results.clear();
        self.cache = Default::default();
        self.mapping = None;
        self.pending_watches.clear();
        self.coalescer.reset();
    }
}
fn signature(s: &Snapshot) -> String {
    let projects = s
        .projects
        .iter()
        .map(|p| json!([p.id, p.directory]))
        .collect::<Vec<_>>();
    let cards = s
        .cards
        .iter()
        .map(|c| {
            json!([
                c.id,
                c.project_id,
                c.archived_at,
                c.worktree,
                c.branch,
                c.workspace,
                c.workspace_history
            ])
        })
        .collect::<Vec<_>>();
    let worktrees = s
        .worktrees
        .iter()
        .map(|wt| json!([wt.id, wt.project_id, wt.path, wt.branch, wt.directory]))
        .collect::<Vec<_>>();
    canban_context::digest(&serde_json::to_vec(&json!([projects, cards, worktrees])).unwrap())
}
pub(super) struct CheckPermit<'a>(&'a AppServer);
impl Drop for CheckPermit<'_> {
    fn drop(&mut self) {
        *self.0.check_slots.lock().unwrap() = false;
        self.0.check_signal.notify_all();
    }
}
impl AppServer {
    pub(super) fn check(&self) -> CheckPermit<'_> {
        let mut busy = self.check_slots.lock().unwrap();
        while *busy {
            busy = self.check_signal.wait(busy).unwrap();
        }
        *busy = true;
        CheckPermit(self)
    }
    pub(super) fn browse_directory(&self, p: Value) -> Result<Value> {
        let host = canban_herdr::host::Host::with_runner(self.config(), self.exec_runner.clone());
        let raw = p["path"].as_str().map(str::trim).filter(|v| !v.is_empty());
        let path = match raw {
            Some("~") => host.home()?,
            Some(rest) if rest.starts_with("~/") => {
                let home = host.home()?;
                if home == "/" {
                    format!("/{}", &rest[2..])
                } else {
                    format!("{home}/{}", &rest[2..])
                }
            }
            Some(other) => other.to_owned(),
            None => host.home()?,
        };
        ensure!(
            Path::new(&path).is_absolute(),
            "An absolute directory is required"
        );
        let home = host.home().unwrap_or_default();
        let target = Path::new(&path);
        if !target.is_dir() {
            return Ok(json!({
                "path": path,
                "parent": target.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into()),
                "home": home,
                "exists": false,
                "directories": [],
            }));
        }
        let real = target
            .canonicalize()
            .unwrap_or_else(|_| target.to_path_buf());
        let mut directories = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&real) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.is_dir()
                    && let Some(name) = entry.file_name().to_str()
                {
                    directories.push((name.to_owned(), entry_path.to_string_lossy().into_owned()));
                }
            }
        }
        directories.sort_by(|a, b| a.0.cmp(&b.0));
        let parent = real
            .parent()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".into());
        Ok(json!({
            "path": real.to_string_lossy(),
            "parent": parent,
            "home": home,
            "exists": true,
            "directories": directories
                .into_iter()
                .map(|(name, p)| json!({"name": name, "path": p}))
                .collect::<Vec<_>>(),
        }))
    }
    pub(super) fn queue_context(
        &self,
        s: &Snapshot,
        id: &str,
        core: &str,
        terminal: &str,
        value: &Value,
    ) {
        let Some(cwd) = canban_herdr::cwd(value) else {
            return;
        };
        let key = cwd.to_owned();
        let mut q = self.contexts.lock().unwrap();
        let digest = signature(s);
        if q.mapping.as_ref().is_none_or(|(old, _)| old != &digest) {
            // Share only association inputs. Never retain a full Timeline/report
            // snapshot once per Terminal in the background discovery queue.
            let mapping = Snapshot {
                projects: s
                    .projects
                    .iter()
                    .cloned()
                    .map(|mut p| {
                        p.workflow = Workflow::default();
                        p.github = None;
                        p
                    })
                    .collect(),
                cards: s
                    .cards
                    .iter()
                    .cloned()
                    .map(|mut c| {
                        c.body = String::new();
                        c.title = String::new();
                        c.context_notes = None;
                        c.content = Default::default();
                        c
                    })
                    .collect(),
                worktrees: s.worktrees.clone(),
                ..Default::default()
            };
            q.mapping = Some((digest.clone(), Arc::new(mapping)));
        }
        let job = Job {
            snapshot: q.mapping.as_ref().unwrap().1.clone(),
            signature: digest,
            generation: q.generation,
            context: TerminalContext {
                terminal_id: id.into(),
                core_id: core.into(),
                terminal: terminal.into(),
                cwd: Some(cwd.into()),
                directory_root: None,
                project_id: None,
                card_id: None,
                worktree_id: None,
                branch: None,
                dirty: false,
                additions: 0,
                deletions: 0,
                ahead: 0,
                behind: 0,
            },
        };
        let cached = q
            .results
            .get(&key)
            .filter(|r| r.signature == job.signature)
            .map(|r| r.context.clone());
        let fresh = cached.is_some();
        q.targets.insert(id.into(), job.clone());
        if let Some(mut context) = cached {
            context.terminal_id = id.into();
            context.core_id = core.into();
            context.terminal = terminal.into();
            let mut contexts = self.terminal_contexts.lock().unwrap();
            if !contexts.iter().any(|c| c == &context) {
                contexts.retain(|c| c.terminal_id != id);
                contexts.push(context);
                contexts.sort_by(|a, b| a.terminal_id.cmp(&b.terminal_id));
            }
        }
        if !fresh {
            q.pending.insert(key, job);
            self.context_notify.notify_one();
        }
    }
    pub(super) fn cached_context(
        &self,
        id: &str,
        core: &str,
        terminal: &str,
        cwd: Option<&str>,
    ) -> Option<TerminalContext> {
        self.terminal_contexts
            .lock()
            .unwrap()
            .iter()
            .find(|c| {
                c.terminal_id == id
                    && c.core_id == core
                    && c.terminal == terminal
                    && c.cwd.as_deref() == cwd
            })
            .cloned()
    }
    pub(super) fn start_context_workers(self: &Arc<Self>) {
        let watcher = if self.disable_os_watchers {
            None
        } else {
            let weak = Arc::downgrade(self);
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let cls = match result {
                    Ok(e) if !e.kind.is_access() => classify_watcher_paths(&e.paths),
                    Ok(_) => return,
                    Err(_) => WatcherClassification {
                        kind: Some(InvalidationKind::Structural),
                        new_watches: Vec::new(),
                    },
                };
                if cls.kind.is_none() && cls.new_watches.is_empty() {
                    return;
                }
                if let Some(server) = weak.upgrade() {
                    let had_new_watches = !cls.new_watches.is_empty();
                    let mut wake_runtime_now = false;
                    let mut wake_worker = false;
                    {
                        let mut q = server.contexts.lock().unwrap();
                        q.pending_watches.extend(cls.new_watches);
                        let now_ms = server.now_ms();
                        match cls.kind {
                            Some(InvalidationKind::Structural) => {
                                q.cache = Default::default();
                                q.results.clear();
                                q.generation += 1;
                                q.coalescer.record_immediate_flush(now_ms);
                                wake_runtime_now = true;
                            }
                            Some(InvalidationKind::Immediate) => {
                                q.cache.lock().unwrap().invalidate_branches();
                                q.results.clear();
                                q.generation += 1;
                                q.coalescer.record_immediate_flush(now_ms);
                                wake_runtime_now = true;
                            }
                            Some(InvalidationKind::Coalesced) => {
                                q.coalescer.record(now_ms);
                                wake_worker = true;
                            }
                            None => {}
                        }
                        if had_new_watches {
                            wake_worker = true;
                        }
                    }
                    if wake_runtime_now {
                        server.wake_runtime();
                    }
                    if wake_worker {
                        server.context_notify.notify_one();
                    }
                }
            })
            .ok()
        };
        self.contexts.lock().unwrap().watcher = watcher;
        let weak = Arc::downgrade(self);
        let notify = self.context_notify.clone();
        let stop_rx = self.stop_receiver();
        self.spawn_bg(context_worker_loop(weak, notify, stop_rx));
    }
    pub(super) fn git_changed(&self) {
        let mut q = self.contexts.lock().unwrap();
        q.generation += 1;
        q.results.clear();
        q.cache.lock().unwrap().invalidate_branches();
        q.coalescer.record_immediate_flush(self.now_ms());
        drop(q);
        self.wake_runtime();
    }
    pub(super) fn invalidate_contexts(&self) {
        let mut q = self.contexts.lock().unwrap();
        q.generation += 1;
        q.results.clear();
        q.cache = Default::default();
        q.coalescer.record_immediate_flush(self.now_ms());
        drop(q);
        self.wake_runtime();
    }
    pub(super) fn refresh_context(
        &self,
        s: &Snapshot,
        id: &str,
        core: &str,
        terminal: &str,
        value: &Value,
    ) -> Result<TerminalContext> {
        let cwd = canban_herdr::cwd(value).context("Current directory unavailable")?;
        let key: Key = cwd.into();
        let ttl_ms = 15_000u64;
        let now_ms = self.now_ms();
        let mut q = self.contexts.lock().unwrap();
        if let Some(r) = q.results.get(&key)
            && r.signature == signature(s)
            && now_ms.saturating_sub(r.at) < ttl_ms
        {
            let mut context = r.context.clone();
            context.terminal_id = id.into();
            context.core_id = core.into();
            context.terminal = terminal.into();
            return Ok(context);
        }
        q.results.remove(&key);
        q.cache.lock().unwrap().invalidate_branches();
        drop(q);
        self.queue_context(s, id, core, terminal, value);
        self.cached_context(id, core, terminal, Some(cwd))
            .context("Directory context is updating")
    }
}

enum WorkerStep {
    More,
    Wait(Option<Duration>),
}

fn register_watch_target(q: &mut ContextQueue, target: WatchTarget) {
    if !q.watched.insert(target.clone()) {
        return;
    }
    if let Some(w) = &mut q.watcher {
        let mode = if target.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        if let Err(e) = w.watch(&target.path, mode) {
            eprintln!("Canban context watcher {}: {e}", target.path.display());
        }
    }
}

async fn context_worker_loop(
    weak: std::sync::Weak<AppServer>,
    notify: Arc<tokio::sync::Notify>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) {
    while !*stop_rx.borrow() {
        let step_weak = weak.clone();
        let inline = weak.upgrade().is_some_and(|s| s.inline_workers);
        let work = move || {
            let server = step_weak.upgrade()?;
            if server.stopped() {
                return None;
            }
            let mut wake_runtime = false;
            let next = {
                let mut q = server.contexts.lock().unwrap();
                let pending_watches = std::mem::take(&mut q.pending_watches);
                for target in pending_watches {
                    register_watch_target(&mut q, target);
                }
                let now_ms = server.now_ms();
                if q.coalescer.poll_flush(now_ms) {
                    q.cache.lock().unwrap().invalidate_branches();
                    q.results.clear();
                    q.generation += 1;
                    wake_runtime = true;
                }
                if !q.running
                    && let Some(key) = q.pending.keys().next().cloned()
                {
                    let job = q.pending.remove(&key).unwrap();
                    q.running = true;
                    let cache = q.cache.clone();
                    Ok((key, job, cache))
                } else {
                    let wait = q
                        .coalescer
                        .next_deadline()
                        .map(|due| Duration::from_millis(due.saturating_sub(now_ms)));
                    Err(wait)
                }
            };
            let (key, job, cache) = match next {
                Ok(work) => work,
                Err(wait) => {
                    if wake_runtime {
                        server.wake_runtime();
                    }
                    return Some(if wait == Some(Duration::ZERO) {
                        WorkerStep::More
                    } else {
                        WorkerStep::Wait(wait)
                    });
                }
            };
            let permit = server.check();
            let result = server.ops().directory_context_full_cached(
                &job.snapshot,
                &key,
                &mut cache.lock().unwrap(),
            );
            drop(permit);
            let mut q = server.contexts.lock().unwrap();
            q.running = false;
            let mut changed = false;
            if job.generation == q.generation
                && let Ok(resolved) = result
            {
                let mut context = job.context.clone();
                context.directory_root = resolved.directory_root;
                context.project_id = resolved.project_id;
                context.card_id = resolved.card_id;
                context.worktree_id = resolved.worktree_id;
                context.branch = resolved.branch;
                context.dirty = resolved.dirty;
                context.additions = resolved.additions;
                context.deletions = resolved.deletions;
                context.ahead = resolved.ahead;
                context.behind = resolved.behind;
                changed = true;
                if q.pending
                    .get(&key)
                    .is_some_and(|p| p.generation == job.generation && p.signature == job.signature)
                {
                    q.pending.remove(&key);
                }
                q.results.insert(
                    key.clone(),
                    Cached {
                        context: context.clone(),
                        signature: job.signature.clone(),
                        at: server.now_ms(),
                    },
                );
                let targets = q
                    .targets
                    .values()
                    .filter(|t| {
                        t.context.cwd.as_ref() == Some(&key)
                            && t.signature == job.signature
                            && t.generation == job.generation
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                for target in targets {
                    let mut context = context.clone();
                    context.terminal_id = target.context.terminal_id;
                    context.core_id = target.context.core_id;
                    context.terminal = target.context.terminal;
                    server.upsert_terminal_context(context);
                }
                let watch_targets = cache.lock().unwrap().watch_targets();
                for target in watch_targets {
                    register_watch_target(&mut q, target);
                }
            }
            let more = !q.pending.is_empty();
            let now_ms = server.now_ms();
            let wait = q
                .coalescer
                .next_deadline()
                .map(|due| Duration::from_millis(due.saturating_sub(now_ms)));
            drop(q);
            if wake_runtime {
                server.wake_runtime();
            }
            if changed {
                server.emit("terminals", None);
            }
            Some(if more || wait == Some(Duration::ZERO) {
                WorkerStep::More
            } else {
                WorkerStep::Wait(wait)
            })
        };
        let worked = if inline {
            let res = work();
            tokio::task::yield_now().await;
            Ok(res)
        } else {
            tokio::task::spawn_blocking(work).await
        };
        match worked {
            Ok(Some(WorkerStep::More)) => continue,
            Ok(Some(WorkerStep::Wait(timeout))) => {
                if !AppServer::wait_or_stop(&notify, &mut stop_rx, timeout).await {
                    break;
                }
            }
            _ => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    #[test]
    fn check_serializes_concurrent_checks() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        let (tx, rx) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let wait = Arc::new(Mutex::new(wait));
        let mut joins = vec![];
        for i in 0..3 {
            let app = app.clone();
            let tx = tx.clone();
            let wait = wait.clone();
            joins.push(std::thread::spawn(move || {
                let _permit = app.check();
                tx.send(i).unwrap();
                wait.lock().unwrap().recv().unwrap();
            }));
        }
        let _first = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        for _ in 0..3 {
            release.send(()).unwrap();
        }
        for j in joins {
            j.join().unwrap();
        }
        assert!(!*app.check_slots.lock().unwrap());
    }

    #[test]
    fn coalescer_debounces_bursts_preserves_trailing_event_and_prevents_starvation() {
        let t0: u64 = 1_000_000;
        let mut c = StatusInvalidationCoalescer::default();

        // Single event flushes after STATUS_DEBOUNCE_MS (300ms).
        assert_eq!(c.record(t0), t0 + STATUS_DEBOUNCE_MS);
        assert!(!c.poll_flush(t0 + STATUS_DEBOUNCE_MS - 1));
        assert!(c.poll_flush(t0 + STATUS_DEBOUNCE_MS));
        assert_eq!(c.next_deadline(), None);

        // Cooldown follow-up (`git push` right after `git commit` flush) is scheduled, not dropped.
        let t1 = t0 + STATUS_DEBOUNCE_MS;
        let follow = t1 + 400;
        assert_eq!(c.record(follow), t1 + STATUS_MIN_INTERVAL_MS);
        assert!(!c.poll_flush(t1 + STATUS_MIN_INTERVAL_MS - 1));
        assert!(c.poll_flush(t1 + STATUS_MIN_INTERVAL_MS));
        assert_eq!(c.next_deadline(), None);

        // Burst debounce shifts deadline cleanly until quiet.
        let b0 = t1 + STATUS_MIN_INTERVAL_MS + 5_000;
        assert_eq!(c.record(b0), b0 + STATUS_DEBOUNCE_MS);
        let b1 = b0 + 150;
        assert_eq!(c.record(b1), b1 + STATUS_DEBOUNCE_MS);
        assert!(!c.poll_flush(b1 + STATUS_DEBOUNCE_MS - 1));
        assert!(c.poll_flush(b1 + STATUS_DEBOUNCE_MS));

        // Continuous writes every 100ms for 5s never starve and still flush a trailing event.
        let churn_start = b1 + STATUS_MIN_INTERVAL_MS + 5_000;
        let mut flushes = Vec::new();
        for step in 0..=50u64 {
            let now_ms = churn_start + step * 100;
            if c.poll_flush(now_ms) {
                flushes.push(now_ms);
            }
            c.record(now_ms);
        }
        assert!(
            flushes.len() >= 2,
            "expected periodic flushes during churn: {flushes:?}"
        );
        let settle = churn_start + 5_000 + STATUS_MIN_INTERVAL_MS;
        assert!(c.poll_flush(settle));
        assert_eq!(c.next_deadline(), None);

        // Immediate flush cancels any pending coalesced deadline and resets cooldown baseline.
        let t_imm = settle + 1_000;
        c.record(t_imm);
        c.record_immediate_flush(t_imm + 50);
        assert_eq!(c.next_deadline(), None);
        assert_eq!(c.record(t_imm + 100), t_imm + 50 + STATUS_MIN_INTERVAL_MS);
    }

    #[test]
    fn classify_watcher_paths_handles_refs_worktrees_dynamic_remotes_and_ignores_noise() {
        let dir = tempfile::tempdir().unwrap();
        let dot_git = dir.path().join(".git");
        let refs_heads = dot_git.join("refs/heads");
        let refs_remotes = dot_git.join("refs/remotes");
        let reftable = dot_git.join("reftable");
        std::fs::create_dir_all(&refs_heads).unwrap();
        std::fs::create_dir_all(&refs_remotes).unwrap();
        std::fs::create_dir_all(&reftable).unwrap();

        let cls_git = classify_watcher_paths(std::slice::from_ref(&dot_git));
        assert_eq!(cls_git.kind, Some(InvalidationKind::Structural));
        assert!(cls_git.new_watches.contains(&WatchTarget {
            path: refs_heads.clone(),
            recursive: true,
        }));
        assert!(cls_git.new_watches.contains(&WatchTarget {
            path: refs_remotes.clone(),
            recursive: true,
        }));
        assert!(cls_git.new_watches.contains(&WatchTarget {
            path: reftable.clone(),
            recursive: false,
        }));

        assert_eq!(
            classify_watcher_paths(&[dot_git.join("HEAD")]).kind,
            Some(InvalidationKind::Immediate)
        );
        assert_eq!(
            classify_watcher_paths(&[dot_git.join("HEAD.lock")]).kind,
            Some(InvalidationKind::Immediate)
        );
        for rel in [
            "index",
            "index.lock",
            "FETCH_HEAD",
            "ORIG_HEAD",
            "packed-refs",
            "packed-refs.lock",
            "refs/heads/main",
            "refs/heads/feat/x.lock",
            "refs/remotes/origin/main",
            "refs/remotes/origin",
            "reftable/tables.list",
        ] {
            let p = dot_git.join(rel);
            assert_eq!(
                classify_watcher_paths(&[p]).kind,
                Some(InvalidationKind::Coalesced),
                "expected Coalesced for {rel}"
            );
        }

        // Dynamic `refs/remotes` directory creation yields a new recursive watch.
        let dyn_cls = classify_watcher_paths(std::slice::from_ref(&refs_remotes));
        assert_eq!(dyn_cls.kind, Some(InvalidationKind::Coalesced));
        assert_eq!(
            dyn_cls.new_watches,
            vec![WatchTarget {
                path: refs_remotes,
                recursive: true,
            }]
        );

        // Unwatched noise must not trigger invalidations.
        for noise in [
            dot_git.join("refs/tags/v1"),
            dot_git.join("objects/ab/1234"),
            dir.path().join("src/main.rs"),
        ] {
            let cls = classify_watcher_paths(&[noise]);
            assert_eq!(cls.kind, None);
            assert!(cls.new_watches.is_empty());
        }
    }

    #[tokio::test]
    async fn watcher_refreshes_ahead_after_external_git_commit_and_push_without_rpc() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let remote = base.join("remote.git");
        let repo = base.join("repo");
        let home = base.join("canban-home");
        std::fs::create_dir_all(&remote).unwrap();
        std::fs::create_dir_all(&repo).unwrap();

        let git_ok = |cwd: &Path, args: &[&str]| {
            let out = canban_exec::raw_command("git")
                .arg("-C")
                .arg(cwd)
                .args(args)
                .output()
                .unwrap_or_else(|e| panic!("failed to run git {args:?} in {}: {e}", cwd.display()));
            assert!(
                out.status.success(),
                "git {args:?} failed in {}: {}",
                cwd.display(),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git_ok(&remote, &["init", "--bare", "-b", "main"]);
        git_ok(&repo, &["init", "-b", "main"]);
        let commit_env = [
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ];
        let mut init_commit = commit_env.to_vec();
        init_commit.extend(["commit", "--allow-empty", "-m", "Initial"]);
        git_ok(&repo, &init_commit);
        git_ok(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git_ok(&repo, &["push", "-u", "origin", "main"]);

        let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
        let wake = Arc::new(tokio::sync::Notify::new());
        *app.runtime_handle.lock().unwrap() = Some(tokio::runtime::Handle::current());
        *app.runtime_wake.lock().unwrap() = Some(wake.clone());
        app.start_context_workers();

        let snap = Arc::new(Snapshot::default());
        let repo_str = repo.to_str().unwrap().to_owned();
        let term_val = json!({"cwd": repo_str});
        app.queue_context(&snap, "t1", "c1", "pty1", &term_val);

        let refs_heads_watch = WatchTarget {
            path: repo.join(".git/refs/heads"),
            recursive: true,
        };
        let refs_remotes_watch = WatchTarget {
            path: repo.join(".git/refs/remotes"),
            recursive: true,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let ready_ctx = app.cached_context("t1", "c1", "pty1", Some(&repo_str));
            let watched_ok = {
                let q = app.contexts.lock().unwrap();
                q.watched.contains(&refs_heads_watch) && q.watched.contains(&refs_remotes_watch)
            };
            if let Some(ctx) = ready_ctx
                && ctx.ahead == 0
                && ctx.behind == 0
                && watched_ok
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for initial context and ref watches"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        // Wait until any initial watch registration noise settles cleanly, then commit externally.
        let mut next_commit = commit_env.to_vec();
        next_commit.extend(["commit", "--allow-empty", "-m", "Local commit"]);
        git_ok(&repo, &next_commit);

        let commit_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.queue_context(&snap, "t1", "c1", "pty1", &term_val);
            if app
                .cached_context("t1", "c1", "pty1", Some(&repo_str))
                .is_some_and(|ctx| ctx.ahead == 1 && ctx.behind == 0)
            {
                break;
            }
            let remaining = commit_deadline
                .checked_duration_since(Instant::now())
                .expect("timed out waiting for external git commit ahead=1");
            tokio::select! {
                _ = wake.notified() => {}
                _ = tokio::time::sleep(remaining.min(Duration::from_millis(100))) => {}
            }
        }

        // Immediately run `git push` inside the 2s cooldown window: trailing coalescer must still update ahead -> 0.
        git_ok(&repo, &["push", "origin", "main"]);
        let push_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.queue_context(&snap, "t1", "c1", "pty1", &term_val);
            if app
                .cached_context("t1", "c1", "pty1", Some(&repo_str))
                .is_some_and(|ctx| ctx.ahead == 0 && ctx.behind == 0)
            {
                break;
            }
            let remaining = push_deadline
                .checked_duration_since(Instant::now())
                .expect("timed out waiting for external git push ahead=0");
            tokio::select! {
                _ = wake.notified() => {}
                _ = tokio::time::sleep(remaining.min(Duration::from_millis(100))) => {}
            }
        }

        app.trigger_stop();
        *app.runtime_handle.lock().unwrap() = None;
    }
}
