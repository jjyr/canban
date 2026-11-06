use super::*;
fn uuid_session_id() -> String {
    let s = id();
    format!(
        "{}-{}-{}-{}-{}",
        &s[..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..]
    )
}

impl AppServer {
    pub fn start(self: &Arc<Self>) -> Result<Running> {
        let socket = self.endpoint();
        if socket.exists() {
            ensure!(
                UnixStream::connect(&socket).is_err(),
                "Another Canban AppServer already owns {}",
                socket.display()
            );
            std::fs::remove_file(&socket)?;
        }
        let listener = UnixListener::bind(&socket).context("Could not bind Canban local socket")?;
        listener.set_nonblocking(true)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        // Persisted starting intents must not be replayed as fresh launches.
        // A known binding can reconnect. A lost launch reply remains unknown.
        self.store.lock().unwrap().write(|db|{for mut req in db.list::<Request>()?.into_iter().filter(|r|r.state=="starting"){let run=db.get::<Attempt>(&Attempt::key(&req.id,req.attempt)).ok();req.state=if run.as_ref().and_then(|a|a.terminal.as_ref()).is_some(){"running"}else{"unknown"}.into();db.save(&req)?;if req.state=="unknown"{if let Some(mut run)=run {run.state="unknown".into();db.save(&run)?;}let mut card:Card=db.get(&req.card_id)?;attention(db,&mut card,"Launch interrupted before its receipt was saved. Inspect Herdr before explicitly retrying.")?;card.revision+=1;db.save(&card)?;}}Ok(())})?;
        self.store.lock().unwrap().write(|db| {
            for mut q in db.list::<Question>()?.into_iter().filter(|q| q.state == "delivering") {
                q.state = "delivery_unknown".into();
                q.error = Some("AppServer restarted during delivery; inspect the Agent terminal before resending".into());
                let mut card: Card = db.get(&q.card_id)?;
                attention(db, &mut card, q.error.as_deref().unwrap())?;
                card.revision += 1;
                db.save(&card)?;
                db.save(&q)?;
            }
            Ok(())
        })?;
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            while let Some(server) = weak.upgrade() {
                if server.stop.load(Ordering::SeqCst) {
                    break;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        let server = server.clone();
                        std::thread::spawn(move || {
                            let _ = server.serve(stream);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(50))
                    }
                    Err(_) => break,
                }
            }
            let _ = std::fs::remove_file(socket);
        });
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            while let Some(server) = weak.upgrade() {
                if server.stop.load(Ordering::SeqCst) {
                    break;
                }
                if let Err(e) = server.tick() {
                    eprintln!("Canban scheduler: {e:#}");
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
        Ok(Running {
            server: self.clone(),
        })
    }
    pub(super) fn serve(&self, mut stream: UnixStream) -> Result<()> {
        // macOS accept() can inherit O_NONBLOCK from the listener. Explicitly
        // use blocking per-client IO so large JSON frames cannot be truncated.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(20)))?;
        stream.set_write_timeout(Some(Duration::from_secs(90)))?;
        let mut line = String::new();
        BufReader::new(stream.try_clone()?)
            .take(48 * 1024 * 1024)
            .read_line(&mut line)?;
        let input: Value = serde_json::from_str(&line)?;
        let id = input["id"].clone();
        let reply = match self.call(strp(&input, "method")?, input["params"].clone()) {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(e) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":format!("{e:#}")}})
            }
        };
        writeln!(stream, "{reply}")?;
        Ok(())
    }
    pub(super) fn tick(&self) -> Result<()> {
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let snapshot = self.snapshot()?;
        let bound: Vec<_> = snapshot
            .attempts
            .iter()
            .filter(|a| {
                a.terminal.is_some()
                    && (a.observed.as_ref().is_none_or(|v| v["state"] != "finished")
                        || a.output_path.is_none())
            })
            .cloned()
            .collect();
        if !bound.is_empty() {
            let core = client.status()?;
            for run in bound {
                if run.core_id.as_deref() != core["core"]["id"].as_str() {
                    continue;
                }
                let Ok(value) = client.get(run.terminal.as_ref().unwrap()) else {
                    continue;
                };
                let finished = value["state"] == "finished";
                if finished && run.output_path.is_none() {
                    let _ = self.capture_output(&run);
                }
                let observed = canban_herdr::observation(&value);
                if run.observed.as_ref() != Some(&observed)
                    || (finished && run.close_state != "closed")
                {
                    self.store.lock().unwrap().write(|db|{
                        let mut current:Attempt=db.get(&run.id)?;
                        current.observed=Some(observed.clone());
                        if finished {
                            current.close_state="closed".into();
                            if matches!(current.state.as_str(),"running"|"working"|"starting") {
                                current.state="needs_report".into();
                                let mut req:Request=db.get(&run.request_id)?;
                                if req.attempt==run.number && req.state!="completed" {req.state="needs_report".into();db.save(&req)?;}
                                let mut card:Card=db.get(&run.card_id)?;
                                attention(db,&mut card,"Agent exited without an explicit outcome. Inspect the retained output before resuming.")?;
                                card.revision+=1;db.save(&card)?;
                            }
                        }
                        db.save(&current)
                    })?;
                    self.emit("runtime", Some(run.card_id.clone()));
                }
            }
        }
        self.store.lock().unwrap().write(|db| {
            for mut run in db.list::<Attempt>()?.into_iter().filter(|a| a.close_state == "closing" && a.turn_ended_at.is_some_and(|t| now().saturating_sub(t) > 30_000)) {
                run.close_state = "error".into();
                run.error = Some("Agent did not exit after the normal exit request. Inspect its terminal or use End session.".into());
                let mut card: Card = db.get(&run.card_id)?;
                attention(db, &mut card, run.error.as_deref().unwrap())?;
                card.revision += 1;
                db.save(&card)?;
                db.save(&run)?;
            }
            Ok(())
        })?;
        self.deliver_questions(&client)?;
        self.close_completed(&client)?;
        let snapshot = self.snapshot()?;
        let request = snapshot
            .requests
            .iter()
            .find(|req| {
                if req.state != "queued" {
                    return false;
                }
                !snapshot.attempts.iter().any(|a| {
                    a.card_id == req.card_id
                        && snapshot
                            .requests
                            .iter()
                            .any(|r| r.id == a.request_id && r.profile.id == req.profile.id)
                        && (a.terminal.is_some()
                            && a.observed.as_ref().is_none_or(|v| v["state"] != "finished")
                            || a.state == "unknown")
                })
            })
            .cloned();
        if let Some(request) = request {
            let req = self.store.lock().unwrap().write(|db| {
                let mut req: Request = db.get(&request.id)?;
                let card: Card = db.get(&req.card_id)?;
                if card.archived_at.is_some() {
                    req.state = "cancelled".into();
                    db.save(&req)?;
                    return Ok(None);
                }
                req.state = "starting".into();
                req.attempt += 1;
                db.save(&req)?;
                Ok(Some(req))
            })?;
            if let Some(req) = req
                && let Err(e) = self.launch(&req, &snapshot)
            {
                self.launch_failed(&req, &format!("{e:#}"))?;
            }
        }
        Ok(())
    }
    fn deliver_questions(&self, client: &canban_herdr::Client) -> Result<()> {
        let snapshot = self.snapshot()?;
        for question in snapshot
            .questions
            .iter()
            .filter(|q| matches!(q.state.as_str(), "pending" | "delivered"))
        {
            let Some(req) = snapshot
                .requests
                .iter()
                .find(|r| r.id == question.request_id)
            else {
                continue;
            };
            if req.state == "completed" && question.state == "pending" {
                // A question that raced task completion follows the same conversation
                // after close. It is never silently redirected to the Reviewer.
                let config = self.config();
                self.store.lock().unwrap().write(|db|{
                    let mut q:Question=db.get(&question.id)?;
                    if q.state=="replied"{return Ok(());}
                    let mut card:Card=db.get(&q.card_id)?;
                    if card.archived_at.is_some(){return Ok(());}
                    let project:Project=db.get(&card.project_id)?;
                    let stage=canban_workflow_engine::stage(&project.workflow,&card.stage)?;
                    let request=enqueue(db,&config,&mut card,&project,stage,&format!("comment:{}",q.comment_id),Some(&q.profile_id),Some(format!("Answer this retained question, publish its reply, then report completion.\n{}",q.body)))?.context("Could not retain question")?;
                    q.request_id=request.id;q.attempt=0;q.state="pending".into();db.save(&q)
                })?;
                continue;
            }
            if question.state != "pending"
                || !matches!(req.state.as_str(), "running" | "working" | "blocked")
            {
                continue;
            }
            let Some(run) = snapshot
                .attempts
                .iter()
                .find(|a| a.request_id == req.id && a.number == req.attempt)
            else {
                continue;
            };
            let Some(terminal) = &run.terminal else {
                continue;
            };
            if run
                .observed
                .as_ref()
                .is_some_and(|v| v["state"] == "finished")
            {
                continue;
            }
            self.store.lock().unwrap().write(|db| {
                let mut q: Question = db.get(&question.id)?;
                q.state = "delivering".into();
                q.attempt = run.number;
                db.save(&q)
            })?;
            let text = format!(
                "Canban question {} (additional input; keep your original task and completion callback):\n{}\nPublish the answer using \"$CANBAN_CLI\" reply --to {} --file /absolute/path/to/answer.md. A reply does not complete your original task.",
                question.id,
                question.body,
                canban_context::shell_quote(&question.id)
            );
            let result = if client.status()?["core"]["id"].as_str() == run.core_id.as_deref() {
                client.call("terminal.send", json!({"terminal":terminal,"text":text}))
            } else {
                Err(anyhow::anyhow!("Original core identity changed"))
            };
            self.store.lock().unwrap().write(|db|{
                let mut q:Question=db.get(&question.id)?;
                if q.state=="replied"{return Ok(());}
                match &result {Ok(_)=>q.state="delivered".into(),Err(e)=>{q.state="delivery_unknown".into();q.error=Some(format!("{e:#}"));let mut c:Card=db.get(&q.card_id)?;attention(db,&mut c,"Question delivery needs inspection; open the Agent terminal. It was not automatically resent.")?;c.revision+=1;db.save(&c)?;}}
                db.save(&q)
            })?;
            self.emit("question", Some(question.card_id.clone()));
        }
        Ok(())
    }
    fn close_completed(&self, client: &canban_herdr::Client) -> Result<()> {
        for run in self.snapshot()?.attempts.into_iter().filter(|a| {
            a.close_state == "pending"
                && a.turn_ended_at
                    .is_some_and(|t| now().saturating_sub(t) > 300)
        }) {
            if run
                .observed
                .as_ref()
                .is_some_and(|v| v["state"] == "finished")
            {
                continue;
            }
            let Some(terminal) = &run.terminal else {
                continue;
            };
            self.store.lock().unwrap().write(|db| {
                let mut a: Attempt = db.get(&run.id)?;
                a.close_state = "closing".into();
                db.save(&a)
            })?;
            let result = (|| {
                ensure!(
                    client.status()?["core"]["id"].as_str() == run.core_id.as_deref(),
                    "Original Herdr core is unavailable"
                );
                client.call("terminal.send", json!({"terminal":terminal,"text":"/exit"}))
            })();
            if let Err(e) = result {
                self.store.lock().unwrap().write(|db| {
                    let mut a: Attempt = db.get(&run.id)?;
                    a.close_state = "error".into();
                    a.error = Some(format!(
                        "Normal exit delivery uncertain: {e:#}. Inspect before retrying."
                    ));
                    db.save(&a)
                })?;
            }
            self.emit("runtime", Some(run.card_id));
        }
        Ok(())
    }
    pub(super) fn launch(&self, req: &Request, snapshot: &Snapshot) -> Result<()> {
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
        let cwd = canban_workspaces::prepare(&self.home, project, card)?;
        let input = canban_context::capture(
            &self.home,
            project,
            card,
            req,
            req.attempt,
            snapshot,
            &self.endpoint().to_string_lossy(),
            &self.cli,
        )?;
        let (mut context, fresh) = self.store.lock().unwrap().write(|db| {
            let mut c = crate::sessions::conversation(db, card, &req.profile)?;
            let fresh = c.provider_session_id.is_none();
            if fresh && req.profile.provider_kind() == "claude" {
                c.provider_session_id = Some(uuid_session_id());
            }
            c.last_request_id = Some(req.id.clone());
            db.save(&c)?;
            Ok((c, fresh))
        })?;
        let mut run = Attempt {
            id: Attempt::key(&req.id, req.attempt),
            request_id: req.id.clone(),
            project_id: req.project_id.clone(),
            card_id: req.card_id.clone(),
            number: req.attempt,
            state: "starting".into(),
            core_id: None,
            terminal: None,
            cwd: cwd.to_string_lossy().into_owned(),
            input_path: input.clone(),
            output_path: None,
            observed: None,
            error: None,
            created_at: now(),
            conversation_id: Some(context.id.clone()),
            provider_session_id: context.provider_session_id.clone(),
            close_state: String::new(),
            turn_ended_at: None,
            response_path: None,
        };
        self.store.lock().unwrap().write(|db| {
            db.save(&run)?;
            for mut q in db
                .list::<Question>()?
                .into_iter()
                .filter(|q| q.request_id == req.id && q.attempt == 0)
            {
                q.attempt = req.attempt;
                db.save(&q)?;
            }
            let mut c: Card = db.get(&card.id)?;
            c.worktree = Some(run.cwd.clone());
            db.save(&c)
        })?;
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let core = client.status()?;
        let env = std::collections::BTreeMap::from([
            (
                "CANBAN_ENDPOINT".into(),
                self.endpoint().to_string_lossy().into_owned(),
            ),
            ("CANBAN_REQUEST_ID".into(), req.id.clone()),
            ("CANBAN_PROVIDER".into(), req.profile.provider_kind().into()),
            // Do not misidentify a child invocation using its launching Agent's ID.
            ("CODEX_THREAD_ID".into(), String::new()),
            ("CANBAN_ATTEMPT".into(), req.attempt.to_string()),
            ("CANBAN_CLI".into(), self.cli.to_string_lossy().into_owned()),
            (
                "CANBAN_HOME".into(),
                self.home.to_string_lossy().into_owned(),
            ),
        ]);
        let labels = std::collections::BTreeMap::from([
            ("canban.project".into(), req.project_id.clone()),
            ("canban.card".into(), req.card_id.clone()),
            ("canban.request".into(), req.id.clone()),
            ("canban.attempt".into(), req.attempt.to_string()),
        ]);
        let prompt = std::fs::read_to_string(self.home.join(input))?;
        let profile = crate::providers::prepare(
            &req.profile,
            context.provider_session_id.as_deref(),
            fresh,
            &self.cli,
            &self.home,
            &canban_context::run_dir(&self.home, req, req.attempt),
        )?;
        let terminal = client.launch(
            &profile,
            &prompt,
            &run.cwd,
            env,
            labels,
            &self.home.to_string_lossy(),
        )?;
        run.core_id = Some(strp(&core["core"], "id")?.into());
        run.terminal = Some(strp(&terminal, "address")?.into());
        let delivery_error=terminal.get("first_message").filter(|v|v["sent"]==false).map(|v|format!("Herdr did not deliver the initial task: {v}. Inspect this terminal before retrying."));
        run.observed = Some(canban_herdr::observation(&terminal));
        run.state = "running".into();
        self.store.lock().unwrap().write(|db|{let current:Attempt=db.get(&run.id)?;if current.state!="starting"{run.state=current.state;}db.save(&run)?;let mut r:Request=db.get(&req.id)?;if r.state=="starting"{r.state="running".into();db.save(&r)?;}let c:Card=db.get(&req.card_id)?;log(db,&c,"execution.started","system",&format!("@{} started",req.profile.id),Some(req.id.clone()),json!({"attempt":req.attempt,"terminal":run.terminal,"core_id":run.core_id,"cwd":run.cwd}))})?;
        self.store.lock().unwrap().write(|db| {
            for q in snapshot
                .questions
                .iter()
                .filter(|q| q.request_id == req.id && q.state == "pending")
            {
                let mut current: Question = db.get(&q.id)?;
                if current.state == "pending" {
                    current.state = "delivered".into();
                    current.attempt = req.attempt;
                    db.save(&current)?;
                }
            }
            // A callback can have bound the provider ID before launch returned.
            context = db.get(&context.id)?;
            Ok(())
        })?;
        self.emit("execution", Some(req.card_id.clone()));
        if let Some(error) = delivery_error {
            bail!("{error}");
        }
        Ok(())
    }
    pub(super) fn launch_failed(&self, req: &Request, error: &str) -> Result<()> {
        self.store.lock().unwrap().write(|db| {
            let mut r: Request = db.get(&req.id)?;
            if matches!(r.state.as_str(), "starting" | "running") {
                r.state = "failed".into();
                db.save(&r)?;
            }
            if let Ok(mut a) = db.get::<Attempt>(&Attempt::key(&req.id, req.attempt))
                && a.state != "completed"
            {
                a.state = if a.terminal.is_none() && error.contains("timed out") {
                    "unknown"
                } else {
                    "failed"
                }
                .into();
                if a.state == "unknown" {
                    r.state = "unknown".into();
                    db.save(&r)?;
                }
                a.error = Some(error.into());
                db.save(&a)?;
            }
            let mut c: Card = db.get(&req.card_id)?;
            attention(db, &mut c, error)?;
            c.revision += 1;
            db.save(&c)?;
            log(
                db,
                &c,
                "execution.failed",
                "system",
                error,
                Some(req.id.clone()),
                json!({}),
            )
        })?;
        self.emit("execution", Some(req.card_id.clone()));
        Ok(())
    }
}
