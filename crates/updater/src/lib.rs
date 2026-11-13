//! Release discovery, verified staging and detached desktop replacement.
pub mod feed;
pub mod handoff;
mod quarantine;
pub mod worker;
pub const DESKTOP_API: u32 = 19;
pub const AUTOMATIC_CHECK_INTERVAL: u64 = 24 * 60 * 60;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Build {
    pub version: String,
    pub commit: String,
    pub schema: u32,
    pub api: u32,
    pub platform: String,
}
impl Build {
    pub fn current() -> Self {
        Self {
            version: option_env!("CANBAN_RELEASE_VERSION")
                .unwrap_or("development")
                .into(),
            commit: option_env!("CANBAN_RELEASE_COMMIT")
                .unwrap_or("unknown")
                .into(),
            schema: canban_storage::migrations::CURRENT,
            api: DESKTOP_API,
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        }
    }
}

/// Published identities are ordered numerically; legacy/local builds use the API.
pub fn backend_order(ping: &serde_json::Value, app: &Build) -> std::cmp::Ordering {
    if let (Ok(server), Ok(installed)) = (
        feed::Version::parse(ping["build"]["version"].as_str().unwrap_or_default()),
        feed::Version::parse(&app.version),
    ) {
        return server.cmp(&installed);
    }
    ping["api_version"]
        .as_u64()
        .unwrap_or(0)
        .cmp(&u64::from(app.api))
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub phase: String,
    pub available: Option<feed::Available>,
    pub progress: u8,
    pub error: Option<String>,
    pub checked_at: u64,
    #[serde(default)]
    pub last_check_started_at: u64,
}
impl State {
    pub fn automatic_delay(&self, now: u64) -> u64 {
        let last = if self.last_check_started_at == 0 {
            self.checked_at
        } else {
            self.last_check_started_at
        };
        if last == 0 {
            0
        } else {
            last.saturating_add(AUTOMATIC_CHECK_INTERVAL)
                .saturating_sub(now)
                .min(AUTOMATIC_CHECK_INTERVAL)
        }
    }
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("Missing parent directory")?;
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".write-{}", canban_types::id()));
    let mut f = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&temp, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
pub fn load_state(home: &Path) -> State {
    std::fs::read(home.join("updates/state.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
pub fn save_state(home: &Path, state: &State) -> Result<()> {
    atomic(
        &home.join("updates/state.json"),
        &serde_json::to_vec_pretty(state)?,
    )
}
pub fn lock(home: &Path, name: &str) -> Result<File> {
    std::fs::create_dir_all(home.join("updates"))?;
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.join("updates").join(name))?;
    f.try_lock().with_context(|| format!("{name} is busy"))?;
    Ok(f)
}
pub fn service_lock(home: &Path) -> Result<File> {
    std::fs::create_dir_all(home)?;
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.join("server.lock"))?;
    f.try_lock()
        .context("Another Canban AppServer owns this home")?;
    Ok(f)
}

#[cfg(test)]
mod version_and_check_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn backend_versions_use_numeric_releases_and_legacy_api_fallback() {
        let mut app = Build::current();
        app.version = "20261111-10".into();
        assert!(
            backend_order(
                &json!({"api_version":8,"build":{"version":"20261111-9"}}),
                &app
            )
            .is_lt()
        );
        assert!(
            backend_order(
                &json!({"api_version":8,"build":{"version":"20261111-10"}}),
                &app
            )
            .is_eq()
        );
        assert!(
            backend_order(
                &json!({"api_version":8,"build":{"version":"20261111-11"}}),
                &app
            )
            .is_gt()
        );
        assert!(backend_order(&json!({"version":"0.1.0","api_version":4}), &app).is_lt());
        assert!(backend_order(&json!({"api_version":app.api+1}), &app).is_gt());
    }
    #[test]
    fn daily_check_timestamp_survives_restarts_and_counts_failed_attempts() {
        let now = 1_800_000_000;
        let mut saved = State::default();
        assert_eq!(saved.automatic_delay(now), 0);
        saved.checked_at = now;
        assert_eq!(
            saved.automatic_delay(now + 60),
            AUTOMATIC_CHECK_INTERVAL - 60
        );
        saved.last_check_started_at = now + 120;
        saved.phase = "error".into();
        let restarted: State =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(
            restarted.automatic_delay(now + 180),
            AUTOMATIC_CHECK_INTERVAL - 60
        );
        assert_eq!(
            restarted.automatic_delay(now + 120 + AUTOMATIC_CHECK_INTERVAL),
            0
        );
        assert_eq!(
            restarted.automatic_delay(now - 100),
            AUTOMATIC_CHECK_INTERVAL
        );
    }
}
