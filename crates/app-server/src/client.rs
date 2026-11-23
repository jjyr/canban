//! UI-side access to authoritative Canban services over local Unix sockets,
//! authenticated Tailscale/TCP sockets, or SSH-forwarded Unix sockets.
use super::*;
pub use canban_transport::ClientEndpoint;
pub(crate) use canban_transport::rpc_read_timeout;

pub(crate) fn rpc_endpoint(
    endpoint: &ClientEndpoint,
    method: &str,
    params: Value,
) -> Result<Value> {
    let _rpc = canban_profiler::Span::new("rpc.request");
    canban_transport::rpc_once(endpoint, method, &params)
}

pub(crate) fn rpc_tcp(
    addr: &str,
    token: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value> {
    rpc_endpoint(
        &ClientEndpoint::Tcp {
            addr: addr.to_owned(),
            token: token.unwrap_or_default().to_owned(),
        },
        method,
        params,
    )
}

pub struct AppClient {
    pub home: PathBuf,
    transport: Arc<canban_transport::ClientTransport>,
    cli: PathBuf,
    explicit_endpoint: bool,
    server_id: Option<String>,
    respawn_lock: Mutex<()>,
    config: RwLock<Config>,
    server_notice: Option<String>,
}

fn init_mux_relay_opener() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        canban_herdr::transport::register_relay_opener(Arc::new(
            |endpoint, terminal, expected_core_id| {
                let key = endpoint
                    .strip_prefix("mux://")
                    .context("Invalid mux relay endpoint")?;
                let mut app_side = canban_transport::open_mux_bridge(key)?;
                let _ = app_side.set_read_timeout(Some(Duration::from_secs(90)));
                let _ = app_side.set_write_timeout(Some(Duration::from_secs(90)));
                canban_herdr::transport::handshake_relay(
                    &mut app_side,
                    terminal,
                    expected_core_id,
                    None,
                )?;
                let _ = app_side.set_read_timeout(None);
                let _ = app_side.set_write_timeout(None);
                canban_herdr::transport::Transport::from_unix_stream(app_side)
            },
        ));
    });
}

#[derive(Clone, Debug)]
pub enum ClientPushEvent {
    State(AppEvent),
    UpdateChanged(Value),
    UtilitiesChanged(Value),
    UsageChanged(Value),
    HerdrInstall(Value),
}

#[derive(Debug, Clone)]
pub struct BackendOlder {
    pub home: PathBuf,
    pub ping: Value,
    pub explicit_endpoint: bool,
}
impl std::fmt::Display for BackendOlder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The backend is older than this App (backend API {}, App API {API_VERSION}). Update the backend using the installed App.",
            self.ping["api_version"]
        )
    }
}
impl std::error::Error for BackendOlder {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectProgress {
    pub phase: String,
    pub tag: String,
    pub detail: String,
}

impl AppClient {
    pub fn connect_or_start(home: PathBuf, cli: &Path) -> Result<Arc<Self>> {
        Self::connect_or_start_inner(home, cli, None, true, |_| {})
    }

    pub fn connect_or_start_ui(home: PathBuf, cli: &Path, ui_pid: u32) -> Result<Arc<Self>> {
        Self::connect_or_start_inner(home, cli, Some(ui_pid), true, |_| {})
    }

    pub fn connect_or_start_ui_with_progress(
        home: PathBuf,
        cli: &Path,
        ui_pid: u32,
        on_progress: impl FnMut(ConnectProgress),
    ) -> Result<Arc<Self>> {
        Self::connect_or_start_inner(home, cli, Some(ui_pid), true, on_progress)
    }

    /// Connect directly to an explicit `home`'s Unix socket (`home/app.sock`) without
    /// consulting `CANBAN_ENDPOINT` in the process environment.
    pub fn connect_home(home: PathBuf, cli: &Path) -> Result<Arc<Self>> {
        Self::connect_or_start_inner(home, cli, None, false, |_| {})
    }

    /// Connect to a remote authenticated TCP `canband` daemon without spawning a local daemon.
    pub fn connect_tcp(addr: String, token: String) -> Result<Arc<Self>> {
        init_mux_relay_opener();
        let transport = Arc::new(canban_transport::ClientTransport::connect_tcp(
            addr,
            token,
            network::hostname_string(),
        )?);
        Self::connect_remote_transport(transport)
    }

    /// Connect to a remote `canband` exposed via a local SSH-forwarded Unix socket.
    pub fn connect_ssh_socket(socket_path: PathBuf) -> Result<Arc<Self>> {
        let transport = Arc::new(canban_transport::ClientTransport::connect_direct(
            ClientEndpoint::Unix(socket_path),
        )?);
        Self::connect_remote_transport(transport)
    }

    fn connect_remote_transport(
        transport: Arc<canban_transport::ClientTransport>,
    ) -> Result<Arc<Self>> {
        let ping = transport
            .call("app.ping", &json!({}))
            .inspect_err(|_| transport.close())?;
        let mut server_notice = None;
        match canban_updater::backend_order(&ping, &canban_updater::Build::current()) {
            std::cmp::Ordering::Less => {
                transport.close();
                return Err(BackendOlder {
                    home: PathBuf::from(ping["home"].as_str().unwrap_or_default()),
                    ping,
                    explicit_endpoint: true,
                }
                .into());
            }
            std::cmp::Ordering::Greater => {
                server_notice =
                    Some("The remote backend is newer than this App. Please update Canban.".into());
            }
            std::cmp::Ordering::Equal => {}
        }
        let state = transport
            .call("app.state", &json!({}))
            .inspect_err(|_| transport.close())?;
        let server_id = state["server_id"]
            .as_str()
            .or_else(|| ping["server_id"].as_str())
            .map(str::to_owned);
        Ok(Arc::new(Self {
            home: PathBuf::from(strp(&state, "home")?),
            transport,
            cli: PathBuf::new(),
            explicit_endpoint: true,
            server_id,
            respawn_lock: Mutex::new(()),
            config: RwLock::new(serde_json::from_value(state["config"].clone())?),
            server_notice,
        }))
    }

    fn connect_or_start_inner(
        home: PathBuf,
        cli: &Path,
        ui_pid: Option<u32>,
        honor_env_endpoint: bool,
        mut on_progress: impl FnMut(ConnectProgress),
    ) -> Result<Arc<Self>> {
        let mut emit = |phase: &str, tag: &str, detail: String| {
            on_progress(ConnectProgress {
                phase: phase.to_owned(),
                tag: tag.to_owned(),
                detail,
            });
        };
        let explicit = honor_env_endpoint
            .then(|| std::env::var_os("CANBAN_ENDPOINT"))
            .flatten();
        if let Some(raw_os) = &explicit {
            let raw_str = raw_os.to_string_lossy();
            if let Some((addr, token)) = canban_types::parse_pairing_uri(&raw_str) {
                emit("connect", "REMOTE", format!("Connecting to {addr}…"));
                return Self::connect_tcp(addr, token);
            }
            if let Some(addr) = raw_str.strip_prefix("tcp://") {
                emit("connect", "REMOTE", format!("Connecting to tcp://{addr}…"));
                let token = std::env::var("CANBAN_TOKEN").unwrap_or_default();
                return Self::connect_tcp(addr.to_owned(), token);
            }
        }
        let sock_path = explicit
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("app.sock"));
        if explicit.is_none()
            && !canban_updater::handoff::authorized(&home)
            && canban_updater::handoff::active(&home)
        {
            emit(
                "handoff",
                "HANDOFF",
                "Resuming pending backend upgrade handoff…".into(),
            );
            let updater = cli.with_file_name("canban-updater");
            let replaces_ui = canban_updater::handoff::resume_with(
                &home,
                updater.is_file().then_some(updater.as_path()),
                ui_pid,
            )?;
            let until = std::time::Instant::now() + Duration::from_secs(100);
            let mut last_handoff_phase = String::new();
            while canban_updater::handoff::active(&home) {
                let journal = canban_updater::handoff::journal(&home)?;
                ensure!(
                    journal.phase != "manual_recovery",
                    "Upgrade recovery failed; inspect updates/updater.log"
                );
                if journal.phase != last_handoff_phase && !journal.phase.is_empty() {
                    last_handoff_phase = journal.phase.clone();
                    emit(
                        "handoff",
                        "HANDOFF",
                        format!("Upgrade handoff phase: {}", journal.phase),
                    );
                }
                ensure!(
                    std::time::Instant::now() < until,
                    "Upgrade/recovery is still in progress"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
            if replaces_ui && ui_pid.is_some() {
                std::thread::sleep(Duration::from_secs(5));
            }
        }
        if rpc(&sock_path, "app.ping", json!({})).is_err() && explicit.is_none() {
            emit(
                "spawn",
                "DAEMON",
                "app.sock offline → spawning `canban serve`".into(),
            );
            Self::spawn_daemon(&home, cli)?;
        }
        let mut state = None;
        let mut server_notice = None;
        let mut last_error: Option<anyhow::Error> = None;
        let mut server_id = None;
        let mut last_daemon_progress: Option<(String, String)> = None;
        for _ in 0..100 {
            let ping_started = std::time::Instant::now();
            match rpc(&sock_path, "app.ping", json!({})) {
                Ok(ping) => {
                    let ping_ms = ping_started.elapsed().as_millis();
                    let api_ver = ping["api_version"].as_u64().unwrap_or(API_VERSION as u64);
                    emit(
                        "rpc",
                        "RPC",
                        format!("app.ping OK (API v{api_ver}, {ping_ms}ms)"),
                    );
                    server_id = ping["server_id"].as_str().map(str::to_owned);
                    match canban_updater::backend_order(&ping, &canban_updater::Build::current()) {
                        std::cmp::Ordering::Less => {
                            return Err(BackendOlder {
                                home,
                                ping,
                                explicit_endpoint: explicit.is_some(),
                            }
                            .into());
                        }
                        std::cmp::Ordering::Greater => server_notice = Some(
                            "The backend is newer than this App. Please use a newer Canban App."
                                .into(),
                        ),
                        std::cmp::Ordering::Equal => {}
                    }
                    match rpc(&sock_path, "app.state", json!({})) {
                        Ok(v) => {
                            if server_id.is_none() {
                                server_id = v["server_id"].as_str().map(str::to_owned);
                            }
                            state = Some(v);
                            break;
                        }
                        Err(err) => {
                            if err.downcast_ref::<std::io::Error>().is_some() {
                                last_error = Some(err);
                            } else {
                                // `app.ping` succeeded, so an application-level JSON-RPC error
                                // from `app.state` is deterministic—surface the root cause immediately.
                                return Err(err.context("AppServer failed to load state"));
                            }
                        }
                    }
                }
                Err(err) => {
                    last_error = Some(err);
                    if explicit.is_none()
                        && let Some(prog) = crate::startup_progress::read_startup_progress(&home)
                    {
                        let key = (prog.phase.clone(), prog.detail.clone());
                        if last_daemon_progress.as_ref() != Some(&key) {
                            emit(&prog.phase, &prog.tag, prog.detail);
                            last_daemon_progress = Some(key);
                        }
                    }
                }
            }
            if explicit.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let state = match state {
            Some(v) => v,
            None => {
                let detail = last_error.map(|e| format!(": {e:#}")).unwrap_or_default();
                bail!("AppServer unavailable; inspect server.log or CANBAN_ENDPOINT{detail}");
            }
        };
        Ok(Arc::new(Self {
            home: PathBuf::from(strp(&state, "home")?),
            transport: Arc::new(canban_transport::ClientTransport::unix(sock_path)),
            cli: cli.to_path_buf(),
            explicit_endpoint: explicit.is_some(),
            server_id,
            respawn_lock: Mutex::new(()),
            config: RwLock::new(serde_json::from_value(state["config"].clone())?),
            server_notice,
        }))
    }

    fn daemon_command(home: &Path, cli: &Path) -> std::process::Command {
        let mut cmd = canban_exec::raw_command(cli);
        cmd.arg("--home").arg(home).arg("serve");
        canban_exec::strip_herdr_terminal_scope_env(&mut cmd);
        cmd.env_remove("CANBAN_ENDPOINT")
            .env_remove("CANBAN_CARD")
            .env_remove("CARD_ID")
            .env_remove("CANBAN_PROJECT")
            .env_remove("CANBAN_PROFILE_ID")
            .env_remove("CANBAN_SESSION_ID")
            .env_remove("CANBAN_RUN_DIR");
        cmd
    }

    fn spawn_daemon(home: &Path, cli: &Path) -> Result<()> {
        std::fs::create_dir_all(home)?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.join("server.log"))?;
        use std::os::unix::process::CommandExt;
        // Separate process group, lifetime owned by the service, not its UI.
        Self::daemon_command(home, cli)
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0)
            .spawn()?;
        Ok(())
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let _rpc = canban_profiler::Span::new("rpc.request");
        let value = match self.transport.call(method, &params) {
            Ok(v) => v,
            Err(first_err) => {
                let ClientEndpoint::Unix(sock_path) = self.transport.endpoint() else {
                    return Err(first_err);
                };
                if self.transport.is_mux() {
                    return Err(first_err);
                }
                let connect_failed = first_err
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io| {
                        matches!(
                            io.kind(),
                            std::io::ErrorKind::ConnectionRefused
                                | std::io::ErrorKind::NotFound
                                | std::io::ErrorKind::UnexpectedEof
                                | std::io::ErrorKind::BrokenPipe
                                | std::io::ErrorKind::ConnectionReset
                        )
                    });
                if !connect_failed
                    || self.explicit_endpoint
                    || !self.cli.exists()
                    || instance::read_executable(&self.cli)
                        .ok()
                        .flatten()
                        .is_some()
                    || canban_updater::handoff::active(&self.home)
                {
                    return Err(first_err);
                }
                let _guard = self.respawn_lock.lock().unwrap();
                self.transport.clear_pool();
                if rpc(sock_path, "app.ping", json!({})).is_err()
                    && !canban_updater::handoff::active(&self.home)
                {
                    let _ = Self::spawn_daemon(&self.home, &self.cli);
                    for _ in 0..50 {
                        if rpc(sock_path, "app.ping", json!({})).is_ok() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                self.transport.call(method, &params)?
            }
        };
        self.cache_config_reply(method, &value);
        Ok(value)
    }

    fn cache_config_reply(&self, method: &str, value: &Value) {
        match method {
            "config.agents.save" | "agent.setup" => {
                if let Ok(cfg) = serde_json::from_value::<Config>(value.clone()) {
                    self.set_cached_config(cfg);
                }
            }
            "scheduler.configure" => {
                if let Ok(settings) = serde_json::from_value::<SchedulerSettings>(value.clone()) {
                    self.config.write().unwrap().scheduler = settings;
                }
            }
            _ => {}
        }
    }

    pub fn ping_with_rtt(&self) -> Result<(Value, u64)> {
        let started = std::time::Instant::now();
        let ping = self.call("app.ping", json!({}))?;
        let rtt_ms = started.elapsed().as_millis() as u64;
        Ok((ping, rtt_ms))
    }

    pub fn server_id(&self) -> Option<&str> {
        self.server_id.as_deref()
    }

    pub fn endpoint(&self) -> &ClientEndpoint {
        self.transport.endpoint()
    }

    pub(crate) fn transport(&self) -> &Arc<canban_transport::ClientTransport> {
        &self.transport
    }

    pub(crate) fn upgraded_token(&self) -> Option<&str> {
        self.transport.upgraded_token()
    }

    pub fn is_remote(&self) -> bool {
        self.explicit_endpoint
    }

    pub fn server_notice(&self) -> Option<&str> {
        self.server_notice.as_deref()
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let mut state = self.call("app.state", json!({}))?;
        *self.config.write().unwrap() = serde_json::from_value(state["config"].take())?;
        Ok(serde_json::from_value(state["snapshot"].take())?)
    }

    pub fn runtime_state(&self) -> Result<RuntimeState> {
        let mut state = self.call("runtime.state", json!({}))?;
        Ok(serde_json::from_value(state["runtime"].take())?)
    }

    pub(crate) fn set_cached_config(&self, cfg: Config) {
        *self.config.write().unwrap() = cfg;
    }

    pub fn config(&self) -> Config {
        let mut c = self.config.read().unwrap().clone();
        let (relay_endpoint, relay_token) = self.transport.relay_endpoint_and_token();
        c.relay_endpoint = Some(relay_endpoint);
        c.relay_token = relay_token;
        c
    }

    pub fn subscribe(self: &Arc<Self>) -> mpsc::Receiver<AppEvent> {
        let (tx, rx) = mpsc::channel();
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            run_subscribe_worker(
                &weak,
                &["snapshot"],
                || false,
                || {},
                |ev| match ev {
                    ClientPushEvent::State(app_ev) => tx.send(app_ev).is_ok(),
                    _ => true,
                },
            );
        });
        rx
    }

    pub fn subscribe_topics(
        self: &Arc<Self>,
        topics: &'static [&'static str],
    ) -> tokio::sync::mpsc::UnboundedReceiver<ClientPushEvent> {
        self.subscribe_mapped(topics, |ev| ev)
    }

    #[cfg(test)]
    pub(crate) fn subscribe_topics_ready(
        self: &Arc<Self>,
        topics: &'static [&'static str],
    ) -> (
        tokio::sync::mpsc::UnboundedReceiver<ClientPushEvent>,
        mpsc::Receiver<()>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            run_subscribe_worker(
                &weak,
                topics,
                || tx.is_closed(),
                || {
                    let _ = ready_tx.send(());
                },
                |ev| tx.send(ev).is_ok(),
            );
        });
        (rx, ready_rx)
    }

    pub fn subscribe_mapped<T: Send + 'static>(
        self: &Arc<Self>,
        topics: &'static [&'static str],
        mut map: impl FnMut(ClientPushEvent) -> T + Send + 'static,
    ) -> tokio::sync::mpsc::UnboundedReceiver<T> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            run_subscribe_worker(
                &weak,
                topics,
                || tx.is_closed(),
                || {},
                |ev| tx.send(map(ev)).is_ok(),
            );
        });
        rx
    }
}

fn run_subscribe_worker(
    weak: &std::sync::Weak<AppClient>,
    topics: &[&str],
    is_closed: impl Fn() -> bool,
    mut on_ready: impl FnMut(),
    mut emit: impl FnMut(ClientPushEvent) -> bool,
) {
    let mut last = Value::Null;
    let mut connected = false;
    while !is_closed() {
        let Some(app) = weak.upgrade() else {
            break;
        };
        let transport = app.transport.clone();
        drop(app);
        match subscribe_session(
            &transport,
            weak,
            topics,
            &mut last,
            &mut connected,
            &is_closed,
            &mut on_ready,
            &mut emit,
        ) {
            Ok(true) => return,
            Ok(false) | Err(_) => {
                if is_closed() {
                    return;
                }
                connected = false;
                std::thread::sleep(Duration::from_millis(750));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn subscribe_session(
    transport: &canban_transport::ClientTransport,
    weak: &std::sync::Weak<AppClient>,
    topics: &[&str],
    last: &mut Value,
    connected: &mut bool,
    is_closed: &impl Fn() -> bool,
    on_ready: &mut impl FnMut(),
    emit: &mut impl FnMut(ClientPushEvent) -> bool,
) -> Result<bool> {
    let stream =
        transport.open_sync_stream(Some(Duration::from_secs(45)), Some(Duration::from_secs(30)))?;
    let (read_io, mut write_io) = (stream.reader, stream.writer);
    let mut req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "app.subscribe",
        "params": {
            "after": last["revision"],
            "topics": topics,
        }
    });
    if let Some(token) = stream.auth_token {
        req["auth"] = json!(token);
    }
    canban_transport::write_json_line(&mut write_io, &req)?;

    let mut reader = BufReader::new(read_io);
    let mut line = String::new();
    let n = (&mut reader).take(48 * 1024 * 1024).read_line(&mut line)?;
    if n == 0 || !line.ends_with('\n') {
        return Ok(false);
    }
    let reply: Value = serde_json::from_str(&line)?;
    if let Some(err) = reply.get("error") {
        let msg = err["message"].as_str().unwrap_or("app.subscribe failed");
        bail!("{msg}");
    }
    let result = reply
        .get("result")
        .context("Invalid app.subscribe response")?;
    let want_snapshot = topics.is_empty() || topics.contains(&"snapshot");
    if want_snapshot && (!*connected || result["changed"] == true) {
        if !emit(ClientPushEvent::State(AppEvent::Changed {
            topic: change_topic(*connected, last, result),
            card_id: None,
        })) {
            return Ok(true);
        }
        *last = super::state::state_marker(result);
    }
    *connected = true;
    on_ready();

    loop {
        line.clear();
        let n = (&mut reader).take(48 * 1024 * 1024).read_line(&mut line)?;
        if n == 0 || !line.ends_with('\n') {
            return Ok(false);
        }
        if weak.strong_count() == 0 || is_closed() {
            return Ok(true);
        }
        let mut msg: Value = serde_json::from_str(&line)?;
        let push_ev = match msg["method"].as_str() {
            Some("heartbeat") => continue,
            Some("event") => {
                let params = &msg["params"];
                let topic = static_event_topic(params["topic"].as_str(), last, params);
                let card_id = params["card_id"].as_str().map(str::to_owned);
                *last = super::state::state_marker(params);
                ClientPushEvent::State(AppEvent::Changed { topic, card_id })
            }
            Some("update.changed") => ClientPushEvent::UpdateChanged(msg["params"]["state"].take()),
            Some("utilities.changed") => {
                ClientPushEvent::UtilitiesChanged(msg["params"]["state"].take())
            }
            Some("usage.changed") => ClientPushEvent::UsageChanged(msg["params"]["state"].take()),
            Some("herdr.install") => ClientPushEvent::HerdrInstall(msg["params"]["state"].take()),
            _ => continue,
        };
        if !emit(push_ev) {
            return Ok(true);
        }
    }
}

fn static_event_topic(raw: Option<&str>, previous: &Value, params: &Value) -> &'static str {
    match raw {
        Some("resync") => "resync",
        Some("github") => "github",
        Some("review") => "review",
        Some("configuration") => "configuration",
        Some("desktop") => "desktop",
        Some("schedules") => "schedules",
        Some("terminals") => "terminals",
        Some("reclaim") => "reclaim",
        Some("runtime") => "runtime",
        Some("worktrees") => "worktrees",
        Some("execution") => "execution",
        Some("host") => "host",
        Some("application") => "application",
        _ => change_topic(true, previous, params),
    }
}

pub(crate) enum PushFrame {
    Heartbeat,
    StateChanged {
        topic: &'static str,
        card_id: Option<String>,
        marker: Value,
    },
    UpdateChanged(Value),
    UtilitiesChanged(Value),
    UsageChanged(Value),
    HerdrInstall(Value),
}

pub(crate) struct SubscribeHandshake {
    pub changed: bool,
    pub marker: Value,
    pub result: Value,
}

pub(crate) struct AsyncSubscribeStream {
    reader: tokio::io::BufReader<tokio::io::ReadHalf<canban_transport::TransportStream>>,
    _writer: tokio::io::WriteHalf<canban_transport::TransportStream>,
    heartbeat_ms: u64,
    last_marker: Value,
}

impl AsyncSubscribeStream {
    pub(crate) fn read_timeout(&self) -> Duration {
        Duration::from_millis(self.heartbeat_ms.saturating_mul(3).max(500))
    }

    pub(crate) async fn next_frame(&mut self) -> Result<Option<PushFrame>> {
        loop {
            let line_opt = match tokio::time::timeout(
                self.read_timeout(),
                canban_transport::read_bounded_line(
                    &mut self.reader,
                    canban_transport::MAX_FRAME_BYTES,
                ),
            )
            .await
            {
                Ok(Ok(opt)) => opt,
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "app.subscribe heartbeat timed out",
                    )
                    .into());
                }
            };
            let Some(line) = line_opt else {
                return Ok(None);
            };
            let mut msg: Value = serde_json::from_str(&line)?;
            match msg["method"].as_str() {
                Some("heartbeat") => return Ok(Some(PushFrame::Heartbeat)),
                Some("event") => {
                    let params = &msg["params"];
                    let topic =
                        static_event_topic(params["topic"].as_str(), &self.last_marker, params);
                    let card_id = params["card_id"].as_str().map(str::to_owned);
                    let marker = super::state::state_marker(params);
                    self.last_marker = marker.clone();
                    return Ok(Some(PushFrame::StateChanged {
                        topic,
                        card_id,
                        marker,
                    }));
                }
                Some("update.changed") => {
                    return Ok(Some(PushFrame::UpdateChanged(
                        msg["params"]["state"].take(),
                    )));
                }
                Some("utilities.changed") => {
                    return Ok(Some(PushFrame::UtilitiesChanged(
                        msg["params"]["state"].take(),
                    )));
                }
                Some("usage.changed") => {
                    return Ok(Some(PushFrame::UsageChanged(msg["params"]["state"].take())));
                }
                Some("herdr.install") => {
                    return Ok(Some(PushFrame::HerdrInstall(msg["params"]["state"].take())));
                }
                _ => continue,
            }
        }
    }
}

/// Open an async persistent `app.subscribe` stream without holding any client mutexes.
pub(crate) async fn open_async_subscribe(
    transport: &canban_transport::ClientTransport,
    after: &Value,
    topics: &[&str],
    heartbeat_ms: Option<u64>,
) -> Result<(AsyncSubscribeStream, SubscribeHandshake)> {
    let (stream, auth_token) = transport.open_async_stream().await?;
    let (read_half, mut writer) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(read_half);
    let mut params = json!({
        "after": after,
        "topics": topics,
    });
    if let Some(ms) = heartbeat_ms {
        params["heartbeat_ms"] = json!(ms);
    }
    let mut req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "app.subscribe",
        "params": params,
    });
    if let Some(token) = auth_token {
        req["auth"] = json!(token);
    }
    tokio::time::timeout(
        Duration::from_secs(10),
        canban_transport::write_json_line_async(&mut writer, &req),
    )
    .await
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Timed out sending app.subscribe request",
        )
    })??;

    let line_opt = tokio::time::timeout(
        Duration::from_secs(15),
        canban_transport::read_bounded_line(&mut reader, canban_transport::MAX_FRAME_BYTES),
    )
    .await
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Timed out waiting for app.subscribe handshake",
        )
    })??;
    let Some(line) = line_opt else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "Server closed connection during app.subscribe handshake",
        )
        .into());
    };
    let reply: Value = serde_json::from_str(&line)?;
    if let Some(err) = reply.get("error") {
        let code = err["code"].as_i64().unwrap_or(-32000);
        let msg = err["message"].as_str().unwrap_or("app.subscribe failed");
        if code == -32001 {
            bail!("auth_failed: {msg}");
        }
        bail!("{msg}");
    }
    let result = reply
        .get("result")
        .cloned()
        .context("Invalid app.subscribe response")?;
    let marker = super::state::state_marker(&result);
    let changed = result["changed"] == true;
    let hb_ms = result["heartbeat_ms"].as_u64().unwrap_or(15_000);
    Ok((
        AsyncSubscribeStream {
            reader,
            _writer: writer,
            heartbeat_ms: hb_ms,
            last_marker: marker.clone(),
        },
        SubscribeHandshake {
            changed,
            marker,
            result,
        },
    ))
}

pub(crate) fn change_topic(connected: bool, previous: &Value, next: &Value) -> &'static str {
    if !connected {
        "resync"
    } else if previous["github_revision"] != next["github_revision"] {
        "github"
    } else {
        "application"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn github_cache_updates_keep_their_topic_across_the_existing_state_channel() {
        let old = json!({"github_revision":3,"snapshot":{"cards":[]}});
        let new = json!({"github_revision":4,"snapshot":{"cards":[]}});
        assert_eq!(change_topic(true, &old, &new), "github");
        assert_eq!(change_topic(false, &old, &new), "resync");
        assert_eq!(change_topic(true, &old, &old), "application");
    }
    #[test]
    fn legacy_service_order_uses_api_not_cargo_package_version() {
        let app = canban_updater::Build::current();
        assert!(
            canban_updater::backend_order(&json!({"version":env!("CARGO_PKG_VERSION")}), &app)
                .is_lt()
        );
        assert!(canban_updater::backend_order(&json!({"api_version":API_VERSION-1}), &app).is_lt());
        assert!(canban_updater::backend_order(&json!({"api_version":API_VERSION}), &app).is_eq());
        assert!(canban_updater::backend_order(&json!({"api_version":API_VERSION+1}), &app).is_gt());
    }
    #[test]
    fn spawn_daemon_command_strips_herdr_terminal_scope_and_context_env() {
        let cmd = AppClient::daemon_command(Path::new("/tmp/canban-home"), Path::new("canban"));
        let envs: std::collections::BTreeMap<String, Option<String>> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|val| val.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for key in [
            "HC_SCOPE",
            "HC_TERMINAL_ID",
            "HC_TERMINAL_SOCKET",
            "CANBAN_ENDPOINT",
            "CANBAN_CARD",
            "CARD_ID",
            "CANBAN_PROJECT",
            "CANBAN_PROFILE_ID",
            "CANBAN_SESSION_ID",
            "CANBAN_RUN_DIR",
        ] {
            assert_eq!(envs.get(key), Some(&None), "{key} must be removed");
        }
        assert!(!envs.contains_key("HC_SESSION"));
        assert!(!envs.contains_key("HC_SOCKET"));
    }
}
