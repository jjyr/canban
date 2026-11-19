use super::*;
use canban_kanban::github::delivery as policy;
impl AppServer {
    fn snapshot_card_for_delivery(&self, db: Db<'_>, id: &str) -> Result<Card> {
        let c: Card = db.get(id)?;
        let mut snapshot = Snapshot {
            cards: vec![c],
            ..Default::default()
        };
        self.project_github(&mut snapshot);
        Ok(snapshot.cards.remove(0))
    }

    pub(super) fn github_publish(&self, p: Value) -> Result<Value> {
        let card_id = strp(&p, "card_id")?.to_owned();
        let _guard = self.try_lock_card_delivery(&card_id, "GitHub PR publish")?;
        let key = strp(&p, "request_key")?.to_owned();
        if let Some(receipt) = self.github_receipt("github.pr.publish", &p)? {
            return Ok(receipt);
        }
        let (card, project) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let c = self.snapshot_card_for_delivery(db, strp(&p, "card_id")?)?;
            let project: Project = db.get(&c.project_id)?;
            (c.clone(), self.bound_project(&project, &c))
        };
        revise(&card, &p)?;
        ensure!(card.active(), "Restore the Card before delivery");
        ensure!(
            card.pull_request
                .as_ref()
                .is_none_or(|pr| pr.state == "not_published"
                    || (pr.number > 0
                        && (pr.state == "open" || pr.state == "closed" || pr.merged))),
            "Reconcile the uncertain PR publication before publishing again"
        );
        let client = self.github_client()?.refreshing();
        let (repository, repository_id) =
            if let Some(pr) = card.pull_request.as_ref().filter(|p| !p.merged) {
                (pr.repository.clone(), pr.repository_id)
            } else {
                let source = project
                    .github
                    .as_ref()
                    .context("Connect this Project to GitHub first")?;
                (source.repository.clone(), source.repository_id)
            };
        ensure!(
            client.repo(&repository)?.id == repository_id,
            "Delivery repository identity changed"
        );
        let ops = self.ops(&project.work_location().machine);
        let push_repo = canban_github::remote_repository(&ops.push_remote(&project, &card)?)?;
        let push_info: Value = client.get(&format!("/repos/{push_repo}"))?;
        ensure!(
            push_info["id"].as_u64() == Some(repository_id)
                || push_info["parent"]["id"].as_u64() == Some(repository_id),
            "Push remote is not the target repository or its fork"
        );
        let plan = ops.review_preview(&project, &card, p["target_branch"].as_str())?;
        ensure!(
            plan.token == strp(&p, "token")?,
            "Review is stale; refresh before publishing"
        );
        ensure!(
            plan.merge_allowed,
            "{}",
            plan.reason.as_deref().unwrap_or("Delivery unavailable")
        );
        if let Some(pr) = card
            .pull_request
            .as_ref()
            .filter(|pr| pr.number > 0 && pr.state == "open")
        {
            ensure!(
                plan.target_branch == pr.base_branch,
                "Existing PR target is pinned; resolve it before changing delivery targets"
            );
        }
        let branch = card.branch.as_deref().context("Card has no branch")?;
        let head_branch = format!("{}:{branch}", push_repo.split('/').next().unwrap());
        let workspace = card
            .workspace
            .clone()
            .context("Card has no saved workspace")?;
        let pending = GitHubPr {
            review_summary: String::new(),
            repository: repository.clone(),
            repository_id,
            number: card
                .pull_request
                .as_ref()
                .filter(|pr| pr.state == "open")
                .map(|pr| pr.number)
                .unwrap_or(0),
            url: card
                .pull_request
                .as_ref()
                .filter(|pr| pr.state == "open")
                .map(|pr| pr.url.clone())
                .unwrap_or_default(),
            state: "publishing".into(),
            head: plan.card_head.clone(),
            head_branch: head_branch.clone(),
            base_branch: plan.target_branch.clone(),
            merged: false,
            checks: "unknown".into(),
            checks_summary: None,
            workspace,
        };
        let mut execute = false;
        let receipt=self.mutate("github.pr.publish",&p,|db|{
            let mut c:Card=db.get(&card.id)?;revise(&c,&p)?;
            if let Some(previous)=c.pull_request.take() && card.pull_request.as_ref().is_some_and(|p|p.merged||p.state=="closed") {c.pull_request_history.push(previous);}
            c.delivery_mode=Some(DeliveryMode::GitHubPr);c.pull_request=Some(pending.clone());c.revision+=1;db.save(&c)?;
            log(db,&c,"github.pr_publish_started","user","Accepted reviewed changes for PR publication",None,json!({"request_key":key,"head":plan.card_head,"target":plan.target_branch}))?;
            execute=true;Ok(json!({"state":"unknown","message":"Publication intent saved; refresh PR state before retrying","request_key":key}))
        })?;
        if !execute {
            return Ok(receipt);
        }
        let result = (|| -> Result<GitHubPr> {
            let message = p["message"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Canban #{}: {}", card.number, card.title));
            let head = ops.publish(&project, &card, &plan.target_branch, &plan.token, &message)?;
            self.store.lock().unwrap().write(|db| {
                let mut c: Card = db.get(&card.id)?;
                if let Some(pr) = &mut c.pull_request {
                    pr.head = head.clone();
                }
                db.save(&c)
            })?;
            let found: Vec<Value> = client.get(&format!(
                "/repos/{repository}/pulls?state=open&head={}&base={}",
                canban_github::segment(&head_branch),
                canban_github::segment(&plan.target_branch)
            ))?;
            ensure!(found.len() <= 1, "Multiple matching PRs; inspect delivery");
            let remote = if let Some(pr) = found.first() {
                pr.clone()
            } else {
                client.create_pr(
                    &repository,
                    &head_branch,
                    &plan.target_branch,
                    &card.title,
                    &format!("Canban Card #{}\n\n{}", card.number, card.body),
                )?
            };
            let pr = policy::project(&remote, &pending)?;
            client.seed(&format!("/repos/{repository}/pulls/{}", pr.number), remote);
            ensure!(
                pr.head == head,
                "Published PR head changed; refresh and review"
            );
            Ok(pr)
        })();
        let value = self.store.lock().unwrap().write(|db| {
            let mut c: Card = db.get(&card.id)?;
            let value = match result {
                Ok(pr) => {
                    let result =
                        json!({"state":"published","url":pr.url,"number":pr.number,"head":pr.head});
                    c.pull_request = Some(pr);
                    c.attention = None;
                    log(
                        db,
                        &c,
                        "github.pr_published",
                        "user",
                        "Delivery PR published",
                        None,
                        result.clone(),
                    )?;
                    result
                }
                Err(e) => {
                    let message = format!("{e:#}");
                    if let Some(pr) = &mut c.pull_request {
                        pr.state = "unknown".into();
                    }
                    attention(db, &mut c, &message)?;
                    log(
                        db,
                        &c,
                        "github.pr_publish_unknown",
                        "system",
                        &message,
                        None,
                        json!({"request_key":key}),
                    )?;
                    json!({"state":"unknown","message":message,"request_key":key})
                }
            };
            c.revision += 1;
            db.save(&c)?;
            db.conn.execute(
                "UPDATE operations SET result=?2 WHERE id=?1",
                params![key, serde_json::to_string(&value)?],
            )?;
            Ok(value)
        })?;
        self.github_sync_due.store(true, Ordering::SeqCst);
        self.emit("github", Some(card.id));
        Ok(value)
    }
    pub(crate) fn github_pr_refresh(&self, p: Value) -> Result<Value> {
        let mut card: Card = self.store.lock().unwrap().db().get(strp(&p, "card_id")?)?;
        if card.pull_request.is_none() && card.is_github_pr() {
            return self.refresh_external_pr_checks(&card, false);
        }
        if card.pull_request.is_none()
            && card.branch.as_deref().is_some_and(|b| !b.trim().is_empty())
            && self.discover_branch_pr_for_card(&card)?
        {
            card = self.store.lock().unwrap().db().get(&card.id)?;
        }
        let old = card
            .pull_request
            .as_ref()
            .context("Card has no delivery PR")?;
        let account_generation = self.github.generation();
        let client = self.github_client()?.refreshing();
        ensure!(
            client.repo(&old.repository)?.id == old.repository_id,
            "Delivery repository identity changed"
        );
        let number = if old.number > 0 {
            old.number
        } else {
            let values: Vec<Value> = client.pages(&format!(
                "/repos/{}/pulls?state=all&head={}&base={}",
                old.repository,
                canban_github::segment(&old.head_branch),
                canban_github::segment(&old.base_branch)
            ))?;
            let values: Vec<_> = values
                .into_iter()
                .filter(|v| {
                    !card
                        .pull_request_history
                        .iter()
                        .any(|p| Some(p.number) == v["number"].as_u64())
                })
                .filter(|v| v["state"] == "open" || v["head"]["sha"] == old.head)
                .collect();
            if values.is_empty() {
                let mut pending = old.clone();
                pending.state = "not_published".into();
                self.store.lock().unwrap().write(|db| {
                    ensure!(
                        self.github.generation() == account_generation,
                        "GitHub account changed; refresh again"
                    );
                    let mut c: Card = db.get(&card.id)?;
                    ensure!(
                        c.pull_request.as_ref() == Some(old),
                        "PR changed; refresh again"
                    );
                    if c.pull_request.as_ref() != Some(&pending) {
                        c.pull_request = Some(pending.clone());
                        c.attention = Some(
                            "No delivery PR found. Refresh the diff and explicitly publish again."
                                .into(),
                        );
                        c.revision += 1;
                        db.save(&c)?;
                        log(
                            db,
                            &c,
                            "github.pr_not_found",
                            "github",
                            "No matching delivery PR; explicit publication is available",
                            None,
                            json!({}),
                        )?;
                    }
                    Ok(())
                })?;
                self.emit("github", Some(card.id));
                return Ok(serde_json::to_value(pending)?);
            }
            ensure!(
                values.len() == 1,
                "Publication is uncertain: multiple matching PRs. Inspect delivery before retrying"
            );
            values[0]["number"].as_u64().context("Invalid PR")?
        };
        let remote: Value = client.get(&format!("/repos/{}/pulls/{number}", old.repository))?;
        client.seed(
            &format!("/repos/{}/pulls/{number}", old.repository),
            remote.clone(),
        );
        let mut next = policy::project(&remote, old)?;
        let checks_summary =
            fetch_pr_checks_summary(&client, &old.repository, &next.url, &next.head)?;
        next.checks = checks_summary.state.clone();
        next.checks_summary = Some(checks_summary.clone());
        let reviews: Vec<Value> =
            client.pages(&format!("/repos/{}/pulls/{number}/reviews", old.repository))?;

        let mut latest = std::collections::HashMap::new();
        for r in &reviews {
            if matches!(
                r["state"].as_str(),
                Some("APPROVED" | "CHANGES_REQUESTED" | "DISMISSED")
            ) {
                latest.insert(
                    r["user"]["id"].to_string(),
                    r["state"].as_str().unwrap_or(""),
                );
            }
        }
        next.review_summary = format!(
            "{} approvals · {} changes requested",
            latest.values().filter(|s| **s == "APPROVED").count(),
            latest
                .values()
                .filter(|s| **s == "CHANGES_REQUESTED")
                .count()
        );
        client.seed(
            &format!(
                "/repos/{}/pulls/{}/canban-summary",
                next.repository, next.number
            ),
            json!({
                "checks": next.checks,
                "checks_summary": checks_summary,
                "reviews": next.review_summary
            }),
        );
        let prev_activity_sha = self.previous_pr_activity_sha(next.repository_id, next.number);
        let activity_payload = fetch_pr_activity(
            &client,
            &next.repository,
            next.number,
            &next.head,
            prev_activity_sha.as_deref(),
            Some(&reviews),
        );
        seed_pr_activity_cache(&client, &next.repository, next.number, &activity_payload);
        let current_login = self
            .github
            .status()
            .login
            .or_else(|| self.github_cli.login());
        let pr_draft = remote["draft"].as_bool().unwrap_or(false);
        let mut newly_merged = false;
        let mut ci_event_emitted = false;
        let mut activity_emitted = false;
        let pr_open = next.state == "open";
        self.store.lock().unwrap().write(|db| {
            ensure!(
                self.github.generation() == account_generation,
                "GitHub account changed; discarded stale PR refresh"
            );
            let mut current: Card = db.get(&card.id)?;
            ensure!(
                current.pull_request.as_ref() == Some(old),
                "PR binding changed during refresh; refresh again"
            );
            let before = current.clone();
            if old.number == 0 {
                let mut binding = old.clone();
                binding.number = next.number;
                binding.url = next.url.clone();
                binding.state = "published".into();
                current.pull_request = Some(binding);
            }
            if current.delivery_mode.is_none()
                && let Some(pr) = &mut current.pull_request
            {
                pr.head = next.head.clone();
                pr.base_branch = next.base_branch.clone();
            }
            if next.merged
                && let Some(pr) = &mut current.pull_request
            {
                pr.merged = true;
                pr.state = "closed".into();
            }
            if next.merged
                && current
                    .delivery
                    .as_ref()
                    .is_none_or(|d| d.request_key != format!("github:{}:{}", next.repository_id, next.number))
            {
                newly_merged = true;
                let resolved_at = now();
                current.delivery = Some(Delivery {
                    target_branch: next.base_branch.clone(),
                    commit: remote["merge_commit_sha"]
                        .as_str()
                        .unwrap_or(&next.head)
                        .into(),
                    request_key: format!("github:{}:{}", next.repository_id, next.number),
                    merged_at: resolved_at,
                });
                if current.resolution.is_none() {
                    current.resolution = Some(CardResolution {
                        resolved_at,
                        kind: "merged".into(),
                        reason: None,
                        actor: "github".into(),
                    });
                }
                current.attention = None;
                if let Some(gate) = current.human_gate.clone()
                    && let Some(id) = gate.terminal_id
                {
                    let att: TerminalEntry = db.get(&id)?;
                    if att.agent.as_ref().is_some_and(|a| a.state == "completed") {
                        current.human_gate = None;
                        crate::workflow::advance(db, &self.config(), &mut current, Some(&att))?;
                    }
                }
                log(
                    db,
                    &current,
                    "github.pr_delivered",
                    "github",
                    "Delivery PR merged",
                    None,
                    json!({"number": next.number, "commit": current.delivery.as_ref().map(|d| &d.commit)}),
                )?;
            } else {
                ci_event_emitted = record_pr_ci_observation(
                    db,
                    &mut current,
                    &next.repository,
                    next.repository_id,
                    next.number,
                    &next.url,
                    pr_open,
                    next.merged,
                    &checks_summary,
                    false,
                )?;
                activity_emitted = record_pr_activity_updates(
                    db,
                    &mut current,
                    &next.repository,
                    next.repository_id,
                    next.number,
                    &next.url,
                    &next.state,
                    pr_draft,
                    next.merged,
                    &next.head,
                    &activity_payload,
                    current_login.as_deref(),
                )?;
                if next.state == "closed" && !next.merged {
                    current.attention = Some("Delivery PR was closed without merging".into());
                } else if current.delivery_mode == Some(DeliveryMode::GitHubPr)
                    && next.head != old.head
                    && old.number > 0
                    && next.checks != "failure"
                    && !current
                        .attention
                        .as_deref()
                        .is_some_and(is_review_attention_message)
                {
                    current.attention =
                        Some("GitHub PR head changed; review the latest commits".into());
                }
            }
            if current != before {
                current.revision += 1;
                db.save(&current)?;
            }
            if ci_event_emitted || activity_emitted {
                crate::events::pump(db, &self.config())?;
            }
            Ok(())
        })?;
        if newly_merged {
            self.sync_merged_target(&card, Some(&next.base_branch));
        }
        if next.merged {
            let _ = self.try_auto_archive_resolved(&card.id);
        }
        if ci_event_emitted || activity_emitted {
            self.wake_runtime();
            self.request_scripts();
            self.emit("application", Some(card.id.clone()));
        }
        self.emit("github", Some(card.id));
        Ok(serde_json::to_value(next)?)
    }

    pub(crate) fn refresh_external_pr_checks(
        &self,
        card: &Card,
        baseline_if_unseen: bool,
    ) -> Result<Value> {
        let source = card
            .github_issue()
            .filter(|_| card.is_github_pr())
            .context("Card is not an external GitHub PR")?;
        let account_generation = self.github.generation();
        let client = self.github_client()?.refreshing();
        ensure!(
            client.repo(&source.repository)?.id == source.repository_id,
            "GitHub repository identity changed"
        );
        let remote: Value = client.get(&format!(
            "/repos/{}/pulls/{}",
            source.repository, source.number
        ))?;
        client.seed(
            &format!("/repos/{}/pulls/{}", source.repository, source.number),
            remote.clone(),
        );
        let head_sha = remote["head"]["sha"]
            .as_str()
            .context("Invalid PR head")?
            .to_owned();
        let pr_url = remote["html_url"]
            .as_str()
            .unwrap_or(&source.url)
            .to_owned();
        let pr_state = remote["state"].as_str().unwrap_or("open").to_owned();
        let pr_draft = remote["draft"].as_bool().unwrap_or(false);
        let pr_open = pr_state == "open";
        let pr_merged =
            remote["merged"].as_bool().unwrap_or(false) || remote["merged_at"].is_string();
        let checks_summary =
            fetch_pr_checks_summary(&client, &source.repository, &pr_url, &head_sha)?;
        client.seed(
            &format!(
                "/repos/{}/pulls/{}/canban-summary",
                source.repository, source.number
            ),
            json!({
                "checks": checks_summary.state,
                "checks_summary": checks_summary,
                "reviews": "",
            }),
        );
        let prev_activity_sha = self.previous_pr_activity_sha(source.repository_id, source.number);
        let activity_payload = fetch_pr_activity(
            &client,
            &source.repository,
            source.number,
            &head_sha,
            prev_activity_sha.as_deref(),
            None,
        );
        seed_pr_activity_cache(
            &client,
            &source.repository,
            source.number,
            &activity_payload,
        );
        let current_login = self
            .github
            .status()
            .login
            .or_else(|| self.github_cli.login());
        if pr_merged {
            let _ = self.reconcile_remote_pr_merge(&source.repository, source.number, &remote);
        }
        let mut ci_event_emitted = false;
        let mut activity_emitted = false;
        self.store.lock().unwrap().write(|db| {
            ensure!(
                self.github.generation() == account_generation,
                "GitHub account changed; discarded stale PR refresh"
            );
            let mut current: Card = db.get(&card.id)?;
            let before = current.clone();
            ci_event_emitted = record_pr_ci_observation(
                db,
                &mut current,
                &source.repository,
                source.repository_id,
                source.number,
                &pr_url,
                pr_open,
                pr_merged,
                &checks_summary,
                baseline_if_unseen,
            )?;
            activity_emitted = record_pr_activity_updates(
                db,
                &mut current,
                &source.repository,
                source.repository_id,
                source.number,
                &pr_url,
                &pr_state,
                pr_draft,
                pr_merged,
                &head_sha,
                &activity_payload,
                current_login.as_deref(),
            )?;
            if current != before {
                current.revision += 1;
                db.save(&current)?;
            }
            if ci_event_emitted || activity_emitted {
                crate::events::pump(db, &self.config())?;
            }
            Ok(())
        })?;
        if ci_event_emitted || activity_emitted {
            self.wake_runtime();
            self.request_scripts();
            self.emit("application", Some(card.id.clone()));
        }
        self.emit("github", Some(card.id.clone()));
        Ok(serde_json::to_value(checks_summary)?)
    }

    fn previous_pr_activity_sha(&self, repository_id: u64, pr_number: u64) -> Option<String> {
        let obs_id = format!("pr-activity:{repository_id}:{pr_number}");
        let raw: Option<String> = self
            .store
            .lock()
            .unwrap()
            .db()
            .conn
            .query_row(
                "SELECT data FROM source_observations WHERE id=?1",
                [&obs_id],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        raw.and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v["head_sha"].as_str().map(str::to_owned))
            .filter(|s| !s.is_empty())
    }

    pub(crate) fn github_discover_branch_prs(&self, project: &Project) -> Result<()> {
        let Some(source) = &project.github else {
            return Ok(());
        };
        if source.paused {
            return Ok(());
        }
        let candidates: Vec<Card> = self
            .store
            .lock()
            .unwrap()
            .db()
            .list::<Card>()?
            .into_iter()
            .filter(|c| {
                c.project_id == project.id
                    && c.active()
                    && !c.resolved()
                    && !c.is_github_pr()
                    && c.pull_request.is_none()
                    && c.branch.as_deref().is_some_and(|b| !b.trim().is_empty())
            })
            .collect();
        if candidates.is_empty() {
            return Ok(());
        }
        let account_generation = self.github.generation();
        let client = self.github_client()?.refreshing();
        ensure!(
            client.repo(&source.repository)?.id == source.repository_id,
            "GitHub repository identity changed"
        );
        let open_prs: Vec<Value> = client.get(&format!(
            "/repos/{}/pulls?state=open&sort=updated&direction=desc&per_page=100",
            source.repository
        ))?;
        let push_owner = self.project_push_owner(project, &source.repository);
        for card in candidates {
            if self.stopped() {
                break;
            }
            let branch = card.branch.as_deref().unwrap_or("");
            let excluded: Vec<u64> = card.pull_request_history.iter().map(|p| p.number).collect();
            let matched = integration::pull_requests::match_branch_pr(
                &open_prs,
                source.repository_id,
                branch,
                &excluded,
                push_owner.as_deref(),
                None,
            )
            .cloned()
            .or_else(|| {
                if card.worktree.is_none() || !self.should_probe_branch_history(&card.id, branch) {
                    return None;
                }
                let local_head_sha = self
                    .ops(&project.work_location().machine)
                    .card_head_sha(project, &card)?;
                let head_owner = push_owner
                    .as_deref()
                    .or_else(|| source.repository.split('/').next())?;
                let all_prs: Vec<Value> = client
                    .get(&format!(
                        "/repos/{}/pulls?state=all&head={}&per_page=10",
                        source.repository,
                        canban_github::segment(&format!("{head_owner}:{branch}"))
                    ))
                    .ok()?;
                integration::pull_requests::match_branch_pr(
                    &all_prs,
                    source.repository_id,
                    branch,
                    &excluded,
                    push_owner.as_deref(),
                    Some(&local_head_sha),
                )
                .cloned()
            });
            let Some(pr_item) = matched else {
                continue;
            };
            let Some(number) = pr_item["number"].as_u64() else {
                continue;
            };
            let full_pr: Value =
                client.get(&format!("/repos/{}/pulls/{number}", source.repository))?;
            client.seed(
                &format!("/repos/{}/pulls/{number}", source.repository),
                full_pr.clone(),
            );
            let Some(linked) = integration::pull_requests::build_linked_pr(
                project,
                &card,
                source.repository_id,
                &source.repository,
                &full_pr,
            ) else {
                continue;
            };
            let saved = self.store.lock().unwrap().write(|db| {
                ensure!(
                    self.github.generation() == account_generation,
                    "GitHub account changed; discarded stale branch PR discovery"
                );
                let mut current: Card = db.get(&card.id)?;
                if !current.active()
                    || current.pull_request.is_some()
                    || current.branch.as_deref() != Some(branch)
                    || current
                        .pull_request_history
                        .iter()
                        .any(|p| p.number == linked.number)
                {
                    return Ok(false);
                }
                current.pull_request = Some(linked.clone());
                current.revision += 1;
                db.save(&current)?;
                log(
                    db,
                    &current,
                    "github.pr_linked",
                    "github",
                    &format!("Linked PR #{} from branch {branch}", linked.number),
                    None,
                    json!({
                        "origin": "github",
                        "repository": linked.repository,
                        "repository_id": linked.repository_id,
                        "number": linked.number,
                        "url": linked.url,
                        "branch": branch,
                        "head": linked.head,
                    }),
                )?;
                Ok(true)
            })?;
            if saved {
                self.emit("github", Some(card.id.clone()));
                let _ = self.github_pr_refresh(json!({"card_id": card.id}));
            }
        }
        Ok(())
    }

    fn discover_branch_pr_for_card(&self, card: &Card) -> Result<bool> {
        let Some(branch) = card.branch.as_deref().filter(|b| !b.trim().is_empty()) else {
            return Ok(false);
        };
        if !card.active() || card.is_github_pr() || card.pull_request.is_some() {
            return Ok(false);
        }
        let project: Project = self.store.lock().unwrap().db().get(&card.project_id)?;
        let Some(source) = &project.github else {
            return Ok(false);
        };
        let account_generation = self.github.generation();
        let client = self.github_client()?.refreshing();
        ensure!(
            client.repo(&source.repository)?.id == source.repository_id,
            "GitHub repository identity changed"
        );
        let push_owner = self.project_push_owner(&project, &source.repository);
        let head_owner = push_owner
            .as_deref()
            .or_else(|| source.repository.split('/').next())
            .unwrap_or("");
        let excluded: Vec<u64> = card.pull_request_history.iter().map(|p| p.number).collect();
        let open_prs: Vec<Value> = client.get(&format!(
            "/repos/{}/pulls?state=open&head={}&per_page=10",
            source.repository,
            canban_github::segment(&format!("{head_owner}:{branch}"))
        ))?;
        let mut matched = integration::pull_requests::match_branch_pr(
            &open_prs,
            source.repository_id,
            branch,
            &excluded,
            push_owner.as_deref(),
            None,
        )
        .cloned();
        if matched.is_none() {
            let local_head_sha = self
                .ops(&project.work_location().machine)
                .card_head_sha(&project, card);
            if let Some(sha) = local_head_sha {
                let all_prs: Vec<Value> = client.get(&format!(
                    "/repos/{}/pulls?state=all&head={}&per_page=10",
                    source.repository,
                    canban_github::segment(&format!("{head_owner}:{branch}"))
                ))?;
                matched = integration::pull_requests::match_branch_pr(
                    &all_prs,
                    source.repository_id,
                    branch,
                    &excluded,
                    push_owner.as_deref(),
                    Some(&sha),
                )
                .cloned();
            }
        }
        let Some(pr_item) = matched else {
            return Ok(false);
        };
        let Some(number) = pr_item["number"].as_u64() else {
            return Ok(false);
        };
        let full_pr: Value = client.get(&format!("/repos/{}/pulls/{number}", source.repository))?;
        client.seed(
            &format!("/repos/{}/pulls/{number}", source.repository),
            full_pr.clone(),
        );
        let Some(linked) = integration::pull_requests::build_linked_pr(
            &project,
            card,
            source.repository_id,
            &source.repository,
            &full_pr,
        ) else {
            return Ok(false);
        };
        let saved = self.store.lock().unwrap().write(|db| {
            ensure!(
                self.github.generation() == account_generation,
                "GitHub account changed; discarded stale branch PR discovery"
            );
            let mut current: Card = db.get(&card.id)?;
            if !current.active()
                || current.pull_request.is_some()
                || current.branch.as_deref() != Some(branch)
                || current
                    .pull_request_history
                    .iter()
                    .any(|p| p.number == linked.number)
            {
                return Ok(false);
            }
            current.pull_request = Some(linked.clone());
            current.revision += 1;
            db.save(&current)?;
            log(
                db,
                &current,
                "github.pr_linked",
                "github",
                &format!("Linked PR #{} from branch {branch}", linked.number),
                None,
                json!({
                    "origin": "github",
                    "repository": linked.repository,
                    "repository_id": linked.repository_id,
                    "number": linked.number,
                    "url": linked.url,
                    "branch": branch,
                    "head": linked.head,
                }),
            )?;
            Ok(true)
        })?;
        Ok(saved)
    }

    fn project_push_owner(&self, project: &Project, default_repo: &str) -> Option<String> {
        let loc = project.work_location();
        let remote_name = if project.workflow.workspace.remote.trim().is_empty() {
            "origin"
        } else {
            project.workflow.workspace.remote.trim()
        };
        let urls = self
            .ops(&loc.machine)
            .remote_urls(&loc.directory, remote_name);
        urls.into_iter()
            .find(|(name, _)| name == remote_name)
            .and_then(|(_, url)| canban_github::remote_repository(&url).ok())
            .and_then(|repo| repo.split('/').next().map(str::to_owned))
            .or_else(|| default_repo.split('/').next().map(str::to_owned))
    }

    fn should_probe_branch_history(&self, card_id: &str, branch: &str) -> bool {
        let mut cache = self.branch_pr_probes.lock().unwrap();
        let now_instant = std::time::Instant::now();
        let key = (card_id.to_owned(), branch.to_owned());
        if let Some(last) = cache.get(&key)
            && now_instant.duration_since(*last) < Duration::from_secs(300)
        {
            return false;
        }
        cache.insert(key, now_instant);
        true
    }
    pub(crate) fn sync_merged_target(&self, card: &Card, target_branch: Option<&str>) {
        let project = {
            let store = self.store.lock().unwrap();
            store.db().get::<Project>(&card.project_id).ok()
        };
        if let Some(project) = project {
            let project = self.bound_project(&project, card);
            let holder =
                self.checkout_holder_label("GitHub PR target sync", &project, target_branch);
            let Ok(_gate) = self.try_lock_checkout(&project, holder) else {
                return;
            };
            let _ = self
                .ops(&project.work_location().machine)
                .sync_target(&project, target_branch);
            self.git_changed(&project.work_location().machine);
        }
    }
    pub(crate) fn reconcile_remote_pr_merge(
        &self,
        repository: &str,
        number: u64,
        pr_data: &Value,
    ) -> Result<()> {
        if pr_data["merged"] != true {
            return Ok(());
        }
        let mut merged_cards = Vec::new();
        self.store.lock().unwrap().write(|db| {
            let repo_id_fallback = pr_data["base"]["repo"]["id"].as_u64().unwrap_or(0);
            for mut current in db.list::<Card>()? {
                if !current.active() {
                    continue;
                }
                let matches_delivery = current.pull_request.as_ref().is_some_and(|p| {
                    p.repository.eq_ignore_ascii_case(repository) && p.number == number
                });
                let matches_source = current.is_github_pr()
                    && current.github_issue().is_some_and(|s| {
                        s.repository.eq_ignore_ascii_case(repository) && s.number == number
                    });
                if !matches_delivery && !matches_source {
                    continue;
                }
                let before = current.clone();
                let repo_id = current
                    .pull_request
                    .as_ref()
                    .map(|p| p.repository_id)
                    .or_else(|| current.github_issue().map(|s| s.repository_id))
                    .unwrap_or(repo_id_fallback);
                let request_key = format!("github:{repo_id}:{number}");
                let base_branch = pr_data["base"]["ref"]
                    .as_str()
                    .or_else(|| {
                        current
                            .pull_request
                            .as_ref()
                            .map(|p| p.base_branch.as_str())
                    })
                    .unwrap_or("main")
                    .to_owned();
                let commit = pr_data["merge_commit_sha"]
                    .as_str()
                    .or_else(|| pr_data["head"]["sha"].as_str())
                    .or_else(|| current.pull_request.as_ref().map(|p| p.head.as_str()))
                    .unwrap_or("merged")
                    .to_owned();
                if let Some(pr) = &mut current.pull_request {
                    pr.merged = true;
                    pr.state = "closed".into();
                }
                let newly_merged = current
                    .delivery
                    .as_ref()
                    .is_none_or(|d| d.request_key != request_key);
                if newly_merged {
                    let resolved_at = now();
                    current.delivery = Some(Delivery {
                        target_branch: base_branch.clone(),
                        commit: commit.clone(),
                        request_key,
                        merged_at: resolved_at,
                    });
                    if current.resolution.is_none() {
                        current.resolution = Some(CardResolution {
                            resolved_at,
                            kind: "merged".into(),
                            reason: None,
                            actor: "github".into(),
                        });
                    }
                    current.attention = None;
                    if let Some(gate) = current.human_gate.clone()
                        && let Some(id) = gate.terminal_id
                    {
                        let att: TerminalEntry = db.get(&id)?;
                        if att.agent.as_ref().is_some_and(|a| a.state == "completed") {
                            current.human_gate = None;
                            crate::workflow::advance(db, &self.config(), &mut current, Some(&att))?;
                        }
                    }
                    log(
                        db,
                        &current,
                        "github.pr_delivered",
                        "github",
                        "Delivery PR merged",
                        None,
                        json!({"number":number,"commit":commit}),
                    )?;
                }
                if current != before {
                    current.revision += 1;
                    db.save(&current)?;
                }
                merged_cards.push((current, base_branch, newly_merged));
            }
            Ok(())
        })?;
        for (card, base_branch, newly_merged) in merged_cards {
            if newly_merged {
                self.sync_merged_target(&card, Some(&base_branch));
            }
            let _ = self.try_auto_archive_resolved(&card.id);
        }
        Ok(())
    }
    pub(super) fn github_pr_merge(&self, p: Value) -> Result<Value> {
        let card_id = strp(&p, "card_id")?.to_owned();
        let _guard = self.try_lock_card_delivery(&card_id, "GitHub PR merge")?;
        let key = strp(&p, "request_key")?.to_owned();
        if let Some(receipt) = self.github_receipt("github.pr.merge", &p)? {
            return Ok(receipt);
        }
        let card = self
            .snapshot()?
            .cards
            .into_iter()
            .find(|c| Some(c.id.as_str()) == p["card_id"].as_str())
            .context("Card not found")?;
        let pr = policy::validate_merge(&card, strp(&p, "head")?)?;
        self.ops(&pr.workspace.location.machine)
            .verify_clean_head(&pr.workspace.path, &pr.head)?;
        let mut execute = false;
        let mut saved_pr = None;
        let receipt=self.mutate("github.pr.merge",&p,|db|{
            let c:Card=db.get(strp(&p,"card_id")?)?;revise(&c,&p)?;
            ensure!(c.pull_request.as_ref().is_some_and(|binding|binding.repository_id==pr.repository_id && binding.number==pr.number),"PR binding changed; refresh before merging");saved_pr=Some(pr.clone());execute=true;
            log(db,&c,"github.pr_merge_started","user","Accepted delivery PR for GitHub merge",None,json!({"head":pr.head,"request_key":key}))?;
            Ok(json!({"state":"unknown","message":"Merge intent saved; refresh remote PR before retrying","request_key":key}))
        })?;
        if !execute {
            return Ok(receipt);
        }
        let _merging = self.track_merging(&card.id);
        let pr = saved_pr.unwrap();
        let result = (|| -> Result<Value> {
            let client = self.github_client()?.refreshing();
            ensure!(
                client.repo(&pr.repository)?.id == pr.repository_id,
                "Delivery repository identity changed"
            );
            let live: Value =
                client.get(&format!("/repos/{}/pulls/{}", pr.repository, pr.number))?;
            let live = policy::project(&live, &pr)?;
            ensure!(
                live.head == pr.head && live.state == "open" && !live.merged,
                "PR changed; refresh and review before merging"
            );
            client.merge_pr(&pr.repository, pr.number, &pr.head)
        })();
        let value = match result {
            Ok(v) if v["merged"] == true => json!({"state":"merged","commit":v["sha"]}),
            Ok(_) => {
                json!({"state":"failed","message":"GitHub did not merge this PR. Check branch protections and required reviews."})
            }
            Err(e) => {
                json!({"state":"unknown","message":format!("{e:#}; refresh PR state before retrying")})
            }
        };
        self.store.lock().unwrap().write(|db| {
            let mut current: Card = db.get(strp(&p, "card_id")?)?;
            if value["state"] == "merged" {
                let commit = value["commit"]
                    .as_str()
                    .context("GitHub returned no merge commit")?;
                let resolved_at = now();
                current.delivery = Some(Delivery {
                    target_branch: pr.base_branch.clone(),
                    commit: commit.into(),
                    request_key: format!("github:{}:{}", pr.repository_id, pr.number),
                    merged_at: resolved_at,
                });
                current.resolution = Some(CardResolution {
                    resolved_at,
                    kind: "merged".into(),
                    reason: None,
                    actor: "user".into(),
                });
                if let Some(pr) = &mut current.pull_request {
                    pr.merged = true;
                    pr.state = "closed".into();
                }
                current.attention = None;
                if let Some(gate) = current.human_gate.clone()
                    && let Some(id) = gate.terminal_id
                {
                    let att: TerminalEntry = db.get(&id)?;
                    if att.agent.as_ref().is_some_and(|a| a.state == "completed") {
                        current.human_gate = None;
                        crate::workflow::advance(db, &self.config(), &mut current, Some(&att))?;
                    }
                }
                log(
                    db,
                    &current,
                    "github.pr_merged",
                    "user",
                    "Delivery PR merged on GitHub",
                    None,
                    json!({"commit":commit,"request_key":key,"number":pr.number}),
                )?;
            } else {
                let message = value["message"]
                    .as_str()
                    .unwrap_or("Inspect GitHub delivery before retrying");
                if value["state"] == "unknown"
                    && let Some(pr) = &mut current.pull_request
                {
                    pr.state = "merge_unknown".into();
                }
                attention(db, &mut current, message)?;
                log(
                    db,
                    &current,
                    "github.pr_merge_attention",
                    "system",
                    message,
                    None,
                    json!({"request_key":key,"number":pr.number}),
                )?;
            }
            current.revision += 1;
            db.save(&current)?;
            db.conn.execute(
                "UPDATE operations SET result=?2 WHERE id=?1",
                params![key, serde_json::to_string(&value)?],
            )?;
            Ok(())
        })?;
        drop(_merging);
        drop(_guard);
        if value["state"] == "merged" {
            self.sync_merged_target(&card, Some(&pr.base_branch));
            let _ = self.try_auto_archive_resolved(&card.id);
        }
        self.github_sync_due.store(true, Ordering::SeqCst);
        self.emit("github", Some(strp(&p, "card_id")?.into()));
        Ok(value)
    }
}

pub(crate) fn is_ci_attention_message(msg: &str) -> bool {
    msg.starts_with("CI failed") || msg.starts_with("GitHub checks failed")
}

fn ci_failure_attention_message(pr_number: u64, summary: &GitHubPrChecks) -> String {
    let names: Vec<&str> = summary
        .failed_checks
        .iter()
        .take(3)
        .map(|c| c.name.as_str())
        .collect();
    if names.is_empty() {
        format!(
            "CI failed on PR #{pr_number} ({}/{} failed)",
            summary.failed, summary.total
        )
    } else {
        format!(
            "CI failed on PR #{pr_number}: {} ({}/{} failed)",
            names.join(", "),
            summary.failed,
            summary.total
        )
    }
}

fn fetch_pr_checks_summary(
    client: &canban_github::Client,
    repository: &str,
    pr_url: &str,
    head_sha: &str,
) -> Result<GitHubPrChecks> {
    let mut runs = Vec::new();
    for page in 1..=100 {
        let response: Value = client.get(&format!(
            "/repos/{repository}/commits/{}/check-runs?per_page=100&page={page}",
            canban_github::segment(head_sha)
        ))?;
        let page_runs = response["check_runs"]
            .as_array()
            .context("Invalid GitHub checks response")?;
        runs.extend(page_runs.iter().cloned());
        if page_runs.len() < 100 {
            break;
        }
        ensure!(
            page < 100,
            "Check listing incomplete; refresh was not committed"
        );
    }
    let statuses: Value = client.get(&format!(
        "/repos/{repository}/commits/{}/status",
        canban_github::segment(head_sha)
    ))?;
    Ok(policy::summarize_checks(pr_url, head_sha, &runs, &statuses))
}

#[allow(
    clippy::too_many_arguments,
    reason = "Records per-SHA PR CI checkpoint and emits Card CI transition events"
)]
fn record_pr_ci_observation(
    db: Db<'_>,
    card: &mut Card,
    repository: &str,
    repository_id: u64,
    pr_number: u64,
    pr_url: &str,
    pr_open: bool,
    pr_merged: bool,
    summary: &GitHubPrChecks,
    baseline_if_unseen: bool,
) -> Result<bool> {
    if !pr_open || pr_merged || summary.head_sha.is_empty() {
        return Ok(false);
    }
    let obs_id = format!("pr-ci:{repository_id}:{pr_number}");
    let prev: Option<String> = db
        .conn
        .query_row(
            "SELECT data FROM source_observations WHERE id=?1",
            [&obs_id],
            |r| r.get(0),
        )
        .optional()?;
    let prev_json: Value = prev
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let prev_sha = prev_json["head_sha"].as_str().unwrap_or("");
    let prev_outcome = prev_json["outcome"].as_str().unwrap_or("");

    let save_obs = |outcome: &str| -> Result<()> {
        let data = json!({
            "head_sha": summary.head_sha,
            "outcome": outcome,
            "total": summary.total,
            "failed": summary.failed,
        });
        db.conn.execute(
            "INSERT INTO source_observations(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
            params![obs_id, serde_json::to_string(&data)?],
        )?;
        Ok(())
    };

    if summary.state == "running" {
        if prev_sha != summary.head_sha || prev_outcome != "running" {
            save_obs("running")?;
        }
        return Ok(false);
    }

    if !matches!(summary.state.as_str(), "success" | "failure") {
        return Ok(false);
    }

    if prev_sha == summary.head_sha && prev_outcome == summary.state {
        return Ok(false);
    }

    if prev_sha.is_empty() && baseline_if_unseen {
        save_obs(&summary.state)?;
        return Ok(false);
    }

    save_obs(&summary.state)?;

    if card.pull_request.is_some() {
        if summary.state == "failure" {
            card.attention = Some(ci_failure_attention_message(pr_number, summary));
        } else if card
            .attention
            .as_deref()
            .is_some_and(is_ci_attention_message)
        {
            card.attention = None;
        }
    }

    let (kind, message, summary_text) = if summary.state == "failure" {
        let names: Vec<&str> = summary
            .failed_checks
            .iter()
            .take(3)
            .map(|c| c.name.as_str())
            .collect();
        let detail = if names.is_empty() {
            format!("{}/{} failed", summary.failed, summary.total)
        } else {
            format!(
                "{}/{} failed ({})",
                summary.failed,
                summary.total,
                names.join(", ")
            )
        };
        let msg = format!("CI failed on PR #{pr_number} · {detail}");
        ("github.pr.ci_failed", msg, detail)
    } else {
        let detail = format!("{}/{} checks passed", summary.passed, summary.total);
        let msg = format!("CI passed on PR #{pr_number} ({detail})");
        ("github.pr.ci_succeeded", msg, detail)
    };

    log(
        db,
        card,
        kind,
        "github",
        &message,
        None,
        json!({
            "origin": "github",
            "repository": repository,
            "repository_id": repository_id,
            "number": pr_number,
            "url": pr_url,
            "checks_url": summary.url,
            "head_sha": summary.head_sha,
            "branch": card.branch,
            "stage": card.stage,
            "state": summary.state,
            "total": summary.total,
            "passed": summary.passed,
            "failed": summary.failed,
            "running": summary.running,
            "summary": summary_text,
            "failed_checks": summary.failed_checks,
        }),
    )?;
    Ok(true)
}

fn is_review_attention_message(msg: &str) -> bool {
    msg.starts_with("Changes requested on PR")
}

struct PrActivityPayload {
    issue_comments: Vec<Value>,
    review_comments: Vec<Value>,
    reviews: Vec<Value>,
    commits: Vec<String>,
    commit_authors: Vec<String>,
    compare_url: Option<String>,
}

fn fetch_pr_activity(
    client: &canban_github::Client,
    repository: &str,
    pr_number: u64,
    head_sha: &str,
    prev_sha: Option<&str>,
    preloaded_reviews: Option<&[Value]>,
) -> PrActivityPayload {
    let issue_comments = client
        .pages::<Value>(&format!("/repos/{repository}/issues/{pr_number}/comments"))
        .unwrap_or_default();
    let review_comments = client
        .pages::<Value>(&format!("/repos/{repository}/pulls/{pr_number}/comments"))
        .unwrap_or_default();
    let reviews = match preloaded_reviews {
        Some(existing) => existing.to_vec(),
        None => client
            .pages::<Value>(&format!("/repos/{repository}/pulls/{pr_number}/reviews"))
            .unwrap_or_default(),
    };
    let mut commits = Vec::new();
    let mut commit_authors = Vec::new();
    let mut compare_url = None;
    if let Some(prev) = prev_sha.filter(|s| !s.is_empty() && *s != head_sha)
        && let Ok(cmp) = client.get::<Value>(&format!(
            "/repos/{repository}/compare/{}...{}",
            canban_github::segment(prev),
            canban_github::segment(head_sha)
        ))
    {
        compare_url = cmp["html_url"].as_str().map(str::to_owned);
        if let Some(list) = cmp["commits"].as_array() {
            for c in list.iter().take(5) {
                let sha_short: String = c["sha"].as_str().unwrap_or("").chars().take(7).collect();
                let msg = c["commit"]["message"]
                    .as_str()
                    .and_then(|m| m.lines().next())
                    .unwrap_or("")
                    .trim();
                if !msg.is_empty() {
                    commits.push(format!("{sha_short} · {msg}"));
                }
                if let Some(login) = c["author"]["login"]
                    .as_str()
                    .or_else(|| c["committer"]["login"].as_str())
                    .filter(|s| !s.is_empty())
                {
                    commit_authors.push(login.to_owned());
                }
            }
        }
    }
    PrActivityPayload {
        issue_comments,
        review_comments,
        reviews,
        commits,
        commit_authors,
        compare_url,
    }
}

fn seed_pr_activity_cache(
    client: &canban_github::Client,
    repository: &str,
    pr_number: u64,
    payload: &PrActivityPayload,
) {
    let mut issue_map = serde_json::Map::new();
    for c in &payload.issue_comments {
        if let Some(id) = c["id"].as_u64() {
            issue_map.insert(
                id.to_string(),
                json!({
                    "body": c["body"].as_str().unwrap_or(""),
                    "url": c["html_url"].as_str().unwrap_or(""),
                }),
            );
        }
    }
    let mut review_comment_map = serde_json::Map::new();
    for c in &payload.review_comments {
        if let Some(id) = c["id"].as_u64() {
            review_comment_map.insert(
                id.to_string(),
                json!({
                    "body": c["body"].as_str().unwrap_or(""),
                    "url": c["html_url"].as_str().unwrap_or(""),
                    "path": c["path"].as_str().unwrap_or(""),
                    "line": c["line"].as_u64().or_else(|| c["original_line"].as_u64()),
                }),
            );
        }
    }
    let mut review_map = serde_json::Map::new();
    for r in &payload.reviews {
        if let Some(id) = r["id"].as_u64() {
            review_map.insert(
                id.to_string(),
                json!({
                    "body": r["body"].as_str().unwrap_or(""),
                    "url": r["html_url"].as_str().unwrap_or(""),
                    "state": r["state"].as_str().unwrap_or(""),
                }),
            );
        }
    }
    client.seed(
        &format!("/repos/{repository}/pulls/{pr_number}/canban-activity"),
        json!({
            "issue_comments": issue_map,
            "review_comments": review_comment_map,
            "reviews": review_map,
        }),
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "Tracks PR activity baseline and emits metadata-only update/comment/review events"
)]
fn record_pr_activity_updates(
    db: Db<'_>,
    card: &mut Card,
    repository: &str,
    repository_id: u64,
    pr_number: u64,
    pr_url: &str,
    pr_state: &str,
    pr_draft: bool,
    pr_merged: bool,
    head_sha: &str,
    payload: &PrActivityPayload,
    current_login: Option<&str>,
) -> Result<bool> {
    let obs_id = format!("pr-activity:{repository_id}:{pr_number}");
    let prev: Option<String> = db
        .conn
        .query_row(
            "SELECT data FROM source_observations WHERE id=?1",
            [&obs_id],
            |r| r.get(0),
        )
        .optional()?;

    let issue_ids: Vec<u64> = payload
        .issue_comments
        .iter()
        .filter_map(|c| c["id"].as_u64())
        .collect();
    let review_comment_ids: Vec<u64> = payload
        .review_comments
        .iter()
        .filter_map(|c| c["id"].as_u64())
        .collect();
    let review_ids: Vec<u64> = payload
        .reviews
        .iter()
        .filter(|r| r["state"].as_str() != Some("PENDING"))
        .filter_map(|r| r["id"].as_u64())
        .collect();

    let next_obs = json!({
        "head_sha": head_sha,
        "state": pr_state,
        "draft": pr_draft,
        "issue_comment_ids": issue_ids,
        "review_comment_ids": review_comment_ids,
        "review_ids": review_ids,
    });
    let save_obs = || -> Result<()> {
        db.conn.execute(
            "INSERT INTO source_observations(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
            params![obs_id, serde_json::to_string(&next_obs)?],
        )?;
        Ok(())
    };

    let Some(prev_raw) = prev else {
        save_obs()?;
        return Ok(false);
    };
    let prev_json: Value = serde_json::from_str(&prev_raw).unwrap_or(Value::Null);
    let prev_sha = prev_json["head_sha"].as_str().unwrap_or("");
    let prev_state = prev_json["state"].as_str().unwrap_or("");
    let prev_draft = prev_json["draft"].as_bool().unwrap_or(false);
    let prev_issue_ids: Vec<u64> = prev_json["issue_comment_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_u64())
        .collect();
    let prev_rc_ids: Vec<u64> = prev_json["review_comment_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_u64())
        .collect();
    let prev_review_ids: Vec<u64> = prev_json["review_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_u64())
        .collect();

    let pr_open = pr_state == "open";
    let mut emitted = false;
    let is_self = |author: &str| current_login.is_some_and(|l| l.eq_ignore_ascii_case(author));

    if pr_open && !pr_merged && !prev_sha.is_empty() && !head_sha.is_empty() && prev_sha != head_sha
    {
        let short_sha: String = head_sha.chars().take(7).collect();
        let diff_url = payload
            .compare_url
            .clone()
            .unwrap_or_else(|| format!("{}/commits/{head_sha}", pr_url.trim_end_matches('/')));
        let self_action =
            !payload.commit_authors.is_empty() && payload.commit_authors.iter().all(|a| is_self(a));
        let msg = if payload.commits.is_empty() {
            format!("PR #{pr_number} updated · new commits ({short_sha})")
        } else {
            format!(
                "PR #{pr_number} updated · {} new commit{} ({short_sha})",
                payload.commits.len(),
                if payload.commits.len() == 1 { "" } else { "s" }
            )
        };
        log(
            db,
            card,
            "github.pr.updated",
            "github",
            &msg,
            None,
            json!({
                "origin": "github",
                "repository": repository,
                "repository_id": repository_id,
                "number": pr_number,
                "url": pr_url,
                "diff_url": diff_url,
                "head_sha": head_sha,
                "prev_sha": prev_sha,
                "commits": payload.commits,
                "stage": card.stage,
                "self_action": self_action,
            }),
        )?;
        emitted = true;
    }

    if !pr_merged && !prev_state.is_empty() && prev_state != pr_state {
        let (state_key, msg) = if pr_state == "open" {
            ("reopened", format!("PR #{pr_number} reopened"))
        } else {
            ("closed", format!("PR #{pr_number} closed without merging"))
        };
        log(
            db,
            card,
            "github.pr.state_changed",
            "github",
            &msg,
            None,
            json!({
                "origin": "github",
                "repository": repository,
                "repository_id": repository_id,
                "number": pr_number,
                "url": pr_url,
                "state": state_key,
                "stage": card.stage,
                "self_action": false,
            }),
        )?;
        emitted = true;
    } else if pr_open && !pr_merged && prev_draft != pr_draft {
        let (state_key, msg) = if pr_draft {
            ("draft", format!("PR #{pr_number} converted to draft"))
        } else {
            (
                "ready_for_review",
                format!("PR #{pr_number} marked ready for review"),
            )
        };
        log(
            db,
            card,
            "github.pr.state_changed",
            "github",
            &msg,
            None,
            json!({
                "origin": "github",
                "repository": repository,
                "repository_id": repository_id,
                "number": pr_number,
                "url": pr_url,
                "state": state_key,
                "stage": card.stage,
                "self_action": false,
            }),
        )?;
        emitted = true;
    }

    for c in &payload.issue_comments {
        let Some(cid) = c["id"].as_u64() else {
            continue;
        };
        if prev_issue_ids.contains(&cid) {
            continue;
        }
        let author = c["user"]["login"].as_str().unwrap_or("github");
        let url = c["html_url"].as_str().unwrap_or(pr_url);
        log(
            db,
            card,
            "github.pr.commented",
            &format!("github:{author}"),
            &format!("{author} commented on PR #{pr_number}"),
            None,
            json!({
                "origin": "github",
                "repository": repository,
                "repository_id": repository_id,
                "number": pr_number,
                "comment_id": cid,
                "kind": "issue_comment",
                "url": url,
                "pr_url": pr_url,
                "stage": card.stage,
                "self_action": is_self(author),
            }),
        )?;
        emitted = true;
    }

    for c in &payload.review_comments {
        let Some(cid) = c["id"].as_u64() else {
            continue;
        };
        if prev_rc_ids.contains(&cid) {
            continue;
        }
        let author = c["user"]["login"].as_str().unwrap_or("github");
        let url = c["html_url"].as_str().unwrap_or(pr_url);
        let path = c["path"].as_str().unwrap_or("");
        let line = c["line"].as_u64().or_else(|| c["original_line"].as_u64());
        log(
            db,
            card,
            "github.pr.commented",
            &format!("github:{author}"),
            &format!("{author} commented on {path} in PR #{pr_number}"),
            None,
            json!({
                "origin": "github",
                "repository": repository,
                "repository_id": repository_id,
                "number": pr_number,
                "comment_id": cid,
                "kind": "review_comment",
                "path": path,
                "line": line,
                "url": url,
                "pr_url": pr_url,
                "stage": card.stage,
                "self_action": is_self(author),
            }),
        )?;
        emitted = true;
    }

    for r in &payload.reviews {
        let Some(rid) = r["id"].as_u64() else {
            continue;
        };
        let state = r["state"].as_str().unwrap_or("");
        if state == "PENDING" || prev_review_ids.contains(&rid) {
            continue;
        }
        let has_body = !r["body"].as_str().unwrap_or("").trim().is_empty();
        if state == "COMMENTED" && !has_body {
            continue;
        }
        let author = r["user"]["login"].as_str().unwrap_or("github");
        let self_action = is_self(author);
        let url = r["html_url"].as_str().unwrap_or(pr_url);
        let msg = match state {
            "APPROVED" => format!("{author} approved PR #{pr_number}"),
            "CHANGES_REQUESTED" => format!("{author} requested changes on PR #{pr_number}"),
            "DISMISSED" => format!("{author} dismissed review on PR #{pr_number}"),
            _ => format!("{author} reviewed PR #{pr_number}"),
        };
        if card.pull_request.is_some() && pr_open && !pr_merged {
            if state == "CHANGES_REQUESTED" && !self_action {
                card.attention = Some(format!("Changes requested on PR #{pr_number} by @{author}"));
            } else if state == "APPROVED"
                && card
                    .attention
                    .as_deref()
                    .is_some_and(is_review_attention_message)
            {
                card.attention = None;
            }
        }
        log(
            db,
            card,
            "github.pr.reviewed",
            &format!("github:{author}"),
            &msg,
            None,
            json!({
                "origin": "github",
                "repository": repository,
                "repository_id": repository_id,
                "number": pr_number,
                "review_id": rid,
                "state": state,
                "has_body": has_body,
                "url": url,
                "pr_url": pr_url,
                "stage": card.stage,
                "self_action": self_action,
            }),
        )?;
        emitted = true;
    }

    save_obs()?;
    Ok(emitted)
}
