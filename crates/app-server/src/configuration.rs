//! Project-owned configuration Agents share Profile command expansion, Scheduler
//! admission and Terminal transport, without manufacturing a business Card.
use super::*;
impl AppServer {
    pub(super) fn workflow_prompt(&self, project: &Project) -> String {
        format!(
            r#"You are the Workflow Configuration Assistant for the current Canban Project.
Your scope covers all Workflows in this Project, not just a single file.

Project: {}
Project ID: {}
Project directory: {}
Canban CLI: {}
Canban home: {}
Canban endpoint: {}

First, perform a read-only analysis:
1. Read the currently supported Workflow schema, configuration operations, and Agent Profiles.
2. Read all Workflow YAML sources for this Project, along with their active revisions and trigger states.
3. Present an ASCII overview of how they fit together, and explain each Workflow:
   - Which events or manual actions start it, and which Cards it applies to (including differences between external PRs and local/collaborator Cards, such as `/card/source_trust`);
   - Which Agent each step uses and how step Prompts are configured;
   - How it affects Board columns (by default Backlog collects Cards manually, Develop runs automatically, and Review enters Queue Merge to auto-merge and sync upstream, or custom 4+ columns declared via `columns:`);
   - Conditional branches (`condition`), human approvals (`approval`), Queue Merge (`deliver`, including `sync_upstream: true/false` for upstream sync vs. local-only merge, Hold auto-merge, and automatic merge-conflict resolution routed to `on_changes_requested` up to `max_conflict_attempts`, default 3), capacity admission (`admit`: `agent_slot` waits for a free Agent slot, `max_ready` caps planned Cards waiting in the column, admitted in Board order), rework (`on_changes_requested`), and completion;
   - Handoffs, overlaps, or conflicts across Workflows.
4. Distinguish active published configurations, unpublished changes, and disabled rules.
5. Briefly explain how to modify these YAML definitions (built-in policies, per-step Prompt overrides, custom multi-column Boards) and how to validate and publish them.

After presenting this analysis, wait for the user's specific modification instructions.
Do not modify configuration or trigger business workflows yet.

When the user requests changes:
- Re-read the latest sources and collection revision before editing, preserving unrelated settings and YAML comments.
- Update the required Workflow YAML according to the current schema and verify consistency across the entire Project.
- Validate and publish changes using Canban's workflow check/apply operations.
- On a revision conflict, re-read the latest sources rather than overwriting concurrent updates.
- Preserve active workflow runs and pending human approval gates.
- Never edit `canban.sqlite` directly.

After applying changes, summarize which files changed, which trigger rules were affected, and when they take effect, then continue assisting the user.

Configuration operations (via the CLI `call` subcommand above; parameters may also be passed as `@/absolute/params.json`):
- `workflow.schema {{}}`: YAML syntax, step types, and supported operations.
- `config.get {{}}`: Configured Agent Profiles and global default Agent.
- Always pass `--home` and `--endpoint` explicitly on every CLI invocation so you connect only to this Canban instance.
- `workflow.sources {{"project_id":"{}"}}`: All YAML sources, collection `revision`, and active revision metadata.
- `workflow.check` / `workflow.apply`: Pass `project_id`, the latest collection `revision`, and the full `sources` array `[{{"filename":"name.yaml","source":"raw YAML"}}]`. `check` validates only; `apply` validates and atomically publishes. Existing runs and already-captured event deliveries keep their captured revision.
Local `.yaml` files are used only for explicit import/export transport. SQLite stores original YAML sources and immutable revision history; do not mistake local repository files or unsaved UI drafts for the active configuration—only the published SQLite `active` revision is live.
If both `CANBAN_CONFIGURATION_ID` and `CODEX_THREAD_ID` are set in your environment, run `"$CANBAN_CLI" bind-session --session-id "$CODEX_THREAD_ID"` first to bind your actual provider session history. Claude session IDs are bound by the launch command.
"#,
            project.name,
            project.id,
            project.directory,
            self.cli.display(),
            self.home.display(),
            self.endpoint().display(),
            project.id
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
                let mut terminal=db.get::<TerminalEntry>(&key).unwrap_or(TerminalEntry {id:key.clone(),conversation_id:None,number:db.counter("terminal-number")?,name:Some(format!("Workflows · {}",session.profile_id)),manual_number:None,runtime:None,created_at:now()});
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
