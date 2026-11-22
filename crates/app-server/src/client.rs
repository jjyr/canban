//! UI-side access to authoritative Canban services over local Unix sockets,
//! authenticated Tailscale/TCP sockets, or SSH-forwarded Unix sockets.
use super::*;
use std::net::{TcpStream, ToSocketAddrs};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientEndpoint {
    Unix(PathBuf),
    Tcp { addr: String, token: String },
}

enum ClientStream {
    Unix {
        writer: UnixStream,
        reader: BufReader<UnixStream>,
    },
    Tcp {
        writer: TcpStream,
        reader: BufReader<TcpStream>,
        token: String,
    },
}

pub(crate) const DEFAULT_RPC_READ_TIMEOUT: Duration = Duration::from_secs(90);
pub(crate) const DAEMON_UPDATE_RPC_READ_TIMEOUT: Duration = Duration::from_secs(20 * 60);

pub(crate) fn rpc_read_timeout(method: &str) -> Duration {
    if method == "update.daemon" {
        DAEMON_UPDATE_RPC_READ_TIMEOUT
    } else {
        DEFAULT_RPC_READ_TIMEOUT
    }
}

struct RpcConn {
    stream: ClientStream,
    next_id: u64,
}

impl RpcConn {
    fn connect(endpoint: &ClientEndpoint) -> Result<Self> {
        match endpoint {
            ClientEndpoint::Unix(path) => {
                let writer = UnixStream::connect(path)
                    .with_context(|| format!("Canban is not running at {}", path.display()))?;
                writer.set_read_timeout(Some(DEFAULT_RPC_READ_TIMEOUT))?;
                writer.set_write_timeout(Some(DEFAULT_RPC_READ_TIMEOUT))?;
                let reader = BufReader::new(writer.try_clone()?);
                Ok(Self {
                    stream: ClientStream::Unix { writer, reader },
                    next_id: 1,
                })
            }
            ClientEndpoint::Tcp { addr, token } => {
                let sock_addr = addr
                    .to_socket_addrs()
                    .with_context(|| format!("Invalid Canban TCP address {addr}"))?
                    .next()
                    .with_context(|| format!("Could not resolve Canban TCP address {addr}"))?;
                let writer = TcpStream::connect_timeout(&sock_addr, Duration::from_secs(10))
                    .with_context(|| format!("Canban is not reachable at {addr}"))?;
                let _ = writer.set_nodelay(true);
                writer.set_read_timeout(Some(DEFAULT_RPC_READ_TIMEOUT))?;
                writer.set_write_timeout(Some(DEFAULT_RPC_READ_TIMEOUT))?;
                let reader = BufReader::new(writer.try_clone()?);
                Ok(Self {
                    stream: ClientStream::Tcp {
                        writer,
                        reader,
                        token: token.clone(),
                    },
                    next_id: 1,
                })
            }
        }
    }

    fn call(&mut self, method: &str, params: &Value) -> Result<Value> {
        let _rpc = canban_profiler::Span::new("rpc.request");
        let req_id = self.next_id;
        self.next_id += 1;
        let read_timeout = rpc_read_timeout(method);
        let extended_timeout = read_timeout != DEFAULT_RPC_READ_TIMEOUT;
        let mut line = String::new();
        match &mut self.stream {
            ClientStream::Unix { writer, reader } => {
                runtime::write_json_line(
                    writer,
                    &json!({"jsonrpc":"2.0","id":req_id,"method":method,"params":params}),
                )?;
                if extended_timeout {
                    writer.set_read_timeout(Some(read_timeout))?;
                    reader.get_ref().set_read_timeout(Some(read_timeout))?;
                }
                let read_res = reader.read_line(&mut line);
                if extended_timeout {
                    let _ = writer.set_read_timeout(Some(DEFAULT_RPC_READ_TIMEOUT));
                    let _ = reader
                        .get_ref()
                        .set_read_timeout(Some(DEFAULT_RPC_READ_TIMEOUT));
                }
                let n = read_res?;
                if n == 0 || !line.ends_with('\n') {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        if n == 0 {
                            "AppServer closed RPC socket"
                        } else {
                            "AppServer closed RPC socket mid-frame"
                        },
                    )
                    .into());
                }
            }
            ClientStream::Tcp {
                writer,
                reader,
                token,
            } => {
                runtime::write_json_line(
                    writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "method": method,
                        "params": params,
                        "auth": token
                    }),
                )?;
                if extended_timeout {
                    writer.set_read_timeout(Some(read_timeout))?;
                    reader.get_ref().set_read_timeout(Some(read_timeout))?;
                }
                let read_res = reader.read_line(&mut line);
                if extended_timeout {
                    let _ = writer.set_read_timeout(Some(DEFAULT_RPC_READ_TIMEOUT));
                    let _ = reader
                        .get_ref()
                        .set_read_timeout(Some(DEFAULT_RPC_READ_TIMEOUT));
                }
                let n = read_res?;
                if n == 0 || !line.ends_with('\n') {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        if n == 0 {
                            "Remote AppServer closed TCP socket"
                        } else {
                            "Remote AppServer closed TCP socket mid-frame"
                        },
                    )
                    .into());
                }
            }
        }
        let mut decode = canban_profiler::Span::new("rpc.decode");
        decode.counts(1, line.len());
        let mut reply: Value = serde_json::from_str(&line)?;
        if let Some(err) = reply.get("error") {
            let code = err["code"].as_i64().unwrap_or(-32000);
            let msg = err["message"].as_str().unwrap_or("Canban RPC error");
            if code == -32001 {
                bail!("auth_failed: {msg}");
            }
            bail!("{msg}");
        }
        reply
            .get_mut("result")
            .map(Value::take)
            .context("Invalid Canban response")
    }
}

pub(crate) fn rpc_endpoint(
    endpoint: &ClientEndpoint,
    method: &str,
    params: Value,
) -> Result<Value> {
    let mut conn = RpcConn::connect(endpoint)?;
    conn.call(method, &params)
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
    endpoint: ClientEndpoint,
    cli: PathBuf,
    explicit_endpoint: bool,
    server_id: Option<String>,
    respawn_lock: Mutex<()>,
    rpc_pool: Mutex<Vec<RpcConn>>,
    config: RwLock<Config>,
    server_notice: Option<String>,
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
        let normalized = canban_types::normalize_daemon_addr(&addr);
        let endpoint = ClientEndpoint::Tcp {
            addr: normalized,
            token,
        };
        Self::connect_remote_endpoint(endpoint)
    }

    /// Connect to a remote `canband` exposed via a local SSH-forwarded Unix socket.
    pub fn connect_ssh_socket(socket_path: PathBuf) -> Result<Arc<Self>> {
        Self::connect_remote_endpoint(ClientEndpoint::Unix(socket_path))
    }

    fn connect_remote_endpoint(endpoint: ClientEndpoint) -> Result<Arc<Self>> {
        let mut conn = RpcConn::connect(&endpoint)?;
        let ping = conn.call("app.ping", &json!({}))?;
        let mut server_notice = None;
        match canban_updater::backend_order(&ping, &canban_updater::Build::current()) {
            std::cmp::Ordering::Less => {
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
        let state = conn.call("app.state", &json!({}))?;
        let server_id = state["server_id"]
            .as_str()
            .or_else(|| ping["server_id"].as_str())
            .map(str::to_owned);
        Ok(Arc::new(Self {
            home: PathBuf::from(strp(&state, "home")?),
            endpoint,
            cli: PathBuf::new(),
            explicit_endpoint: true,
            server_id,
            respawn_lock: Mutex::new(()),
            rpc_pool: Mutex::new(vec![conn]),
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
            endpoint: ClientEndpoint::Unix(sock_path),
            cli: cli.to_path_buf(),
            explicit_endpoint: explicit.is_some(),
            server_id,
            respawn_lock: Mutex::new(()),
            rpc_pool: Mutex::new(Vec::new()),
            config: RwLock::new(serde_json::from_value(state["config"].clone())?),
            server_notice,
        }))
    }

    fn spawn_daemon(home: &Path, cli: &Path) -> Result<()> {
        std::fs::create_dir_all(home)?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.join("server.log"))?;
        use std::os::unix::process::CommandExt;
        // Separate process group, lifetime owned by the service, not its UI.
        canban_exec::raw_command(cli)
            .arg("--home")
            .arg(home)
            .arg("serve")
            .env_remove("CANBAN_ENDPOINT")
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0)
            .spawn()?;
        Ok(())
    }

    fn pooled_rpc(&self, method: &str, params: &Value) -> Result<Value> {
        let pooled = self.rpc_pool.lock().unwrap().pop();
        let (mut conn, reused) = match pooled {
            Some(c) => (c, true),
            None => (RpcConn::connect(&self.endpoint)?, false),
        };
        match conn.call(method, params) {
            Ok(v) => {
                let mut pool = self.rpc_pool.lock().unwrap();
                if pool.len() < 4 {
                    pool.push(conn);
                }
                Ok(v)
            }
            Err(err)
                if reused
                    && !err.to_string().starts_with("auth_failed:")
                    && (err.downcast_ref::<std::io::Error>().is_some()
                        || err.to_string().contains("closed")) =>
            {
                let mut fresh = RpcConn::connect(&self.endpoint)?;
                let v = fresh.call(method, params)?;
                let mut pool = self.rpc_pool.lock().unwrap();
                if pool.len() < 4 {
                    pool.push(fresh);
                }
                Ok(v)
            }
            Err(err) => Err(err),
        }
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let value = match self.pooled_rpc(method, &params) {
            Ok(v) => v,
            Err(first_err) => {
                let ClientEndpoint::Unix(sock_path) = &self.endpoint else {
                    return Err(first_err);
                };
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
                self.rpc_pool.lock().unwrap().clear();
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
                self.pooled_rpc(method, &params)?
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
        &self.endpoint
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

    pub(crate) fn set_cached_config(&self, cfg: Config) {
        *self.config.write().unwrap() = cfg;
    }

    pub fn config(&self) -> Config {
        let mut c = self.config.read().unwrap().clone();
        match &self.endpoint {
            ClientEndpoint::Unix(path) => {
                c.relay_endpoint = Some(path.to_string_lossy().into());
                c.relay_token = None;
            }
            ClientEndpoint::Tcp { addr, token } => {
                c.relay_endpoint = Some(format!("tcp://{addr}"));
                c.relay_token = Some(token.clone());
            }
        }
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
    mut emit: impl FnMut(ClientPushEvent) -> bool,
) {
    let mut last = Value::Null;
    let mut connected = false;
    while !is_closed() {
        let Some(app) = weak.upgrade() else {
            break;
        };
        let endpoint = app.endpoint.clone();
        drop(app);
        match subscribe_session(
            &endpoint,
            weak,
            topics,
            &mut last,
            &mut connected,
            &is_closed,
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

fn open_subscribe_io(
    endpoint: &ClientEndpoint,
) -> Result<(Box<dyn Read + Send>, Box<dyn Write + Send>)> {
    let read_timeout = Some(Duration::from_secs(45));
    let write_timeout = Some(Duration::from_secs(30));
    match endpoint {
        ClientEndpoint::Unix(path) => {
            let writer = UnixStream::connect(path)
                .with_context(|| format!("Canban is not running at {}", path.display()))?;
            writer.set_read_timeout(read_timeout)?;
            writer.set_write_timeout(write_timeout)?;
            let reader = writer.try_clone()?;
            Ok((Box::new(reader), Box::new(writer)))
        }
        ClientEndpoint::Tcp { addr, .. } => {
            let sock_addr = addr
                .to_socket_addrs()
                .with_context(|| format!("Invalid Canban TCP address {addr}"))?
                .next()
                .with_context(|| format!("Could not resolve Canban TCP address {addr}"))?;
            let writer = TcpStream::connect_timeout(&sock_addr, Duration::from_secs(10))
                .with_context(|| format!("Canban is not reachable at {addr}"))?;
            let _ = writer.set_nodelay(true);
            writer.set_read_timeout(read_timeout)?;
            writer.set_write_timeout(write_timeout)?;
            let reader = writer.try_clone()?;
            Ok((Box::new(reader), Box::new(writer)))
        }
    }
}

fn subscribe_session(
    endpoint: &ClientEndpoint,
    weak: &std::sync::Weak<AppClient>,
    topics: &[&str],
    last: &mut Value,
    connected: &mut bool,
    is_closed: &impl Fn() -> bool,
    emit: &mut impl FnMut(ClientPushEvent) -> bool,
) -> Result<bool> {
    let (read_io, mut write_io) = open_subscribe_io(endpoint)?;
    let mut req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "app.subscribe",
        "params": {
            "after": last["revision"],
            "topics": topics,
        }
    });
    if let ClientEndpoint::Tcp { token, .. } = endpoint {
        req["auth"] = json!(token);
    }
    runtime::write_json_line(&mut write_io, &req)?;

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
        Some("worktrees") => "worktrees",
        Some("execution") => "execution",
        Some("host") => "host",
        Some("application") => "application",
        _ => change_topic(true, previous, params),
    }
}

enum AsyncClientStream {
    Unix(tokio::net::UnixStream),
    Tcp(tokio::net::TcpStream),
}

impl tokio::io::AsyncRead for AsyncClientStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Unix(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            Self::Tcp(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for AsyncClientStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Unix(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            Self::Tcp(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Unix(s) => std::pin::Pin::new(s).poll_flush(cx),
            Self::Tcp(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Unix(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            Self::Tcp(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
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
    reader: tokio::io::BufReader<tokio::io::ReadHalf<AsyncClientStream>>,
    _writer: tokio::io::WriteHalf<AsyncClientStream>,
    heartbeat_ms: u64,
    last_marker: Value,
}

impl AsyncSubscribeStream {
    pub(crate) fn read_timeout(&self) -> Duration {
        Duration::from_millis(self.heartbeat_ms.saturating_mul(3).max(500))
    }

    pub(crate) async fn next_frame(&mut self) -> Result<Option<PushFrame>> {
        const MAX_FRAME_BYTES: usize = 48 * 1024 * 1024;
        loop {
            let line_opt = match tokio::time::timeout(
                self.read_timeout(),
                runtime::read_bounded_line(&mut self.reader, MAX_FRAME_BYTES),
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
    endpoint: &ClientEndpoint,
    after: &Value,
    topics: &[&str],
    heartbeat_ms: Option<u64>,
) -> Result<(AsyncSubscribeStream, SubscribeHandshake)> {
    const MAX_FRAME_BYTES: usize = 48 * 1024 * 1024;
    let stream = match endpoint {
        ClientEndpoint::Unix(path) => {
            let s = tokio::time::timeout(
                Duration::from_secs(10),
                tokio::net::UnixStream::connect(path),
            )
            .await
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("Timed out connecting to {}", path.display()),
                )
            })?
            .with_context(|| format!("Canban is not running at {}", path.display()))?;
            AsyncClientStream::Unix(s)
        }
        ClientEndpoint::Tcp { addr, .. } => {
            let s = tokio::time::timeout(
                Duration::from_secs(10),
                tokio::net::TcpStream::connect(addr.as_str()),
            )
            .await
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("Timed out connecting to {addr}"),
                )
            })?
            .with_context(|| format!("Canban is not reachable at {addr}"))?;
            let _ = s.set_nodelay(true);
            AsyncClientStream::Tcp(s)
        }
    };

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
    if let ClientEndpoint::Tcp { token, .. } = endpoint {
        req["auth"] = json!(token);
    }
    tokio::time::timeout(
        Duration::from_secs(10),
        runtime::write_json_line_async(&mut writer, &req),
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
        runtime::read_bounded_line(&mut reader, MAX_FRAME_BYTES),
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

trait RelayIo: Read + Write + Send + 'static {
    fn clone_io(&self) -> Result<Self>
    where
        Self: Sized;
    fn clear_timeouts(&self) -> Result<()>;
    fn close_both(&self);
}

impl RelayIo for UnixStream {
    fn clone_io(&self) -> Result<Self> {
        Ok(self.try_clone()?)
    }
    fn clear_timeouts(&self) -> Result<()> {
        self.set_nonblocking(false)?;
        self.set_read_timeout(None)?;
        self.set_write_timeout(None)?;
        Ok(())
    }
    fn close_both(&self) {
        let _ = self.shutdown(std::net::Shutdown::Both);
    }
}

impl RelayIo for TcpStream {
    fn clone_io(&self) -> Result<Self> {
        Ok(self.try_clone()?)
    }
    fn clear_timeouts(&self) -> Result<()> {
        self.set_nonblocking(false)?;
        self.set_read_timeout(None)?;
        self.set_write_timeout(None)?;
        Ok(())
    }
    fn close_both(&self) {
        let _ = self.shutdown(std::net::Shutdown::Both);
    }
}

impl AppServer {
    pub(super) fn relay(&self, stream: UnixStream, params: &Value) -> Result<()> {
        self.relay_io(stream, params)
    }

    pub(super) fn relay_tcp(&self, stream: TcpStream, params: &Value) -> Result<()> {
        self.relay_io(stream, params)
    }

    fn relay_io<S: RelayIo>(&self, mut stream: S, params: &Value) -> Result<()> {
        let mut target = None;
        let viewer_setup = (|| -> Result<Option<canban_herdr::activity::Viewer>> {
            if let Some(terminal) = params["terminal"].as_str() {
                let core_id = self.verified_core_id()?;
                if let Some(expected) = params["expected_core_id"].as_str() {
                    ensure!(core_id == expected, "Original core identity changed");
                }
                target = Some((core_id.clone(), terminal.to_owned()));
                Ok(Some(canban_herdr::activity::Viewer::new(
                    &core_id, terminal,
                )))
            } else {
                Ok(None)
            }
        })();
        let _viewer = match viewer_setup {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(
                    stream,
                    "{}",
                    json!({"jsonrpc":"2.0","id":0,"error":{"code":-32000,"message":format!("{e:#}")}})
                );
                return Err(e);
            }
        };
        let (mut child, mut input, mut output) = match canban_herdr::transport::child(
            &self.config(),
        ) {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(
                    stream,
                    "{}",
                    json!({"jsonrpc":"2.0","id":0,"error":{"code":-32000,"message":format!("{e:#}")}})
                );
                return Err(e);
            }
        };
        writeln!(stream, "{}", json!({"result":{"ready":true}}))?;
        stream.flush()?;
        stream.clear_timeouts()?;
        let mut source = stream.clone_io()?;
        let reverse = std::thread::spawn(move || {
            if target.is_some() {
                canban_herdr::stream::relay_input(&mut source, &mut input, target)
            } else {
                std::io::copy(&mut source, &mut input)
                    .map(|_| ())
                    .map_err(Into::into)
            }
        });
        let _ = std::io::copy(&mut output, &mut stream);
        stream.close_both();
        let _ = child.kill();
        let _ = child.wait();
        let _ = reverse.join();
        Ok(())
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
    fn update_daemon_extends_read_timeout_and_restores_default_after_reply() {
        assert_eq!(
            rpc_read_timeout("update.daemon"),
            DAEMON_UPDATE_RPC_READ_TIMEOUT
        );
        assert_eq!(rpc_read_timeout("app.ping"), DEFAULT_RPC_READ_TIMEOUT);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (req_seen_tx, req_seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            req_seen_tx.send(line).unwrap();
            release_rx.recv().unwrap();
            writeln!(
                stream,
                "{}",
                json!({"jsonrpc":"2.0","id":1,"result":{"updated":true}})
            )
            .unwrap();
            stream.flush().unwrap();
        });

        let mut conn = RpcConn::connect(&ClientEndpoint::Tcp {
            addr,
            token: "test-token".into(),
        })
        .unwrap();
        let probe_fd = match &conn.stream {
            ClientStream::Tcp { reader, .. } => reader.get_ref().try_clone().unwrap(),
            ClientStream::Unix { .. } => unreachable!(),
        };
        assert_eq!(
            probe_fd.read_timeout().unwrap(),
            Some(DEFAULT_RPC_READ_TIMEOUT)
        );

        let caller = std::thread::spawn(move || {
            let res = conn.call("update.daemon", &json!({})).unwrap();
            let after = match &conn.stream {
                ClientStream::Tcp { reader, .. } => reader.get_ref().read_timeout().unwrap(),
                ClientStream::Unix { .. } => unreachable!(),
            };
            (res, after)
        });

        let raw_req = req_seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(raw_req.contains("\"update.daemon\""));
        assert_eq!(
            probe_fd.read_timeout().unwrap(),
            Some(DAEMON_UPDATE_RPC_READ_TIMEOUT)
        );
        release_tx.send(()).unwrap();

        let (res, restored_timeout) = caller.join().unwrap();
        server.join().unwrap();
        assert_eq!(res["updated"], true);
        assert_eq!(restored_timeout, Some(DEFAULT_RPC_READ_TIMEOUT));
    }

    #[test]
    fn truncated_rpc_frame_returns_unexpected_eof_instead_of_json_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            // Write a partial JSON frame without trailing newline, then close.
            stream
                .write_all(br#"{"jsonrpc":"2.0","id":1,"result":{"partial":"#)
                .unwrap();
            stream.flush().unwrap();
            let _ = stream.shutdown(std::net::Shutdown::Both);
        });

        let mut conn = RpcConn::connect(&ClientEndpoint::Tcp {
            addr,
            token: "test-token".into(),
        })
        .unwrap();
        let err = conn.call("app.state", &json!({})).unwrap_err();
        server.join().unwrap();

        let io_err = err
            .downcast_ref::<std::io::Error>()
            .expect("truncated frame must surface as std::io::Error so pooling/respawn can retry");
        assert_eq!(io_err.kind(), std::io::ErrorKind::UnexpectedEof);
    }
}
