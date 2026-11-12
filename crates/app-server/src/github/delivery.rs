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
        let _guard = self
            .review_gate
            .try_lock()
            .map_err(|_| anyhow::anyhow!("Another delivery is in progress"))?;
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
            workspace,
        };
        let mut execute = false;
        let receipt=self.mutate("github.pr.publish",&p,|db|{
            let mut c:Card=db.get(&card.id)?;revise(&c,&p)?;
            if let Some(reason)=crate::review::blocker(db,&c)?{bail!("{reason}")}
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
        let card: Card = self.store.lock().unwrap().db().get(strp(&p, "card_id")?)?;
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
        let mut next = policy::project(&remote, old)?;
        let mut runs = Vec::new();
        for page in 1..=100 {
            let response: Value = client.get(&format!(
                "/repos/{}/commits/{}/check-runs?per_page=100&page={page}",
                old.repository,
                canban_github::segment(&next.head)
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
            "/repos/{}/commits/{}/status",
            old.repository,
            canban_github::segment(&next.head)
        ))?;
        next.checks = if runs.iter().any(|r| r["status"] != "completed") {
            "pending"
        } else if runs.iter().any(|r| {
            !matches!(
                r["conclusion"].as_str(),
                Some("success" | "neutral" | "skipped")
            )
        }) {
            "failure"
        } else if statuses["total_count"].as_u64().unwrap_or(0) > 0 {
            statuses["state"].as_str().unwrap_or("unknown")
        } else if !runs.is_empty() {
            "success"
        } else {
            "none"
        }
        .into();
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
            json!({"checks":next.checks,"reviews":next.review_summary}),
        );
        let mut newly_merged = false;
        self.store.lock().unwrap().write(|db|{
            ensure!(self.github.generation()==account_generation,"GitHub account changed; discarded stale PR refresh");
            let mut current:Card=db.get(&card.id)?;
            ensure!(current.pull_request.as_ref()==Some(old),"PR binding changed during refresh; refresh again");
            let before=current.clone();
            if old.number==0 {let mut binding=old.clone();binding.number=next.number;binding.url=next.url.clone();binding.state="published".into();current.pull_request=Some(binding);}
            if next.merged
                && let Some(pr) = &mut current.pull_request
            {
                pr.merged = true;
                pr.state = "closed".into();
            }
            if next.merged && current.delivery.as_ref().is_none_or(|d|d.request_key!=format!("github:{}:{}",next.repository_id,next.number)){
                newly_merged = true;
                current.delivery=Some(Delivery{target_branch:next.base_branch.clone(),commit:remote["merge_commit_sha"].as_str().unwrap_or(&next.head).into(),request_key:format!("github:{}:{}",next.repository_id,next.number),merged_at:now()});
                current.attention=None;
                if let Some(gate)=current.human_gate.clone() && let Some(id)=gate.request_id {let request:Request=db.get(&id)?;if request.state=="completed" {current.human_gate=None;crate::workflow::advance(db,&self.config(),&mut current,&request)?;}}
                log(db,&current,"github.pr_delivered","github","Delivery PR merged",None,json!({"number":next.number,"commit":current.delivery.as_ref().map(|d|&d.commit)}))?;
            }else if next.state=="closed" && !next.merged {current.attention=Some("Delivery PR was closed without merging".into());}
            else if next.head!=old.head && old.number>0 {current.attention=Some("GitHub PR head changed; review the latest commits".into());}
            else if next.checks=="failure" {current.attention=Some("GitHub checks failed; inspect the delivery PR".into());}
            if current!=before{current.revision+=1;db.save(&current)?;}
            Ok(())
        })?;
        if newly_merged {
            self.sync_merged_target(&card, Some(&next.base_branch));
        }
        if next.merged {
            let _ = self.try_auto_archive_merged(&card.id);
        }
        self.emit("github", Some(card.id));
        Ok(serde_json::to_value(next)?)
    }
    pub(crate) fn sync_merged_target(&self, card: &Card, target_branch: Option<&str>) {
        let project = {
            let store = self.store.lock().unwrap();
            store.db().get::<Project>(&card.project_id).ok()
        };
        if let Some(project) = project {
            let project = self.bound_project(&project, card);
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
                    current.delivery = Some(Delivery {
                        target_branch: base_branch.clone(),
                        commit: commit.clone(),
                        request_key,
                        merged_at: now(),
                    });
                    current.attention = None;
                    if let Some(gate) = current.human_gate.clone()
                        && let Some(id) = gate.request_id
                    {
                        let request: Request = db.get(&id)?;
                        if request.state == "completed" {
                            current.human_gate = None;
                            crate::workflow::advance(db, &self.config(), &mut current, &request)?;
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
            let _ = self.try_auto_archive_merged(&card.id);
        }
        Ok(())
    }
    pub(super) fn github_pr_merge(&self, p: Value) -> Result<Value> {
        let _guard = self
            .review_gate
            .try_lock()
            .map_err(|_| anyhow::anyhow!("Another delivery is in progress"))?;
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
            if let Some(reason)=crate::review::blocker(db,&c)?{bail!("{reason}")}
            ensure!(c.pull_request.as_ref().is_some_and(|binding|binding.repository_id==pr.repository_id && binding.number==pr.number),"PR binding changed; refresh before merging");saved_pr=Some(pr.clone());execute=true;
            log(db,&c,"github.pr_merge_started","user","Accepted delivery PR for GitHub merge",None,json!({"head":pr.head,"request_key":key}))?;
            Ok(json!({"state":"unknown","message":"Merge intent saved; refresh remote PR before retrying","request_key":key}))
        })?;
        if !execute {
            return Ok(receipt);
        }
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
                current.delivery = Some(Delivery {
                    target_branch: pr.base_branch.clone(),
                    commit: commit.into(),
                    request_key: format!("github:{}:{}", pr.repository_id, pr.number),
                    merged_at: now(),
                });
                if let Some(pr) = &mut current.pull_request {
                    pr.merged = true;
                    pr.state = "closed".into();
                }
                current.attention = None;
                if let Some(gate) = current.human_gate.clone()
                    && let Some(id) = gate.request_id
                {
                    let request: Request = db.get(&id)?;
                    if request.state == "completed" {
                        current.human_gate = None;
                        crate::workflow::advance(db, &self.config(), &mut current, &request)?;
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
        drop(_guard);
        if value["state"] == "merged" {
            self.sync_merged_target(&card, Some(&pr.base_branch));
            let _ = self.try_auto_archive_merged(&card.id);
        }
        self.github_sync_due.store(true, Ordering::SeqCst);
        self.emit("github", Some(strp(&p, "card_id")?.into()));
        Ok(value)
    }
}
