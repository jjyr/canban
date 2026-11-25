//! GitHub browsing and explicit actions. Remote bodies live only in provider caches.
use super::*;
use canban_github::cache::Response;

fn reference(p: &Value) -> Result<GitHubReference> {
    let r: GitHubReference = serde_json::from_value(p["reference"].clone())?;
    canban_github::repository(&r.repository)?;
    ensure!(
        matches!(r.kind.as_str(), "issue" | "pr" | "commit"),
        "Unsupported GitHub reference"
    );
    if r.kind == "commit" {
        ensure!(
            r.sha.as_ref().is_some_and(
                |s| (7..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
            ),
            "Invalid commit SHA"
        );
    } else {
        ensure!(r.number.is_some_and(|n| n > 0), "Invalid Issue/PR number");
    }
    Ok(r)
}
fn item_path(r: &GitHubReference) -> String {
    match r.kind.as_str() {
        "pr" => format!("/repos/{}/pulls/{}", r.repository, r.number.unwrap()),
        "commit" => format!(
            "/repos/{}/commits/{}",
            r.repository,
            r.sha.as_deref().unwrap()
        ),
        _ => format!("/repos/{}/issues/{}", r.repository, r.number.unwrap()),
    }
}
fn item_reference(value: &Value) -> Option<GitHubReference> {
    if let Some(url) = value["html_url"].as_str() {
        return canban_github::parse_reference(url).ok();
    }
    // Notifications use an API subject URL. Repository identity comes from the envelope.
    let repository = value["repository"]["full_name"].as_str()?;
    let subject = value["subject"]["url"].as_str()?;
    let tail = subject.rsplit('/').next()?;
    let kind = match value["subject"]["type"].as_str()? {
        "Issue" => "issue",
        "PullRequest" => "pr",
        "Commit" => "commit",
        _ => return None,
    };
    let url = format!(
        "https://github.com/{repository}/{}/{tail}",
        match kind {
            "pr" => "pull",
            "commit" => "commit",
            _ => "issues",
        }
    );
    canban_github::parse_reference(&url).ok()
}
fn next_page(response: &Response) -> bool {
    response
        .link
        .as_ref()
        .is_some_and(|l| l.contains("rel=\"next\""))
}
impl AppServer {
    pub(super) fn github_browser_call(&self, method: &str, p: &Value) -> Option<Result<Value>> {
        Some(match method {
            "github.browse" => self.github_browse(p),
            "github.detail" => self.github_detail(p),
            "github.item.add" => self.github_item_add(p),
            "github.notification.read" | "github.notification.done" => {
                self.github_notification_action(method, p)
            }
            "github.reference.add" | "github.reference.remove" => self.github_reference(method, p),
            "github.cli.status" => {
                if self.github_transport.is_some() {
                    return Some(Ok(json!({
                        "state": "missing",
                        "installed": false,
                        "last_login": Value::Null,
                        "message": "GitHub CLI is disabled when a custom GitHub transport is configured",
                    })));
                }
                let had_identity = self.github_cli.identity().is_some();
                match self.github_cli.identify() {
                    Ok(u) => {
                        let has_unlinked = self
                            .store
                            .lock()
                            .unwrap()
                            .db()
                            .list::<Project>()
                            .unwrap_or_default()
                            .iter()
                            .any(|p| p.github.is_none() && !p.github_disconnected);
                        if !had_identity && has_unlinked {
                            self.reconcile_project_github_bindings();
                        }
                        Ok(json!({
                            "state": "connected",
                            "installed": true,
                            "login": u.login,
                            "account_matches": self.github.status().login.is_none_or(|login| login.eq_ignore_ascii_case(&u.login))
                        }))
                    }
                    Err(e) => {
                        let installed = canban_github::cli::Service::installed();
                        Ok(json!({
                            "state": if installed { "signed_out" } else { "missing" },
                            "installed": installed,
                            "last_login": self.github_cli.login(),
                            "message": format!("{e:#}"),
                        }))
                    }
                }
            }
            "github.cli.login" => (|| {
                ensure!(
                    self.github_transport.is_none(),
                    "GitHub CLI login is disabled when a custom GitHub transport is configured"
                );
                ensure!(
                    canban_github::cli::Service::installed(),
                    "GitHub CLI (`gh`) is not installed. Install it first (e.g. `brew install gh`), then sign in."
                );
                let gh = canban_github::cli::Service::binary_path()
                    .to_string_lossy()
                    .into_owned();
                self.create_terminal(json!({
                    "setup_argv": [gh, "auth", "login", "--hostname", "github.com"],
                    "request_key": p["request_key"],
                }))
            })(),
            "github.cache.status" => {
                Ok(json!({"bytes":self.github.cache.bytes()+self.github_cli.cache.bytes()}))
            }
            "github.cache.clear" => (|| {
                self.github.cache.clear()?;
                self.github_cli.clear()?;
                let legacy = self.home.join("cache/github/legacy");
                if legacy.exists() {
                    std::fs::remove_dir_all(legacy)?;
                }
                self.emit("github", None);
                Ok(json!({"cleared":true}))
            })(),
            "github.remote.comment"
            | "github.remote.comment.update"
            | "github.remote.comment.delete"
            | "github.remote.review"
            | "github.remote.reply" => self.github_remote_action(method, p),
            _ => return None,
        })
    }
    pub(crate) fn github_read(
        &self,
        path: &str,
        refresh: bool,
        notification: bool,
    ) -> Result<Response> {
        let generation = self.github.generation();
        let result = if self.github_transport.is_some() {
            // Fail closed to the injected HttpTransport (e.g. DST fake GitHub) — never spawn `gh`.
            let client = self.github_client()?;
            if refresh {
                client.refreshing().response(path)
            } else {
                client.response(path)
            }
        } else if notification {
            let value = self.github_cli.response(path, refresh)?;
            if let Some(login) = self.github.status().login {
                ensure!(
                    self.github_cli
                        .login()
                        .is_some_and(|l| l.eq_ignore_ascii_case(&login)),
                    "GitHub App and gh use different accounts; align the accounts before reading notifications"
                );
            }
            Ok(value)
        } else if self.github.status().state == "connected" {
            let client = self.github_client()?;
            if refresh {
                client.refreshing().response(path)
            } else {
                client.response(path)
            }
        } else {
            self.github_cli.response(path, refresh)
        };
        ensure!(
            self.github.generation() == generation,
            "GitHub account changed; discarded stale response"
        );
        if let Ok(response) = &result {
            self.observe_github_response(path, &response.data)?;
        }
        if result.as_ref().is_ok_and(|response| !response.cached) {
            self.emit("github", None);
        }
        result
    }
    fn github_browse(&self, p: &Value) -> Result<Value> {
        let kind = p["kind"].as_str().unwrap_or("notifications");
        let page = p["page"].as_u64().unwrap_or(1);
        ensure!((1..=1000).contains(&page), "Invalid page");
        let refresh = p["refresh"].as_bool().unwrap_or(false);
        let path = if kind == "notifications" {
            format!(
                "/notifications?all=true&is_done={}&participating={}&per_page=50&page={page}",
                p["state"] == "done",
                p["relation"] == "participating"
            )
        } else {
            ensure!(matches!(kind, "issues" | "prs"), "Invalid GitHub list");
            let login = if self.github_transport.is_some() {
                self.github.status().login.unwrap_or_else(|| "sim".into())
            } else {
                self.github
                    .status()
                    .login
                    .or_else(|| self.github_cli.login())
                    .or_else(|| self.github_cli.identify().ok().map(|u| u.login))
                    .context("Sign in on the GitHub page or run gh auth login")?
            };
            let mut query = format!("is:{}", if kind == "prs" { "pr" } else { "issue" });
            if let Some(repo) = p["repository"].as_str().filter(|s| !s.is_empty()) {
                query.push_str(&format!(" repo:{}", canban_github::repository(repo)?));
            }
            match p["relation"].as_str().unwrap_or("assigned") {
                "authored" => query.push_str(&format!(" author:{login}")),
                "review_requested" if kind == "prs" => {
                    query.push_str(&format!(" review-requested:{login}"))
                }
                "assigned" => query.push_str(&format!(" assignee:{login}")),
                "mentions" => query.push_str(&format!(" mentions:{login}")),
                _ => (),
            }
            match p["state"].as_str().unwrap_or("open") {
                "open" => query.push_str(" is:open"),
                "closed" => query.push_str(" is:closed"),
                "merged" if kind == "prs" => query.push_str(" is:merged"),
                _ => (),
            }
            if let Some(text) = p["query"].as_str() {
                query.push(' ');
                query.push_str(text);
            }
            format!(
                "/search/issues?q={}&sort=updated&order=desc&per_page=30&page={page}",
                canban_github::segment(&query)
            )
        };
        let response = self.github_read(&path, refresh, kind == "notifications")?;
        let snapshot = self.snapshot()?;
        let values = if kind == "notifications" {
            response.data.as_array()
        } else {
            response.data["items"].as_array()
        };
        let items:Vec<_>=values.into_iter().flatten().map(|v| {
            let r=item_reference(v);
            let cards:Vec<_>=snapshot.cards.iter().filter(|c|c.removed_at.is_none() && r.as_ref().is_some_and(|r|{
                c.github_issue().is_some_and(|s|s.repository.eq_ignore_ascii_case(&r.repository) && Some(s.number)==r.number && c.source_kind()==r.kind)
                || c.pull_request.as_ref().is_some_and(|pr|r.kind=="pr" && pr.repository.eq_ignore_ascii_case(&r.repository) && Some(pr.number)==r.number)
                || c.github_links.iter().any(|l|l.repository.eq_ignore_ascii_case(&r.repository) && l.kind==r.kind && l.number==r.number && l.sha==r.sha)
            })).map(|c|json!({"id":c.id,"number":c.number,"project_id":c.project_id,"archived":c.archived_at.is_some()})).collect();
            json!({"item":v,"reference":r,"cards":cards})
        }).collect();
        self.github_watch(&format!("list:{path}"), p, "browse");
        Ok(
            json!({"items":items,"page":page,"has_next":next_page(&response),"cached":response.cached,"validated_at":response.validated_at,"poll_interval":response.poll_interval}),
        )
    }
    fn github_detail(&self, p: &Value) -> Result<Value> {
        let r = reference(p)?;
        let refresh = p["refresh"].as_bool().unwrap_or(false);
        let page = p["page"].as_u64().unwrap_or(1);
        ensure!((1..=1000).contains(&page), "Invalid page");
        let section = p["section"].as_str().unwrap_or("overview");
        let root = item_path(&r);
        let path = match section {
            "overview" => root,
            "discussion" if r.kind != "commit" => format!(
                "/repos/{}/issues/{}/comments?per_page=30&page={page}",
                r.repository,
                r.number.unwrap()
            ),
            "files" if r.kind == "pr" => format!(
                "/repos/{}/pulls/{}/files?per_page=30&page={page}",
                r.repository,
                r.number.unwrap()
            ),
            "reviews" if r.kind == "pr" => format!(
                "/repos/{}/pulls/{}/reviews?per_page=30&page={page}",
                r.repository,
                r.number.unwrap()
            ),
            "threads" if r.kind == "pr" => format!(
                "/repos/{}/pulls/{}/comments?per_page=30&page={page}",
                r.repository,
                r.number.unwrap()
            ),
            "checks" if r.kind == "pr" => {
                let sha = strp(p, "head")?;
                ensure!(
                    sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
                    "Invalid head SHA"
                );
                format!(
                    "/repos/{}/commits/{sha}/check-runs?per_page=30&page={page}",
                    r.repository
                )
            }
            _ => bail!("Unsupported GitHub detail section"),
        };
        // A file-page cache is bound to both comparison refs. It must never drift under a review draft.
        let response = if section == "files" {
            let current = self.github_read(&item_path(&r), refresh, false)?;
            let expected = strp(p, "head")?;
            ensure!(
                current.data["head"]["sha"] == expected,
                "PR has a new version; refresh its overview before loading files"
            );
            let base = current.data["base"]["sha"]
                .as_str()
                .context("Missing PR base")?;
            let versioned = format!("{path}&canban_version={base}-{expected}");
            // Local version key is separate from the HTTP endpoint (see provider versioned response).
            let cache_client = self.github_cached_client().ok();
            let cached = cache_client
                .as_ref()
                .and_then(|c| c.cached_response(&versioned))
                .or_else(|| {
                    self.github_transport
                        .is_none()
                        .then(|| self.github_cli.cached(&versioned))
                        .flatten()
                });
            if !refresh && let Some(cached) = cached {
                cached
            } else {
                let value = self.github_read(&path, true, false)?;
                let after = self.github_read(&item_path(&r), true, false)?.data;
                ensure!(
                    after["head"]["sha"] == expected && after["base"]["sha"] == base,
                    "PR comparison changed while fetching files; refresh before reviewing"
                );
                if let Some(c) = cache_client {
                    c.seed_response(&versioned, &value)
                } else if self.github_transport.is_none() {
                    self.github_cli.seed(&versioned, &value)
                }
                value
            }
        } else {
            self.github_read(&path, refresh, false)?
        };
        self.github_watch(&format!("detail:{}:{}:{page}", r.url, section), p, "detail");
        Ok(
            json!({"data":response.data,"page":page,"has_next":next_page(&response),"validated_at":response.validated_at,"cached":response.cached}),
        )
    }
    fn github_reference(&self, method: &str, p: &Value) -> Result<Value> {
        let r = if method.ends_with("add") {
            canban_github::parse_reference(strp(p, "url")?)?
        } else {
            reference(p)?
        };
        self.mutate(method, p, |db| {
            let mut c: Card = db.get(strp(p, "card_id")?)?;
            revise(&c, p)?;
            ensure!(c.active(), "Restore the Card before editing its references");
            if method.ends_with("add") {
                if !c.github_links.contains(&r) {
                    c.github_links.push(r.clone());
                }
            } else {
                c.github_links.retain(|v| v != &r);
            }
            c.revision += 1;
            db.save(&c)?;
            log(
                db,
                &c,
                "github.reference",
                "user",
                "Updated GitHub reference",
                None,
                json!({"reference":r}),
                self.now_ms(),
            )?;
            Ok(serde_json::to_value(c)?)
        })
    }
    fn github_item_add(&self, p: &Value) -> Result<Value> {
        let r = reference(p)?;
        ensure!(
            r.kind != "commit",
            "Commits can be references, not source Cards"
        );
        let project: Project = self
            .store
            .lock()
            .unwrap()
            .db()
            .get(strp(p, "project_id")?)?;
        let source = project
            .github
            .as_ref()
            .context("Connect this Project to the repository first")?;
        ensure!(
            source.repository.eq_ignore_ascii_case(&r.repository),
            "Choose a Project connected to this repository"
        );
        let generation = self.github.generation();
        let client = self.github_client()?.refreshing();
        ensure!(
            client.repo(&r.repository)?.id == source.repository_id,
            "Repository identity changed"
        );
        let issue = client.item(&r.repository, r.number.unwrap())?;
        ensure!(
            (issue.pull_request.is_some()) == (r.kind == "pr"),
            "Source type changed; search again"
        );
        self.mutate("github.item.add", p, |db| {
            ensure!(
                self.github.generation() == generation,
                "GitHub account changed; add again"
            );
            let current: Project = db.get(&project.id)?;
            ensure!(
                current
                    .github
                    .as_ref()
                    .is_some_and(|s| s.generation == source.generation),
                "Project source changed; add again"
            );
            let card = integration::upsert_at(
                db,
                &current,
                Change {
                    issue,
                    comments: Vec::new(),
                },
                true,
                p["instructions"].as_str().unwrap_or(""),
                self.now_ms(),
            )?;
            Ok(serde_json::to_value(card)?)
        })
    }
    pub(super) fn github_pr_intake(&self, project: &Project) -> Result<()> {
        let Some(source) = &project.github else {
            return Ok(());
        };
        if source.paused {
            return Ok(());
        }
        let client = self.github_client()?.refreshing();
        let generation = self.github.generation();
        let cards = self.store.lock().unwrap().db().list::<Card>()?;
        let login = if self.github_transport.is_some() {
            self.github.status().login
        } else {
            self.github
                .status()
                .login
                .or_else(|| self.github_cli.login())
        }
        .unwrap_or_default();
        let sync = integration::pull_requests::fetch(&client, project, &cards, &login)?;
        let changes = sync.changes;
        if !changes.is_empty() {
            let pr_numbers: Vec<u64> = changes.iter().map(|c| c.issue.number).collect();
            // Provider cache reads and JSON conversion must happen before taking Store.
            let observations = changes
                .iter()
                .map(|c| {
                    Ok(client
                        .cached::<Value>(&format!(
                            "/repos/{}/pulls/{}",
                            source.repository, c.issue.number
                        ))
                        .unwrap_or(serde_json::to_value(&c.issue)?))
                })
                .collect::<Result<Vec<_>>>()?;
            self.github_apply_changes(project, generation, changes, observations, Some("pr"))?;
            for number in &pr_numbers {
                if let Some(pr_value) =
                    client.cached::<Value>(&format!("/repos/{}/pulls/{number}", source.repository))
                    && pr_value["merged"] == true
                {
                    let _ = self.reconcile_remote_pr_merge(&source.repository, *number, &pr_value);
                }
            }
            // Refresh CI checks for open external PR cards (baselining newly imported ones silently).
            let current_cards = self.store.lock().unwrap().db().list::<Card>()?;
            for card in current_cards.into_iter().filter(|c| {
                c.project_id == project.id
                    && c.active()
                    && !c.resolved()
                    && c.is_github_pr()
                    && c.pull_request.is_none()
            }) {
                let is_newly_added = card
                    .github_issue()
                    .is_some_and(|s| pr_numbers.contains(&s.number))
                    && !cards.iter().any(|old| old.id == card.id);
                let _ = self.refresh_external_pr_checks(&card, is_newly_added);
            }
            self.emit("github", None);
        }
        Ok(())
    }
    pub(crate) fn lock_github_action_gate(
        &self,
        timeout: Duration,
    ) -> Result<GithubActionGateGuard<'_>> {
        let timeout_ms = if timeout.is_zero() {
            0
        } else {
            u64::try_from(timeout.as_millis())
                .unwrap_or(u64::MAX)
                .max(1)
        };
        let deadline_ms = self.now_ms().saturating_add(timeout_ms);
        let mut busy = self.github_action_gate.0.lock().unwrap();
        while *busy {
            let remaining_ms = deadline_ms.saturating_sub(self.now_ms());
            if remaining_ms == 0 {
                bail!("Another GitHub action is pending (timed out waiting for lock)");
            }
            let (next_busy, wait_res) = self
                .github_action_gate
                .1
                .wait_timeout(busy, Duration::from_millis(remaining_ms))
                .unwrap();
            busy = next_busy;
            if wait_res.timed_out() && *busy {
                bail!("Another GitHub action is pending (timed out waiting for lock)");
            }
        }
        *busy = true;
        Ok(GithubActionGateGuard {
            gate: &self.github_action_gate,
        })
    }

    pub(super) fn github_review_publish(&self, p: &Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Review publishing is only permitted from a user action or an active workflow script step"
        );
        let (card, draft) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let card = super::resolve_card(db, p)?;
            if let Some(run_id) = p["workflow_run_id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let run: WorkflowRun = db
                    .get(run_id)
                    .context("Workflow run not found for review publish authorization")?;
                ensure!(
                    run.active() && run.state == "waiting_script",
                    "Workflow run is not currently executing a script step"
                );
                ensure!(
                    run.card_id.as_deref() == Some(card.id.as_str()),
                    "Workflow run does not belong to the target Card"
                );
                let active_step = run
                    .step
                    .as_deref()
                    .context("Workflow run has no active step")?;
                if let Some(step_id) = p["step_id"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    ensure!(
                        active_step == step_id,
                        "Workflow step `{step_id}` is not the active script step"
                    );
                }
                ensure!(
                    matches!(
                        run.definition.steps.get(active_step),
                        Some(WorkflowStep::Script { .. })
                    ),
                    "Review publish is only authorized for workflow script steps"
                );
            }
            let key = format!("review-draft:{}", card.id);
            let raw: Option<String> = db
                .conn
                .query_row(
                    "SELECT data FROM editor_drafts WHERE id=?1 AND data != 'null'",
                    [&key],
                    |r| r.get(0),
                )
                .optional()?;
            let draft: Option<ReviewDraft> = raw
                .map(|s| serde_json::from_str(&s))
                .transpose()
                .context("Stored review draft is malformed")?;
            (card, draft)
        };

        let Some(draft) = draft else {
            return Ok(json!({
                "state": "skipped",
                "reason": "No unpublished review draft",
                "card_id": card.id,
            }));
        };
        if draft.summary.trim().is_empty() && draft.comments.is_empty() {
            return Ok(json!({
                "state": "skipped",
                "reason": "Review draft is empty",
                "card_id": card.id,
            }));
        }

        let (repository, number) = if let Some(pr) = &card.pull_request {
            (pr.repository.clone(), pr.number)
        } else if card.is_github_pr()
            && let Some(source) = card.github_issue()
        {
            (source.repository.clone(), source.number)
        } else {
            bail!("Card is not linked to a GitHub pull request");
        };

        let event = if let Some(raw_override) = p["event_override"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match raw_override.to_ascii_uppercase().as_str() {
                "COMMENT" => "COMMENT",
                "APPROVE" => "APPROVE",
                "REQUEST_CHANGES" => "REQUEST_CHANGES",
                other => bail!(
                    "Invalid event_override `{other}` (expected COMMENT, APPROVE, or REQUEST_CHANGES)"
                ),
            }
        } else {
            match draft.verdict.trim() {
                v if v.eq_ignore_ascii_case("approve") => "APPROVE",
                v if v.eq_ignore_ascii_case("request_changes") => "REQUEST_CHANGES",
                _ => "COMMENT",
            }
        };

        let head = if let Some(sha) = draft
            .compare_sha
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            sha.to_owned()
        } else {
            let client = self.github_client()?.refreshing();
            let live: Value = client.get(&format!("/repos/{repository}/pulls/{number}"))?;
            live["head"]["sha"]
                .as_str()
                .filter(|s| !s.is_empty())
                .context("Unable to resolve PR head SHA")?
                .to_owned()
        };

        let comments: Vec<Value> = draft
            .comments
            .iter()
            .map(|c| {
                json!({
                    "path": c.path,
                    "line": c.line,
                    "side": c.side,
                    "body": c.body,
                })
            })
            .collect();
        let request_key = p["request_key"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| self.next_id());
        let review_payload = json!({
            "request_key": request_key,
            "card_id": card.id,
            "reference": {
                "kind": "pr",
                "repository": repository,
                "number": number,
            },
            "head": head,
            "event": event,
            "body": draft.summary,
            "comments": comments,
        });
        self.github_remote_action("github.remote.review", &review_payload)
    }

    fn github_remote_action(&self, method: &str, p: &Value) -> Result<Value> {
        if let Some(receipt) = self.github_receipt(method, p)? {
            return Ok(receipt);
        }
        let _guard = self.lock_github_action_gate(Duration::from_secs(30))?;
        if let Some(receipt) = self.github_receipt(method, p)? {
            return Ok(receipt);
        }
        let key = strp(p, "request_key")?;
        let r = reference(p)?;
        ensure!(r.kind != "commit", "Commit discussion is read-only");
        let number = r.number.unwrap();
        let body = p["body"].as_str().unwrap_or("");
        let (http_method, path, payload, review_head) = if method == "github.remote.comment.update"
        {
            ensure!(!body.trim().is_empty(), "Comment is empty");
            let id = p["comment_id"]
                .as_u64()
                .filter(|n| *n > 0)
                .context("Missing comment ID")?;
            let section = p["section"].as_str().unwrap_or("discussion");
            match section {
                "discussion" => (
                    "PATCH",
                    format!("/repos/{}/issues/comments/{id}", r.repository),
                    json!({"body": body}),
                    None,
                ),
                "threads" => {
                    ensure!(r.kind == "pr", "Only PRs support review thread comments");
                    (
                        "PATCH",
                        format!("/repos/{}/pulls/comments/{id}", r.repository),
                        json!({"body": body}),
                        None,
                    )
                }
                "reviews" => {
                    ensure!(r.kind == "pr", "Only PRs support Reviews");
                    (
                        "PUT",
                        format!("/repos/{}/pulls/{number}/reviews/{id}", r.repository),
                        json!({"body": body}),
                        None,
                    )
                }
                _ => bail!("Unsupported GitHub comment section"),
            }
        } else if method == "github.remote.comment.delete" {
            let id = p["comment_id"]
                .as_u64()
                .filter(|n| *n > 0)
                .context("Missing comment ID")?;
            let section = p["section"].as_str().unwrap_or("discussion");
            match section {
                "discussion" => (
                    "DELETE",
                    format!("/repos/{}/issues/comments/{id}", r.repository),
                    json!({}),
                    None,
                ),
                "threads" => {
                    ensure!(r.kind == "pr", "Only PRs support review thread comments");
                    (
                        "DELETE",
                        format!("/repos/{}/pulls/comments/{id}", r.repository),
                        json!({}),
                        None,
                    )
                }
                "reviews" => {
                    bail!("GitHub does not support deleting submitted pull request reviews")
                }
                _ => bail!("Unsupported GitHub comment section"),
            }
        } else if method == "github.remote.review" {
            ensure!(r.kind == "pr", "Only PRs support Review");
            let head = strp(p, "head")?;
            let event = strp(p, "event")?;
            ensure!(
                matches!(event, "COMMENT" | "APPROVE" | "REQUEST_CHANGES"),
                "Invalid Review decision"
            );
            let comments = p["comments"].as_array().cloned().unwrap_or_default();
            ensure!(
                event != "COMMENT" || !body.trim().is_empty() || !comments.is_empty(),
                "Add a Review summary or line comment"
            );
            ensure!(
                event != "REQUEST_CHANGES" || !body.trim().is_empty(),
                "Request changes requires a Review summary"
            );
            for c in &comments {
                ensure!(
                    c["path"].as_str().is_some()
                        && c["line"].as_u64().is_some_and(|n| n > 0)
                        && matches!(c["side"].as_str(), Some("LEFT" | "RIGHT"))
                        && c["body"].as_str().is_some_and(|s| !s.trim().is_empty()),
                    "Invalid line comment"
                );
            }
            (
                "POST",
                format!("/repos/{}/pulls/{number}/reviews", r.repository),
                json!({"body":body,"event":event,"commit_id":head,"comments":comments}),
                Some(head.to_owned()),
            )
        } else if method == "github.remote.reply" {
            ensure!(
                r.kind == "pr" && !body.trim().is_empty(),
                "A PR reply needs text"
            );
            let id = p["comment_id"]
                .as_u64()
                .context("Missing thread comment ID")?;
            (
                "POST",
                format!(
                    "/repos/{}/pulls/{number}/comments/{id}/replies",
                    r.repository
                ),
                json!({"body":body}),
                None,
            )
        } else {
            ensure!(!body.trim().is_empty(), "Comment is empty");
            (
                "POST",
                format!("/repos/{}/issues/{number}/comments", r.repository),
                json!({"body":body}),
                None,
            )
        };
        let client = self.github_client()?.refreshing();
        if let Some(head) = review_head {
            let live: Value = client.get(&item_path(&r))?;
            ensure!(
                live["head"]["sha"] == head && live["state"] == "open",
                "PR changed; review the latest version before submitting"
            );
        }
        self.mutate(method,p,|_db|Ok(json!({"state":"unknown","message":"Action recorded; reconcile on GitHub before retrying","request_key":key})))?;
        let result = match client.write(http_method, &path, &payload) {
            Ok(value) => {
                json!({"state":"published","id":value["id"],"url":value["html_url"],"request_key":key})
            }
            Err(e) => {
                json!({"state":if e.downcast_ref::<canban_github::HttpError>().is_some_and(|e|(400..500).contains(&e.0)&&e.0!=408){"failed"}else{"unknown"},"message":format!("{e:#}"),"request_key":key})
            }
        };
        self.store.lock().unwrap().write(|db| {
            db.conn.execute(
                "UPDATE operations SET result=?2 WHERE id=?1",
                params![key, serde_json::to_string(&result)?],
            )?;
            Ok(())
        })?;
        if result["state"] == "published" {
            if let Some(item_id) = result["id"].as_u64() {
                let receipt_kind = if method == "github.remote.review" {
                    Some("review")
                } else if matches!(method, "github.remote.comment" | "github.remote.reply") {
                    Some("comment")
                } else {
                    None
                };
                if let Some(kind) = receipt_kind {
                    let _ = self.store.lock().unwrap().write(|db| {
                        events::github::record_written_receipt(
                            db,
                            &r.repository,
                            kind,
                            item_id,
                            self.now_ms(),
                        )
                    });
                }
            }
            let page = p["page"].as_u64().filter(|n| *n > 1);
            for section in ["overview", "discussion", "reviews", "threads"] {
                if section == "overview" || section == "discussion" || r.kind == "pr" {
                    let _ = self
                        .github_detail(&json!({"reference":r,"section":section,"refresh":true}));
                    if let Some(page) = page
                        && section != "overview"
                    {
                        let _ = self.github_detail(
                            &json!({"reference":r,"section":section,"page":page,"refresh":true}),
                        );
                    }
                }
            }
            if r.kind == "pr"
                && (method == "github.remote.reply"
                    || method == "github.remote.review"
                    || ((method == "github.remote.comment.update"
                        || method == "github.remote.comment.delete")
                        && p["section"].as_str() == Some("threads")))
            {
                let _ = self.github_read(
                    &format!(
                        "/repos/{}/pulls/{number}/comments?per_page=100",
                        r.repository
                    ),
                    true,
                    false,
                );
            }
            if r.kind == "pr" && method == "github.remote.review" {
                let _ =
                    self.clear_review_draft_for_pr(&r.repository, number, p["card_id"].as_str());
            }
            self.trigger_github_sync();
        }
        self.emit("github", None);
        Ok(result)
    }
    fn github_notification_action(&self, method: &str, p: &Value) -> Result<Value> {
        if let Some(receipt) = self.github_receipt(method, p)? {
            return Ok(receipt);
        }
        let _guard = self.lock_github_action_gate(Duration::from_secs(30))?;
        if let Some(receipt) = self.github_receipt(method, p)? {
            return Ok(receipt);
        }
        let key = strp(p, "request_key")?;
        let thread = strp(p, "thread_id")?;
        ensure!(
            !thread.is_empty() && thread.bytes().all(|b| b.is_ascii_digit()),
            "Invalid notification ID"
        );
        ensure!(
            self.github_transport.is_none(),
            "GitHub notification CLI actions are disabled when a custom GitHub transport is configured"
        );
        let user = self.github_cli.identify()?;
        ensure!(
            self.github
                .status()
                .login
                .is_none_or(|l| l.eq_ignore_ascii_case(&user.login)),
            "GitHub App and gh use different accounts"
        );
        self.mutate(method,p,|_|Ok(json!({"state":"unknown","message":"Notification action recorded; refresh before retrying","request_key":key})))?;
        let result = match self.github_cli.mutate(
            if method.ends_with("done") {
                "DELETE"
            } else {
                "PATCH"
            },
            &format!("/notifications/threads/{thread}"),
        ) {
            Ok(_) => json!({"state":"published","request_key":key}),
            Err(e) => json!({"state":"unknown","message":format!("{e:#}"),"request_key":key}),
        };
        self.store.lock().unwrap().write(|db| {
            db.conn.execute(
                "UPDATE operations SET result=?2 WHERE id=?1",
                params![key, serde_json::to_string(&result)?],
            )?;
            Ok(())
        })?;
        if result["state"] == "published" {
            self.github_cli
                .notification_changed(thread, method.ends_with("done"))?;
            self.trigger_github_sync();
        }
        self.emit("github", None);
        Ok(result)
    }
    fn github_watch(&self, key: &str, p: &Value, operation: &str) {
        if p["_background"] == true {
            return;
        }
        let mut query = p.clone();
        let Some(object) = query.as_object_mut() else {
            return;
        };
        object.remove("refresh");
        object.insert("_operation".into(), json!(operation));
        object.insert("_used_at".into(), json!(self.now_ms()));
        let mut warm = self.github_queries.lock().unwrap();
        warm.insert(key.into(), query);
        while warm.len() > 12 {
            if let Some(k) = warm
                .iter()
                .min_by_key(|(_, p)| p["_used_at"].as_u64().unwrap_or(0))
                .map(|(k, _)| k.clone())
            {
                warm.remove(&k);
            }
        }
        let path = self.home.join("cache/github/queries.json");
        let _ = canban_context::atomic_with_durability(
            &path,
            &serde_json::to_vec(&*warm).unwrap_or_default(),
            self.fs_durability,
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
    pub(crate) fn github_refresh_queries(&self) {
        let mut queries: Vec<_> = self
            .github_queries
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        if queries.is_empty()
            && self.github_transport.is_none()
            && self.github_cli.login().is_some()
        {
            queries.push(json!({"kind":"notifications","state":"inbox","relation":"all","page":1}));
        }
        for mut p in queries {
            p["refresh"] = json!(true);
            p["_background"] = json!(true);
            if p["_operation"] == "detail" {
                let _ = self.github_detail(&p);
            } else {
                let _ = self.github_browse(&p);
            }
        }
        self.emit("github", None);
    }
}

#[cfg(test)]
mod fail_closed_tests {
    use super::*;
    use canban_github::{HttpRequest, HttpResponse, HttpTransport};
    use std::sync::{Arc, Mutex};

    struct RecordingTransport {
        seen: Mutex<Vec<(String, String)>>,
    }

    impl HttpTransport for RecordingTransport {
        fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
            self.seen
                .lock()
                .unwrap()
                .push((req.method.clone(), req.path.clone()));
            if req.path.starts_with("/notifications") {
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: br#"[{"id":"n1","unread":true}]"#.to_vec(),
                })
            } else if req.path == "/repos/acme/app/issues/1" {
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: br#"{"number":1,"title":"Mock issue"}"#.to_vec(),
                })
            } else {
                anyhow::bail!("simulated transport failure for {}", req.path)
            }
        }
    }

    #[test]
    fn github_read_routes_through_injected_transport_and_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let transport = Arc::new(RecordingTransport {
            seen: Mutex::new(Vec::new()),
        });
        let env = ServerRuntimeEnv {
            github_transport: Some(transport.clone()),
            fs_durability: FsDurability::NoSync,
            sqlite_synchronous_off: true,
            disable_os_watchers: true,
            ..Default::default()
        };
        let app = AppServer::open_with_env(home, PathBuf::from("/bin/echo"), env).unwrap();

        // Standard read with unauthenticated App state must route through github_transport, not `gh`.
        let issue = app
            .github_read("/repos/acme/app/issues/1", true, false)
            .unwrap();
        assert_eq!(issue.data["title"], "Mock issue");

        // Notification read must also route through github_transport and never spawn `gh`.
        let notifications = app
            .github_read("/notifications?all=true", true, true)
            .unwrap();
        assert_eq!(notifications.data[0]["id"], "n1");

        // Missing/failing route must fail closed rather than falling back to `gh`.
        let err = app
            .github_read("/repos/acme/app/issues/404", true, false)
            .unwrap_err();
        assert!(
            err.to_string().contains("simulated transport failure"),
            "unexpected error: {err:#}"
        );

        // `github.cli.status` must report disabled/missing without invoking `gh`.
        let cli_status = app.call("github.cli.status", json!({})).unwrap();
        assert_eq!(cli_status["installed"], false);
        assert_eq!(cli_status["state"], "missing");

        // `card.create` must not invoke `gh` for author login.
        let project: Project = serde_json::from_value(
            app.call(
                "project.create",
                json!({"name": "P", "directory": dir.path().join("repo")}),
            )
            .unwrap(),
        )
        .unwrap();
        let card: Card = serde_json::from_value(
            app.call(
                "card.create",
                json!({"view": "full", "project_id": project.id, "title": "T"}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(card.created_by, "Local");

        let seen = transport.seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].1, "/repos/acme/app/issues/1");
        assert_eq!(seen[1].1, "/notifications?all=true");
        assert_eq!(seen[2].1, "/repos/acme/app/issues/404");
    }
}
