//! UI-side access to the single authoritative service. No database is opened here.
use super::*;
pub struct AppClient {
    pub home: PathBuf,
    endpoint: PathBuf,
    cli: PathBuf,
    explicit_endpoint: bool,
    respawn_lock: Mutex<()>,
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
        let explicit = std::env::var_os("CANBAN_ENDPOINT");
        let endpoint = explicit
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("app.sock"));
        if explicit.is_none()
            && !canban_updater::handoff::authorized(&home)
            && canban_updater::handoff::active(&home)
        {
            canban_updater::handoff::resume(&home)?;
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
        }
        if rpc(&endpoint, "app.ping", json!({})).is_err() && explicit.is_none() {
            Self::spawn_daemon(&home, cli)?;
        }
        let mut state = None;
        let mut server_notice = None;
        for _ in 0..100 {
            if let Ok(ping) = rpc(&endpoint, "app.ping", json!({})) {
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
                if let Ok(v) = rpc(&endpoint, "app.state", json!({})) {
                    state = Some(v);
                    break;
                }
            }
            if explicit.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let state =
            state.context("AppServer unavailable; inspect server.log or CANBAN_ENDPOINT")?;
        Ok(Arc::new(Self {
            home: PathBuf::from(strp(&state, "home")?),
            endpoint,
            cli: cli.to_path_buf(),
            explicit_endpoint: explicit.is_some(),
            respawn_lock: Mutex::new(()),
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
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        match rpc(&self.endpoint, method, params.clone()) {
            Ok(v) => Ok(v),
            Err(first_err) => {
                let connect_failed = first_err
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io| {
                        matches!(
                            io.kind(),
                            std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
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
                rpc(&self.endpoint, method, params)
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
        let _viewer = if let Some(terminal) = params["terminal"].as_str() {
            let core = canban_herdr::Client {
                config: self.config(),
            }
            .status()?;
            target = Some((strp(&core["core"], "id")?.to_owned(), terminal.to_owned()));
            Some(canban_herdr::activity::Viewer::new(
                strp(&core["core"], "id")?,
                terminal,
            ))
        } else {
            None
        };
        let (mut child, mut input, mut output) = canban_herdr::transport::child(&self.config())?;
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
