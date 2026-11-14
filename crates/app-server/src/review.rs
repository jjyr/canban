use super::*;
pub(super) fn activity_warning(db: Db<'_>, card: &Card) -> Result<Option<String>> {
    Ok(canban_types::agent_activity_warning(
        &card.id,
        &db.list::<TerminalEntry>()?,
    ))
}

fn default_project_merge_strategy(db: Db<'_>, card: &Card) -> Result<MergeStrategy> {
    for run in db.list::<WorkflowRun>()? {
        if run.card_id == card.id
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
        let mut plan = self.ops(&project.work_location().machine).review_preview(
            &project,
            &card,
            p["target_branch"].as_str(),
        )?;
        plan.warning = warning;
        Ok(serde_json::to_value(plan)?)
    }
    pub(super) fn review_merge(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Code acceptance requires explicit human review"
        );
        let _gate = self.review_gate.try_lock().map_err(|_| {
            anyhow::anyhow!("Another code delivery is in progress; retry after it completes")
        })?;
        let key = strp(&p, "request_key")?.to_owned();
        let requested_strategy = match p["strategy"].as_str() {
            Some(raw) if !raw.trim().is_empty() => Some(
                MergeStrategy::parse(raw)
                    .with_context(|| format!("Unknown merge strategy `{}`", raw.trim()))?,
            ),
            _ => None,
        };
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
            let plan=self.ops(&project.work_location().machine).review_preview(&project,&card,p["target_branch"].as_str())?;
            ensure!(plan.token==strp(&p,"token")?,"Review is stale; refresh the diff before accepting");
            ensure!(plan.merge_allowed,"{}",plan.reason.as_deref().unwrap_or("Merge unavailable"));
            log(db,&card,"review.merge_started","user","Accepted reviewed changes for commit/merge",None,json!({"request_key":key,"target_branch":plan.target_branch,"target_head":plan.target_head,"card_head":plan.card_head,"strategy":strategy,"token":plan.token}))?;
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
        let result = self
            .ops(&project.work_location().machine)
            .review_merge_with_strategy(
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
            let value=match result {
                Ok(commit)=>{
                    current.delivery_mode=Some(DeliveryMode::Local);
                    current.delivery=Some(Delivery{target_branch:plan.target_branch.clone(),commit:commit.clone(),request_key:key.clone(),merged_at:now()});
                    current.attention=None;
                    let gate = current.human_gate.take();
                    let mut completed_deliver = false;
                    for mut active_run in db.list::<WorkflowRun>()? {
                        if active_run.card_id == current.id
                            && active_run.active()
                            && active_run.step.as_ref().is_some_and(|k| {
                                matches!(active_run.definition.steps.get(k), Some(WorkflowStep::Deliver { .. }))
                            })
                        {
                            completed_deliver = true;
                            canban_workflow_engine::runtime::complete_delivery(
                                db,
                                &self.config(),
                                &mut current,
                                &mut active_run,
                                json!({"state":"merged","commit":commit,"target_branch":plan.target_branch,"manual":true}),
                                enqueue,
                            )?;
                        }
                    }
                    if !completed_deliver
                        && let Some(gate) = gate
                        && let Some(tid) = gate.terminal_id
                    {
                        let att: TerminalEntry = db.get(&tid)?;
                        if att.agent.as_ref().is_some_and(|a| a.state == "completed") {
                            crate::workflow::advance(db, &self.config(), &mut current, Some(&att))?;
                        }
                    }
                    log(db,&current,"review.merged","user",&format!("Merged into {}",plan.target_branch),None,json!({"commit":commit,"target_branch":plan.target_branch,"request_key":key}))?;
                    json!({"state":"merged","commit":commit,"target_branch":plan.target_branch,"card_id":card.id})
                }
                Err(error)=>{
                    let message=format!("{error:#}");attention(db,&mut current,&message)?;
                    log(db,&current,"review.merge_failed","system",&message,None,json!({"target_branch":plan.target_branch,"request_key":key}))?;
                    json!({"state":"failed","message":message,"target_branch":plan.target_branch,"card_id":card.id})
                }
            };
            current.revision+=1;db.save(&current)?;
            db.conn.execute("UPDATE operations SET result=?2 WHERE id=?1",params![key,serde_json::to_string(&value)?])?;
            Ok(value)
        })?;
        self.git_changed(&project.work_location().machine);
        drop(_merging);
        drop(_gate);
        if value["state"] == "merged" {
            let _ = self.try_auto_archive_merged(&card.id);
            self.request_deliveries();
            self.request_scripts();
        }
        self.emit("review", Some(card.id));
        Ok(value)
    }
    pub(crate) fn try_auto_archive_merged(&self, card_id: &str) -> Result<bool> {
        self.try_auto_archive_merged_at(card_id, now())
    }

    pub(crate) fn try_auto_archive_merged_at(&self, card_id: &str, now_ms: u64) -> Result<bool> {
        let (card, project) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let Ok(card) = db.get::<Card>(card_id) else {
                return Ok(false);
            };
            if !card.active()
                || !card.merged()
                || card.attention.is_some()
                || card.human_gate.is_some()
                || activity_warning(db, &card)?.is_some()
            {
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
                                | "card.restored"
                                | "card.archived"
                        )
                })
                .max_by_key(|e| e.sequence);
            if last_lifecycle
                .as_ref()
                .is_some_and(|e| matches!(e.kind.as_str(), "card.restored" | "card.archived"))
            {
                return Ok(false);
            }
            // Keep Cards merged today visible on the Board until the next local calendar day.
            let merged_at = card
                .delivery
                .as_ref()
                .map(|d| d.merged_at)
                .or_else(|| last_lifecycle.as_ref().map(|e| e.created_at))
                .unwrap_or(card.created_at);
            let same_local_day: bool = db.conn.query_row(
                "SELECT date(?1 / 1000, 'unixepoch', 'localtime') >= date(?2 / 1000, 'unixepoch', 'localtime')",
                params![merged_at as i64, now_ms as i64],
                |r| r.get(0),
            )?;
            if same_local_day {
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
            if let Ok(info) = self
                .ops(&project.work_location().machine)
                .info(&project, &card)
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

    pub(crate) fn pump_merged_archives(&self) -> Result<()> {
        let now_ms = now();
        let candidates: Vec<String> = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let today: String = db.conn.query_row(
                "SELECT date(?1 / 1000, 'unixepoch', 'localtime')",
                params![now_ms as i64],
                |r| r.get(0),
            )?;
            let mut ids = Vec::new();
            for c in db.list::<Card>()? {
                if !c.active() || !c.merged() || c.attention.is_some() || c.human_gate.is_some() {
                    continue;
                }
                if let Some(d) = &c.delivery {
                    let merged_day: String = db.conn.query_row(
                        "SELECT date(?1 / 1000, 'unixepoch', 'localtime')",
                        params![d.merged_at as i64],
                        |r| r.get(0),
                    )?;
                    if merged_day >= today {
                        continue;
                    }
                }
                ids.push(c.id);
            }
            ids
        };
        for id in candidates {
            let _ = self.try_auto_archive_merged_at(&id, now_ms);
        }
        Ok(())
    }

    pub(crate) fn pump_deliveries(&self) -> Result<()> {
        let Ok(_gate) = self.review_gate.try_lock() else {
            return Ok(());
        };
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
        for project_id in project_ids {
            self.pump_project_delivery_queue(&project_id)?;
        }
        Ok(())
    }

    fn pump_project_delivery_queue(&self, project_id: &str) -> Result<()> {
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
                    .filter(|r| {
                        r.project_id == project_id
                            && r.state == "waiting_delivery"
                            && cards
                                .get(&r.card_id)
                                .is_some_and(|c| c.active() && c.hold_auto_merge)
                    })
                    .map(|r| (r.card_id, r.id))
                    .collect()
            };
            for (card_id, run_id) in held_runs {
                self.store.lock().unwrap().write(|db| {
                    let mut current_card: Card = db.get(&card_id)?;
                    let mut current_run: WorkflowRun = db.get(&run_id)?;
                    if current_run.state == "waiting_delivery" && current_card.hold_auto_merge {
                        canban_workflow_engine::runtime::hold_delivery(
                            db,
                            &mut current_card,
                            &mut current_run,
                            "Auto-merge held by user; approve or resume to merge",
                        )?;
                    }
                    Ok(())
                })?;
                self.emit("application", Some(card_id));
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
                    if !self.execute_workflow_delivery(&run_id)? {
                        break;
                    }
                }
                crate::merge_batch::BatchSelection::Batch { run_ids } => {
                    if !self.execute_batch_delivery(project_id, &run_ids)? {
                        break;
                    }
                }
            }
        }
        Ok(())
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
            current_run.updated_at = now();
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
    /// Returns `Ok(true)` when this run exited the head of the queue (merged or stepped aside
    /// to a human gate) so the caller can immediately advance to the next Card in line, or
    /// `Ok(false)` when this run still holds the head of the queue (settling, conflict handed
    /// to `@merger`, or waiting on a transient main-checkout condition).
    fn execute_workflow_delivery(&self, run_id: &str) -> Result<bool> {
        let (card, project, run, has_pending_attempts, settling_agent) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let Ok(run) = db.get::<WorkflowRun>(run_id) else {
                return Ok(false);
            };
            if run.state != "waiting_delivery" {
                return Ok(false);
            }
            let card: Card = db.get(&run.card_id)?;
            if !card.active() {
                return Ok(false);
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
            let now_ms = now();
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
            self.store.lock().unwrap().write(|db| {
                let mut current_card: Card = db.get(&card.id)?;
                let mut current_run: WorkflowRun = db.get(&run.id)?;
                if current_run.state == "waiting_delivery" {
                    canban_workflow_engine::runtime::hold_delivery(
                        db,
                        &mut current_card,
                        &mut current_run,
                        "Auto-merge held by user; approve or resume to merge",
                    )?;
                }
                Ok(())
            })?;
            self.emit("application", Some(card.id));
            return Ok(true);
        }
        // No worktree means there is nothing to merge (e.g. create_worktree: false);
        // complete the step without waiting on Agent terminals.
        if card.worktree.is_none() {
            self.store.lock().unwrap().write(|db| {
                let mut current_card: Card = db.get(&card.id)?;
                let mut current_run: WorkflowRun = db.get(&run.id)?;
                if current_run.state == "waiting_delivery" {
                    canban_workflow_engine::runtime::complete_delivery(
                        db,
                        &config,
                        &mut current_card,
                        &mut current_run,
                        json!({"state":"delivered","mode":"no_worktree"}),
                        enqueue,
                    )?;
                }
                Ok(())
            })?;
            self.emit("application", Some(card.id));
            return Ok(true);
        }
        if has_pending_attempts || settling_agent {
            return Ok(false);
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
        let ops = self.ops(&project.work_location().machine);
        if sync_upstream
            && ops.has_push_remote(&workspace_project)
            && let Err(e) = ops.sync_target(&workspace_project, target_branch)
        {
            if let Some(canban_workspaces::review::DeliveryError::MainCheckoutBusy {
                reason: busy,
            }) = e.downcast_ref::<canban_workspaces::review::DeliveryError>()
            {
                let pause_msg = format!("Merge queue waiting: {busy}");
                self.set_queue_pause_reason(&card.id, &run.id, Some(&pause_msg))?;
                return Ok(false);
            }
            let reason = format!("Auto-merge paused (upstream sync): {e:#}");
            self.store.lock().unwrap().write(|db| {
                let mut current_card: Card = db.get(&card.id)?;
                let mut current_run: WorkflowRun = db.get(&run.id)?;
                if current_run.state == "waiting_delivery" {
                    canban_workflow_engine::runtime::hold_delivery(
                        db,
                        &mut current_card,
                        &mut current_run,
                        &reason,
                    )?;
                }
                Ok(())
            })?;
            self.emit("review", Some(card.id));
            return Ok(true);
        }
        let plan = match ops.review_preview(&workspace_project, &card, target_branch) {
            Ok(plan) => plan,
            Err(e) => {
                let reason = format!("Auto-merge paused: {e:#}");
                self.store.lock().unwrap().write(|db| {
                    let mut current_card: Card = db.get(&card.id)?;
                    let mut current_run: WorkflowRun = db.get(&run.id)?;
                    if current_run.state == "waiting_delivery" {
                        canban_workflow_engine::runtime::hold_delivery(
                            db,
                            &mut current_card,
                            &mut current_run,
                            &reason,
                        )?;
                    }
                    Ok(())
                })?;
                self.emit("review", Some(card.id));
                return Ok(true);
            }
        };
        if !plan.merge_allowed {
            let pending_conflict = ops
                .pending_merge_conflict(&workspace_project, &card, target_branch)
                .ok()
                .flatten();
            if pending_conflict.is_none()
                && let Some(raw_reason) = plan.reason.as_deref()
                && canban_workspaces::review::is_main_checkout_busy_reason(raw_reason)
            {
                let pause_msg = format!("Merge queue waiting: {raw_reason}");
                self.set_queue_pause_reason(&card.id, &run.id, Some(&pause_msg))?;
                return Ok(false);
            }
            let reason = format!(
                "Auto-merge paused: {}",
                plan.reason
                    .as_deref()
                    .unwrap_or("Merge unavailable; human review required")
            );
            let mut dispatched_conflict = false;
            self.store.lock().unwrap().write(|db| {
                let mut current_card: Card = db.get(&card.id)?;
                let mut current_run: WorkflowRun = db.get(&run.id)?;
                if current_run.state == "waiting_delivery" {
                    if let Some((ref conflict_branch, ref conflict_head, ref files)) =
                        pending_conflict
                        && canban_workflow_engine::runtime::rework_conflict(
                            db,
                            &config,
                            &mut current_card,
                            &mut current_run,
                            conflict_branch,
                            conflict_head,
                            files,
                            enqueue,
                        )?
                    {
                        dispatched_conflict = true;
                        return Ok(());
                    }
                    canban_workflow_engine::runtime::hold_delivery(
                        db,
                        &mut current_card,
                        &mut current_run,
                        &reason,
                    )?;
                }
                Ok(())
            })?;
            self.emit("review", Some(card.id));
            return Ok(!dispatched_conflict);
        }
        // Main checkout is clean and ready: clear any previous transient wait reason.
        let _ = self.set_queue_pause_reason(&card.id, &run.id, None);
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
        let mut merged_and_completed = false;
        let mut advance_queue = true;
        self.store.lock().unwrap().write(|db| {
            let mut current_card: Card = db.get(&card.id)?;
            let mut current_run: WorkflowRun = db.get(&run.id)?;
            if current_run.state != "waiting_delivery" {
                advance_queue = false;
                return Ok(());
            }
            match result {
                Ok(commit) => {
                    current_card.delivery_mode = Some(DeliveryMode::Local);
                    current_card.delivery = Some(Delivery {
                        target_branch: plan.target_branch.clone(),
                        commit: commit.clone(),
                        request_key: key.clone(),
                        merged_at: now(),
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
                            )?;
                            canban_workflow_engine::runtime::hold_delivery(
                                db,
                                &mut current_card,
                                &mut current_run,
                                &reason,
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
                            )?;
                            canban_workflow_engine::runtime::complete_delivery(
                                db,
                                &config,
                                &mut current_card,
                                &mut current_run,
                                json!({"state":"merged","commit":commit,"target_branch":plan.target_branch,"synced_upstream":true,"remote":remote_name}),
                                enqueue,
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
                            )?;
                            canban_workflow_engine::runtime::complete_delivery(
                                db,
                                &config,
                                &mut current_card,
                                &mut current_run,
                                json!({"state":"merged","commit":commit,"target_branch":plan.target_branch,"synced_upstream":false,"reason":"no_remote"}),
                                enqueue,
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
                            )?;
                            canban_workflow_engine::runtime::complete_delivery(
                                db,
                                &config,
                                &mut current_card,
                                &mut current_run,
                                json!({"state":"merged","commit":commit,"target_branch":plan.target_branch,"synced_upstream":false}),
                                enqueue,
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
                            )?;
                            return Ok(());
                        }
                        Some(DeliveryError::MainCheckoutBusy { reason: busy }) => {
                            current_run.inputs["queue_pause_reason"] =
                                json!(format!("Merge queue waiting: {busy}"));
                            current_run.updated_at = now();
                            db.save(&current_run)?;
                            advance_queue = false;
                            return Ok(());
                        }
                        Some(DeliveryError::Conflict {
                            target_branch,
                            target_head,
                            files,
                            ..
                        }) if canban_workflow_engine::runtime::rework_conflict(
                            db,
                            &config,
                            &mut current_card,
                            &mut current_run,
                            target_branch,
                            target_head,
                            files,
                            enqueue,
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
                    )?;
                    canban_workflow_engine::runtime::hold_delivery(
                        db,
                        &mut current_card,
                        &mut current_run,
                        &reason,
                    )?;
                }
            }
            Ok(())
        })?;
        self.git_changed(&project.work_location().machine);
        drop(_merging);
        if merged_and_completed {
            let _ = self.try_auto_archive_merged(&card.id);
        }
        self.emit("review", Some(card.id));
        Ok(advance_queue)
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
        let machine = project.work_location().machine;
        let _permit = self.check(&machine);
        let ops = self.ops(&machine);
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

                // When reviewing a GitHub PR Card that has no local worktree yet and dispatching
                // to a local Agent, seed `refs/canban/cards/<card_id>` from `refs/pull/<n>/head`
                // so `canban_workspaces::Ops::prepare` checks out the exact PR head commit!
                if profile_opt.is_some() {
                    let seed_target = {
                        let store = self.store.lock().unwrap();
                        let db = store.db();
                        let c: Card = db.get(card_id)?;
                        if c.worktree.is_none()
                            && c.branch.is_none()
                            && let Some(pr_num) = c
                                .github_issue()
                                .filter(|_| c.is_github_pr())
                                .map(|s| s.number)
                                .or_else(|| c.pull_request.as_ref().map(|pr| pr.number))
                        {
                            let proj: Project = db.get(&c.project_id)?;
                            Some((self.bound_project(&proj, &c), pr_num, c.id.clone()))
                        } else {
                            None
                        }
                    };
                    if let Some((proj, pr_num, cid)) = seed_target {
                        let _ = self
                            .ops(&proj.work_location().machine)
                            .fetch_pr_for_card(&proj, pr_num, &cid);
                    }
                }

                if p["move_to_develop"].as_bool().unwrap_or(false) {
                    let current: Card = self.store.lock().unwrap().db().get(card_id)?;
                    let proj: Project = self.store.lock().unwrap().db().get(&current.project_id)?;
                    if current.stage != "develop"
                        && proj.workflow.stages.iter().any(|s| s.id == "develop")
                    {
                        let move_key = p["request_key"]
                            .as_str()
                            .map(|k| format!("{k}:move"))
                            .unwrap_or_else(|| format!("review-move-{}", id()));
                        let _ = self.call(
                            "card.move",
                            json!({
                                "card_id": card_id,
                                "revision": current.revision,
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
                    .unwrap_or_else(|| format!("review-comment-{}", id()));
                let event = self.call(
                    "comment.add",
                    json!({
                        "card_id": card_id,
                        "body": formatted,
                        "mentions": mentions,
                        "request_key": comment_key,
                    }),
                )?;
                Ok(json!({
                    "state": "sent",
                    "mode": "card",
                    "card_id": card_id,
                    "event": event,
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
                    .unwrap_or_else(|| format!("review-card-{}", id()));
                let created = self.call(
                    "card.create",
                    json!({
                        "project_id": project_id,
                        "title": title,
                        "body": formatted,
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
                        .unwrap_or_else(|| format!("review-ask-{}", id()));
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
                Ok(json!({
                    "state": "created",
                    "mode": "new_card",
                    "card_id": card_id,
                }))
            }
            "terminal" => self.launch_review_terminal(&p, &formatted),
            _ => bail!("Unsupported review dispatch mode: {mode}"),
        }
    }

    fn launch_review_terminal(&self, p: &Value, formatted_review: &str) -> Result<Value> {
        let _gate = self.terminal_gate.lock().unwrap();
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
        let machine = project.work_location().machine;
        let compare_ref = p["compare_ref"].as_str().unwrap_or("WORKDIR");
        let isolate = p["create_worktree"].as_bool().unwrap_or(false);

        let mut launch: Option<(TerminalEntry, String, Option<String>)> = None;
        let saved = self.mutate("review.send_agent", p, |db| {
            let run_slug = &id()[..8];
            let cwd = if isolate {
                self.ops(&machine)
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
            let host = canban_herdr::host::Host::new(self.config(), &machine);
            ensure!(
                Path::new(&cwd).is_absolute() && host.is_dir(Path::new(&cwd)),
                "Working directory is unavailable on machine '{machine}': {cwd}"
            );
            let session_id = profile
                .command()
                .iter()
                .any(|s| s.contains("{session_id}"))
                .then(id);
            let label = p["context_label"].as_str().unwrap_or("Diff Review");
            let title = format!("@{} · {}", profile.id, label);
            let title_trimmed: String = title.chars().take(76).collect();
            let item = TerminalEntry {
                id: format!("review-agent-{}", id()),
                conversation_id: None,
                number: db.counter("terminal-number")?,
                name: Some(title_trimmed),
                manual_number: Some(db.counter("terminal-manual-number")?),
                origin_card_id: None,
                agent_engine: None,
                created_at: now(),
                runtime: Some(TerminalRuntime {
                    machine: machine.clone(),
                    owner_core_id: None,
                    core_id: None,
                    terminal: None,
                    state: "starting".into(),
                    connection: "checking".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                }),
                agent: None,
            };
            db.save(&item)?;
            launch = Some((item.clone(), cwd, session_id));
            Ok(json!({
                "state": "starting",
                "mode": "terminal",
                "terminal_id": item.id,
            }))
        })?;
        let Some((mut item, cwd, session_id)) = launch else {
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
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let runtime = item.runtime.as_mut().unwrap();
        let result = (|| -> Result<()> {
            let core = client.status()?["core"]["id"]
                .as_str()
                .context("Herdr Core has no identity")?
                .to_owned();
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
            if machine == "local" && !argv.is_empty() && !is_herdr_template {
                argv[0] = canban_profiles::executable(&argv[0]);
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
            let receipt = client.launch_on(
                &machine,
                argv,
                &cwd,
                env,
                std::collections::BTreeMap::from([
                    ("canban.terminal".into(), item.id.clone()),
                    ("canban.project".into(), project.id.clone()),
                ]),
            )?;
            let address = strp(&receipt, "address")?.to_owned();
            runtime.owner_core_id = if machine == "local" {
                Some(core.clone())
            } else {
                Some(machine.clone())
            };
            runtime.terminal = Some(address.clone());
            {
                let mut inv = self.inventory.lock().unwrap();
                if inv.core == core {
                    let key = inv.address(&address);
                    inv.records.entry(key.clone()).or_insert_with(|| {
                        let mut seeded = receipt.clone();
                        seeded["address"] = json!(key);
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
        let outcome = json!({
            "state": runtime.state,
            "mode": "terminal",
            "terminal_id": item.id,
            "cwd": cwd,
            "error": runtime.error,
        });
        self.store.lock().unwrap().write(|db| {
            let mut current: TerminalEntry = db.get(&item.id)?;
            current.runtime = item.runtime.clone();
            db.save(&current)?;
            if let Some(key) = p["request_key"].as_str() {
                db.conn.execute(
                    "UPDATE operations SET result=?2 WHERE id=?1",
                    params![key, serde_json::to_string(&outcome)?],
                )?;
            }
            Ok(())
        })?;
        if let Ok(value) = self.terminal_context(json!({"terminal_id": item.id}))
            && let Ok(context) = serde_json::from_value::<TerminalContext>(value)
        {
            let mut contexts = self.terminal_contexts.lock().unwrap();
            contexts.retain(|c| c.terminal_id != item.id);
            contexts.push(context);
        }
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
