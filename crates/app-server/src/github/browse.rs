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
                            "message": format!("{e:#}"),
                        }))
                    }
                }
            }
            "github.cli.login" => (|| {
                ensure!(
                    canban_github::cli::Service::installed(),
                    "GitHub CLI (`gh`) is not installed. Install it first (e.g. `brew install gh`), then sign in."
                );
                let gh = canban_github::cli::Service::binary_path()
                    .to_string_lossy()
                    .into_owned();
                self.create_terminal(json!({
                    "machine": "local",
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
        let result = if notification {
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
            let login = self
                .github
                .status()
                .login
                .or_else(|| self.github_cli.login())
                .or_else(|| self.github_cli.identify().ok().map(|u| u.login))
                .context("Sign in on the GitHub page or run gh auth login")?;
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
                .or_else(|| self.github_cli.cached(&versioned));
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
                } else {
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
            let card = integration::upsert(
                db,
                &current,
                Change {
                    issue,
                    comments: Vec::new(),
                },
                true,
                p["instructions"].as_str().unwrap_or(""),
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
        let login = self
            .github
            .status()
            .login
            .or_else(|| self.github_cli.login())
            .unwrap_or_default();
        let sync = integration::pull_requests::fetch(&client, project, &cards, &login)?;
        let changes = sync.changes;
        if !changes.is_empty() {
            let pr_numbers: Vec<u64> = changes.iter().map(|c| c.issue.number).collect();
            self.store.lock().unwrap().write(|db| {
                ensure!(
                    self.github.generation() == generation,
                    "GitHub account changed"
                );
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
                integration::apply(
                    db,
                    project,
                    &source.generation,
                    integration::Sync { changes },
                )?;
                for item in observations {
                    events::github::observe(
                        db,
                        &source.repository,
                        item["number"].as_u64().context("Missing source number")?,
                        "pr",
                        "overview",
                        &item,
                        false,
                    )?;
                }
                Ok(())
            })?;
            for number in pr_numbers {
                if let Some(pr_value) =
                    client.cached::<Value>(&format!("/repos/{}/pulls/{number}", source.repository))
                    && pr_value["merged"] == true
                {
                    let _ = self.reconcile_remote_pr_merge(&source.repository, number, &pr_value);
                }
            }
            self.emit("github", None);
        }
        Ok(())
    }
    fn github_remote_action(&self, method: &str, p: &Value) -> Result<Value> {
        if let Some(receipt) = self.github_receipt(method, p)? {
            return Ok(receipt);
        }
        let _guard = self
            .github_action_gate
            .try_lock()
            .map_err(|_| anyhow::anyhow!("Another GitHub action is pending"))?;
        if let Some(receipt) = self.github_receipt(method, p)? {
            return Ok(receipt);
        }
        let key = strp(p, "request_key")?;
        let r = reference(p)?;
        ensure!(r.kind != "commit", "Commit discussion is read-only");
        let client = self.github_client()?.refreshing();
        let number = r.number.unwrap();
        let body = p["body"].as_str().unwrap_or("");
        let (http_method, path, payload) = if method == "github.remote.comment.update" {
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
                ),
                "threads" => {
                    ensure!(r.kind == "pr", "Only PRs support review thread comments");
                    (
                        "PATCH",
                        format!("/repos/{}/pulls/comments/{id}", r.repository),
                        json!({"body": body}),
                    )
                }
                "reviews" => {
                    ensure!(r.kind == "pr", "Only PRs support Reviews");
                    (
                        "PUT",
                        format!("/repos/{}/pulls/{number}/reviews/{id}", r.repository),
                        json!({"body": body}),
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
                ),
                "threads" => {
                    ensure!(r.kind == "pr", "Only PRs support review thread comments");
                    (
                        "DELETE",
                        format!("/repos/{}/pulls/comments/{id}", r.repository),
                        json!({}),
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
            let live: Value = client.get(&item_path(&r))?;
            ensure!(
                live["head"]["sha"] == head && live["state"] == "open",
                "PR changed; review the latest version before submitting"
            );
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
            )
        } else {
            ensure!(!body.trim().is_empty(), "Comment is empty");
            (
                "POST",
                format!("/repos/{}/issues/{number}/comments", r.repository),
                json!({"body":body}),
            )
        };
        self.mutate(method,p,|_db|Ok(json!({"state":"unknown","message":"Action recorded; reconcile on GitHub before retrying","request_key":key})))?;
        let result = match client.write(http_method, &path, &payload) {
            Ok(value) => {
                json!({"state":"published","id":value["id"],"url":value["html_url"],"request_key":key})
            }
            Err(e) => {
                json!({"state":if e.downcast_ref::<canban_github::HttpError>().is_some_and(|e|(400..500).contains(&e.0)&&e.0!=408){"failed"}else{"unknown"},"message":format!("{e:#}"),"request_key":key})
            }
        };
        self.store.lock().unwrap().db().conn.execute(
            "UPDATE operations SET result=?2 WHERE id=?1",
            params![key, serde_json::to_string(&result)?],
        )?;
        if result["state"] == "published" {
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
            self.github_sync_due.store(true, Ordering::SeqCst);
        }
        self.emit("github", None);
        Ok(result)
    }
    fn github_notification_action(&self, method: &str, p: &Value) -> Result<Value> {
        if let Some(receipt) = self.github_receipt(method, p)? {
            return Ok(receipt);
        }
        let _guard = self
            .github_action_gate
            .try_lock()
            .map_err(|_| anyhow::anyhow!("Another GitHub action is pending"))?;
        if let Some(receipt) = self.github_receipt(method, p)? {
            return Ok(receipt);
        }
        let key = strp(p, "request_key")?;
        let thread = strp(p, "thread_id")?;
        ensure!(
            !thread.is_empty() && thread.bytes().all(|b| b.is_ascii_digit()),
            "Invalid notification ID"
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
        self.store.lock().unwrap().db().conn.execute(
            "UPDATE operations SET result=?2 WHERE id=?1",
            params![key, serde_json::to_string(&result)?],
        )?;
        if result["state"] == "published" {
            self.github_cli
                .notification_changed(thread, method.ends_with("done"))?;
            self.github_sync_due.store(true, Ordering::SeqCst);
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
        object.insert("_used_at".into(), json!(now()));
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
        let _ = canban_context::atomic(&path, &serde_json::to_vec(&*warm).unwrap_or_default());
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
        if queries.is_empty() && self.github_cli.login().is_some() {
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
