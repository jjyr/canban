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
                let notice = match canban_workspaces::archive(&self.home, &project, &card) {
                    Ok(n) => n,
                    Err(e) => Some(format!("Archive saved; worktree retained: {e:#}")),
                };
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
        if c.archived_at.is_some()
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
