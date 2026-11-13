//! Project-owned configuration Agents share Profile command expansion, Scheduler
//! admission and Terminal transport, without manufacturing a business Card.
use super::*;
impl AppServer {
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
                let project:Project=db.get(strp(&p,"project_id")?)?;
                let explicit_profile=p["profile_id"].as_str().map(str::trim).filter(|s|!s.is_empty());
                let profile=canban_scheduler::select_in_project(&self.config(),Some(&project),explicit_profile)?;
                let key=format!("configuration:{}:workflows:{}",project.id,profile.id);
                let old=db.get::<ConfigurationSession>(&key).ok();
                let mut fresh=p["fresh"]==true || old.is_none();
                if let Some(old)=&old {
                    let terminal=db.get::<TerminalEntry>(&key)?;
                    if terminal.runtime.as_ref().is_some_and(|r|r.state!="finished") || old.state=="queued" {
                        ensure!(p["fresh"]!=true,"End the existing configuration Agent before starting a new conversation");
                        return Ok(json!({"terminal_id":key,"reused":true,"state":terminal.runtime.as_ref().map(|r|r.state.as_str()).unwrap_or("queued")}));
                    }
                    if p["auto_fresh"]==true && (old.provider_session_id.is_none() || profile.resume_command().is_empty()) {
                        fresh=true;
                    }
                    ensure!(old.provider_session_id.is_some() || fresh,"Exact provider history unavailable; explicitly start a new conversation");
                    ensure!(fresh || !profile.resume_command().is_empty(),"Profile does not support Resume");
                }
                let prompt=self.workflow_prompt(&project);
                if p["prompt"].is_string() {ensure!(p["prompt"]==prompt,"Prompt context changed; reopen Edit with agent to review it");}
                let session=ConfigurationSession {id:key.clone(),project_id:project.id,profile_id:profile.id,provider_session_id:if fresh {None}else{old.as_ref().and_then(|s|s.provider_session_id.clone())},generation:old.as_ref().map_or(1,|s|s.generation+1),state:"queued".into(),prompt,error:None};
                let mut terminal=db.get::<TerminalEntry>(&key).unwrap_or(TerminalEntry {id:key.clone(),conversation_id:None,number:db.counter("terminal-number")?,name:Some(format!("Workflows · {}",session.profile_id)),manual_number:None,origin_card_id:None,agent_engine:None,runtime:None,created_at:now()});
                terminal.runtime=None;db.save(&terminal)?;db.save(&session)?;
                Ok(json!({"terminal_id":key,"reused":false,"state":"queued"}))
            }),
            _=>bail!("Unknown configuration operation {method}"),
        }
    }
    pub(super) fn pump_configuration(&self) -> Result<()> {
        let snapshot = self.snapshot()?;
        for mut session in snapshot.configuration_sessions.clone() {
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
                        session.state = next.into();
                        self.store.lock().unwrap().write(|db| db.save(&session))?;
                        self.emit("configuration", None);
                    }
                }
                continue;
            }
            if canban_scheduler::active_count(&self.snapshot()?)
                >= self.config().scheduler.max_agents
            {
                continue;
            }
            let project = snapshot
                .projects
                .iter()
                .find(|p| p.id == session.project_id);
            let profile = canban_scheduler::select_in_project(
                &self.config(),
                project,
                Some(&session.profile_id),
            )?;
            let mut terminal: TerminalEntry = self.store.lock().unwrap().db().get(&session.id)?;
            session.state = "starting".into();
            let fresh = session.provider_session_id.is_none();
            if fresh && profile.command().iter().any(|s| s.contains("{session_id}")) {
                session.provider_session_id = Some(super::runtime::uuid_session_id());
            }
            terminal.runtime = Some(TerminalRuntime {
                machine: "local".into(),
                owner_core_id: None,
                core_id: None,
                terminal: None,
                state: "starting".into(),
                connection: "connecting".into(),
                error: None,
            });
            self.store.lock().unwrap().write(|db| {
                db.save(&session)?;
                db.save(&terminal)
            })?;
            let result = (|| -> Result<()> {
                let client = canban_herdr::Client {
                    config: self.config(),
                };
                let core = client.status()?["core"]["id"]
                    .as_str()
                    .context("Core ID missing")?
                    .to_owned();
                let prompt = if fresh {
                    session.prompt.clone()
                } else {
                    "Continue the existing Project Workflows configuration conversation. Re-read current sources and active revisions before any modification. Wait for the user's next instruction.".into()
                };
                let cwd = self.home.join("projects").join(&session.project_id);
                std::fs::create_dir_all(&cwd)?;
                let cwd_str = cwd.to_string_lossy().into_owned();
                let project_dir = project
                    .map(|p| p.directory.as_str())
                    .unwrap_or(cwd_str.as_str());
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
                    argv[0] = canban_profiles::executable(&argv[0]);
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
                let receipt = client.launch_on(
                    "local",
                    argv,
                    &cwd_str,
                    env,
                    std::collections::BTreeMap::from([
                        ("canban.configuration".into(), session.id.clone()),
                        ("canban.project".into(), session.project_id.clone()),
                    ]),
                )?;
                let runtime = terminal.runtime.as_mut().unwrap();
                runtime.core_id = Some(core.clone());
                runtime.owner_core_id = Some(core);
                runtime.terminal = Some(strp(&receipt, "address")?.into());
                runtime.state = "running".into();
                runtime.connection = "connected".into();
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
            self.store.lock().unwrap().write(|db| {
                db.save(&session)?;
                db.save(&terminal)
            })?;
            self.emit("configuration", None);
        }
        Ok(())
    }
}
