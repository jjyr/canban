//! Project-owned configuration Agents share Profile command expansion, Scheduler
//! admission and Terminal transport, without manufacturing a business Card.
use super::*;
impl AppServer {
    pub(super) fn global_workflow_prompt(&self) -> String {
        let cli_str = self.cli.display().to_string();
        let home_str = self.home.display().to_string();
        let endpoint_str = self.endpoint().display().to_string();
        canban_context::render_template(
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../prompts/runtime/workflow_assistant.txt"
            )),
            &[
                ("project_name", "Global (Host)"),
                ("project_id", GLOBAL_PROJECT_ID),
                ("project_directory", &home_str),
                ("cli", &cli_str),
                ("home", &home_str),
                ("endpoint", &endpoint_str),
            ],
        )
    }

    pub(super) fn workflow_prompt(&self, project: &Project) -> String {
        let cli_str = self.cli.display().to_string();
        let home_str = self.home.display().to_string();
        let endpoint_str = self.endpoint().display().to_string();
        canban_context::render_template(
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../prompts/runtime/workflow_assistant.txt"
            )),
            &[
                ("project_name", &project.name),
                ("project_id", &project.id),
                ("project_directory", &project.directory),
                ("cli", &cli_str),
                ("home", &home_str),
                ("endpoint", &endpoint_str),
            ],
        )
    }
    pub(super) fn configuration_call(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "configuration.bind" => self.mutate(method,&p,|db| {
                let mut session:ConfigurationSession=db.get(strp(&p,"configuration_id")?)?;
                ensure!(p["generation"].as_u64()==Some(session.generation),"Obsolete configuration invocation");
                let id=strp(&p,"session_id")?;
                ensure!(!id.is_empty() && id.len()<512,"Invalid provider session ID");
                ensure!(session.provider_session_id.as_deref().is_none_or(|s|s==id),"Provider history changed");
                session.provider_session_id=Some(id.into());db.save(&session)?;Ok(json!({"bound":true}))
            }),
            "configuration.open" => self.mutate(method,&p,|db| {
                let scope_id = if p["scope"]
                    .as_str()
                    .is_some_and(|s| s.trim().eq_ignore_ascii_case("global"))
                {
                    GLOBAL_PROJECT_ID.to_owned()
                } else {
                    super::resolve_workflow_scope(db, strp(&p, "project_id")?)?
                };
                let project_opt = if is_global_project_id(&scope_id) {
                    None
                } else {
                    Some(db.get::<Project>(&scope_id)?)
                };
                let explicit_profile=p["profile_id"].as_str().map(str::trim).filter(|s|!s.is_empty());
                let profile=canban_scheduler::select_in_project(&self.config(),project_opt.as_ref(),explicit_profile)?;
                let key=format!("configuration:{scope_id}:workflows:{}",profile.id);
                let old=db.get::<ConfigurationSession>(&key).ok();
                let mut fresh=p["fresh"]==true || old.is_none();
                if let Some(old)=&old {
                    let terminal=db.get::<TerminalEntry>(&key)?;
                    if terminal.runtime.as_ref().is_some_and(|r|r.state!="finished") || old.state=="queued" {
                        ensure!(p["fresh"]!=true,"End the existing configuration Agent before starting a new conversation");
                        let state = terminal.runtime.as_ref().map(|r|r.state.as_str()).unwrap_or("queued").to_owned();
                        return Ok(json!({"terminal_id":key,"reused":true,"state":state,"terminal":terminal,"session":old}));
                    }
                    if p["auto_fresh"]==true && (old.provider_session_id.is_none() || profile.resume_command().is_empty()) {
                        fresh=true;
                    }
                    ensure!(old.provider_session_id.is_some() || fresh,"Exact provider history unavailable; explicitly start a new conversation");
                    ensure!(fresh || !profile.resume_command().is_empty(),"Profile does not support Resume");
                }
                let prompt=match project_opt.as_ref() {
                    Some(project) => self.workflow_prompt(project),
                    None => self.global_workflow_prompt(),
                };
                if p["prompt"].is_string() {ensure!(p["prompt"]==prompt,"Prompt context changed; reopen Edit with agent to review it");}
                let session=ConfigurationSession {id:key.clone(),project_id:scope_id.clone(),profile_id:profile.id,provider_session_id:if fresh {None}else{old.as_ref().and_then(|s|s.provider_session_id.clone())},generation:old.as_ref().map_or(1,|s|s.generation+1),state:"queued".into(),prompt,error:None};
                let term_name = if is_global_project_id(&scope_id) {
                    format!("Global Workflows · {}", session.profile_id)
                } else {
                    format!("Workflows · {}", session.profile_id)
                };
                let origin_project_id = (!is_global_project_id(&scope_id)).then_some(scope_id);
                let mut terminal=db.get::<TerminalEntry>(&key).unwrap_or(TerminalEntry {id:key.clone(),conversation_id:None,number:db.counter("terminal-number")?,name:Some(term_name),origin_project_id,origin_card_id:None,agent_engine:None,runtime:None,agent:None,created_at:now()});
                terminal.created_at=now();
                terminal.runtime=None;db.save(&terminal)?;db.save(&session)?;
                Ok(json!({"terminal_id":key,"reused":false,"state":"queued","terminal":terminal,"session":session}))
            }),
            _=>bail!("Unknown configuration operation {method}"),
        }
    }
    pub(super) fn pump_configuration(&self) -> Result<()> {
        let snapshot = self.terminal_snapshot()?;
        for session in snapshot.configuration_sessions.clone() {
            if session.state != "queued" {
                if let Some(t) = snapshot
                    .terminals
                    .iter()
                    .find(|t| t.id == session.id)
                    .and_then(|t| t.runtime.as_ref())
                {
                    let observed = self.runtime_inventory().ok().and_then(|(_, v)| {
                        v.into_iter().find(|v| {
                            Some(v["address"].as_str().unwrap_or_default()) == t.terminal.as_deref()
                        })
                    });
                    let next = if t.state == "finished" {
                        "finished"
                    } else if t.state == "unknown" {
                        "unknown"
                    } else {
                        observed
                            .as_ref()
                            .and_then(|v| v["status"]["state"].as_str())
                            .unwrap_or("unknown")
                    };
                    if session.state != next {
                        let expected_gen = session.generation;
                        let expected_state = session.state.clone();
                        let next_state: String = next.into();
                        let mut updated = false;
                        self.store.lock().unwrap().write(|db| {
                            let mut current: ConfigurationSession = db.get(&session.id)?;
                            if current.generation == expected_gen && current.state == expected_state
                            {
                                current.state = next_state;
                                db.save(&current)?;
                                updated = true;
                            }
                            Ok(())
                        })?;
                        if updated {
                            self.emit("configuration", None);
                        }
                    }
                }
                continue;
            }
            if canban_scheduler::active_count(
                &self.store.lock().unwrap().db().admission_snapshot()?,
            ) >= self.config().scheduler.max_agents
            {
                continue;
            }
            let _gate = self.terminal_gate.lock().unwrap();
            let admitted = self.store.lock().unwrap().write(|db| {
                let current_snap = db.admission_snapshot()?;
                if canban_scheduler::active_count(&current_snap)
                    >= self.config().scheduler.max_agents
                {
                    return Ok(None);
                }
                let mut current: ConfigurationSession = db.get(&session.id)?;
                if current.state != "queued" || current.generation != session.generation {
                    return Ok(None);
                }
                let project = if is_global_project_id(&current.project_id) {
                    None
                } else {
                    db.get::<Project>(&current.project_id).ok()
                };
                let profile = canban_scheduler::select_in_project(
                    &self.config(),
                    project.as_ref(),
                    Some(&current.profile_id),
                )?;
                let mut terminal: TerminalEntry = db.get(&current.id)?;
                current.state = "starting".into();
                let fresh = current.provider_session_id.is_none();
                if fresh && profile.command().iter().any(|s| s.contains("{session_id}")) {
                    current.provider_session_id = Some(super::runtime::uuid_session_id());
                }
                terminal.runtime = Some(TerminalRuntime {
                    core_id: None,
                    terminal: None,
                    state: "starting".into(),
                    connection: "connecting".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                    observed_agent: None,
                    waiting: None,
                });
                db.save(&current)?;
                db.save(&terminal)?;
                Ok(Some((current, terminal, project, profile, fresh)))
            })?;
            let Some((mut session, mut terminal, project, profile, fresh)) = admitted else {
                continue;
            };
            let result = (|| -> Result<()> {
                let client = canban_herdr::Client {
                    config: self.config(),
                };
                let core = self.verified_core_id()?;
                let prompt = if fresh {
                    session.prompt.clone()
                } else {
                    "Continue the existing Project Workflows configuration conversation. Re-read current sources and active revisions before any modification. Wait for the user's next instruction.".into()
                };
                let cwd = if is_global_project_id(&session.project_id) {
                    self.home.join("global")
                } else {
                    self.home.join("projects").join(&session.project_id)
                };
                std::fs::create_dir_all(&cwd)?;
                let cwd_str = cwd.to_string_lossy().into_owned();
                let home_str = self.home.to_string_lossy().into_owned();
                let project_dir = project
                    .as_ref()
                    .map(|p| p.directory.as_str())
                    .unwrap_or(home_str.as_str());
                let mut argv = canban_profiles::command(
                    &profile,
                    session.provider_session_id.as_deref(),
                    fresh,
                    &[
                        ("home", &self.home.to_string_lossy()),
                        ("cwd", &cwd_str),
                        ("project_dir", project_dir),
                        ("cli", &self.cli.to_string_lossy()),
                        ("prompt", &prompt),
                    ],
                )?;
                let is_herdr_template = profile
                    .preset
                    .as_deref()
                    .is_some_and(|s| s.starts_with("herdr:"));
                if !is_herdr_template {
                    argv[0] = canban_profiles::executable_with_env(&argv[0], &client.config.hc_env);
                }
                let env = std::collections::BTreeMap::from([
                    (
                        "CANBAN_HOME".into(),
                        self.home.to_string_lossy().into_owned(),
                    ),
                    (
                        "CANBAN_ENDPOINT".into(),
                        self.endpoint().to_string_lossy().into_owned(),
                    ),
                    ("CANBAN_CLI".into(), self.cli.to_string_lossy().into_owned()),
                    ("CANBAN_CONFIGURATION_ID".into(), session.id.clone()),
                    (
                        "CANBAN_CONFIGURATION_GENERATION".into(),
                        session.generation.to_string(),
                    ),
                    (
                        "CANBAN_SESSION_ID".into(),
                        session.provider_session_id.clone().unwrap_or_default(),
                    ),
                ]);
                let receipt = self
                    .launch_herdr_with_env(crate::env::HerdrLaunchRequest {
                        profile: Some(&profile),
                        project: project.as_ref(),
                        protected_context: &env,
                        workflow_env: None,
                        source_trust: Some("local"),
                        argv,
                        cwd: cwd_str.clone(),
                        labels: std::collections::BTreeMap::from([
                            ("canban.configuration".into(), session.id.clone()),
                            ("canban.project".into(), session.project_id.clone()),
                        ]),
                    })?
                    .receipt;
                let address = strp(&receipt, "address")?.to_owned();
                let runtime = terminal.runtime.as_mut().unwrap();
                runtime.core_id = Some(core.clone());
                runtime.terminal = Some(address.clone());
                runtime.state = "running".into();
                runtime.connection = "connected".into();
                {
                    let mut inv = self.inventory.lock().unwrap();
                    if inv.core == core {
                        inv.records.entry(address.clone()).or_insert_with(|| {
                            let mut seeded = receipt.clone();
                            seeded["address"] = json!(address);
                            if seeded["state"].is_null() {
                                seeded["state"] = json!("live");
                            }
                            if seeded["cwd"].is_null() {
                                seeded["cwd"] = json!(cwd_str);
                            }
                            seeded
                        });
                    }
                }
                ensure!(
                    receipt
                        .get("first_message")
                        .is_none_or(|v| v["sent"] != false),
                    "Initial prompt delivery uncertain; inspect Terminal before retrying"
                );
                Ok(())
            })();
            match result {
                Ok(()) => session.state = "running".into(),
                Err(e) => {
                    session.state = "unknown".into();
                    session.error = Some(format!("{e:#}; launch was not replayed"));
                    let r = terminal.runtime.as_mut().unwrap();
                    r.state = "unknown".into();
                    r.error = session.error.clone();
                }
            }
            let expected_gen = session.generation;
            self.store.lock().unwrap().write(|db| {
                let current: ConfigurationSession = db.get(&session.id)?;
                if current.generation == expected_gen {
                    db.save(&session)?;
                    db.save(&terminal)?;
                }
                Ok(())
            })?;
            self.emit("configuration", None);
        }
        Ok(())
    }
}
