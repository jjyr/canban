//! Independent terminals share Herdr transport, never Scheduler admission.
use super::*;

impl AppServer {
    /// Close is a product lifecycle operation, independent of UI tab shortcuts.
    pub(super) fn close_terminal(&self, p: Value) -> Result<Value> {
        let _gate = self.terminal_gate.lock().unwrap();
        ensure!(
            p["execution"].is_null(),
            "Closing a terminal requires a user action"
        );
        let terminal_id = strp(&p, "terminal_id")?;
        let mut target = None;
        let mut invocation = None;
        let saved = self.mutate("terminal.close", &p, |db| {
            let mut item: TerminalEntry = db.get(terminal_id)?;
            if let Ok(mut configuration)=db.get::<ConfigurationSession>(terminal_id) && configuration.state=="queued" {
                configuration.state="cancelled".into();db.save(&configuration)?;
                item.runtime=Some(TerminalRuntime{machine:"local".into(),owner_core_id:None,core_id:None,terminal:None,state:"finished".into(),connection:"connected".into(),error:None});db.save(&item)?;
            }
            if let Some(conversation) = &item.conversation_id {
                let row = canban_notifications::overview(&db.snapshot()?).into_iter().find(|r| &r.id == conversation);
                if let Some(mut run) = row.and_then(|r| r.attempt)
                    && !run.finished() {
                        let core = run.core_id.clone().context("Original Core binding unavailable")?;
                        let terminal = run.terminal.clone().context("Terminal is still starting; wait before closing")?;
                        target = Some((core, terminal));
                        run.close_state = "closing".into();
                        run.close_requested_at = Some(now());
                        db.save(&run)?;
                        let card: Card = db.get(&run.card_id)?;
                        log(db, &card, "execution.close_requested", "user", "Terminal closed by user", Some(run.request_id.clone()), json!({"attempt":run.number}))?;
                        invocation = Some(run.id);
                }
            } else if let Some(runtime) = &mut item.runtime
                && runtime.state != "finished" {
                target = Some((runtime.core_id.clone().context("Original Core binding unavailable")?, runtime.terminal.clone().context("Terminal binding unavailable; inspect before closing")?));
                runtime.state = "closing".into();
                db.save(&item)?;
            }
            Ok(json!({"state":if target.is_some(){"closing"}else{"closed"},"terminal_id":terminal_id}))
        })?;
        let Some((core, terminal)) = target else {
            return Ok(saved);
        };
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let result = (|| -> Result<Value> {
            ensure!(
                client.status()?["core"]["id"] == core,
                "Original Herdr core is unavailable"
            );
            client.call("terminal.kill", json!({"terminal":terminal}))
        })();
        let value = match &result {
            Ok(_) => json!({"state":"closed","terminal_id":terminal_id}),
            Err(e) => {
                json!({"state":"unknown","terminal_id":terminal_id,"message":format!("{e:#}; inspect before retrying close")})
            }
        };
        self.store.lock().unwrap().write(|db| {
            if let Some(key) = p["request_key"].as_str() {
                db.conn.execute(
                    "UPDATE operations SET result=?2 WHERE id=?1",
                    params![key, serde_json::to_string(&value)?],
                )?;
            }
            if let Some(id) = invocation {
                let mut run: Attempt = db.get(&id)?;
                if let Err(e) = &result {
                    run.close_state = "error".into();
                    run.error = Some(format!("Terminal close failed: {e:#}"));
                    db.save(&run)?;
                }
            } else if result.is_err() {
                let mut item: TerminalEntry = db.get(terminal_id)?;
                if let Some(r) = &mut item.runtime {
                    r.state = "unknown".into();
                    r.connection = "unavailable".into();
                    r.error = Some(value["message"].as_str().unwrap_or("").into());
                }
                db.save(&item)?;
            }
            Ok(())
        })?;
        self.emit("terminals", None);
        Ok(value)
    }
    /// Resolve exactly one owning binding; Agent Attempts remain authoritative.
    pub(super) fn terminal_binding(s: &Snapshot, item: &TerminalEntry) -> Option<(String, String)> {
        if let Some(conversation) = &item.conversation_id {
            let row = canban_notifications::overview(s)
                .into_iter()
                .find(|r| &r.id == conversation)?;
            let a = row.attempt?;
            return Some((a.core_id?, a.terminal?));
        }
        let r = item.runtime.as_ref()?;
        Some((r.core_id.clone()?, r.terminal.clone()?))
    }
    pub(super) fn create_terminal(&self, p: Value) -> Result<Value> {
        let _gate = self.terminal_gate.lock().unwrap();
        let inherited = p["inherit_terminal_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(|id| self.terminal_context(json!({"terminal_id":id})))
            .transpose()?;
        if p["inherit_terminal_id"]
            .as_str()
            .is_some_and(|v| !v.is_empty())
            && inherited.as_ref().is_some_and(|v| v.get("error").is_some())
        {
            ensure!(
                p["machine"].is_string(),
                "Selected terminal is unavailable; explicitly choose a machine for a new terminal"
            );
        }
        let default_machine = self.config().default_machine;
        let explicit_profile = p["profile_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let want_agent = p["agent"] == true || explicit_profile.is_some();
        if want_agent {
            ensure!(
                p["execution"].is_null(),
                "Standalone Agent CLI launch requires a user action"
            );
        }
        let mut launch = None;
        let saved = self.mutate("terminal.create", &p, |db| {
            let mut project = p["project_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(|id| db.get::<Project>(id))
                .transpose()?;
            let mut card = p["card_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(|id| db.get::<Card>(id))
                .transpose()?;
            if card.is_none()
                && let Some(cid) = inherited.as_ref().and_then(|v| v["card_id"].as_str())
                && !cid.is_empty()
            {
                card = db.get::<Card>(cid).ok();
            }
            if project.is_none() {
                if let Some(c) = &card {
                    project = db.get::<Project>(&c.project_id).ok();
                } else if let Some(pid) = inherited.as_ref().and_then(|v| v["project_id"].as_str())
                    && !pid.is_empty()
                {
                    project = db.get::<Project>(pid).ok();
                }
            }
            if let Some(c) = &card {
                ensure!(
                    project.as_ref().is_none_or(|p| c.project_id == p.id),
                    "Card belongs to another Project"
                );
                ensure!(
                    c.archived_at.is_none(),
                    "Restore this Card before opening a terminal"
                );
            }
            let location = card
                .as_ref()
                .and_then(|c| c.workspace.as_ref().map(|w| w.location.clone()))
                .or_else(|| project.as_ref().map(Project::work_location));
            let inherited_machine = inherited.as_ref().and_then(|v| v["machine"].as_str());
            let machine = self.resolve_machine(
                p["machine"]
                    .as_str()
                    .or_else(|| location.as_ref().map(|l| l.machine.as_str()))
                    .or(inherited_machine)
                    .unwrap_or(&default_machine),
                true,
            )?;
            let host = canban_herdr::host::Host::new(self.config(), &machine);
            let explicit_machine = p["machine"].is_string();
            let cwd = p["cwd"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    card.as_ref()
                        .and_then(|c| c.worktree.clone())
                        .filter(|wt| Path::new(wt).is_absolute() && host.is_dir(Path::new(wt)))
                })
                .or_else(|| {
                    location
                        .as_ref()
                        .filter(|l| l.machine == machine)
                        .map(|l| l.directory.clone())
                })
                .or_else(|| {
                    (!explicit_machine)
                        .then(|| {
                            inherited
                                .as_ref()
                                .and_then(|v| v["cwd"].as_str())
                                .map(str::to_owned)
                        })
                        .flatten()
                })
                .unwrap_or(host.home()?);
            ensure!(
                Path::new(&cwd).is_absolute() && host.is_dir(Path::new(&cwd)),
                "Terminal directory no longer exists on the selected machine"
            );
            let custom_name = p["name"]
                .as_str()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_owned);
            if let Some(n) = &custom_name {
                ensure!(
                    n.chars().count() <= 80 && !n.chars().any(char::is_control),
                    "Use at most 80 printable characters"
                );
            }
            let agent_profile = if want_agent {
                let cfg = self.config();
                Some(canban_scheduler::select_in_project(
                    &cfg,
                    project.as_ref(),
                    explicit_profile.as_deref(),
                )?)
            } else {
                None
            };
            let agent_engine = agent_profile.as_ref().map(|prof| {
                let cmd = prof.command_name();
                match cmd.as_str() {
                    "codex" | "claude" => cmd,
                    _ => prof.preset.clone().unwrap_or(cmd),
                }
            });
            let origin_card_id = if p["card_id"].as_str().is_some_and(|s| !s.is_empty()) {
                card.as_ref().map(|c| c.id.clone())
            } else {
                None
            };
            let prefix = if want_agent { "agent" } else { "shell" };
            let item = TerminalEntry {
                id: format!("{prefix}-{}", id()),
                conversation_id: None,
                number: db.counter("terminal-number")?,
                name: custom_name,
                manual_number: Some(db.counter("terminal-manual-number")?),
                origin_card_id,
                agent_engine,
                created_at: now(),
                runtime: Some(TerminalRuntime {
                    machine: machine.clone(),
                    owner_core_id: None,
                    core_id: None,
                    terminal: None,
                    state: "starting".into(),
                    connection: "checking".into(),
                    error: None,
                }),
            };
            db.save(&item)?;
            let project_dir = project
                .as_ref()
                .map(|proj| proj.directory.clone())
                .unwrap_or_else(|| cwd.clone());
            // Persist the launch intent before network IO, but cwd is only an argument.
            launch = Some((item.clone(), cwd, project_dir, agent_profile));
            Ok(serde_json::to_value(item)?)
        })?;
        let Some((mut item, cwd, project_dir, agent_profile)) = launch else {
            return Ok(serde_json::to_value(TerminalEntry::from_json(saved)?)?);
        };
        let runtime = item.runtime.as_mut().unwrap();
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let result = (|| -> Result<()> {
            runtime.core_id = Some(
                client.status()?["core"]["id"]
                    .as_str()
                    .context("Core has no identity")?
                    .into(),
            );
            let host = canban_herdr::host::Host::new(self.config(), &runtime.machine);
            let mut env = std::collections::BTreeMap::new();
            let argv = if let Some(profile) = &agent_profile {
                let exec_home = self.execution_home(&runtime.machine)?;
                let session_id = profile
                    .command()
                    .iter()
                    .any(|s| s.contains("{session_id}"))
                    .then(super::runtime::uuid_session_id);
                let mut argv = if let Some(prompt) = p["prompt"].as_str().filter(|s| !s.is_empty())
                {
                    canban_profiles::command(
                        profile,
                        session_id.as_deref(),
                        true,
                        &[
                            ("home", &exec_home.to_string_lossy()),
                            ("cwd", &cwd),
                            ("project_dir", &project_dir),
                            ("prompt", prompt),
                            ("cli", &self.cli.to_string_lossy()),
                        ],
                    )?
                } else {
                    canban_profiles::interactive_command(
                        profile,
                        session_id.as_deref(),
                        &[
                            ("home", &exec_home.to_string_lossy()),
                            ("cwd", &cwd),
                            ("project_dir", &project_dir),
                            ("cli", &self.cli.to_string_lossy()),
                        ],
                    )?
                };
                let is_herdr_template = profile
                    .preset
                    .as_deref()
                    .is_some_and(|s| s.starts_with("herdr:"));
                if !argv.is_empty() && !is_herdr_template {
                    if runtime.machine == "local" {
                        argv[0] = canban_profiles::executable(&argv[0]);
                    } else if Path::new(&argv[0]).components().count() == 1 {
                        let program = host.python("import os,shutil,json,sys;h=os.path.expanduser('~');p=os.environ.get('PATH','')+os.pathsep+os.path.join(h,'.local/bin')+os.pathsep+os.path.join(h,'.cargo/bin');print(json.dumps(shutil.which(sys.argv[1],path=p) or sys.argv[1]))", &[&argv[0]])?;
                        argv[0] = program.as_str().context("No remote executable")?.into();
                    }
                }
                if runtime.machine == "local" {
                    env.insert(
                        "CANBAN_HOME".into(),
                        self.home.to_string_lossy().into_owned(),
                    );
                    env.insert(
                        "CANBAN_ENDPOINT".into(),
                        self.endpoint().to_string_lossy().into_owned(),
                    );
                    env.insert("CANBAN_CLI".into(), self.cli.to_string_lossy().into_owned());
                }
                argv
            } else {
                let shell = if runtime.machine == "local" {
                    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
                } else {
                    host.python("import os,json,pwd;print(json.dumps(pwd.getpwuid(os.getuid()).pw_shell or '/bin/sh'))",&[])?.as_str().context("No login shell")?.into()
                };
                if let Some(argv) = p["setup_argv"].as_array() {
                    ensure!(
                        runtime.machine == "local",
                        "Setup must run on AppServer host"
                    );
                    let c = self.config();
                    env.extend(c.hc_env);
                    env.insert("HC_SESSION".into(), c.hc_session);
                    argv.iter()
                        .map(|v| Ok(v.as_str().context("Invalid command")?.to_owned()))
                        .collect::<Result<Vec<_>>>()?
                } else {
                    vec![shell, "-l".into()]
                }
            };
            let receipt = client.launch_on(
                &runtime.machine,
                argv,
                &cwd,
                env,
                std::collections::BTreeMap::from([("canban.terminal".into(), item.id.clone())]),
            )?;
            let address = receipt["address"]
                .as_str()
                .context("Terminal launch receipt has no address")?
                .to_owned();
            runtime.owner_core_id = if runtime.machine == "local" {
                runtime.core_id.clone()
            } else {
                Some(runtime.machine.clone())
            };
            runtime.terminal = Some(address.clone());
            // Seed the in-memory inventory immediately so an observe_terminals pass
            // racing ahead of Herdr's async "created" subscription event does not
            // transiently downgrade connection -> "unavailable" (read_only = true).
            {
                let mut inv = self.inventory.lock().unwrap();
                if let Some(core_id) = &runtime.core_id
                    && inv.core == *core_id
                {
                    let key = inv.address(&address);
                    inv.records.entry(key.clone()).or_insert_with(|| {
                        let mut seeded = receipt.clone();
                        seeded["address"] = json!(key);
                        if seeded["state"].is_null() {
                            seeded["state"] = json!("live");
                        }
                        if seeded["cwd"].is_null() {
                            seeded["cwd"] = json!(cwd);
                        }
                        seeded
                    });
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                runtime.state = "running".into();
                runtime.connection = "connected".into();
            }
            Err(e) => {
                runtime.state = "unknown".into();
                runtime.connection = "unavailable".into();
                runtime.error = Some(format!(
                    "{e:#}. Launch was not replayed; inspect Herdr before creating another terminal."
                ));
            }
        }
        self.store.lock().unwrap().write(|db| {
            let mut current: TerminalEntry = db.get(&item.id)?;
            current.runtime = item.runtime.clone();
            item = current;
            db.save(&item)?;
            if let Some(key) = p["request_key"].as_str() {
                db.conn.execute(
                    "UPDATE operations SET result=?2 WHERE id=?1",
                    params![key, serde_json::to_string(&item)?],
                )?;
            }
            Ok(())
        })?;
        // Publish an actual Core observation before selecting the new tab,
        // avoiding a temporary Other group when its directory is already known.
        if let Ok(value) = self.terminal_context(json!({"terminal_id":item.id}))
            && let Ok(context) = serde_json::from_value::<TerminalContext>(value)
        {
            let mut contexts = self.terminal_contexts.lock().unwrap();
            contexts.retain(|c| c.terminal_id != item.id);
            contexts.push(context);
        }
        self.emit("terminals", None);
        Ok(serde_json::to_value(item)?)
    }
    pub(super) fn rename_terminal(&self, p: Value) -> Result<Value> {
        self.mutate("terminal.rename", &p, |db| {
            let mut item: TerminalEntry = db.get(strp(&p, "terminal_id")?)?;
            let name = strp(&p, "name")?.trim();
            ensure!(
                name.chars().count() <= 80 && !name.chars().any(char::is_control),
                "Use at most 80 printable characters"
            );
            item.name = (!name.is_empty()).then(|| name.into());
            db.save(&item)?;
            Ok(serde_json::to_value(item)?)
        })
    }
    pub(super) fn control_terminal(&self, p: Value, end: bool) -> Result<Value> {
        let _gate = self.terminal_gate.lock().unwrap();
        let item: TerminalEntry = self
            .store
            .lock()
            .unwrap()
            .db()
            .get(strp(&p, "terminal_id")?)?;
        let runtime = item
            .runtime
            .as_ref()
            .context("Select an independent terminal")?;
        let core = runtime
            .core_id
            .as_ref()
            .context("Terminal has no Core binding")?;
        let terminal = runtime
            .terminal
            .as_ref()
            .context("Terminal has no runtime binding")?;
        ensure!(runtime.state != "finished", "Terminal has already ended");
        let client = canban_herdr::Client {
            config: self.config(),
        };
        ensure!(
            client.status()?["core"]["id"] == *core,
            "Original Herdr core is unavailable"
        );
        let method = if end {
            "terminal.end"
        } else {
            "terminal.interrupt"
        };
        let mut send = false;
        let saved = self.mutate(method, &p, |_| {
            send = true;
            Ok(json!({"state":"unknown","message":"Control requested; inspect before retrying"}))
        })?;
        if !send {
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
            Err(e) => {
                json!({"state":"unknown","message":format!("{e:#}; inspect before retrying")})
            }
        };
        self.store.lock().unwrap().write(|db| {
            if let Some(key) = p["request_key"].as_str() {
                db.conn.execute(
                    "UPDATE operations SET result=?2 WHERE id=?1",
                    params![key, serde_json::to_string(&result)?],
                )?;
            }
            Ok(())
        })?;
        self.emit("terminals", None);
        Ok(result)
    }
    #[cfg(test)]
    fn directory_context(
        &self,
        s: &Snapshot,
        id: &str,
        core: &str,
        terminal: &str,
        value: &Value,
    ) -> TerminalContext {
        let machine = terminal.split_once('/').map(|(m, _)| m).unwrap_or("local");
        let cwd = canban_herdr::cwd(value).map(str::to_owned);
        let (project_id, card_id, branch) = cwd
            .as_ref()
            .map(|d| self.ops(machine).directory_context(s, d))
            .unwrap_or_default();
        TerminalContext {
            machine: machine.into(),
            terminal_id: id.into(),
            core_id: core.into(),
            terminal: terminal.into(),
            cwd,
            project_id,
            card_id,
            branch,
        }
    }
    pub(super) fn terminal_context(&self, p: Value) -> Result<Value> {
        let s = self.snapshot()?;
        let key = strp(&p, "terminal_id")?;
        let item = s
            .terminals
            .iter()
            .find(|t| t.id == key)
            .context("Terminal not found")?;
        let result = (|| -> Result<TerminalContext> {
            let (core, terminal) =
                Self::terminal_binding(&s, item).context("No runtime binding")?;
            #[cfg(test)]
            if !self.observer_running.load(Ordering::SeqCst) {
                let client = canban_herdr::Client {
                    config: self.config(),
                };
                ensure!(
                    client.status()?["core"]["id"] == core,
                    "Original Core unavailable"
                );
                return Ok(self.directory_context(
                    &s,
                    key,
                    &core,
                    &terminal,
                    &client.get(&terminal)?,
                ));
            }
            let (observed_core, values) = self.runtime_inventory()?;
            ensure!(observed_core == core, "Original Core unavailable");
            let value = values
                .iter()
                .find(|v| v["address"] == terminal)
                .context("Terminal is not in the current inventory")?;
            if p["refresh"] == true {
                self.invalidate_contexts();
            }
            self.refresh_context(&s, key, &core, &terminal, value)
        })();
        match result {
            Ok(context) => Ok(serde_json::to_value(context)?),
            // Never present the task/launch directory as a live observation.
            Err(e) => Ok(
                json!({"terminal_id":key,"cwd":null,"project_id":null,"card_id":null,"branch":null,"error":format!("{e:#}")}),
            ),
        }
    }
    pub(super) fn observe_terminals(&self) -> Result<()> {
        let snapshot = self.snapshot()?;
        if snapshot.terminals.is_empty() {
            return Ok(());
        }
        let observed = self.runtime_inventory();
        let mut contexts = Vec::new();
        let mut wanted = std::collections::BTreeSet::new();
        for mut item in snapshot.terminals.clone() {
            let before = item.clone();
            let binding = Self::terminal_binding(&snapshot, &item);
            let value = binding.as_ref().and_then(|(core, terminal)| {
                let (observed_core, values) = observed.as_ref().ok()?;
                (core == observed_core)
                    .then(|| values.iter().find(|v| v["address"] == *terminal))
                    .flatten()
            });
            let value = value.filter(|_| {
                binding
                    .as_ref()
                    .is_none_or(|(_, terminal)| self.remote_connected(terminal))
            });
            if let (Some((core, terminal)), Some(value)) = (&binding, value)
                && value["state"] != "finished"
            {
                #[cfg(test)]
                if !self.observer_running.load(Ordering::SeqCst) {
                    contexts
                        .push(self.directory_context(&snapshot, &item.id, core, terminal, value));
                }
                wanted.insert(item.id.clone());
                self.queue_context(&snapshot, &item.id, core, terminal, value);
                if let Some(context) =
                    self.cached_context(&item.id, core, terminal, canban_herdr::cwd(value))
                {
                    contexts.push(context);
                }
            }
            if let Some(runtime) = &mut item.runtime {
                match (&observed, value) {
                    (Ok(_), Some(v)) => {
                        runtime.connection = "connected".into();
                        runtime.error = None;
                        if v["state"] == "finished" {
                            runtime.state = "finished".into();
                        }
                    }
                    (Ok((core, _)), _) if runtime.core_id.as_ref().is_some_and(|id| id != core) => {
                        runtime.connection = "replaced".into();
                        runtime.error = Some(
                            "Original Core identity changed; this terminal was not replaced".into(),
                        );
                    }
                    (Err(e), _) => {
                        runtime.connection = "unavailable".into();
                        runtime.error = Some(format!("{e:#}"));
                    }
                    _ if runtime.state != "finished" => {
                        // Guard against async subscription lag immediately after launch.
                        let fresh_launch = runtime.connection == "connected"
                            && now().saturating_sub(item.created_at) < 3_000;
                        if !fresh_launch {
                            runtime.connection = "unavailable".into();
                        }
                    }
                    _ => {}
                }
            }
            if item != before {
                self.store.lock().unwrap().write(|db| {
                    let mut current: TerminalEntry = db.get(&item.id)?;
                    // A launch receipt may have arrived after this observation
                    // began. Never erase its binding with an older starting row.
                    if current.runtime == before.runtime {
                        current.runtime = item.runtime;
                        db.save(&current)?;
                    }
                    Ok(())
                })?;
                self.emit("terminals", None);
            }
        }
        if observed.is_ok() {
            let mut q = self.contexts.lock().unwrap();
            q.retain_targets(&wanted);
        }
        let changed = {
            let mut current = self.terminal_contexts.lock().unwrap();
            if *current == contexts {
                false
            } else {
                *current = contexts;
                true
            }
        };
        if changed {
            self.emit("terminals", None);
        }
        Ok(())
    }
    /// Any independent terminal in this checkout protects it, regardless of origin.
    pub(super) fn terminal_retains_worktree(&self, card: &Card) -> Result<bool> {
        let Some(worktree) = &card.worktree else {
            return Ok(false);
        };
        self.observe_terminals()?;
        let s = self.snapshot()?;
        Ok(s.terminals
            .iter()
            .filter(|t| t.conversation_id.is_none())
            .any(|t| {
                let live = t.runtime.as_ref().is_some_and(|r| r.state != "finished");
                let machine = t
                    .runtime
                    .as_ref()
                    .map(|r| r.machine.as_str())
                    .unwrap_or("local");
                let owner = card
                    .workspace
                    .as_ref()
                    .map(|w| w.location.machine.as_str())
                    .unwrap_or("local");
                live && machine == owner
                    && s.terminal_contexts
                        .iter()
                        .find(|c| c.terminal_id == t.id)
                        .and_then(|c| c.cwd.as_deref())
                        .is_none_or(|cwd| self.ops(owner).c_contains(worktree, cwd))
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshly_launched_terminal_stays_connected_before_subscription_catches_up() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        app.observer_running.store(true, Ordering::SeqCst);
        {
            let mut inv = app.inventory.lock().unwrap();
            inv.core = "core-live".into();
            inv.last_at = now();
        }
        let created = now();
        let entry = TerminalEntry {
            id: "shell-fresh".into(),
            conversation_id: None,
            number: 1,
            name: None,
            manual_number: Some(1),
            origin_card_id: None,
            agent_engine: None,
            created_at: created,
            runtime: Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: Some("core-live".into()),
                core_id: Some("core-live".into()),
                terminal: Some("local/tty-1".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
            }),
        };
        app.store
            .lock()
            .unwrap()
            .write(|db| db.save(&entry))
            .unwrap();

        // While within the launch grace window, async inventory lag must not mark it unavailable.
        app.observe_terminals().unwrap();
        let s = app.snapshot().unwrap();
        assert_eq!(
            s.terminals[0].runtime.as_ref().unwrap().connection,
            "connected"
        );

        // Once older than the launch grace window and still absent from inventory, it becomes unavailable.
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut stale: TerminalEntry = db.get("shell-fresh")?;
                stale.created_at = created.saturating_sub(5_000);
                db.save(&stale)
            })
            .unwrap();
        app.observe_terminals().unwrap();
        let s2 = app.snapshot().unwrap();
        assert_eq!(
            s2.terminals[0].runtime.as_ref().unwrap().connection,
            "unavailable"
        );
    }
}
