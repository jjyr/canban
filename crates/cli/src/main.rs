use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::{io::Read, path::PathBuf};

const SKILL_HELP: &str = "Agent skill:
  Read the bundled instructions: canban skill (or canban --skill).
  To add the skill to your agent, create canban/SKILL.md in that agent's skills
  directory and write the output of canban skill into that file.";

#[derive(Parser)]
#[command(
    name = "canban",
    version,
    about = "Canban local task and Agent interface",
    after_help = SKILL_HELP
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
    #[command(after_help = SKILL_HELP)]
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
        /// Allow binding `--listen` to non-loopback / non-Tailscale addresses (e.g. WireGuard/ZeroTier or 0.0.0.0).
        #[arg(long)]
        allow_insecure_bind: bool,
    },
    /// Generate a single-use TCP pairing URI (`canban://addr:port#pair_...`) or issue a per-device token.
    Pair {
        #[arg(long)]
        tailscale: bool,
        #[arg(long, default_value_t = canban_types::DEFAULT_DAEMON_PORT)]
        port: u16,
        #[arg(long)]
        listen: Option<String>,
        #[arg(long)]
        allow_insecure_bind: bool,
        /// Issue a permanent revocable device token directly for this device name instead of a one-time pairing code.
        #[arg(long)]
        device_name: Option<String>,
        /// Time-to-live in seconds for a single-use pairing code (default: 300).
        #[arg(long, default_value_t = 300)]
        ttl_secs: u64,
        #[arg(long)]
        json: bool,
    },
    /// Manage paired remote devices and single-use pairing invites on this daemon.
    Device {
        #[command(subcommand)]
        command: DeviceCmd,
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
    /// Inspect, create, trigger, toggle, or delete Global and Project scheduled tasks.
    Schedule {
        #[command(subcommand)]
        command: ScheduleCmd,
    },
    /// Inspect or garbage-collect merged worktrees across projects.
    Worktree {
        #[command(subcommand)]
        command: WorktreeCmd,
    },
    /// Batch merge queue reporting for a project `_merge_queue` Agent session.
    Batch {
        #[command(subcommand)]
        command: BatchCmd,
    },
    /// Stage or clear local unpublished PR/diff review drafts for the current Card.
    Review {
        #[command(subcommand)]
        command: ReviewCmd,
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
        #[arg(long)]
        allow_insecure_bind: bool,
    },
    /// Show background service installation and active status.
    Status,
}
#[derive(Subcommand)]
enum DeviceCmd {
    /// List all paired remote devices and whether legacy `server.token` still exists.
    List,
    /// Generate a single-use short-lived pairing invite URI on the running daemon.
    Invite {
        #[arg(long, default_value_t = 300)]
        ttl_secs: u64,
    },
    /// Immediately revoke a paired device by ID and disconnect active sessions.
    Revoke { device_id: String },
    /// Remove the deprecated static `server.token` file once all hosts have upgraded.
    RemoveLegacyToken,
}
#[derive(Subcommand)]
enum ScheduleCmd {
    /// List configured scheduled tasks and recent runs.
    List {
        #[arg(long, env = "CANBAN_PROJECT", conflicts_with = "global")]
        project_id: Option<String>,
        /// Filter to Global (`__global`) host-level schedules.
        #[arg(long)]
        global: bool,
    },
    /// List recent execution history for scheduled tasks.
    Runs {
        #[arg(long, env = "CANBAN_PROJECT", conflicts_with = "global")]
        project_id: Option<String>,
        /// Filter to Global (`__global`) host-level schedules.
        #[arg(long)]
        global: bool,
        #[arg(long)]
        schedule_id: Option<String>,
        #[arg(long, default_value_t = 25)]
        limit: u64,
    },
    /// Create a new Global or Project scheduled task workflow.
    Create(Box<ScheduleCreateArgs>),
    /// Manually trigger a scheduled task immediately.
    Trigger {
        #[arg(long)]
        schedule_id: Option<String>,
        #[arg(long, env = "CANBAN_PROJECT", conflicts_with = "global")]
        project_id: Option<String>,
        #[arg(long)]
        global: bool,
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
        #[arg(long, env = "CANBAN_PROJECT", conflicts_with = "global")]
        project_id: Option<String>,
        #[arg(long)]
        global: bool,
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
    /// Remove a scheduled trigger rule (and standalone schedule file when empty).
    Delete {
        #[arg(long)]
        schedule_id: Option<String>,
        #[arg(long, env = "CANBAN_PROJECT", conflicts_with = "global")]
        project_id: Option<String>,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        filename: Option<String>,
        #[arg(long, alias = "rule")]
        rule_id: Option<String>,
        #[arg(long)]
        request_key: Option<String>,
    },
    /// Read a log or report artifact produced by a scheduled run.
    Artifact {
        #[arg(long, env = "CANBAN_SCHEDULE_RUN_ID")]
        schedule_run_id: String,
        #[arg(long)]
        path: String,
    },
}
#[derive(clap::Args)]
struct ScheduleCreateArgs {
    #[arg(long, env = "CANBAN_PROJECT", conflicts_with = "global")]
    project_id: Option<String>,
    /// Create under Global (`__global`) scope instead of a Project.
    #[arg(long)]
    global: bool,
    #[arg(long)]
    name: String,
    #[arg(long)]
    cron: String,
    #[arg(long)]
    timezone: Option<String>,
    #[arg(long, default_value = "skip")]
    overlap: String,
    /// Mode: agent | script | script_agent | card.
    #[arg(long, default_value = "agent")]
    mode: String,
    #[arg(long)]
    script: Option<String>,
    #[arg(long)]
    shell: Option<String>,
    #[arg(long)]
    working_directory: Option<String>,
    #[arg(long)]
    timeout_minutes: Option<u32>,
    #[arg(long)]
    profile_id: Option<String>,
    #[arg(long)]
    instruction: Option<String>,
    #[arg(long)]
    card_column: Option<String>,
    #[arg(long)]
    card_title: Option<String>,
    #[arg(long)]
    card_body: Option<String>,
    #[arg(long)]
    filename: Option<String>,
    #[arg(long)]
    disabled: bool,
    #[arg(long)]
    request_key: Option<String>,
}
#[derive(Subcommand)]
enum WorktreeCmd {
    /// Remove clean worktrees for Cards resolved or merged before the retention window.
    Gc {
        /// Retention window (e.g. 7d, 24h, 30m).
        #[arg(long, alias = "resolved-before", default_value = "7d")]
        merged_before: String,
        /// Restrict cleanup to a specific Project id.
        #[arg(long)]
        project_id: Option<String>,
        /// Report candidates without deleting worktrees.
        #[arg(long)]
        dry_run: bool,
        /// Write skipped dirty/active worktrees to $CANBAN_ENV under this variable name.
        #[arg(long)]
        env_dirty: Option<String>,
        /// Exit non-zero if any dirty or active worktrees could not be removed.
        #[arg(long)]
        fail_on_dirty: bool,
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
enum ReviewCmd {
    /// Save a local unpublished PR/diff review draft (summary, verdict, inline line comments).
    Draft {
        /// Path to JSON file (`-` for stdin) containing `{ summary, verdict, comments }`.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Overall review summary (used when `--file` is omitted or to override `summary`).
        #[arg(long)]
        summary: Option<String>,
        /// Suggested verdict: comment | approve | request_changes.
        #[arg(long)]
        verdict: Option<String>,
        #[arg(long, env = "CANBAN_CARD")]
        card: Option<u64>,
        #[arg(long, env = "CANBAN_PROJECT")]
        project: Option<String>,
        #[arg(long)]
        request_key: Option<String>,
    },
    /// Clear the local unpublished review draft for a Card.
    Clear {
        #[arg(long, env = "CANBAN_CARD")]
        card: Option<u64>,
        #[arg(long, env = "CANBAN_PROJECT")]
        project: Option<String>,
    },
    /// Publish the local review draft for a PR Card to GitHub.
    Publish {
        /// Override the review verdict event: COMMENT | APPROVE | REQUEST_CHANGES.
        #[arg(long)]
        event_override: Option<String>,
        #[arg(long, env = "CANBAN_CARD")]
        card: Option<u64>,
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
    /// Mark the Card as resolved (stops active runs and schedules next-day auto-archive).
    Resolve {
        /// Optional resolution reason or summary message.
        #[arg(long, alias = "reason")]
        message: Option<String>,
        #[arg(long, env = "CANBAN_CARD")]
        card: u64,
        #[arg(long, env = "CANBAN_PROJECT")]
        project: Option<String>,
        #[arg(long)]
        request_key: Option<String>,
    },
    /// Reopen a manually resolved Card (cannot reopen Cards with merged code).
    Reopen {
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
    /// Inspect stored YAML sources and current revision for a Project or Global scope.
    Sources {
        #[arg(long, required_unless_present = "global", conflicts_with = "global")]
        project_id: Option<String>,
        /// Target Global (`__global`) workflows.
        #[arg(long)]
        global: bool,
    },
    Submit {
        #[arg(long, required_unless_present = "global", conflicts_with = "global")]
        project_id: Option<String>,
        /// Target Global (`__global`) workflows.
        #[arg(long)]
        global: bool,
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
        #[arg(long, required_unless_present = "global", conflicts_with = "global")]
        project_id: Option<String>,
        /// Target Global (`__global`) workflows.
        #[arg(long)]
        global: bool,
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
        allow_insecure_bind,
    } = command
    {
        let override_listen = if tailscale || listen.is_some() || allow_insecure_bind {
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
            if allow_insecure_bind {
                cfg.allow_insecure_bind = true;
            }
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
        app.wait_stopped();
        return Ok(());
    }
    if let Cmd::Pair {
        tailscale,
        port,
        listen,
        allow_insecure_bind,
        device_name,
        ttl_secs,
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
        if allow_insecure_bind {
            cfg.allow_insecure_bind = true;
        }
        if !cfg.tcp_enabled() {
            cfg.tailscale = true;
        }
        canban_app_server::network::save_listen_config(&home, &cfg)?;
        let non_unspec = |raw: &str| {
            let norm = canban_types::normalize_daemon_addr(raw);
            norm.parse::<std::net::SocketAddr>()
                .ok()
                .is_none_or(|sa| !sa.ip().is_unspecified())
                .then_some(norm)
        };
        let addr = if let Some(explicit) = cfg.listen.as_deref().and_then(non_unspec) {
            explicit
        } else if let Ok(ts_ip) = canban_app_server::network::detect_tailscale_ipv4() {
            format!("{ts_ip}:{}", cfg.port)
        } else {
            format!("<host-ip>:{}", cfg.port)
        };
        let (token, kind, device_id, expires_at) = if let Some(name) = device_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let store = canban_app_server::auth::DeviceTokenStore::open(&home);
            let (dev, tok) = store.issue_device(name)?;
            (tok, "device_token", Some(dev.id), None)
        } else {
            let codes = canban_app_server::auth::PairingCodeStore::open(&home);
            let (code, exp) = codes.create_code(ttl_secs)?;
            (code, "pairing_code", None, Some(exp))
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
                    "kind": kind,
                    "device_id": device_id,
                    "expires_at": expires_at,
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
                allow_insecure_bind,
            } => {
                let status = canban_app_server::network::install_user_service(
                    &home,
                    &std::env::current_exe()?,
                    tailscale,
                    port,
                    listen,
                    allow_insecure_bind,
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
        let workflow_scope_id = |project_id: &Option<String>, global: bool| -> Result<String> {
            if global {
                Ok(canban_types::GLOBAL_PROJECT_ID.to_owned())
            } else {
                project_id
                    .clone()
                    .context("Specify `--project-id <ID>` or `--global`")
            }
        };
        match command {
            WorkflowCmd::Sources { project_id, global } => {
                let scope_id = workflow_scope_id(project_id, *global)?;
                let collection = canban_app_server::rpc(
                    &endpoint,
                    "workflow.sources",
                    json!({"project_id": scope_id}),
                )?;
                println!("{}", output.encode(&collection)?);
            }
            WorkflowCmd::Submit {
                project_id,
                global,
                revision,
                file,
                name,
                check,
            } => {
                let scope_id = workflow_scope_id(project_id, *global)?;
                let collection = canban_app_server::rpc(
                    &endpoint,
                    "workflow.sources",
                    json!({"project_id": scope_id}),
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
                    json!({"project_id":scope_id,"revision":revision,"sources":sources,"request_key":canban_types::id()}),
                )?;
                println!("{}", output.encode(&value)?);
            }
            WorkflowCmd::Export {
                project_id,
                global,
                name,
                output,
            } => {
                use std::io::Write;
                let scope_id = workflow_scope_id(project_id, *global)?;
                let collection = canban_app_server::rpc(
                    &endpoint,
                    "workflow.sources",
                    json!({"project_id": scope_id}),
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
        Cmd::Review { command } => match command {
            ReviewCmd::Draft {
                file,
                summary,
                verdict,
                card,
                project,
                request_key,
            } => {
                let mut params = card_scope(card, project);
                if let Some(path) = file {
                    let raw = input_body(Some(path))?;
                    let parsed: Value = serde_json::from_str(&raw)
                        .context("Review draft file must contain valid JSON")?;
                    let obj = parsed
                        .as_object()
                        .context("Review draft JSON must be an object")?;
                    for (k, v) in obj {
                        params[k] = v.clone();
                    }
                }
                if let Some(s) = summary {
                    params["summary"] = json!(s);
                }
                if let Some(v) = verdict {
                    params["verdict"] = json!(v);
                }
                if let Some(key) = request_key {
                    params["request_key"] = json!(key);
                }
                if params.get("card_id").is_none()
                    && params.get("card").is_none()
                    && let Ok(cid) = std::env::var("CARD_ID")
                    && !cid.trim().is_empty()
                {
                    params["card_id"] = json!(cid);
                }
                ("review.draft.set".into(), params)
            }
            ReviewCmd::Clear { card, project } => {
                let mut params = card_scope(card, project);
                if params.get("card_id").is_none()
                    && params.get("card").is_none()
                    && let Ok(cid) = std::env::var("CARD_ID")
                    && !cid.trim().is_empty()
                {
                    params["card_id"] = json!(cid);
                }
                ("review.draft.clear".into(), params)
            }
            ReviewCmd::Publish {
                event_override,
                card,
                project,
                request_key,
            } => {
                let mut params = card_scope(card, project);
                if let Some(ev) = event_override {
                    params["event_override"] = json!(ev);
                }
                if let Some(key) = request_key {
                    params["request_key"] = json!(key);
                }
                if params.get("card_id").is_none()
                    && params.get("card").is_none()
                    && let Ok(cid) = std::env::var("CARD_ID")
                    && !cid.trim().is_empty()
                {
                    params["card_id"] = json!(cid);
                }
                if let Ok(run_id) = std::env::var("CANBAN_RUN_ID")
                    && !run_id.trim().is_empty()
                {
                    params["workflow_run_id"] = json!(run_id);
                }
                if let Ok(step_id) = std::env::var("CANBAN_STEP_ID")
                    && !step_id.trim().is_empty()
                {
                    params["step_id"] = json!(step_id);
                }
                ("github.review.publish".into(), params)
            }
        },
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
                if let Ok(terminal) = std::env::var("HC_TERMINAL_ID")
                    && !terminal.trim().is_empty()
                {
                    params["runtime_terminal"] = json!(terminal);
                    if let Ok(session) = std::env::var("HC_SESSION") {
                        params["runtime_session"] = json!(session);
                    }
                    if let Ok(cwd) = std::env::current_dir() {
                        params["cwd"] = json!(cwd);
                    }
                }
                ("card.adopt".into(), params)
            }
            CardCmd::Resolve {
                message,
                card,
                project,
                request_key,
            } => {
                let mut params = card_scope(Some(card), project);
                params["request_key"] = json!(request_key.unwrap_or_else(canban_types::id));
                if let Some(msg) = message {
                    params["message"] = json!(msg);
                }
                if let Ok(terminal) = std::env::var("HC_TERMINAL_ID")
                    && !terminal.trim().is_empty()
                {
                    params["runtime_terminal"] = json!(terminal);
                }
                ("card.resolve".into(), params)
            }
            CardCmd::Reopen {
                card,
                project,
                request_key,
            } => {
                let mut params = card_scope(Some(card), project);
                params["request_key"] = json!(request_key.unwrap_or_else(canban_types::id));
                ("card.reopen".into(), params)
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
            ScheduleCmd::List { project_id, global } => (
                "schedule.list".into(),
                json!({
                    "project_id": project_id,
                    "scope": if global { Some("global") } else { None },
                }),
            ),
            ScheduleCmd::Runs {
                project_id,
                global,
                schedule_id,
                limit,
            } => (
                "schedule.runs".into(),
                json!({
                    "project_id": project_id,
                    "scope": if global { Some("global") } else { None },
                    "schedule_id": schedule_id,
                    "limit": limit,
                }),
            ),
            ScheduleCmd::Create(args) => (
                "schedule.create".into(),
                json!({
                    "project_id": args.project_id,
                    "scope": if args.global || args.project_id.is_none() { "global" } else { "project" },
                    "name": args.name,
                    "cron": args.cron,
                    "timezone": args.timezone,
                    "overlap": args.overlap,
                    "mode": args.mode,
                    "script": args.script,
                    "shell": args.shell,
                    "working_directory": args.working_directory,
                    "timeout_minutes": args.timeout_minutes,
                    "profile_id": args.profile_id,
                    "instruction": args.instruction,
                    "card_column": args.card_column,
                    "card_title": args.card_title,
                    "card_body": args.card_body,
                    "filename": args.filename,
                    "enabled": !args.disabled,
                    "request_key": args.request_key.unwrap_or_else(canban_types::id),
                }),
            ),
            ScheduleCmd::Trigger {
                schedule_id,
                project_id,
                global,
                filename,
                workflow_id,
                rule_id,
                request_key,
            } => (
                "schedule.trigger".into(),
                json!({
                    "schedule_id": schedule_id,
                    "project_id": project_id,
                    "scope": if global { Some("global") } else { None },
                    "filename": filename,
                    "workflow_id": workflow_id,
                    "rule_id": rule_id,
                    "request_key": request_key.unwrap_or_else(canban_types::id),
                }),
            ),
            ScheduleCmd::Toggle {
                schedule_id,
                project_id,
                global,
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
                        "scope": if global { Some("global") } else { None },
                        "filename": filename,
                        "rule_id": rule_id,
                        "enabled": target_enabled,
                        "request_key": request_key.unwrap_or_else(canban_types::id),
                    }),
                )
            }
            ScheduleCmd::Delete {
                schedule_id,
                project_id,
                global,
                filename,
                rule_id,
                request_key,
            } => (
                "schedule.delete".into(),
                json!({
                    "schedule_id": schedule_id,
                    "project_id": project_id,
                    "scope": if global { Some("global") } else { None },
                    "filename": filename,
                    "rule_id": rule_id,
                    "request_key": request_key.unwrap_or_else(canban_types::id),
                }),
            ),
            ScheduleCmd::Artifact {
                schedule_run_id,
                path,
            } => (
                "schedule.read_artifact".into(),
                json!({
                    "schedule_run_id": schedule_run_id,
                    "path": path,
                }),
            ),
        },
        Cmd::Worktree { command } => match command {
            WorktreeCmd::Gc {
                merged_before,
                project_id,
                dry_run,
                env_dirty,
                fail_on_dirty,
            } => {
                let mut params = json!({
                    "merged_before": merged_before,
                    "project_id": project_id,
                    "dry_run": dry_run,
                });
                output.params("worktree.gc", &mut params)?;
                let value = canban_app_server::rpc(&endpoint, "worktree.gc", params)?;
                let dirty = value["dirty"].as_array().cloned().unwrap_or_default();
                if let Some(env_key) = env_dirty
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    && !dirty.is_empty()
                {
                    use std::io::Write;
                    let serialized = serde_json::to_string(&dirty)?;
                    if let Ok(env_file) = std::env::var("CANBAN_ENV")
                        && !env_file.trim().is_empty()
                    {
                        let mut f = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(env_file)?;
                        writeln!(f, "{env_key}={serialized}")?;
                    } else if env_key.contains('/') || env_key.contains('\\') {
                        std::fs::write(env_key, format!("{serialized}\n"))?;
                    }
                }
                println!("{}", output.encode(&value)?);
                if fail_on_dirty && !dirty.is_empty() {
                    anyhow::bail!(
                        "{} dirty or active worktree(s) prevented full cleanup",
                        dirty.len()
                    );
                }
                return Ok(());
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
        Cmd::Device { command } => match command {
            DeviceCmd::List => ("device.list".into(), json!({})),
            DeviceCmd::Invite { ttl_secs } => (
                "device.invite.create".into(),
                json!({ "ttl_secs": ttl_secs }),
            ),
            DeviceCmd::Revoke { device_id } => {
                ("device.revoke".into(), json!({ "device_id": device_id }))
            }
            DeviceCmd::RemoveLegacyToken => ("device.remove_legacy_token".into(), json!({})),
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
            | "device.list"
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
    if (matches!(
        method.as_str(),
        "note.save"
            | "note.write"
            | "note.rename"
            | "card.continue"
            | "card.move"
            | "card.reopen"
            | "execution.retry"
            | "review.merge"
            | "card.archive"
            | "update.install"
            | "update.daemon"
            | "workflow.decide"
            | "workflow.deliver"
            | "workflow.stop"
            | "schedule.create"
            | "schedule.trigger"
            | "schedule.toggle"
            | "schedule.delete"
            | "card.hold_auto_merge"
    ) || (method.starts_with("github.") && method != "github.context"))
        && p.get("execution").is_none()
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

    #[test]
    fn parses_card_resolve_reopen_and_worktree_gc_alias() {
        let res = Args::try_parse_from([
            "canban",
            "card",
            "resolve",
            "--card",
            "132",
            "--message",
            "Fixed in #130",
        ])
        .unwrap();
        match res.command {
            Some(Cmd::Card {
                command: CardCmd::Resolve { card, message, .. },
            }) => {
                assert_eq!(card, 132);
                assert_eq!(message.as_deref(), Some("Fixed in #130"));
            }
            _ => panic!("expected card resolve"),
        }

        let reopen = Args::try_parse_from(["canban", "card", "reopen", "--card", "132"]).unwrap();
        match reopen.command {
            Some(Cmd::Card {
                command: CardCmd::Reopen { card, .. },
            }) => assert_eq!(card, 132),
            _ => panic!("expected card reopen"),
        }

        let gc =
            Args::try_parse_from(["canban", "worktree", "gc", "--resolved-before", "3d"]).unwrap();
        match gc.command {
            Some(Cmd::Worktree {
                command: WorktreeCmd::Gc { merged_before, .. },
            }) => assert_eq!(merged_before, "3d"),
            _ => panic!("expected worktree gc"),
        }
    }

    #[test]
    fn parses_review_publish_command() {
        let parsed = Args::try_parse_from([
            "canban",
            "review",
            "publish",
            "--event-override",
            "COMMENT",
            "--card",
            "150",
            "--request-key",
            "rk-150",
        ])
        .unwrap();
        match parsed.command {
            Some(Cmd::Review {
                command:
                    ReviewCmd::Publish {
                        event_override,
                        card,
                        request_key,
                        ..
                    },
            }) => {
                assert_eq!(event_override.as_deref(), Some("COMMENT"));
                assert_eq!(card, Some(150));
                assert_eq!(request_key.as_deref(), Some("rk-150"));
            }
            _ => panic!("expected review publish"),
        }
    }
}
