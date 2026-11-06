use super::*;

impl AppServer {
    pub fn call(&self, method: &str, p: Value) -> Result<Value> {
        let config = self.config();
        match method {
            "app.ping" => Ok(json!({"version":env!("CARGO_PKG_VERSION"),"home":self.home})),
            "app.snapshot" => Ok(serde_json::to_value(self.snapshot()?)?),
            "project.list" => Ok(serde_json::to_value(
                self.store.lock().unwrap().db().list::<Project>()?,
            )?),
            "card.get" => Ok(serde_json::to_value(
                self.store
                    .lock()
                    .unwrap()
                    .db()
                    .get::<Card>(strp(&p, "card_id")?)?,
            )?),
            "card.list" => {
                let cards = self.store.lock().unwrap().db().list::<Card>()?;
                Ok(serde_json::to_value(
                    cards
                        .into_iter()
                        .filter(|c| p["project_id"].as_str().is_none_or(|id| c.project_id == id))
                        .collect::<Vec<_>>(),
                )?)
            }
            "timeline.list" => {
                let card = strp(&p, "card_id")?;
                let after = p["after_sequence"].as_u64().unwrap_or(0);
                let limit = p["limit"].as_u64().unwrap_or(100).clamp(1, 500) as usize;
                let events = self.store.lock().unwrap().db().list::<Event>()?;
                let mut events = events
                    .into_iter()
                    .filter(|e| e.card_id == card && e.sequence > after)
                    .take(limit + 1)
                    .collect::<Vec<_>>();
                let more = events.len() > limit;
                events.truncate(limit);
                Ok(
                    json!({"next_cursor":if more{events.last().map(|e|e.sequence)}else{None},"events":events}),
                )
            }
            "execution.get" => {
                let request: Request = self
                    .store
                    .lock()
                    .unwrap()
                    .db()
                    .get(strp(&p, "request_id")?)?;
                let s = self.snapshot()?;
                Ok(
                    json!({"attempts":s.attempts.into_iter().filter(|a|a.request_id==request.id).collect::<Vec<_>>(),"reports":s.reports.into_iter().filter(|r|r.request_id==request.id).collect::<Vec<_>>(),"request":request}),
                )
            }
            "config.get" => Ok(serde_json::to_value(config)?),
            "config.reload" => {
                let c: Config =
                    serde_json::from_slice(&std::fs::read(self.home.join("config/agents.json"))?)?;
                Self::validate_config(&c)?;
                *self.config.write().unwrap() = c;
                self.emit("configuration", None);
                Ok(json!({"reloaded":true}))
            }
            "herdr.status" => canban_herdr::Client { config }.status(),
            "herdr.templates" => canban_herdr::Client { config }.call("templates.list", json!({})),
            "project.create" => {
                let workflow = if p["workflow"].is_object() {
                    serde_json::from_value(p["workflow"].clone())?
                } else {
                    serde_json::from_slice(&std::fs::read(
                        self.home.join("config/workflows/default.json"),
                    )?)?
                };
                canban_workflow_engine::validate(&workflow)?;
                let project =
                    canban_kanban::project(strp(&p, "name")?, strp(&p, "directory")?, workflow)?;
                self.mutate(method, &p, |db| {
                    db.save(&project)?;
                    Ok(serde_json::to_value(project)?)
                })
            }
            "card.create" => self.mutate(method, &p, |db| {
                let project: Project = db.get(strp(&p, "project_id")?)?;
                let number = db.counter(&format!("cards:{}", project.id))?;
                let c = canban_kanban::card(
                    &project,
                    number,
                    strp(&p, "title")?,
                    p["body"].as_str().unwrap_or(""),
                    strings(&p, "tags"),
                )?;
                db.save(&c)?;
                log(
                    db,
                    &c,
                    "card.created",
                    "user",
                    "Card created",
                    None,
                    json!({"title":c.title,"body":c.body}),
                )?;
                Ok(serde_json::to_value(c)?)
            }),
            "card.update" => self.mutate(method, &p, |db| {
                let mut c: Card = db.get(strp(&p, "card_id")?)?;
                revise(&c, &p)?;
                ensure!(c.archived_at.is_none(), "Restore the Card before editing");
                if let Some(t) = p["title"].as_str() {
                    ensure!(!t.trim().is_empty(), "Title is required");
                    c.title = t.trim().into();
                }
                if let Some(b) = p["body"].as_str() {
                    c.body = b.into();
                }
                if p.get("tags").is_some() {
                    c.tags = strings(&p, "tags");
                }
                c.revision += 1;
                log(
                    db,
                    &c,
                    "card.updated",
                    "user",
                    "Card updated",
                    None,
                    json!({"title":c.title,"body":c.body,"tags":c.tags}),
                )?;
                db.save(&c)?;
                Ok(serde_json::to_value(c)?)
            }),
            "card.move" => self.mutate(method, &p, |db| {
                let mut c: Card = db.get(strp(&p, "card_id")?)?;
                revise(&c, &p)?;
                ensure!(c.archived_at.is_none(), "Restore the Card before moving");
                let target = strp(&p, "stage")?;
                if c.stage == target {
                    return Ok(serde_json::to_value(c)?);
                }
                let project: Project = db.get(&c.project_id)?;
                let stage = canban_workflow_engine::stage(&project.workflow, target)?;
                let from = c.stage.clone();
                // Ordinary queued work from a stage the user has left must not
                // start later. Already-running executions are deliberately not drained.
                for mut request in db.list::<Request>()?.into_iter().filter(|r| {
                    r.card_id == c.id && r.state == "queued" && r.trigger == "stage_entry"
                }) {
                    request.state = "cancelled".into();
                    db.save(&request)?;
                }
                c.stage = target.into();
                c.revision += 1;
                c.attention = None;
                log(
                    db,
                    &c,
                    "card.stage_changed",
                    "user",
                    &format!("{from} → {target}"),
                    None,
                    json!({"from":from,"to":target}),
                )?;
                enqueue(
                    db,
                    &config,
                    &mut c,
                    &project,
                    stage,
                    "stage_entry",
                    None,
                    None,
                )?;
                db.save(&c)?;
                Ok(serde_json::to_value(c)?)
            }),
            "card.context.update" => self.mutate(method, &p, |db| {
                let mut card: Card = db.get(strp(&p, "card_id")?)?;
                revise(&card, &p)?;
                ensure!(
                    card.archived_at.is_none(),
                    "Restore the Card before editing context"
                );
                let notes = strings(&p, "note_ids");
                for id in &notes {
                    let note: Note = db.get(id)?;
                    ensure!(
                        note.project_id == card.project_id,
                        "Context Note belongs to a different Project"
                    );
                }
                card.context_notes = Some(notes.clone());
                card.revision += 1;
                db.save(&card)?;
                log(
                    db,
                    &card,
                    "card.context_updated",
                    "user",
                    "Context Note selection updated",
                    None,
                    json!({"note_ids":notes}),
                )?;
                Ok(serde_json::to_value(card)?)
            }),
            "comment.add" => self.mutate(method, &p, |db| {
                let mut c: Card = db.get(strp(&p, "card_id")?)?;
                ensure!(
                    c.archived_at.is_none(),
                    "Restore the Card before commenting"
                );
                let body = strp(&p, "body")?;
                ensure!(!body.trim().is_empty(), "Comment is empty");
                let event = db.append(canban_timeline::event(
                    &c.project_id,
                    &c.id,
                    "comment.created",
                    "user",
                    body,
                    None,
                    json!({"body":body}),
                ))?;
                let project: Project = db.get(&c.project_id)?;
                let stage = canban_workflow_engine::stage(&project.workflow, &c.stage)?;
                let mut mentions = strings(&p, "mentions");
                for word in body.split_whitespace() {
                    if let Some(name) = word.strip_prefix('@') {
                        let name = name.trim_end_matches(|ch: char| {
                            !ch.is_alphanumeric() && ch != '-' && ch != '_'
                        });
                        if config.profiles.iter().any(|a| a.id == name) {
                            mentions.push(name.into());
                        }
                    }
                }
                mentions.sort();
                mentions.dedup();
                for agent in mentions {
                    enqueue(
                        db,
                        &config,
                        &mut c,
                        &project,
                        stage,
                        &format!("comment:{}", event.id),
                        Some(&agent),
                        Some(format!(
                            "Answer this comment/question in the context of the Card.\n\n{body}"
                        )),
                    )?;
                }
                c.revision += 1;
                db.save(&c)?;
                Ok(serde_json::to_value(event)?)
            }),
            "card.attention.clear" => self.mutate(method, &p, |db| {
                let mut c: Card = db.get(strp(&p, "card_id")?)?;
                c.attention = None;
                c.revision += 1;
                db.save(&c)?;
                log(
                    db,
                    &c,
                    "attention.cleared",
                    "user",
                    "Attention acknowledged",
                    None,
                    json!({}),
                )?;
                for mut n in db
                    .list::<Notice>()?
                    .into_iter()
                    .filter(|n| n.card_id == c.id && !n.read)
                {
                    n.read = true;
                    db.save(&n)?;
                }
                Ok(json!({"ok":true}))
            }),
            "note.save" => self.mutate(method, &p, |db| {
                let project: Project = db.get(strp(&p, "project_id")?)?;
                let mut note = if let Some(id) = p["note_id"].as_str() {
                    let n: Note = db.get(id)?;
                    ensure!(n.project_id == project.id, "Note is in a different Project");
                    ensure!(
                        p["revision"].as_u64() == Some(n.revision),
                        "Note revision conflict"
                    );
                    n
                } else {
                    Note {
                        id: id(),
                        project_id: project.id,
                        title: String::new(),
                        body: String::new(),
                        revision: 0,
                        created_at: now(),
                    }
                };
                note.title = strp(&p, "title")?.into();
                note.body = strp(&p, "body")?.into();
                note.revision += 1;
                db.save(&note)?;
                Ok(serde_json::to_value(note)?)
            }),
            "execution.report" => self.report(p),
            "execution.retry" => self.mutate(method, &p, |db| {
                let mut req: Request = db.get(strp(&p, "request_id")?)?;
                let c: Card = db.get(&req.card_id)?;
                ensure!(c.archived_at.is_none(), "Restore the Card before retrying");
                ensure!(
                    !matches!(
                        req.state.as_str(),
                        "queued" | "starting" | "running" | "working" | "completed"
                    ),
                    "Execution is not retryable"
                );
                req.state = "queued".into();
                db.save(&req)?;
                log(
                    db,
                    &c,
                    "execution.retry_queued",
                    "user",
                    "Fresh attempt queued",
                    Some(req.id.clone()),
                    json!({}),
                )?;
                Ok(serde_json::to_value(req)?)
            }),
            "execution.output" => self.output(strp(&p, "attempt_id")?),
            "resource.read" => self.read_resource(strp(&p, "card_id")?, strp(&p, "path")?),
            "resource.materialize" => {
                let card: Card = self.store.lock().unwrap().db().get(strp(&p, "card_id")?)?;
                let path = strp(&p, "path")?;
                let full = self.home.join(path);
                let root = canban_context::card_dir(&self.home, &card.project_id, &card.id);
                ensure!(
                    full.starts_with(root)
                        && !Path::new(path)
                            .components()
                            .any(|c| matches!(c, std::path::Component::ParentDir)),
                    "Resource outside Card"
                );
                if full.exists() && full.extension().is_some() {
                    Ok(json!({"path":full}))
                } else {
                    let bytes = if full.exists() {
                        std::fs::read(&full)?
                    } else {
                        canban_context::archived_file(&self.home, &card, path)?
                    };
                    let suffix = self
                        .store
                        .lock()
                        .unwrap()
                        .db()
                        .list::<Artifact>()?
                        .iter()
                        .find(|a| a.card_id == card.id && a.path == path)
                        .map(|a| canban_context::suffix(&a.name))
                        .unwrap_or_else(|| canban_context::suffix(path));
                    let cache = self.home.join("cache").join(format!(
                        "{}{}",
                        canban_context::digest(path.as_bytes()),
                        suffix
                    ));
                    canban_context::atomic(&cache, &bytes)?;
                    Ok(json!({"path":cache}))
                }
            }
            "attachment.add" => self.add_attachment(&p),
            "card.archive" => self.archive(&p),
            "card.restore" => self.restore(&p),
            _ => bail!("Unknown operation: {method}"),
        }
    }
}
