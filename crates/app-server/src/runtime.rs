use super::*;

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
        self.store.lock().unwrap().write(|db|{for mut req in db.list::<Request>()?.into_iter().filter(|r|r.state=="starting"){let run=db.get::<Attempt>(&Attempt::key(&req.id,req.attempt)).ok();req.state=if run.as_ref().and_then(|a|a.terminal.as_ref()).is_some(){"running"}else{"unknown"}.into();db.save(&req)?;if req.state=="unknown"{let mut card:Card=db.get(&req.card_id)?;attention(db,&mut card,"Launch interrupted before its receipt was saved. Inspect Herdr before explicitly retrying.")?;card.revision+=1;db.save(&card)?;}}Ok(())})?;
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
        let config = self.config();
        let snapshot = self.snapshot()?;
        if let Some(request) = snapshot.requests.iter().find(|r| {
            r.state == "queued" && canban_scheduler::ready(&config, r, &snapshot.requests)
        }) {
            let id = request.id.clone();
            let req = self.store.lock().unwrap().write(|db| {
                let mut r: Request = db.get(&id)?;
                let c: Card = db.get(&r.card_id)?;
                if c.archived_at.is_some() {
                    r.state = "cancelled".into();
                    db.save(&r)?;
                    return Ok(None);
                }
                r.state = "starting".into();
                r.attempt += 1;
                db.save(&r)?;
                Ok(Some(r))
            })?;
            if let Some(req) = req {
                let result = self.launch(&req, &snapshot);
                if let Err(e) = result {
                    self.launch_failed(&req, &format!("{e:#}"))?;
                }
            }
        }
        // Runtime observation is presentation/reconciliation, never completion inference.
        for run in snapshot.attempts.iter().filter(|a| {
            matches!(a.state.as_str(), "starting" | "running" | "working")
                || (a.state == "completed" && a.output_path.is_none() && a.error.is_none())
        }) {
            if let Some(terminal) = &run.terminal {
                let client = canban_herdr::Client {
                    config: config.clone(),
                };
                let status = client.status()?;
                if run.core_id.as_deref() != status["core"]["id"].as_str() {
                    if run.state == "completed" {
                        self.store.lock().unwrap().write(|db| {
                            let mut a: Attempt = db.get(&run.id)?;
                            a.error =
                                Some("Output unavailable: original core identity changed".into());
                            db.save(&a)
                        })?;
                    } else {
                        self.launch_failed(
                            snapshot
                                .requests
                                .iter()
                                .find(|r| r.id == run.request_id)
                                .context("Request missing")?,
                            "Herdr core identity changed. Original execution cannot be resolved.",
                        )?;
                    }
                    continue;
                }
                if let Ok(value) = client.get(terminal) {
                    if (value["state"] == "finished" || run.state == "completed")
                        && run.output_path.is_none()
                        && let Err(e) = self.capture_output(run)
                        && run.state == "completed"
                    {
                        self.store.lock().unwrap().write(|db| {
                            let mut a: Attempt = db.get(&run.id)?;
                            a.error = Some(format!("Output capture unavailable: {e:#}"));
                            db.save(&a)
                        })?;
                    }
                    let value = canban_herdr::observation(&value);
                    if value != run.observed.clone().unwrap_or(Value::Null) {
                        self.store.lock().unwrap().write(|db|{let mut a:Attempt=db.get(&run.id)?;a.observed=Some(value.clone());db.save(&a)?;
                        if value["state"]=="finished"&&matches!(a.state.as_str(),"running"|"working"|"starting"){let mut c:Card=db.get(&a.card_id)?;attention(db,&mut c,"Agent exited without an explicit Canban outcome. Inspect output, report an outcome, or retry.")?;c.revision+=1;db.save(&c)?;a.state="needs_report".into();db.save(&a)?;let mut r:Request=db.get(&a.request_id)?;if r.attempt==a.number&&r.state!="completed"{r.state="needs_report".into();db.save(&r)?;}}
                        Ok(())})?;
                        self.emit("runtime", Some(run.card_id.clone()));
                    }
                }
            }
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
        };
        self.store.lock().unwrap().write(|db| {
            db.save(&run)?;
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
        let terminal = client.launch(
            &req.profile,
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
                a.state = "failed".into();
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
