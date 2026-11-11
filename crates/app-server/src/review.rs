use super::*;
pub(super) fn blocker(db: Db<'_>, card: &Card) -> Result<Option<String>> {
    if db.list::<Attempt>()?.iter().any(|a| {
        // Continued assignments retain their last observation, but runtime
        // ownership has moved to the next assignment in the conversation.
        a.card_id == card.id && a.terminal.is_some() && !a.finished()
    }) {
        return Ok(Some(
            "End active Card Agent terminals before accepting code".into(),
        ));
    }
    if db.list::<Request>()?.iter().any(|r| {
        r.card_id == card.id && matches!(r.state.as_str(), "queued" | "starting" | "unknown")
    }) {
        return Ok(Some(
            "Card has queued or uncertain work; resolve it before accepting code".into(),
        ));
    }
    Ok(None)
}
impl AppServer {
    pub(super) fn review_preview(&self, p: Value) -> Result<Value> {
        let (card, project, reason) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let c: Card = db.get(strp(&p, "card_id")?)?;
            let project: Project = db.get(&c.project_id)?;
            let project = self.bound_project(&project, &c);
            let reason = blocker(db, &c)?;
            (c, project, reason)
        };
        let mut plan = self.ops(&project.work_location().machine).review_preview(
            &project,
            &card,
            p["target_branch"].as_str(),
        )?;
        if reason.is_some() {
            plan.merge_allowed = false;
            plan.reason = reason;
        }
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
        // A stored intent must win before any Git effects. Duplicate/unknown
        // delivery requests are never blindly replayed after a lost reply.
        let mut execute = false;
        let mut context = None;
        let saved=self.mutate("review.merge",&p,|db|{
            let card:Card=db.get(strp(&p,"card_id")?)?;revise(&card,&p)?;
            ensure!(card.active(),"Restore the Card before accepting");
            ensure!(card.pull_request.as_ref().is_none_or(|pr|pr.merged),"Resolve the existing PR delivery before local merge");
            if let Some(reason)=blocker(db,&card)? {bail!("{reason}");}
            let project:Project=db.get(&card.project_id)?;let project=self.bound_project(&project,&card);
            let plan=self.ops(&project.work_location().machine).review_preview(&project,&card,p["target_branch"].as_str())?;
            ensure!(plan.token==strp(&p,"token")?,"Review is stale; refresh the diff before accepting");
            ensure!(plan.merge_allowed,"{}",plan.reason.as_deref().unwrap_or("Merge unavailable"));
            log(db,&card,"review.merge_started","user","Accepted reviewed changes for commit/merge",None,json!({"request_key":key,"target_branch":plan.target_branch,"target_head":plan.target_head,"card_head":plan.card_head,"token":plan.token}))?;
            execute=true;context=Some((card,project,plan));
            Ok(json!({"state":"unknown","request_key":key,"message":"Merge intent saved; inspect Git heads before repeating an uncertain delivery"}))
        })?;
        if !execute {
            return Ok(saved);
        }
        let (card, project, plan) = context.unwrap();
        let message = p["message"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Canban #{}: {}", card.number, card.title));
        let result = self.ops(&project.work_location().machine).review_merge(
            &project,
            &card,
            Some(&plan.target_branch),
            &plan.token,
            &message,
        );
        let value=self.store.lock().unwrap().write(|db|{
            let mut current:Card=db.get(&card.id)?;
            let value=match result {
                Ok(commit)=>{
                    current.delivery_mode=Some(DeliveryMode::Local);
                    current.delivery=Some(Delivery{target_branch:plan.target_branch.clone(),commit:commit.clone(),request_key:key.clone(),merged_at:now()});
                    current.attention=None;
                    if let Some(gate)=current.human_gate.take() && let Some(request)=gate.request_id {
                        let request:Request=db.get(&request)?;
                        if request.state=="completed" { crate::workflow::advance(db,&self.config(),&mut current,&request)?; }
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
        drop(_gate);
        if value["state"] == "merged" {
            let _ = self.try_auto_archive_merged(&card.id);
        }
        self.emit("review", Some(card.id));
        Ok(value)
    }
    pub(crate) fn try_auto_archive_merged(&self, card_id: &str) -> Result<bool> {
        let (card, project) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let Ok(card) = db.get::<Card>(card_id) else {
                return Ok(false);
            };
            if !card.active()
                || (card.delivery.is_none()
                    && !card.pull_request.as_ref().is_some_and(|p| p.merged))
                || card.attention.is_some()
                || card.human_gate.is_some()
                || blocker(db, &card)?.is_some()
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
                .is_some_and(|e| matches!(e.kind.as_str(), "card.restored" | "card.archived"))
            {
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

    pub(crate) fn pump_deliveries(&self) -> Result<()> {
        let pending: Vec<WorkflowRun> = self
            .store
            .lock()
            .unwrap()
            .db()
            .list::<WorkflowRun>()?
            .into_iter()
            .filter(|r| r.state == "waiting_delivery")
            .collect();
        for run in pending {
            let _ = self.execute_workflow_delivery(&run.id);
        }
        Ok(())
    }

    fn execute_workflow_delivery(&self, run_id: &str) -> Result<()> {
        let Ok(_gate) = self.review_gate.try_lock() else {
            return Ok(());
        };
        let (card, project, run, blocker_reason) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let Ok(run) = db.get::<WorkflowRun>(run_id) else {
                return Ok(());
            };
            if run.state != "waiting_delivery" {
                return Ok(());
            }
            let card: Card = db.get(&run.card_id)?;
            if !card.active() {
                return Ok(());
            }
            let project: Project = db.get(&card.project_id)?;
            let project = self.bound_project(&project, &card);
            let blocker_reason = blocker(db, &card)?;
            (card, project, run, blocker_reason)
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
            return Ok(());
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
            return Ok(());
        }
        if blocker_reason.is_some() {
            // An Agent terminal may still be closing right after reporting; retry on the next tick.
            return Ok(());
        }
        let target_branch = run.inputs["target_branch"].as_str();
        let plan = match self
            .ops(&project.work_location().machine)
            .review_preview(&project, &card, target_branch)
        {
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
                return Ok(());
            }
        };
        if !plan.merge_allowed {
            let reason = format!(
                "Auto-merge paused: {}",
                plan.reason
                    .as_deref()
                    .unwrap_or("Merge unavailable; human review required")
            );
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
            return Ok(());
        }
        let key = format!("workflow-deliver:{}:{}", run.id, run.visit);
        let message = format!("Canban #{}: {}", card.number, card.title);
        let result = self.ops(&project.work_location().machine).review_merge(
            &project,
            &card,
            Some(&plan.target_branch),
            &plan.token,
            &message,
        );
        let mut merged = false;
        self.store.lock().unwrap().write(|db| {
            let mut current_card: Card = db.get(&card.id)?;
            let mut current_run: WorkflowRun = db.get(&run.id)?;
            if current_run.state != "waiting_delivery" {
                return Ok(());
            }
            match result {
                Ok(commit) => {
                    merged = true;
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
                    log(
                        db,
                        &current_card,
                        "review.merged",
                        "system",
                        &format!("Auto-merged into {}", plan.target_branch),
                        None,
                        json!({"commit":commit,"target_branch":plan.target_branch,"request_key":key,"workflow_run_id":run.id}),
                    )?;
                    canban_workflow_engine::runtime::complete_delivery(
                        db,
                        &config,
                        &mut current_card,
                        &mut current_run,
                        json!({"state":"merged","commit":commit,"target_branch":plan.target_branch}),
                        enqueue,
                    )?;
                }
                Err(error) => {
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
        drop(_gate);
        if merged {
            let _ = self.try_auto_archive_merged(&card.id);
        }
        self.emit("review", Some(card.id));
        Ok(())
    }
}
