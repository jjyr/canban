//! UI-side access to the single authoritative service. No database is opened here.
use super::*;
struct RpcConn {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
}

impl RpcConn {
    fn connect(endpoint: &Path) -> Result<Self> {
        let writer = UnixStream::connect(endpoint)
            .with_context(|| format!("Canban is not running at {}", endpoint.display()))?;
        writer.set_read_timeout(Some(Duration::from_secs(90)))?;
        writer.set_write_timeout(Some(Duration::from_secs(90)))?;
        let reader = BufReader::new(writer.try_clone()?);
        Ok(Self {
            writer,
            reader,
            next_id: 1,
        })
    }

    fn call(&mut self, method: &str, params: &Value) -> Result<Value> {
        let req_id = self.next_id;
        self.next_id += 1;
        writeln!(
            self.writer,
            "{}",
            json!({"jsonrpc":"2.0","id":req_id,"method":method,"params":params})
        )?;
        self.writer.flush()?;
        let mut line = String::new();
        let n = self.reader.read_line(&mut line)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "AppServer closed RPC socket",
            )
            .into());
        }
        let reply: Value = serde_json::from_str(&line)?;
        ensure!(
            reply.get("error").is_none(),
            "{}",
            reply["error"]["message"]
        );
        reply
            .get("result")
            .cloned()
            .context("Invalid Canban response")
    }
}

pub struct AppClient {
    pub home: PathBuf,
    endpoint: PathBuf,
    cli: PathBuf,
    explicit_endpoint: bool,
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
        Self::connect_or_start_inner(home, cli, None)
    }

    pub fn connect_or_start_ui(home: PathBuf, cli: &Path, ui_pid: u32) -> Result<Arc<Self>> {
        Self::connect_or_start_inner(home, cli, Some(ui_pid))
    }

    fn connect_or_start_inner(home: PathBuf, cli: &Path, ui_pid: Option<u32>) -> Result<Arc<Self>> {
        let explicit = std::env::var_os("CANBAN_ENDPOINT");
        let endpoint = explicit
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
        if rpc(&endpoint, "app.ping", json!({})).is_err() && explicit.is_none() {
            Self::spawn_daemon(&home, cli)?;
        }
        let mut state = None;
        let mut server_notice = None;
        let mut last_error: Option<anyhow::Error> = None;
        for _ in 0..100 {
            match rpc(&endpoint, "app.ping", json!({})) {
                Ok(ping) => {
                    match canban_updater::backend_order(&ping, &canban_updater::Build::current()) {
                        std::cmp::Ordering::Less => {
                            return Err(BackendOlder {
                                home,
                                ping,
                                explicit_endpoint: explicit.is_some(),
                            }
                            .into());
                        }
                        std::cmp::Ordering::Greater => {
                            server_notice = Some(
                                "The backend is newer than this App. Please use a newer Canban App."
                                    .into(),
                            )
                        }
                        std::cmp::Ordering::Equal => {}
                    }
                    match rpc(&endpoint, "app.state", json!({})) {
                        Ok(v) => {
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
                let detail = last_error
                    .map(|e| format!(": {e:#}"))
                    .unwrap_or_default();
                bail!("AppServer unavailable; inspect server.log or CANBAN_ENDPOINT{detail}");
            }
        };
        Ok(Arc::new(Self {
            home: PathBuf::from(strp(&state, "home")?),
            endpoint,
            cli: cli.to_path_buf(),
            explicit_endpoint: explicit.is_some(),
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
            Err(err) if reused && err.downcast_ref::<std::io::Error>().is_some() => {
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
                if rpc(&self.endpoint, "app.ping", json!({})).is_err()
                    && !canban_updater::handoff::active(&self.home)
                {
                    let _ = Self::spawn_daemon(&self.home, &self.cli);
                    for _ in 0..50 {
                        if rpc(&self.endpoint, "app.ping", json!({})).is_ok() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                self.pooled_rpc(method, &params)
            }
        }
    }
    pub fn server_notice(&self) -> Option<&str> {
        self.server_notice.as_deref()
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        let state = self.call("app.state", json!({}))?;
        *self.config.write().unwrap() = serde_json::from_value(state["config"].clone())?;
        Ok(serde_json::from_value(state["snapshot"].clone())?)
    }
    pub fn config(&self) -> Config {
        let mut c = self.config.read().unwrap().clone();
        c.relay_endpoint = Some(self.endpoint.to_string_lossy().into());
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

fn change_topic(connected: bool, previous: &Value, next: &Value) -> &'static str {
    if !connected {
        "resync"
    } else if previous["github_revision"] != next["github_revision"] {
        "github"
    } else {
        "application"
    }
}

impl AppServer {
    pub(super) fn relay(&self, mut stream: UnixStream, params: &Value) -> Result<()> {
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
        stream.set_read_timeout(None)?;
        stream.set_write_timeout(None)?;
        let mut source = stream.try_clone()?;
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
        let _ = stream.shutdown(std::net::Shutdown::Both);
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
}
