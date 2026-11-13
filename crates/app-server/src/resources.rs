use super::*;

impl AppServer {
    pub(super) fn read_resource(&self, card_id: &str, path: &str) -> Result<Value> {
        let c: Card = self.store.lock().unwrap().db().get(card_id)?;
        let root = canban_context::card_dir(&self.home, &c.project_id, &c.id);
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
            for mut r in db
                .list::<Request>()?
                .into_iter()
                .filter(|r| r.card_id == c.id && r.state == "queued")
            {
                r.state = "cancelled".into();
                db.save(&r)?;
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
        for run in self
            .snapshot()?
            .attempts
            .iter()
            .filter(|a| a.card_id == card_id && a.terminal.is_some())
        {
            let _ = self.capture_output(run);
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
            configuration_sessions: Vec::new(),
            event_subscriptions: Vec::new(),
            workflow_runs: snapshot
                .workflow_runs
                .iter()
                .filter(|r| r.card_id == card_id)
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
                    v.conversation_id.as_ref().is_some_and(|id| {
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
            events: snapshot
                .events
                .into_iter()
                .filter(|v| v.card_id == card_id)
                .collect(),
            requests: snapshot
                .requests
                .into_iter()
                .filter(|v| v.card_id == card_id)
                .collect(),
            attempts: snapshot
                .attempts
                .into_iter()
                .filter(|v| v.card_id == card_id)
                .collect(),
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
        };
        // Repack after restore first, so previously compressed bytes are not omitted.
        if let Some(old) = &card.archive {
            canban_context::restore_files(&self.home, &card, old)?;
        }
        let result = canban_context::pack(&self.home, &card, &filtered);
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
        let attempts = self.snapshot()?.attempts;
        for run in attempts
            .iter()
            .filter(|a| a.card_id == card_id && a.terminal.is_some() && !a.finished())
        {
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
            self.capture_output(run)?;
            self.store.lock().unwrap().write(|db| {
                let mut a: Attempt = db.get(&run.id)?;
                a.observed = Some(canban_herdr::observation(&actual));
                a.close_state = "closed".into();
                a.close_requested_at = Some(now());
                a.lifecycle.close_reason = "archive".into();
                a.connection_state = "connected".into();
                db.save(&a)?;
                let mut r: Request = db.get(&run.request_id)?;
                if !matches!(r.state.as_str(), "completed" | "backlog") {
                    r.state = "cancelled".into();
                    db.save(&r)?;
                }
                log(
                    db,
                    &db.get::<Card>(card_id)?,
                    "execution.archived",
                    "system",
                    "Closed Card Agent after saving its terminal history",
                    Some(run.request_id.clone()),
                    json!({"attempt":run.number}),
                )
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
                if snapshot.attempts.iter().any(|a| {
                    a.card_id == card.id
                        && a.cwd == w.path
                        && a.location
                            .as_ref()
                            .is_some_and(|l| l.machine == w.location.machine)
                        && a.terminal.is_some()
                        && a.observed.as_ref().is_none_or(|v| v["state"] != "finished")
                }) || self.terminal_retains_worktree(&c)?
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
        let run: Attempt = self.store.lock().unwrap().db().get(id)?;
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
    }
    pub(super) fn capture_output(&self, run: &Attempt) -> Result<()> {
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
        let req: Request = self.store.lock().unwrap().db().get(&run.request_id)?;
        let path = canban_context::run_dir(&self.home, &req, run.number)
            .join(format!("output-{}.txt", id()));
        canban_context::atomic(&path, text.as_bytes())?;
        let relative = canban_context::relative(&self.home, &path)?;
        self.store.lock().unwrap().write(|db| {
            let mut current: Attempt = db.get(&run.id)?;
            current.output_path = Some(relative);
            db.save(&current)
        })?;
        Ok(())
    }
}
