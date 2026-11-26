//! Bounded project workers, independent of browsing/cache refresh and UI RPCs.
use super::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[derive(Default)]
pub(crate) struct Jobs {
    pending: VecDeque<(String, String, bool)>,
    running: BTreeSet<String>,
    gates: BTreeMap<String, Arc<Mutex<()>>>,
    pub status: BTreeMap<String, GitHubSyncStatus>,
}
impl AppServer {
    pub(super) fn github_sync_gate(&self, id: &str) -> Arc<Mutex<()>> {
        self.github_jobs
            .lock()
            .unwrap()
            .gates
            .entry(id.into())
            .or_default()
            .clone()
    }
    pub(crate) fn queue_github_sync(&self, project_id: &str, manual: bool) -> Result<()> {
        let project = {
            let store = self.store.lock().unwrap();
            let pid = super::super::resolve_project_id(store.db(), project_id)?;
            store.db().get::<Project>(&pid)?
        };
        let source = project.github.context("Project has no GitHub source")?;
        if source.paused && !manual {
            return Ok(());
        }
        let mut jobs = self.github_jobs.lock().unwrap();
        if jobs.status.get(&project.id).is_some_and(|s| {
            s.generation == source.generation
                && matches!(
                    s.phase.as_str(),
                    "queued" | "issues" | "prs" | "observations"
                )
        }) {
            return Ok(());
        }
        jobs.pending.retain(|(p, _, _)| p != &project.id);
        jobs.pending
            .push_back((project.id.clone(), source.generation.clone(), manual));
        jobs.status.insert(
            project.id,
            GitHubSyncStatus {
                generation: source.generation,
                phase: "queued".into(),
                ..Default::default()
            },
        );
        drop(jobs);
        self.github_job_notify.notify_one();
        self.emit("github", None);
        Ok(())
    }
    pub(super) fn github_phase(&self, project: &Project, phase: &str) {
        let mut jobs = self.github_jobs.lock().unwrap();
        if let Some(s) = jobs.status.get_mut(&project.id)
            && project
                .github
                .as_ref()
                .is_some_and(|g| g.generation == s.generation)
        {
            s.phase = phase.into();
        }
        drop(jobs);
        self.emit("github", None);
    }
    pub(crate) fn start_github_workers(self: &Arc<Self>) {
        for _ in 0..2 {
            self.spawn_bg(github_worker_loop(
                Arc::downgrade(self),
                self.github_job_notify.clone(),
                self.stop_receiver(),
            ));
        }
    }
}

async fn github_worker_loop(
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
            let job = {
                let mut jobs = server.github_jobs.lock().unwrap();
                let index = jobs
                    .pending
                    .iter()
                    .position(|(id, _, _)| !jobs.running.contains(id));
                let claimed = index
                    .and_then(|i| jobs.pending.remove(i))
                    .inspect(|(id, _, _)| {
                        jobs.running.insert(id.clone());
                    });
                if claimed.is_some()
                    && jobs
                        .pending
                        .iter()
                        .any(|(id, _, _)| !jobs.running.contains(id))
                {
                    server.github_job_notify.notify_one();
                }
                claimed
            };
            let Some((pid, generation, manual)) = job else {
                return Some(false);
            };
            let before = server
                .store
                .lock()
                .unwrap()
                .db()
                .list::<Card>()
                .unwrap_or_default();
            let result = server.github_sync_run(&pid, manual, Some(&generation));
            let after = server
                .store
                .lock()
                .unwrap()
                .db()
                .list::<Card>()
                .unwrap_or_default();
            let added: Vec<_> = after
                .iter()
                .filter(|c| c.project_id == pid && !before.iter().any(|b| b.id == c.id))
                .collect();
            let has_more = {
                let mut jobs = server.github_jobs.lock().unwrap();
                jobs.running.remove(&pid);
                if let Some(s) = jobs.status.get_mut(&pid)
                    && s.generation == generation
                {
                    s.phase = if result.is_ok() { "success" } else { "failed" }.into();
                    s.error = result.err().map(|e| format!("{e:#}"));
                    s.issues_added = added
                        .iter()
                        .filter(|c| c.github_issue().is_some() && !c.is_github_pr())
                        .count();
                    s.prs_added = added.iter().filter(|c| c.is_github_pr()).count();
                }
                jobs.pending
                    .iter()
                    .any(|(id, _, _)| !jobs.running.contains(id))
            };
            if has_more {
                server.github_job_notify.notify_one();
            }
            server.emit("github", None);
            Some(has_more)
        };
        let worked = if inline {
            let res = work();
            tokio::task::yield_now().await;
            Ok(res)
        } else {
            tokio::task::spawn_blocking(work).await
        };
        match worked {
            Ok(Some(true)) => continue,
            Ok(Some(false)) => {
                if !AppServer::wait_or_stop(&notify, &mut stop_rx, None).await {
                    break;
                }
            }
            _ => break,
        }
    }
}
