use super::*;

impl AppServer {
    pub(super) fn report(&self, p: Value) -> Result<Value> {
        let state = strp(&p, "state")?;
        ensure!(
            ["working", "completed", "backlog", "blocked", "failed"].contains(&state),
            "invalid_report: unsupported state"
        );
        let key = strp(&p, "request_key")?;
        let fingerprint = canban_context::digest(&serde_json::to_vec(&p)?);
        let requested_profile = p["profile_id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        // Validate association and duplicate key before publishing any report file.
        let (attempt_opt, card, project) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            if let Some(old) = db
                .list::<Report>()?
                .into_iter()
                .find(|r| r.request_key == key)
            {
                ensure!(old.fingerprint == fingerprint, "request_key_conflict");
                return Ok(json!({"accepted":true,"duplicate":true}));
            }
            if let Some(aid) = p["attempt_id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let att: Attempt = db.get(aid)?;
                ensure!(
                    att.state != "continued",
                    "This assignment was superseded; report against the current Card"
                );
                let card: Card = db.get(&att.card_id)?;
                let project: Project = db.get(&att.project_id)?;
                let project = self.bound_project(&project, &card);
                (Some(att), card, project)
            } else {
                let card = super::resolve_card(db, &p)?;
                let project: Project = db.get(&card.project_id)?;
                let project = self.bound_project(&project, &card);
                let wf_attempt_id = db
                    .list::<WorkflowRun>()?
                    .into_iter()
                    .find(|r| r.card_id == card.id && r.active())
                    .and_then(|r| r.attempt_id);
                let attempts = db.list::<Attempt>()?;
                let matched = wf_attempt_id
                    .as_deref()
                    .and_then(|aid| {
                        attempts.iter().find(|a| {
                            a.id == aid
                                && a.state != "continued"
                                && !matches!(
                                    a.state.as_str(),
                                    "completed" | "backlog" | "cancelled" | "superseded"
                                )
                                && requested_profile
                                    .as_deref()
                                    .is_none_or(|pid| a.profile.id == pid)
                        })
                    })
                    .or_else(|| {
                        attempts.iter().rev().find(|a| {
                            a.card_id == card.id
                                && a.state != "continued"
                                && !a.finished()
                                && requested_profile
                                    .as_deref()
                                    .is_none_or(|pid| a.profile.id == pid)
                        })
                    })
                    .or_else(|| {
                        attempts.iter().rev().find(|a| {
                            a.card_id == card.id
                                && a.state != "continued"
                                && !matches!(
                                    a.state.as_str(),
                                    "completed"
                                        | "backlog"
                                        | "cancelled"
                                        | "superseded"
                                        | "idle_closed"
                                )
                                && requested_profile
                                    .as_deref()
                                    .is_none_or(|pid| a.profile.id == pid)
                        })
                    })
                    .cloned();
                (matched, card, project)
            }
        };
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
            let path = if let Some(att) = &attempt_opt {
                canban_context::run_dir(&self.home, att)
                    .join("reports")
                    .join(format!("{report_id}.md"))
            } else {
                canban_context::card_dir(&self.home, &card.project_id, &card.id)
                    .join("reports")
                    .join(format!("{report_id}.md"))
            };
            canban_context::atomic(&path, body.as_bytes())?;
            Some(canban_context::relative(&self.home, &path)?)
        } else {
            None
        };
        let config = self.config();
        let mut emit_card = None;
        let res = self.mutate("execution.report", &p, |db| {
            if let Some(old) = db
                .list::<Report>()?
                .into_iter()
                .find(|r| r.request_key == key)
            {
                ensure!(old.fingerprint == fingerprint, "request_key_conflict");
                return Ok(json!({"accepted":true,"duplicate":true}));
            }
            let mut card: Card = db.get(&card.id)?;
            emit_card = Some(card.id.clone());
            let mut run_opt = match &attempt_opt {
                Some(att) => {
                    let run: Attempt = db.get(&att.id)?;
                    ensure!(
                        run.state != "continued",
                        "This assignment was superseded; report against the current Card"
                    );
                    Some(run)
                }
                None => None,
            };
            let active_wf_id = run_opt
                .as_ref()
                .and_then(|r| r.workflow_run_id.clone())
                .or_else(|| {
                    db.list::<WorkflowRun>()
                        .ok()?
                        .into_iter()
                        .find(|r| r.card_id == card.id && r.active())
                        .map(|r| r.id)
                });
            let profile_label = run_opt
                .as_ref()
                .map(|r| r.profile.id.clone())
                .or_else(|| requested_profile.clone())
                .unwrap_or_else(|| config.default_agent.clone());
            let actor = format!("agent:{profile_label}");
            let attempt_id = run_opt.as_ref().map(|r| r.id.clone());
            let attempt_num = run_opt.as_ref().map(|r| r.number).unwrap_or(0);
            let report = Report {
                id: report_id,
                attempt_id: attempt_id.clone(),
                workflow_run_id: active_wf_id.clone(),
                project_id: card.project_id.clone(),
                card_id: card.id.clone(),
                actor: actor.clone(),
                state: state.into(),
                message: p["message"].as_str().unwrap_or("").into(),
                path,
                request_key: key.into(),
                fingerprint,
                created_at: now(),
            };
            db.save(&report)?;
            let reported_msg = if state == "backlog" {
                if report.message.trim().is_empty() {
                    "Returned to Backlog".to_owned()
                } else {
                    format!("Returned to Backlog: {}", report.message.trim())
                }
            } else {
                format!("{state}: {}", report.message)
            };
            let is_comment = run_opt
                .as_ref()
                .is_some_and(|r| r.trigger.is_comment());
            let comment_questions = if is_comment {
                db.list::<Question>()?
                    .into_iter()
                    .filter(|q| q.attempt_id == attempt_id)
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let comment_already_replied = state == "completed"
                && !comment_questions.is_empty()
                && comment_questions.iter().all(|q| q.state == "replied");
            log(
                db,
                &card,
                "execution.reported",
                &actor,
                &reported_msg,
                attempt_id.clone(),
                json!({
                    "report_id": report.id,
                    "path": report.path,
                    "attempt": attempt_num,
                    "state": state,
                    "comment_replied": comment_already_replied,
                }),
            )?;
            if matches!(state, "completed" | "backlog") && is_comment {
                let reply_msg = if state == "backlog" {
                    &reported_msg
                } else {
                    &report.message
                };
                for mut q in comment_questions
                    .into_iter()
                    .filter(|q| q.state != "replied")
                {
                    q.state = "replied".into();
                    q.reply_path = report.path.clone();
                    db.save(&q)?;
                    log(
                        db,
                        &card,
                        "comment.replied",
                        &actor,
                        reply_msg,
                        attempt_id.clone(),
                        json!({"reply_to":q.comment_id,"question_id":q.id,"path":report.path,"report_id":report.id}),
                    )?;
                }
            }
            let terminal_done = matches!(state, "completed" | "backlog");
            let mut should_advance = run_opt.is_none();
            if let Some(ref mut run) = run_opt
                && !matches!(run.state.as_str(), "completed" | "backlog")
            {
                run.state = state.into();
                if terminal_done {
                    run.close_state = String::new();
                    run.completed_at = Some(now());
                    run.completion_turn = Some(
                        run.observed
                            .as_ref()
                            .and_then(|v| v["turn"].as_u64())
                            .unwrap_or(0),
                    );
                    run.turn_ended_at = None;
                }
                db.save(run)?;
                should_advance = true;
            }
            if should_advance {
                if state == "blocked" || state == "failed" {
                    attention(
                        db,
                        &mut card,
                        if report.message.is_empty() {
                            state
                        } else {
                            &report.message
                        },
                    )?;
                    if let Some(gate) = &mut card.human_gate
                        && gate.attempt_id.is_none()
                    {
                        gate.attempt_id = attempt_id.clone();
                    }
                }
                if active_wf_id.is_some()
                    || run_opt.as_ref().is_some_and(|r| {
                        r.workflow_run_id.is_some()
                            || r.on_completed.is_some()
                            || r.require_human_review
                    })
                {
                    crate::workflow::reported(
                        db,
                        &config,
                        &mut card,
                        run_opt.as_ref(),
                        &report,
                    )?;
                }
                if state == "backlog" {
                    crate::workflow::stop_card(db, &card.id, "returned_to_backlog")?;
                    for mut pending in db.list::<Attempt>()?.into_iter().filter(|a| {
                        a.card_id == card.id && a.state == "queued" && canban_scheduler::automatic(a)
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
                            &actor,
                            &format!("{from} → {} (returned by @{profile_label})", card.stage),
                            attempt_id.clone(),
                            json!({
                                "from": from,
                                "to": card.stage,
                                "reason": "agent_returned_backlog",
                                "attempt_id": attempt_id,
                                "workflow_run_id": active_wf_id,
                            }),
                        )?;
                    }
                }
                card.revision += 1;
                db.save(&card)?;
            }
            Ok(json!({
                "accepted": true,
                "duplicate": false,
                "recorded_state": state,
                "card_id": card.id,
                "card": card.number,
                "attempt_id": attempt_id,
            }))
        })?;
        if p["card_id"].is_null()
            && let Some(id) = emit_card
        {
            self.emit("application", Some(id));
        }
        Ok(res)
    }
}
