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
    /// Read and edit database Notes using file-like input/output.
    Note {
        #[command(subcommand)]
        command: NoteCmd,
    },
    /// Bind the actual provider conversation supplied by your profile integration.
    BindSession {
        #[arg(long, env = "CANBAN_SESSION_ID")]
        session_id: String,
        #[arg(long, env = "CANBAN_REQUEST_ID")]
        request_id: String,
        #[arg(long, env = "CANBAN_ATTEMPT")]
        attempt: u32,
    },
    /// Publish a reply without completing the current task.
    Reply {
        #[arg(long = "to")]
        question_id: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long, env = "CANBAN_REQUEST_ID")]
        request_id: String,
        #[arg(long, env = "CANBAN_ATTEMPT")]
        attempt: u32,
    },
    /// Compatibility shim for an older in-flight callback; lifecycle is observed through Herdr.
    #[command(hide = true)]
    AgentNotify {
        provider: String,
        payload: Option<String>,
        #[arg(long, env = "CANBAN_REQUEST_ID")]
        request_id: String,
        #[arg(long, env = "CANBAN_ATTEMPT")]
        attempt: u32,
    },
    /// Report your own outcome for the execution injected by Canban.
    Report {
        #[arg(long)]
        state: String,
        #[arg(long, default_value = "")]
        message: String,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long, env = "CANBAN_REQUEST_ID")]
        request_id: String,
        #[arg(long, env = "CANBAN_ATTEMPT")]
        attempt: u32,
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
fn main() {
    if let Err(e) = run() {
        eprintln!("canban: {e:#}");
        std::process::exit(1)
    }
}
fn run() -> Result<()> {
    let args = Args::parse();
    let home = args.home.unwrap_or(canban_app_server::default_home()?);
    let endpoint = args.endpoint.unwrap_or_else(|| home.join("app.sock"));
    if matches!(args.command, Cmd::Serve) {
        let app = canban_app_server::AppServer::open(home, std::env::current_exe()?)?;
        let _runtime = app.start()?;
        println!("Canban AppServer listening at {}", app.endpoint().display());
        while !app.stopped() {
            std::thread::sleep(std::time::Duration::from_millis(100));
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
            serde_json::from_str::<Value>(&params).context("Parameters must be a JSON object")?,
        ),
        Cmd::BindSession {
            session_id,
            request_id,
            attempt,
        } => (
            "agent.bind".into(),
            json!({"session_id":session_id,"request_id":request_id,"attempt":attempt}),
        ),
        Cmd::Reply {
            question_id,
            file,
            request_id,
            attempt,
        } => (
            "agent.reply".into(),
            json!({"request_key":format!("reply:{question_id}"),"question_id":question_id,"request_id":request_id,"attempt":attempt,"body":std::fs::read_to_string(file)?}),
        ),
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
            request_id,
            attempt,
            request_key,
        } => {
            let key = request_key.unwrap_or_else(|| {
                if state == "completed" {
                    format!("completed:{request_id}:{attempt}")
                } else {
                    canban_types::id()
                }
            });
            let body = file.map(std::fs::read_to_string).transpose()?;
            (
                "execution.report".into(),
                json!({"request_key":key,"execution":{"request_id":request_id,"attempt_number":attempt},"state":state,"message":message,"body":body}),
            )
        }
        Cmd::Serve => unreachable!(),
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
    ) && p.get("execution").is_none()
    {
        let request = std::env::var("CANBAN_REQUEST_ID").ok();
        let attempt = std::env::var("CANBAN_ATTEMPT").ok();
        if request.is_some() || attempt.is_some() {
            p["execution"] = json!({"request_id":request.context("CANBAN_REQUEST_ID missing")?,"attempt_number":attempt.context("CANBAN_ATTEMPT missing")?.parse::<u32>()?});
        }
    }
    let result = canban_app_server::rpc(&endpoint, &method, p)?;
    if raw_note {
        print!("{}", result["body"].as_str().context("Note body missing")?);
        return Ok(());
    }
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
