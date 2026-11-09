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
type Key = (String, String);
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
    running: BTreeSet<String>,
    results: BTreeMap<Key, Cached>,
    caches: BTreeMap<String, Arc<Mutex<canban_workspaces::context::ContextCache>>>,
    mappings: BTreeMap<String, (String, Arc<Snapshot>)>,
    watcher: Option<RecommendedWatcher>,
    watched: BTreeSet<PathBuf>,
}
impl ContextQueue {
    pub fn retain_targets(&mut self, wanted: &BTreeSet<String>) {
        self.targets.retain(|id, _| wanted.contains(id));
        let keys = self
            .targets
            .values()
            .filter_map(|t| {
                t.context
                    .cwd
                    .as_ref()
                    .map(|cwd| (t.context.machine.clone(), cwd.clone()))
            })
            .collect::<BTreeSet<_>>();
        self.pending.retain(|key, _| keys.contains(key));
    }
    pub fn probes(&self) -> u64 {
        self.caches.values().map(|c| c.lock().unwrap().probes).sum()
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len() + self.running.len()
    }
    pub fn reset(&mut self) {
        self.generation += 1;
        self.targets.clear();
        self.pending.clear();
        self.results.clear();
        self.caches.clear();
        self.mappings.clear();
    }
}
fn signature(s: &Snapshot, machine: &str) -> String {
    let projects = s
        .projects
        .iter()
        .map(|p| json!([p.id, p.work_location()]))
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
    canban_context::digest(&serde_json::to_vec(&json!([machine, projects, cards])).unwrap())
}
pub(super) struct CheckPermit<'a>(&'a AppServer, String);
impl Drop for CheckPermit<'_> {
    fn drop(&mut self) {
        self.0.check_slots.lock().unwrap().remove(&self.1);
        self.0.check_signal.notify_all();
    }
}
impl AppServer {
    pub(super) fn check(&self, machine: &str) -> CheckPermit<'_> {
        let mut slots = self.check_slots.lock().unwrap();
        while slots.len() >= 4 || slots.contains(machine) {
            slots = self.check_signal.wait(slots).unwrap();
        }
        slots.insert(machine.into());
        CheckPermit(self, machine.into())
    }
    pub(super) fn remote_connected(&self, terminal: &str) -> bool {
        let Some((machine, _)) = terminal.split_once('/') else {
            return true;
        };
        #[cfg(test)]
        if !self.observer_running.load(Ordering::SeqCst) {
            return true;
        }
        let inv = self.inventory.lock().unwrap();
        inv.remote_error.is_none()
            && inv.machines["machines"].as_array().is_some_and(|ms| {
                ms.iter().any(|m| {
                    (m["id"] == machine || m["name"] == machine) && m["state"] == "connected"
                })
            })
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
        let machine = terminal
            .split_once('/')
            .map(|(m, _)| m)
            .unwrap_or("local")
            .to_owned();
        let key = (machine.clone(), cwd.to_owned());
        let mut q = self.contexts.lock().unwrap();
        let digest = signature(s, &machine);
        if q.mappings
            .get(&machine)
            .is_none_or(|(old, _)| old != &digest)
        {
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
                        c.tags = Vec::new();
                        c.context_notes = None;
                        c.content = Default::default();
                        c
                    })
                    .collect(),
                ..Default::default()
            };
            q.mappings
                .insert(machine.clone(), (digest.clone(), Arc::new(mapping)));
        }
        let job = Job {
            snapshot: q.mappings[&machine].1.clone(),
            signature: digest,
            generation: q.generation,
            context: TerminalContext {
                machine,
                terminal_id: id.into(),
                core_id: core.into(),
                terminal: terminal.into(),
                cwd: Some(cwd.into()),
                project_id: None,
                card_id: None,
                branch: None,
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
            let (relevant, structural) = match result {
                Ok(e) => (
                    !e.kind.is_access()
                        && e.paths.iter().any(|p| {
                            p.file_name()
                                .is_some_and(|n| n == "HEAD" || n == "HEAD.lock" || n == ".git")
                        }),
                    e.paths
                        .iter()
                        .any(|p| p.file_name().is_some_and(|n| n == ".git")),
                ),
                Err(_) => (true, true),
            };
            if !relevant {
                return;
            }
            if let Some(server) = weak.upgrade() {
                let mut q = server.contexts.lock().unwrap();
                if structural {
                    q.caches.remove("local");
                } else if let Some(cache) = q.caches.get("local") {
                    cache.lock().unwrap().invalidate_branches();
                }
                q.results.retain(|(machine, _), _| machine != "local");
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
                        let key = q
                            .pending
                            .keys()
                            .find(|(m, _)| !q.running.contains(m))
                            .cloned();
                        if let Some(key) = key {
                            let job = q.pending.remove(&key).unwrap();
                            q.running.insert(key.0.clone());
                            let cache = q.caches.entry(key.0.clone()).or_default().clone();
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
                    let permit = server.check(&key.0);
                    let result = server.ops(&key.0).directory_context_cached(
                        &job.snapshot,
                        &key.1,
                        &mut cache.lock().unwrap(),
                    );
                    drop(permit);
                    let mut q = server.contexts.lock().unwrap();
                    q.running.remove(&key.0);
                    let mut changed = false;
                    if job.generation == q.generation
                        && let Ok((project_id, card_id, branch)) = result
                    {
                        let mut context = job.context.clone();
                        context.project_id = project_id;
                        context.card_id = card_id;
                        context.branch = branch;
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
                                t.context.machine == key.0
                                    && t.context.cwd.as_ref() == Some(&key.1)
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
                        if key.0 == "local" {
                            for path in cache.lock().unwrap().watch_paths() {
                                if q.watched.insert(path.clone())
                                    && let Some(w) = &mut q.watcher
                                    && let Err(e) = w.watch(&path, RecursiveMode::NonRecursive)
                                {
                                    eprintln!("Canban context watcher {}: {e}", path.display());
                                }
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
    pub(super) fn git_changed(&self, machine: &str) {
        let mut q = self.contexts.lock().unwrap();
        q.generation += 1;
        q.results.retain(|(m, _), _| m != machine);
        if let Some(cache) = q.caches.get(machine) {
            cache.lock().unwrap().invalidate_branches();
        }
        drop(q);
        self.wake_runtime();
    }
    pub(super) fn invalidate_contexts(&self) {
        let mut q = self.contexts.lock().unwrap();
        q.generation += 1;
        q.results.clear();
        q.caches.clear();
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
        let machine = terminal.split_once('/').map(|(m, _)| m).unwrap_or("local");
        let key = (machine.into(), cwd.into());
        let mut q = self.contexts.lock().unwrap();
        if let Some(r) = q.results.get(&key)
            && r.signature == signature(s, machine)
            && (machine == "local" || r.at.elapsed() < Duration::from_secs(30))
        {
            let mut context = r.context.clone();
            context.terminal_id = id.into();
            context.core_id = core.into();
            context.terminal = terminal.into();
            return Ok(context);
        }
        if machine != "local" {
            q.results.remove(&key);
            if let Some(cache) = q.caches.get(machine) {
                cache.lock().unwrap().invalidate_branches();
            }
        }
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
    fn check_budget_bounds_parallel_machines_and_serializes_one_machine() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        let (tx, rx) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let wait = Arc::new(Mutex::new(wait));
        let mut joins = vec![];
        for machine in ["a", "b", "c", "d", "a", "e"] {
            let app = app.clone();
            let tx = tx.clone();
            let wait = wait.clone();
            joins.push(std::thread::spawn(move || {
                let _permit = app.check(machine);
                tx.send(machine).unwrap();
                wait.lock().unwrap().recv().unwrap();
            }));
        }
        let first = (0..4)
            .map(|_| rx.recv_timeout(Duration::from_secs(2)).unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(first.len(), 4);
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        for _ in 0..6 {
            release.send(()).unwrap();
        }
        for j in joins {
            j.join().unwrap();
        }
        assert!(app.check_slots.lock().unwrap().is_empty());
    }
}
