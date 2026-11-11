//! Release discovery, verified staging and detached desktop replacement.
pub mod feed;
pub mod handoff;
mod quarantine;
pub mod worker;
pub const DESKTOP_API: u32 = 10;
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
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub phase: String,
    pub available: Option<feed::Available>,
    pub progress: u8,
    pub error: Option<String>,
    pub checked_at: u64,
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
