use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::{io::Read, path::PathBuf};

#[derive(Parser)]
#[command(
    name = "canban",
    version,
    about = "Canban local task and Agent interface"
)]
struct Args {
    #[arg(long, env = "CANBAN_HOME")]
    home: Option<PathBuf>,
    #[arg(long, env = "CANBAN_ENDPOINT")]
    endpoint: Option<PathBuf>,
    /// Print the bundled Canban CLI SKILL.md to stdout and exit.
    #[arg(long)]
    skill: bool,
    #[command(flatten)]
    output: Output,
    #[command(subcommand)]
    command: Option<Cmd>,
}
/// Formatting is local; content selection is an AppServer responsibility.
#[derive(clap::Args, Default)]
struct Output {
    /// Pretty-print the selected JSON for human reading. Does not fetch more data.
    #[arg(long, global = true)]
    human: bool,
    /// Select response fields (comma-separated). See the RPC response contract.
    #[arg(long, global = true, value_delimiter = ',', conflicts_with = "full")]
    fields: Option<Vec<String>>,
    /// Request the complete business view, still with normalized comment bodies.
    #[arg(long, global = true, conflicts_with = "fields")]
    full: bool,
}
impl Output {
    fn params(&self, method: &str, p: &mut Value) -> Result<()> {
        if self.full || self.fields.is_some() {
            anyhow::ensure!(
                p.get("view").is_none() && p.get("fields").is_none(),
                "Specify output selection in flags or RPC parameters, not both"
            );
        }
        if self.full {
            p["view"] = json!("full");
        }
        if let Some(fields) = &self.fields {
            p["fields"] = json!(fields);
        }
        canban_app_server::representation::validate(method, p)
    }
    fn encode(&self, value: &Value) -> Result<String> {
        Ok(if self.human {
            serde_json::to_string_pretty(value)?
        } else {
            serde_json::to_string(value)?
        })
    }
}
#[derive(Subcommand)]
enum Cmd {
    /// Show immutable release identity without opening application data.
    BuildInfo,
    /// Print the bundled Canban CLI SKILL.md without opening application data.
    Skill,
    /// Submit/export YAML; SQLite remains the only configuration source.
    Workflow {
        #[command(subcommand)]
        command: WorkflowCmd,
    },
    /// Apply a supported schema up/down batch without starting product services.
    Database {
        #[arg(long)]
        target: u32,
    },
    /// Run the shared AppServer without a UI (also useful for remote daemons and acceptance).
    Serve {
        /// Bind to the host's Tailscale IPv4 address for direct remote client access.
        #[arg(long)]
        tailscale: bool,
        /// TCP port when `--tailscale` is enabled (default: 7680).
        #[arg(long, default_value_t = canban_types::DEFAULT_DAEMON_PORT)]
        port: u16,
        /// Explicit private/loopback TCP bind address (`ip:port`).
        #[arg(long)]
        listen: Option<String>,
    },
    /// Generate or display the authenticated TCP pairing URI (`canban://addr:port#token`).
    Pair {
        #[arg(long)]
        tailscale: bool,
        #[arg(long, default_value_t = canban_types::DEFAULT_DAEMON_PORT)]
        port: u16,
        #[arg(long)]
        listen: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Install or inspect the user background daemon (`systemd --user` on Linux, `launchd` on macOS).
    Service {
        #[command(subcommand)]
        command: ServiceCmd,
    },
    /// Call a public operation with JSON parameters. See docs/interfaces.
    Call {
        method: String,
        #[arg(default_value = "{}")]
        params: String,
    },
    Snapshot,
    Status,
    /// Update or adopt a workflow stage on the current Card from an Agent session.
    Card {
        #[command(subcommand)]
        command: CardCmd,
    },
    /// Legacy Note compatibility for already captured executions.
    #[command(hide = true)]
    Note {
        #[command(subcommand)]
        command: NoteCmd,
    },
    /// Bind the actual provider conversation supplied by your profile integration.
    BindSession {
        #[arg(long, env = "CANBAN_SESSION_ID")]
        session_id: String,
        #[arg(long, env = "CANBAN_CARD")]
        card: Option<u64>,
        #[arg(long, env = "CANBAN_PROJECT")]
        project: Option<String>,
    },
    /// Publish a reply without completing the current task.
    Reply {
        #[arg(long = "to")]
        question_id: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long, env = "CANBAN_CARD")]
        card: Option<u64>,
        #[arg(long, env = "CANBAN_PROJECT")]
        project: Option<String>,
    },
    /// Compatibility shim for an older in-flight callback; lifecycle is observed through Herdr.
    #[command(hide = true)]
    AgentNotify {
        provider: String,
        payload: Option<String>,
    },
    /// Report your own outcome for the Card or Scheduled Run injected by Canban.
    Report {
        /// Outcome state: completed | backlog | blocked | failed | working
        #[arg(long)]
        state: String,
        #[arg(long, default_value = "")]
        message: String,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long, env = "CANBAN_CARD")]
        card: Option<u64>,
        #[arg(long, env = "CANBAN_SCHEDULE_RUN_ID")]
        schedule_run_id: Option<String>,
        #[arg(long, env = "CANBAN_PROJECT")]
        project: Option<String>,
        #[arg(long)]
        request_key: Option<String>,
    },
    /// Inspect, trigger, or toggle scheduled tasks configured in Project workflows.
    Schedule {
        #[command(subcommand)]
        command: ScheduleCmd,
    },
    /// Batch merge queue reporting for a project `_merge_queue` Agent session.
    Batch {
        #[command(subcommand)]
        command: BatchCmd,
    },
}
#[derive(Subcommand)]
enum ServiceCmd {
    /// Install and enable the background daemon service.
    Install {
        #[arg(long)]
        tailscale: bool,
        #[arg(long, default_value_t = canban_types::DEFAULT_DAEMON_PORT)]
        port: u16,
        #[arg(long)]
        listen: Option<String>,
    },
    /// Show background service installation and active status.
    Status,
}
#[derive(Subcommand)]
enum ScheduleCmd {
    /// List configured scheduled tasks and recent runs.
    List {
        #[arg(long, env = "CANBAN_PROJECT")]
        project_id: Option<String>,
    },
    /// List recent execution history for scheduled tasks.
    Runs {
        #[arg(long, env = "CANBAN_PROJECT")]
        project_id: Option<String>,
        #[arg(long)]
        schedule_id: Option<String>,
        #[arg(long, default_value_t = 25)]
        limit: u64,
    },
    /// Manually trigger a scheduled task immediately.
    Trigger {
        #[arg(long)]
        schedule_id: Option<String>,
        #[arg(long, env = "CANBAN_PROJECT")]
        project_id: Option<String>,
        #[arg(long)]
        filename: Option<String>,
        #[arg(long)]
        workflow_id: Option<String>,
        #[arg(long, alias = "rule")]
        rule_id: Option<String>,
        #[arg(long)]
        request_key: Option<String>,
    },
    /// Enable or disable a scheduled task rule in its Workflow YAML.
    Toggle {
        #[arg(long)]
        schedule_id: Option<String>,
        #[arg(long, env = "CANBAN_PROJECT")]
        project_id: Option<String>,
        #[arg(long)]
        filename: Option<String>,
        #[arg(long, alias = "rule")]
        rule_id: Option<String>,
        #[arg(long, action = clap::ArgAction::Set)]
        enabled: Option<bool>,
        #[arg(long, conflicts_with_all = ["enabled", "off"])]
        on: bool,
        #[arg(long, conflicts_with_all = ["enabled", "on"])]
        off: bool,
        #[arg(long)]
        request_key: Option<String>,
    },
}
#[derive(Subcommand)]
enum BatchCmd {
    /// Report batch resolution outcome (and optionally evict unresolvable Cards).
    Report {
        /// Outcome state: completed | blocked | failed | working
        #[arg(long)]
        state: String,
        #[arg(long, default_value = "")]
        message: String,
        #[arg(long)]
        file: Option<PathBuf>,
        /// Card number or ID to evict back to human review (repeatable).
        #[arg(long)]
        evict: Vec<String>,
        #[arg(long, env = "CANBAN_BATCH_ID")]
        batch: Option<String>,
        #[arg(long, env = "CANBAN_PROJECT")]
        project: Option<String>,
        #[arg(long)]
        request_key: Option<String>,
    },
}
#[derive(Subcommand)]
enum CardCmd {
    /// Show the current Card's details and discussion.
    Show {
        #[arg(long, env = "CANBAN_CARD")]
        card: u64,
        #[arg(long, env = "CANBAN_PROJECT")]
        project: Option<String>,
    },
    /// Update the current Card's title and/or Markdown body.
    Update {
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long, env = "CANBAN_CARD")]
        card: u64,
        #[arg(long, env = "CANBAN_PROJECT")]
        project: Option<String>,
        #[arg(long)]
        request_key: Option<String>,
    },
    /// Move the current Card to a stage and adopt its workflow in this session without re-triggering scheduling.
    Adopt {
        #[arg(long, default_value = "develop")]
        stage: String,
        #[arg(long, env = "CANBAN_CARD")]
        card: u64,
        #[arg(long, env = "CANBAN_PROJECT")]
        project: Option<String>,
        #[arg(long)]
        request_key: Option<String>,
    },
}
#[derive(Subcommand)]
enum NoteCmd {
    List {
        #[arg(long)]
        project_id: Option<String>,
    },
    Read {
        note_id: String,
        #[arg(long)]
        json: bool,
    },
    Create {
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        file: Option<PathBuf>,
    },
    Write {
        note_id: String,
        #[arg(long)]
        revision: u64,
        #[arg(long)]
        file: Option<PathBuf>,
    },
    Rename {
        note_id: String,
        title: String,
        #[arg(long)]
        revision: u64,
    },
    History {
        note_id: String,
    },
    Revision {
        note_id: String,
        revision: u64,
    },
}
fn input_body(file: Option<PathBuf>) -> Result<String> {
    if let Some(file) = file.filter(|p| p.as_os_str() != "-") {
        return Ok(std::fs::read_to_string(file)?);
    }
    let mut body = String::new();
    std::io::stdin().read_to_string(&mut body)?;
    Ok(body)
}
fn card_scope(card: Option<u64>, project: Option<String>) -> Value {
    let mut params = json!({});
    if let Some(n) = card {
        params["card"] = json!(n);
    }
    if let Some(proj) = project.filter(|s| !s.trim().is_empty()) {
        params["project_id"] = json!(proj);
    } else if let Ok(cwd) = std::env::current_dir() {
        params["cwd"] = json!(cwd);
    }
    if let Ok(profile) = std::env::var("CANBAN_PROFILE_ID")
        && !profile.trim().is_empty()
    {
        params["profile_id"] = json!(profile);
    }
    params
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{}", json!({"error":{"message":format!("{e:#}")}}));
        std::process::exit(1)
    }
}
#[derive(Subcommand)]
enum WorkflowCmd {
    Submit {
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        revision: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        check: bool,
    },
    Export {
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Save a Project's published workflow collection as the global default for new Projects.
    SetDefault {
        #[arg(long)]
        project_id: String,
    },
    /// Reset the global default workflow for new Projects back to the built-in default.
    ResetDefault,
}

fn run() -> Result<()> {
    let args = Args::parse();
    let output = args.output;
    if args.skill || matches!(args.command, Some(Cmd::Skill)) {
        print!("{}", canban_context::SKILL_MD);
        return Ok(());
    }
    let Some(command) = args.command else {
        use clap::CommandFactory;
        Args::command().print_help()?;
        println!();
        return Ok(());
    };
    let home = args.home.unwrap_or(canban_app_server::default_home()?);
    let endpoint = args.endpoint.unwrap_or_else(|| home.join("app.sock"));
    if matches!(command, Cmd::BuildInfo) {
        println!(
            "{}",
            serde_json::to_string(&canban_updater::Build::current())?
        );
        return Ok(());
    }
    if let Cmd::Database { target } = command {
        let _startup = canban_updater::handoff::startup_guard(&home)?;
        let _service = canban_updater::service_lock(&home)?;
        let version = canban_storage::migrations::migrate(&home, target)?;
        println!("{}", json!({"schema":version}));
        return Ok(());
    }
    if let Cmd::Serve {
        tailscale,
        port,
        listen,
    } = command
    {
        let override_listen = if tailscale || listen.is_some() {
            let cfg = canban_types::ServerListenConfig {
                tailscale,
                port,
                listen,
            };
            canban_app_server::network::save_listen_config(&home, &cfg)?;
            Some(cfg)
        } else {
            None
        };
        let app = canban_app_server::AppServer::open(home, std::env::current_exe()?)?;
        let _runtime = app.start_with_listen(override_listen)?;
        println!("Canban AppServer listening at {}", app.endpoint().display());
        for tcp in app.bound_tcp_addrs() {
            println!("Canban TCP listener bound at {tcp}");
        }
        while !app.stopped() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        return Ok(());
    }
    if let Cmd::Pair {
        tailscale,
        port,
        listen,
        json: as_json,
    } = command
    {
        let mut cfg = canban_app_server::network::load_listen_config(&home);
        if tailscale {
            cfg.tailscale = true;
        }
        if port != canban_types::DEFAULT_DAEMON_PORT || cfg.port == 0 {
            cfg.port = port;
        }
        if listen.is_some() {
            cfg.listen = listen;
        }
        if !cfg.tcp_enabled() {
            cfg.tailscale = true;
        }
        canban_app_server::network::save_listen_config(&home, &cfg)?;
        let token = canban_app_server::network::ensure_server_token(&home)?;
        let addr = if let Some(explicit) = cfg.listen.as_deref() {
            canban_types::normalize_daemon_addr(explicit)
        } else if let Ok(ts_ip) = canban_app_server::network::detect_tailscale_ipv4() {
            format!("{ts_ip}:{}", cfg.port)
        } else {
            format!("<host-ip>:{}", cfg.port)
        };
        let uri = canban_types::format_pairing_uri(&addr, &token);
        if as_json {
            let sid = std::fs::read_to_string(home.join("server-id"))
                .ok()
                .map(|s| s.trim().to_owned());
            println!(
                "{}",
                json!({
                    "addr": addr,
                    "token": token,
                    "uri": uri,
                    "server_id": sid,
                    "hostname": canban_app_server::network::hostname_string(),
                    "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
                })
            );
        } else {
            println!("{uri}");
        }
        return Ok(());
    }
    if let Cmd::Service { command } = command {
        match command {
            ServiceCmd::Install {
                tailscale,
                port,
                listen,
            } => {
                let status = canban_app_server::network::install_user_service(
                    &home,
                    &std::env::current_exe()?,
                    tailscale,
                    port,
                    listen,
                )?;
                println!("{}", output.encode(&status)?);
            }
            ServiceCmd::Status => {
                let status = canban_app_server::network::service_status(&home);
                println!("{}", output.encode(&status)?);
            }
        }
        return Ok(());
    }
    if let Cmd::Workflow { command } = &command {
        let mut selection = json!({});
        output.params("workflow.sources", &mut selection)?;
        match command {
            WorkflowCmd::Submit {
                project_id,
                revision,
                file,
                name,
                check,
            } => {
                let collection = canban_app_server::rpc(
                    &endpoint,
                    "workflow.sources",
                    json!({"project_id":project_id}),
                )?;
                anyhow::ensure!(
                    collection["revision"].as_str() == Some(revision),
                    "revision_conflict: read workflow.sources before submitting"
                );
                let name = name
                    .clone()
                    .or_else(|| file.file_name().map(|s| s.to_string_lossy().into_owned()))
                    .context("Source name required")?;
                let raw = std::fs::read_to_string(file)?;
                let mut sources = collection["sources"]
                    .as_array()
                    .context("No Workflow sources")?
                    .iter()
                    .filter(|s| s["filename"] != name)
                    .map(|s| json!({"filename":s["filename"],"source":s["source"]}))
                    .collect::<Vec<_>>();
                sources.push(json!({"filename":name,"source":raw}));
                let value = canban_app_server::rpc(
                    &endpoint,
                    if *check {
                        "workflow.check"
                    } else {
                        "workflow.apply"
                    },
                    json!({"project_id":project_id,"revision":revision,"sources":sources,"request_key":canban_types::id()}),
                )?;
                println!("{}", output.encode(&value)?);
            }
            WorkflowCmd::Export {
                project_id,
                name,
                output,
            } => {
                use std::io::Write;
                let collection = canban_app_server::rpc(
                    &endpoint,
                    "workflow.sources",
                    json!({"project_id":project_id}),
                )?;
                let source = collection["sources"]
                    .as_array()
                    .context("No Workflow sources")?
                    .iter()
                    .find(|s| s["filename"] == *name)
                    .context("Workflow source not found")?;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(output)
                    .context(
                        "Choose a new output path; export never overwrites an existing file",
                    )?;
                file.write_all(
                    source["source"]
                        .as_str()
                        .context("YAML missing")?
                        .as_bytes(),
                )?;
                file.sync_all()?;
                println!(
                    "{}",
                    json!({"output":output,"revision":collection["revision"]})
                );
            }
            WorkflowCmd::SetDefault { project_id } => {
                let value = canban_app_server::rpc(
                    &endpoint,
                    "workflow.default.set",
                    json!({"project_id": project_id, "request_key": canban_types::id()}),
                )?;
                println!("{}", output.encode(&value)?);
            }
            WorkflowCmd::ResetDefault => {
                let value = canban_app_server::rpc(
                    &endpoint,
                    "workflow.default.reset",
                    json!({"request_key": canban_types::id()}),
                )?;
                println!("{}", output.encode(&value)?);
            }
        }
        return Ok(());
    }
    let raw_note = matches!(
        &command,
        Cmd::Note {
            command: NoteCmd::Read { json: false, .. }
        }
    );
    let (method, mut p) = match command {
        Cmd::Workflow { .. } => unreachable!(),
        Cmd::Card { command } => match command {
            CardCmd::Show { card, project } => {
                ("card.show".into(), card_scope(Some(card), project))
            }
            CardCmd::Update {
                title,
                file,
                card,
                project,
                request_key,
            } => {
                anyhow::ensure!(
                    title.is_some() || file.is_some(),
                    "Supply --title and/or --file to update the Card"
                );
                let mut params = card_scope(Some(card), project);
                params["agent"] = json!(true);
                if let Some(key) = request_key {
                    params["request_key"] = json!(key);
                }
                if let Some(t) = title {
                    params["title"] = json!(t);
                }
                if let Some(f) = file {
                    params["body"] = json!(input_body(Some(f))?);
                }
                ("card.update".into(), params)
            }
            CardCmd::Adopt {
                stage,
                card,
                project,
                request_key,
            } => {
                let mut params = card_scope(Some(card), project);
                params["request_key"] = json!(request_key.unwrap_or_else(canban_types::id));
                params["stage"] = json!(stage);
                ("card.adopt".into(), params)
            }
        },
        Cmd::Note { command } => match command {
            NoteCmd::List { project_id } => ("note.list".into(), json!({"project_id":project_id})),
            NoteCmd::Read { note_id, .. } => ("note.read".into(), json!({"note_id":note_id})),
            NoteCmd::Create {
                project_id,
                title,
                file,
            } => (
                "note.save".into(),
                json!({"project_id":project_id,"title":title,"body":input_body(file)?}),
            ),
            NoteCmd::Write {
                note_id,
                revision,
                file,
            } => (
                "note.write".into(),
                json!({"note_id":note_id,"revision":revision,"body":input_body(file)?}),
            ),
            NoteCmd::Rename {
                note_id,
                title,
                revision,
            } => (
                "note.rename".into(),
                json!({"note_id":note_id,"title":title,"revision":revision}),
            ),
            NoteCmd::History { note_id } => ("note.history".into(), json!({"note_id":note_id})),
            NoteCmd::Revision { note_id, revision } => (
                "note.revision".into(),
                json!({"note_id":note_id,"revision":revision}),
            ),
        },
        Cmd::Call { method, params } => (
            method,
            serde_json::from_str::<Value>(&if let Some(path) = params.strip_prefix('@') {
                std::fs::read_to_string(path)
                    .with_context(|| format!("Could not read parameter file {path}"))?
            } else {
                params
            })
            .context("Parameters must be a JSON object")?,
        ),
        Cmd::BindSession {
            session_id,
            card,
            project,
        } => {
            if let Ok(configuration) = std::env::var("CANBAN_CONFIGURATION_ID") {
                (
                    "configuration.bind".into(),
                    json!({"configuration_id":configuration,"generation":std::env::var("CANBAN_CONFIGURATION_GENERATION")?.parse::<u64>()?,"session_id":session_id}),
                )
            } else if let Ok(schedule_run_id) = std::env::var("CANBAN_SCHEDULE_RUN_ID")
                && !schedule_run_id.trim().is_empty()
            {
                let mut params = card_scope(card, project);
                params["schedule_run_id"] = json!(schedule_run_id);
                params["session_id"] = json!(session_id);
                ("schedule.bind".into(), params)
            } else if let Ok(batch_id) = std::env::var("CANBAN_BATCH_ID")
                && !batch_id.trim().is_empty()
            {
                let mut params = card_scope(card, project);
                params["batch_id"] = json!(batch_id);
                params["session_id"] = json!(session_id);
                ("batch.bind".into(), params)
            } else {
                let mut params = card_scope(card, project);
                params["session_id"] = json!(session_id);
                ("agent.bind".into(), params)
            }
        }
        Cmd::Reply {
            question_id,
            file,
            card,
            project,
        } => {
            let mut params = card_scope(card, project);
            params["request_key"] = json!(format!("reply:{question_id}"));
            params["question_id"] = json!(question_id);
            params["body"] = json!(std::fs::read_to_string(file)?);
            ("agent.reply".into(), params)
        }
        Cmd::AgentNotify { .. } => {
            // Accept old callbacks without duplicating Herdr's lifecycle channel.
            println!("{{}}");
            return Ok(());
        }
        Cmd::Snapshot => ("app.snapshot".into(), json!({})),
        Cmd::Status => ("herdr.status".into(), json!({})),
        Cmd::Report {
            state,
            message,
            file,
            card,
            schedule_run_id,
            project,
            request_key,
        } => {
            let srid = schedule_run_id
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty());
            anyhow::ensure!(
                card.is_some() || srid.is_some(),
                "Supply --card (or CANBAN_CARD) or --schedule-run-id (or CANBAN_SCHEDULE_RUN_ID)"
            );
            let mut params = card_scope(card, project);
            params["request_key"] = json!(request_key.unwrap_or_else(canban_types::id));
            params["state"] = json!(state);
            params["message"] = json!(message);
            params["body"] = json!(file.map(std::fs::read_to_string).transpose()?);
            if let Some(srid) = srid
                && card.is_none()
            {
                params["schedule_run_id"] = json!(srid);
                ("schedule.report".into(), params)
            } else {
                ("execution.report".into(), params)
            }
        }
        Cmd::Schedule { command } => match command {
            ScheduleCmd::List { project_id } => {
                ("schedule.list".into(), json!({ "project_id": project_id }))
            }
            ScheduleCmd::Runs {
                project_id,
                schedule_id,
                limit,
            } => (
                "schedule.runs".into(),
                json!({ "project_id": project_id, "schedule_id": schedule_id, "limit": limit }),
            ),
            ScheduleCmd::Trigger {
                schedule_id,
                project_id,
                filename,
                workflow_id,
                rule_id,
                request_key,
            } => (
                "schedule.trigger".into(),
                json!({
                    "schedule_id": schedule_id,
                    "project_id": project_id,
                    "filename": filename,
                    "workflow_id": workflow_id,
                    "rule_id": rule_id,
                    "request_key": request_key.unwrap_or_else(canban_types::id),
                }),
            ),
            ScheduleCmd::Toggle {
                schedule_id,
                project_id,
                filename,
                rule_id,
                enabled,
                on,
                off,
                request_key,
            } => {
                let target_enabled = if let Some(v) = enabled {
                    v
                } else if on {
                    true
                } else if off {
                    false
                } else {
                    anyhow::bail!("Specify `--enabled <true|false>`, `--on`, or `--off`");
                };
                (
                    "schedule.toggle".into(),
                    json!({
                        "schedule_id": schedule_id,
                        "project_id": project_id,
                        "filename": filename,
                        "rule_id": rule_id,
                        "enabled": target_enabled,
                        "request_key": request_key.unwrap_or_else(canban_types::id),
                    }),
                )
            }
        },
        Cmd::Batch { command } => match command {
            BatchCmd::Report {
                state,
                message,
                file,
                evict,
                batch,
                project,
                request_key,
            } => {
                let mut params = card_scope(None, project);
                if let Some(bid) = batch.filter(|s| !s.trim().is_empty()) {
                    params["batch_id"] = json!(bid);
                }
                if !evict.is_empty() {
                    params["evict"] = json!(evict);
                }
                params["request_key"] = json!(request_key.unwrap_or_else(canban_types::id));
                params["state"] = json!(state);
                params["message"] = json!(message);
                params["body"] = json!(file.map(std::fs::read_to_string).transpose()?);
                ("batch.report".into(), params)
            }
        },
        Cmd::Serve { .. }
        | Cmd::Pair { .. }
        | Cmd::Service { .. }
        | Cmd::BuildInfo
        | Cmd::Skill
        | Cmd::Database { .. } => unreachable!(),
    };
    anyhow::ensure!(p.is_object(), "Parameters must be an object");
    output.params(&method, &mut p)?;
    if !matches!(
        method.as_str(),
        "app.snapshot"
            | "app.ping"
            | "config.get"
            | "herdr.status"
            | "herdr.templates"
            | "profile.presets"
            | "review.preview"
            | "resource.read"
            | "execution.output"
            | "card.show"
            | "card.get"
            | "diagnostics.status"
            | "diagnostics.logs"
            | "host.discover"
            | "host.probe"
            | "schedule.list"
            | "schedule.runs"
            | "schedule.read_artifact"
    ) && p.get("request_key").is_none()
    {
        p["request_key"] = json!(canban_types::id());
    }
    if matches!(
        method.as_str(),
        "note.save"
            | "note.write"
            | "note.rename"
            | "card.continue"
            | "card.move"
            | "execution.retry"
            | "review.merge"
            | "card.archive"
            | "update.install"
            | "update.daemon"
            | "workflow.decide"
            | "workflow.deliver"
            | "workflow.stop"
            | "schedule.trigger"
            | "schedule.toggle"
            | "card.hold_auto_merge"
    ) && p.get("execution").is_none()
        && let Ok(profile) = std::env::var("CANBAN_PROFILE_ID")
        && !profile.trim().is_empty()
    {
        let mut ex = json!({
            "agent": true,
            "profile_id": profile,
        });
        if let Ok(cwd) = std::env::current_dir() {
            ex["cwd"] = json!(cwd);
        }
        if let Ok(card) = std::env::var("CANBAN_CARD")
            && let Ok(n) = card.trim().trim_start_matches('#').parse::<u64>()
        {
            ex["card"] = json!(n);
        }
        if let Ok(proj) = std::env::var("CANBAN_PROJECT")
            && !proj.trim().is_empty()
        {
            ex["project_id"] = json!(proj);
        }
        p["execution"] = ex;
    }
    let retryable = matches!(
        method.as_str(),
        "execution.report" | "batch.report" | "schedule.report" | "agent.reply"
    ) && endpoint == home.join("app.sock");
    let queued = if retryable {
        Some(canban_updater::handoff::enqueue(&home, &method, &p)?)
    } else {
        None
    };
    if retryable && canban_updater::handoff::active(&home) {
        println!("{}", json!({"queued":true,"request_key":p["request_key"]}));
        return Ok(());
    }
    let result = match canban_app_server::rpc(&endpoint, &method, p.clone()) {
        Ok(result) => {
            if let Some(path) = queued {
                let _ = std::fs::remove_file(path);
            }
            result
        }
        Err(error) => {
            if retryable && canban_updater::handoff::active(&home) {
                println!("{}", json!({"queued":true,"request_key":p["request_key"]}));
                return Ok(());
            }
            if let Some(path) = queued {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }
    };
    if raw_note {
        print!("{}", result["body"].as_str().context("Note body missing")?);
        return Ok(());
    }
    println!("{}", output.encode(&result)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_flags_select_server_content_without_changing_human_content() {
        let args = Args::try_parse_from([
            "canban",
            "card",
            "show",
            "--card",
            "111",
            "--fields",
            "number,body",
            "--human",
        ])
        .unwrap();
        let mut p = json!({});
        args.output.params("card.show", &mut p).unwrap();
        assert_eq!(p, json!({"fields":["number","body"]}));
        let value = json!({"body":"text","number":111});
        let human = args.output.encode(&value).unwrap();
        let compact = Output::default().encode(&value).unwrap();
        assert!(human.contains('\n'));
        assert!(!compact.contains('\n'));
        assert_eq!(serde_json::from_str::<Value>(&human).unwrap(), value);
        assert_eq!(serde_json::from_str::<Value>(&compact).unwrap(), value);
        assert!(
            Args::try_parse_from([
                "canban", "--full", "--fields", "body", "card", "show", "--card", "111"
            ])
            .is_err()
        );
        let invalid = Output {
            fields: Some(vec!["typo".into()]),
            ..Default::default()
        };
        assert!(invalid.params("card.create", &mut json!({})).is_err());
        assert!(
            args.output
                .params("comment.delete", &mut json!({}))
                .is_err()
        );
        assert!(
            args.output
                .params("card.show", &mut json!({"fields":["id"]}))
                .is_err()
        );
    }

    #[test]
    fn parses_skill_flag_and_subcommand() {
        let by_flag = Args::try_parse_from(["canban", "--skill"]).unwrap();
        assert!(by_flag.skill);
        assert!(by_flag.command.is_none());

        let by_cmd = Args::try_parse_from(["canban", "skill"]).unwrap();
        assert!(matches!(by_cmd.command, Some(Cmd::Skill)));
    }

    #[test]
    fn parses_batch_report_with_repeatable_evict() {
        let parsed = Args::try_parse_from([
            "canban",
            "batch",
            "report",
            "--state",
            "completed",
            "--evict",
            "2",
            "--evict",
            "5",
            "--message",
            "ok",
        ])
        .unwrap();
        match parsed.command {
            Some(Cmd::Batch {
                command:
                    BatchCmd::Report {
                        state,
                        message,
                        evict,
                        ..
                    },
            }) => {
                assert_eq!(state, "completed");
                assert_eq!(message, "ok");
                assert_eq!(evict, vec!["2", "5"]);
            }
            _ => panic!("expected batch report"),
        }
    }

    #[test]
    fn parses_schedule_toggle_and_trigger_aliases() {
        let p_false = Args::try_parse_from([
            "canban",
            "schedule",
            "toggle",
            "--schedule-id",
            "schedule:p:wf:r",
            "--enabled",
            "false",
        ])
        .unwrap();
        match p_false.command {
            Some(Cmd::Schedule {
                command: ScheduleCmd::Toggle { enabled, .. },
            }) => assert_eq!(enabled, Some(false)),
            _ => panic!("unexpected command"),
        }

        let p_off = Args::try_parse_from([
            "canban",
            "schedule",
            "toggle",
            "--schedule-id",
            "schedule:p:wf:r",
            "--off",
        ])
        .unwrap();
        match p_off.command {
            Some(Cmd::Schedule {
                command: ScheduleCmd::Toggle { off, .. },
            }) => assert!(off),
            _ => panic!("unexpected command"),
        }

        let trig = Args::try_parse_from([
            "canban",
            "schedule",
            "trigger",
            "--project-id",
            "p",
            "--workflow-id",
            "wf",
            "--rule",
            "nightly",
        ])
        .unwrap();
        match trig.command {
            Some(Cmd::Schedule {
                command: ScheduleCmd::Trigger { rule_id, .. },
            }) => assert_eq!(rule_id.as_deref(), Some("nightly")),
            _ => panic!("unexpected command"),
        }
    }
}
