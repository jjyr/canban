//! Desktop preferences, host power assertion and public subscription readings.
use super::*;
use canban_types::BoardFilter;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub const DEFAULT_TERMINAL_FONT_SIZE: u16 = 13;
pub const MIN_TERMINAL_FONT_SIZE: u16 = 9;
pub const MAX_TERMINAL_FONT_SIZE: u16 = 32;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(from = "SavedPreferences")]
struct Preferences {
    theme: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    board_filters: BTreeMap<String, BTreeMap<String, BoardFilter>>,
    #[serde(skip_serializing_if = "enabled")]
    notification_sound: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_font_family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_font_size: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_size: Option<[u16; 2]>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct SavedPreferences {
    theme: Option<String>,
    board_filters: Value,
    last_project: Option<String>,
    // Retire appearance/palette modes while preserving the optional Project ID.
    palette: Option<String>,
    notification_sound: Option<bool>,
    terminal_font_family: Option<String>,
    terminal_font_size: Option<u16>,
    terminal_size: Option<[u16; 2]>,
}
fn enabled(value: &bool) -> bool {
    *value
}
fn normalize_font_family(value: Option<String>) -> Option<String> {
    value
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty() && s.chars().count() <= 80 && !s.chars().any(char::is_control))
}
fn normalize_font_size(value: Option<u16>) -> Option<u16> {
    value
        .filter(|&sz| (MIN_TERMINAL_FONT_SIZE..=MAX_TERMINAL_FONT_SIZE).contains(&sz))
        .filter(|&sz| sz != DEFAULT_TERMINAL_FONT_SIZE)
}
fn normalize_terminal_size(value: Option<[u16; 2]>) -> Option<[u16; 2]> {
    let [cols, rows] = value?;
    canban_herdr::TerminalSize::normalized(cols, rows).map(|sz| [sz.cols, sz.rows])
}
impl From<SavedPreferences> for Preferences {
    fn from(saved: SavedPreferences) -> Self {
        let requested = saved.theme.or(saved.palette).unwrap_or_default();
        Self {
            theme: canban_themes::resolve(&requested).id.into(),
            last_project: saved.last_project,
            board_filters:
                serde_json::from_value::<BTreeMap<String, BTreeMap<String, BoardFilter>>>(
                    saved.board_filters,
                )
                .unwrap_or_default()
                .into_iter()
                .map(|(account, filters)| {
                    (
                        account,
                        filters
                            .into_iter()
                            .filter(|(_, f)| f.valid() && f.active())
                            .collect(),
                    )
                })
                .collect(),
            notification_sound: saved.notification_sound.unwrap_or(true),
            terminal_font_family: normalize_font_family(saved.terminal_font_family),
            terminal_font_size: normalize_font_size(saved.terminal_font_size),
            terminal_size: normalize_terminal_size(saved.terminal_size),
        }
    }
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: canban_themes::DEFAULT.into(),
            last_project: None,
            board_filters: BTreeMap::new(),
            notification_sound: true,
            terminal_font_family: None,
            terminal_font_size: None,
            terminal_size: None,
        }
    }
}

type PowerRequest = (bool, mpsc::SyncSender<std::result::Result<(), String>>);

struct Power {
    sender: Option<mpsc::SyncSender<PowerRequest>>,
    active: Mutex<bool>,
}
impl Power {
    fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel::<PowerRequest>(1);
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
            sender: Some(tx),
            active: Mutex::new(false),
        }
    }
    fn disabled() -> Self {
        Self {
            sender: None,
            active: Mutex::new(false),
        }
    }
    fn set(&self, on: bool) -> Result<()> {
        let mut active = self.active.lock().unwrap();
        if let Some(sender) = &self.sender {
            let (tx, rx) = mpsc::sync_channel(1);
            sender.send((on, tx)).context("Power worker unavailable")?;
            rx.recv_timeout(Duration::from_secs(5))
                .context("Power assertion timed out")?
                .map_err(anyhow::Error::msg)?;
        }
        *active = on;
        Ok(())
    }
}
pub(super) struct Utilities {
    preferences: Mutex<Preferences>,
    power: Power,
    fs_durability: canban_context::FsDurability,
    pub(super) usage: canban_usage::Service,
}
impl Utilities {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(home: &Path) -> Self {
        Self::new_with_options(home, canban_context::FsDurability::Full, false)
    }
    pub fn new_with_options(
        home: &Path,
        fs_durability: canban_context::FsDurability,
        disable_os_watchers: bool,
    ) -> Self {
        let prefs = std::fs::read(home.join("config/desktop.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            preferences: Mutex::new(prefs),
            power: if disable_os_watchers {
                Power::disabled()
            } else {
                Power::new()
            },
            fs_durability,
            usage: Default::default(),
        }
    }
    pub fn clear_project(&self, home: &Path, project_id: &str) -> Result<bool> {
        let mut prefs = self.preferences.lock().unwrap();
        let mut next = prefs.clone();
        if next.last_project.as_deref() == Some(project_id) {
            next.last_project = None;
        }
        next.board_filters.retain(|_, filters| {
            filters.remove(project_id);
            !filters.is_empty()
        });
        let changed = next != *prefs;
        if changed {
            canban_context::atomic_with_durability(
                &home.join("config/desktop.json"),
                &serde_json::to_vec_pretty(&next)?,
                self.fs_durability,
            )?;
            *prefs = next;
        }
        Ok(changed)
    }

    pub(super) fn default_terminal_size(&self) -> Option<canban_herdr::TerminalSize> {
        let [cols, rows] = self.preferences.lock().unwrap().terminal_size?;
        canban_herdr::TerminalSize::normalized(cols, rows)
    }

    pub(super) fn save_terminal_size(
        &self,
        home: &Path,
        size: canban_herdr::TerminalSize,
    ) -> Result<bool> {
        let normalized = [size.cols, size.rows];
        let mut prefs = self.preferences.lock().unwrap();
        if prefs.terminal_size == Some(normalized) {
            return Ok(false);
        }
        let mut next = prefs.clone();
        next.terminal_size = Some(normalized);
        canban_context::atomic_with_durability(
            &home.join("config/desktop.json"),
            &serde_json::to_vec_pretty(&next)?,
            self.fs_durability,
        )?;
        *prefs = next;
        Ok(true)
    }

    pub(super) fn state(&self) -> Value {
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
                    p.get("board_filter").is_some()
                        || p.get("theme").is_some()
                        || p.get("last_project").is_some()
                        || p.get("notification_sound").is_some()
                        || p.get("terminal_font_family").is_some()
                        || p.get("terminal_font_size").is_some(),
                    "board_filter, theme, last_project, notification_sound, terminal_font_family or terminal_font_size required"
                );
                if let Some(sound) = p.get("notification_sound") {
                    ensure!(sound.is_boolean(), "notification_sound must be a boolean");
                }
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
                if let Some(family) = p.get("terminal_font_family") {
                    ensure!(
                        family.is_null() || family.is_string(),
                        "terminal_font_family must be a string or null"
                    );
                    if let Some(s) = family.as_str() {
                        let trimmed = s.trim();
                        ensure!(
                            trimmed.chars().count() <= 80 && !trimmed.chars().any(char::is_control),
                            "Use at most 80 printable characters for terminal font family"
                        );
                    }
                }
                if let Some(size) = p.get("terminal_font_size") {
                    ensure!(
                        size.is_null()
                            || size.as_u64().is_some_and(|n| {
                                (u64::from(MIN_TERMINAL_FONT_SIZE)
                                    ..=u64::from(MAX_TERMINAL_FONT_SIZE))
                                    .contains(&n)
                            }),
                        "terminal_font_size must be an integer between {MIN_TERMINAL_FONT_SIZE} and {MAX_TERMINAL_FONT_SIZE} or null"
                    );
                }
                let board_filter = if let Some(patch) = p.get("board_filter") {
                    let account = patch["account"]
                        .as_str()
                        .context("board_filter.account required")?;
                    let project = patch["project_id"]
                        .as_str()
                        .context("board_filter.project_id required")?;
                    ensure!(
                        !account.is_empty()
                            && account.len() <= 256
                            && !account.chars().any(char::is_control),
                        "Invalid filter account"
                    );
                    ensure!(
                        !project.is_empty()
                            && project.len() <= 512
                            && !project.chars().any(char::is_control),
                        "Invalid filter Project"
                    );
                    let filter: BoardFilter = serde_json::from_value(patch["value"].clone())?;
                    ensure!(filter.valid(), "Invalid Board filter");
                    Some((account.to_lowercase(), project.to_owned(), filter))
                } else {
                    None
                };
                let mut prefs = self.utilities.preferences.lock().unwrap();
                let mut next = prefs.clone();
                if let Some(sound) = p["notification_sound"].as_bool() {
                    next.notification_sound = sound;
                }
                if let Some(theme) = p["theme"].as_str() {
                    next.theme = theme.into();
                }
                if let Some(project) = p.get("last_project") {
                    next.last_project = project.as_str().map(str::to_owned);
                }
                if let Some(family) = p.get("terminal_font_family") {
                    next.terminal_font_family =
                        normalize_font_family(family.as_str().map(str::to_owned));
                }
                if let Some(size) = p.get("terminal_font_size") {
                    next.terminal_font_size = normalize_font_size(size.as_u64().map(|n| n as u16));
                }
                if let Some((account, project, filter)) = board_filter {
                    let filters = next.board_filters.entry(account.clone()).or_default();
                    if filter.active() {
                        filters.insert(project, filter);
                    } else {
                        filters.remove(&project);
                    }
                    if filters.is_empty() {
                        next.board_filters.remove(&account);
                    }
                }
                let changed = next != *prefs;
                if changed {
                    canban_context::atomic_with_durability(
                        &self.home.join("config/desktop.json"),
                        &serde_json::to_vec_pretty(&next)?,
                        self.fs_durability,
                    )?;
                    *prefs = next;
                }
                drop(prefs);
                if changed {
                    self.emit("desktop", None);
                    self.emit_utilities();
                }
                Ok(self.utilities.state())
            }
            "utilities.awake" => {
                self.utilities
                    .power
                    .set(p["enabled"].as_bool().context("enabled required")?)?;
                self.emit("desktop", None);
                self.emit_utilities();
                Ok(self.utilities.state())
            }
            "usage.status" | "usage.refresh" => Ok(self
                .utilities
                .usage
                .request(self.config(), method == "usage.refresh")),
            _ => anyhow::bail!("Unknown utility operation"),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn board_filters_survive_restart_and_reset_only_the_selected_account_project() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
        let filter = json!({"kind":"pr","author_mode":"mine","tags":["bug"],"state":"open","scheduled":true});
        for (account, pid) in [("alice", "p1"), ("alice", "remote:p1"), ("bob", "p1")] {
            app.call(
                "utilities.configure",
                json!({"board_filter":{"account":account,"project_id":pid,"value":filter}}),
            )
            .unwrap();
        }
        let saved = std::fs::read(home.join("config/desktop.json")).unwrap();
        assert!(app.call("utilities.configure",json!({"theme":"nord","board_filter":{"account":"alice","project_id":"p1","value":{"kind":"local","state":"open"}}})).is_err());
        assert_eq!(
            saved,
            std::fs::read(home.join("config/desktop.json")).unwrap()
        );
        drop(app);
        let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
        let prefs = app.call("utilities.state", json!({})).unwrap()["preferences"].clone();
        assert_eq!(prefs["board_filters"]["alice"]["p1"]["author_mode"], "mine");
        assert_eq!(
            prefs["board_filters"]["alice"]["remote:p1"]["tags"],
            json!(["bug"])
        );
        app.call(
            "utilities.configure",
            json!({"board_filter":{"account":"ALICE","project_id":"p1","value":{}}}),
        )
        .unwrap();
        app.call("utilities.configure", json!({"theme":"nord"}))
            .unwrap();
        drop(app);
        let app = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
        let prefs = app.call("utilities.state", json!({})).unwrap()["preferences"].clone();
        assert!(prefs["board_filters"]["alice"]["p1"].is_null());
        assert_eq!(prefs["board_filters"]["alice"]["remote:p1"]["kind"], "pr");
        assert_eq!(prefs["board_filters"]["bob"]["p1"]["kind"], "pr");
        assert_eq!(prefs["theme"], "nord");
    }
    #[test]
    fn notification_sound_is_local_persistent_and_independent_of_theme() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
        app.call("utilities.configure", json!({"notification_sound":false}))
            .unwrap();
        app.call("utilities.configure", json!({"theme":"nord"}))
            .unwrap();
        assert!(
            app.call("utilities.configure", json!({"notification_sound":"yes"}))
                .is_err()
        );
        drop(app);
        let reopened = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
        let state = reopened.call("utilities.state", json!({})).unwrap();
        assert_eq!(state["preferences"]["notification_sound"], false);
        assert_eq!(state["preferences"]["theme"], "nord");
        reopened
            .call("utilities.configure", json!({"notification_sound":true}))
            .unwrap();
        assert!(reopened.call("utilities.state",json!({})).unwrap()["preferences"]["notification_sound"].is_null());
    }
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
        assert!(reopened.snapshot().unwrap().terminals.is_empty());
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
        let home = app.home.clone();
        drop(app);
        let reopened = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
        let s = reopened.call("utilities.state", json!({})).unwrap();
        assert_eq!(s["preferences"], json!({"theme":"dracula"}));
        assert_eq!(s["awake"], false);
        assert!(
            reopened
                .call("utilities.awake", json!({"execution":{},"enabled":true}))
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
    #[test]
    fn terminal_font_preferences_validate_persist_and_reset_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();

        app.call(
            "utilities.configure",
            json!({"terminal_font_family":"  JetBrains Mono  ","terminal_font_size":15}),
        )
        .unwrap();

        assert!(
            app.call("utilities.configure", json!({"terminal_font_size":6}))
                .is_err()
        );
        assert!(
            app.call("utilities.configure", json!({"terminal_font_size":48}))
                .is_err()
        );
        assert!(
            app.call(
                "utilities.configure",
                json!({"terminal_font_family":"bad\nname"})
            )
            .is_err()
        );

        drop(app);
        let reopened = AppServer::open(home, PathBuf::from("/bin/echo")).unwrap();
        assert_eq!(
            reopened.call("utilities.state", json!({})).unwrap()["preferences"],
            json!({
                "theme":"canban",
                "terminal_font_family":"JetBrains Mono",
                "terminal_font_size":15
            })
        );

        reopened
            .call(
                "utilities.configure",
                json!({"terminal_font_family":"","terminal_font_size":DEFAULT_TERMINAL_FONT_SIZE}),
            )
            .unwrap();
        assert_eq!(
            reopened.call("utilities.state", json!({})).unwrap()["preferences"],
            json!({"theme":"canban"})
        );
    }
}
