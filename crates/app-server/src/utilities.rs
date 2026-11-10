//! Desktop preferences, host power assertion and public subscription readings.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(from = "SavedPreferences")]
struct Preferences {
    theme: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_project: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct SavedPreferences {
    theme: Option<String>,
    last_project: Option<String>,
    // Retire appearance/palette modes while preserving the optional Project ID.
    palette: Option<String>,
}
impl From<SavedPreferences> for Preferences {
    fn from(saved: SavedPreferences) -> Self {
        let requested = saved.theme.or(saved.palette).unwrap_or_default();
        Self {
            theme: canban_themes::resolve(&requested).id.into(),
            last_project: saved.last_project,
        }
    }
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: canban_themes::DEFAULT.into(),
            last_project: None,
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
                ensure!(
                    p.get("theme").is_some() || p.get("last_project").is_some(),
                    "theme or last_project required"
                );
                if let Some(theme) = p.get("theme") {
                    ensure!(
                        theme
                            .as_str()
                            .is_some_and(|t| canban_themes::find(t).is_some()),
                        "Unknown theme"
                    );
                }
                if let Some(project) = p.get("last_project") {
                    ensure!(
                        project.is_null() || project.is_string(),
                        "last_project must be a Project ID or null"
                    );
                    if let Some(id) = project.as_str() {
                        let _: Project = self.store.lock().unwrap().db().get(id)?;
                    }
                }
                let mut prefs = self.utilities.preferences.lock().unwrap();
                let mut next = prefs.clone();
                if let Some(theme) = p["theme"].as_str() {
                    next.theme = theme.into();
                }
                if let Some(project) = p.get("last_project") {
                    next.last_project = project.as_str().map(str::to_owned);
                }
                let changed = next != *prefs;
                if changed {
                    canban_context::atomic(
                        &self.home.join("config/desktop.json"),
                        &serde_json::to_vec_pretty(&next)?,
                    )?;
                    *prefs = next;
                }
                drop(prefs);
                if changed {
                    self.emit("desktop", None);
                }
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
    fn last_project_and_theme_are_independent_validated_preferences_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
        let project = app
            .call(
                "project.create",
                json!({"name":"Recent","directory":dir.path().join("recent")}),
            )
            .unwrap();
        app.call("utilities.configure", json!({"theme":"nord"}))
            .unwrap();
        app.call("utilities.configure", json!({"last_project":project["id"]}))
            .unwrap();
        app.call("utilities.configure", json!({"theme":"dracula"}))
            .unwrap();
        let path = home.join("config/desktop.json");
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            app.call(
                "utilities.configure",
                json!({"theme":"nord","last_project":"missing"})
            )
            .is_err()
        );
        assert!(
            app.call("utilities.configure", json!({"last_project":42}))
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        drop(app);
        let reopened = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
        assert_eq!(
            reopened.call("utilities.state", json!({})).unwrap()["preferences"],
            json!({"theme":"dracula","last_project":project["id"]})
        );
        assert!(reopened.snapshot().unwrap().requests.is_empty());
        reopened
            .call("utilities.configure", json!({"last_project":null}))
            .unwrap();
        assert_eq!(
            reopened.call("utilities.state", json!({})).unwrap()["preferences"],
            json!({"theme":"dracula"})
        );
    }
    #[test]
    fn preferences_validate_before_atomic_save_and_survive_restart_without_power_state() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        assert_eq!(
            app.call("utilities.state", json!({})).unwrap()["preferences"],
            json!({"theme":"canban"})
        );
        app.call("utilities.configure", json!({"theme":"dracula"}))
            .unwrap();
        let path = app.home.join("config/desktop.json");
        let saved = std::fs::read(&path).unwrap();
        assert!(
            app.call("utilities.configure", json!({"theme":"unknown"}))
                .is_err()
        );
        assert!(
            app.call(
                "utilities.configure",
                json!({"appearance":"light","palette":"canban"})
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        let reopened = AppServer::open(app.home.clone(), PathBuf::from("/bin/echo")).unwrap();
        let s = reopened.call("utilities.state", json!({})).unwrap();
        assert_eq!(s["preferences"], json!({"theme":"dracula"}));
        assert_eq!(s["awake"], false);
        assert!(
            app.call("utilities.awake", json!({"execution":{},"enabled":true}))
                .is_err()
        );
    }
    #[test]
    fn legacy_and_unknown_preferences_load_without_retaining_appearance_modes() {
        for (saved, expected) in [
            (json!({"appearance":"light","palette":"forest"}), "canban"),
            (json!({"appearance":"system","palette":"plum"}), "canban"),
            (json!({"appearance":"light","palette":"canban"}), "canban"),
            (json!({"theme":"nord"}), "nord"),
            (json!({"theme":"removed"}), "canban"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join("config")).unwrap();
            let path = dir.path().join("config/desktop.json");
            let bytes = serde_json::to_vec(&saved).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            let utilities = Utilities::new(dir.path());
            assert_eq!(utilities.state()["preferences"], json!({"theme":expected}));
            // Loading does not rewrite an existing configuration.
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
}
