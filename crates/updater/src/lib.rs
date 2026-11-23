//! Release discovery, verified staging and detached desktop replacement.
pub mod feed;
pub mod handoff;
mod quarantine;
pub mod worker;
pub const DESKTOP_API: u32 = 85;
pub const AUTOMATIC_CHECK_INTERVAL: u64 = 24 * 60 * 60;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
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
pub(crate) fn try_upgrade_lock_shared(home: &Path) -> Result<Option<File>> {
    std::fs::create_dir_all(home.join("updates"))?;
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.join("updates/upgrade.lock"))?;
    match f.try_lock_shared() {
        Ok(()) => Ok(Some(f)),
        Err(_) => Ok(None),
    }
}
fn is_stale_in_progress_phase(phase: &str) -> bool {
    matches!(phase, "checking" | "downloading" | "installing")
}
/// Recover an interrupted `checking`, `downloading`, or `installing` state when the caller
/// already holds `operation.lock` and no active upgrade handoff or worker owns the state.
pub(crate) fn recover_stale_state_with_op_lock(home: &Path) -> Result<bool> {
    if handoff::active(home) {
        return Ok(false);
    }
    let Some(_upgrade_shared) = try_upgrade_lock_shared(home)? else {
        return Ok(false);
    };
    if handoff::active(home) {
        return Ok(false);
    }
    let mut state = load_state(home);
    if !is_stale_in_progress_phase(&state.phase) {
        return Ok(false);
    }
    feed::clear_stale_staging(home);
    if let Some(avail) = &state.available
        && let (Ok(current), Ok(offered)) = (
            feed::Version::parse(&Build::current().version),
            feed::Version::parse(&avail.manifest.version),
        )
        && current >= offered
    {
        state.phase = "current".into();
        state.available = None;
        state.progress = 0;
        state.error = None;
        save_state(home, &state)?;
        return Ok(true);
    }
    let message = match state.phase.as_str() {
        "checking" => {
            "The previous update check was interrupted. Check again manually or wait for the next daily check."
        }
        "downloading" => "Previous update download was interrupted.",
        "installing" => "Previous update preparation was interrupted.",
        _ => return Ok(false),
    };
    state.phase = "error".into();
    state.progress = 0;
    state.error = Some(message.into());
    save_state(home, &state)?;
    Ok(true)
}
/// Recover stale in-progress update state when neither `operation.lock` nor an exclusive
/// `upgrade.lock` / active install journal owns the transition.
pub fn recover_stale_state(home: &Path) -> Result<bool> {
    let state = load_state(home);
    if !is_stale_in_progress_phase(&state.phase) || handoff::active(home) {
        return Ok(false);
    }
    let Ok(_operation) = lock(home, "operation.lock") else {
        return Ok(false);
    };
    recover_stale_state_with_op_lock(home)
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

/// Returns `true` when `bundle` uses macOS `.app` directory layout (`Contents/MacOS/...`),
/// and `false` for Linux desktop directory bundles (`bin/...`, `bundle.json`, `share/...`).
pub fn is_app_bundle(bundle: &Path) -> bool {
    bundle.extension().and_then(|s| s.to_str()) == Some("app")
}

pub fn bundle_bin_dir(bundle: &Path) -> PathBuf {
    if is_app_bundle(bundle) {
        bundle.join("Contents/MacOS")
    } else {
        bundle.join("bin")
    }
}

pub fn bundle_cli(bundle: &Path) -> PathBuf {
    bundle_bin_dir(bundle).join("canban")
}

pub fn bundle_ui(bundle: &Path) -> PathBuf {
    bundle_bin_dir(bundle).join("canban-ui")
}

pub fn bundle_updater(bundle: &Path) -> PathBuf {
    bundle_bin_dir(bundle).join("canban-updater")
}

pub fn bundle_dev_instance(bundle: &Path) -> PathBuf {
    if is_app_bundle(bundle) {
        bundle.join("Contents/Resources/dev-instance.json")
    } else {
        bundle.join("share/dev-instance.json")
    }
}

pub fn next_bundle_name(bundle: &Path, token: &str) -> String {
    if is_app_bundle(bundle) {
        format!(".Canban-next-{token}.app")
    } else {
        format!(".canban-next-{token}")
    }
}

pub fn previous_bundle_name(bundle: &Path, token: &str) -> String {
    if is_app_bundle(bundle) {
        format!(".Canban-previous-{token}.app")
    } else {
        format!(".canban-previous-{token}")
    }
}

pub fn failed_bundle_name(bundle: &Path, token: &str) -> String {
    if is_app_bundle(bundle) {
        format!(".Canban-failed-{token}.app")
    } else {
        format!(".canban-failed-{token}")
    }
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

    fn sample_available(version: &str) -> feed::Available {
        feed::Available {
            manifest: feed::Manifest {
                version: version.into(),
                commit: "0123456789abcdef0123456789abcdef01234567".into(),
                platform: Build::current().platform,
                api: DESKTOP_API,
                schema: canban_storage::migrations::CURRENT,
                rollback_floor: 8,
                asset: format!("Canban-{}.tar.zst", Build::current().platform),
                size: 1024,
                sha256: "a".repeat(64),
            },
            url: "https://example.invalid/update.tar.zst".into(),
            notes: String::new(),
        }
    }

    #[test]
    fn recover_stale_state_recovers_interrupted_phases_and_respects_locks() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        for (phase, expected_msg) in [
            ("downloading", "download was interrupted"),
            ("checking", "check was interrupted"),
            ("installing", "preparation was interrupted"),
        ] {
            let staging_leftover = home.join("updates/staging/leftover");
            std::fs::create_dir_all(&staging_leftover).unwrap();
            std::fs::write(staging_leftover.join("download.tar.zst"), b"partial").unwrap();

            let state = State {
                phase: phase.into(),
                available: Some(sample_available("20990101-1")),
                progress: 42,
                error: None,
                checked_at: 100,
                last_check_started_at: 100,
            };
            save_state(home, &state).unwrap();

            // Busy operation.lock skips recovery.
            let op_lock = lock(home, "operation.lock").unwrap();
            assert!(!recover_stale_state(home).unwrap());
            assert_eq!(load_state(home).phase, phase);
            assert!(staging_leftover.exists());
            drop(op_lock);

            // Exclusive upgrade.lock skips recovery.
            let up_lock = lock(home, "upgrade.lock").unwrap();
            assert!(!recover_stale_state(home).unwrap());
            assert_eq!(load_state(home).phase, phase);
            drop(up_lock);

            // Free locks recover stale state, clean staging, preserve available, and reset progress.
            assert!(recover_stale_state(home).unwrap());
            let recovered = load_state(home);
            assert_eq!(recovered.phase, "error");
            assert_eq!(recovered.progress, 0);
            assert_eq!(
                recovered
                    .available
                    .as_ref()
                    .map(|a| a.manifest.version.as_str()),
                Some("20990101-1")
            );
            assert!(
                recovered
                    .error
                    .as_deref()
                    .unwrap_or_default()
                    .contains(expected_msg),
                "unexpected error for {phase}: {:?}",
                recovered.error
            );
            assert!(!staging_leftover.exists());
            // Idempotent when already recovered.
            assert!(!recover_stale_state(home).unwrap());
        }

        // Active install journal prevents stale recovery.
        save_state(
            home,
            &State {
                phase: "installing".into(),
                available: Some(sample_available("20990101-1")),
                progress: 100,
                ..Default::default()
            },
        )
        .unwrap();
        let journal = worker::Journal {
            local: None,
            token: "tok-active".into(),
            phase: "stopping".into(),
            home: home.into(),
            target: home.join("Canban.app"),
            staged: home.join(".Canban-next-tok-active.app"),
            previous: home.join(".Canban-previous-tok-active.app"),
            build: Build::current(),
            schema_before: canban_storage::migrations::CURRENT,
            before: None,
            ui: None,
            app_id: "com.canban.test".into(),
            error: None,
            server: None,
            candidate_ui: None,
            recovery_ui: None,
            systemd_managed: false,
        };
        journal.save().unwrap();
        assert!(!recover_stale_state(home).unwrap());
        assert_eq!(load_state(home).phase, "installing");
    }
}
