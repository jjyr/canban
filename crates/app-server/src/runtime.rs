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

/// A bare turn change includes interruptions and permission dialogs. Require a
/// later root `done` for this invocation, never an old idle/completed screen.
pub(super) fn completion_ready(run: &Attempt, value: &Value) -> bool {
    value["state"] != "finished"
        && value["status"]["state"] == "done"
        && value["turn"]
            .as_u64()
            .is_some_and(|t| t > run.completion_turn.unwrap_or(u64::MAX))
        && run
            .completed_at
            .is_some_and(|at| value["status_ms"].as_u64().is_some_and(|t| t >= at))
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
        // Verify retained statuses again after restart; never reclaim immediately from old idle time.
        self.store.lock().unwrap().write(|db| {
            for mut a in db
                .list::<Attempt>()?
                .into_iter()
                .filter(|a| !a.finished() && a.terminal.is_some())
            {
                a.connection_state = "checking".into();
                a.lifecycle.observed_at = None;
                a.lifecycle.idle_since = None;
                db.save(&a)?;
            }
            Ok(())
        })?;
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
            let core = match client.status() {
                Ok(core) => core,
                Err(e) => {
                    for run in &bound {
                        self.connection_lost(
                            run,
                            "unavailable",
                            &format!("Herdr unavailable: {e:#}"),
                        )?;
                    }
                    return Ok(());
                }
            };
            for run in bound {
                if run.core_id.as_deref() != core["core"]["id"].as_str() {
                    self.connection_lost(&run,"replaced","Original Herdr core identity changed; inspect the original runtime before starting a replacement")?;
                    continue;
                }
                let value = match client.get(run.terminal.as_ref().unwrap()) {
                    Ok(value) => value,
                    Err(e) => {
                        self.connection_lost(
                            &run,
                            "unavailable",
                            &format!("Original terminal unavailable: {e:#}"),
                        )?;
                        continue;
                    }
                };
                let finished = value["state"] == "finished";
                if finished && run.output_path.is_none() {
                    let _ = self.capture_output(&run);
                }
                let observed = canban_herdr::observation(&value);
                self.observe_lifecycle(&run, &observed)?;
                if run.observed.as_ref() != Some(&observed)
                    || run.connection_state != "connected"
                    || (finished && run.close_state != "closed")
                {
                    self.store.lock().unwrap().write(|db|{
                        let mut current:Attempt=db.get(&run.id)?;
                        current.observed=Some(observed.clone());
                        if matches!(current.connection_state.as_str(),"unavailable"|"replaced") {
                            log(db,&db.get::<Card>(&run.card_id)?,"execution.reconnected","system","Reconnected to the original terminal",Some(run.request_id.clone()),json!({"terminal":run.terminal}))?;
                        }
                        current.connection_state="connected".into();
                        if current.close_state=="waiting" && completion_ready(&current,&observed) {
                            current.close_state="pending".into();current.turn_ended_at=Some(now());
                        }
                        if finished {
                            current.close_state="closed".into();
                            if current.lifecycle.close_reason == "idle" {
                                current.state="idle_closed".into();
                                log(db,&db.get::<Card>(&run.card_id)?,"execution.idle_closed","system","Idle Agent exited; exact conversation retained for resume",Some(run.request_id.clone()),json!({"attempt":run.number}))?;
                                let mut req:Request=db.get(&run.request_id)?;
                                if req.attempt==run.number && req.state!="completed" { req.state="idle_closed".into();db.save(&req)?; }
                            } else if matches!(current.state.as_str(),"running"|"working"|"starting") {
                                current.state="needs_report".into();
                                let mut req:Request=db.get(&run.request_id)?;
                                if req.attempt==run.number && req.state!="completed" {req.state="needs_report".into();db.save(&req)?;}
                                let mut card:Card=db.get(&run.card_id)?;
                                attention(db,&mut card,"Agent exited without an explicit outcome. Inspect the retained output before resuming.")?;
                                if let Some(gate)=&mut card.human_gate && gate.request_id.is_none() { gate.request_id=Some(req.id.clone()); }
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
            for mut run in db.list::<Attempt>()?.into_iter().filter(|a| a.close_state == "closing" && a.close_requested_at.is_some_and(|t| now().saturating_sub(t) > 30_000)) {
                run.close_state = "error".into();
                run.error = Some("Agent did not exit after the normal exit request. Inspect its terminal or use End session.".into());
                let mut card: Card = db.get(&run.card_id)?;
                if run.lifecycle.close_reason == "idle" {
                    db.save(&canban_notifications::attention(&card.project_id,&card.id,run.error.as_deref().unwrap()))?;
                    log(db,&card,"execution.idle_close_failed","system",run.error.as_deref().unwrap(),Some(run.request_id.clone()),json!({"attempt":run.number}))?;
                } else {
                    attention(db, &mut card, run.error.as_deref().unwrap())?;
                    card.revision += 1;
                    db.save(&card)?;
                }
                db.save(&run)?;
            }
            Ok(())
        })?;
        self.store.lock().unwrap().write(|db| {
            for mut run in db.list::<Attempt>()?.into_iter().filter(|a|a.close_state=="waiting" && a.completed_at.is_some_and(|t|now().saturating_sub(t)>60_000) && a.observed.as_ref().is_none_or(|v|v["status"]["state"].as_str().is_none_or(|s|s.is_empty()||s=="unknown"))) {
                run.close_state="manual".into();
                run.error=Some("Completion was recorded, but Herdr supplied no usable Agent status. Automatic exit is disabled; use the terminal or End session.".into());
                let mut card:Card=db.get(&run.card_id)?;attention(db,&mut card,run.error.as_deref().unwrap())?;card.revision+=1;db.save(&card)?;db.save(&run)?;
            }
            Ok(())
        })?;
        self.deliver_questions(&client)?;
        self.close_completed(&client)?;
        self.close_idle(&client)?;
        let snapshot = self.snapshot()?;
        let request = canban_scheduler::next(&snapshot, self.config().scheduler.max_agents);
        if let Some(request) = request {
            let req = self.store.lock().unwrap().write(|db| {
                let mut req: Request = db.get(&request.id)?;
                let card: Card = db.get(&req.card_id)?;
                let current = db.snapshot()?;
                if canban_scheduler::active_count(&current) >= self.config().scheduler.max_agents
                    || !canban_scheduler::eligible(&current, &req)
                {
                    return Ok(None);
                }
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
    fn connection_lost(&self, run: &Attempt, state: &str, message: &str) -> Result<()> {
        if run.connection_state == state {
            return Ok(());
        }
        self.store.lock().unwrap().write(|db| {
            let mut current: Attempt = db.get(&run.id)?;
            if current.connection_state == state {
                return Ok(());
            }
            current.connection_state = state.into();
            db.save(&current)?;
            let mut card: Card = db.get(&run.card_id)?;
            attention(db, &mut card, message)?;
            card.revision += 1;
            db.save(&card)?;
            log(
                db,
                &card,
                "execution.disconnected",
                "system",
                message,
                Some(run.request_id.clone()),
                json!({"terminal":run.terminal,"connection_state":state}),
            )
        })?;
        self.emit("runtime", Some(run.card_id.clone()));
        Ok(())
    }
    pub(super) fn deliver_questions(&self, client: &canban_herdr::Client) -> Result<()> {
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
            if matches!(req.state.as_str(), "completed" | "idle_closed")
                && question.state == "pending"
            {
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
            if run.runtime_status() == "blocked" || run.connection_state != "connected" {
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
            let result = (|| -> Result<Value> {
                ensure!(
                    client.status()?["core"]["id"].as_str() == run.core_id.as_deref(),
                    "Original core identity changed"
                );
                client.call("terminal.send", json!({"terminal":terminal,"text":text}))
            })();
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
            let request: Request = self.store.lock().unwrap().db().get(&run.request_id)?;
            let Some(ExitAction::Input { text }) = &request.profile.exit else {
                continue;
            };
            ensure!(
                client.status()?["core"]["id"].as_str() == run.core_id.as_deref(),
                "Original Herdr core is unavailable"
            );
            let actual = client.get(terminal)?;
            if !completion_ready(&run, &actual) {
                continue;
            }
            self.store.lock().unwrap().write(|db| {
                let mut a: Attempt = db.get(&run.id)?;
                a.close_state = "closing".into();
                a.close_requested_at = Some(now());
                db.save(&a)
            })?;
            let result = (|| {
                ensure!(
                    client.status()?["core"]["id"].as_str() == run.core_id.as_deref(),
                    "Original Herdr core is unavailable"
                );
                client.call("terminal.send", json!({"terminal":terminal,"text":text}))
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
        let mut prepared = card.clone();
        prepared.worktree = Some(cwd.to_string_lossy().into_owned());
        let info = canban_workspaces::info(project, &prepared)?;
        self.store.lock().unwrap().write(|db| {
            let mut current: Card = db.get(&card.id)?;
            let changed = current.worktree != prepared.worktree || current.branch != info.branch;
            current.worktree = prepared.worktree.clone();
            current.branch = info.branch.clone();
            if current.base_commit.is_none() {
                current.base_commit = info.base_commit.clone();
            }
            if changed {
                log(db, &current, "workspace.prepared", "system", "Card branch and workspace prepared", Some(req.id.clone()), json!({"worktree":current.worktree,"branch":current.branch,"base_commit":current.base_commit}))?;
            }
            db.save(&current)
        })?;
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
            if fresh
                && req
                    .profile
                    .command()
                    .iter()
                    .any(|a| a.contains("{session_id}"))
            {
                c.provider_session_id = Some(uuid_session_id());
            }
            c.last_request_id = Some(req.id.clone());
            db.save(&c)?;
            Ok((c, fresh))
        })?;
        let mut run = Attempt {
            lifecycle: Lifecycle::default(),
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
            completed_at: None,
            completion_turn: None,
            close_requested_at: None,
            connection_state: String::new(),
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
            (
                "CANBAN_RUN_DIR".into(),
                canban_context::run_dir(&self.home, req, req.attempt)
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "CANBAN_SESSION_ID".into(),
                context.provider_session_id.clone().unwrap_or_default(),
            ),
            // Do not misidentify a child invocation using its launching Agent's ID.
            ("CANBAN_PROFILE_ID".into(), req.profile.id.clone()),
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
        let argv = canban_profiles::command(
            &req.profile,
            context.provider_session_id.as_deref(),
            fresh,
            &[
                ("home", &self.home.to_string_lossy()),
                ("prompt", &prompt),
                ("cli", &self.cli.to_string_lossy()),
            ],
        )?;
        let terminal = client.launch(argv, &run.cwd, env, labels)?;
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
            if let Some(gate) = &mut c.human_gate
                && gate.request_id.is_none()
            {
                gate.request_id = Some(req.id.clone());
            }
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
