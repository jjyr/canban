//! UI-side access to the single authoritative service. No database is opened here.
use super::*;
pub struct AppClient {
    pub home: PathBuf,
    endpoint: PathBuf,
    config: RwLock<Config>,
}
impl AppClient {
    pub fn connect_or_start(home: PathBuf, cli: &Path) -> Result<Arc<Self>> {
        let explicit = std::env::var_os("CANBAN_ENDPOINT");
        let endpoint = explicit
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("app.sock"));
        if rpc(&endpoint, "app.ping", json!({})).is_err() && explicit.is_none() {
            std::fs::create_dir_all(&home)?;
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(home.join("server.log"))?;
            use std::os::unix::process::CommandExt;
            // Separate process group, lifetime owned by the service, not its UI.
            std::process::Command::new(cli)
                .arg("--home")
                .arg(&home)
                .arg("serve")
                .env_remove("CANBAN_ENDPOINT")
                .stdin(std::process::Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log)
                .process_group(0)
                .spawn()?;
        }
        let mut state = None;
        for _ in 0..100 {
            if let Ok(ping) = rpc(&endpoint, "app.ping", json!({})) {
                check_api(&ping)?;
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
            config: RwLock::new(serde_json::from_value(state["config"].clone())?),
        }))
    }
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        rpc(&self.endpoint, method, params)
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

fn check_api(ping: &Value) -> Result<()> {
    ensure!(
        ping["api_version"]
            .as_u64()
            .is_some_and(|v| v >= u64::from(API_VERSION)),
        "The running AppServer is older than this UI. Use `mise run run` to restart the local service with the current development build. For CANBAN_ENDPOINT, update the service on its owning host."
    );
    Ok(())
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
    fn old_service_is_rejected_even_when_package_versions_match() {
        assert!(check_api(&json!({"version":env!("CARGO_PKG_VERSION")})).is_err());
        assert!(check_api(&json!({"api_version":0})).is_err());
        assert!(check_api(&json!({"api_version":API_VERSION-1})).is_err());
        check_api(&json!({"api_version":API_VERSION})).unwrap();
    }
}
