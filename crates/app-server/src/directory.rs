//! Bounded, coalesced context discovery. Git IO never holds the product DB lock.
use super::*;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

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
    at: Instant,
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
    watched: BTreeSet<PathBuf>,
    pending_watches: BTreeSet<PathBuf>,
    last_status_invalidation: Option<Instant>,
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
    pub fn reset(&mut self) {
        self.generation += 1;
        self.targets.clear();
        self.pending.clear();
        self.results.clear();
        self.cache = Default::default();
        self.mapping = None;
        self.pending_watches.clear();
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
        let host = canban_herdr::host::Host::new(self.config());
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
            }
        }
        if !fresh {
            q.pending.insert(key, job);
            self.context_signal.notify_all();
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
        let weak = Arc::downgrade(self);
        let watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            let (head_or_git, structural, status_meta, dot_git_dirs) = match result {
                Ok(e) if !e.kind.is_access() => {
                    let has_name = |pred: fn(&std::ffi::OsStr) -> bool| {
                        e.paths.iter().any(|p| p.file_name().is_some_and(pred))
                    };
                    let dot_git_dirs = e
                        .paths
                        .iter()
                        .filter(|p| p.file_name().is_some_and(|n| n == ".git") && p.is_dir())
                        .cloned()
                        .collect();
                    (
                        has_name(|n| n == "HEAD" || n == "HEAD.lock" || n == ".git"),
                        has_name(|n| n == ".git"),
                        has_name(|n| {
                            n == "index"
                                || n == "FETCH_HEAD"
                                || n == "ORIG_HEAD"
                                || n == "packed-refs"
                        }),
                        dot_git_dirs,
                    )
                }
                Ok(_) => (false, false, false, Vec::new()),
                Err(_) => (true, true, false, Vec::new()),
            };
            if !head_or_git && !status_meta {
                return;
            }
            if let Some(server) = weak.upgrade() {
                let mut q = server.contexts.lock().unwrap();
                if !head_or_git {
                    if q.last_status_invalidation
                        .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
                    {
                        return;
                    }
                    q.last_status_invalidation = Some(Instant::now());
                }
                q.pending_watches.extend(dot_git_dirs);
                if structural {
                    q.cache = Default::default();
                } else {
                    q.cache.lock().unwrap().invalidate_branches();
                }
                q.results.clear();
                q.generation += 1;
                drop(q);
                server.wake_runtime();
            }
        })
        .ok();
        self.contexts.lock().unwrap().watcher = watcher;
        for _ in 0..4 {
            let weak = Arc::downgrade(self);
            std::thread::spawn(move || {
                loop {
                    let Some(server) = weak.upgrade() else {
                        break;
                    };
                    if server.stopped() {
                        break;
                    }
                    let next = {
                        let mut q = server.contexts.lock().unwrap();
                        let pending_watches = std::mem::take(&mut q.pending_watches);
                        for path in pending_watches {
                            if q.watched.insert(path.clone())
                                && let Some(w) = &mut q.watcher
                                && let Err(e) = w.watch(&path, RecursiveMode::NonRecursive)
                            {
                                eprintln!("Canban context watcher {}: {e}", path.display());
                            }
                        }
                        if !q.running
                            && let Some(key) = q.pending.keys().next().cloned()
                        {
                            let job = q.pending.remove(&key).unwrap();
                            q.running = true;
                            let cache = q.cache.clone();
                            Some((key, job, cache))
                        } else {
                            let _ = server
                                .context_signal
                                .wait_timeout(q, Duration::from_secs(1))
                                .unwrap();
                            None
                        }
                    };
                    let Some((key, job, cache)) = next else {
                        continue;
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
                        if q.pending.get(&key).is_some_and(|p| {
                            p.generation == job.generation && p.signature == job.signature
                        }) {
                            q.pending.remove(&key);
                        }
                        q.results.insert(
                            key.clone(),
                            Cached {
                                context: context.clone(),
                                signature: job.signature.clone(),
                                at: Instant::now(),
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
                            let mut contexts = server.terminal_contexts.lock().unwrap();
                            contexts.retain(|c| c.terminal_id != context.terminal_id);
                            contexts.push(context);
                        }
                        for path in cache.lock().unwrap().watch_paths() {
                            if q.watched.insert(path.clone())
                                && let Some(w) = &mut q.watcher
                                && let Err(e) = w.watch(&path, RecursiveMode::NonRecursive)
                            {
                                eprintln!("Canban context watcher {}: {e}", path.display());
                            }
                        }
                    }
                    server.context_signal.notify_all();
                    drop(q);
                    if changed {
                        server.emit("terminals", None);
                    }
                }
            });
        }
    }
    pub(super) fn git_changed(&self) {
        let mut q = self.contexts.lock().unwrap();
        q.generation += 1;
        q.results.clear();
        q.cache.lock().unwrap().invalidate_branches();
        drop(q);
        self.wake_runtime();
    }
    pub(super) fn invalidate_contexts(&self) {
        let mut q = self.contexts.lock().unwrap();
        q.generation += 1;
        q.results.clear();
        q.cache = Default::default();
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
        let ttl = Duration::from_secs(15);
        let mut q = self.contexts.lock().unwrap();
        if let Some(r) = q.results.get(&key)
            && r.signature == signature(s)
            && r.at.elapsed() < ttl
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
