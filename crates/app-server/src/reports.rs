use super::*;

impl AppServer {
    pub(super) fn report(&self, p: Value) -> Result<Value> {
        let request_id = p["execution"]["request_id"]
            .as_str()
            .context("execution.request_id is required")?;
        let number = u32::try_from(
            p["execution"]["attempt_number"]
                .as_u64()
                .context("execution.attempt_number is required")?,
        )?;
        ensure!(number > 0, "attempt_number must be positive");
        let state = strp(&p, "state")?;
        ensure!(
            ["working", "completed", "backlog", "blocked", "failed"].contains(&state),
            "invalid_report: unsupported state"
        );
        let key = strp(&p, "request_key")?;
        let fingerprint = canban_context::digest(&serde_json::to_vec(&p)?);
        // Validate association and duplicate key before publishing any report file.
        let (req, attempt, card, project) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let req: Request = db.get(request_id)?;
            let attempt: Attempt = db.get(&Attempt::key(request_id, number))?;
            let card: Card = db.get(&req.card_id)?;
            let project: Project = db.get(&req.project_id)?;
            let project = self.bound_project(&project, &card);
            (req, attempt, card, project)
        };
        if attempt.state == "continued" {
            if let Some(old) = self
                .store
                .lock()
                .unwrap()
                .db()
                .list::<Report>()?
                .into_iter()
                .find(|r| r.request_key == key)
            {
                ensure!(old.fingerprint == fingerprint, "request_key_conflict");
                return Ok(json!({"accepted":true,"duplicate":true}));
            }
            bail!("This assignment was superseded; use the current request and attempt binding");
        }
        // Checkpoint any uncommitted deliverable changes on the Card branch when an Agent
        // finishes its turn without running `git commit` itself (never fails the report).
        if state == "completed" && card.worktree.is_some() {
            let commit_msg = format!("Canban #{}: {}", card.number, card.title);
            if let Ok(Some(_)) = self
                .ops(&project.work_location().machine)
                .commit_card_worktree_if_dirty(&project, &card, &commit_msg)
            {
                self.git_changed(&project.work_location().machine);
            }
        }
        let report_id = id();
        let path = if let Some(body) = p["body"].as_str() {
            let path = canban_context::run_dir(&self.home, &req, number)
                .join("reports")
                .join(format!("{report_id}.md"));
            canban_context::atomic(&path, body.as_bytes())?;
            Some(canban_context::relative(&self.home, &path)?)
        } else {
            None
        };
        let config = self.config();
        self.mutate("execution.report",&p,|db|{
            if let Some(old)=db.list::<Report>()?.into_iter().find(|r|r.request_key==key){ensure!(old.fingerprint==fingerprint,"request_key_conflict");return Ok(json!({"accepted":true,"duplicate":true}));}
            let mut req:Request=db.get(&req.id)?;let mut run:Attempt=db.get(&attempt.id)?;ensure!(run.state!="continued","This assignment was superseded; use the current request and attempt binding");let mut card:Card=db.get(&card.id)?;
            let report=Report{id:report_id,request_id:req.id.clone(),project_id:req.project_id.clone(),card_id:req.card_id.clone(),attempt:number,state:state.into(),message:p["message"].as_str().unwrap_or("").into(),path,request_key:key.into(),fingerprint,created_at:now()};db.save(&report)?;
            let reported_msg = if state == "backlog" {
                if report.message.trim().is_empty() {
                    "Returned to Backlog".to_owned()
                } else {
                    format!("Returned to Backlog: {}", report.message.trim())
                }
            } else {
                format!("{state}: {}", report.message)
            };
            log(db,&card,"execution.reported",&format!("agent:{}",req.profile.id),&reported_msg,Some(req.id.clone()),json!({"report_id":report.id,"path":report.path,"attempt":number,"state":state}))?;
            if matches!(state, "completed" | "backlog") && req.trigger.starts_with("comment:") {
                let reply_msg = if state == "backlog" { &reported_msg } else { &report.message };
                for mut q in db.list::<Question>()?.into_iter().filter(|q|q.request_id==req.id && q.state!="replied") {
                    q.state="replied".into();q.reply_path=report.path.clone();db.save(&q)?;
                    log(db,&card,"comment.replied",&format!("agent:{}",req.profile.id),reply_msg,Some(req.id.clone()),json!({"reply_to":q.comment_id,"question_id":q.id,"path":report.path,"report_id":report.id}))?;
                }
            }
            let current=req.attempt==number;
            let terminal_done = matches!(state, "completed" | "backlog");
            if !matches!(run.state.as_str(), "completed" | "backlog") {
                run.state=state.into();
                if terminal_done {
                    run.close_state=String::new();
                    run.completed_at=Some(now());run.completion_turn=Some(run.observed.as_ref().and_then(|v|v["turn"].as_u64()).unwrap_or(0));run.turn_ended_at=None;
                }
                db.save(&run)?;
            }
            if current && !matches!(req.state.as_str(), "completed" | "backlog") {
                req.state=state.into();db.save(&req)?;
                if state=="blocked"||state=="failed" {
                    attention(db,&mut card,if report.message.is_empty(){state}else{&report.message})?;
                    if let Some(gate)=&mut card.human_gate && gate.request_id.is_none() { gate.request_id=Some(req.id.clone()); }
                }
                if req.workflow_run_id.is_some() {
                    crate::workflow::reported(db,&config,&mut card,&req,&report)?;
                }
                if state == "backlog" {
                    crate::workflow::stop_card(db, &card.id, "returned_to_backlog")?;
                    for mut pending in db.list::<Request>()?.into_iter().filter(|r| {
                        r.card_id == card.id && r.state == "queued" && canban_scheduler::automatic(r)
                    }) {
                        pending.state = "cancelled".into();
                        db.save(&pending)?;
                    }
                    card.attention = None;
                    card.human_gate = None;
                    let backlog_stage = project
                        .workflow
                        .stages
                        .first()
                        .map(|s| s.id.clone())
                        .unwrap_or_else(|| "backlog".into());
                    if card.stage != backlog_stage {
                        let from = std::mem::replace(&mut card.stage, backlog_stage);
                        card.board_order = None;
                        canban_kanban::order::place(db, &mut card, None)?;
                        log(
                            db,
                            &card,
                            "card.stage_changed",
                            &format!("agent:{}", req.profile.id),
                            &format!("{from} → {} (returned by @{})", card.stage, req.profile.id),
                            Some(req.id.clone()),
                            json!({
                                "from": from,
                                "to": card.stage,
                                "reason": "agent_returned_backlog",
                                "request_id": req.id,
                                "workflow_run_id": req.workflow_run_id,
                            }),
                        )?;
                    }
                }
                card.revision+=1;db.save(&card)?;
            }
            Ok(json!({"accepted":true,"duplicate":false,"recorded_state":state,"execution":{"request_id":request_id,"attempt_number":number}}))
        })
    }
}
