use super::*;

pub(super) fn conversation(
    db: Db<'_>,
    card: &Card,
    profile: &Profile,
    now_ms: u64,
) -> Result<Conversation> {
    let key = Conversation::key(&card.id, &profile.id);
    if let Ok(old) = db.get::<Conversation>(&key) {
        return Ok(old);
    }
    let item = Conversation {
        id: key,
        project_id: card.project_id.clone(),
        card_id: card.id.clone(),
        profile_id: profile.id.clone(),
        provider: profile.command_name(),
        provider_session_id: None,
        created_at: now_ms,
    };
    db.save(&item)?;
    db.agent_terminal(&item)?;
    Ok(item)
}

pub(super) fn enqueue_interactive(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    project: &Project,
    profile_id: &str,
    now_ms: u64,
) -> Result<TerminalEntry> {
    let stage = canban_workflow_engine::stage(&project.workflow, &card.stage)?;
    let init_prompt = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../prompts/runtime/interactive_init.txt"
    ))
    .trim_end()
    .to_owned();
    enqueue(
        db,
        config,
        card,
        project,
        stage,
        ExecutionTrigger::Interactive,
        Some(profile_id),
        Some(init_prompt),
        now_ms,
    )?
    .context("Agent was not queued")
}

pub(super) const CARD_MENTION_TASK_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../prompts/runtime/card_mention_task.txt"
));

pub(super) fn render_card_mention_instruction(
    profile_id: &str,
    message_body: Option<&str>,
) -> String {
    let (target, message_block) = match message_body.map(str::trim).filter(|s| !s.is_empty()) {
        Some(body) => ("the user's message below", format!("\n\n{body}")),
        None => (
            "the user's message in the Card title and description",
            String::new(),
        ),
    };
    canban_context::render_template(
        CARD_MENTION_TASK_TEMPLATE.trim_end(),
        &[
            ("profile_id", profile_id),
            ("target", target),
            ("message_block", &message_block),
        ],
    )
}

/// Resolve and validate an optional `assignee` RPC field (`null`/empty clears, string selects an enabled profile).
pub(super) fn parse_assignee_param(
    config: &Config,
    project: Option<&Project>,
    p: &Value,
) -> Result<Option<Option<String>>> {
    let Some(raw) = p.get("assignee") else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(Some(None));
    }
    let text = raw
        .as_str()
        .context("assignee must be a profile ID string or null")?;
    let trimmed = text.trim().trim_start_matches('@');
    if trimmed.is_empty() {
        return Ok(Some(None));
    }
    let profile = canban_scheduler::select_in_project(config, project, Some(trimmed))?;
    Ok(Some(Some(profile.id)))
}

pub(super) fn queue_card_mentions(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    project: &Project,
    p: &Value,
    prose_sources: &[&str],
    now_ms: u64,
) -> Result<()> {
    let mut mentions = strings(p, "mentions");
    // Explicit recipients distinguish addressing an Agent from discussing its name.
    // When `mentions` is omitted, explicit `@profile` mentions in prose take precedence
    // and only fall back to `card.assignee` when no Agent was explicitly addressed.
    if p.get("mentions").is_none() {
        for source in prose_sources {
            for name in canban_profiles::addressed(source, &config.profiles) {
                if !mentions.contains(&name) {
                    mentions.push(name);
                }
            }
        }
        if mentions.is_empty()
            && let Some(assignee) = card.assignee.as_deref()
            && config
                .profiles
                .iter()
                .any(|a| a.id == assignee && a.enabled)
        {
            mentions.push(assignee.to_owned());
        }
    }
    mentions.sort();
    mentions.dedup();
    if mentions.is_empty() {
        return Ok(());
    }
    let stage = canban_workflow_engine::stage(&project.workflow, &card.stage)?;
    let anchor_event_id: String = db
        .conn
        .query_row(
            "SELECT id FROM events WHERE project_id=?1 AND card_id=?2 ORDER BY number DESC LIMIT 1",
            params![&card.project_id, &card.id],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| format!("card:{}", card.id));
    let question_body = if card.body.trim().is_empty() {
        card.title.clone()
    } else {
        format!("{}\n\n{}", card.title, card.body.trim())
    };
    let created_at = now_ms;
    for agent in mentions {
        let qid = format!("{anchor_event_id}:{agent}");
        if db.get::<Question>(&qid).is_ok() {
            continue;
        }
        let profile = canban_scheduler::select_in_project(config, Some(project), Some(&agent))?;
        let context = conversation(db, card, &profile, now_ms)?;
        let active = db.get::<TerminalEntry>(&context.id).ok().filter(|t| {
            t.agent.as_ref().is_some_and(|a| {
                a.card_id == card.id
                    && a.profile.id == profile.id
                    && (!(card.human_gate.is_some()
                        && a.state == "queued"
                        && canban_scheduler::automatic(a))
                        && matches!(
                            a.state.as_str(),
                            "queued" | "starting" | "running" | "working" | "blocked"
                        )
                        || matches!(a.state.as_str(), "completed" | "backlog")
                            && a.profile == profile
                            && a.terminal.is_some()
                            && !a.finished())
            })
        });
        let entry = if let Some(entry) = active {
            entry
        } else {
            let instruction = render_card_mention_instruction(&profile.id, None);
            enqueue(
                db,
                config,
                card,
                project,
                stage,
                ExecutionTrigger::Comment {
                    comment_id: anchor_event_id.clone(),
                },
                Some(&profile.id),
                Some(instruction),
                now_ms,
            )?
            .context("Card mention was not queued")?
        };
        let question = Question {
            id: qid,
            project_id: card.project_id.clone(),
            card_id: card.id.clone(),
            profile_id: profile.id,
            comment_id: anchor_event_id.clone(),
            body: question_body.clone(),
            terminal_id: Some(entry.id),
            state: "pending".into(),
            error: None,
            reply_path: None,
            created_at,
        };
        db.save(&question)?;
    }
    Ok(())
}

pub(super) fn retry_agent(
    db: Db<'_>,
    card: &Card,
    entry: &mut TerminalEntry,
    now_ms: u64,
) -> Result<()> {
    let previous = entry
        .agent
        .clone()
        .context("Terminal has no Agent execution to retry")?;
    let number = u32::try_from(db.counter(&format!("card-attempt:{}", card.id))?)?;
    let mut next = canban_scheduler::agent_run_at(
        &previous.project_id,
        &card.id,
        number,
        &previous.stage,
        previous.trigger.clone(),
        previous.profile.clone(),
        previous.instruction.clone(),
        previous.on_completed.clone(),
        now_ms,
    );
    next.workflow_run_id = previous.workflow_run_id.clone();
    next.workflow_step = previous.workflow_step.clone();
    next.card_number = card.number;
    next.require_human_review = previous.require_human_review;
    next.resume_session = previous.provider_session_id.is_some() || previous.resume_session;
    if next.resume_session {
        next.workspace = previous.workspace.or_else(|| {
            previous.directory.map(|directory| WorkspaceBinding {
                directory,
                path: previous.cwd.clone(),
                branch: None,
                base_commit: None,
                target_branch: None,
            })
        });
        next.cwd = previous.cwd;
        next.provider_session_id = previous.provider_session_id;
    }
    entry.agent = Some(next);
    db.save(entry)?;
    if let Some(run_id) = entry.agent.as_ref().and_then(|a| a.workflow_run_id.clone())
        && let Ok(mut wf_run) = db.get::<WorkflowRun>(&run_id)
        && wf_run.active()
        && wf_run.terminal_id.as_deref() == Some(&entry.id)
    {
        wf_run.terminal_id = Some(entry.id.clone());
        db.save(&wf_run)?;
    }
    Ok(())
}

impl AppServer {
    pub(super) fn bind_session(&self, p: Value) -> Result<Value> {
        if p["schedule_run_id"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
            || p["terminal_id"]
                .as_str()
                .is_some_and(|s| s.trim().starts_with("scheduled:"))
        {
            return self.schedule_bind(p);
        }
        if p["batch_id"].as_str().is_some_and(|s| !s.trim().is_empty())
            || p["terminal_id"]
                .as_str()
                .is_some_and(|s| s.trim().starts_with("merge-queue:"))
        {
            return self.batch_bind(p);
        }
        let config = self.config();
        self.mutate("agent.bind", &p, |db| {
            let session = strp(&p, "session_id")?;
            ensure!(
                !session.is_empty() && session.len() < 512,
                "Invalid provider session ID"
            );
            let terminals = db.list::<TerminalEntry>()?;
            let (card, profile, mut entry_opt) = if let Some(tid) = p["terminal_id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let entry: TerminalEntry = db.get(tid)?;
                let run = entry
                    .agent
                    .as_ref()
                    .context("Terminal has no Card Agent run")?;
                let card: Card = db.get(&run.card_id)?;
                let prof = run.profile.clone();
                (card, prof, Some(entry))
            } else {
                let card = super::resolve_card(db, &p)?;
                let project: Project = db.get(&card.project_id)?;
                let requested_prof = p["profile_id"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let active_entry = terminals
                    .iter()
                    .rev()
                    .find(|t| {
                        t.agent.as_ref().is_some_and(|a| {
                            a.card_id == card.id
                                && !a.finished()
                                && requested_prof.is_none_or(|pid| a.profile.id == pid)
                        })
                    })
                    .cloned();
                let prof = if let Some(entry) = &active_entry {
                    entry.agent.as_ref().unwrap().profile.clone()
                } else {
                    canban_scheduler::select_in_project(&config, Some(&project), requested_prof)?
                };
                (card, prof, active_entry)
            };
            let mut context = conversation(db, &card, &profile, self.now_ms())?;
            if let Some(old) = &context.provider_session_id {
                ensure!(
                    old == session,
                    "Provider conversation changed unexpectedly; inspect the selected terminal"
                );
            }
            ensure!(
                !db.list::<Conversation>()?.iter().any(|c| c.id != context.id
                    && c.provider == context.provider
                    && c.provider_session_id.as_deref() == Some(session)
                    && terminals
                        .iter()
                        .any(|t| t.conversation_id.as_deref() == Some(&c.id)
                            && t.agent.as_ref().is_some_and(|a| {
                                a.provider_session_id.as_deref() == Some(session)
                            }))),
                "Provider session is already bound to another Card/profile"
            );
            context.provider_session_id = Some(session.into());
            db.save(&context)?;
            if let Some(ref mut entry) = entry_opt
                && let Some(ref mut run) = entry.agent
            {
                run.provider_session_id = Some(session.into());
                db.save(entry)?;
            } else if let Ok(mut entry) = db.get::<TerminalEntry>(&context.id)
                && let Some(ref mut run) = entry.agent
            {
                run.provider_session_id = Some(session.into());
                db.save(&entry)?;
            }
            Ok(json!({"bound":true,"conversation_id":context.id,"session_id":session}))
        })
    }
    pub(super) fn open_agent(&self, p: Value) -> Result<Value> {
        let config = self.config();
        if let Some(sz) = Self::parse_terminal_size(&p["size"]) {
            self.record_terminal_size(sz, true);
        }
        self.mutate("agent.open", &p, |db| {
            let mut card = super::resolve_card(db, &p)?;
            ensure!(
                card.archived_at.is_none(),
                "Restore this Card before opening its Agent"
            );
            let project: Project = db.get(&card.project_id)?;
            let requested = p["profile_id"]
                .as_str()
                .map(str::trim)
                .map(|s| s.trim_start_matches('@'))
                .filter(|s| !s.is_empty());
            let target_profile = requested
                .or(card.assignee.as_deref())
                .context("profile_id is required when Card has no assignee")?;
            let profile =
                canban_scheduler::select_in_project(&config, Some(&project), Some(target_profile))?;
            let mut context = conversation(db, &card, &profile, self.now_ms())?;
            if let Some(expected) = p["expected_terminal_id"].as_str() {
                let entry: TerminalEntry = db
                    .get(&context.id)
                    .context("Conversation no longer exists")?;
                let run = entry.agent.context("Conversation has no invocation")?;
                ensure!(
                    entry.id == expected && run.finished(),
                    "Overview invocation changed; select it again"
                );
                ensure!(
                    context.provider_session_id.is_some()
                        && context.provider_session_id == run.provider_session_id,
                    "Exact provider session is unavailable"
                );
                ensure!(
                    !profile.resume_command().is_empty(),
                    "Profile has no resume command"
                );
            }
            if let Ok(existing) = db.get::<TerminalEntry>(&context.id)
                && let Some(att) = &existing.agent
            {
                let live = att.terminal.is_some()
                    && att
                        .observed
                        .as_ref()
                        .is_none_or(|v| v["state"] != "finished");
                if (matches!(att.state.as_str(), "queued" | "starting" | "unknown")
                    && !(card.human_gate.is_some()
                        && att.state == "queued"
                        && canban_scheduler::automatic(att)))
                    || live
                {
                    return Ok(
                        json!({"terminal_id":existing.id,"reused":true,"terminal":existing}),
                    );
                }
            }
            if p["fresh"] == true || context.provider != profile.command_name() {
                context.provider_session_id = None;
                context.provider = profile.command_name();
                db.save(&context)?;
            }
            let entry =
                enqueue_interactive(db, &config, &mut card, &project, &profile.id, self.now_ms())?;
            Ok(json!({"terminal_id":entry.id,"reused":false,"terminal":entry}))
        })
    }
    pub fn continue_assignment(&self, entry: &TerminalEntry, snapshot: &Snapshot) -> Result<()> {
        let old = entry
            .agent
            .as_ref()
            .context("Terminal has no retained Agent run")?;
        let pending = old
            .pending
            .as_ref()
            .context("Terminal has no queued assignment to continue")?;
        let project = snapshot
            .projects
            .iter()
            .find(|p| p.id == old.project_id)
            .context("Project missing")?;
        let card = snapshot
            .cards
            .iter()
            .find(|c| c.id == old.card_id)
            .context("Card missing")?;
        let client = self.herdr_client();
        ensure!(
            Some(self.verified_core_id()?) == old.core_id,
            "Original Core changed"
        );
        let terminal = old.terminal.as_ref().context("Terminal missing")?;
        let actual = client.get(terminal)?;
        ensure!(
            actual["state"] == "live"
                && matches!(actual["status"]["state"].as_str(), Some("idle" | "done")),
            "Retained Agent is no longer idle"
        );
        self.capture_output(entry)?;
        let prev_number = old.number;
        let next_number = u32::try_from(
            self.store
                .lock()
                .unwrap()
                .write(|db| db.counter(&format!("card-attempt:{}", card.id)))?,
        )?;
        let now_ms = self.now_ms();
        let mut promoted_entry = entry.clone();
        {
            let run = promoted_entry.agent.as_mut().unwrap();
            run.promote_pending_at(now_ms);
            run.card_number = card.number;
            run.number = next_number;
            run.started_at = Some(now_ms);
            run.state = "running".into();
            run.output_path = None;
            run.response_path = None;
            run.close_state.clear();
            run.error = None;
            run.completed_at = None;
            run.completion_turn = None;
            run.turn_ended_at = None;
            run.close_requested_at = None;
            run.observed = Some(json!({"state":"live","status":{"state":"working"}}));
            run.lifecycle.observed_at = Some(now_ms);
            run.lifecycle.waiting = None;
            run.lifecycle.idle_since = None;
            run.lifecycle.close_reason.clear();
            run.lifecycle.interactive_followup = false;
        }
        let card_events = self.store.lock().unwrap().db().events_for_card(&card.id)?;
        let input = canban_context::capture_with_durability(
            &self.home,
            project,
            card,
            &promoted_entry,
            snapshot,
            &card_events,
            self.fs_durability,
        )?;
        promoted_entry.agent.as_mut().unwrap().input_path = input.clone();
        let _ = pending;
        let prompt = std::fs::read_to_string(self.home.join(&input))?;
        let card_number = card.number.to_string();
        let text = canban_context::render_template(
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../prompts/runtime/continue_assignment.txt"
            ))
            .trim_end(),
            &[("card_number", &card_number), ("prompt", &prompt)],
        );
        let mut delivering_questions = Vec::new();
        self.store.lock().unwrap().write(|db| {
            let mut current: TerminalEntry = db.get(&entry.id)?;
            current.agent = promoted_entry.agent.clone();
            db.save(&current)?;
            for mut q in db
                .list::<Question>()?
                .into_iter()
                .filter(|q| q.terminal_id.as_deref() == Some(&entry.id) && q.state == "pending")
            {
                q.state = "delivering".into();
                delivering_questions.push(q.id.clone());
                db.save(&q)?;
            }
            log(
                db,
                card,
                "execution.continued",
                "system",
                "Started a new assignment in the retained Agent terminal",
                Some(entry.id.clone()),
                json!({"previous_run":prev_number,"run_number":next_number}),
                self.now_ms(),
            )
        })?;
        let send_res = client
            .call("terminal.send", json!({"terminal":terminal,"text":text}))
            .and_then(|_| self.check_failpoint(canban_types::FP_HERDR_AFTER_SEND_BEFORE_DB));
        if let Err(e) = send_res {
            let err_msg = format!("Assignment delivery uncertain: {e:#}");
            let failed_at = self.now_ms();
            self.store.lock().unwrap().write(|db| {
                let mut current: TerminalEntry = db.get(&entry.id)?;
                if let Some(run) = &mut current.agent {
                    run.state = "unknown".into();
                    run.error = Some(err_msg.clone());
                }
                db.save(&current)?;
                let mut marked_unknown = false;
                for qid in &delivering_questions {
                    if let Ok(mut q) = db.get::<Question>(qid)
                        && q.state == "delivering"
                    {
                        q.state = "delivery_unknown".into();
                        q.error = Some(err_msg.clone());
                        db.save(&q)?;
                        marked_unknown = true;
                    }
                }
                let mut c: Card = db.get(&card.id)?;
                let notice = if marked_unknown {
                    "Assignment and question delivery needs inspection; open the Agent terminal. It was not automatically resent."
                } else {
                    "Assignment delivery needs inspection; open the Agent terminal. It was not automatically resent."
                };
                attention(db, &mut c, notice, failed_at)?;
                if let Some(gate) = &mut c.human_gate
                    && gate.terminal_id.is_none()
                {
                    gate.terminal_id = Some(entry.id.clone());
                }
                c.revision += 1;
                db.save(&c)?;
                Ok(())
            })?;
            if !delivering_questions.is_empty() {
                self.emit("question", Some(card.id.clone()));
            }
            self.emit("runtime", Some(card.id.clone()));
            return Err(e);
        }
        if !delivering_questions.is_empty() {
            self.store.lock().unwrap().write(|db| {
                for qid in &delivering_questions {
                    if let Ok(mut q) = db.get::<Question>(qid)
                        && q.state == "delivering"
                    {
                        q.state = "delivered".into();
                        db.save(&q)?;
                    }
                }
                Ok(())
            })?;
            self.emit("question", Some(card.id.clone()));
        }
        self.emit("runtime", Some(card.id.clone()));
        Ok(())
    }
    pub(super) fn control_agent(&self, p: Value, end: bool) -> Result<Value> {
        let terminal_id = strp(&p, "terminal_id")?;
        let entry: TerminalEntry = self.store.lock().unwrap().db().get(terminal_id)?;
        let run = entry
            .agent
            .as_ref()
            .context("Terminal has no Card Agent run")?;
        let client = self.herdr_client();
        ensure!(
            Some(self.verified_core_id()?) == run.core_id,
            "Original Herdr core is unavailable"
        );
        let terminal = run.terminal.as_deref().context("Agent has no terminal")?;
        let card_id = run.card_id.clone();
        let run_number = run.number;
        // Commit the intent before sending a physical control action. A repeated
        // request key returns its saved result and never interrupts twice.
        let method = if end { "agent.end" } else { "agent.interrupt" };
        let mut execute = false;
        let saved = self.mutate(method, &p, |db| {
            let card: Card = db.get(&card_id)?;
            log(db, &card, "execution.control_requested", "user",
                "User requested terminal control", Some(entry.id.clone()),
                json!({"run_number":run_number,"action":method}), self.now_ms())?;
            execute = true;
            Ok(json!({"state":"unknown","message":"Control requested; inspect the terminal before repeating an uncertain action"}))
        })?;
        if !execute {
            return Ok(saved);
        }
        let result = match client.call(
            if end {
                "terminal.kill"
            } else {
                "terminal.interrupt"
            },
            json!({"terminal":terminal}),
        ) {
            Ok(value) => json!({"state":"sent","result":value}),
            Err(error) => {
                json!({"state":"unknown","message":format!("{error:#}; inspect the terminal before retrying")})
            }
        };
        self.store.lock().unwrap().write(|db| {
            if let Some(key) = p["request_key"].as_str() {
                db.conn.execute(
                    "UPDATE operations SET result=?2 WHERE id=?1",
                    params![key, serde_json::to_string(&result)?],
                )?;
            }
            let card: Card = db.get(&card_id)?;
            log(
                db,
                &card,
                if end {
                    "execution.ended"
                } else {
                    "execution.interrupted"
                },
                "user",
                "Terminal control delivery observed",
                Some(entry.id.clone()),
                json!({"run_number":run_number,"result":result}),
                self.now_ms(),
            )?;
            Ok(())
        })?;
        self.emit("runtime", Some(card_id));
        Ok(result)
    }
    pub(super) fn reply(&self, p: Value) -> Result<Value> {
        let (question, card, entry_opt) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let raw_qid = strp(&p, "question_id")?;
            let q: Question = match db.get(raw_qid) {
                Ok(q) => q,
                Err(_) => {
                    let mapped: Option<String> = db
                        .conn
                        .query_row(
                            "SELECT new_id FROM id_migrations WHERE kind='question' AND old_id=?1",
                            [raw_qid],
                            |r| r.get(0),
                        )
                        .optional()
                        .unwrap_or(None);
                    if let Some(new_id) = mapped {
                        db.get(&new_id)?
                    } else {
                        db.get(raw_qid)?
                    }
                }
            };
            if p["card_id"].as_str().is_some_and(|s| !s.trim().is_empty())
                || super::parse_card_number(&p["card"]).is_some()
            {
                let resolved = super::resolve_card(db, &p)?;
                ensure!(resolved.id == q.card_id, "Question belongs to another Card");
            }
            let card: Card = db.get(&q.card_id)?;
            let entry = q
                .terminal_id
                .as_ref()
                .and_then(|tid| db.get::<TerminalEntry>(tid).ok());
            (q, card, entry)
        };
        let body = strp(&p, "body")?;
        let file = if let Some(entry) = &entry_opt
            && let Some(run) = &entry.agent
        {
            canban_context::card_dir(&self.home, &card.project_id, card.number)
                .join("agents")
                .join(&run.profile.id)
                .join("replies")
                .join(format!("{}.md", question.id))
        } else {
            canban_context::card_dir(&self.home, &card.project_id, card.number)
                .join("replies")
                .join(format!("{}.md", question.id))
        };
        // The transaction's operation key makes repeated reply publication idempotent.
        self.mutate("agent.reply", &p, |db| {
            let mut q: Question = db.get(&question.id)?;
            ensure!(q.state != "replied", "This question already has a reply");
            canban_context::atomic_with_durability(&file, body.as_bytes(), self.fs_durability)?;
            q.reply_path = Some(canban_context::relative(&self.home, &file)?);
            q.state = "replied".into();
            q.error = None;
            db.save(&q)?;
            let card: Card = db.get(&q.card_id)?;
            log(
                db,
                &card,
                "comment.replied",
                &format!("agent:{}", q.profile_id),
                body,
                q.terminal_id.clone(),
                json!({"reply_to":q.comment_id,"question_id":q.id,"path":q.reply_path}),
                self.now_ms(),
            )?;
            Ok(json!({"accepted":true,"question_id":q.id}))
        })
    }
}
