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
        let req_id = self.next_id;
        self.next_id += 1;
        let read_timeout = rpc_read_timeout(method);
        let extended_timeout = read_timeout != DEFAULT_RPC_READ_TIMEOUT;
        let mut line = String::new();
        match &mut self.stream {
            ClientStream::Unix { writer, reader } => {
                writeln!(
                    writer,
                    "{}",
                    json!({"jsonrpc":"2.0","id":req_id,"method":method,"params":params})
                )?;
                writer.flush()?;
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
                if n == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "AppServer closed RPC socket",
                    )
                    .into());
                }
            }
            ClientStream::Tcp {
                writer,
                reader,
                token,
            } => {
                writeln!(
                    writer,
                    "{}",
                    json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "method": method,
                        "params": params,
                        "auth": token
                    })
                )?;
                writer.flush()?;
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
                if n == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "Remote AppServer closed TCP socket",
                    )
                    .into());
                }
            }
        }
        let reply: Value = serde_json::from_str(&line)?;
        if let Some(err) = reply.get("error") {
            let code = err["code"].as_i64().unwrap_or(-32000);
            let msg = err["message"].as_str().unwrap_or("Canban RPC error");
            if code == -32001 {
                bail!("auth_failed: {msg}");
            }
            bail!("{msg}");
        }
        reply
            .get("result")
            .cloned()
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

impl AppClient {
    pub fn connect_or_start(home: PathBuf, cli: &Path) -> Result<Arc<Self>> {
        Self::connect_or_start_inner(home, cli, None, true)
    }

    pub fn connect_or_start_ui(home: PathBuf, cli: &Path, ui_pid: u32) -> Result<Arc<Self>> {
        Self::connect_or_start_inner(home, cli, Some(ui_pid), true)
    }

    /// Connect directly to an explicit `home`'s Unix socket (`home/app.sock`) without
    /// consulting `CANBAN_ENDPOINT` in the process environment.
    pub fn connect_home(home: PathBuf, cli: &Path) -> Result<Arc<Self>> {
        Self::connect_or_start_inner(home, cli, None, false)
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
    ) -> Result<Arc<Self>> {
        let explicit = honor_env_endpoint
            .then(|| std::env::var_os("CANBAN_ENDPOINT"))
            .flatten();
        if let Some(raw_os) = &explicit {
            let raw_str = raw_os.to_string_lossy();
            if let Some((addr, token)) = canban_types::parse_pairing_uri(&raw_str) {
                return Self::connect_tcp(addr, token);
            }
            if let Some(addr) = raw_str.strip_prefix("tcp://") {
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
            let updater = cli.with_file_name("canban-updater");
            let replaces_ui = canban_updater::handoff::resume_with(
                &home,
                updater.is_file().then_some(updater.as_path()),
                ui_pid,
            )?;
            let until = std::time::Instant::now() + Duration::from_secs(100);
            while canban_updater::handoff::active(&home) {
                ensure!(
                    canban_updater::handoff::journal(&home)?.phase != "manual_recovery",
                    "Upgrade recovery failed; inspect updates/updater.log"
                );
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
            Self::spawn_daemon(&home, cli)?;
        }
        let mut state = None;
        let mut server_notice = None;
        let mut last_error: Option<anyhow::Error> = None;
        let mut server_id = None;
        for _ in 0..100 {
            match rpc(&sock_path, "app.ping", json!({})) {
                Ok(ping) => {
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
        std::process::Command::new(cli)
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
        match self.pooled_rpc(method, &params) {
            Ok(v) => Ok(v),
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
                self.pooled_rpc(method, &params)
            }
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
        let state = self.call("app.state", json!({}))?;
        *self.config.write().unwrap() = serde_json::from_value(state["config"].clone())?;
        Ok(serde_json::from_value(state["snapshot"].clone())?)
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
            let mut last = Value::Null;
            let mut connected = false;
            while let Some(app) = weak.upgrade() {
                match app.call("app.state", json!({})) {
                    Ok(state) => {
                        if !connected || state != last {
                            if tx
                                .send(AppEvent::Changed {
                                    topic: change_topic(connected, &last, &state),
                                    card_id: None,
                                })
                                .is_err()
                            {
                                break;
                            }
                            last = state;
                        }
                        connected = true;
                    }
                    Err(_) => connected = false,
                }
                drop(app);
                std::thread::sleep(Duration::from_millis(750));
            }
        });
        rx
    }
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
}
