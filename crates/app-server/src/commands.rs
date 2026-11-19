use super::*;

impl AppServer {
    pub fn call(&self, method: &str, p: Value) -> Result<Value> {
        representation::validate(method, &p)?;
        let mut business = p.clone();
        business.as_object_mut().unwrap().remove("view");
        business.as_object_mut().unwrap().remove("fields");
        let result = self.call_business(method, business)?;
        Ok(representation::render(method, &p, result))
    }

    fn call_business(&self, method: &str, p: Value) -> Result<Value> {
        if canban_updater::handoff::active(&self.home)
            && !matches!(
                method,
                "app.ping"
                    | "app.shutdown"
                    | "app.state"
                    | "app.watch"
                    | "app.snapshot"
                    | "host.probe"
                    | "host.discover"
                    | "diagnostics.status"
                    | "diagnostics.logs"
                    | "update.state"
                    | "utilities.state"
                    | "machine.list"
                    | "usage.status"
                    | "config.get"
                    | "profile.presets"
                    | "agent.catalog"
                    | "draft.list"
                    | "herdr.check"
                    | "project.detect_github"
                    | "workflow.default.get"
            )
        {
            bail!("Canban is upgrading; retry after the service handoff");
        }
        if method.starts_with("update.") {
            return self.update_call(method, p);
        }

        if matches!(
            method,
            "workflow.sources"
                | "workflow.apply"
                | "workflow.check"
                | "workflow.prompt"
                | "workflow.default.get"
                | "workflow.default.set"
                | "workflow.default.reset"
        ) {
            return self.authoring_call(method, p);
        }
        if method.starts_with("configuration.") {
            return self.configuration_call(method, p);
        }
        if method.starts_with("schedule.") {
            return self.schedule_call(method, p);
        }
        if method.starts_with("workflow.") {
            return self.workflow_call(method, p);
        }
        if method.starts_with("event.") || method.starts_with("subscription.") {
            return self.event_call(method, p);
        }
        if method.starts_with("github.")
            || matches!(method, "project.github" | "project.detect_github")
        {
            return self.github_call(method, p);
        }
        if method.starts_with("utilities.") || method.starts_with("usage.") {
            return self.utility_call(method, p);
        }
        if method.starts_with("draft.") {
            return self.draft_call(method, p);
        }
        if method.starts_with("worktree.") {
            return self.worktree_call(method, p);
        }
        if method.starts_with("diagnostics.") {
            return self.diagnostics_call(method, p);
        }
        if method.starts_with("git.") {
            return self.git_call(method, p);
        }
        let config = self.config();
        match method {
            "notifications.read" => self.read_card(p),
            "notifications.read_all" => self.read_all_cards(p),
            "notifications.list" => Ok(serde_json::to_value(canban_notifications::reminders(
                &self.snapshot()?,
            ))?),
            "app.shutdown" => {
                if let Some(expected) = p["expected_pid"].as_u64() {
                    ensure!(
                        expected == u64::from(std::process::id()),
                        "AppServer identity changed; refusing shutdown"
                    );
                }
                self.stop.store(true, Ordering::SeqCst);
                self.close_active_tcp_streams();
                self.wake_runtime();
                self.context_signal.notify_all();
                Ok(json!({"stopping":true}))
            }
            "app.state" => self.state_response(&p, true),
            "app.watch" => self.state_response(&p, false),
            "host.probe" => Ok(self.host_probe_payload()),
            "host.discover" => Ok(serde_json::to_value(network::discover_all())?),
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
            "project.agents" => self.update_project_agents(p),
            "agent.catalog" => self.agent_catalog(p),
            "agent.setup" => self.agent_setup(p),
            "config.agents.save" => self.save_agents_config(p),
            "terminal.create" => self.create_terminal(p),
            "terminal.rename" => self.rename_terminal(p),
            "terminal.interrupt" => self.control_terminal(p, false),
            "terminal.end" => self.control_terminal(p, true),
            "terminal.close" => self.close_terminal(p),
            "terminal.context" => self.terminal_context(p),
            "runtime.observation" => {
                let (core, generation, connected, terminals) = {
                    let inv = self.inventory.lock().unwrap();
                    (
                        inv.core.clone(),
                        inv.generation,
                        inv.error.is_none() && inv.last_at > 0,
                        inv.records.len(),
                    )
                };
                let q = self.contexts.lock().unwrap();
                Ok(
                    json!({"core_id":core,"generation":generation,"connected":connected,"terminal_count":terminals,"context_probes":q.probes(),"pending_contexts":q.pending_count(),"refine_probes":self.refine_probes(),"refined_working":self.refined_working_count(),"check_limit":4,"checks_running":self.check_slots.lock().unwrap().len()}),
                )
            }
            "app.ping" => Ok(
                json!({"version":env!("CARGO_PKG_VERSION"),"api_version":API_VERSION,"pid":std::process::id(),"home":self.home,"server_id":self.identity,"hostname":network::hostname_string(),"build":canban_updater::Build::current(),"updates_disabled":self.updates_disabled}),
            ),
            "app.snapshot" => Ok(serde_json::to_value(self.snapshot()?)?),
            "project.list" => Ok(serde_json::to_value(
                self.store.lock().unwrap().db().list::<Project>()?,
            )?),
            "card.get" | "card.show" => {
                let store = self.store.lock().unwrap();
                Ok(serde_json::to_value(super::resolve_card(store.db(), &p)?)?)
            }
            "card.list" => self.card_page(&p),
            "timeline.list" => self.timeline_page(&p),
            "execution.get" => {
                let s = self.snapshot()?;
                if let Some(tid) = p["terminal_id"].as_str().filter(|v| !v.is_empty()) {
                    let terminal = s
                        .terminals
                        .iter()
                        .find(|t| t.id == tid)
                        .cloned()
                        .context("Terminal not found")?;
                    let reports = s
                        .reports
                        .into_iter()
                        .filter(|r| r.terminal_id.as_deref() == Some(tid))
                        .collect::<Vec<_>>();
                    Ok(json!({"terminal":terminal,"terminals":[terminal],"reports":reports}))
                } else {
                    let card = {
                        let store = self.store.lock().unwrap();
                        super::resolve_card(store.db(), &p)?
                    };
                    let terminals = s
                        .terminals
                        .into_iter()
                        .filter(|t| t.agent.as_ref().is_some_and(|a| a.card_id == card.id))
                        .collect::<Vec<_>>();
                    let reports = s
                        .reports
                        .into_iter()
                        .filter(|r| r.card_id == card.id)
                        .collect::<Vec<_>>();
                    Ok(json!({"terminal":terminals.last(),"terminals":terminals,"reports":reports}))
                }
            }
            "config.get" => Ok(serde_json::to_value(config)?),
            "overview.list" => Ok(serde_json::to_value(canban_notifications::overview(
                &self.snapshot()?,
            ))?),
            "overview.preview" => self.overview_preview(p),
            "scheduler.status" => {
                let snapshot = self.snapshot()?;
                let queued = snapshot
                    .terminals
                    .iter()
                    .filter_map(|t| t.agent.as_ref())
                    .filter(|a| a.state == "queued" || a.pending.is_some())
                    .count()
                    + snapshot
                        .scheduled_runs
                        .iter()
                        .filter(|r| !r.creates_card && r.state == "queued")
                        .count();
                Ok(
                    json!({"active":canban_scheduler::active_count(&snapshot),"max_agents":config.scheduler.max_agents,"queued":queued}),
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
                canban_herdr::invalidate_local_shell_path_cache();
                let (c, migration) = Self::read_config(&self.home)?;
                self.validate_references(&c)?;
                Self::publish_migration(&self.home, migration)?;
                *self.config.write().unwrap() = c;
                self.emit("configuration", None);
                Ok(json!({"reloaded":true}))
            }
            "herdr.status" => canban_herdr::Client { config }.status(),
            "herdr.check" => self.herdr_check(p),
            "herdr.install" => self.herdr_install(p),
            "herdr.templates" => canban_herdr::Client { config }.call("templates.list", json!({})),
            "project.create" => {
                ensure!(
                    p.get("workflow").is_none(),
                    "JSON workflow templates have been removed; pass workspace and/or workflow_yaml, or publish YAML via workflow.apply"
                );
                let mut workflow = Workflow::default();
                let explicit_workspace: Option<WorkspaceSettings> = match p.get("workspace") {
                    Some(ws) if !ws.is_null() => Some(serde_json::from_value(ws.clone())?),
                    _ => None,
                };
                if let Some(ws) = &explicit_workspace {
                    workflow.workspace = ws.clone();
                }
                canban_workflow_engine::validate(&workflow)?;
                let workflow_yaml = match p.get("workflow_yaml") {
                    Some(v) if !v.is_null() => Some(
                        v.as_str()
                            .context("workflow_yaml must be a YAML string")?
                            .to_owned(),
                    ),
                    _ => None,
                };
                let workflow_sources: Option<Vec<WorkflowTemplateFile>> =
                    match p.get("workflow_sources") {
                        Some(v) if !v.is_null() => {
                            let files: Vec<WorkflowTemplateFile> =
                                serde_json::from_value(v.clone())
                                    .context("workflow_sources must be a list of {filename, source}")?;
                            ensure!(!files.is_empty(), "workflow_sources cannot be empty");
                            Some(files)
                        }
                        _ => None,
                    };
                let workflow_mode_raw = match p.get("workflow_mode") {
                    Some(v) if !v.is_null() => Some(
                        v.as_str()
                            .context("workflow_mode must be a string")?
                            .trim()
                            .to_owned(),
                    ),
                    _ => None,
                };
                let source_project_raw = match p.get("workflow_source_project_id") {
                    Some(v) if !v.is_null() => Some(
                        v.as_str()
                            .context("workflow_source_project_id must be a string")?
                            .trim()
                            .to_owned(),
                    ),
                    _ => None,
                }
                .filter(|s| !s.is_empty());
                if workflow_yaml.is_some() {
                    ensure!(
                        workflow_sources.is_none()
                            && workflow_mode_raw.is_none()
                            && source_project_raw.is_none(),
                        "workflow_yaml cannot be combined with workflow_sources, workflow_mode, or workflow_source_project_id"
                    );
                }
                if workflow_sources.is_some() {
                    ensure!(
                        workflow_mode_raw.is_none() && source_project_raw.is_none(),
                        "workflow_sources cannot be combined with workflow_mode or workflow_source_project_id"
                    );
                }
                if let Some(yaml) = &workflow_yaml {
                    let doc = canban_workflow_engine::source::parse(yaml)?;
                    let compiled = doc.compile()?;
                    let cols: Vec<String> = if doc.columns.is_empty() {
                        workflow.stages.iter().map(|s| s.id.clone()).collect()
                    } else {
                        doc.columns.iter().map(|c| c.id.clone()).collect()
                    };
                    let profiles: Vec<String> =
                        config.profiles.iter().map(|a| a.id.clone()).collect();
                    canban_workflow_engine::validate_definition(&compiled, &cols, &profiles)?;
                }
                let (resolved_source_pid, effective_remote) = {
                    let mut store = self.store.lock().unwrap();
                    store.write(|db| {
                        let resolved_pid = match (
                            workflow_mode_raw.as_deref(),
                            source_project_raw.as_deref(),
                        ) {
                            (_, Some(raw_pid)) => {
                                ensure!(
                                    workflow_mode_raw.as_deref().is_none_or(|m| m == "project"),
                                    "workflow_source_project_id requires workflow_mode 'project' (or omitted)"
                                );
                                let pid = super::resolve_project_id(db, raw_pid)?;
                                let src_proj: Project = db.get(&pid)?;
                                authoring::initialize(&self.home, db, &config, &src_proj)?;
                                Some(pid)
                            }
                            (Some("project"), None) => {
                                bail!(
                                    "workflow_source_project_id is required when workflow_mode is 'project'"
                                );
                            }
                            (Some("default") | Some("builtin") | None, None) => None,
                            (Some(other), None) => {
                                bail!(
                                    "Unsupported workflow_mode '{other}'; expected 'default', 'builtin', or 'project'"
                                );
                            }
                        };
                        let probe_mode = if let Some(yaml) = workflow_yaml.as_deref() {
                            authoring::NewProjectWorkflow::ExplicitYaml(yaml)
                        } else if let Some(files) = &workflow_sources {
                            authoring::NewProjectWorkflow::ExplicitSources(files.clone())
                        } else if let Some(pid) = &resolved_pid {
                            authoring::NewProjectWorkflow::FromProject(pid.clone())
                        } else if workflow_mode_raw.as_deref() == Some("builtin") {
                            authoring::NewProjectWorkflow::Builtin
                        } else {
                            authoring::NewProjectWorkflow::Default
                        };
                        let remote = authoring::effective_initial_remote(
                            db,
                            &probe_mode,
                            explicit_workspace.as_ref(),
                        )?;
                        Ok((resolved_pid, remote))
                    })?
                };
                workflow.workspace.remote = effective_remote;
                let location = self.work_location(&p)?;
                let agent_overrides =
                    self.parse_agent_overrides(&config, p.get("agent_overrides"))?;
                let name = strp(&p, "name")?.trim();
                ensure!(!name.is_empty(), "Project name is required");
                let auto_link = p["github_auto_link"].as_bool().unwrap_or(true);
                let detected_github = self.detect_project_github(
                    &location.machine,
                    &location.directory,
                    &workflow.workspace.remote,
                );
                let github = if auto_link {
                    detected_github
                        .as_ref()
                        .and_then(|d| self.try_auto_link_github(&d.repository))
                } else {
                    None
                };
                let linked_github = github.is_some();
                let project = Project {
                    github,
                    detected_github,
                    github_disconnected: !auto_link,
                    delivery_mode: Default::default(),
                    agent_overrides,
                    tags: Default::default(),
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
                let result = self.mutate(method, &p, |db| {
                    db.save(&project)?;
                    let init_mode = if let Some(yaml) = workflow_yaml.as_deref() {
                        authoring::NewProjectWorkflow::ExplicitYaml(yaml)
                    } else if let Some(files) = workflow_sources.clone() {
                        authoring::NewProjectWorkflow::ExplicitSources(files)
                    } else if let Some(pid) = &resolved_source_pid {
                        authoring::NewProjectWorkflow::FromProject(pid.clone())
                    } else if workflow_mode_raw.as_deref() == Some("builtin") {
                        authoring::NewProjectWorkflow::Builtin
                    } else {
                        authoring::NewProjectWorkflow::Default
                    };
                    authoring::initialize_new(
                        &self.home,
                        db,
                        &config,
                        &project,
                        init_mode,
                        explicit_workspace.as_ref(),
                    )?;
                    let saved: Project = db.get(&project.id)?;
                    Ok(serde_json::to_value(saved)?)
                })?;
                if linked_github {
                    self.github_sync_due.store(true, Ordering::SeqCst);
                }
                Ok(result)
            }
            "tag.create" => self.mutate(method, &p, |db| {
                let pid = super::resolve_project_id(db, strp(&p, "project_id")?)?;
                let mut project: Project = db.get(&pid)?;
                let raw = strp(&p, "tag")?;
                let tag = canban_types::normalize_tag(raw).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Tag must be 1–48 letters, numbers, '-' or '_' and cannot be only digits"
                    )
                })?;
                if !project
                    .tags
                    .iter()
                    .any(|existing| existing.to_lowercase() == tag.to_lowercase())
                {
                    project.tags.push(tag.clone());
                    db.save(&project)?;
                }
                let canonical = project
                    .tags
                    .iter()
                    .find(|existing| existing.to_lowercase() == tag.to_lowercase())
                    .cloned()
                    .unwrap_or(tag);
                Ok(json!({"project_id": project.id, "tag": canonical, "tags": project.tags}))
            }),
            "card.create" => self.mutate(method, &p, |db| {
                ensure!(
                    p.get("tags").is_none(),
                    "Routing tags are no longer supported; use a workflow Profile or @profile"
                );
                let project: Project =
                    db.get(&super::resolve_project_id(db, strp(&p, "project_id")?)?)?;
                let mut c = canban_kanban::create(
                    db,
                    &project,
                    strp(&p, "title")?,
                    p["body"].as_str().unwrap_or(""),
                    "user",
                )?;
                c.created_by = self
                    .github
                    .status()
                    .login
                    .or_else(|| self.github_cli.login())
                    .unwrap_or_else(|| "Local".into());
                db.save(&c)?;
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
                let title_text = c.title.clone();
                let body_text = c.body.clone();
                crate::sessions::queue_interactive_mentions(
                    db,
                    &config,
                    &mut c,
                    &project,
                    &p,
                    &[&title_text, &body_text],
                )?;
                Ok(serde_json::to_value(c)?)
            }),
            "card.update" => {
                let mut emit_card = p["card_id"].as_str().map(str::to_owned);
                let res = self.mutate(method, &p, |db| {
                    ensure!(
                        p.get("tags").is_none(),
                        "Routing tags are no longer supported; use a workflow Profile or @profile"
                    );
                    let mut c = super::resolve_card(db, &p)?;
                    let has_revision = p.get("revision").is_some_and(|v| !v.is_null());
                    if has_revision {
                        revise(&c, &p)?;
                    }
                    let profile_opt = p["profile_id"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    let is_agent = p["agent"] == true || profile_opt.is_some() || !has_revision;
                    let active_entry = if is_agent {
                        db.list::<TerminalEntry>()?.into_iter().rev().find(|t| {
                            t.agent.as_ref().is_some_and(|a| {
                                a.card_id == c.id
                                    && !a.finished()
                                    && profile_opt.is_none_or(|pid| a.profile.id == pid)
                            })
                        })
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
                    emit_card = Some(c.id.clone());
                    ensure!(c.archived_at.is_none(), "Restore the Card before editing");
                    ensure!(
                        c.removed_at.is_none(),
                        "Restore the removed Card before editing"
                    );
                    let before = c.clone();
                    if let Some(t) = p["title"].as_str()
                        && c.github_issue().is_none()
                    {
                        ensure!(!t.trim().is_empty(), "Title is required");
                        c.title = t.trim().into();
                    }
                    if let Some(b) = p["body"].as_str() {
                        c.body = b.into();
                    }
                    if let Some(mode) = p.get("delivery_mode") {
                        c.delivery_mode = serde_json::from_value(mode.clone())?;
                    }
                    c.revision += 1;
                    log(
                        db,
                        &c,
                        "card.updated",
                        &actor,
                        "Card updated",
                        active_entry.as_ref().map(|t| t.id.clone()),
                        json!({"title":c.title,"body":c.body,"changed_fields": ([if c.title!=before.title {Some("title")}else{None},if c.body!=before.body {Some("body")}else{None}].into_iter().flatten().collect::<Vec<_>>()),"before_revision":before.revision,"after_revision":c.revision}),
                    )?;
                    db.save(&c)?;
                    if is_agent {
                        for mut wf_run in db.list::<WorkflowRun>()?.into_iter().filter(|r| {
                            r.card_id == c.id
                                && r.active()
                                && (r.state == "waiting_agent"
                                    || active_entry
                                        .as_ref()
                                        .is_some_and(|t| r.terminal_id.as_deref() == Some(&t.id)))
                        }) {
                            wf_run.inputs["title"] = json!(c.title);
                            wf_run.inputs["body"] = json!(c.body);
                            wf_run.input_digest =
                                canban_workflow_engine::runtime::card_digest(db, &c);
                            wf_run.updated_at = now();
                            db.save(&wf_run)?;
                        }
                    }
                    Ok(serde_json::to_value(c)?)
                })?;
                if p["card_id"].is_null()
                    && let Some(id) = emit_card
                {
                    self.emit("application", Some(id));
                }
                Ok(res)
            }
            "card.adopt" => {
                let pre_card_id = {
                    let store = self.store.lock().unwrap();
                    super::resolve_card(store.db(), &p)?.id
                };
                {
                    let _gate = self.terminal_gate.lock().unwrap();
                    let _ = self.ensure_card_worktree(&pre_card_id, None)?;
                }
                let caller_terminal = self.prepare_adopt_terminal(&p);
                let mut emit_card = None;
                let res = self.mutate(method, &p, |db| {
                    let mut c = super::resolve_card(db, &p)?;
                    if p.get("revision").is_some_and(|v| !v.is_null()) {
                        revise(&c, &p)?;
                    }
                    ensure!(c.active(), "Restore the Card before adopting a stage");
                    ensure!(!c.resolved(), "Resolved Cards cannot enter development");
                    ensure!(
                        c.human_gate.is_none(),
                        "Human review must be approved by the user before moving this Card"
                    );
                    let target = p["stage"].as_str().unwrap_or("develop");
                    let project: Project = db.get(&c.project_id)?;
                    canban_workflow_engine::stage(&project.workflow, target)?;
                    emit_card = Some(c.id.clone());
                    let requested_prof = p["profile_id"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    let bound_entry = caller_terminal.as_ref().and_then(|caller| {
                        crate::adoption::bind(db, &config, &c, &p, caller)
                    });
                    let mut live_entry = bound_entry.or(db.list::<TerminalEntry>()?.into_iter().rev().find(|t| {
                        t.agent.as_ref().is_some_and(|a| {
                            a.card_id == c.id
                                && !a.finished()
                                && requested_prof.is_none_or(|pid| a.profile.id == pid)
                        })
                    }));
                    let profile_label = live_entry
                        .as_ref()
                        .and_then(|t| t.agent.as_ref())
                        .map(|a| a.profile.id.clone())
                        .or_else(|| requested_prof.map(str::to_owned))
                        .unwrap_or_else(|| config.default_agent.clone());
                    let live_id = live_entry.as_ref().map(|t| t.id.clone());

                    let cancel_other_pending = |db: Db<'_>| -> Result<()> {
                        for mut pending in db.list::<TerminalEntry>()? {
                            if Some(pending.id.as_str()) == live_id.as_deref() {
                                continue;
                            }
                            if let Some(a) = &mut pending.agent
                                && a.card_id == c.id
                            {
                                let mut changed = false;
                                if a.state == "queued" && canban_scheduler::automatic(a) {
                                    a.state = "cancelled".into();
                                    changed = true;
                                }
                                if a.pending.as_ref().is_some_and(|q| q.trigger.is_automatic()) {
                                    a.pending = None;
                                    changed = true;
                                }
                                if changed {
                                    db.save(&pending)?;
                                }
                            }
                        }
                        Ok(())
                    };

                    if c.stage == target {
                        if let Some(mut existing) = db
                            .list::<WorkflowRun>()?
                            .into_iter()
                            .find(|r| r.card_id == c.id && r.active() && r.state == "waiting_agent")
                        {
                            if existing.terminal_id.is_none()
                                && let Some(entry) = &mut live_entry
                                && let Some(agent) = &mut entry.agent
                            {
                                agent.workflow_run_id = Some(existing.id.clone());
                                agent.workflow_step = existing.step.as_ref().map(|step| format!("{step}:{}", existing.visit));
                                agent.stage = target.into();
                                db.save(entry)?;
                                existing.terminal_id = Some(entry.id.clone());
                                existing.updated_at = now();
                                db.save(&existing)?;
                            }
                            return Ok(json!({
                                "card": c,
                                "workflow_run": existing,
                                "worktree": c.worktree,
                                "branch": c.branch,
                                "adopted": true,
                            }));
                        }
                        if db
                            .list::<WorkflowRun>()?
                            .iter()
                            .any(|r| r.card_id == c.id && r.active())
                        {
                            workflow::stop_card(db, &config, &c.id, "superseded")?;
                        }
                        cancel_other_pending(db)?;
                        let wf_run = workflow::adopt_stage(
                            db,
                            &config,
                            &mut c,
                            live_entry.as_mut(),
                            target,
                            None,
                        )?;
                        db.save(&c)?;
                        return Ok(json!({
                            "card": c,
                            "workflow_run": wf_run,
                            "worktree": c.worktree,
                            "branch": c.branch,
                            "adopted": true,
                        }));
                    }

                    let from = c.stage.clone();
                    cancel_other_pending(db)?;
                    workflow::stop_card(db, &config, &c.id, "superseded")?;
                    c.stage = target.into();
                    c.board_order = None;
                    canban_kanban::order::place(db, &mut c, None)?;
                    c.revision += 1;
                    c.attention = None;
                    c.human_gate = None;
                    db.save(&c)?;
                    let mut event_data = json!({
                        "from": from,
                        "to": target,
                        "reason": "agent_adopted_stage",
                    });
                    if let Some(tid) = &live_id {
                        event_data["terminal_id"] = json!(tid);
                    }
                    let event = db.append(canban_timeline::event(
                        &c.project_id,
                        &c.id,
                        "card.stage_changed",
                        &format!("agent:{profile_label}"),
                        &format!("{from} → {target} (adopted by @{profile_label})"),
                        event_data,
                    ))?;
                    let wf_run = workflow::adopt_stage(
                        db,
                        &config,
                        &mut c,
                        live_entry.as_mut(),
                        target,
                        Some(event.id),
                    )?;
                    db.save(&c)?;
                    Ok(json!({
                        "card": c,
                        "workflow_run": wf_run,
                        "worktree": c.worktree,
                        "branch": c.branch,
                        "adopted": true,
                    }))
                })?;
                if p["card_id"].is_null()
                    && let Some(id) = emit_card
                {
                    self.emit("application", Some(id));
                }
                Ok(res)
            }
            "card.move" => self.mutate(method, &p, |db| {
                let mut c: Card = db.get(strp(&p, "card_id")?)?;
                revise(&c, &p)?;
                ensure!(c.active(), "Restore the Card before moving");
                ensure!(
                    c.human_gate.is_none() || p["execution"].is_null(),
                    "Human review must be approved by the user before moving this Card"
                );
                let target = strp(&p, "stage")?;
                let project: Project = db.get(&c.project_id)?;
                canban_workflow_engine::stage(&project.workflow, target)?;
                let from = c.stage.clone();
                let placement = p.get("before_card_id");
                ensure!(
                    placement.is_none_or(|v| v.is_null() || v.is_string()),
                    "before_card_id must be a Card ID or null"
                );
                if c.stage == target {
                    if placement.is_some()
                        && canban_kanban::order::place(db, &mut c, p["before_card_id"].as_str())?
                    {
                        c.revision += 1;
                        log(
                            db,
                            &c,
                            "card.order_changed",
                            "user",
                            "Card reordered",
                            None,
                            json!({"stage":target,"before_card_id":p["before_card_id"]}),
                        )?;
                        db.save(&c)?;
                    }
                    return Ok(serde_json::to_value(c)?);
                }
                // Ordinary queued work from a stage the user has left must not
                // start later. Already-running executions are deliberately not drained.
                for mut entry in db.list::<TerminalEntry>()? {
                    if let Some(a) = &mut entry.agent
                        && a.card_id == c.id
                    {
                        let mut changed = false;
                        if a.state == "queued" && a.trigger == ExecutionTrigger::StageEntry {
                            a.state = "cancelled".into();
                            changed = true;
                        }
                        if a.pending
                            .as_ref()
                            .is_some_and(|q| q.trigger == ExecutionTrigger::StageEntry)
                        {
                            a.pending = None;
                            changed = true;
                        }
                        if changed {
                            db.save(&entry)?;
                        }
                    }
                }
                c.stage = target.into();
                c.board_order = None;
                canban_kanban::order::place(db, &mut c, p["before_card_id"].as_str())?;
                c.revision += 1;
                c.attention = None;
                c.human_gate = None;
                db.save(&c)?;
                log(
                    db,
                    &c,
                    "card.stage_changed",
                    "user",
                    &format!("{from} → {target}"),
                    None,
                    json!({"from":from,"to":target,"before_card_id":p["before_card_id"]}),
                )?;
                workflow::stop_card(db, &self.config(), &c.id, "superseded")?;
                Ok(serde_json::to_value(c)?)
            }),
            "card.continue" => self.continue_card(p),
            "card.hold_auto_merge" => {
                ensure!(
                    p["execution"].is_null(),
                    "Holding or resuming auto-merge requires a user invocation"
                );
                let hold_arg = p["hold"].as_bool();
                let res = self.mutate(method, &p, |db| {
                    let mut c: Card = db.get(strp(&p, "card_id")?)?;
                    revise(&c, &p)?;
                    ensure!(c.active(), "Restore the Card before changing auto-merge");
                    let hold = hold_arg.unwrap_or(!c.hold_auto_merge);
                    c.hold_auto_merge = hold;
                    c.revision += 1;
                    log(
                        db,
                        &c,
                        if hold {
                            "card.auto_merge_held"
                        } else {
                            "card.auto_merge_resumed"
                        },
                        "user",
                        if hold {
                            "Held automatic merge for this Card"
                        } else {
                            "Resumed automatic merge for this Card"
                        },
                        None,
                        json!({"hold_auto_merge":hold}),
                    )?;
                    if !hold
                        && let Some(mut run) = db.list::<WorkflowRun>()?.into_iter().find(|r| {
                            r.card_id == c.id
                                && r.active()
                                && r.step_is_deliver()
                                && matches!(r.state.as_str(), "waiting_approval" | "waiting_delivery")
                        })
                    {
                        if run.state == "waiting_approval" {
                            canban_workflow_engine::runtime::approve_with_feedback(
                                db, &config, &mut c, &mut run, false, None, enqueue,
                            )?;
                        } else {
                            run.inputs["queued_at"] = json!(
                                canban_workflow_engine::runtime::next_merge_queued_at(
                                    db,
                                    &c.project_id
                                )?
                            );
                            run.updated_at = now();
                            db.save(&run)?;
                        }
                    }
                    db.save(&c)?;
                    Ok(serde_json::to_value(c)?)
                })?;
                self.request_deliveries();
                self.request_scripts();
                Ok(res)
            }
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
                let mut c = super::resolve_card(db, &p)?;
                ensure!(c.active(), "Restore the Card before commenting");
                let before = c.clone();
                let body = strp(&p, "body")?;
                ensure!(!body.trim().is_empty(), "Comment is empty");
                let event = db.append(canban_timeline::event(
                    &c.project_id,
                    &c.id,
                    "comment.created",
                    "user",
                    body,
                    json!({"body":body}),
                ))?;
                let project: Project = db.get(&c.project_id)?;
                let stage = canban_workflow_engine::stage(&project.workflow, &c.stage)?;
                let mut mentions = strings(&p, "mentions");
                // Explicit recipients distinguish addressing an Agent from discussing its name.
                // Older CLI clients without the field retain their text-mention behavior.
                if p.get("mentions").is_none() {
                    for word in body.split_whitespace() {
                        if let Some(name) = word.strip_prefix('@') {
                            let name = name.trim_end_matches(|ch: char| {
                                !ch.is_alphanumeric() && ch != '-' && ch != '_'
                            });
                            if config.profiles.iter().any(|a| a.id == name && a.enabled) {
                                mentions.push(name.into());
                            }
                        }
                    }
                }
                mentions.sort();
                mentions.dedup();
                for agent in mentions {
                    let profile =
                        canban_scheduler::select_in_project(&config, Some(&project), Some(&agent))?;
                    let context = crate::sessions::conversation(db, &c, &profile)?;
                    let active = db.get::<TerminalEntry>(&context.id).ok().filter(|t| {
                        t.agent.as_ref().is_some_and(|a| {
                            a.card_id == c.id
                                && a.profile.id == agent
                                && (!(c.human_gate.is_some()
                                    && a.state == "queued"
                                    && canban_scheduler::automatic(a))
                                    && matches!(
                                        a.state.as_str(),
                                        "queued" | "starting" | "running" | "working" | "blocked"
                                    )
                                    || matches!(a.state.as_str(), "completed" | "backlog")
                                        && a.profile == profile
                                        && a.terminal.is_some()
                                        && !a.finished())
                        })
                    });
                    let entry = if let Some(entry) = active {
                        entry
                    } else {
                        let instruction = canban_context::render_template(
                            include_str!(concat!(
                                env!("CARGO_MANIFEST_DIR"),
                                "/../../prompts/runtime/comment_task.txt"
                            ))
                            .trim_end(),
                            &[("body", body)],
                        );
                        enqueue(
                            db,
                            &config,
                            &mut c,
                            &project,
                            stage,
                            ExecutionTrigger::Comment {
                                comment_id: event.id.clone(),
                            },
                            Some(&agent),
                            Some(instruction),
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
                        terminal_id: Some(entry.id.clone()),
                        state: "pending".into(),
                        error: None,
                        reply_path: None,
                        created_at: now(),
                    };
                    db.save(&question)?;
                }
                if c != before {
                    c.revision += 1;
                    db.save(&c)?;
                }
                let mut result = serde_json::to_value(event)?;
                result["card_revision"] = json!(c.revision);
                Ok(result)
            }),
            "comment.update" => self.mutate(method, &p, |db| {
                let mut c = super::resolve_card(db, &p)?;
                ensure!(c.active(), "Restore the Card before editing comments");
                let before = c.clone();
                let comment_id = strp(&p, "comment_id")?;
                let body = strp(&p, "body")?;
                ensure!(!body.trim().is_empty(), "Comment is empty");
                let mut event: Event = db
                    .get(comment_id)
                    .map_err(|_| anyhow::anyhow!("Comment not found"))?;
                ensure!(event.card_id == c.id, "Comment belongs to another Card");
                ensure!(
                    event.kind == "comment.created" && event.actor == "user",
                    "Only your own comments can be edited"
                );
                let previous_body = event.data["body"]
                    .as_str()
                    .unwrap_or(&event.message)
                    .to_owned();
                let previous_revision = event.data["revision"].as_u64().unwrap_or(1);
                if previous_body == body {
                    return Ok(serde_json::to_value(event)?);
                }
                let edited_at = now();
                let next_revision = previous_revision + 1;
                event.message = body.into();
                if !event.data.is_object() {
                    event.data = json!({});
                }
                event.data["body"] = json!(body);
                event.data["revision"] = json!(next_revision);
                event.data["edited_at"] = json!(edited_at);
                db.save(&event)?;

                let instruction = canban_context::render_template(
                    include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../../prompts/runtime/comment_task.txt"
                    ))
                    .trim_end(),
                    &[("body", body)],
                );
                for mut question in db.list::<Question>()?.into_iter().filter(|q| {
                    q.card_id == c.id && q.comment_id == event.id && q.state == "pending"
                }) {
                    question.body = body.into();
                    db.save(&question)?;
                    if let Some(tid) = question.terminal_id.as_deref()
                        && let Ok(mut entry) = db.get::<TerminalEntry>(tid)
                        && let Some(agent) = entry.agent.as_mut()
                    {
                        let mut entry_changed = false;
                        if agent.state == "queued"
                            && matches!(
                                &agent.trigger,
                                ExecutionTrigger::Comment { comment_id: cid } if cid == &event.id
                            )
                        {
                            agent.instruction = instruction.clone();
                            agent.updated_at = edited_at;
                            entry_changed = true;
                        }
                        if let Some(pending) = agent.pending.as_mut()
                            && matches!(
                                &pending.trigger,
                                ExecutionTrigger::Comment { comment_id: cid } if cid == &event.id
                            )
                        {
                            pending.instruction = instruction.clone();
                            entry_changed = true;
                        }
                        if entry_changed {
                            db.save(&entry)?;
                        }
                    }
                }

                // Preserve existing mentions without redelivery; allow newly added @Agent mentions.
                if p.get("mentions").is_some() {
                    let project: Project = db.get(&c.project_id)?;
                    let stage = canban_workflow_engine::stage(&project.workflow, &c.stage)?;
                    let mut mentions = strings(&p, "mentions");
                    mentions.sort();
                    mentions.dedup();
                    for agent in mentions {
                        let qid = format!("{}:{}", event.id, agent);
                        if db.get::<Question>(&qid).is_ok() {
                            continue;
                        }
                        let profile = canban_scheduler::select_in_project(
                            &config,
                            Some(&project),
                            Some(&agent),
                        )?;
                        let context = crate::sessions::conversation(db, &c, &profile)?;
                        let active = db.get::<TerminalEntry>(&context.id).ok().filter(|t| {
                            t.agent.as_ref().is_some_and(|a| {
                                a.card_id == c.id
                                    && a.profile.id == agent
                                    && (!(c.human_gate.is_some()
                                        && a.state == "queued"
                                        && canban_scheduler::automatic(a))
                                        && matches!(
                                            a.state.as_str(),
                                            "queued"
                                                | "starting"
                                                | "running"
                                                | "working"
                                                | "blocked"
                                        )
                                        || matches!(a.state.as_str(), "completed" | "backlog")
                                            && a.profile == profile
                                            && a.terminal.is_some()
                                            && !a.finished())
                            })
                        });
                        let entry = if let Some(entry) = active {
                            entry
                        } else {
                            enqueue(
                                db,
                                &config,
                                &mut c,
                                &project,
                                stage,
                                ExecutionTrigger::Comment {
                                    comment_id: event.id.clone(),
                                },
                                Some(&agent),
                                Some(instruction.clone()),
                            )?
                            .context("Question not queued")?
                        };
                        let question = Question {
                            id: qid,
                            project_id: c.project_id.clone(),
                            card_id: c.id.clone(),
                            profile_id: agent,
                            comment_id: event.id.clone(),
                            body: body.into(),
                            terminal_id: Some(entry.id.clone()),
                            state: "pending".into(),
                            error: None,
                            reply_path: None,
                            created_at: edited_at,
                        };
                        db.save(&question)?;
                    }
                }

                if c != before {
                    c.revision += 1;
                    db.save(&c)?;
                }
                db.append(canban_timeline::event(
                    &c.project_id,
                    &c.id,
                    "comment.updated",
                    "user",
                    "Comment updated",
                    json!({
                        "comment_id": event.id,
                        "revision": next_revision,
                        "previous_body": previous_body,
                        "body": body,
                        "edited_at": edited_at,
                    }),
                ))?;
                let mut result = serde_json::to_value(event)?;
                result["card_revision"] = json!(c.revision);
                Ok(result)
            }),
            "comment.delete" => self.mutate(method, &p, |db| {
                let c = super::resolve_card(db, &p)?;
                ensure!(c.active(), "Restore the Card before deleting comments");
                let comment_id = strp(&p, "comment_id")?;
                let mut event: Event = db
                    .get(comment_id)
                    .map_err(|_| anyhow::anyhow!("Comment not found"))?;
                ensure!(event.card_id == c.id, "Comment belongs to another Card");
                ensure!(
                    event.kind == "comment.created" && event.actor == "user",
                    "Only your own comments can be deleted"
                );
                let deleted_at = now();
                let previous_body = event.data["body"]
                    .as_str()
                    .unwrap_or(&event.message)
                    .to_owned();
                let next_revision = event.data["revision"].as_u64().unwrap_or(1) + 1;
                event.kind = "comment.deleted".into();
                event.message = "Comment deleted".into();
                event.data = json!({
                    "comment_id": event.id,
                    "revision": next_revision,
                    "deleted_at": deleted_at,
                });
                db.save(&event)?;

                let all_questions = db.list::<Question>()?;
                let mut affected_terminals = Vec::new();
                for mut question in all_questions
                    .iter()
                    .filter(|q| {
                        q.card_id == c.id && q.comment_id == event.id && q.state == "pending"
                    })
                    .cloned()
                {
                    question.state = "cancelled".into();
                    db.save(&question)?;
                    if let Some(tid) = question.terminal_id
                        && !affected_terminals.contains(&tid)
                    {
                        affected_terminals.push(tid);
                    }
                }
                for tid in affected_terminals {
                    let other_pending = all_questions.iter().any(|q| {
                        q.card_id == c.id
                            && q.comment_id != event.id
                            && q.state == "pending"
                            && q.terminal_id.as_deref() == Some(tid.as_str())
                    });
                    if other_pending {
                        continue;
                    }
                    if let Ok(mut entry) = db.get::<TerminalEntry>(&tid)
                        && let Some(agent) = entry.agent.as_mut()
                    {
                        let mut entry_changed = false;
                        if agent.state == "queued"
                            && matches!(
                                &agent.trigger,
                                ExecutionTrigger::Comment { comment_id: cid } if cid == &event.id
                            )
                        {
                            agent.state = "cancelled".into();
                            agent.updated_at = deleted_at;
                            entry_changed = true;
                        }
                        if agent.pending.as_ref().is_some_and(|pending| {
                            matches!(
                                &pending.trigger,
                                ExecutionTrigger::Comment { comment_id: cid } if cid == &event.id
                            )
                        }) {
                            agent.pending = None;
                            agent.updated_at = deleted_at;
                            entry_changed = true;
                        }
                        if entry_changed {
                            db.save(&entry)?;
                        }
                    }
                }

                db.append(canban_timeline::event(
                    &c.project_id,
                    &c.id,
                    "comment.deleted",
                    "user",
                    "Comment deleted",
                    json!({
                        "comment_id": event.id,
                        "revision": next_revision,
                        "previous_body": previous_body,
                        "deleted_at": deleted_at,
                    }),
                ))?;
                Ok(json!({
                    "deleted": true,
                    "comment_id": event.id,
                    "card_id": c.id,
                    "revision": c.revision,
                }))
            }),
            "card.attention.clear" => self.mutate(method, &p, |db| {
                let mut c: Card = db.get(strp(&p, "card_id")?)?;
                if c.human_gate.is_none() && c.attention.take().is_some() {
                    c.revision += 1;
                    db.save(&c)?;
                }
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
            "review.github_pr" => self.review_github_pr(p),
            "review.github_pr_file" => self.review_github_pr_file(p),
            "review.send_agent" => self.review_send_agent(p),
            "review.run_agent" => self.review_run_agent(p),
            "review.draft.get" => self.review_draft_get(p),
            "review.draft.set" => self.review_draft_set(p),
            "review.draft.clear" => self.review_draft_clear(p),
            "agent.open" => self.open_agent(p),
            "agent.bind" => self.bind_session(p),
            // Old in-flight hook commands can finish, but never drive lifecycle.
            "agent.turn_ended" => Ok(json!({"deprecated":true,"recorded":false,"source":"herdr"})),
            "profile.presets" => Ok(serde_json::to_value(canban_profiles::presets(&self.home)?)?),
            "agent.reply" => self.reply(p),
            "agent.interrupt" => self.control_agent(p, false),
            "agent.end" => self.control_agent(p, true),
            "batch.bind" => self.batch_bind(p),
            "batch.report" => self.batch_report(p),
            "execution.report" => self.report(p),
            "execution.retry" => self.mutate(method, &p, |db| {
                let mut entry: TerminalEntry = db.get(strp(&p, "terminal_id")?)?;
                if entry.id.starts_with("merge-queue:") {
                    return self.retry_merge_batch_runs(db, &p, &entry);
                }
                let prev = entry
                    .agent
                    .as_ref()
                    .context("Terminal has no Agent assignment")?;
                let mut c: Card = db.get(&prev.card_id)?;
                let deliver_run = db.list::<WorkflowRun>()?.into_iter().find(|r| {
                    r.card_id == c.id
                        && r.active()
                        && r.step_is_deliver()
                        && (r.terminal_id.as_deref() == Some(entry.id.as_str())
                            || prev.workflow_run_id.as_deref() == Some(r.id.as_str()))
                });
                ensure!(
                    p["execution"].is_null(),
                    "Retry requires an explicit user decision"
                );
                ensure!(c.archived_at.is_none(), "Restore the Card before retrying");
                ensure!(
                    !matches!(
                        prev.state.as_str(),
                        "queued" | "starting" | "running" | "working"
                    ) && !(prev.state == "completed"
                        && deliver_run
                            .as_ref()
                            .is_none_or(|r| r.state != "blocked")),
                    "Execution is not retryable"
                );
                ensure!(
                    prev.terminal.is_none()
                        || prev.observed.as_ref().is_some_and(|v| {
                            matches!(v["state"].as_str(), Some("finished" | "missing"))
                        }),
                    "End the previous Agent before retrying"
                );
                if let Some(mut wf_run) = deliver_run {
                    if wf_run.state == "waiting_delivery" {
                        c.hold_auto_merge = false;
                        c.human_gate = None;
                        c.attention = None;
                        c.revision += 1;
                        db.save(&c)?;
                        return Ok(serde_json::to_value(entry)?);
                    }
                    if wf_run.state == "waiting_approval" {
                        let config = self.config();
                        canban_workflow_engine::runtime::approve_with_feedback(
                            db, &config, &mut c, &mut wf_run, false, None, enqueue,
                        )?;
                    } else {
                        canban_workflow_engine::runtime::requeue_delivery(db, &mut c, &mut wf_run)?;
                    }
                    entry = db.get(&entry.id)?;
                    log(
                        db,
                        &c,
                        "execution.retry_queued",
                        "user",
                        "Queued in merge queue",
                        Some(entry.id.clone()),
                        json!({"terminal_id":entry.id,"workflow_run_id":wf_run.id,"queued_for_delivery":true}),
                    )?;
                    return Ok(serde_json::to_value(entry)?);
                }
                crate::sessions::retry_agent(db, &c, &mut entry)?;
                let run_number = entry.agent.as_ref().map(|a| a.number).unwrap_or(1);
                c.human_gate = None;
                c.attention = None;
                c.revision += 1;
                db.save(&c)?;
                log(
                    db,
                    &c,
                    "execution.retry_queued",
                    "user",
                    "Fresh Agent run queued",
                    Some(entry.id.clone()),
                    json!({"terminal_id":entry.id,"run_number":run_number}),
                )?;
                Ok(serde_json::to_value(entry)?)
            }),
            "execution.output" | "terminal.output" => self.output(strp(&p, "terminal_id")?),
            "resource.read" => self.read_resource(strp(&p, "card_id")?, strp(&p, "path")?),
            "resource.materialize" => {
                let card: Card = self.store.lock().unwrap().db().get(strp(&p, "card_id")?)?;
                let path = strp(&p, "path")?;
                let full = self.home.join(path);
                let root = canban_context::card_dir(&self.home, &card.project_id, card.number);
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
            "card.remove" => {
                ensure!(
                    p["execution"].is_null(),
                    "Removing a Card requires an explicit user decision"
                );
                self.mutate(method, &p, |db| {
                    let mut c: Card = db.get(strp(&p, "card_id")?)?;
                    revise(&c, &p)?;
                    canban_kanban::remove(db, &mut c)?;
                    Ok(json!({"removed":true,"card_id":c.id}))
                })
            }
            "card.archive" => self.archive(&p),
            "card.restore" => self.restore(&p),
            "card.resolve.preview" => self.resolve_card_preview(&p),
            "card.resolve" => self.resolve_card_action(&p),
            "card.reopen" => self.reopen_card_action(&p),
            _ => bail!("Unknown operation: {method}"),
        }
    }
}
