//! Independent terminals share Herdr transport, never Scheduler admission.
use super::*;

impl AppServer {
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
        let inherited = p["inherit_terminal_id"].as_str().and_then(|id| {
            self.terminal_context(json!({"terminal_id":id})).ok()?["cwd"]
                .as_str()
                .map(str::to_owned)
        });
        let mut launch = None;
        let saved = self.mutate("terminal.create", &p, |db| {
            let project = p["project_id"]
                .as_str()
                .map(|id| db.get::<Project>(id))
                .transpose()?;
            let card = p["card_id"]
                .as_str()
                .map(|id| db.get::<Card>(id))
                .transpose()?;
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
            let cwd = p["cwd"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| card.as_ref().and_then(|c| c.worktree.clone()))
                .or_else(|| project.as_ref().map(|p| p.directory.clone()))
                .or_else(|| {
                    card.as_ref()
                        .and_then(|c| db.get::<Project>(&c.project_id).ok())
                        .map(|p| p.directory)
                })
                .or_else(|| inherited.clone())
                .or_else(|| std::env::var("HOME").ok())
                .context("Home directory unavailable")?;
            ensure!(
                Path::new(&cwd).is_absolute() && Path::new(&cwd).is_dir(),
                "Terminal directory no longer exists or is not absolute"
            );
            let item = TerminalEntry {
                id: format!("shell-{}", id()),
                conversation_id: None,
                number: db.counter("terminal-number")?,
                name: None,
                manual_number: Some(db.counter("terminal-manual-number")?),
                created_at: now(),
                runtime: Some(TerminalRuntime {
                    core_id: None,
                    terminal: None,
                    state: "starting".into(),
                    connection: "checking".into(),
                    error: None,
                }),
            };
            db.save(&item)?;
            // Persist the launch intent before network IO, but cwd is only an argument.
            launch = Some((item.clone(), cwd));
            Ok(serde_json::to_value(item)?)
        })?;
        let Some((mut item, cwd)) = launch else {
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
            let shell = std::env::var("SHELL")
                .ok()
                .filter(|s| Path::new(s).is_absolute() && Path::new(s).is_file())
                .unwrap_or_else(|| "/bin/zsh".into());
            let receipt = client.launch(
                vec![shell, "-l".into()],
                &cwd,
                Default::default(),
                std::collections::BTreeMap::from([("canban.terminal".into(), item.id.clone())]),
            )?;
            runtime.terminal = Some(
                receipt["address"]
                    .as_str()
                    .context("Terminal launch receipt has no address")?
                    .into(),
            );
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
    fn directory_context(
        s: &Snapshot,
        id: &str,
        core: &str,
        terminal: &str,
        value: &Value,
    ) -> TerminalContext {
        let cwd = canban_herdr::cwd(value).map(str::to_owned);
        let (project_id, card_id, branch) = cwd
            .as_ref()
            .map(|d| canban_workspaces::context::resolve(s, d))
            .unwrap_or_default();
        TerminalContext {
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
            let client = canban_herdr::Client {
                config: self.config(),
            };
            ensure!(
                client.status()?["core"]["id"] == core,
                "Original Core unavailable"
            );
            let value = client.get(&terminal)?;
            Ok(Self::directory_context(&s, key, &core, &terminal, &value))
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
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let observed = (|| -> Result<(String, Vec<Value>)> {
            let core = client.status()?["core"]["id"]
                .as_str()
                .context("Core identity unavailable")?
                .to_owned();
            let list = client.call("terminal.list", json!({"all":true}))?;
            let terminals = list["terminals"]
                .as_array()
                .context("Terminal list unavailable")?
                .clone();
            ensure!(
                client.status()?["core"]["id"] == core,
                "Core changed during observation"
            );
            Ok((core, terminals))
        })();
        let mut contexts = Vec::new();
        for mut item in snapshot.terminals.clone() {
            let before = item.clone();
            let binding = Self::terminal_binding(&snapshot, &item);
            let value = binding.as_ref().and_then(|(core, terminal)| {
                let (observed_core, values) = observed.as_ref().ok()?;
                (core == observed_core)
                    .then(|| values.iter().find(|v| v["address"] == *terminal))
                    .flatten()
            });
            if let (Some((core, terminal)), Some(value)) = (&binding, value) {
                contexts.push(Self::directory_context(
                    &snapshot, &item.id, core, terminal, value,
                ));
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
                        runtime.connection = "unavailable".into();
                    }
                    _ => {}
                }
            }
            if item != before {
                self.store.lock().unwrap().write(|db| {
                    let mut current: TerminalEntry = db.get(&item.id)?;
                    current.runtime = item.runtime;
                    db.save(&current)
                })?;
                self.emit("terminals", None);
            }
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
                live && s
                    .terminal_contexts
                    .iter()
                    .find(|c| c.terminal_id == t.id)
                    .and_then(|c| c.cwd.as_deref())
                    .is_none_or(|cwd| canban_workspaces::context::contains(worktree, cwd))
            }))
    }
}
