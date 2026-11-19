use super::*;

impl AppServer {
    pub(super) fn read_resource(&self, card_id: &str, path: &str) -> Result<Value> {
        let c: Card = self.store.lock().unwrap().db().get(card_id)?;
        let root = canban_context::card_dir(&self.home, &c.project_id, c.number);
        let full = self.home.join(path);
        ensure!(
            full.starts_with(&root)
                && !Path::new(path)
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir)),
            "Resource is outside this Card"
        );
        let bytes = if full.exists() {
            std::fs::read(full)?
        } else {
            canban_context::archived_file(&self.home, &c, path)?
        };
        Ok(json!({"body":String::from_utf8(bytes).context("Resource is not UTF-8 text")?}))
    }
    pub(super) fn add_attachment(&self, p: &Value) -> Result<Value> {
        let c: Card = self.store.lock().unwrap().db().get(strp(p, "card_id")?)?;
        ensure!(
            c.archived_at.is_none(),
            "Restore the Card before adding attachments"
        );
        let bytes = if let Some(path) = p["source"].as_str() {
            ensure!(
                std::fs::metadata(path)?.len() <= 32 * 1024 * 1024,
                "Attachment exceeds 32 MiB"
            );
            std::fs::read(path)?
        } else {
            p["bytes"]
                .as_array()
                .context("source or byte array required")?
                .iter()
                .map(|b| {
                    b.as_u64()
                        .filter(|n| *n < 256)
                        .map(|n| n as u8)
                        .context("Invalid byte")
                })
                .collect::<Result<Vec<_>>>()?
        };
        let a = canban_context::attachment(
            &self.home,
            &c,
            strp(p, "name")?,
            p["media_type"]
                .as_str()
                .unwrap_or("application/octet-stream"),
            &bytes,
        )?;
        self.mutate("attachment.add", p, |db| {
            db.save(&a)?;
            log(
                db,
                &c,
                "attachment.added",
                "user",
                &a.name,
                None,
                json!({"artifact_id":a.id}),
            )?;
            Ok(serde_json::to_value(a)?)
        })
    }
    pub(super) fn archive(&self, p: &Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Archiving requires an explicit user decision"
        );
        let _gate = self.terminal_gate.lock().unwrap();
        let card_id = strp(p, "card_id")?;
        self.mutate("card.archive.mark", p, |db| {
            let mut c: Card = db.get(card_id)?;
            revise(&c, p)?;
            if c.archived_at.is_none() {
                c.archived_at = Some(now());
                c.revision += 1;
                log(
                    db,
                    &c,
                    "card.archived",
                    "user",
                    "Card archived",
                    None,
                    json!({}),
                )?;
                db.save(&c)?;
            }
            crate::workflow::stop_card(db, &self.config(), &c.id, "cancelled")?;
            for mut entry in db.list::<TerminalEntry>()? {
                if let Some(a) = &mut entry.agent
                    && a.card_id == c.id
                {
                    let mut changed = false;
                    if a.state == "queued" {
                        a.state = "cancelled".into();
                        changed = true;
                    }
                    if a.pending.take().is_some() {
                        changed = true;
                    }
                    if changed {
                        db.save(&entry)?;
                    }
                }
            }
            Ok(json!({"ok":true}))
        })?;
        if let Err(error) = self.close_archived_agents(card_id) {
            self.store.lock().unwrap().write(|db| {
                let mut card: Card = db.get(card_id)?;
                card.archive_error = Some(format!("Archive cleanup needs retry: {error:#}"));
                db.save(&card)
            })?;
            self.emit("archive", Some(card_id.into()));
            return Err(error);
        }
        // A receipt retry reuses the already published package.
        let already: Card = self.store.lock().unwrap().db().get(card_id)?;
        if let Some(path) = already
            .archive
            .as_ref()
            .filter(|_| already.archive_error.is_none())
            && p["request_key"].as_str().is_some()
        {
            return Ok(json!({"archived":true,"path":path}));
        }
        for entry in self.snapshot()?.terminals.iter().filter(|t| {
            t.agent
                .as_ref()
                .is_some_and(|a| a.card_id == card_id && a.terminal.is_some())
        }) {
            let _ = self.capture_output(entry);
        }
        let snapshot = self.snapshot()?;
        let card = snapshot
            .cards
            .iter()
            .find(|c| c.id == card_id)
            .context("Card missing")?
            .clone();
        let project = snapshot
            .projects
            .iter()
            .find(|p| p.id == card.project_id)
            .unwrap()
            .clone();
        let filtered = Snapshot {
            worktrees: Vec::new(),
            workflow_sources: Vec::new(),
            workflow_templates: Vec::new(),
            configuration_sessions: Vec::new(),
            merge_batches: Vec::new(),
            event_subscriptions: Vec::new(),
            workflow_runs: snapshot
                .workflow_runs
                .iter()
                .filter(|r| r.card_id.as_deref() == Some(card_id))
                .cloned()
                .collect(),
            workflow_revisions: snapshot
                .workflow_revisions
                .iter()
                .filter(|r| r.project_id == card.project_id)
                .cloned()
                .collect(),
            card_reads: snapshot
                .card_reads
                .iter()
                .filter(|r| r.card_id == card_id)
                .cloned()
                .collect(),
            github: Default::default(),
            github_issues: Vec::new(),
            terminal_contexts: Vec::new(),
            merging_cards: Vec::new(),
            terminals: snapshot
                .terminals
                .iter()
                .filter(|v| {
                    v.agent.as_ref().is_some_and(|a| a.card_id == card_id)
                        || v.origin_card_id.as_deref() == Some(card_id)
                        || v.conversation_id.as_ref().is_some_and(|id| {
                            snapshot
                                .conversations
                                .iter()
                                .any(|c| &c.id == id && c.card_id == card_id)
                        })
                })
                .cloned()
                .collect(),
            note_events: snapshot
                .note_events
                .iter()
                .filter(|v| v.card_id.as_deref() == Some(card_id))
                .cloned()
                .collect(),
            conversations: snapshot
                .conversations
                .iter()
                .filter(|v| v.card_id == card_id)
                .cloned()
                .collect(),
            questions: snapshot
                .questions
                .iter()
                .filter(|v| v.card_id == card_id)
                .cloned()
                .collect(),
            projects: vec![project.clone()],
            cards: vec![card.clone()],
            reports: snapshot
                .reports
                .into_iter()
                .filter(|v| v.card_id == card_id)
                .collect(),
            notes: snapshot
                .notes
                .into_iter()
                .filter(|v| v.project_id == card.project_id)
                .collect(),
            notices: snapshot
                .notices
                .into_iter()
                .filter(|v| v.card_id == card_id)
                .collect(),
            artifacts: snapshot
                .artifacts
                .into_iter()
                .filter(|v| v.card_id == card_id)
                .collect(),
            ..Default::default()
        };
        let card_events = self.store.lock().unwrap().db().events_for_card(card_id)?;
        // Repack after restore first, so previously compressed bytes are not omitted.
        if let Some(old) = &card.archive {
            canban_context::restore_files(&self.home, &card, old)?;
        }
        let result = canban_context::pack(&self.home, &card, &filtered, &card_events);
        match result {
            Ok(path) => {
                self.store.lock().unwrap().write(|db| {
                    let mut c: Card = db.get(card_id)?;
                    c.archive = Some(path.clone());
                    c.archive_error = None;
                    db.save(&c)?;
                    Ok(())
                })?;
                canban_context::compact_files(&self.home, &card, &path)?;
                let notice = self.archive_workspaces(&project, &card);
                if let Some(message) = notice {
                    self.store.lock().unwrap().write(|db| {
                        let mut c: Card = db.get(card_id)?;
                        attention(db, &mut c, &message)?;
                        db.save(&c)
                    })?;
                }
                self.emit("archive", Some(card_id.into()));
                Ok(json!({"archived":true,"path":path}))
            }
            Err(e) => {
                self.store.lock().unwrap().write(|db| {
                    let mut c: Card = db.get(card_id)?;
                    c.archive_error = Some(e.to_string());
                    db.save(&c)
                })?;
                Err(e)
            }
        }
    }
    fn close_archived_agents(&self, card_id: &str) -> Result<()> {
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let terminals = self.snapshot()?.terminals;
        for entry in terminals.iter().filter(|t| {
            t.agent
                .as_ref()
                .is_some_and(|a| a.card_id == card_id && a.terminal.is_some() && !a.finished())
        }) {
            let run = entry.agent.as_ref().unwrap();
            let terminal = run.terminal.as_ref().unwrap();
            ensure!(
                client.status()?["core"]["id"].as_str() == run.core_id.as_deref(),
                "Original Core is unavailable; restore its connection and retry Archive"
            );
            let actual = client.get(terminal)?;
            if actual["state"] != "finished" {
                client.call("terminal.kill", json!({"terminal":terminal}))?;
            }
            let actual = client.get(terminal)?;
            ensure!(
                actual["state"] == "finished",
                "Terminal close is not confirmed; retry Archive"
            );
            self.capture_output(entry)?;
            self.store.lock().unwrap().write(|db| {
                let mut current: TerminalEntry = db.get(&entry.id)?;
                if let Some(a) = &mut current.agent {
                    a.observed = Some(canban_herdr::observation(&actual));
                    a.close_state = "closed".into();
                    a.close_requested_at = Some(now());
                    a.lifecycle.close_reason = "archive".into();
                    a.connection_state = "connected".into();
                    a.pending = None;
                    if !matches!(a.state.as_str(), "completed" | "backlog") {
                        a.state = "cancelled".into();
                    }
                    let run_number = a.number;
                    db.save(&current)?;
                    log(
                        db,
                        &db.get::<Card>(card_id)?,
                        "execution.archived",
                        "system",
                        "Closed Card Agent after saving its terminal history",
                        Some(entry.id.clone()),
                        json!({"run_number":run_number}),
                    )?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
    fn archive_workspaces(&self, project: &Project, card: &Card) -> Option<String> {
        let snapshot = match self.snapshot() {
            Ok(s) => s,
            Err(e) => return Some(format!("Archive saved; workspace cleanup deferred: {e:#}")),
        };
        let mut workspaces = card.workspace_history.clone();
        if let Some(w) = &card.workspace {
            workspaces.push(w.clone())
        } else if let Some(path) = &card.worktree {
            workspaces.push(WorkspaceBinding {
                location: project.work_location(),
                path: path.clone(),
                branch: card.branch.clone(),
                base_commit: card.base_commit.clone(),
                target_branch: project.target_branch.clone(),
            });
        }
        let mut seen = std::collections::HashSet::new();
        let mut notes = Vec::new();
        for w in workspaces {
            if !seen.insert((w.location.machine.clone(), w.path.clone())) {
                continue;
            }
            let mut c = card.clone();
            c.worktree = Some(w.path.clone());
            c.branch = w.branch.clone();
            c.base_commit = w.base_commit.clone();
            c.workspace = Some(w.clone());
            let result = (|| -> Result<Option<String>> {
                if snapshot
                    .terminals
                    .iter()
                    .filter_map(|t| t.agent.as_ref())
                    .any(|a| {
                        a.card_id == card.id
                            && a.cwd == w.path
                            && a.location
                                .as_ref()
                                .is_some_and(|l| l.machine == w.location.machine)
                            && a.terminal.is_some()
                            && a.observed.as_ref().is_none_or(|v| v["state"] != "finished")
                    })
                    || self.terminal_retains_worktree(&c)?
                {
                    return Ok(Some("A terminal may still be using this worktree".into()));
                }
                let p = self.bound_project(project, &c);
                self.ops(&w.location.machine).archive(
                    &self.execution_home(&w.location.machine)?,
                    &p,
                    &c,
                )
            })();
            match result {
                Ok(Some(message)) => notes.push(format!("{}: {message}", w.location.machine)),
                Err(e) => notes.push(format!("{}: {e:#}", w.location.machine)),
                _ => {}
            }
        }
        (!notes.is_empty())
            .then(|| format!("Archive saved; workspaces retained: {}", notes.join("; ")))
    }
    pub(super) fn restore(&self, p: &Value) -> Result<Value> {
        let id = strp(p, "card_id")?;
        let c: Card = self.store.lock().unwrap().db().get(id)?;
        revise(&c, p)?;
        if let Some(path) = &c.archive {
            canban_context::restore_files(&self.home, &c, path)?;
        }
        self.mutate("card.restore", p, |db| {
            let mut c: Card = db.get(id)?;
            revise(&c, p)?;
            c.archived_at = None;
            c.removed_at = None;
            c.archive = None;
            c.revision += 1;
            db.save(&c)?;
            log(
                db,
                &c,
                "card.restored",
                "user",
                "Card restored; no execution automatically scheduled",
                None,
                json!({}),
            )?;
            Ok(serde_json::to_value(c)?)
        })
    }
    pub(super) fn output(&self, id: &str) -> Result<Value> {
        let entry: TerminalEntry = self.store.lock().unwrap().db().get(id)?;
        if let Some(run) = &entry.agent {
            let c: Card = self.store.lock().unwrap().db().get(&run.card_id)?;
            if (c.archived_at.is_some() || run.finished())
                && let Some(path) = &run.output_path
            {
                return self.read_resource(&c.id, path);
            }
            let terminal = run
                .terminal
                .as_ref()
                .context("Execution has no terminal binding")?;
            let client = canban_herdr::Client {
                config: self.config(),
            };
            let live = (|| {
                let status = client.status()?;
                ensure!(
                    run.core_id.as_deref() == status["core"]["id"].as_str(),
                    "Core identity changed; original terminal cannot be resolved"
                );
                client.read(terminal)
            })();
            match live {
                Ok(value) => Ok(value),
                Err(e) => {
                    if let Some(path) = &run.output_path {
                        self.read_resource(&c.id, path)
                    } else {
                        Err(e)
                    }
                }
            }
        } else {
            if let Ok(sched) = self.store.lock().unwrap().db().get::<ScheduledRun>(id)
                && !sched.creates_card
                && entry
                    .runtime
                    .as_ref()
                    .is_some_and(|r| r.state == "finished")
                && let Some(path) = &sched.output_path
            {
                return self.schedule_call(
                    "schedule.read_artifact",
                    json!({ "schedule_run_id": sched.id, "path": path }),
                );
            }
            let runtime = entry
                .runtime
                .as_ref()
                .context("Terminal has no runtime binding")?;
            let terminal = runtime
                .terminal
                .as_ref()
                .context("Terminal has no Herdr address")?;
            let client = canban_herdr::Client {
                config: self.config(),
            };
            let status = client.status()?;
            ensure!(
                runtime.core_id.as_deref() == status["core"]["id"].as_str(),
                "Core identity changed; original terminal cannot be resolved"
            );
            client.read(terminal)
        }
    }
    pub(super) fn capture_output(&self, entry: &TerminalEntry) -> Result<()> {
        let Some(run) = &entry.agent else {
            return Ok(());
        };
        let Some(terminal) = &run.terminal else {
            return Ok(());
        };
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let status = client.status()?;
        ensure!(
            run.core_id.as_deref() == status["core"]["id"].as_str(),
            "Core identity changed; skip unrelated terminal"
        );
        let value = client.read(terminal)?;
        let text = value["text"].as_str().context("No terminal text")?;
        let dir = if run.card_number > 0 {
            canban_context::run_dir(&self.home, entry)
        } else {
            let card_number = self
                .store
                .lock()
                .unwrap()
                .db()
                .get::<Card>(&run.card_id)
                .map(|c| c.number)
                .unwrap_or(0);
            if card_number > 0 {
                canban_context::card_dir(&self.home, &run.project_id, card_number)
                    .join("agents")
                    .join(&run.profile.id)
            } else {
                canban_context::run_dir(&self.home, entry)
            }
        };
        let path = dir.join(format!("output-{}.txt", id()));
        canban_context::atomic(&path, text.as_bytes())?;
        let relative = canban_context::relative(&self.home, &path)?;
        self.store.lock().unwrap().write(|db| {
            let mut current: TerminalEntry = db.get(&entry.id)?;
            if let Some(a) = &mut current.agent {
                a.output_path = Some(relative);
                db.save(&current)?;
            }
            Ok(())
        })?;
        Ok(())
    }

    pub(super) fn resolve_card_preview(&self, p: &Value) -> Result<Value> {
        let (card, project, active_workflows, agent_warning) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let c = super::resolve_card(db, p)?;
            let project: Project = db.get(&c.project_id)?;
            let project = self.bound_project(&project, &c);
            let active_workflows = db
                .list::<WorkflowRun>()?
                .into_iter()
                .any(|r| r.card_id.as_deref() == Some(c.id.as_str()) && r.active());
            let agent_warning = crate::review::activity_warning(db, &c)?;
            (c, project, active_workflows, agent_warning)
        };
        let mut warnings = Vec::<String>::new();
        if active_workflows {
            warnings.push("Active workflow run will be stopped".into());
        }
        if let Some(w) = &agent_warning {
            warnings.push(w.clone());
        }
        if card.worktree.is_some() && !card.merged() {
            let ops = self.ops(&project.work_location().machine);
            if let Ok(info) = ops.info(&project, &card) {
                if info.dirty || info.state == "conflict" {
                    warnings.push("Worktree has uncommitted changes".into());
                } else if info.state == "available" {
                    let target = info.target_branch.as_deref().unwrap_or("");
                    let branch = info.branch.as_deref().unwrap_or("");
                    if !target.is_empty()
                        && !branch.is_empty()
                        && !ops.branch_merged(Path::new(&project.directory), branch, target)
                    {
                        warnings.push(format!(
                            "Branch {branch} has unmerged changes against {target}"
                        ));
                    }
                }
            }
        }
        Ok(json!({
            "card_id": card.id,
            "revision": card.revision,
            "warnings": warnings,
            "has_active_agent": agent_warning.is_some(),
        }))
    }

    pub(super) fn resolve_card_action(&self, p: &Value) -> Result<Value> {
        let config = self.config();
        let mut emit_card = p["card_id"].as_str().map(str::to_owned);
        let res = self.mutate("card.resolve", p, |db| {
            let mut c = super::resolve_card(db, p)?;
            if p.get("revision").is_some_and(|v| !v.is_null()) {
                revise(&c, p)?;
            }
            ensure!(c.active(), "Restore the Card before resolving");
            emit_card = Some(c.id.clone());
            if c.resolved() {
                return Ok(serde_json::to_value(c)?);
            }
            let profile_opt = p["profile_id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let runtime_terminal = p["runtime_terminal"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let is_agent = p["agent"] == true || profile_opt.is_some();
            let terminals = db.list::<TerminalEntry>()?;
            let active_entry = if is_agent {
                runtime_terminal
                    .and_then(|rt| {
                        terminals.iter().find(|t| {
                            t.agent.as_ref().is_some_and(|a| {
                                a.card_id == c.id
                                    && a.terminal.as_deref() == Some(rt)
                                    && !a.finished()
                            })
                        })
                    })
                    .or_else(|| {
                        terminals.iter().rev().find(|t| {
                            t.agent.as_ref().is_some_and(|a| {
                                a.card_id == c.id
                                    && !a.finished()
                                    && profile_opt.is_none_or(|pid| a.profile.id == pid)
                            })
                        })
                    })
                    .cloned()
            } else {
                None
            };
            let actor = if is_agent {
                let pid = active_entry
                    .as_ref()
                    .and_then(|t| t.agent.as_ref())
                    .map(|a| a.profile.id.as_str())
                    .or(profile_opt)
                    .unwrap_or(&config.default_agent);
                format!("agent:{pid}")
            } else {
                "user".to_owned()
            };
            let reason = p["message"]
                .as_str()
                .or_else(|| p["reason"].as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let resolved_at = now();
            let kind = if is_agent { "agent" } else { "manual" };
            c.resolution = Some(CardResolution {
                resolved_at,
                kind: kind.into(),
                reason: reason.clone(),
                actor: actor.clone(),
            });
            c.attention = None;
            c.human_gate = None;
            c.revision += 1;
            db.save(&c)?;

            for mut n in db
                .list::<Notice>()?
                .into_iter()
                .filter(|n| n.card_id == c.id && !n.read)
            {
                n.read = true;
                db.save(&n)?;
            }

            crate::workflow::stop_card(db, &config, &c.id, "cancelled")?;
            for mut entry in db.list::<TerminalEntry>()? {
                if let Some(a) = &mut entry.agent
                    && a.card_id == c.id
                {
                    let mut changed = false;
                    if a.state == "queued" {
                        a.state = "cancelled".into();
                        changed = true;
                    }
                    if a.pending.take().is_some() {
                        changed = true;
                    }
                    if let Some(caller) = &active_entry
                        && entry.id == caller.id
                        && matches!(
                            a.state.as_str(),
                            "starting" | "running" | "working" | "blocked" | "needs_report"
                        )
                    {
                        a.state = "completed".into();
                        a.completed_at = Some(resolved_at);
                        a.updated_at = resolved_at;
                        a.close_state = String::new();
                        changed = true;
                    }
                    if changed {
                        db.save(&entry)?;
                    }
                }
            }

            let event_msg = match &reason {
                Some(r) => format!("Resolved: {r}"),
                None => "Card resolved".into(),
            };
            log(
                db,
                &c,
                "card.resolved",
                &actor,
                &event_msg,
                active_entry.as_ref().map(|t| t.id.clone()),
                json!({
                    "kind": kind,
                    "reason": reason,
                    "resolved_at": resolved_at,
                }),
            )?;
            Ok(serde_json::to_value(c)?)
        })?;
        if let Some(cid) = emit_card.as_deref() {
            let _ = self.try_auto_archive_resolved(cid);
        }
        if p["card_id"].is_null()
            && let Some(id) = emit_card
        {
            self.emit("application", Some(id));
        }
        Ok(res)
    }

    pub(super) fn reopen_card_action(&self, p: &Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Reopening a Card requires an explicit user decision"
        );
        let mut emit_card = p["card_id"].as_str().map(str::to_owned);
        let res = self.mutate("card.reopen", p, |db| {
            let mut c = super::resolve_card(db, p)?;
            if p.get("revision").is_some_and(|v| !v.is_null()) {
                revise(&c, p)?;
            }
            ensure!(c.active(), "Restore the Card before reopening");
            ensure!(!c.merged(), "Cards with merged code cannot be reopened");
            emit_card = Some(c.id.clone());
            if c.resolution.is_none() {
                return Ok(serde_json::to_value(c)?);
            }
            c.resolution = None;
            c.revision += 1;
            db.save(&c)?;
            log(
                db,
                &c,
                "card.reopened",
                "user",
                "Card reopened",
                None,
                json!({}),
            )?;
            Ok(serde_json::to_value(c)?)
        })?;
        if p["card_id"].is_null()
            && let Some(id) = emit_card
        {
            self.emit("application", Some(id));
        }
        Ok(res)
    }
}
