use super::*;

pub(super) fn conversation(db: Db<'_>, card: &Card, profile: &Profile) -> Result<Conversation> {
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
        last_attempt_id: None,
        created_at: now(),
    };
    db.save(&item)?;
    db.agent_terminal(&item)?;
    Ok(item)
}

pub(super) fn retry_attempt(db: Db<'_>, card: &Card, previous: &Attempt) -> Result<Attempt> {
    if previous.state == "unknown" {
        let mut old = previous.clone();
        old.state = "superseded".into();
        db.save(&old)?;
    }
    let number = u32::try_from(db.counter(&format!("card-attempt:{}", card.id))?)?;
    let mut next = canban_scheduler::attempt(
        &previous.project_id,
        &card.id,
        number,
        &previous.stage,
        previous.trigger.clone(),
        previous.profile.clone(),
        previous.instruction.clone(),
        previous.on_completed.clone(),
    );
    next.workflow_run_id = previous.workflow_run_id.clone();
    next.workflow_step = previous.workflow_step.clone();
    next.require_human_review = previous.require_human_review;
    next.conversation_id = previous.conversation_id.clone();
    next.resume_from = previous
        .provider_session_id
        .as_ref()
        .map(|_| previous.id.clone())
        .or_else(|| previous.resume_from.clone());
    db.save(&next)?;
    if let Some(cid) = &next.conversation_id
        && let Ok(mut ctx) = db.get::<Conversation>(cid)
    {
        ctx.last_attempt_id = Some(next.id.clone());
        db.save(&ctx)?;
    }
    if let Some(run_id) = &next.workflow_run_id
        && let Ok(mut wf_run) = db.get::<WorkflowRun>(run_id)
        && wf_run.active()
        && wf_run.attempt_id.as_deref() == Some(&previous.id)
    {
        wf_run.attempt_id = Some(next.id.clone());
        db.save(&wf_run)?;
    }
    Ok(next)
}

impl AppServer {
    pub(super) fn bind_session(&self, p: Value) -> Result<Value> {
        let config = self.config();
        self.mutate("agent.bind", &p, |db| {
            let session = strp(&p, "session_id")?;
            ensure!(
                !session.is_empty() && session.len() < 512,
                "Invalid provider session ID"
            );
            let attempts = db.list::<Attempt>()?;
            let (card, profile, mut run_opt) = if let Some(aid) = p["attempt_id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let run: Attempt = db.get(aid)?;
                ensure!(
                    run.state != "continued",
                    "Cannot bind an obsolete invocation"
                );
                let card: Card = db.get(&run.card_id)?;
                let prof = run.profile.clone();
                (card, prof, Some(run))
            } else {
                let card = super::resolve_card(db, &p)?;
                let project: Project = db.get(&card.project_id)?;
                let requested_prof = p["profile_id"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let active_run = attempts
                    .iter()
                    .rev()
                    .find(|a| {
                        a.card_id == card.id
                            && a.state != "continued"
                            && !a.finished()
                            && requested_prof.is_none_or(|pid| a.profile.id == pid)
                    })
                    .cloned();
                let prof = if let Some(run) = &active_run {
                    run.profile.clone()
                } else {
                    canban_scheduler::select_in_project(&config, Some(&project), requested_prof)?
                };
                (card, prof, active_run)
            };
            let mut context = conversation(db, &card, &profile)?;
            if let Some(old) = &context.provider_session_id {
                ensure!(
                    old == session,
                    "Provider conversation changed unexpectedly; inspect the selected terminal"
                );
            }
            let machine = run_opt
                .as_ref()
                .and_then(|r| r.location.as_ref())
                .map(|l| l.machine.as_str())
                .or_else(|| card.workspace.as_ref().map(|w| w.location.machine.as_str()))
                .unwrap_or("local");
            ensure!(
                !db.list::<Conversation>()?.iter().any(|c| c.id != context.id
                    && c.provider == context.provider
                    && c.provider_session_id.as_deref() == Some(session)
                    && attempts
                        .iter()
                        .any(|a| a.conversation_id.as_deref() == Some(&c.id)
                            && a.provider_session_id.as_deref() == Some(session)
                            && a.location
                                .as_ref()
                                .map(|l| l.machine.as_str())
                                .unwrap_or("local")
                                == machine)),
                "Provider session is already bound to another Card/profile on this machine"
            );
            context.provider_session_id = Some(session.into());
            db.save(&context)?;
            if let Some(ref mut run) = run_opt {
                run.provider_session_id = Some(session.into());
                run.conversation_id = Some(context.id.clone());
                db.save(run)?;
            }
            Ok(json!({"bound":true,"conversation_id":context.id,"session_id":session}))
        })
    }
    pub(super) fn open_agent(&self, p: Value) -> Result<Value> {
        let config = self.config();
        self.mutate("agent.open", &p, |db| {
            let mut card = super::resolve_card(db, &p)?;
            ensure!(
                card.archived_at.is_none(),
                "Restore this Card before opening its Agent"
            );
            let project: Project = db.get(&card.project_id)?;
            let profile = canban_scheduler::select_in_project(
                &config,
                Some(&project),
                Some(strp(&p, "profile_id")?),
            )?;
            let mut context = conversation(db, &card, &profile)?;
            if let Some(expected) = p["expected_attempt_id"].as_str() {
                let snapshot = db.snapshot()?;
                let row = canban_notifications::overview(&snapshot)
                    .into_iter()
                    .find(|r| r.id == context.id)
                    .context("Conversation no longer exists")?;
                let run = row.attempt.context("Conversation has no invocation")?;
                ensure!(
                    run.id == expected && run.finished(),
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
            if let Some(id) = &context.last_attempt_id
                && let Ok(att) = db.get::<Attempt>(id)
            {
                let live = att.terminal.is_some()
                    && att
                        .observed
                        .as_ref()
                        .is_none_or(|v| v["state"] != "finished");
                if (matches!(att.state.as_str(), "queued" | "starting" | "unknown")
                    && !(card.human_gate.is_some()
                        && att.state == "queued"
                        && canban_scheduler::automatic(&att)))
                    || live
                {
                    return Ok(json!({"attempt_id":att.id,"reused":true,"attempt":att}));
                }
            }
            if p["fresh"] == true || context.provider != profile.command_name() {
                context.provider_session_id = None;
                context.provider = profile.command_name();
                db.save(&context)?;
            }
            let stage = canban_workflow_engine::stage(&project.workflow, &card.stage)?;
            let init_prompt = include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../prompts/runtime/interactive_init.txt"
            ))
            .trim_end()
            .to_owned();
            let att = enqueue(
                db,
                &config,
                &mut card,
                &project,
                stage,
                ExecutionTrigger::Interactive,
                Some(&profile.id),
                Some(init_prompt),
            )?
            .context("Agent was not queued")?;
            context.last_attempt_id = Some(att.id.clone());
            db.save(&context)?;
            Ok(json!({"attempt_id":att.id,"reused":false,"attempt":att}))
        })
    }
    pub(super) fn continue_assignment(
        &self,
        att: &Attempt,
        old: &Attempt,
        snapshot: &Snapshot,
    ) -> Result<()> {
        let project = snapshot
            .projects
            .iter()
            .find(|p| p.id == att.project_id)
            .context("Project missing")?;
        let card = snapshot
            .cards
            .iter()
            .find(|c| c.id == att.card_id)
            .context("Card missing")?;
        let location = old
            .location
            .as_ref()
            .context("Retained execution location missing")?;
        let runtime_home =
            PathBuf::from(old.runtime_home.as_ref().context("Runtime home missing")?);
        let remote = location.machine != "local";
        let endpoint = if remote {
            runtime_home.join("app.sock")
        } else {
            self.endpoint()
        };
        let cli = if remote {
            runtime_home.join("bin/canban")
        } else {
            self.cli.clone()
        };
        let input = canban_context::capture_for(
            &self.home,
            project,
            card,
            att,
            snapshot,
            &endpoint.to_string_lossy(),
            &cli,
            &runtime_home,
        )?;
        if remote {
            self.stage_remote(&self.ops(&location.machine).host, &runtime_home, card)?;
        }
        let client = canban_herdr::Client {
            config: self.config(),
        };
        ensure!(
            client.status()?["core"]["id"].as_str() == old.core_id.as_deref(),
            "Original Core changed"
        );
        let terminal = old.terminal.as_ref().context("Terminal missing")?;
        let actual = client.get(terminal)?;
        ensure!(
            actual["state"] == "live"
                && matches!(actual["status"]["state"].as_str(), Some("idle" | "done")),
            "Retained Agent is no longer idle"
        );
        self.capture_output(old)?;
        let mut run = att.clone();
        run.runtime_home = old.runtime_home.clone();
        run.workspace = old.workspace.clone();
        run.location = old.location.clone();
        run.owner_core_id = old.owner_core_id.clone();
        run.core_id = old.core_id.clone();
        run.terminal = old.terminal.clone();
        run.cwd = old.cwd.clone();
        run.conversation_id = old.conversation_id.clone();
        run.provider_session_id = old.provider_session_id.clone();
        run.connection_state = old.connection_state.clone();
        run.state = "running".into();
        run.input_path = input.clone();
        run.output_path = None;
        run.response_path = None;
        run.close_state.clear();
        run.error = None;
        run.completed_at = None;
        run.completion_turn = None;
        run.turn_ended_at = None;
        run.close_requested_at = None;
        run.observed = Some(json!({"state":"live","status":{"state":"working"}}));
        run.lifecycle = old.lifecycle.clone();
        run.lifecycle.observed_at = Some(now());
        run.lifecycle.waiting = None;
        run.lifecycle.idle_since = None;
        run.lifecycle.close_reason.clear();
        self.store.lock().unwrap().write(|db| {
            let mut previous: Attempt = db.get(&old.id)?;
            previous.state = "continued".into();
            db.save(&previous)?;
            db.save(&run)?;
            let mut context = conversation(db, card, &att.profile)?;
            context.last_attempt_id = Some(run.id.clone());
            db.save(&context)?;
            for mut q in db
                .list::<Question>()?
                .into_iter()
                .filter(|q| q.attempt_id.as_deref() == Some(&run.id) && q.state == "pending")
            {
                q.state = "delivered".into();
                db.save(&q)?;
            }
            log(
                db,
                card,
                "execution.continued",
                "system",
                "Started a new assignment in the retained Agent terminal",
                Some(run.id.clone()),
                json!({"previous_attempt":old.id,"attempt":run.number}),
            )
        })?;
        let prompt = std::fs::read_to_string(self.home.join(&input))?;
        let run_dir = canban_context::run_dir(&runtime_home, &run);
        let proj_quoted = canban_context::shell_quote(&card.project_id);
        let card_number = card.number.to_string();
        let run_dir_quoted = canban_context::shell_quote(&run_dir.to_string_lossy());
        let text = canban_context::render_template(
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../prompts/runtime/continue_assignment.txt"
            ))
            .trim_end(),
            &[
                ("project_id", &proj_quoted),
                ("card_number", &card_number),
                ("run_dir", &run_dir_quoted),
                ("prompt", &prompt),
            ],
        );
        if let Err(e) = client.call("terminal.send", json!({"terminal":terminal,"text":text})) {
            self.store.lock().unwrap().write(|db| {
                let mut run: Attempt = db.get(&run.id)?;
                run.state = "unknown".into();
                run.error = Some(format!("Assignment delivery uncertain: {e:#}"));
                db.save(&run)
            })?;
            return Err(e);
        }
        self.emit("runtime", Some(card.id.clone()));
        Ok(())
    }
    pub(super) fn control_agent(&self, p: Value, end: bool) -> Result<Value> {
        let run: Attempt = self
            .store
            .lock()
            .unwrap()
            .db()
            .get(strp(&p, "attempt_id")?)?;
        let client = canban_herdr::Client {
            config: self.config(),
        };
        ensure!(
            client.status()?["core"]["id"].as_str() == run.core_id.as_deref(),
            "Original Herdr core is unavailable"
        );
        let terminal = run.terminal.as_deref().context("Agent has no terminal")?;
        // Commit the intent before sending a physical control action. A repeated
        // request key returns its saved result and never interrupts twice.
        let method = if end { "agent.end" } else { "agent.interrupt" };
        let mut execute = false;
        let saved = self.mutate(method, &p, |db| {
            let card: Card = db.get(&run.card_id)?;
            log(db, &card, "execution.control_requested", "user",
                "User requested terminal control", Some(run.id.clone()),
                json!({"attempt":run.number,"action":method}))?;
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
            let card: Card = db.get(&run.card_id)?;
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
                Some(run.id.clone()),
                json!({"attempt":run.number,"result":result}),
            )?;
            Ok(())
        })?;
        self.emit("runtime", Some(run.card_id));
        Ok(result)
    }
    pub(super) fn reply(&self, p: Value) -> Result<Value> {
        let (question, attempt_opt) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let q: Question = db.get(strp(&p, "question_id")?)?;
            if p["card_id"].as_str().is_some_and(|s| !s.trim().is_empty())
                || super::parse_card_number(&p["card"]).is_some()
            {
                let resolved = super::resolve_card(db, &p)?;
                ensure!(resolved.id == q.card_id, "Question belongs to another Card");
            }
            let att = q
                .attempt_id
                .as_ref()
                .and_then(|aid| db.get::<Attempt>(aid).ok());
            (q, att)
        };
        let body = strp(&p, "body")?;
        let file = if let Some(att) = &attempt_opt {
            canban_context::run_dir(&self.home, att)
                .join("replies")
                .join(format!("{}.md", question.id))
        } else {
            canban_context::card_dir(&self.home, &question.project_id, &question.card_id)
                .join("replies")
                .join(format!("{}.md", question.id))
        };
        // The transaction's operation key makes repeated reply publication idempotent.
        self.mutate("agent.reply", &p, |db| {
            let mut q: Question = db.get(&question.id)?;
            ensure!(q.state != "replied", "This question already has a reply");
            canban_context::atomic(&file, body.as_bytes())?;
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
                q.attempt_id.clone(),
                json!({"reply_to":q.comment_id,"question_id":q.id,"path":q.reply_path}),
            )?;
            Ok(json!({"accepted":true,"question_id":q.id}))
        })
    }
}
