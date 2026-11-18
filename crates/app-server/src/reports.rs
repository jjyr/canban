use super::*;

impl AppServer {
    pub(super) fn report(&self, p: Value) -> Result<Value> {
        if p["schedule_run_id"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
            || p["terminal_id"]
                .as_str()
                .is_some_and(|s| s.trim().starts_with("scheduled:"))
        {
            return self.schedule_report(p);
        }
        if p["batch_id"].as_str().is_some_and(|s| !s.trim().is_empty())
            || p["batch"].as_str().is_some_and(|s| !s.trim().is_empty())
            || p["terminal_id"]
                .as_str()
                .is_some_and(|s| s.trim().starts_with("merge-queue:"))
            || (p["card"].is_null()
                && p["card_id"].is_null()
                && p["cwd"].as_str().is_some_and(|cwd| {
                    self.store
                        .lock()
                        .unwrap()
                        .db()
                        .list::<canban_types::MergeBatch>()
                        .is_ok_and(|list| {
                            list.into_iter()
                                .any(|b| b.active() && path_within(&b.worktree, cwd))
                        })
                }))
        {
            return self.batch_report(p);
        }
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
        let (entry_opt, card, project) = {
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
            if let Some(tid) = p["terminal_id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let entry: TerminalEntry = db.get(tid)?;
                if let Some(att) = &entry.agent {
                    ensure!(
                        !matches!(att.state.as_str(), "continued" | "cancelled" | "superseded"),
                        "Execution invocation is no longer active"
                    );
                    let card: Card = db.get(&att.card_id)?;
                    let project: Project = db.get(&att.project_id)?;
                    let project = self.bound_project(&project, &card);
                    (Some(entry), card, project)
                } else {
                    let card = super::resolve_card(db, &p)?;
                    let project: Project = db.get(&card.project_id)?;
                    let project = self.bound_project(&project, &card);
                    (None, card, project)
                }
            } else {
                let card = super::resolve_card(db, &p)?;
                let project: Project = db.get(&card.project_id)?;
                let project = self.bound_project(&project, &card);
                let wf_terminal_id = db
                    .list::<WorkflowRun>()?
                    .into_iter()
                    .find(|r| r.card_id == card.id && r.active())
                    .and_then(|r| r.terminal_id);
                let terminals = db.list::<TerminalEntry>()?;
                let matched = wf_terminal_id
                    .as_deref()
                    .and_then(|tid| {
                        terminals.iter().find(|t| {
                            t.id == tid
                                && t.agent.as_ref().is_some_and(|a| {
                                    !matches!(a.state.as_str(), "cancelled" | "superseded")
                                        && requested_profile
                                            .as_deref()
                                            .is_none_or(|pid| a.profile.id == pid)
                                })
                        })
                    })
                    .or_else(|| {
                        terminals.iter().rev().find(|t| {
                            t.agent.as_ref().is_some_and(|a| {
                                a.card_id == card.id
                                    && !a.finished()
                                    && requested_profile
                                        .as_deref()
                                        .is_none_or(|pid| a.profile.id == pid)
                            })
                        })
                    })
                    .or_else(|| {
                        terminals.iter().rev().find(|t| {
                            t.agent.as_ref().is_some_and(|a| {
                                a.card_id == card.id
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
            let path = if let Some(entry) = &entry_opt {
                if let Some(run) = &entry.agent {
                    canban_context::card_dir(&self.home, &card.project_id, card.number)
                        .join("agents")
                        .join(&run.profile.id)
                        .join("reports")
                        .join(format!("{report_id}.md"))
                } else {
                    canban_context::run_dir(&self.home, entry)
                        .join("reports")
                        .join(format!("{report_id}.md"))
                }
            } else {
                canban_context::card_dir(&self.home, &card.project_id, card.number)
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
            let mut entry_opt = match &entry_opt {
                Some(entry) => {
                    let current: TerminalEntry = db.get(&entry.id)?;
                    if p["terminal_id"]
                        .as_str()
                        .is_some_and(|s| !s.trim().is_empty())
                        && let Some(att) = &current.agent
                    {
                        ensure!(
                            !matches!(att.state.as_str(), "continued" | "cancelled" | "superseded"),
                            "Execution invocation is no longer active"
                        );
                    }
                    Some(current)
                }
                None => None,
            };
            let is_non_workflow_task = entry_opt
                .as_ref()
                .and_then(|t| t.agent.as_ref())
                .is_some_and(|r| {
                    r.workflow_run_id.is_none()
                        && (r.trigger.is_comment()
                            || r.trigger == ExecutionTrigger::Custom("diff_review".into()))
                });
            let active_wf_id = entry_opt
                .as_ref()
                .and_then(|t| t.agent.as_ref())
                .and_then(|r| r.workflow_run_id.clone())
                .or_else(|| {
                    if is_non_workflow_task {
                        return None;
                    }
                    db.list::<WorkflowRun>()
                        .ok()?
                        .into_iter()
                        .find(|r| r.card_id == card.id && r.active())
                        .map(|r| r.id)
                });
            let profile_label = entry_opt
                .as_ref()
                .and_then(|t| t.agent.as_ref())
                .map(|r| r.profile.id.clone())
                .or_else(|| requested_profile.clone())
                .unwrap_or_else(|| config.default_agent.clone());
            let actor = format!("agent:{profile_label}");
            let terminal_id = entry_opt.as_ref().map(|t| t.id.clone());
            let run_number = entry_opt
                .as_ref()
                .and_then(|t| t.agent.as_ref())
                .map(|r| r.number)
                .unwrap_or(0);
            let report = Report {
                id: report_id,
                terminal_id: terminal_id.clone(),
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
            let active_wf = active_wf_id
                .as_deref()
                .and_then(|id| db.get::<WorkflowRun>(id).ok())
                .filter(|r| r.active());
            let wf_awaiting_report = active_wf.as_ref().is_some_and(|r| {
                matches!(r.state.as_str(), "waiting_agent" | "blocked")
                    && r.terminal_id
                        .as_deref()
                        .is_none_or(|tid| Some(tid) == terminal_id.as_deref())
            });
            let is_comment = entry_opt
                .as_ref()
                .and_then(|t| t.agent.as_ref())
                .is_some_and(|r| r.trigger.is_comment() && r.workflow_run_id.is_none());
            let comment_questions = if is_comment {
                db.list::<Question>()?
                    .into_iter()
                    .filter(|q| q.terminal_id == terminal_id)
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
                terminal_id.clone(),
                json!({
                    "report_id": report.id,
                    "path": report.path,
                    "run_number": run_number,
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
                        terminal_id.clone(),
                        json!({"reply_to":q.comment_id,"question_id":q.id,"path":report.path,"report_id":report.id}),
                    )?;
                }
            }
            let terminal_done = matches!(state, "completed" | "backlog");
            let mut should_advance =
                entry_opt.as_ref().and_then(|t| t.agent.as_ref()).is_none() || wf_awaiting_report;
            if let Some(ref mut entry) = entry_opt
                && let Some(ref mut run) = entry.agent
                && (!matches!(run.state.as_str(), "completed" | "backlog") || wf_awaiting_report)
            {
                let report_at = now();
                run.state = state.into();
                run.updated_at = report_at;
                if state == "working" && run.lifecycle.idle_since.is_some() {
                    run.lifecycle.idle_since = Some(report_at);
                }
                if terminal_done {
                    run.close_state = String::new();
                    run.completed_at = Some(report_at);
                    run.completion_turn = Some(
                        run.observed
                            .as_ref()
                            .and_then(|v| v["turn"].as_u64())
                            .unwrap_or(0),
                    );
                    run.turn_ended_at = None;
                }
                db.save(entry)?;
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
                        && gate.terminal_id.is_none()
                    {
                        gate.terminal_id = terminal_id.clone();
                    }
                }
                if active_wf_id.is_some()
                    || entry_opt
                        .as_ref()
                        .and_then(|t| t.agent.as_ref())
                        .is_some_and(|r| {
                            r.workflow_run_id.is_some()
                                || r.on_completed.is_some()
                                || r.require_human_review
                        })
                {
                    crate::workflow::reported(
                        db,
                        &config,
                        &mut card,
                        entry_opt.as_ref(),
                        &report,
                    )?;
                }
                let mut returned_from = None;
                if state == "backlog" {
                    crate::workflow::stop_card(db, &config, &card.id, "returned_to_backlog")?;
                    for mut pending in db.list::<TerminalEntry>()? {
                        if let Some(a) = &mut pending.agent
                            && a.card_id == card.id
                        {
                            let mut changed = false;
                            if a.state == "queued" && canban_scheduler::automatic(a) {
                                a.state = "cancelled".into();
                                changed = true;
                            }
                            if a.pending
                                .as_ref()
                                .is_some_and(|q| q.trigger.is_automatic())
                            {
                                a.pending = None;
                                changed = true;
                            }
                            if changed {
                                db.save(&pending)?;
                            }
                        }
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
                        returned_from = Some(from);
                    }
                }
                card.revision += 1;
                db.save(&card)?;
                if let Some(from) = returned_from {
                    log(
                        db,
                        &card,
                        "card.stage_changed",
                        &actor,
                        &format!("{from} → {} (returned by @{profile_label})", card.stage),
                        terminal_id.clone(),
                        json!({
                            "from": from,
                            "to": card.stage,
                            "reason": "agent_returned_backlog",
                            "terminal_id": terminal_id,
                            "workflow_run_id": active_wf_id,
                        }),
                    )?;
                }
            }
            Ok(json!({
                "accepted": true,
                "duplicate": false,
                "recorded_state": state,
                "card_id": card.id,
                "card": card.number,
                "terminal_id": terminal_id,
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
