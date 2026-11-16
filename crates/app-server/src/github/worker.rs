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
        self.github_job_signal.notify_one();
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
            let weak = Arc::downgrade(self);
            std::thread::spawn(move || {
                while let Some(server) = weak.upgrade() {
                    if server.stopped() {
                        break;
                    }
                    let job = {
                        let mut jobs = server.github_jobs.lock().unwrap();
                        if !jobs
                            .pending
                            .iter()
                            .any(|(id, _, _)| !jobs.running.contains(id))
                        {
                            jobs = server
                                .github_job_signal
                                .wait_timeout(jobs, Duration::from_secs(1))
                                .unwrap()
                                .0;
                        }
                        let index = jobs
                            .pending
                            .iter()
                            .position(|(id, _, _)| !jobs.running.contains(id));
                        index
                            .and_then(|i| jobs.pending.remove(i))
                            .inspect(|(id, _, _)| {
                                jobs.running.insert(id.clone());
                            })
                    };
                    if let Some((pid, generation, manual)) = job {
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
                        drop(jobs);
                        server.github_job_signal.notify_all();
                        server.emit("github", None);
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_project_coalesces_and_new_rules_replace_pending_generation_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        let mut project: Project = serde_json::from_value(
            app.call(
                "project.create",
                json!({"name":"Queue", "directory":dir.path().join("source")}),
            )
            .unwrap(),
        )
        .unwrap();
        project.github = Some(serde_json::from_value(json!({"repository":"owner/repo", "repository_id":1,"generation":"first","intake":"all","pr_intake":"all","paused":true,"synced_at":null,"error":null})).unwrap());
        app.store
            .lock()
            .unwrap()
            .write(|db| db.save(&project))
            .unwrap();
        app.queue_github_sync(&project.id, false).unwrap();
        assert!(app.github_jobs.lock().unwrap().pending.is_empty());
        for _ in 0..3 {
            app.queue_github_sync(&project.id, true).unwrap();
        }
        assert_eq!(app.github_jobs.lock().unwrap().pending.len(), 1);
        // Same-repository collaborator rules save without access to a GitHub transport.
        let updated = app.call("project.github", json!({"project_id":project.id,"repository":"owner/repo","intake":"collaborators","pr_intake":"collaborators"})).unwrap();
        let jobs = app.github_jobs.lock().unwrap();
        assert_eq!(jobs.pending.len(), 1);
        assert_ne!(jobs.pending[0].1, "first");
        assert_eq!(
            jobs.pending[0].1,
            updated["github"]["generation"].as_str().unwrap()
        );
        drop(jobs);
        assert!(
            app.github_sync_run(&project.id, true, Some("first"))
                .is_ok()
        );
        assert!(app.snapshot().unwrap().cards.is_empty());
    }
}
