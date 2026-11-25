//! Thin operation/lifecycle adapter; intake and reconciliation live in Kanban.
use super::*;
mod browse;
pub(super) mod delivery;
pub(crate) mod projection;
pub(super) mod worker;
use canban_kanban::github::{self as integration, Change};
impl AppServer {
    fn github_client(&self) -> Result<canban_github::Client> {
        if let Some(transport) = &self.github_transport {
            return Ok(canban_github::Client::new("sim-token".into())
                .with_cache(self.github.cache.clone(), "github.com:sim:bot".into())
                .with_transport(transport.clone())
                .with_clock(self.clock.clone()));
        }
        if self.github.status().state == "connected" {
            Ok(self.github.client()?.with_clock(self.clock.clone()))
        } else {
            Ok(
                canban_github::Client::from_cli(self.github_cli.clone(), true)?
                    .with_clock(self.clock.clone()),
            )
        }
    }
    pub(super) fn github_cached_client(&self) -> Result<canban_github::Client> {
        if let Some(transport) = &self.github_transport {
            return Ok(canban_github::Client::new("sim-token".into())
                .with_cache(self.github.cache.clone(), "github.com:sim:bot".into())
                .with_transport(transport.clone())
                .with_clock(self.clock.clone()));
        }
        if self.github.status().state == "connected" {
            Ok(self.github.cached_client()?.with_clock(self.clock.clone()))
        } else {
            Ok(
                canban_github::Client::from_cli(self.github_cli.clone(), false)?
                    .with_clock(self.clock.clone()),
            )
        }
    }
    pub(super) fn detect_project_github(
        &self,
        directory: &str,
        preferred_remote: &str,
    ) -> Option<DetectedGitHub> {
        let dir = directory.trim();
        if dir.is_empty() || !Path::new(dir).is_absolute() {
            return None;
        }
        let preferred = if preferred_remote.trim().is_empty() || preferred_remote.starts_with('-') {
            "origin"
        } else {
            preferred_remote.trim()
        };
        let urls = self.ops().remote_urls(dir, preferred);
        if let Some((remote, repo)) =
            urls.iter()
                .filter(|(r, _)| r == preferred)
                .find_map(|(r, u)| {
                    canban_github::remote_repository(u)
                        .ok()
                        .map(|repo| (r.clone(), repo))
                })
        {
            return Some(DetectedGitHub {
                repository: repo,
                remote,
            });
        }
        let mut fallback: Option<DetectedGitHub> = None;
        for (remote, url) in urls {
            let Ok(repo) = canban_github::remote_repository(&url) else {
                continue;
            };
            match &fallback {
                None => {
                    fallback = Some(DetectedGitHub {
                        repository: repo,
                        remote,
                    });
                }
                Some(first) if !first.repository.eq_ignore_ascii_case(&repo) => {
                    return None;
                }
                Some(_) => {}
            }
        }
        fallback
    }
    pub(super) fn try_auto_link_github_with_default(
        &self,
        repository: &str,
    ) -> Option<(GitHubSource, Option<String>)> {
        let repo = canban_github::repository(repository).ok()?;
        let client = self.github_client().ok()?;
        let info = client.repo(&repo).ok()?;
        let default_branch = Some(info.default_branch.trim().to_owned()).filter(|s| !s.is_empty());
        Some((
            GitHubSource {
                pr_intake: GitHubPrIntake::Off,
                authors: Vec::new(),
                labels: Vec::new(),
                include_drafts: false,
                paused: false,
                repository: info.full_name,
                repository_id: info.id,
                generation: self.next_id(),
                intake: GitHubIntake::Off,
                synced_at: None,
                error: None,
            },
            default_branch,
        ))
    }
    pub(super) fn try_auto_link_github(&self, repository: &str) -> Option<GitHubSource> {
        self.try_auto_link_github_with_default(repository)
            .map(|(source, _)| source)
    }
    /// Backfills `detected_github` and automatic repository bindings (`intake: Off`,
    /// `pr_intake: Off`) for unlinked Projects that have not been manually disconnected.
    pub(super) fn reconcile_project_github_bindings(&self) {
        let projects = self
            .store
            .lock()
            .unwrap()
            .db()
            .list::<Project>()
            .unwrap_or_default();
        let mut changed = false;
        for project in projects {
            if self.stopped() {
                break;
            }
            let dir = project.directory.clone();
            let detected = self.detect_project_github(&dir, &project.workflow.workspace.remote);
            let next_detected = detected.or_else(|| project.detected_github.clone());
            let auto_source = if project.github.is_none() && !project.github_disconnected {
                next_detected
                    .as_ref()
                    .and_then(|d| self.try_auto_link_github(&d.repository))
            } else {
                None
            };
            if next_detected == project.detected_github && auto_source.is_none() {
                continue;
            }
            let account_generation = self.github.generation();
            let saved = self.store.lock().unwrap().write(|db| {
                let mut current: Project = db.get(&project.id)?;
                if current.directory != dir {
                    return Ok(false);
                }
                let mut dirty = false;
                if current.detected_github != next_detected {
                    current.detected_github = next_detected.clone();
                    dirty = true;
                }
                if let Some(source) = auto_source
                    && current.github.is_none()
                    && !current.github_disconnected
                    && self.github.generation() == account_generation
                {
                    current.github = Some(source);
                    dirty = true;
                }
                if dirty {
                    db.save(&current)?;
                }
                Ok(dirty)
            });
            if matches!(saved, Ok(true)) {
                changed = true;
            }
        }
        if changed {
            self.emit("github", None);
        }
    }

    fn github_receipt(&self, method: &str, p: &Value) -> Result<Option<Value>> {
        let Some(key) = p["request_key"].as_str() else {
            return Ok(None);
        };
        let store = self.store.lock().unwrap();
        let saved = store
            .db()
            .conn
            .query_row(
                "SELECT fingerprint,result FROM operations WHERE id=?1",
                [key],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((fingerprint, result)) = saved {
            ensure!(
                fingerprint == canban_context::digest(&serde_json::to_vec(&json!([method, p]))?),
                "request_key_conflict: same key, different payload"
            );
            return Ok(Some(serde_json::from_str(&result)?));
        }
        Ok(None)
    }
    pub(super) fn github_call(&self, method: &str, p: Value) -> Result<Value> {
        if method == "github.context" {
            let card = {
                let store = self.store.lock().unwrap();
                super::resolve_card(store.db(), &p)?
            };
            let content = if let Some(source) = card.github_issue() {
                let path = format!("/repos/{}/issues/{}", source.repository, source.number);
                if let Some(value) = self
                    .github_cached_client()
                    .ok()
                    .and_then(|c| c.cached::<Value>(&path))
                {
                    value
                } else {
                    self.github_client()?.get::<Value>(&path)?
                }
            } else {
                Value::Null
            };
            return Ok(
                json!({"source":card.github_issue(),"content":content,"references":card.github_links}),
            );
        }
        if method == "github.review.publish" {
            return self.github_review_publish(&p);
        }
        ensure!(
            p["execution"].is_null(),
            "GitHub account/source and delivery operations require a user action"
        );
        if let Some(result) = self.github_browser_call(method, &p) {
            return result;
        }
        match method {
            "github.auth.status" => Ok(serde_json::to_value(self.github.status())?),
            "github.auth.begin" => {
                let status = self.github.begin()?;
                self.github_sync_notify.notify_one();
                Ok(serde_json::to_value(status)?)
            }
            "github.auth.refresh" => {
                self.github.refresh()?;
                Ok(serde_json::to_value(self.github.status())?)
            }
            "github.auth.cancel" => {
                self.github.cancel();
                Ok(json!({"cancelled":true}))
            }
            "github.auth.disconnect" => {
                self.github.disconnect()?;
                self.emit("github", None);
                Ok(json!({"disconnected":true}))
            }
            "github.installations" => self.github_client()?.get("/user/installations"),
            "project.detect_github" => {
                let directory = p["directory"].as_str().unwrap_or("");
                let remote = p["remote"].as_str().unwrap_or("origin");
                let detected = self.detect_project_github(directory, remote);
                Ok(json!({ "detected": detected }))
            }
            "project.github" => {
                let project_id = strp(&p, "project_id")?;
                let repo = canban_github::repository(strp(&p, "repository")?)?;
                let intake: GitHubIntake =
                    serde_json::from_value(p.get("intake").cloned().unwrap_or(json!("all")))?;
                let account_generation = self.github.generation();
                let existing = {
                    let store = self.store.lock().unwrap();
                    let pid = super::resolve_project_id(store.db(), project_id)?;
                    store.db().get::<Project>(&pid)?.github
                };
                let (repository, repository_id) = if let Some(source) =
                    existing.filter(|s| s.repository.eq_ignore_ascii_case(&repo))
                {
                    (source.repository, source.repository_id)
                } else {
                    let info = self.github_client()?.refreshing().repo(&repo)?;
                    (info.full_name, info.id)
                };
                let result = self.mutate(method, &p, |db| {
                    ensure!(
                        self.github.generation() == account_generation,
                        "GitHub account changed; connect the source again"
                    );
                    let pid = super::resolve_project_id(db, project_id)?;
                    let mut project: Project = db.get(&pid)?;
                    project.github_disconnected = false;
                    project.github = Some(GitHubSource {
                        pr_intake: serde_json::from_value(
                            p.get("pr_intake").cloned().unwrap_or(json!("off")),
                        )?,
                        authors: strings(&p, "authors"),
                        labels: strings(&p, "labels"),
                        include_drafts: p["include_drafts"].as_bool().unwrap_or(false),
                        paused: p["paused"].as_bool().unwrap_or(false),
                        repository,
                        repository_id,
                        generation: self.next_id(),
                        intake,
                        synced_at: None,
                        error: None,
                    });
                    if let Some(mode) = p.get("delivery_mode") {
                        project.delivery_mode = serde_json::from_value(mode.clone())?;
                    }
                    db.save(&project)?;
                    Ok(serde_json::to_value(project)?)
                })?;
                self.queue_github_sync(result["id"].as_str().context("Missing project")?, true)?;
                Ok(result)
            }
            "github.source.disconnect" => {
                let existing = {
                    let store = self.store.lock().unwrap();
                    let db = store.db();
                    let pid = super::resolve_project_id(db, strp(&p, "project_id")?)?;
                    db.get::<Project>(&pid)?
                };
                let detected = self
                    .detect_project_github(&existing.directory, &existing.workflow.workspace.remote)
                    .or_else(|| existing.detected_github.clone());
                self.mutate(method, &p, |db| {
                    let pid = super::resolve_project_id(db, strp(&p, "project_id")?)?;
                    let mut project: Project = db.get(&pid)?;
                    project.github = None;
                    project.github_disconnected = true;
                    project.detected_github = detected;
                    db.save(&project)?;
                    Ok(serde_json::to_value(project)?)
                })
            }
            "github.reference.search" => {
                let project: Project = {
                    let store = self.store.lock().unwrap();
                    let db = store.db();
                    let pid = super::resolve_project_id(db, strp(&p, "project_id")?)?;
                    db.get(&pid)?
                };
                let source = project
                    .github
                    .context("Connect this Project to a GitHub repository first")?;
                ensure!(
                    strp(&p, "source_generation")? == source.generation,
                    "Project source changed; search again"
                );
                let generation = self.github.generation();
                let issues = self
                    .github_client()?
                    .references(&source.repository, strp(&p, "query")?)?;
                ensure!(
                    generation == self.github.generation(),
                    "GitHub account changed; search again"
                );
                let current: Project = self.store.lock().unwrap().db().get(&project.id)?;
                ensure!(
                    current
                        .github
                        .as_ref()
                        .is_some_and(|s| s.generation == source.generation),
                    "Project source changed; search again"
                );
                Ok(json!(issues))
            }
            "github.issue.search" => {
                let project: Project = self
                    .store
                    .lock()
                    .unwrap()
                    .db()
                    .get(strp(&p, "project_id")?)?;
                let source = project
                    .github
                    .context("Connect this Project to a GitHub repository first")?;
                let client = self.github_client()?;
                let issues = if p["kind"] == "pr" {
                    let q = strp(&p, "query")?;
                    if let Ok(number) = q.trim().trim_start_matches('#').parse::<u64>() {
                        let item = client.item(&source.repository, number)?;
                        ensure!(item.pull_request.is_some(), "This number is an Issue");
                        vec![item]
                    } else {
                        let value: Value = client.get(&format!(
                            "/search/issues?q={}&per_page=30",
                            canban_github::segment(&format!(
                                "repo:{} is:pr in:title {}",
                                source.repository, q
                            ))
                        ))?;
                        serde_json::from_value(value["items"].clone())?
                    }
                } else {
                    client.search(&source.repository, strp(&p, "query")?)?
                };
                let cards = self.store.lock().unwrap().db().list::<Card>()?;
                Ok(json!(issues.into_iter().map(|issue|{
                    let existing=cards.iter().find(|c|c.project_id==project.id && c.github_issue().is_some_and(|s|s.repository_id==source.repository_id && s.issue_id==issue.id));
                    json!({"issue":issue,"source_generation":source.generation,"card_id":existing.map(|c|&c.id),"removed":existing.is_some_and(|c|c.removed_at.is_some())})
                }).collect::<Vec<_>>()))
            }
            "github.issue.add" => {
                let project: Project = self
                    .store
                    .lock()
                    .unwrap()
                    .db()
                    .get(strp(&p, "project_id")?)?;
                let source = project
                    .github
                    .as_ref()
                    .context("Connect a GitHub repository first")?;
                ensure!(
                    strp(&p, "source_generation")? == source.generation,
                    "Project source changed; search again"
                );
                let account_generation = self.github.generation();
                let number = p["number"].as_u64().context("Issue number is required")?;
                let client = self.github_client()?.refreshing();
                let info = client.repo(&source.repository)?;
                ensure!(
                    info.id == source.repository_id,
                    "GitHub repository identity changed; reconnect the source"
                );
                let issue = if p["kind"] == "pr" {
                    let item = client.item(&source.repository, number)?;
                    ensure!(item.pull_request.is_some(), "Select a PR");
                    item
                } else {
                    client.issue(&source.repository, number)?
                };
                let comments = Vec::new();
                self.mutate(method, &p, |db| {
                    ensure!(
                        self.github.generation() == account_generation,
                        "GitHub account changed; search again"
                    );
                    let current: Project = db.get(&project.id)?;
                    ensure!(
                        current
                            .github
                            .as_ref()
                            .is_some_and(|s| s.generation == source.generation),
                        "Project source changed; search again"
                    );
                    let instructions = p["instructions"].as_str().unwrap_or("");
                    let now_ms = self.now_ms();
                    let mut card = integration::upsert_at(
                        db,
                        &current,
                        Change { issue, comments },
                        true,
                        instructions,
                        now_ms,
                    )?;
                    if let Some(note_id) = p["source_note_id"].as_str() {
                        let note: Note = db.get(note_id)?;
                        ensure!(
                            note.project_id == project.id,
                            "Note belongs to another Project"
                        );
                        let notes = card.context_notes.get_or_insert_with(Vec::new);
                        if !notes.contains(&note.id) {
                            notes.push(note.id.clone());
                        }
                        db.save(&card)?;
                        crate::notes::relate_card(
                            db,
                            &note.id,
                            &card,
                            true,
                            now_ms,
                            self.id_gen.as_ref(),
                        )?;
                    }
                    crate::sessions::queue_card_mentions(
                        db,
                        &self.config(),
                        &mut card,
                        &current,
                        &p,
                        &[instructions],
                        now_ms,
                    )?;
                    Ok(serde_json::to_value(card)?)
                })
            }
            "github.sync.request" => {
                self.queue_github_sync(strp(&p, "project_id")?, true)?;
                Ok(json!({"queued":true}))
            }
            "github.repository.info" => {
                let repo = canban_github::repository(strp(&p, "repository")?)?;
                self.github_cached_client()?
                    .get::<Value>(&format!("/repos/{repo}"))
            }
            "github.repositories" => {
                let project = {
                    let store = self.store.lock().unwrap();
                    let pid = super::resolve_project_id(store.db(), strp(&p, "project_id")?)?;
                    store.db().get::<Project>(&pid)?
                };
                let mut candidates = Vec::new();
                for (remote, url) in self
                    .ops()
                    .remote_urls(&project.directory, &project.workflow.workspace.remote)
                {
                    if let Ok(repository) = canban_github::remote_repository(&url) {
                        candidates.push(json!({"remote":remote,"repository":repository}));
                    }
                }
                Ok(json!({"candidates":candidates}))
            }
            "github.sync" => {
                self.github_sync(strp(&p, "project_id")?)?;
                Ok(json!({"synced":true}))
            }
            "github.pr.publish" => self.github_publish(p),
            "github.pr.refresh" => self.github_pr_refresh(p),
            "github.pr.merge" => self.github_pr_merge(p),
            _ => bail!("Unknown GitHub operation: {method}"),
        }
    }
    pub(super) fn github_sync(&self, project_id: &str) -> Result<()> {
        self.github_sync_run(project_id, false, None)
    }
    pub(super) fn github_sync_run(
        &self,
        project_id: &str,
        manual: bool,
        expected: Option<&str>,
    ) -> Result<()> {
        let gate = self.github_sync_gate(project_id);
        let _guard = gate.lock().unwrap();
        let mut project: Project = self.store.lock().unwrap().db().get(project_id)?;
        let source = project
            .github
            .as_mut()
            .context("Project has no GitHub source")?;
        if expected.is_some_and(|g| g != source.generation) {
            return Ok(());
        }
        if source.paused && !manual {
            return Ok(());
        }
        source.paused = false;
        let generation = source.generation.clone();
        let last_success = source.synced_at;
        self.github_phase(&project, "issues");
        let issues = self.github_sync_issues(&project);
        self.github_phase(&project, "prs");
        let prs = self.github_pr_intake(&project);
        let branch_prs = self.github_discover_branch_prs(&project);
        self.github_phase(&project, "observations");
        let observations = self.github_event_refresh(&project);
        let errors: Vec<_> = [
            ("Issues", issues),
            ("PRs", prs),
            ("Branch PRs", branch_prs),
            ("Observations", observations),
        ]
        .into_iter()
        .filter_map(|(stage, r)| r.err().map(|e| format!("{stage}: {e:#}")))
        .collect();
        let error = (!errors.is_empty()).then(|| errors.join("; "));
        self.store.lock().unwrap().write(|db| {
            let mut current: Project = db.get(project_id)?;
            if let Some(g) = &mut current.github
                && g.generation == generation
            {
                g.synced_at = if error.is_none() {
                    Some(self.now_ms())
                } else {
                    last_success
                };
                g.error = error.clone();
                db.save(&current)?;
            }
            Ok(())
        })?;
        self.emit("github", None);
        if let Some(error) = error {
            bail!("{error}");
        }
        Ok(())
    }
    fn github_sync_issues(&self, project: &Project) -> Result<()> {
        let project_id = project.id.as_str();
        let (cards, cache) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            (db.list::<Card>()?, db.list::<GitHubIssueSnapshot>()?)
        };
        let source = project
            .github
            .as_ref()
            .context("Project has no GitHub source")?;
        if source.paused {
            return Ok(());
        }
        let account_generation = self.github.generation();
        let fetched = self
            .github_client()
            .map(|c| c.refreshing())
            .and_then(|client| {
                ensure!(
                    client.repo(&source.repository)?.id == source.repository_id,
                    "Repository identity changed; reconnect source"
                );
                integration::fetch(&client, project, &cards, &cache)
            });
        let error = fetched.as_ref().err().map(|e| format!("{e:#}"));
        let result = match fetched {
            Ok(sync) => {
                let observations = sync
                    .changes
                    .iter()
                    .map(|c| serde_json::to_value(&c.issue))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                self.github_apply_changes(
                    project,
                    account_generation,
                    sync.changes,
                    observations,
                    None,
                )
            }
            Err(ref e) => self.store.lock().unwrap().write(|db| {
                let mut current: Project = db.get(project_id)?;
                if let Some(s) = &mut current.github
                    && s.generation == source.generation
                {
                    s.error = Some(format!("{e:#}"));
                    db.save(&current)?;
                }
                Ok(())
            }),
        };
        self.emit("github", None);
        result?;
        if let Some(error) = error {
            bail!("{error}")
        }
        Ok(())
    }
    /// Release the shared Store between bounded transactions. Every batch fences
    /// source/account generations and rebuilds local ownership after removals.
    fn github_apply_changes(
        &self,
        project: &Project,
        account_generation: u64,
        changes: Vec<integration::Change>,
        observations: Vec<Value>,
        kind: Option<&str>,
    ) -> Result<()> {
        let source = project
            .github
            .as_ref()
            .context("Project has no GitHub source")?;
        let mut changes = changes.into_iter().zip(observations);
        loop {
            let batch: Vec<_> = changes.by_ref().take(64).collect();
            if batch.is_empty() {
                break;
            }
            let (changes, observations): (Vec<_>, Vec<_>) = batch.into_iter().unzip();
            let mut store = self.store.lock().unwrap();
            let now_ms = self.now_ms();
            store.write(|db| {
                ensure!(
                    self.github.generation() == account_generation,
                    "GitHub account changed; discarded stale sync"
                );
                integration::apply_batch_at(
                    db,
                    project,
                    &source.generation,
                    integration::Sync { changes },
                    false,
                    now_ms,
                )?;
                events::github::observe_batch(db, &source.repository, &observations, kind, now_ms)
            })?;
            if std::env::var_os("CANBAN_BOARD_TRACE").is_some() {
                eprintln!(
                    "GITHUB_APPLY_BATCH items={} lock_ms={}",
                    observations.len(),
                    self.now_ms().saturating_sub(now_ms)
                );
            }
            drop(store);
            std::thread::yield_now();
        }
        Ok(())
    }
}

impl AppServer {
    fn github_event_refresh(&self, project: &Project) -> Result<()> {
        let project_repo = project.github.as_ref().map(|g| g.repository.as_str());
        let (cards, enabled) = {
            let store = self.store.lock().unwrap();
            let subs = store.db().list::<EventSubscription>()?;
            let enabled = subs.iter().any(|s| {
                s.enabled
                    && (s.selector.types.is_empty()
                        || s.selector
                            .types
                            .iter()
                            .any(|t| t == "*" || t.starts_with("github.")))
                    && (s.selector.project_id.is_none()
                        || s.selector.project_id.as_deref() == Some(project.id.as_str())
                        || project_repo.is_some_and(|repo| {
                            s.selector
                                .repository
                                .as_deref()
                                .is_some_and(|r| r.eq_ignore_ascii_case(repo))
                        }))
            });
            (store.db().list::<Card>()?, enabled)
        };
        if !enabled {
            return Ok(());
        }
        let current_login = self.github.status().login;

        // Repo-level polling for Issues and Issue comments (works even when `intake: off`).
        if let Some(source) = &project.github {
            self.poll_repo_issues_and_comments(&source.repository, current_login.as_deref())?;
        }

        for card in cards
            .iter()
            .filter(|c| c.project_id == project.id && c.active())
        {
            let Some(source) = card.github_issue() else {
                continue;
            };
            let overview = format!(
                "/repos/{}/{}/{}",
                source.repository,
                if card.is_github_pr() {
                    "pulls"
                } else {
                    "issues"
                },
                source.number
            );
            self.github_read(&overview, true, false)?;
            let mut sections = vec![format!(
                "/repos/{}/issues/{}/comments",
                source.repository, source.number
            )];
            if card.is_github_pr() {
                sections.push(format!(
                    "/repos/{}/pulls/{}/reviews",
                    source.repository, source.number
                ));
                sections.push(format!(
                    "/repos/{}/pulls/{}/comments",
                    source.repository, source.number
                ));
            }
            let overview_value = self.github_read(&overview, false, false)?.data;
            if card.is_github_pr()
                && let Some(sha) = overview_value["head"]["sha"].as_str()
            {
                sections.push(format!(
                    "/repos/{}/commits/{sha}/check-runs",
                    source.repository
                ));
            }
            for root in sections {
                let section = if root.contains("/check-runs") {
                    "checks"
                } else if root.ends_with("/reviews") {
                    "reviews"
                } else if root.contains("/pulls/") {
                    "threads"
                } else {
                    "discussion"
                };
                let mut items = Vec::new();
                for page in 1..=100 {
                    let response =
                        self.github_read(&format!("{root}?per_page=30&page={page}"), true, false)?;
                    let values = if section == "checks" {
                        response.data["check_runs"].as_array()
                    } else {
                        response.data.as_array()
                    }
                    .context("Expected GitHub collection")?;
                    items.extend(values.iter().cloned());
                    if !response
                        .link
                        .as_ref()
                        .is_some_and(|l| l.contains("rel=\"next\""))
                    {
                        break;
                    }
                    ensure!(
                        page < 100,
                        "GitHub observation page bound reached; scan is incomplete"
                    );
                }
                self.store.lock().unwrap().write(|db| {
                    events::github::complete_collection_with_login(
                        db,
                        &source.repository,
                        source.number,
                        if card.is_github_pr() { "pr" } else { "issue" },
                        section,
                        &items,
                        current_login.as_deref(),
                        self.now_ms(),
                    )
                })?;
            }
        }
        Ok(())
    }

    /// Polls repo-level `/issues` (filtering out PRs via `pull_request.is_none()`) and
    /// `/issues/comments`, advancing the durable per-repo watermark only after both scans succeed.
    fn poll_repo_issues_and_comments(
        &self,
        repository: &str,
        current_login: Option<&str>,
    ) -> Result<()> {
        let existing_watermark = {
            let store = self.store.lock().unwrap();
            events::github::load_repo_watermark(store.db(), repository)?
        };
        let scan_started_iso = events::github::format_iso8601_utc(self.now_ms());

        let mut issues = Vec::new();
        let issues_root = match &existing_watermark {
            Some(wm) => format!(
                "/repos/{repository}/issues?state=all&sort=updated&direction=asc&since={}&per_page=100",
                wm.issues_since
            ),
            None => {
                format!(
                    "/repos/{repository}/issues?state=all&sort=updated&direction=desc&per_page=100"
                )
            }
        };
        let max_pages = if existing_watermark.is_some() { 10 } else { 1 };
        for page in 1..=max_pages {
            let response = self.github_read(&format!("{issues_root}&page={page}"), true, false)?;
            let values = response
                .data
                .as_array()
                .context("Expected GitHub issues array")?;
            // GitHub `/issues` returns both Issues and PRs; filter out PRs here.
            for item in values {
                if item.get("pull_request").is_none() {
                    issues.push(item.clone());
                }
            }
            if !response
                .link
                .as_ref()
                .is_some_and(|l| l.contains("rel=\"next\""))
            {
                break;
            }
            if existing_watermark.is_some() {
                ensure!(
                    page < max_pages,
                    "GitHub repo issues scan hit page bound; watermark not advanced"
                );
            }
        }

        let mut comments = Vec::new();
        let comments_root = match &existing_watermark {
            Some(wm) => format!(
                "/repos/{repository}/issues/comments?sort=updated&direction=asc&since={}&per_page=100",
                wm.comments_since
            ),
            None => {
                format!(
                    "/repos/{repository}/issues/comments?sort=updated&direction=desc&per_page=100"
                )
            }
        };
        for page in 1..=max_pages {
            let response =
                self.github_read(&format!("{comments_root}&page={page}"), true, false)?;
            let values = response
                .data
                .as_array()
                .context("Expected GitHub issue comments array")?;
            comments.extend(values.iter().cloned());
            if !response
                .link
                .as_ref()
                .is_some_and(|l| l.contains("rel=\"next\""))
            {
                break;
            }
            if existing_watermark.is_some() {
                ensure!(
                    page < max_pages,
                    "GitHub repo comments scan hit page bound; watermark not advanced"
                );
            }
        }

        let max_issue_updated = issues
            .iter()
            .filter_map(|i| i["updated_at"].as_str())
            .max()
            .map(str::to_owned);
        let max_comment_updated = comments
            .iter()
            .filter_map(|c| c["updated_at"].as_str())
            .max()
            .map(str::to_owned);

        let next_watermark = match &existing_watermark {
            Some(wm) => events::github::RepoWatermark {
                baseline_at: wm.baseline_at.clone(),
                issues_since: max_issue_updated
                    .filter(|u| u.as_str() > wm.issues_since.as_str())
                    .unwrap_or_else(|| {
                        if scan_started_iso > wm.issues_since {
                            scan_started_iso.clone()
                        } else {
                            wm.issues_since.clone()
                        }
                    }),
                comments_since: max_comment_updated
                    .filter(|u| u.as_str() > wm.comments_since.as_str())
                    .unwrap_or_else(|| {
                        if scan_started_iso > wm.comments_since {
                            scan_started_iso.clone()
                        } else {
                            wm.comments_since.clone()
                        }
                    }),
            },
            None => events::github::RepoWatermark {
                baseline_at: scan_started_iso.clone(),
                issues_since: max_issue_updated
                    .filter(|u| u.as_str() > scan_started_iso.as_str())
                    .unwrap_or_else(|| scan_started_iso.clone()),
                comments_since: max_comment_updated
                    .filter(|u| u.as_str() > scan_started_iso.as_str())
                    .unwrap_or_else(|| scan_started_iso.clone()),
            },
        };

        self.store.lock().unwrap().write(|db| {
            let now_ms = self.now_ms();
            events::github::observe_batch_with_context(
                db,
                repository,
                &issues,
                Some("issue"),
                existing_watermark.as_ref().map(|w| w.baseline_at.as_str()),
                current_login,
                now_ms,
            )?;
            events::github::observe_repo_comments_batch(
                db,
                repository,
                &comments,
                existing_watermark.is_some(),
                current_login,
                now_ms,
            )?;
            events::github::save_repo_watermark(db, repository, &next_watermark)?;
            Ok(())
        })?;
        self.wake_runtime();
        Ok(())
    }
}
