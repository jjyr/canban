use super::*;

impl AppServer {
    pub fn call(&self, method: &str, p: Value) -> Result<Value> {
        if method.starts_with("github.") || method == "project.github" || method == "card.remove" {
            return self.github_call(method, p);
        }
        let config = self.config();
        match method {
            "app.shutdown" => {
                self.stop.store(true, Ordering::SeqCst);
                Ok(json!({"stopping":true}))
            }
            "app.state" => {
                Ok(json!({"snapshot":self.snapshot()?,"config":self.config(),"home":self.home}))
            }
            "machine.list" => self.machines(),
            "machine.default" => {
                let machine = self.resolve_machine(strp(&p, "machine")?, false)?;
                let mut active = self.config.write().unwrap();
                let path = self.home.join("config/agents.json");
                let mut raw: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
                raw["default_machine"] = json!(machine);
                canban_context::atomic(&path, &serde_json::to_vec_pretty(&raw)?)?;
                active.default_machine = machine.clone();
                drop(active);
                self.emit("configuration", None);
                Ok(json!({"machine":machine}))
            }
            "machine.add" => self.add_machine(p),
            "machine.remove" => self.remove_machine(p),
            "directory.list" => self.browse_directory(p),
            "project.location" => self.update_location(p),
            "terminal.create" => self.create_terminal(p),
            "terminal.rename" => self.rename_terminal(p),
            "terminal.interrupt" => self.control_terminal(p, false),
            "terminal.end" => self.control_terminal(p, true),
            "terminal.close" => self.close_terminal(p),
            "terminal.context" => self.terminal_context(p),
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
                        .filter(|c| c.removed_at.is_none() || p["include_removed"] == true)
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
            "overview.list" => Ok(serde_json::to_value(canban_notifications::overview(
                &self.snapshot()?,
            ))?),
            "overview.preview" => self.overview_preview(p),
            "scheduler.status" => {
                let snapshot = self.snapshot()?;
                Ok(
                    json!({"active":canban_scheduler::active_count(&snapshot),"max_agents":config.scheduler.max_agents,"queued":snapshot.requests.iter().filter(|r| r.state=="queued").count()}),
                )
            }
            "scheduler.configure" => {
                let mut active = self.config.write().unwrap();
                let mut settings = active.scheduler.clone();
                if let Some(max) = p.get("max_agents") {
                    settings.max_agents = usize::try_from(
                        max.as_u64()
                            .context("max_agents must be a positive integer")?,
                    )?;
                }
                if let Some(minutes) = p.get("idle_minutes") {
                    settings.idle_minutes = minutes
                        .as_u64()
                        .context("idle_minutes must be a nonnegative integer")?;
                    ensure!(
                        settings.idle_minutes <= 525600,
                        "idle_minutes must be at most one year"
                    );
                }
                ensure!(
                    settings.max_agents > 0,
                    "max_agents must be greater than zero"
                );
                let path = self.home.join("config/agents.json");
                let mut raw: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
                if !raw["scheduler"].is_object() {
                    raw["scheduler"] = json!({});
                }
                raw["scheduler"]["max_agents"] = json!(settings.max_agents);
                raw["scheduler"]["idle_minutes"] = json!(settings.idle_minutes);
                canban_context::atomic(&path, &serde_json::to_vec_pretty(&raw)?)?;
                active.scheduler = settings.clone();
                drop(active);
                self.emit("configuration", None);
                Ok(serde_json::to_value(settings)?)
            }

            "config.reload" => {
                let (c, migration) = Self::read_config(&self.home)?;
                self.validate_references(&c)?;
                Self::publish_migration(&self.home, migration)?;
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
                Self::validate_workflow(&config, &workflow)?;
                let location = self.work_location(&p)?;
                let name = strp(&p, "name")?.trim();
                ensure!(!name.is_empty(), "Project name is required");
                let project = Project {
                    github: None,
                    delivery_mode: Default::default(),
                    id: id(),
                    name: name.into(),
                    directory: location.directory.clone(),
                    location: Some(location.clone()),
                    workflow,
                    target_branch: p["target_branch"].as_str().map(str::to_owned).or_else(|| {
                        self.ops(&location.machine)
                            .current_branch(&location.directory)
                    }),
                    created_at: now(),
                };
                self.mutate(method, &p, |db| {
                    db.save(&project)?;
                    Ok(serde_json::to_value(project)?)
                })
            }
            "card.create" => self.mutate(method, &p, |db| {
                let project: Project = db.get(strp(&p, "project_id")?)?;
                let mut c = canban_kanban::create(
                    db,
                    &project,
                    strp(&p, "title")?,
                    p["body"].as_str().unwrap_or(""),
                    strings(&p, "tags"),
                    "user",
                )?;
                if let Some(note_id) = p["source_note_id"].as_str() {
                    let note: Note = db.get(note_id)?;
                    ensure!(
                        note.project_id == project.id,
                        "Source Note belongs to a different Project"
                    );
                    c.context_notes = Some(vec![note.id.clone()]);
                    db.save(&c)?;
                    crate::notes::relate_card(db, &note.id, &c, true)?;
                } else {
                    db.save(&c)?;
                }
                Ok(serde_json::to_value(c)?)
            }),
            "card.update" => self.mutate(method, &p, |db| {
                let mut c: Card = db.get(strp(&p, "card_id")?)?;
                revise(&c, &p)?;
                ensure!(c.archived_at.is_none(), "Restore the Card before editing");
                ensure!(
                    c.removed_at.is_none(),
                    "Restore the removed Card before editing"
                );
                if let Some(t) = p["title"].as_str() {
                    ensure!(
                        c.github_issue().is_none() || t == c.title,
                        "GitHub Issue titles are source content; edit local instructions instead"
                    );
                    ensure!(!t.trim().is_empty(), "Title is required");
                    c.title = t.trim().into();
                }
                if let Some(b) = p["body"].as_str() {
                    c.body = b.into();
                }
                if p.get("tags").is_some() {
                    c.tags = strings(&p, "tags");
                }
                if let Some(mode) = p.get("delivery_mode") {
                    c.delivery_mode = serde_json::from_value(mode.clone())?;
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
                ensure!(c.active(), "Restore the Card before moving");
                ensure!(
                    c.human_gate.is_none() || p["execution"].is_null(),
                    "Human review must be approved by the user before moving this Card"
                );
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
                c.human_gate = None;
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
            "card.continue" => self.continue_card(p),
            "card.workspace" => {
                let (card, project) = {
                    let store = self.store.lock().unwrap();
                    let card: Card = store.db().get(strp(&p, "card_id")?)?;
                    let project: Project = store.db().get(&card.project_id)?;
                    (card, project)
                };
                let project = self.bound_project(&project, &card);
                Ok(serde_json::to_value(
                    self.ops(&project.work_location().machine)
                        .info(&project, &card)?,
                )?)
            }
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
                let previously = card.context_notes.clone().unwrap_or_default();
                for id in &previously {
                    if !notes.contains(id) {
                        crate::notes::relate_card(db, id, &card, false)?;
                    }
                }
                for id in &notes {
                    crate::notes::relate_card(db, id, &card, true)?;
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
                ensure!(c.active(), "Restore the Card before commenting");
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
                    let profile = canban_scheduler::select(&config, &[], Some(&agent))?;
                    let _context = crate::sessions::conversation(db, &c, &profile)?;
                    let active = db.list::<Request>()?.into_iter().rev().find(|r| {
                        r.card_id == c.id
                            && !(c.human_gate.is_some()
                                && r.state == "queued"
                                && canban_scheduler::automatic(r))
                            && r.profile.id == agent
                            && matches!(
                                r.state.as_str(),
                                "queued" | "starting" | "running" | "working" | "blocked"
                            )
                    });
                    let req = if let Some(req) = active {
                        req
                    } else {
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
                        )?
                        .context("Question not queued")?
                    };
                    let question = Question {
                        id: format!("{}:{}", event.id, agent),
                        project_id: c.project_id.clone(),
                        card_id: c.id.clone(),
                        profile_id: agent,
                        comment_id: event.id.clone(),
                        body: body.into(),
                        request_id: req.id.clone(),
                        attempt: req.attempt,
                        state: "pending".into(),
                        error: None,
                        reply_path: None,
                        created_at: now(),
                    };
                    db.save(&question)?;
                }
                c.revision += 1;
                db.save(&c)?;
                Ok(serde_json::to_value(event)?)
            }),
            "card.attention.clear" => self.mutate(method, &p, |db| {
                let mut c: Card = db.get(strp(&p, "card_id")?)?;
                if c.human_gate.is_none() {
                    c.attention = None;
                }
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
            "note.list" | "note.get" | "note.read" | "note.history" | "note.revision" => {
                self.note_read(method, &p)
            }
            "note.save" | "note.write" | "note.rename" => self.note_write(method, p),
            "review.preview" => self.review_preview(p),
            "review.merge" => self.review_merge(p),
            "agent.open" => self.open_agent(p),
            "agent.bind" => self.bind_session(p),
            // Old in-flight hook commands can finish, but never drive lifecycle.
            "agent.turn_ended" => Ok(json!({"deprecated":true,"recorded":false,"source":"herdr"})),
            "profile.presets" => Ok(serde_json::to_value(canban_profiles::presets(&self.home)?)?),
            "agent.reply" => self.reply(p),
            "agent.interrupt" => self.control_agent(p, false),
            "agent.end" => self.control_agent(p, true),
            "execution.report" => self.report(p),
            "execution.retry" => self.mutate(method, &p, |db| {
                let mut req: Request = db.get(strp(&p, "request_id")?)?;
                let mut c: Card = db.get(&req.card_id)?;
                ensure!(
                    p["execution"].is_null(),
                    "Retry requires an explicit user decision"
                );
                ensure!(c.archived_at.is_none(), "Restore the Card before retrying");
                ensure!(
                    !matches!(
                        req.state.as_str(),
                        "queued" | "starting" | "running" | "working" | "completed"
                    ),
                    "Execution is not retryable"
                );
                if let Ok(mut previous) = db.get::<Attempt>(&Attempt::key(&req.id, req.attempt)) {
                    ensure!(
                        previous.terminal.is_none()
                            || previous
                                .observed
                                .as_ref()
                                .is_some_and(|v| v["state"] == "finished"),
                        "End the previous Agent before retrying"
                    );
                    if previous.state == "unknown" {
                        previous.state = "superseded".into();
                        db.save(&previous)?;
                    }
                }
                req.state = "queued".into();
                c.human_gate = None;
                c.attention = None;
                c.revision += 1;
                db.save(&c)?;
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
