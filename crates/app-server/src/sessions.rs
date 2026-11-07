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
            ensure!(
                !db.list::<Conversation>()?.iter().any(|c| c.id != context.id
                    && c.provider == context.provider
                    && c.provider_session_id.as_deref() == Some(session)),
                "Provider session is already bound to another Card/profile"
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
            let profile=canban_scheduler::select(&config,&[],Some(strp(&p,"profile_id")?))?;
            let mut context=conversation(db,&card,&profile)?;
            if let Some(id)=&context.last_request_id {
                let req:Request=db.get(id)?;
                let run=db.get::<Attempt>(&Attempt::key(&req.id,req.attempt)).ok();
                let live=run.as_ref().is_some_and(|a|a.terminal.is_some() && a.observed.as_ref().is_none_or(|v|v["state"]!="finished"));
                if matches!(req.state.as_str(),"queued"|"starting"|"unknown") || live {
                    return Ok(json!({"request_id":req.id,"reused":true,"attempt":run}));
                }
            }
            if p["fresh"]==true {context.provider_session_id=None;context.provider=profile.command_name();db.save(&context)?;}
            let project:Project=db.get(&card.project_id)?;
            let stage=canban_workflow_engine::stage(&project.workflow,&card.stage)?;
            let req=enqueue(db,&config,&mut card,&project,stage,"interactive",Some(&profile.id),Some("Initialize this Card conversation and briefly say you are ready. Do not report completion for this initialization. Wait for the user's next instruction; when that requested work is complete, explicitly report its outcome.".into()))?.context("Agent was not queued")?;
            context.last_request_id=Some(req.id.clone());db.save(&context)?;
            Ok(json!({"request_id":req.id,"reused":false}))
        })
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
