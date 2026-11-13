//! Thin operation/lifecycle adapter; intake and reconciliation live in Kanban.
use super::*;
mod browse;
mod delivery;
mod projection;
use canban_kanban::github::{self as integration, Change};
impl AppServer {
    fn github_client(&self) -> Result<canban_github::Client> {
        if self.github.status().state == "connected" {
            self.github.client()
        } else {
            canban_github::Client::from_cli(self.github_cli.clone(), true)
        }
    }
    fn github_cached_client(&self) -> Result<canban_github::Client> {
        if self.github.status().state == "connected" {
            self.github.cached_client()
        } else {
            canban_github::Client::from_cli(self.github_cli.clone(), false)
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
        ensure!(
            p["execution"].is_null(),
            "GitHub account/source and delivery operations require a user action"
        );
        if let Some(result) = self.github_browser_call(method, &p) {
            return result;
        }
        match method {
            "github.auth.status" => Ok(serde_json::to_value(self.github.status())?),
            "github.auth.begin" => Ok(serde_json::to_value(self.github.begin()?)?),
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
            "project.github" => {
                let project_id = strp(&p, "project_id")?;
                let repo = canban_github::repository(strp(&p, "repository")?)?;
                let intake: GitHubIntake =
                    serde_json::from_value(p.get("intake").cloned().unwrap_or(json!("all")))?;
                let account_generation = self.github.generation();
                let client = self.github_client()?.refreshing();
                let info = client.repo(&repo)?;
                if intake == GitHubIntake::Collaborators || p["pr_intake"] == "collaborators" {
                    client.collaborators(&repo).context("Cannot enable collaborator intake: could not verify repository collaborators")?;
                }
                let result = self.mutate(method, &p, |db| {
                    ensure!(
                        self.github.generation() == account_generation,
                        "GitHub account changed; connect the source again"
                    );
                    let mut project: Project = db.get(project_id)?;
                    project.github = Some(GitHubSource {
                        pr_intake: serde_json::from_value(
                            p.get("pr_intake").cloned().unwrap_or(json!("off")),
                        )?,
                        authors: strings(&p, "authors"),
                        labels: strings(&p, "labels"),
                        include_drafts: p["include_drafts"].as_bool().unwrap_or(false),
                        paused: p["paused"].as_bool().unwrap_or(false),
                        repository: info.full_name,
                        repository_id: info.id,
                        generation: id(),
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
                self.github_sync_due.store(true, Ordering::SeqCst);
                Ok(result)
            }
            "github.source.disconnect" => self.mutate(method, &p, |db| {
                let mut project: Project = db.get(strp(&p, "project_id")?)?;
                project.github = None;
                db.save(&project)?;
                Ok(serde_json::to_value(project)?)
            }),
            "github.reference.search" => {
                let project: Project = self
                    .store
                    .lock()
                    .unwrap()
                    .db()
                    .get(strp(&p, "project_id")?)?;
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
                    let mut card = integration::upsert(
                        db,
                        &current,
                        Change { issue, comments },
                        true,
                        p["instructions"].as_str().unwrap_or(""),
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
                        crate::notes::relate_card(db, &note.id, &card, true)?;
                    }
                    Ok(serde_json::to_value(card)?)
                })
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
        let (project, cards, cache) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            (
                db.get::<Project>(project_id)?,
                db.list::<Card>()?,
                db.list::<GitHubIssueSnapshot>()?,
            )
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
                integration::fetch(&client, &project, &cards, &cache)
            });
        let error = fetched.as_ref().err().map(|e| format!("{e:#}"));
        let result = self.store.lock().unwrap().write(|db| match fetched {
            Ok(sync) => {
                ensure!(
                    self.github.generation() == account_generation,
                    "GitHub account changed; discarded stale sync"
                );
                let observations = sync
                    .changes
                    .iter()
                    .map(|c| serde_json::to_value(&c.issue))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                integration::apply(db, &project, &source.generation, sync)?;
                for item in observations {
                    events::github::observe(
                        db,
                        &source.repository,
                        item["number"].as_u64().context("Missing source number")?,
                        if item.get("pull_request").is_some() {
                            "pr"
                        } else {
                            "issue"
                        },
                        "overview",
                        &item,
                        false,
                    )?;
                }
                Ok(())
            }
            Err(ref e) => {
                let mut current: Project = db.get(project_id)?;
                if let Some(s) = &mut current.github
                    && s.generation == source.generation
                {
                    s.error = Some(format!("{e:#}"));
                    db.save(&current)?;
                }
                Ok(())
            }
        });
        self.emit("github", None);
        result?;
        if let Some(error) = error {
            bail!("{error}")
        }
        self.github_pr_intake(&project)?;
        self.github_event_refresh(&project)?;
        Ok(())
    }
}

impl AppServer {
    fn github_event_refresh(&self, project: &Project) -> Result<()> {
        let (cards, enabled) = {
            let store = self.store.lock().unwrap();
            (
                store.db().list::<Card>()?,
                store.db().list::<EventSubscription>()?.iter().any(|s| {
                    s.enabled
                        && (s.selector.types.is_empty()
                            || s.selector
                                .types
                                .iter()
                                .any(|t| t == "*" || t.starts_with("github.")))
                }),
            )
        };
        if !enabled {
            return Ok(());
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
                    events::github::complete_collection(
                        db,
                        &source.repository,
                        source.number,
                        if card.is_github_pr() { "pr" } else { "issue" },
                        section,
                        &items,
                    )
                })?;
            }
        }
        Ok(())
    }
}
