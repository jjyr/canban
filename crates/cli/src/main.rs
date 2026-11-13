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
    #[command(subcommand)]
    command: Cmd,
}
#[derive(Subcommand)]
enum Cmd {
    /// Show immutable release identity without opening application data.
    BuildInfo,
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
    /// Run the shared AppServer without a UI (also useful for acceptance).
    Serve,
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
    /// Report your own outcome for the Card injected by Canban.
    Report {
        /// Outcome state: completed | backlog | blocked | failed | working
        #[arg(long)]
        state: String,
        #[arg(long, default_value = "")]
        message: String,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long, env = "CANBAN_CARD")]
        card: u64,
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
        eprintln!("canban: {e:#}");
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
}

fn run() -> Result<()> {
    let args = Args::parse();
    let home = args.home.unwrap_or(canban_app_server::default_home()?);
    let endpoint = args.endpoint.unwrap_or_else(|| home.join("app.sock"));
    if matches!(args.command, Cmd::BuildInfo) {
        println!(
            "{}",
            serde_json::to_string(&canban_updater::Build::current())?
        );
        return Ok(());
    }
    if let Cmd::Database { target } = args.command {
        let _startup = canban_updater::handoff::startup_guard(&home)?;
        let _service = canban_updater::service_lock(&home)?;
        let version = canban_storage::migrations::migrate(&home, target)?;
        println!("{}", json!({"schema":version}));
        return Ok(());
    }
    if matches!(args.command, Cmd::Serve) {
        let app = canban_app_server::AppServer::open(home, std::env::current_exe()?)?;
        let _runtime = app.start()?;
        println!("Canban AppServer listening at {}", app.endpoint().display());
        while !app.stopped() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        return Ok(());
    }
    if let Cmd::Workflow { command } = &args.command {
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
                println!("{}", serde_json::to_string_pretty(&value)?);
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
        }
        return Ok(());
    }
    let raw_note = matches!(
        &args.command,
        Cmd::Note {
            command: NoteCmd::Read { json: false, .. }
        }
    );
    let (method, mut p) = match args.command {
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
            project,
            request_key,
        } => {
            let mut params = card_scope(Some(card), project);
            params["request_key"] = json!(request_key.unwrap_or_else(canban_types::id));
            params["state"] = json!(state);
            params["message"] = json!(message);
            params["body"] = json!(file.map(std::fs::read_to_string).transpose()?);
            ("execution.report".into(), params)
        }
        Cmd::Serve | Cmd::BuildInfo | Cmd::Database { .. } => unreachable!(),
    };
    anyhow::ensure!(p.is_object(), "Parameters must be an object");
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
            | "workflow.decide"
            | "workflow.deliver"
            | "workflow.stop"
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
    let retryable = matches!(method.as_str(), "execution.report" | "agent.reply")
        && endpoint == home.join("app.sock");
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
                std::fs::remove_file(path)?;
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
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
