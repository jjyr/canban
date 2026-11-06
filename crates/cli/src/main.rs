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
    /// Bind the current provider conversation (Codex tools inherit CODEX_THREAD_ID).
    BindSession {
        #[arg(long, env = "CODEX_THREAD_ID")]
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
    /// Provider lifecycle callback; Codex appends JSON, Claude sends stdin JSON.
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
fn main() {
    if let Err(e) = run() {
        eprintln!("canban: {e:#}");
        std::process::exit(1)
    }
}
fn run() -> Result<()> {
    let args = Args::parse();
    let lifecycle = matches!(&args.command, Cmd::AgentNotify { .. });
    let home = args.home.unwrap_or(canban_app_server::default_home()?);
    let endpoint = args.endpoint.unwrap_or_else(|| home.join("app.sock"));
    if matches!(args.command, Cmd::Serve) {
        let app = canban_app_server::AppServer::open(home, std::env::current_exe()?)?;
        let _runtime = app.start()?;
        println!("Canban AppServer listening at {}", app.endpoint().display());
        loop {
            std::thread::park();
        }
    }
    let (method, mut p) = match args.command {
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
        Cmd::AgentNotify {
            provider,
            payload,
            request_id,
            attempt,
        } => {
            let body = match payload {
                Some(v) => v,
                None => {
                    let mut v = String::new();
                    std::io::stdin().read_to_string(&mut v)?;
                    v
                }
            };
            let event: Value = serde_json::from_str(&body)?;
            let (session, text) = if provider == "codex" {
                anyhow::ensure!(
                    event["type"] == "agent-turn-complete",
                    "Unsupported notification"
                );
                (
                    event["thread-id"].clone(),
                    event["last-assistant-message"].clone(),
                )
            } else {
                anyhow::ensure!(
                    event["hook_event_name"] == "Stop",
                    "Only root Stop events are accepted"
                );
                (
                    event["session_id"].clone(),
                    event["last_assistant_message"].clone(),
                )
            };
            (
                "agent.turn_ended".into(),
                json!({"request_id":request_id,"attempt":attempt,"session_id":session,"body":text}),
            )
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
                json!({"request_key":key,"execution":{"request_id":request_id,"attempt_number":attempt},"state":state,"message":message,"body":body,"session_id":std::env::var("CODEX_THREAD_ID").ok().filter(|id| !id.is_empty() && std::env::var("CANBAN_PROVIDER").as_deref() == Ok("codex"))}),
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
            | "resource.read"
            | "execution.output"
    ) && p.get("request_key").is_none()
    {
        p["request_key"] = json!(canban_types::id());
    }
    let result = canban_app_server::rpc(&endpoint, &method, p)?;
    if lifecycle {
        println!("{{}}");
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(())
}
