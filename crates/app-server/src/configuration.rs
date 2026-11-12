//! Project-owned configuration Agents share Profile command expansion, Scheduler
//! admission and Terminal transport, without manufacturing a business Card.
use super::*;
impl AppServer {
    pub(super) fn workflow_prompt(&self, project: &Project) -> String {
        format!(
            r#"你是当前 Canban 项目的 Workflow 配置助手。
本次范围是整个项目的 Workflows，而非某个选中的文件。

Project: {}
Project ID: {}
Project directory: {}
Canban CLI: {}
Canban home: {}
Canban endpoint: {}

请先完成只读分析：
1. 读取当前支持的 Workflow schema、配置接口和 Agent Profiles。
2. 读取本项目全部 Workflow YAML，以及当前生效的版本和触发状态。
3. 使用 ASCII 展示整体关系，并逐个解释 Workflow：
   - 哪些事件或操作会启动它，适用于哪些 Card（包括外部 PR 与本地/协作者 Card 的规则差异，如 `/card/source_trust`）；
   - 各步骤使用哪个 Agent、各步骤 Prompt 如何设置；
   - 如何影响看板列（默认 Backlog 手动收集、Develop 自动执行、Review 进入 Queue Merge 自动合并并同步上游，或通过 `columns:` 定义的 4 列及更多自定义列）；
   - 条件分支、人工审批（`approval`）、自动合并队列（`deliver`，含 `sync_upstream: true/false` 是否同步上游或仅本地合并，以及 Hold auto-merge）、返工（`on_changes_requested`）和结束方式；
   - 不同 Workflow 之间的衔接、重叠或冲突。
4. 区分已生效配置、未发布修改和已停用规则。
5. 简要说明如何修改这些 YAML（包括内置策略、分步骤 Prompt 覆盖、自定义多列看板），以及如何校验和发布。

完成说明后，等待用户提出具体修改要求。
此时不要修改配置或启动业务流程。

收到修改要求后：
- 修改前重新读取最新源码和版本，保留无关配置与注释。
- 按当前 schema 修改所需 Workflow YAML，并检查整个项目的配置是否一致。
- 使用 Canban 的校验和发布接口应用修改。
- 发生版本冲突时重新读取，不覆盖他人的更新。
- 保留已有运行实例和待审批决定。
- 不直接编辑 SQLite。

完成后说明修改了哪些文件、影响哪些触发规则及何时生效，
然后继续与用户对话。

配置接口（通过上述 CLI 的 call 命令，参数可使用 @/absolute/params.json）：
- workflow.schema {{}}：语法与操作说明。
- config.get {{}}：已配置的 Agent Profiles。
- 所有 CLI 操作显式使用上述 --home 和 --endpoint 参数，不要连接其他 Canban 实例。
- workflow.sources {{"project_id":"{}"}}：全部源码、集合 revision 和 active 引用。
- workflow.check / workflow.apply：传入 project_id、最新集合 revision，以及完整 sources 数组 [{{"filename":"name.yaml","source":"原始 YAML"}}]。check 只校验；apply 校验后立即发布。已有运行和已捕获事件使用旧版本。
本地文件只用于提交或导出，SQLite 保存原始 YAML 和所有版本；不要把本地文件或编辑器草稿误认为已生效配置；active 指向的 SQL revision 才是生效源码。
若环境同时提供 CANBAN_CONFIGURATION_ID 和 CODEX_THREAD_ID，请先运行 "$CANBAN_CLI" bind-session --session-id "$CODEX_THREAD_ID"，绑定实际历史。Claude 的会话 ID 由启动命令绑定。
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
                let profile=canban_scheduler::select(&self.config(),Some(strp(&p,"profile_id")?))?;
                let key=format!("configuration:{}:workflows:{}",project.id,profile.id);
                let old=db.get::<ConfigurationSession>(&key).ok();
                if let Some(old)=&old {
                    let terminal=db.get::<TerminalEntry>(&key)?;
                    if terminal.runtime.as_ref().is_some_and(|r|r.state!="finished") || old.state=="queued" {
                        ensure!(p["fresh"]!=true,"End the existing configuration Agent before starting a new conversation");
                        return Ok(json!({"terminal_id":key,"reused":true,"state":terminal.runtime.as_ref().map(|r|r.state.as_str()).unwrap_or("queued")}));
                    }
                    ensure!(old.provider_session_id.is_some() || p["fresh"]==true,"Exact provider history unavailable; explicitly start a new conversation");
                    ensure!(p["fresh"]==true || !profile.resume_command().is_empty(),"Profile does not support Resume");
                }
                let fresh=p["fresh"]==true || old.is_none();
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
            let profile = canban_scheduler::select(&self.config(), Some(&session.profile_id))?;
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
                let mut argv = canban_profiles::command(
                    &profile,
                    session.provider_session_id.as_deref(),
                    fresh,
                    &[
                        ("home", &self.home.to_string_lossy()),
                        ("cli", &self.cli.to_string_lossy()),
                        ("prompt", &prompt),
                    ],
                )?;
                argv[0] = canban_profiles::executable(&argv[0]);
                let cwd = self.home.join("projects").join(&session.project_id);
                std::fs::create_dir_all(&cwd)?;
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
                    &cwd.to_string_lossy(),
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
