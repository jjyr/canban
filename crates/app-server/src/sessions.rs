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
        last_request_id: None,
        created_at: now(),
    };
    db.save(&item)?;
    db.agent_terminal(&item)?;
    Ok(item)
}

impl AppServer {
    pub(super) fn bind_session(&self, p: Value) -> Result<Value> {
        self.mutate("agent.bind", &p, |db| {
            let request: Request = db.get(strp(&p, "request_id")?)?;
            let number = u32::try_from(p["attempt"].as_u64().context("attempt required")?)?;
            ensure!(
                number == request.attempt,
                "Cannot bind an obsolete invocation"
            );
            let mut run: Attempt = db.get(&Attempt::key(&request.id, number))?;
            let card: Card = db.get(&request.card_id)?;
            let mut context = conversation(db, &card, &request.profile)?;
            let session = strp(&p, "session_id")?;
            ensure!(
                !session.is_empty() && session.len() < 512,
                "Invalid provider session ID"
            );
            if let Some(old) = &context.provider_session_id {
                ensure!(
                    old == session,
                    "Provider conversation changed unexpectedly; inspect the selected terminal"
                );
            }
            let attempts = db.list::<Attempt>()?;
            let machine = run
                .location
                .as_ref()
                .map(|l| l.machine.as_str())
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
            run.provider_session_id = Some(session.into());
            run.conversation_id = Some(context.id.clone());
            db.save(&context)?;
            db.save(&run)?;
            Ok(json!({"bound":true,"conversation_id":context.id,"session_id":session}))
        })
    }
    pub(super) fn open_agent(&self, p: Value) -> Result<Value> {
        let config = self.config();
        self.mutate("agent.open",&p,|db| {
            let mut card:Card=db.get(strp(&p,"card_id")?)?;
            ensure!(card.archived_at.is_none(),"Restore this Card before opening its Agent");
            let project:Project=db.get(&card.project_id)?;
            let profile=canban_scheduler::select_in_project(&config,Some(&project),Some(strp(&p,"profile_id")?))?;
            let mut context=conversation(db,&card,&profile)?;
            if let Some(expected)=p["expected_attempt_id"].as_str() {
                let snapshot=db.snapshot()?;
                let row=canban_notifications::overview(&snapshot).into_iter().find(|r|r.id == context.id).context("Conversation no longer exists")?;
                let run=row.attempt.context("Conversation has no invocation")?;
                ensure!(run.id == expected && run.finished(),"Overview invocation changed; select it again");
                ensure!(context.provider_session_id.is_some() && context.provider_session_id == run.provider_session_id,"Exact provider session is unavailable");
                ensure!(!profile.resume_command().is_empty(),"Profile has no resume command");
            }
            if let Some(id)=&context.last_request_id {
                let req:Request=db.get(id)?;
                let run=db.get::<Attempt>(&Attempt::key(&req.id,req.attempt)).ok();
                let live=run.as_ref().is_some_and(|a|a.terminal.is_some() && a.observed.as_ref().is_none_or(|v|v["state"]!="finished"));
                if (matches!(req.state.as_str(),"queued"|"starting"|"unknown") && !(card.human_gate.is_some() && req.state=="queued" && canban_scheduler::automatic(&req))) || live {
                    return Ok(json!({"request_id":req.id,"reused":true,"attempt":run}));
                }
            }
            if p["fresh"]==true || context.provider != profile.command_name() {
                context.provider_session_id=None;
                context.provider=profile.command_name();
                db.save(&context)?;
            }
            let stage=canban_workflow_engine::stage(&project.workflow,&card.stage)?;
            let req=enqueue(db,&config,&mut card,&project,stage,"interactive",Some(&profile.id),Some("The user sent this Card to you for discussion. Read the Card context, then reply with a brief summary of your understanding and any open questions. Do not start implementing and do not report completion for this initialization. Wait for the user's next instruction; when that requested work is complete, explicitly report its outcome.".into()))?.context("Agent was not queued")?;
            context.last_request_id=Some(req.id.clone());db.save(&context)?;
            Ok(json!({"request_id":req.id,"reused":false}))
        })
    }
    pub(super) fn continue_assignment(
        &self,
        req: &Request,
        old: &Attempt,
        snapshot: &Snapshot,
    ) -> Result<()> {
        let project = snapshot
            .projects
            .iter()
            .find(|p| p.id == req.project_id)
            .context("Project missing")?;
        let card = snapshot
            .cards
            .iter()
            .find(|c| c.id == req.card_id)
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
            req,
            req.attempt,
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
        let mut run = old.clone();
        run.id = Attempt::key(&req.id, req.attempt);
        run.request_id = req.id.clone();
        run.number = req.attempt;
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
        run.created_at = now();
        run.observed = Some(json!({"state":"live","status":{"state":"working"}}));
        run.lifecycle.observed_at = Some(now());
        run.lifecycle.waiting = None;
        run.lifecycle.idle_since = None;
        self.store.lock().unwrap().write(|db| {
            let mut previous: Attempt = db.get(&old.id)?;
            previous.state = "continued".into();
            db.save(&previous)?;
            db.save(&run)?;
            let mut current: Request = db.get(&req.id)?;
            current.state = "running".into();
            db.save(&current)?;
            let mut context = conversation(db, card, &req.profile)?;
            context.last_request_id = Some(req.id.clone());
            db.save(&context)?;
            for mut q in db
                .list::<Question>()?
                .into_iter()
                .filter(|q| q.request_id == req.id && q.attempt == 0)
            {
                q.attempt = req.attempt;
                db.save(&q)?;
            }
            log(
                db,
                card,
                "execution.continued",
                "system",
                "Started a new assignment in the retained Agent terminal",
                Some(req.id.clone()),
                json!({"previous_attempt":old.id,"attempt":req.attempt}),
            )
        })?;
        let prompt = std::fs::read_to_string(self.home.join(&input))?;
        let run_dir = canban_context::run_dir(&runtime_home, req, req.attempt);
        let text = format!(
            "New Canban assignment (use these explicit values instead of earlier terminal env vars):\nCANBAN_REQUEST_ID={} CANBAN_ATTEMPT={} CANBAN_RUN_DIR={}\n\n{}",
            canban_context::shell_quote(&req.id),
            req.attempt,
            canban_context::shell_quote(&run_dir.to_string_lossy()),
            prompt
        );
        if let Err(e) = client.call("terminal.send", json!({"terminal":terminal,"text":text})) {
            self.store.lock().unwrap().write(|db| {
                let mut run: Attempt = db.get(&run.id)?;
                run.state = "unknown".into();
                run.error = Some(format!("Assignment delivery uncertain: {e:#}"));
                db.save(&run)?;
                let mut r: Request = db.get(&req.id)?;
                r.state = "unknown".into();
                db.save(&r)
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
                "User requested terminal control", Some(run.request_id.clone()),
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
                Some(run.request_id.clone()),
                json!({"attempt":run.number,"result":result}),
            )?;
            Ok(())
        })?;
        self.emit("runtime", Some(run.card_id));
        Ok(result)
    }
    pub(super) fn reply(&self, p: Value) -> Result<Value> {
        let question: Question = self
            .store
            .lock()
            .unwrap()
            .db()
            .get(strp(&p, "question_id")?)?;
        let request: Request = self
            .store
            .lock()
            .unwrap()
            .db()
            .get(strp(&p, "request_id")?)?;
        ensure!(
            question.request_id == request.id
                && p["attempt"].as_u64() == Some(u64::from(question.attempt)),
            "Reply belongs to another invocation"
        );
        let body = strp(&p, "body")?;
        let file = canban_context::run_dir(&self.home, &request, question.attempt)
            .join("replies")
            .join(format!("{}.md", question.id));
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
                Some(request.id.clone()),
                json!({"reply_to":q.comment_id,"question_id":q.id,"path":q.reply_path}),
            )?;
            Ok(json!({"accepted":true,"question_id":q.id}))
        })
    }
}
