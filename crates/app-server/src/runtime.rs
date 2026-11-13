use super::*;
pub(super) fn uuid_session_id() -> String {
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
        // The lock is held by the accept loop, including socket removal.
        let lock = self._service_lock.try_clone()?;
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
                if matches!(a.close_state.as_str(), "waiting" | "pending" | "manual") {
                    a.close_state.clear();
                    a.turn_ended_at = None;
                }
                a.connection_state = "checking".into();
                a.lifecycle.observed_at = None;
                a.lifecycle.idle_since = None;
                db.save(&a)?;
            }
            Ok(())
        })?;
        self.store.lock().unwrap().write(|db| {
            for mut item in db.list::<TerminalEntry>()? {
                if let Some(TerminalRuntime {state,connection,error,..})=&mut item.runtime && state=="starting" {
                    *state="unknown".into(); *connection="unavailable".into(); *error=Some("AppServer restarted before the launch receipt was saved; inspect Herdr before opening another terminal.".into()); db.save(&item)?;
                }
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
            drop(lock);
        });
        self.migrate_exit_failures()?;
        self.start_runtime();
        self.start_update_checks();
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            let mut next_sync = std::time::Instant::now();
            while let Some(server) = weak.upgrade() {
                if server.stopped() {
                    break;
                }
                if canban_updater::handoff::active(&server.home) {
                    drop(server);
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                }
                if let Err(e) = server.github.poll() {
                    eprintln!("Canban GitHub sign-in: {e}");
                }
                if std::time::Instant::now() >= next_sync
                    || server.github_sync_due.swap(false, Ordering::SeqCst)
                {
                    next_sync = std::time::Instant::now() + Duration::from_secs(120);
                    let projects = server
                        .store
                        .lock()
                        .unwrap()
                        .db()
                        .list::<Project>()
                        .unwrap_or_default();
                    server.github_refresh_queries();
                    for project in projects.into_iter().filter(|p| p.github.is_some()) {
                        let _ = server.github_sync(&project.id);
                    }
                    let cards = server
                        .store
                        .lock()
                        .unwrap()
                        .db()
                        .list::<Card>()
                        .unwrap_or_default();
                    for c in cards.into_iter().filter(|c| {
                        c.active() && c.pull_request.as_ref().is_some_and(|pr| !pr.merged)
                    }) {
                        let _ = server.github_pr_refresh(json!({"card_id":c.id}));
                    }
                }
                drop(server);
                std::thread::sleep(Duration::from_secs(1));
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
        if input["method"] == "herdr.connect" {
            return self.relay(stream, &input["params"]);
        }
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
        if canban_updater::handoff::active(&self.home) {
            return Ok(());
        }

        if self.remote_health_due.swap(false, Ordering::SeqCst)
            && let Err(e) = self.machines()
        {
            self.inventory.lock().unwrap().remote_error = Some(format!("{e:#}"));
        }
        self.store
            .lock()
            .unwrap()
            .write(|db| events::pump(db, &self.config()))?;
        let _ = self.pump_deliveries();
        let _ = self.pump_merged_archives();
        self.observe_terminals()?;
        self.pump_configuration()?;
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let snapshot = self.snapshot()?;
        let bound: Vec<_> = snapshot
            .attempts
            .iter()
            .filter(|a| {
                a.state != "continued"
                    && a.terminal.is_some()
                    && (!a.finished()
                        || a.observed
                            .as_ref()
                            .is_some_and(|v| v["state"] == "finished")
                            && a.output_path.is_none())
            })
            .cloned()
            .collect();
        if !bound.is_empty() {
            let (core_id, values) = match self.runtime_inventory() {
                Ok(value) => value,
                Err(e) => {
                    for run in &bound {
                        self.connection_lost(
                            run,
                            "unavailable",
                            &format!("Herdr unavailable: {e:#}"),
                        )?;
                    }
                    (String::new(), Vec::new())
                }
            };
            for run in bound {
                if core_id.is_empty() {
                    continue;
                }
                if run.core_id.as_deref() != Some(core_id.as_str()) {
                    self.connection_lost(&run,"replaced","Original Herdr core identity changed; inspect the original runtime before starting a replacement")?;
                    continue;
                }
                if !self.remote_connected(run.terminal.as_deref().unwrap_or("")) {
                    self.connection_lost(
                        &run,
                        "unavailable",
                        "Remote machine connection is unavailable; exit is unconfirmed",
                    )?;
                    continue;
                }
                let Some(value) = values
                    .iter()
                    .find(|v| v["address"].as_str() == run.terminal.as_deref())
                else {
                    self.runtime_missing(&run)?;
                    continue;
                };
                let finished = value["state"] == "finished";
                if finished && run.output_path.is_none() {
                    let _ = self.capture_output(&run);
                }
                let observed = canban_herdr::observation(value);
                self.observe_lifecycle(&run, &observed)?;
                if run.observed.as_ref() != Some(&observed)
                    || run.connection_state != "connected"
                    || (finished && run.close_state != "closed")
                {
                    self.store.lock().unwrap().write(|db| {
                        let mut current: Attempt = db.get(&run.id)?;
                        current.observed = Some(observed.clone());
                        if matches!(
                            current.connection_state.as_str(),
                            "unavailable" | "replaced"
                        ) {
                            log(
                                db,
                                &db.get::<Card>(&run.card_id)?,
                                "execution.reconnected",
                                "system",
                                "Reconnected to the original terminal",
                                Some(run.request_id.clone()),
                                json!({"terminal":run.terminal}),
                            )?;
                        }
                        current.connection_state = "connected".into();
                        if finished {
                            current.close_state = "closed".into();
                            if current.lifecycle.close_reason == "idle" {
                                current.state = "idle_closed".into();
                                let closed_msg = if current.provider_session_id.is_some() {
                                    "Idle Agent exited; exact conversation retained for resume"
                                } else {
                                    "Idle Agent terminal closed; Card history retained"
                                };
                                log(
                                    db,
                                    &db.get::<Card>(&run.card_id)?,
                                    "execution.idle_closed",
                                    "system",
                                    closed_msg,
                                    Some(run.request_id.clone()),
                                    json!({"attempt":run.number}),
                                )?;
                                let mut req: Request = db.get(&run.request_id)?;
                                if req.attempt == run.number
                                    && !matches!(req.state.as_str(), "completed" | "backlog")
                                {
                                    req.state = "idle_closed".into();
                                    db.save(&req)?;
                                }
                            } else if matches!(
                                current.state.as_str(),
                                "running" | "working" | "starting" | "needs_report"
                            ) {
                                current.state = "failed".into();
                                current.error =
                                    Some("Agent exited without reporting an outcome".into());
                                let mut req: Request = db.get(&run.request_id)?;
                                if req.attempt == run.number
                                    && !matches!(req.state.as_str(), "completed" | "backlog")
                                {
                                    req.state = "failed".into();
                                    db.save(&req)?;
                                }
                                log(
                                    db,
                                    &db.get::<Card>(&run.card_id)?,
                                    "execution.failed",
                                    "system",
                                    current.error.as_deref().unwrap(),
                                    Some(req.id),
                                    json!({"attempt":run.number,"exit_code":observed["exit_code"]}),
                                )?;
                            }
                        }
                        db.save(&current)
                    })?;
                    let agent_stopped = finished
                        || matches!(
                            observed["status"]["state"].as_str(),
                            Some("idle" | "done" | "blocked" | "error")
                        )
                        || observed["status"]["state"].is_null();
                    if agent_stopped {
                        let _ = self.pump_deliveries();
                    }
                    if finished {
                        let _ = self.try_auto_archive_merged(&run.card_id);
                    }
                    self.emit("runtime", Some(run.card_id.clone()));
                }
            }
        }
        let timed_out_closing: Vec<Attempt> = self
            .store
            .lock()
            .unwrap()
            .db()
            .list::<Attempt>()?
            .into_iter()
            .filter(|a| {
                a.close_state == "closing"
                    && a.close_requested_at
                        .is_some_and(|t| now().saturating_sub(t) > 30_000)
            })
            .collect();
        for run in timed_out_closing {
            if run.lifecycle.close_reason == "idle" {
                let fallback = (|| -> Result<bool> {
                    let Some(terminal) = run.terminal.as_deref() else {
                        return Ok(false);
                    };
                    if client.status()?["core"]["id"].as_str() != run.core_id.as_deref() {
                        return Ok(false);
                    }
                    let actual = client.get(terminal)?;
                    if actual["state"] == "finished" {
                        return Ok(true);
                    }
                    if actual["state"] != "live"
                        || !matches!(actual["status"]["state"].as_str(), Some("idle" | "done"))
                    {
                        return Ok(false);
                    }
                    let _ = self.capture_output(&run);
                    client.call("terminal.kill", json!({"terminal": terminal}))?;
                    Ok(true)
                })();
                match fallback {
                    Ok(true) => {
                        self.store.lock().unwrap().write(|db| {
                            let mut current: Attempt = db.get(&run.id)?;
                            if current.close_state == "closing" {
                                current.close_requested_at = Some(now());
                                db.save(&current)?;
                            }
                            Ok(())
                        })?;
                        continue;
                    }
                    Ok(false) => {
                        // Agent resumed working or Core changed while closing; cancel the idle close cleanly.
                        self.store.lock().unwrap().write(|db| {
                            let mut current: Attempt = db.get(&run.id)?;
                            if current.close_state == "closing" {
                                current.close_state.clear();
                                current.close_requested_at = None;
                                current.lifecycle.close_reason.clear();
                                db.save(&current)?;
                            }
                            Ok(())
                        })?;
                        continue;
                    }
                    Err(_) => {}
                }
            }
            self.store.lock().unwrap().write(|db| {
                let mut current: Attempt = db.get(&run.id)?;
                if current.close_state != "closing" {
                    return Ok(());
                }
                current.close_state = "error".into();
                current.error = Some(
                    "Agent did not exit after the normal exit request. Inspect its terminal or use End session."
                        .into(),
                );
                let mut card: Card = db.get(&current.card_id)?;
                if current.lifecycle.close_reason == "idle" {
                    db.save(&canban_notifications::attention(
                        &card.project_id,
                        &card.id,
                        current.error.as_deref().unwrap(),
                    ))?;
                    log(
                        db,
                        &card,
                        "execution.idle_close_failed",
                        "system",
                        current.error.as_deref().unwrap(),
                        Some(current.request_id.clone()),
                        json!({"attempt":current.number}),
                    )?;
                } else {
                    attention(db, &mut card, current.error.as_deref().unwrap())?;
                    card.revision += 1;
                    db.save(&card)?;
                }
                db.save(&current)
            })?;
        }
        self.deliver_questions(&client)?;
        self.close_idle(&client)?;
        let _ = self.pump_admissions();
        let snapshot = self.snapshot()?;
        let mut available = snapshot.clone();
        if snapshot
            .projects
            .iter()
            .any(|p| p.work_location().machine != "local")
            || snapshot.requests.iter().any(|r| r.resume_from.is_some())
        {
            let queued_remote =
                canban_scheduler::next(&snapshot, self.config().scheduler.max_agents).is_some_and(
                    |r| {
                        let machine = r
                            .resume_from
                            .as_ref()
                            .and_then(|id| snapshot.attempts.iter().find(|a| &a.id == id))
                            .and_then(|a| a.location.as_ref())
                            .map(|l| l.machine.clone())
                            .or_else(|| {
                                snapshot
                                    .projects
                                    .iter()
                                    .find(|p| p.id == r.project_id)
                                    .map(|p| p.work_location().machine)
                            });
                        machine.is_some_and(|m| m != "local")
                    },
                );
            let catalog = if queued_remote {
                self.machines().ok()
            } else {
                Some(self.inventory.lock().unwrap().machines.clone())
            };
            available.requests.retain(|r| {
                let machine = r
                    .resume_from
                    .as_ref()
                    .and_then(|id| snapshot.attempts.iter().find(|a| &a.id == id))
                    .and_then(|a| a.location.as_ref())
                    .map(|l| l.machine.clone())
                    .or_else(|| {
                        snapshot
                            .projects
                            .iter()
                            .find(|p| p.id == r.project_id)
                            .map(|p| p.work_location().machine)
                    })
                    .unwrap_or_else(|| "local".into());
                machine == "local"
                    || catalog
                        .as_ref()
                        .and_then(|v| v["machines"].as_array())
                        .is_some_and(|ms| {
                            ms.iter()
                                .any(|m| m["id"] == machine && m["state"] == "connected")
                        })
            });
        }
        if self.observer_running.load(Ordering::SeqCst) && self.runtime_inventory().is_err() {
            return Ok(());
        }
        let request = canban_scheduler::next(&available, self.config().scheduler.max_agents);
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
                if !card.active() {
                    req.state = "cancelled".into();
                    db.save(&req)?;
                    return Ok(None);
                }
                let project: Project = db.get(&req.project_id)?;
                req.location = Some(if let Some(id) = &req.resume_from {
                    db.get::<Attempt>(id)?
                        .location
                        .context("Resume location missing")?
                } else {
                    project.work_location()
                });
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
    pub(super) fn connection_lost(&self, run: &Attempt, state: &str, message: &str) -> Result<()> {
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
            let card: Card = db.get(&run.card_id)?;
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
            if matches!(req.state.as_str(), "completed" | "backlog" | "idle_closed")
                && !snapshot.attempts.iter().any(|a| {
                    a.request_id == req.id
                        && a.number == req.attempt
                        && a.terminal.is_some()
                        && !a.finished()
                })
                && question.state == "pending"
            {
                // A question that raced task completion follows the same conversation
                // after close. It is never silently redirected to the Reviewer.
                let config = self.config();
                self.store.lock().unwrap().write(|db| {
                    let mut q: Question = db.get(&question.id)?;
                    if q.state == "replied" {
                        return Ok(());
                    }
                    let mut card: Card = db.get(&q.card_id)?;
                    if !card.active() {
                        return Ok(());
                    }
                    let project: Project = db.get(&card.project_id)?;
                    let stage = canban_workflow_engine::stage(&project.workflow, &card.stage)?;
                    let instruction = canban_context::render_template(
                        include_str!(concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/../../prompts/runtime/comment_task.txt"
                        ))
                        .trim_end(),
                        &[("body", &q.body)],
                    );
                    let request = enqueue(
                        db,
                        &config,
                        &mut card,
                        &project,
                        stage,
                        RequestTrigger::Comment {
                            comment_id: q.comment_id.clone(),
                        },
                        Some(&q.profile_id),
                        Some(instruction),
                    )?
                    .context("Could not retain question")?;
                    q.request_id = request.id;
                    q.attempt = 0;
                    q.state = "pending".into();
                    db.save(&q)
                })?;
                continue;
            }
            if question.state != "pending"
                || !matches!(
                    req.state.as_str(),
                    "running" | "working" | "blocked" | "completed"
                )
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
            let qid_quoted = canban_context::shell_quote(&question.id);
            let run_number = run.number.to_string();
            let text = canban_context::render_template(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../prompts/runtime/live_question_delivery.txt"
                ))
                .trim_end(),
                &[
                    ("question_id", &question.id),
                    ("question_body", &question.body),
                    ("question_id_quoted", &qid_quoted),
                    ("request_id", &req.id),
                    ("attempt", &run_number),
                ],
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
    pub(super) fn launch(&self, req: &Request, snapshot: &Snapshot) -> Result<()> {
        let _gate = self.terminal_gate.lock().unwrap();
        ensure!(
            self.store
                .lock()
                .unwrap()
                .db()
                .get::<Card>(&req.card_id)?
                .active(),
            "Card was archived before launch"
        );
        if let Some(old) = canban_scheduler::retained(snapshot, req) {
            return self.continue_assignment(req, old, snapshot);
        }
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
        let location = req
            .location
            .clone()
            .context("Execution location was not admitted")?;
        self.resolve_machine(&location.machine, true)?;
        let ops = self.ops(&location.machine);
        let remote = location.machine != "local";
        let prior = req
            .resume_from
            .as_ref()
            .and_then(|id| snapshot.attempts.iter().find(|a| &a.id == id));
        let home = prior
            .and_then(|a| a.runtime_home.as_ref())
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(|| self.execution_home(&location.machine))?;
        let mut project = project.clone();
        if let Some(id) = &req.workflow_run_id {
            project.workflow.workspace = self
                .store
                .lock()
                .unwrap()
                .db()
                .get::<WorkflowRun>(id)?
                .definition
                .workspace;
        }
        project.directory = location.directory.clone();
        project.location = Some(location.clone());
        let mut prepared = card.clone();
        if let Some(old) = prior {
            prepared.worktree = Some(old.cwd.clone());
            if let Some(w) = &old.workspace {
                prepared.branch = w.branch.clone();
                prepared.base_commit = w.base_commit.clone();
                project.target_branch = w.target_branch.clone();
            }
        } else if prepared
            .workspace
            .as_ref()
            .is_some_and(|w| w.location != location)
        {
            ensure!(
                card.delivery.is_some(),
                "This Card has undelivered work in its previous environment. Accept/hand off that work before starting a stage in the new location; Resume still uses the original environment."
            );
            prepared.worktree = None;
            prepared.branch = None;
            prepared.base_commit = None;
        }
        let cwd = if prior.is_some() {
            ops.resume(&home, &project, &prepared)?
        } else {
            ops.prepare(&home, &project, &prepared)?
        };
        self.git_changed(&location.machine);
        prepared.worktree = Some(cwd.to_string_lossy().into_owned());
        let info = ops.info(&project, &prepared)?;
        let workspace = WorkspaceBinding {
            location: location.clone(),
            path: cwd.to_string_lossy().into_owned(),
            branch: info.branch.clone(),
            base_commit: info.base_commit.clone(),
            target_branch: project.target_branch.clone(),
        };
        if prior.is_none()
            || card
                .workspace
                .as_ref()
                .is_none_or(|w| w.location == location)
        {
            self.store.lock().unwrap().write(|db| {
                let mut current: Card = db.get(&card.id)?;
                if current.workspace.as_ref() != Some(&workspace) {
                    if let Some(old) = current.workspace.replace(workspace.clone())
                        && old.location != workspace.location
                    {
                        current.workspace_history.push(old);
                        current.delivery = None;
                    }
                    current.worktree = Some(workspace.path.clone());
                    current.branch = workspace.branch.clone();
                    current.base_commit = workspace.base_commit.clone();
                    log(
                        db,
                        &current,
                        "workspace.prepared",
                        "system",
                        "Card workspace prepared",
                        Some(req.id.clone()),
                        json!({"workspace":workspace}),
                    )?;
                    db.save(&current)?;
                }
                Ok(())
            })?;
        }
        let endpoint = if remote {
            format!("spool:{}", home.join("rpc").display())
        } else {
            self.endpoint().to_string_lossy().into_owned()
        };
        let cli = if remote {
            home.join("bin/canban")
        } else {
            self.cli.clone()
        };
        let input = canban_context::capture_for(
            &self.home,
            &project,
            &prepared,
            req,
            req.attempt,
            snapshot,
            &endpoint,
            &cli,
            &home,
        )?;
        if remote {
            self.stage_remote(&ops.host, &home, card)?;
        }
        let (mut context, fresh) = self.store.lock().unwrap().write(|db| {
            let mut c = crate::sessions::conversation(db, card, &req.profile)?;
            if let Some(old) = prior {
                c.provider_session_id = old.provider_session_id.clone();
            } else if c
                .last_request_id
                .as_ref()
                .and_then(|id| snapshot.requests.iter().find(|r| &r.id == id))
                .and_then(|r| {
                    snapshot
                        .attempts
                        .iter()
                        .find(|a| a.request_id == r.id && a.number == r.attempt)
                })
                .is_some_and(|a| a.location.as_ref() != Some(&location))
            {
                c.provider_session_id = None;
            }
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
            runtime_home: Some(home.to_string_lossy().into_owned()),
            workspace: Some(workspace),
            location: req.location.clone(),
            owner_core_id: None,
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
            Ok(())
        })?;
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let core = client.status()?;
        let env = std::collections::BTreeMap::from([
            ("CANBAN_ENDPOINT".into(), endpoint.clone()),
            ("CANBAN_REQUEST_ID".into(), req.id.clone()),
            (
                "CANBAN_RUN_DIR".into(),
                canban_context::run_dir(&home, req, req.attempt)
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
            ("CANBAN_CLI".into(), cli.to_string_lossy().into_owned()),
            ("CANBAN_HOME".into(), home.to_string_lossy().into_owned()),
        ]);
        let labels = std::collections::BTreeMap::from([
            ("canban.project".into(), req.project_id.clone()),
            ("canban.card".into(), req.card_id.clone()),
            ("canban.request".into(), req.id.clone()),
            ("canban.attempt".into(), req.attempt.to_string()),
        ]);
        let prompt = std::fs::read_to_string(self.home.join(input))?;
        let mut argv = canban_profiles::command(
            &req.profile,
            context.provider_session_id.as_deref(),
            fresh,
            &[
                ("home", &home.to_string_lossy()),
                ("cwd", &run.cwd),
                ("project_dir", &project.directory),
                ("prompt", &prompt),
                ("cli", &cli.to_string_lossy()),
            ],
        )?;
        let is_herdr_template = req
            .profile
            .preset
            .as_deref()
            .is_some_and(|s| s.starts_with("herdr:"));
        if !is_herdr_template {
            if !remote {
                argv[0] = canban_profiles::executable(&argv[0]);
            } else if Path::new(&argv[0]).components().count() == 1 {
                let program=ops.host.python("import os,shutil,json,sys;h=os.path.expanduser('~');p=os.environ.get('PATH','')+os.pathsep+os.path.join(h,'.local/bin')+os.pathsep+os.path.join(h,'.cargo/bin');print(json.dumps(shutil.which(sys.argv[1],path=p) or sys.argv[1]))",&[&argv[0]])?;
                argv[0] = program.as_str().context("No remote executable")?.into();
            }
        }
        let terminal = client.launch_on(&location.machine, argv, &run.cwd, env, labels)?;
        let core_id = strp(&core["core"], "id")?.to_owned();
        let terminal_address = strp(&terminal, "address")?.to_owned();
        run.owner_core_id = if remote {
            Some(location.machine.clone())
        } else {
            Some(core_id.clone())
        };
        run.core_id = Some(core_id.clone());
        run.terminal = Some(terminal_address.clone());
        let delivery_error=terminal.get("first_message").filter(|v|v["sent"]==false).map(|v|format!("Herdr did not deliver the initial task: {v}. Inspect this terminal before retrying."));
        run.observed = Some(canban_herdr::observation(&terminal));
        run.lifecycle.observed_at = Some(now());
        run.connection_state = "connected".into();
        run.state = "running".into();
        // Seed the in-memory inventory immediately so a runtime tick racing ahead
        // of Herdr's async "created" subscription event does not transiently mark
        // the freshly launched terminal missing/unavailable.
        {
            let mut inv = self.inventory.lock().unwrap();
            if inv.core == core_id {
                let key = inv.address(&terminal_address);
                inv.records.entry(key.clone()).or_insert_with(|| {
                    let mut seeded = terminal.clone();
                    seeded["address"] = json!(key);
                    if seeded["state"].is_null() {
                        seeded["state"] = json!("live");
                    }
                    if seeded["cwd"].is_null() {
                        seeded["cwd"] = json!(run.cwd);
                    }
                    seeded
                });
            }
        }
        self.store.lock().unwrap().write(|db| {
            let current: Attempt = db.get(&run.id)?;
            if current.state != "starting" {
                run.state = current.state;
                run.completed_at = current.completed_at;
                run.completion_turn = current.completion_turn;
                run.close_state = current.close_state;
                run.turn_ended_at = current.turn_ended_at;
                run.error = current.error;
            }
            if current.provider_session_id.is_some() {
                run.provider_session_id = current.provider_session_id;
            }
            if current.response_path.is_some() {
                run.response_path = current.response_path;
            }
            db.save(&run)?;
            let mut r: Request = db.get(&req.id)?;
            if r.state == "starting" {
                r.state = "running".into();
                db.save(&r)?;
            }
            let c: Card = db.get(&req.card_id)?;
            log(
                db,
                &c,
                "execution.started",
                "system",
                &format!("@{} started", req.profile.id),
                Some(req.id.clone()),
                json!({"attempt":req.attempt,"terminal":run.terminal,"core_id":run.core_id,"owner_core_id":run.owner_core_id,"location":run.location,"cwd":run.cwd}),
            )
        })?;
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
                && !matches!(a.state.as_str(), "completed" | "backlog")
            {
                a.state = if a.state == "unknown"
                    || a.terminal.is_none() && error.contains("timed out")
                {
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
            if r.state == "unknown" || error.contains("undelivered work") {
                attention(db, &mut c, error)?;
                if let Some(gate) = &mut c.human_gate
                    && gate.request_id.is_none()
                {
                    gate.request_id = Some(req.id.clone());
                }
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
