use super::*;

const UTC_DAY_MS: u64 = 86_400_000;

fn same_or_later_utc_day(ts_ms: u64, now_ms: u64) -> bool {
    ts_ms / UTC_DAY_MS >= now_ms / UTC_DAY_MS
}

pub(super) fn activity_warning(db: Db<'_>, card: &Card) -> Result<Option<String>> {
    Ok(canban_types::agent_activity_warning(
        &card.id,
        &db.list::<TerminalEntry>()?,
    ))
}

fn default_project_merge_strategy(db: Db<'_>, card: &Card) -> Result<MergeStrategy> {
    for run in db.list::<WorkflowRun>()? {
        if run.card_id.as_deref() == Some(card.id.as_str())
            && run.active()
            && let Some(step_id) = &run.step
            && let Some(WorkflowStep::Deliver { strategy, .. }) = run.definition.steps.get(step_id)
        {
            return Ok(*strategy);
        }
    }
    let sources = db.list::<WorkflowSource>()?;
    let revisions = db.list::<WorkflowRevision>()?;
    for source in sources
        .iter()
        .filter(|s| s.project_id == card.project_id && s.enabled)
    {
        if let Some(rev) = revisions
            .iter()
            .find(|r| r.workflow_id == source.workflow_id && r.revision == source.revision)
            && let Some((_, WorkflowStep::Deliver { strategy, .. })) = rev
                .definition
                .steps
                .iter()
                .find(|(_, st)| matches!(st, WorkflowStep::Deliver { .. }))
        {
            return Ok(*strategy);
        }
    }
    Ok(MergeStrategy::Rebase)
}
pub(super) fn resolve_review_profile(
    config: &Config,
    project: Option<&Project>,
    explicit: Option<&str>,
) -> Result<Profile> {
    if let Some(id) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return canban_scheduler::select_in_project(config, project, Some(id));
    }
    if config
        .profiles
        .iter()
        .any(|p| p.id == "reviewer" && p.enabled)
    {
        return canban_scheduler::select_in_project(config, project, Some("reviewer"));
    }
    if config
        .profiles
        .iter()
        .any(|p| p.id == config.default_agent && p.enabled)
    {
        return canban_scheduler::select_in_project(config, project, Some(&config.default_agent));
    }
    if let Some(first) = config.profiles.iter().find(|p| p.enabled) {
        return canban_scheduler::select_in_project(config, project, Some(&first.id));
    }
    anyhow::bail!("No enabled Agent profile is available; enable one in Settings");
}

fn normalize_review_verdict(raw: Option<&str>) -> Result<String> {
    let val = raw.unwrap_or("comment").trim().to_ascii_lowercase();
    match val.as_str() {
        "" | "comment" => Ok("comment".into()),
        "approve" => Ok("approve".into()),
        "request_changes" | "request-changes" => Ok("request_changes".into()),
        _ => anyhow::bail!(
            "Unsupported review verdict '{val}'; use comment, approve, or request_changes"
        ),
    }
}

fn validate_draft_comment_path(raw: &str) -> Result<String> {
    let p = raw.trim();
    ensure!(!p.is_empty(), "Draft comment path cannot be empty");
    ensure!(
        !p.starts_with('/') && !p.starts_with('-'),
        "Invalid relative comment path: {p}"
    );
    ensure!(!p.contains('\0'), "Comment path contains NUL byte");
    let rel = Path::new(p);
    ensure!(!rel.is_absolute(), "Comment path must be relative: {p}");
    for comp in rel.components() {
        ensure!(
            !matches!(
                comp,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            ),
            "Invalid path component in '{p}'"
        );
    }
    Ok(p.to_owned())
}

impl AppServer {
    pub(super) fn review_preview(&self, p: Value) -> Result<Value> {
        let (card, project, warning) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let c = super::resolve_card(db, &p)?;
            let project: Project = db.get(&c.project_id)?;
            let project = self.bound_project(&project, &c);
            let warning = activity_warning(db, &c)?;
            (c, project, warning)
        };
        let mut plan = self
            .ops()
            .review_preview(&project, &card, p["target_branch"].as_str())?;
        plan.warning = warning;
        Ok(serde_json::to_value(plan)?)
    }
    pub(super) fn review_merge(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Code acceptance requires explicit human review"
        );
        let key = strp(&p, "request_key")?.to_owned();
        let requested_strategy = match p["strategy"].as_str() {
            Some(raw) if !raw.trim().is_empty() => Some(
                MergeStrategy::parse(raw)
                    .with_context(|| format!("Unknown merge strategy `{}`", raw.trim()))?,
            ),
            _ => None,
        };
        let target_project = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let card: Card = db.get(strp(&p, "card_id")?)?;
            let project: Project = db.get(&card.project_id)?;
            self.bound_project(&project, &card)
        };
        let holder = self.checkout_holder_label(
            "Review merge",
            &target_project,
            p["target_branch"]
                .as_str()
                .or(target_project.target_branch.as_deref()),
        );
        let _gate = self
            .try_lock_checkout(&target_project, holder)
            .map_err(anyhow::Error::msg)?;
        // A stored intent must win before any Git effects. Duplicate/unknown
        // delivery requests are never blindly replayed after a lost reply.
        let mut execute = false;
        let mut context = None;
        let saved=self.mutate("review.merge",&p,|db|{
            let card:Card=db.get(strp(&p,"card_id")?)?;revise(&card,&p)?;
            ensure!(card.active(),"Restore the Card before accepting");
            ensure!(card.pull_request.as_ref().is_none_or(|pr|pr.merged),"Resolve the existing PR delivery before local merge");
            let project:Project=db.get(&card.project_id)?;let project=self.bound_project(&project,&card);
            let strategy=match requested_strategy{Some(s)=>s,None=>default_project_merge_strategy(db,&card)?};
            let plan=self.ops().review_preview(&project,&card,p["target_branch"].as_str())?;
            ensure!(plan.token==strp(&p,"token")?,"Review is stale; refresh the diff before accepting");
            ensure!(plan.merge_allowed,"{}",plan.reason.as_deref().unwrap_or("Merge unavailable"));
            log(db,&card,"review.merge_started","user","Accepted reviewed changes for commit/merge",None,json!({"request_key":key,"target_branch":plan.target_branch,"target_head":plan.target_head,"card_head":plan.card_head,"strategy":strategy,"token":plan.token}),self.now_ms())?;
            execute=true;context=Some((card,project,plan,strategy));
            Ok(json!({"state":"unknown","request_key":key,"message":"Merge intent saved; inspect Git heads before repeating an uncertain delivery"}))
        })?;
        if !execute {
            return Ok(saved);
        }
        let (card, project, plan, strategy) = context.unwrap();
        let _merging = self.track_merging(&card.id);
        let default_msg = format!("Canban #{}: {}", card.number, card.title);
        let message = p["message"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| default_msg.clone());
        let message_customized = p["message_customized"]
            .as_bool()
            .unwrap_or_else(|| p["message"].as_str().is_some() && message.trim() != default_msg);
        let result = self.ops().review_merge_with_strategy(
            &project,
            &card,
            Some(&plan.target_branch),
            &plan.token,
            &message,
            strategy,
            message_customized,
        );
        let value=self.store.lock().unwrap().write(|db|{
            let mut current:Card=db.get(&card.id)?;
            let now_ms = self.now_ms();
            let value=match result {
                Ok(commit)=>{
                    let resolved_at = now_ms;
                    current.delivery_mode=Some(DeliveryMode::Local);
                    current.delivery=Some(Delivery{target_branch:plan.target_branch.clone(),commit:commit.clone(),request_key:key.clone(),merged_at:resolved_at});
                    current.resolution=Some(CardResolution{resolved_at,kind:"merged".into(),reason:None,actor:"user".into()});
                    current.attention=None;
                    let gate = current.human_gate.take();
                    let mut completed_deliver = false;
                    for mut active_run in db.list::<WorkflowRun>()? {
                        if active_run.card_id.as_deref() == Some(current.id.as_str())
                            && active_run.active()
                            && active_run.step.as_ref().is_some_and(|k| {
                                matches!(active_run.definition.steps.get(k), Some(WorkflowStep::Deliver { .. }))
                            })
                        {
                            completed_deliver = true;
                            canban_workflow_engine::runtime::complete_delivery_at(
                                db,
                                &self.config(),
                                &mut current,
                                &mut active_run,
                                json!({"state":"merged","commit":commit,"target_branch":plan.target_branch,"manual":true}),
                                enqueue,
                                resolved_at,
                            )?;
                        }
                    }
                    if !completed_deliver
                        && let Some(gate) = gate
                        && let Some(tid) = gate.terminal_id
                    {
                        let att: TerminalEntry = db.get(&tid)?;
                        if att.agent.as_ref().is_some_and(|a| a.state == "completed") {
                            crate::workflow::advance(
                                db,
                                &self.config(),
                                &mut current,
                                Some(&att),
                                resolved_at,
                            )?;
                        }
                    }
                    log(db,&current,"review.merged","user",&format!("Merged into {}",plan.target_branch),None,json!({"commit":commit,"target_branch":plan.target_branch,"request_key":key}),resolved_at)?;
                    json!({"state":"merged","commit":commit,"target_branch":plan.target_branch,"card_id":card.id})
                }
                Err(error)=>{
                    let message=format!("{error:#}");attention(db,&mut current,&message,now_ms)?;
                    log(db,&current,"review.merge_failed","system",&message,None,json!({"target_branch":plan.target_branch,"request_key":key}),now_ms)?;
                    json!({"state":"failed","message":message,"target_branch":plan.target_branch,"card_id":card.id})
                }
            };
            current.revision+=1;db.save(&current)?;
            db.conn.execute("UPDATE operations SET result=?2 WHERE id=?1",params![key,serde_json::to_string(&value)?])?;
            Ok(value)
        })?;
        self.git_changed();
        drop(_merging);
        drop(_gate);
        if value["state"] == "merged" {
            let _ = self.try_auto_archive_resolved(&card.id);
            self.request_deliveries();
            self.request_scripts();
        }
        self.emit("review", Some(card.id));
        Ok(value)
    }
    pub(crate) fn try_auto_archive_resolved(&self, card_id: &str) -> Result<bool> {
        self.try_auto_archive_resolved_at(card_id, self.now_ms())
    }

    pub(crate) fn try_auto_archive_resolved_at(&self, card_id: &str, now_ms: u64) -> Result<bool> {
        let (card, project) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let Ok(card) = db.get::<Card>(card_id) else {
                return Ok(false);
            };
            if !card.active()
                || !card.resolved()
                || card.attention.is_some()
                || card.human_gate.is_some()
            {
                return Ok(false);
            }
            // Fast path: keep Cards resolved today visible on the Board until the next UTC calendar day
            // before scanning terminals and events.
            if let Some(ts) = card.resolved_at()
                && same_or_later_utc_day(ts, now_ms)
            {
                return Ok(false);
            }
            if activity_warning(db, &card)?.is_some() {
                return Ok(false);
            }
            let last_lifecycle = db
                .list::<Event>()?
                .into_iter()
                .filter(|e| {
                    e.card_id == card.id
                        && matches!(
                            e.kind.as_str(),
                            "review.merged"
                                | "github.pr_merged"
                                | "github.pr_delivered"
                                | "card.resolved"
                                | "card.reopened"
                                | "card.restored"
                                | "card.archived"
                        )
                })
                .max_by_key(|e| e.sequence);
            if last_lifecycle.as_ref().is_some_and(|e| {
                matches!(
                    e.kind.as_str(),
                    "card.restored" | "card.archived" | "card.reopened"
                )
            }) {
                return Ok(false);
            }
            // Keep Cards resolved today visible on the Board until the next UTC calendar day.
            let resolved_at = card
                .resolved_at()
                .or_else(|| last_lifecycle.as_ref().map(|e| e.created_at))
                .unwrap_or(card.created_at);
            if same_or_later_utc_day(resolved_at, now_ms) {
                return Ok(false);
            }
            let project: Project = db.get(&card.project_id)?;
            let project = self.bound_project(&project, &card);
            (card, project)
        };
        if card.worktree.is_some() {
            if self.terminal_retains_worktree(&card).unwrap_or(true) {
                return Ok(false);
            }
            if let Ok(info) = self.ops().info(&project, &card)
                && (info.dirty || info.state == "conflict")
            {
                return Ok(false);
            }
        }
        self.archive(&json!({
            "card_id": card.id,
            "revision": card.revision,
        }))?;
        Ok(true)
    }

    pub(crate) fn pump_resolved_archives(&self) -> Result<()> {
        let now_ms = self.now_ms();
        let candidates: Vec<String> = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let mut ids = Vec::new();
            for c in db.list::<Card>()? {
                if !c.active() || !c.resolved() || c.attention.is_some() || c.human_gate.is_some() {
                    continue;
                }
                if let Some(ts) = c.resolved_at()
                    && same_or_later_utc_day(ts, now_ms)
                {
                    continue;
                }
                ids.push(c.id);
            }
            ids
        };
        for id in candidates {
            let _ = self.try_auto_archive_resolved_at(&id, now_ms);
        }
        Ok(())
    }

    pub(crate) fn pump_deliveries(&self) -> Result<bool> {
        let project_ids: Vec<String> = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let runs = db.list::<WorkflowRun>()?;
            let mut ids = std::collections::BTreeSet::new();
            for run in runs {
                if run.step_is_deliver() && run.active() {
                    ids.insert(run.project_id);
                }
            }
            ids.into_iter().collect()
        };
        let mut any_changed = false;
        for project_id in project_ids {
            if self.pump_project_delivery_queue(&project_id)? {
                any_changed = true;
            }
        }
        Ok(any_changed)
    }

    fn pump_project_delivery_queue(&self, project_id: &str) -> Result<bool> {
        let Some(_queue_gate) = self.try_lock_delivery_queue(project_id) else {
            return Ok(false);
        };
        let mut any_changed = false;
        // Bound iterations per pump pass to guard against pathological retry loops.
        for _ in 0..64 {
            // Reconcile any newly-held cards in `waiting_delivery` so they leave the active queue
            // before selecting the head of line.
            let held_runs: Vec<(String, String)> = {
                let store = self.store.lock().unwrap();
                let db = store.db();
                let cards: std::collections::BTreeMap<String, Card> = db
                    .list::<Card>()?
                    .into_iter()
                    .filter(|c| c.project_id == project_id)
                    .map(|c| (c.id.clone(), c))
                    .collect();
                db.list::<WorkflowRun>()?
                    .into_iter()
                    .filter_map(|r| {
                        let card_id = r.card_id?;
                        if r.project_id == project_id
                            && r.state == "waiting_delivery"
                            && cards
                                .get(&card_id)
                                .is_some_and(|c| c.active() && c.hold_auto_merge)
                        {
                            Some((card_id, r.id))
                        } else {
                            None
                        }
                    })
                    .collect()
            };
            for (card_id, run_id) in held_runs {
                let held = self.store.lock().unwrap().write(|db| {
                    let mut current_card: Card = db.get(&card_id)?;
                    let mut current_run: WorkflowRun = db.get(&run_id)?;
                    if current_run.state == "waiting_delivery" && current_card.hold_auto_merge {
                        canban_workflow_engine::runtime::hold_delivery_at(
                            db,
                            &mut current_card,
                            &mut current_run,
                            "Auto-merge held by user; approve or resume to merge",
                            self.now_ms(),
                        )?;
                        return Ok(true);
                    }
                    Ok(false)
                })?;
                if held {
                    any_changed = true;
                    self.emit("application", Some(card_id));
                }
            }

            let head_run = {
                let store = self.store.lock().unwrap();
                let db = store.db();
                let runs = db.list::<WorkflowRun>()?;
                let cards = db.list::<Card>()?;
                let queue = canban_types::project_merge_queue(&runs, &cards, project_id);
                // Head-of-line serialization: while any Deliver run in this Project is actively
                // resolving a conflict via @merger (`waiting_agent` / `running`), no queued Card
                // may merge or spawn a concurrent resolver.
                if queue.iter().any(|r| {
                    matches!(
                        r.state.as_str(),
                        "waiting_agent" | "running" | "waiting_batch"
                    )
                }) {
                    None
                } else {
                    queue.into_iter().next().cloned()
                }
            };
            let Some(head) = head_run else {
                break;
            };
            if head.state != "waiting_delivery" {
                break;
            }
            match self.collect_compatible_batch(project_id)? {
                crate::merge_batch::BatchSelection::Empty
                | crate::merge_batch::BatchSelection::HoldQueue => break,
                crate::merge_batch::BatchSelection::Single(run_id) => {
                    let (advance, mutated) = self.execute_workflow_delivery(&run_id)?;
                    any_changed |= mutated;
                    if !advance {
                        break;
                    }
                }
                crate::merge_batch::BatchSelection::Batch { run_ids } => {
                    let (advance, mutated) = self.execute_batch_delivery(project_id, &run_ids)?;
                    any_changed |= mutated;
                    if !advance {
                        break;
                    }
                }
            }
        }
        Ok(any_changed)
    }

    pub(super) fn set_queue_pause_reason(
        &self,
        card_id: &str,
        run_id: &str,
        reason: Option<&str>,
    ) -> Result<bool> {
        let mut changed = false;
        self.store.lock().unwrap().write(|db| {
            let mut current_run: WorkflowRun = db.get(run_id)?;
            if current_run.state != "waiting_delivery" {
                return Ok(());
            }
            let prev = current_run.queue_pause_reason().map(str::to_owned);
            let next = reason.map(str::trim).filter(|s| !s.is_empty());
            if prev.as_deref() == next {
                return Ok(());
            }
            if let Some(msg) = next {
                current_run.inputs["queue_pause_reason"] = json!(msg);
            } else if let Some(obj) = current_run.inputs.as_object_mut() {
                obj.remove("queue_pause_reason");
            }
            current_run.updated_at = self.now_ms();
            db.save(&current_run)?;
            changed = true;
            Ok(())
        })?;
        if changed {
            self.emit("review", Some(card_id.to_owned()));
        }
        Ok(changed)
    }

    /// Maximum grace window for a connected Agent that already reported `completed` to
    /// finish wrapping up its interactive turn before Queue Merge integrates the worktree.
    pub(crate) const DELIVERY_AGENT_SETTLE_MS: u64 = 180_000;

    /// Executes one step for the head-of-line `waiting_delivery` run.
    /// Returns `Ok((advance_queue, mutated))`: `advance_queue` is `true` when this run exited
    /// the head of the queue (merged or stepped aside to a human gate) so the caller can
    /// immediately advance to the next Card in line; `mutated` is `true` when DB state changed.
    fn execute_workflow_delivery(&self, run_id: &str) -> Result<(bool, bool)> {
        let (card, project, run, has_pending_attempts, settling_agent) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let Ok(run) = db.get::<WorkflowRun>(run_id) else {
                return Ok((false, false));
            };
            if run.state != "waiting_delivery" {
                return Ok((false, false));
            }
            let Some(card_id) = run.card_id.as_deref() else {
                return Ok((false, false));
            };
            let card: Card = db.get(card_id)?;
            if !card.active() {
                return Ok((false, false));
            }
            let project: Project = db.get(&card.project_id)?;
            let project = self.bound_project(&project, &card);
            let terminals = db.list::<TerminalEntry>()?;
            let has_pending_attempts = terminals.iter().filter_map(|t| t.agent.as_ref()).any(|a| {
                a.card_id == card.id
                    && (a.pending.is_some()
                        || matches!(
                            a.state.as_str(),
                            "queued" | "starting" | "running" | "working" | "unknown"
                        ))
            });
            let now_ms = self.now_ms();
            let settling_agent = terminals.into_iter().any(|t| {
                let Some(a) = &t.agent else {
                    return false;
                };
                if a.card_id != card.id || a.terminal.is_none() || a.agent_stopped() {
                    return false;
                }
                if a.connection_state != "connected" {
                    return false;
                }
                let anchor = a
                    .completed_at
                    .max(Some(run.updated_at))
                    .unwrap_or(t.created_at);
                now_ms.saturating_sub(anchor) < Self::DELIVERY_AGENT_SETTLE_MS
            });
            (card, project, run, has_pending_attempts, settling_agent)
        };
        let config = self.config();
        if card.hold_auto_merge {
            let mutated = self.store.lock().unwrap().write(|db| {
                let mut current_card: Card = db.get(&card.id)?;
                let mut current_run: WorkflowRun = db.get(&run.id)?;
                if current_run.state == "waiting_delivery" {
                    canban_workflow_engine::runtime::hold_delivery_at(
                        db,
                        &mut current_card,
                        &mut current_run,
                        "Auto-merge held by user; approve or resume to merge",
                        self.now_ms(),
                    )?;
                    return Ok(true);
                }
                Ok(false)
            })?;
            if mutated {
                self.emit("application", Some(card.id));
            }
            return Ok((true, mutated));
        }
        // No worktree means there is nothing to merge (e.g. create_worktree: false);
        // complete the step without waiting on Agent terminals.
        if card.worktree.is_none() {
            let mutated = self.store.lock().unwrap().write(|db| {
                let mut current_card: Card = db.get(&card.id)?;
                let mut current_run: WorkflowRun = db.get(&run.id)?;
                if current_run.state == "waiting_delivery" {
                    canban_workflow_engine::runtime::complete_delivery_at(
                        db,
                        &config,
                        &mut current_card,
                        &mut current_run,
                        json!({"state":"delivered","mode":"no_worktree"}),
                        enqueue,
                        self.now_ms(),
                    )?;
                    return Ok(true);
                }
                Ok(false)
            })?;
            if mutated {
                self.emit("application", Some(card.id));
            }
            return Ok((true, mutated));
        }
        if has_pending_attempts || settling_agent {
            return Ok((false, false));
        }
        let _merging = self.track_merging(&card.id);
        let target_branch = run.inputs["target_branch"].as_str();
        let strategy = run.inputs["strategy"]
            .as_str()
            .and_then(MergeStrategy::parse)
            .or_else(|| {
                run.step
                    .as_ref()
                    .and_then(|k| match run.definition.steps.get(k) {
                        Some(WorkflowStep::Deliver { strategy, .. }) => Some(*strategy),
                        _ => None,
                    })
            })
            .unwrap_or_default();
        let sync_upstream = run.inputs["sync_upstream"].as_bool().unwrap_or(true);
        let mut workspace_project = project.clone();
        workspace_project.workflow.workspace = run.definition.workspace.clone();
        let ops = self.ops();
        let holder = self.checkout_holder_label(
            &format!("Code delivery (Card #{})", card.number),
            &workspace_project,
            target_branch.or(workspace_project.target_branch.as_deref()),
        );
        let mut checkout_gate = match self.try_lock_checkout(&workspace_project, holder) {
            Ok(guard) => Some(guard),
            Err(reason) => {
                let pause_msg = format!("Merge queue waiting: {reason}");
                let changed = self.set_queue_pause_reason(&card.id, &run.id, Some(&pause_msg))?;
                return Ok((false, changed));
            }
        };
        if sync_upstream
            && ops.has_push_remote(&workspace_project)
            && let Err(e) = ops.sync_target(&workspace_project, target_branch)
        {
            checkout_gate.take();
            if let Some(canban_workspaces::review::DeliveryError::MainCheckoutBusy {
                reason: busy,
            }) = e.downcast_ref::<canban_workspaces::review::DeliveryError>()
            {
                let pause_msg = format!("Merge queue waiting: {busy}");
                let changed = self.set_queue_pause_reason(&card.id, &run.id, Some(&pause_msg))?;
                return Ok((false, changed));
            }
            let reason = format!("Auto-merge paused (upstream sync): {e:#}");
            let mutated = self.store.lock().unwrap().write(|db| {
                let mut current_card: Card = db.get(&card.id)?;
                let mut current_run: WorkflowRun = db.get(&run.id)?;
                if current_run.state == "waiting_delivery" {
                    canban_workflow_engine::runtime::hold_delivery_at(
                        db,
                        &mut current_card,
                        &mut current_run,
                        &reason,
                        self.now_ms(),
                    )?;
                    return Ok(true);
                }
                Ok(false)
            })?;
            if mutated {
                self.emit("review", Some(card.id));
            }
            return Ok((true, mutated));
        }
        let plan = match ops.review_preview(&workspace_project, &card, target_branch) {
            Ok(plan) => plan,
            Err(e) => {
                checkout_gate.take();
                let reason = format!("Auto-merge paused: {e:#}");
                let mutated = self.store.lock().unwrap().write(|db| {
                    let mut current_card: Card = db.get(&card.id)?;
                    let mut current_run: WorkflowRun = db.get(&run.id)?;
                    if current_run.state == "waiting_delivery" {
                        canban_workflow_engine::runtime::hold_delivery_at(
                            db,
                            &mut current_card,
                            &mut current_run,
                            &reason,
                            self.now_ms(),
                        )?;
                        return Ok(true);
                    }
                    Ok(false)
                })?;
                if mutated {
                    self.emit("review", Some(card.id));
                }
                return Ok((true, mutated));
            }
        };
        if !plan.merge_allowed {
            checkout_gate.take();
            let pending_conflict = ops
                .pending_merge_conflict(&workspace_project, &card, target_branch)
                .ok()
                .flatten();
            if pending_conflict.is_none()
                && let Some(raw_reason) = plan.reason.as_deref()
                && canban_workspaces::review::is_main_checkout_busy_reason(raw_reason)
            {
                let pause_msg = format!("Merge queue waiting: {raw_reason}");
                let changed = self.set_queue_pause_reason(&card.id, &run.id, Some(&pause_msg))?;
                return Ok((false, changed));
            }
            let reason = format!(
                "Auto-merge paused: {}",
                plan.reason
                    .as_deref()
                    .unwrap_or("Merge unavailable; human review required")
            );
            let mut dispatched_conflict = false;
            let mutated = self.store.lock().unwrap().write(|db| {
                let mut current_card: Card = db.get(&card.id)?;
                let mut current_run: WorkflowRun = db.get(&run.id)?;
                if current_run.state == "waiting_delivery" {
                    let now_ms = self.now_ms();
                    if let Some((ref conflict_branch, ref conflict_head, ref files)) =
                        pending_conflict
                        && canban_workflow_engine::runtime::rework_conflict_at(
                            db,
                            &config,
                            &mut current_card,
                            &mut current_run,
                            conflict_branch,
                            conflict_head,
                            files,
                            enqueue,
                            now_ms,
                        )?
                    {
                        dispatched_conflict = true;
                        return Ok(true);
                    }
                    canban_workflow_engine::runtime::hold_delivery_at(
                        db,
                        &mut current_card,
                        &mut current_run,
                        &reason,
                        now_ms,
                    )?;
                    return Ok(true);
                }
                Ok(false)
            })?;
            if mutated {
                self.emit("review", Some(card.id));
            }
            return Ok((!dispatched_conflict, mutated));
        }
        // Main checkout is clean and ready: clear any previous transient wait reason.
        let cleared_pause = self.set_queue_pause_reason(&card.id, &run.id, None)?;
        let key = format!("workflow-deliver:{}:{}", run.id, run.visit);
        let message = format!("Canban #{}: {}", card.number, card.title);
        let result = ops.review_merge_with_strategy(
            &workspace_project,
            &card,
            Some(&plan.target_branch),
            &plan.token,
            &message,
            strategy,
            false,
        );
        let push_result = if result.is_ok() && sync_upstream {
            Some(ops.push_target_branch(&workspace_project, &plan.target_branch))
        } else {
            None
        };
        checkout_gate.take();
        let mut merged_and_completed = false;
        let mut advance_queue = true;
        let mut mutated = cleared_pause;
        self.store.lock().unwrap().write(|db| {
            let mut current_card: Card = db.get(&card.id)?;
            let mut current_run: WorkflowRun = db.get(&run.id)?;
            if current_run.state != "waiting_delivery" {
                advance_queue = false;
                return Ok(());
            }
            let now_ms = self.now_ms();
            mutated = true;
            match result {
                Ok(commit) => {
                    let resolved_at = now_ms;
                    current_card.delivery_mode = Some(DeliveryMode::Local);
                    current_card.delivery = Some(Delivery {
                        target_branch: plan.target_branch.clone(),
                        commit: commit.clone(),
                        request_key: key.clone(),
                        merged_at: resolved_at,
                    });
                    current_card.resolution = Some(CardResolution {
                        resolved_at,
                        kind: "merged".into(),
                        reason: None,
                        actor: "system".into(),
                    });
                    current_card.attention = None;
                    current_card.human_gate = None;
                    current_card.revision += 1;
                    match push_result {
                        Some(Err(push_err)) => {
                            let reason = format!(
                                "Merged locally into {}, but syncing upstream failed: {push_err:#}",
                                plan.target_branch
                            );
                            log(
                                db,
                                &current_card,
                                "review.upstream_sync_failed",
                                "system",
                                &reason,
                                None,
                                json!({"commit":commit,"target_branch":plan.target_branch,"request_key":key,"workflow_run_id":run.id}),
                                resolved_at,
                            )?;
                            canban_workflow_engine::runtime::hold_delivery_at(
                                db,
                                &mut current_card,
                                &mut current_run,
                                &reason,
                                resolved_at,
                            )?;
                        }
                        Some(Ok(Some(remote_name))) => {
                            merged_and_completed = true;
                            log(
                                db,
                                &current_card,
                                "review.merged",
                                "system",
                                &format!(
                                    "Auto-merged into {} and synced to {remote_name}/{}",
                                    plan.target_branch, plan.target_branch
                                ),
                                None,
                                json!({"commit":commit,"target_branch":plan.target_branch,"synced_upstream":true,"remote":remote_name,"request_key":key,"workflow_run_id":run.id}),
                                resolved_at,
                            )?;
                            canban_workflow_engine::runtime::complete_delivery_at(
                                db,
                                &config,
                                &mut current_card,
                                &mut current_run,
                                json!({"state":"merged","commit":commit,"target_branch":plan.target_branch,"synced_upstream":true,"remote":remote_name}),
                                enqueue,
                                resolved_at,
                            )?;
                        }
                        Some(Ok(None)) => {
                            merged_and_completed = true;
                            log(
                                db,
                                &current_card,
                                "review.merged",
                                "system",
                                &format!("Auto-merged into {}", plan.target_branch),
                                None,
                                json!({"commit":commit,"target_branch":plan.target_branch,"synced_upstream":false,"reason":"no_remote","request_key":key,"workflow_run_id":run.id}),
                                resolved_at,
                            )?;
                            canban_workflow_engine::runtime::complete_delivery_at(
                                db,
                                &config,
                                &mut current_card,
                                &mut current_run,
                                json!({"state":"merged","commit":commit,"target_branch":plan.target_branch,"synced_upstream":false,"reason":"no_remote"}),
                                enqueue,
                                resolved_at,
                            )?;
                        }
                        None => {
                            merged_and_completed = true;
                            log(
                                db,
                                &current_card,
                                "review.merged",
                                "system",
                                &format!(
                                    "Auto-merged into {} (local only, upstream sync disabled)",
                                    plan.target_branch
                                ),
                                None,
                                json!({"commit":commit,"target_branch":plan.target_branch,"synced_upstream":false,"request_key":key,"workflow_run_id":run.id}),
                                resolved_at,
                            )?;
                            canban_workflow_engine::runtime::complete_delivery_at(
                                db,
                                &config,
                                &mut current_card,
                                &mut current_run,
                                json!({"state":"merged","commit":commit,"target_branch":plan.target_branch,"synced_upstream":false}),
                                enqueue,
                                resolved_at,
                            )?;
                        }
                    }
                }
                Err(error) => {
                    use canban_workspaces::review::DeliveryError;
                    match error.downcast_ref::<DeliveryError>() {
                        // The Project branch moved under us; retry this head Card immediately.
                        Some(DeliveryError::TargetAdvanced) => {
                            log(
                                db,
                                &current_card,
                                "review.merge_retry",
                                "system",
                                "Target branch advanced during delivery; retrying",
                                None,
                                json!({"target_branch":plan.target_branch,"request_key":key,"workflow_run_id":run.id}),
                                now_ms,
                            )?;
                            return Ok(());
                        }
                        Some(DeliveryError::MainCheckoutBusy { reason: busy }) => {
                            current_run.inputs["queue_pause_reason"] =
                                json!(format!("Merge queue waiting: {busy}"));
                            current_run.updated_at = now_ms;
                            db.save(&current_run)?;
                            advance_queue = false;
                            return Ok(());
                        }
                        Some(DeliveryError::Conflict {
                            target_branch,
                            target_head,
                            files,
                            ..
                        }) if canban_workflow_engine::runtime::rework_conflict_at(
                            db,
                            &config,
                            &mut current_card,
                            &mut current_run,
                            target_branch,
                            target_head,
                            files,
                            enqueue,
                            now_ms,
                        )? =>
                        {
                            advance_queue = false;
                            return Ok(());
                        }
                        _ => {}
                    }
                    let reason = format!("Auto-merge failed: {error:#}");
                    log(
                        db,
                        &current_card,
                        "review.merge_failed",
                        "system",
                        &reason,
                        None,
                        json!({"target_branch":plan.target_branch,"request_key":key,"workflow_run_id":run.id}),
                        now_ms,
                    )?;
                    canban_workflow_engine::runtime::hold_delivery_at(
                        db,
                        &mut current_card,
                        &mut current_run,
                        &reason,
                        now_ms,
                    )?;
                }
            }
            Ok(())
        })?;
        self.git_changed();
        drop(_merging);
        if merged_and_completed {
            let _ = self.try_auto_archive_resolved(&card.id);
        }
        self.emit("review", Some(card.id));
        Ok((advance_queue, mutated))
    }

    pub(super) fn git_compare(&self, p: Value) -> Result<Value> {
        let ctx = self.resolve_project_checkout(&p)?;
        let card_ctx = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            if p["card_id"].as_str().is_some_and(|s| !s.trim().is_empty())
                || super::parse_card_number(&p["card"]).is_some()
            {
                let c = super::resolve_card(db, &p)?;
                let warning = activity_warning(db, &c)?;
                Some((c, warning))
            } else {
                None
            }
        };
        let project = ctx.project;
        let _permit = self.check();
        let ops = self.ops();
        let three_dot = p["three_dot"].as_bool().unwrap_or(true);

        if p["use_card_worktree"].as_bool().unwrap_or(false)
            && let Some((ref card, ref warning)) = card_ctx
        {
            if card.worktree.is_some() {
                let mut plan = ops.review_preview(&project, card, p["target_branch"].as_str())?;
                plan.warning = warning.clone();
                let (files, total_additions, total_deletions) =
                    canban_workspaces::compare::split_unified_patch(&plan.patch);
                let commits = ops
                    .git_compare(
                        &project,
                        &plan.target_head,
                        &plan.card_head,
                        card.worktree.as_deref(),
                        true,
                    )
                    .ok()
                    .and_then(|v| v["commits"].as_array().cloned())
                    .unwrap_or_default();
                return Ok(json!({
                    "base_ref": plan.target_branch,
                    "compare_ref": card.branch.clone().unwrap_or_else(|| "WORKDIR".into()),
                    "base_sha": plan.target_head,
                    "compare_sha": plan.card_head,
                    "three_dot": true,
                    "files": files,
                    "commits": commits,
                    "total_additions": total_additions,
                    "total_deletions": total_deletions,
                    "truncated": plan.truncated,
                    "cwd": plan.worktree,
                    "review_preview": plan,
                }));
            }
            if let Some(branch) = card.branch.as_deref().filter(|b| !b.is_empty()) {
                let base = p["target_branch"]
                    .as_str()
                    .or(project.target_branch.as_deref())
                    .unwrap_or("HEAD");
                return ops.git_compare(&project, base, branch, None, three_dot);
            }
        }

        let base = strp(&p, "base")?;
        let compare = strp(&p, "compare")?;
        ops.git_compare(&project, base, compare, Some(&ctx.cwd), three_dot)
    }

    pub(super) fn review_github_pr(&self, p: Value) -> Result<Value> {
        let repo = canban_github::repository(strp(&p, "repository")?)?;
        let number = p["number"].as_u64().context("Missing PR number")?;
        ensure!(number > 0, "Invalid PR number");
        let refresh = p["refresh"].as_bool().unwrap_or(false);

        let overview =
            self.github_read(&format!("/repos/{repo}/pulls/{number}"), refresh, false)?;
        let head_sha = overview.data["head"]["sha"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let base_sha = overview.data["base"]["sha"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let base_ref = overview.data["base"]["ref"]
            .as_str()
            .unwrap_or("main")
            .to_owned();
        let head_ref = overview.data["head"]["ref"]
            .as_str()
            .unwrap_or("head")
            .to_owned();

        let files_resp = self.github_read(
            &format!("/repos/{repo}/pulls/{number}/files?per_page=100"),
            refresh,
            false,
        )?;
        let raw_files = files_resp.data.as_array().cloned().unwrap_or_default();
        let mut total_additions = 0usize;
        let mut total_deletions = 0usize;
        let files: Vec<Value> = raw_files
            .into_iter()
            .map(|f| {
                let add = f["additions"].as_u64().unwrap_or(0) as usize;
                let del = f["deletions"].as_u64().unwrap_or(0) as usize;
                total_additions += add;
                total_deletions += del;
                json!({
                    "filename": f["filename"].as_str().unwrap_or(""),
                    "status": f["status"].as_str().unwrap_or("modified"),
                    "sha": f["sha"].as_str().unwrap_or(""),
                    "additions": add,
                    "deletions": del,
                    "patch": f["patch"].as_str().unwrap_or(""),
                })
            })
            .collect();

        let threads = self
            .github_read(
                &format!("/repos/{repo}/pulls/{number}/comments?per_page=100"),
                refresh,
                false,
            )
            .ok()
            .and_then(|r| r.data.as_array().cloned())
            .unwrap_or_default();

        Ok(json!({
            "repository": repo,
            "number": number,
            "overview": overview.data,
            "base_ref": base_ref,
            "compare_ref": head_ref,
            "base_sha": base_sha,
            "compare_sha": head_sha,
            "three_dot": true,
            "files": files,
            "commits": [],
            "threads": threads,
            "total_additions": total_additions,
            "total_deletions": total_deletions,
            "truncated": files.len() >= 100,
            "cached": overview.cached && files_resp.cached,
        }))
    }

    pub(super) fn review_github_pr_file(&self, p: Value) -> Result<Value> {
        let repo = canban_github::repository(strp(&p, "repository")?)?;
        let path = validate_draft_comment_path(strp(&p, "path")?)?;
        ensure!(!path.contains('\\'), "Invalid file path");
        let sha = p["sha"].as_str().map(str::trim).unwrap_or("");
        let blob_sha = p["blob_sha"].as_str().map(str::trim).unwrap_or("");
        let valid_sha =
            |s: &str| (7..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit());
        ensure!(
            valid_sha(sha) || valid_sha(blob_sha),
            "Missing valid GitHub commit or blob SHA"
        );
        let refresh = p["refresh"].as_bool().unwrap_or(false);

        let decode_base64_content = |data: &Value| -> Option<Result<Vec<u8>>> {
            let enc = data["encoding"].as_str().unwrap_or("");
            let content = data["content"].as_str().filter(|s| !s.is_empty())?;
            if enc != "base64" {
                return None;
            }
            let cleaned: String = content
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect();
            use base64::{Engine, engine::general_purpose::STANDARD};
            Some(STANDARD.decode(cleaned.as_bytes()).map_err(Into::into))
        };

        let mut bytes_opt: Option<Vec<u8>> = None;
        let mut fallback_blob = valid_sha(blob_sha).then(|| blob_sha.to_owned());

        if valid_sha(sha) {
            let encoded_path: String = path
                .split('/')
                .map(canban_github::segment)
                .collect::<Vec<_>>()
                .join("/");
            let contents_path = format!("/repos/{repo}/contents/{encoded_path}?ref={sha}");
            match self.github_read(&contents_path, refresh, false) {
                Ok(resp) => {
                    if let Some(decoded) = decode_base64_content(&resp.data) {
                        bytes_opt = Some(decoded?);
                    } else if let Some(s) = resp.data["sha"].as_str().filter(|s| valid_sha(s)) {
                        fallback_blob = Some(s.to_owned());
                    }
                }
                Err(err) => {
                    if fallback_blob.is_none() {
                        return Err(err);
                    }
                }
            }
        }

        if bytes_opt.is_none()
            && let Some(bsha) = fallback_blob
        {
            let blob_path = format!("/repos/{repo}/git/blobs/{bsha}");
            let resp = self.github_read(&blob_path, refresh, false)?;
            let decoded = decode_base64_content(&resp.data)
                .context("GitHub blob did not return base64 file content")??;
            bytes_opt = Some(decoded);
        }

        let bytes = bytes_opt.context("File content is unavailable from GitHub")?;
        let start_line = p["start_line"].as_u64().map(|n| n as usize);
        let end_line = p["end_line"].as_u64().map(|n| n as usize);
        canban_workspaces::compare::slice_context_file_bytes(&path, &bytes, start_line, end_line)
    }

    pub(super) fn review_send_agent(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Sending a review requires an explicit user action"
        );
        let mode = p["mode"].as_str().unwrap_or("card");
        let summary = p["summary"].as_str().unwrap_or("").trim();
        let inline = p["comments"].as_array().cloned().unwrap_or_default();
        ensure!(
            !summary.is_empty() || !inline.is_empty(),
            "Write a review summary or add at least one inline line comment"
        );

        let context_label = p["context_label"].as_str().unwrap_or("Git diff");
        let formatted = format_review_markdown(context_label, summary, &inline);

        match mode {
            "card" => {
                let card_id = strp(&p, "card_id")?;
                let profile_opt = p["profile_id"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());

                if p["move_to_develop"].as_bool().unwrap_or(false) {
                    let (current_stage, current_revision, has_develop) = {
                        let store = self.store.lock().unwrap();
                        let db = store.db();
                        let current: Card = db.get(card_id)?;
                        let proj: Project = db.get(&current.project_id)?;
                        (
                            current.stage,
                            current.revision,
                            proj.workflow.stages.iter().any(|s| s.id == "develop"),
                        )
                    };
                    if current_stage != "develop" && has_develop {
                        let move_key = p["request_key"]
                            .as_str()
                            .map(|k| format!("{k}:move"))
                            .unwrap_or_else(|| format!("review-move-{}", self.next_id()));
                        let _ = self.call(
                            "card.move",
                            json!({
                                "card_id": card_id,
                                "revision": current_revision,
                                "stage": "develop",
                                "request_key": move_key,
                            }),
                        );
                    }
                }

                let mentions: Vec<String> = profile_opt.into_iter().map(str::to_owned).collect();
                let comment_key = p["request_key"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("review-comment-{}", self.next_id()));
                let event = self.call(
                    "comment.add",
                    json!({
                        "card_id": card_id,
                        "body": formatted,
                        "mentions": mentions,
                        "request_key": comment_key,
                    }),
                )?;
                let card_record: Option<Card> = self.store.lock().unwrap().db().get(card_id).ok();
                Ok(json!({
                    "state": "sent",
                    "mode": "card",
                    "card_id": card_id,
                    "event": event,
                    "card_record": card_record,
                }))
            }
            "new_card" => {
                let project_id = strp(&p, "project_id")?;
                let title = p["title"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("Review follow-up: {context_label}"));
                let create_key = p["request_key"]
                    .as_str()
                    .map(|k| format!("{k}:create"))
                    .unwrap_or_else(|| format!("review-card-{}", self.next_id()));
                let created = self.call(
                    "card.create",
                    json!({
                        "project_id": project_id,
                        "title": title,
                        "body": formatted,
                        "mentions": [],
                        "request_key": create_key,
                    }),
                )?;
                let card_id = strp(&created, "id")?.to_owned();
                if let Some(profile_id) = p["profile_id"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    let ask_key = p["request_key"]
                        .as_str()
                        .map(|k| format!("{k}:ask"))
                        .unwrap_or_else(|| format!("review-ask-{}", self.next_id()));
                    let _ = self.call(
                        "comment.add",
                        json!({
                            "card_id": card_id,
                            "body": format!("@{profile_id} Please address the review findings in the Card description above."),
                            "mentions": [profile_id],
                            "request_key": ask_key,
                        }),
                    )?;
                }
                let card_record: Option<Card> = self.store.lock().unwrap().db().get(&card_id).ok();
                Ok(json!({
                    "state": "created",
                    "mode": "new_card",
                    "card_id": card_id,
                    "card_record": card_record,
                }))
            }
            "terminal" => self.launch_review_terminal(&p, &formatted),
            _ => bail!("Unsupported review dispatch mode: {mode}"),
        }
    }

    fn launch_review_terminal(&self, p: &Value, formatted_review: &str) -> Result<Value> {
        let _gate = self.terminal_gate.lock().unwrap();
        if let Some(cached) = self.cached_operation("review.send_agent", p)? {
            return Ok(cached);
        }
        let config = self.config();
        let project: Project = self
            .store
            .lock()
            .unwrap()
            .db()
            .get(strp(p, "project_id")?)?;
        let profile = canban_scheduler::select_in_project(
            &config,
            Some(&project),
            p["profile_id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        )?;
        let compare_ref = p["compare_ref"].as_str().unwrap_or("WORKDIR");
        let isolate = p["create_worktree"].as_bool().unwrap_or(false);
        let run_id = self.next_id();
        let run_slug = &run_id[..run_id.len().min(8)];
        let cwd = if isolate {
            self.ops()
                .prepare_review_worktree(&self.home, &project, compare_ref, run_slug)?
                .to_string_lossy()
                .into_owned()
        } else {
            p["cwd"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(&project.directory)
                .to_owned()
        };
        let host = canban_herdr::host::Host::with_runner(self.config(), self.exec_runner.clone());
        ensure!(
            Path::new(&cwd).is_absolute() && host.is_dir(Path::new(&cwd)),
            "Working directory is unavailable: {cwd}"
        );
        let agent_engine = {
            let cmd = profile.command_name();
            match cmd.as_str() {
                "codex" | "claude" => cmd,
                _ => profile.preset.clone().unwrap_or(cmd),
            }
        };

        let mut launch: Option<(TerminalEntry, Option<String>)> = None;
        let saved = self.mutate("review.send_agent", p, |db| {
            let session_id = profile
                .command()
                .iter()
                .any(|s| s.contains("{session_id}"))
                .then(|| self.next_id());
            let label = p["context_label"].as_str().unwrap_or("Diff Review");
            let title = format!("@{} · {}", profile.id, label);
            let title_trimmed: String = title.chars().take(76).collect();
            let item = TerminalEntry {
                id: format!("review-agent-{}", self.next_id()),
                conversation_id: None,
                number: db.counter("terminal-number")?,
                name: Some(title_trimmed),
                origin_project_id: Some(project.id.clone()),
                origin_card_id: None,
                agent_engine: Some(agent_engine),
                created_at: self.now_ms(),
                runtime: Some(TerminalRuntime {
                    core_id: None,
                    terminal: None,
                    state: "starting".into(),
                    connection: "checking".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                    observed_agent: None,
                    waiting: None,
                }),
                agent: None,
            };
            db.save(&item)?;
            launch = Some((item.clone(), session_id));
            Ok(json!({
                "state": "starting",
                "mode": "terminal",
                "terminal_id": item.id,
            }))
        })?;
        let Some((mut item, session_id)) = launch else {
            return Ok(saved);
        };

        let prompt = canban_context::render_template(
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../prompts/runtime/review_assist.txt"
            ))
            .trim_end(),
            &[
                ("project_name", &project.name),
                ("cwd", &cwd),
                ("formatted_review", formatted_review),
            ],
        );
        let client = self.herdr_client();
        let runtime = item.runtime.as_mut().unwrap();
        let result = (|| -> Result<()> {
            let core = self.verified_core_id()?;
            runtime.core_id = Some(core.clone());
            let mut argv = canban_profiles::command(
                &profile,
                session_id.as_deref(),
                true,
                &[
                    ("home", &self.home.to_string_lossy()),
                    ("cwd", &cwd),
                    ("project_dir", &project.directory),
                    ("cli", &self.cli.to_string_lossy()),
                    ("prompt", &prompt),
                ],
            )?;
            let is_herdr_template = profile
                .preset
                .as_deref()
                .is_some_and(|s| s.starts_with("herdr:"));
            if !argv.is_empty() && !is_herdr_template {
                argv[0] = canban_profiles::executable_with_runner(
                    &argv[0],
                    &client.config.hc_env,
                    self.exec_runner.as_ref(),
                );
            }
            let env = std::collections::BTreeMap::from([
                (
                    "CANBAN_HOME".into(),
                    self.home.to_string_lossy().into_owned(),
                ),
                (
                    "CANBAN_ENDPOINT".into(),
                    self.endpoint().to_string_lossy().into_owned(),
                ),
                ("CANBAN_CLI".into(), self.cli.to_string_lossy().into_owned()),
            ]);
            let receipt = self
                .launch_herdr_with_env(crate::env::HerdrLaunchRequest {
                    profile: Some(&profile),
                    project: Some(&project),
                    protected_context: &env,
                    workflow_env: None,
                    source_trust: Some("local"),
                    argv,
                    cwd: cwd.clone(),
                    labels: std::collections::BTreeMap::from([
                        ("canban.terminal".into(), item.id.clone()),
                        ("canban.project".into(), project.id.clone()),
                    ]),
                    size: None,
                })?
                .receipt;
            let address = strp(&receipt, "address")?.to_owned();
            runtime.terminal = Some(address.clone());
            {
                let mut inv = self.inventory.lock().unwrap();
                if inv.core == core {
                    inv.records.entry(address.clone()).or_insert_with(|| {
                        let mut seeded = receipt.clone();
                        seeded["address"] = json!(address);
                        if seeded["state"].is_null() {
                            seeded["state"] = json!("live");
                        }
                        if seeded["cwd"].is_null() {
                            seeded["cwd"] = json!(cwd);
                        }
                        seeded
                    });
                }
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                runtime.state = "running".into();
                runtime.connection = "connected".into();
            }
            Err(e) => {
                runtime.state = "unknown".into();
                runtime.connection = "unavailable".into();
                runtime.error = Some(format!("{e:#}"));
            }
        }
        let terminal_context = if runtime.state == "running"
            && let (Some(core_id), Some(terminal)) =
                (runtime.core_id.clone(), runtime.terminal.clone())
        {
            let ctx = TerminalContext {
                terminal_id: item.id.clone(),
                core_id,
                terminal,
                cwd: Some(cwd.clone()),
                directory_root: Some(if isolate {
                    cwd.clone()
                } else {
                    project.directory.clone()
                }),
                project_id: Some(project.id.clone()),
                card_id: None,
                worktree_id: None,
                branch: (!isolate)
                    .then(|| p["compare_ref"].as_str())
                    .flatten()
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && !matches!(*s, "WORKDIR" | "STAGED" | "UNSTAGED"))
                    .map(str::to_owned),
                dirty: false,
                additions: 0,
                deletions: 0,
                ahead: 0,
                behind: 0,
            };
            self.upsert_terminal_context(ctx.clone());
            Some(ctx)
        } else {
            None
        };
        let final_state = runtime.state.clone();
        let final_error = runtime.error.clone();
        let final_runtime = item.runtime.clone();
        let mut outcome = Value::Null;
        self.store.lock().unwrap().write(|db| {
            let mut current: TerminalEntry = db.get(&item.id)?;
            current.runtime = final_runtime.clone();
            db.save(&current)?;
            outcome = json!({
                "state": final_state,
                "mode": "terminal",
                "terminal_id": current.id,
                "cwd": cwd,
                "error": final_error,
                "terminal": current,
                "terminal_context": terminal_context,
            });
            if let Some(key) = p["request_key"].as_str() {
                db.conn.execute(
                    "UPDATE operations SET result=?2 WHERE id=?1",
                    params![key, serde_json::to_string(&outcome)?],
                )?;
            }
            Ok(())
        })?;
        self.emit("terminals", None);
        ensure!(
            outcome["error"].is_null(),
            "{}",
            outcome["error"]
                .as_str()
                .unwrap_or("Failed to launch Agent terminal")
        );
        Ok(outcome)
    }

    pub(super) fn review_draft_get(&self, p: Value) -> Result<Value> {
        let card = {
            let store = self.store.lock().unwrap();
            super::resolve_card(store.db(), &p)?
        };
        let key = format!("review-draft:{}", card.id);
        let raw: Option<String> = self
            .store
            .lock()
            .unwrap()
            .db()
            .conn
            .query_row(
                "SELECT data FROM editor_drafts WHERE id=?1 AND data != 'null'",
                [&key],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(data) => Ok(serde_json::from_str(&data)?),
            None => Ok(Value::Null),
        }
    }

    pub(super) fn review_draft_clear(&self, p: Value) -> Result<Value> {
        let card = {
            let store = self.store.lock().unwrap();
            super::resolve_card(store.db(), &p)?
        };
        let key = format!("review-draft:{}", card.id);
        self.store.lock().unwrap().write(|db| {
            db.conn
                .execute("DELETE FROM editor_drafts WHERE id=?1", [&key])?;
            Ok(())
        })?;
        let cleared_at = self.now_ms();
        self.emit("review", Some(card.id.clone()));
        Ok(json!({
            "cleared": true,
            "card_id": card.id,
            "card": card.number,
            "cleared_at": cleared_at,
        }))
    }

    pub(crate) fn clear_review_draft_for_pr(
        &self,
        repository: &str,
        number: u64,
        card_id: Option<&str>,
    ) -> Result<()> {
        let mut target_cards = Vec::new();
        {
            let store = self.store.lock().unwrap();
            let db = store.db();
            if let Some(cid) = card_id.map(str::trim).filter(|s| !s.is_empty()) {
                target_cards.push(cid.to_owned());
            } else {
                for c in db.list::<Card>()? {
                    let matches_source = c.is_github_pr()
                        && c.github_issue().is_some_and(|s| {
                            s.repository.eq_ignore_ascii_case(repository) && s.number == number
                        });
                    let matches_delivery = c.pull_request.as_ref().is_some_and(|pr| {
                        pr.repository.eq_ignore_ascii_case(repository) && pr.number == number
                    });
                    if matches_source || matches_delivery {
                        target_cards.push(c.id);
                    }
                }
            }
        }
        if target_cards.is_empty() {
            return Ok(());
        }
        self.store.lock().unwrap().write(|db| {
            for cid in &target_cards {
                let key = format!("review-draft:{cid}");
                db.conn
                    .execute("DELETE FROM editor_drafts WHERE id=?1", [&key])?;
            }
            Ok(())
        })?;
        for cid in target_cards {
            self.emit("review", Some(cid));
        }
        Ok(())
    }

    pub(super) fn review_draft_set(&self, p: Value) -> Result<Value> {
        let card = {
            let store = self.store.lock().unwrap();
            super::resolve_card(store.db(), &p)?
        };
        let summary = p["summary"].as_str().unwrap_or("").trim().to_owned();
        let verdict = normalize_review_verdict(p["verdict"].as_str())?;
        let raw_comments = p["comments"].as_array().cloned().unwrap_or_default();
        let profile_author = p["profile_id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("@{}", s.trim_start_matches('@')));
        let top_author = p["author"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("@{}", s.trim_start_matches('@')))
            .or(profile_author);

        let mut comments = Vec::with_capacity(raw_comments.len());
        for item in &raw_comments {
            let path = validate_draft_comment_path(
                item["path"]
                    .as_str()
                    .context("Each draft comment requires a file 'path'")?,
            )?;
            let line = item["line"]
                .as_u64()
                .filter(|&n| n > 0)
                .context("Each draft comment requires a positive 'line' number")?;
            let side_raw = item["side"]
                .as_str()
                .unwrap_or("RIGHT")
                .trim()
                .to_ascii_uppercase();
            let side = match side_raw.as_str() {
                "" | "RIGHT" => "RIGHT".to_owned(),
                "LEFT" => "LEFT".to_owned(),
                other => anyhow::bail!("Invalid comment side '{other}'; expected LEFT or RIGHT"),
            };
            let body = item["body"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .context("Each draft comment requires a non-empty 'body'")?
                .to_owned();
            let edited_by_user = item["edited_by_user"].as_bool().unwrap_or(false);
            let item_author = item["author"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| format!("@{}", s.trim_start_matches('@')))
                .or_else(|| {
                    if edited_by_user {
                        None
                    } else {
                        top_author.clone()
                    }
                });
            let cid = item["id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| self.next_id());
            comments.push(ReviewDraftComment {
                id: cid,
                path,
                section: item["section"].as_str().unwrap_or("").trim().to_owned(),
                line,
                side,
                snippet: item["snippet"].as_str().unwrap_or("").trim().to_owned(),
                body,
                author: item_author,
                edited_by_user,
            });
        }

        let key = format!("review-draft:{}", card.id);
        if summary.is_empty() && comments.is_empty() {
            self.store.lock().unwrap().write(|db| {
                db.conn
                    .execute("DELETE FROM editor_drafts WHERE id=?1", [&key])?;
                Ok(())
            })?;
            self.emit("review", Some(card.id.clone()));
            return Ok(json!({
                "saved": true,
                "cleared": true,
                "card_id": card.id,
                "card": card.number,
                "comment_count": 0,
            }));
        }

        let repository = p["repository"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                if card.is_github_pr() {
                    card.github_issue().map(|s| s.repository.clone())
                } else {
                    card.pull_request.as_ref().map(|pr| pr.repository.clone())
                }
            });
        let pr_number = p["pr_number"].as_u64().filter(|&n| n > 0).or_else(|| {
            if card.is_github_pr() {
                card.github_issue().map(|s| s.number)
            } else {
                card.pull_request
                    .as_ref()
                    .filter(|pr| pr.number > 0)
                    .map(|pr| pr.number)
            }
        });
        let compare_sha = p["compare_sha"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                if let (Some(repo), Some(num)) = (&repository, pr_number) {
                    self.github_cached_client()
                        .ok()
                        .and_then(|c| c.cached::<Value>(&format!("/repos/{repo}/pulls/{num}")))
                        .and_then(|v| v["head"]["sha"].as_str().map(str::to_owned))
                } else {
                    None
                }
            })
            .or_else(|| {
                card.worktree
                    .as_deref()
                    .and_then(|wt| self.ops().head_sha(Path::new(wt)))
            });

        let draft = ReviewDraft {
            card_id: card.id.clone(),
            repository,
            pr_number,
            compare_sha,
            summary,
            verdict: verdict.clone(),
            comments,
            author: top_author,
            updated_at: self.now_ms(),
        };
        let encoded = serde_json::to_string(&draft)?;
        ensure!(
            encoded.len() <= 4 * 1024 * 1024,
            "Review draft exceeds 4 MiB"
        );

        self.store.lock().unwrap().write(|db| {
            let prev_rev: Option<u64> = db
                .conn
                .query_row(
                    "SELECT revision FROM editor_drafts WHERE id=?1",
                    [&key],
                    |r| r.get(0),
                )
                .optional()?;
            let next = prev_rev.unwrap_or(0) + 1;
            db.conn.execute(
                "INSERT INTO editor_drafts(id,revision,updated_at,data) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,updated_at=excluded.updated_at,data=excluded.data",
                params![key, next, draft.updated_at, encoded],
            )?;
            Ok(())
        })?;
        self.emit("review", Some(card.id.clone()));
        let comment_count = draft.comments.len();
        Ok(json!({
            "saved": true,
            "card_id": card.id,
            "card": card.number,
            "comment_count": comment_count,
            "verdict": verdict,
            "draft": draft,
        }))
    }

    pub(super) fn review_run_agent(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Starting an Agent review requires an explicit user action"
        );
        let config = self.config();
        let (card, bound_project, profile) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let c = super::resolve_card(db, &p)?;
            ensure!(
                c.active(),
                "Restore the Card before running an Agent review"
            );
            ensure!(!c.resolved(), "Resolved Cards do not need a new PR review");
            let proj: Project = db.get(&c.project_id)?;
            let bound = self.bound_project(&proj, &c);
            let explicit_profile = p["profile_id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let profile = resolve_review_profile(&config, Some(&bound), explicit_profile)?;
            let already_active = db.list::<TerminalEntry>()?.into_iter().any(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.card_id == c.id
                        && a.trigger == ExecutionTrigger::Custom("diff_review".into())
                        && !a.finished()
                        && !a.agent_stopped()
                })
            });
            ensure!(
                !already_active,
                "An Agent review is already running for this Card"
            );
            (c, bound, profile)
        };

        let has_ready_worktree = card.worktree.as_deref().is_some_and(|wt| {
            !wt.is_empty()
                && card.branch.as_deref().is_some_and(|b| !b.is_empty())
                && card
                    .workspace
                    .as_ref()
                    .is_some_and(|w| w.directory == bound_project.directory && w.path == wt)
                && self.ops().host.is_dir(Path::new(wt))
        });
        let (prepared_card, prepared_project) = if has_ready_worktree {
            (card, bound_project)
        } else {
            let _gate = self.terminal_gate.lock().unwrap();
            self.ensure_card_worktree(&card.id, None)?
        };
        let cwd = prepared_card
            .worktree
            .clone()
            .unwrap_or_else(|| prepared_project.directory.clone());

        let base_ref = prepared_card
            .workspace
            .as_ref()
            .and_then(|w| w.target_branch.clone())
            .or_else(|| prepared_project.target_branch.clone())
            .unwrap_or_else(|| "main".into());
        let compare_ref = prepared_card
            .branch
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "HEAD".into());

        let pr_info = if prepared_card.is_github_pr() {
            prepared_card
                .github_issue()
                .map(|s| (s.repository.clone(), s.number))
        } else {
            prepared_card
                .pull_request
                .as_ref()
                .filter(|pr| pr.number > 0)
                .map(|pr| (pr.repository.clone(), pr.number))
        };
        let pr_head_sha = pr_info.as_ref().and_then(|(repo, num)| {
            self.github_cached_client()
                .ok()
                .and_then(|c| c.cached::<Value>(&format!("/repos/{repo}/pulls/{num}")))
                .and_then(|v| v["head"]["sha"].as_str().map(str::to_owned))
        });
        let compare_sha = pr_head_sha
            .or_else(|| self.ops().head_sha(Path::new(&cwd)))
            .unwrap_or_else(|| "HEAD".into());

        let context_label = if let Some((repo, num)) = &pr_info {
            format!(
                "PR {repo}#{num} (Card #{}: {})",
                prepared_card.number, prepared_card.title
            )
        } else {
            format!("Card #{}: {}", prepared_card.number, prepared_card.title)
        };
        let focus = p["focus"].as_str().unwrap_or("").trim();
        let focus_section = if focus.is_empty() {
            String::new()
        } else {
            format!("\nFocus areas requested by the user:\n{focus}\n")
        };
        let instruction = canban_context::render_template(
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../prompts/runtime/diff_review.txt"
            ))
            .trim_end(),
            &[
                ("context_label", &context_label),
                ("base_ref", &base_ref),
                ("compare_ref", &compare_ref),
                ("compare_sha", &compare_sha),
                ("cwd", &cwd),
                ("focus_section", &focus_section),
            ],
        );

        let profile_id = profile.id.clone();
        self.mutate("review.run_agent", &p, |db| {
            let mut current: Card = db.get(&prepared_card.id)?;
            ensure!(
                current.active(),
                "Restore the Card before running an Agent review"
            );
            let proj: Project = db.get(&current.project_id)?;
            let stage = canban_workflow_engine::stage(&proj.workflow, &current.stage)?;
            let entry = enqueue(
                db,
                &config,
                &mut current,
                &proj,
                stage,
                ExecutionTrigger::Custom("diff_review".into()),
                Some(&profile_id),
                Some(instruction),
                self.now_ms(),
            )?
            .context("Agent review could not be queued")?;
            current.revision += 1;
            db.save(&current)?;
            Ok(json!({
                "state": "queued",
                "card_id": current.id,
                "card": current.number,
                "profile_id": profile_id,
                "terminal_id": entry.id,
                "card_record": current,
                "terminal": entry,
            }))
        })
    }
}

fn format_review_markdown(context_label: &str, summary: &str, inline: &[Value]) -> String {
    let mut out = format!("### Code Review ({context_label})\n");
    if !summary.is_empty() {
        out.push('\n');
        out.push_str(summary);
        out.push('\n');
    }
    if !inline.is_empty() {
        out.push_str("\n#### Inline Comments\n");
        for (idx, item) in inline.iter().enumerate() {
            let path = item["path"].as_str().unwrap_or("unknown");
            let line = item["line"].as_u64().unwrap_or(1);
            let side = item["side"].as_str().unwrap_or("RIGHT");
            let snippet = item["snippet"].as_str().unwrap_or("").trim();
            let body = item["body"].as_str().unwrap_or("").trim();
            let side_note = if side == "LEFT" { " (old)" } else { "" };
            out.push_str(&format!("\n{}. **`{path}:{line}`**{side_note}\n", idx + 1));
            if !snippet.is_empty() {
                out.push_str(&format!("   > `{snippet}`\n"));
            }
            if !body.is_empty() {
                for b_line in body.lines() {
                    out.push_str(&format!("   {b_line}\n"));
                }
            }
        }
    }
    out
}
