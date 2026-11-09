//! Desktop preferences, host power assertion and public subscription readings.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    appearance: String,
    palette: String,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            appearance: "system".into(),
            palette: "canban".into(),
        }
    }
}

struct Power {
    sender: mpsc::SyncSender<(bool, mpsc::SyncSender<std::result::Result<(), String>>)>,
    active: Mutex<bool>,
}
impl Power {
    fn new() -> Self {
        let (tx, rx) =
            mpsc::sync_channel::<(bool, mpsc::SyncSender<std::result::Result<(), String>>)>(1);
        std::thread::spawn(move || {
            let mut hold = None;
            while let Ok((on, reply)) = rx.recv() {
                let result = if on && hold.is_none() {
                    keepawake::Builder::default()
                        .idle(true)
                        .display(false)
                        .reason("Canban background execution")
                        .app_name("Canban")
                        .app_reverse_domain("com.canban.desktop")
                        .create()
                        .map(|h| hold = Some(h))
                        .map_err(|_| "Could not keep AppServer host awake".to_owned())
                } else {
                    if !on {
                        hold = None;
                    }
                    Ok(())
                };
                let _ = reply.send(result);
            }
        });
        Self {
            sender: tx,
            active: Mutex::new(false),
        }
    }
    fn set(&self, on: bool) -> Result<()> {
        let mut active = self.active.lock().unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender
            .send((on, tx))
            .context("Power worker unavailable")?;
        rx.recv_timeout(Duration::from_secs(5))
            .context("Power assertion timed out")?
            .map_err(anyhow::Error::msg)?;
        *active = on;
        Ok(())
    }
}
pub(super) struct Utilities {
    preferences: Mutex<Preferences>,
    power: Power,
    usage: canban_usage::Service,
}
impl Utilities {
    pub fn new(home: &Path) -> Self {
        let prefs = std::fs::read(home.join("config/desktop.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            preferences: Mutex::new(prefs),
            power: Power::new(),
            usage: Default::default(),
        }
    }
    fn state(&self) -> Value {
        json!({"preferences":self.preferences.lock().unwrap().clone(),"awake":*self.power.active.lock().unwrap(),"awake_machine":"local","awake_scope":"system","version":env!("CARGO_PKG_VERSION"),"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH})
    }
}
impl AppServer {
    pub(super) fn utility_call(&self, method: &str, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Desktop utilities require a user action"
        );
        match method {
            "utilities.state" => Ok(self.utilities.state()),
            "utilities.configure" => {
                let appearance = p["appearance"].as_str().context("appearance required")?;
                let palette = p["palette"].as_str().context("palette required")?;
                ensure!(
                    ["system", "light", "dark"].contains(&appearance),
                    "Unknown appearance"
                );
                ensure!(
                    ["canban", "forest", "plum"].contains(&palette),
                    "Unknown palette"
                );
                let next = Preferences {
                    appearance: appearance.into(),
                    palette: palette.into(),
                };
                let mut prefs = self.utilities.preferences.lock().unwrap();
                canban_context::atomic(
                    &self.home.join("config/desktop.json"),
                    &serde_json::to_vec_pretty(&next)?,
                )?;
                *prefs = next;
                drop(prefs);
                self.emit("desktop", None);
                Ok(self.utilities.state())
            }
            "utilities.awake" => {
                self.utilities
                    .power
                    .set(p["enabled"].as_bool().context("enabled required")?)?;
                self.emit("desktop", None);
                Ok(self.utilities.state())
            }
            "usage.status" | "usage.refresh" => {
                let machine = p["machine"].as_str().unwrap_or("local");
                ensure!(
                    machine.len() <= 128
                        && machine
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
                    "Invalid usage machine"
                );
                Ok(self.utilities.usage.request(
                    self.config(),
                    machine.into(),
                    method == "usage.refresh",
                ))
            }
            _ => anyhow::bail!("Unknown utility operation"),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_validate_before_atomic_save_and_survive_restart_without_power_state() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        assert!(
            app.call(
                "utilities.configure",
                json!({"appearance":"dark","palette":"unknown"})
            )
            .is_err()
        );
        app.call(
            "utilities.configure",
            json!({"appearance":"dark","palette":"forest"}),
        )
        .unwrap();
        let reopened = AppServer::open(app.home.clone(), PathBuf::from("/bin/echo")).unwrap();
        let s = reopened.call("utilities.state", json!({})).unwrap();
        assert_eq!(s["preferences"]["appearance"], "dark");
        assert_eq!(s["awake"], false);
        assert!(
            app.call("utilities.awake", json!({"execution":{},"enabled":true}))
                .is_err()
        );
    }
}
