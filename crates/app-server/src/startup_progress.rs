//! Atomic startup progress sidecar written by `AppServer::open` before `app.sock`
//! is bound, allowing the desktop UI to display real-time POST self-test stages
//! (service lock, SQLite WAL open, schema migrations, workflow validation, snapshot).
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub const PROGRESS_FILE: &str = "startup-progress.json";
const MAX_STALE_MS: u64 = 120_000;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StartupProgress {
    pub pid: u32,
    pub updated_at_ms: u64,
    pub phase: String,
    pub tag: String,
    pub detail: String,
}

pub(crate) struct StartupProgressWriter {
    path: PathBuf,
    pid: u32,
    durability: canban_context::FsDurability,
    clock: Arc<dyn canban_types::Clock>,
}

impl StartupProgressWriter {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(home: &Path) -> Self {
        Self::new_with_durability_and_clock(
            home,
            canban_context::FsDurability::Full,
            canban_types::system_clock(),
        )
    }

    pub(crate) fn new_with_durability_and_clock(
        home: &Path,
        durability: canban_context::FsDurability,
        clock: Arc<dyn canban_types::Clock>,
    ) -> Self {
        Self {
            path: home.join(PROGRESS_FILE),
            pid: std::process::id(),
            durability,
            clock,
        }
    }

    pub(crate) fn set(&self, phase: &str, tag: &str, detail: impl Into<String>) {
        let payload = StartupProgress {
            pid: self.pid,
            updated_at_ms: self.clock.now_ms(),
            phase: phase.to_owned(),
            tag: tag.to_owned(),
            detail: detail.into(),
        };
        if let Ok(bytes) = serde_json::to_vec(&payload) {
            let _ = canban_context::atomic_with_durability(&self.path, &bytes, self.durability);
        }
    }
}

impl Drop for StartupProgressWriter {
    fn drop(&mut self) {
        let Ok(bytes) = std::fs::read(&self.path) else {
            return;
        };
        let Ok(saved) = serde_json::from_slice::<StartupProgress>(&bytes) else {
            let _ = std::fs::remove_file(&self.path);
            return;
        };
        if saved.pid == self.pid {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Best-effort read of the active daemon startup progress sidecar.
/// Returns `None` if no daemon is currently initializing or if the file is stale.
pub fn read_startup_progress(home: &Path) -> Option<StartupProgress> {
    read_startup_progress_at(home, canban_types::system_clock().now_ms())
}

pub fn read_startup_progress_at(home: &Path, now_ms: u64) -> Option<StartupProgress> {
    let bytes = std::fs::read(home.join(PROGRESS_FILE)).ok()?;
    let progress = serde_json::from_slice::<StartupProgress>(&bytes).ok()?;
    let age = now_ms.saturating_sub(progress.updated_at_ms);
    (age <= MAX_STALE_MS).then_some(progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_publishes_and_cleans_up_progress_file() {
        let dir = tempfile::tempdir().unwrap();
        {
            let writer = StartupProgressWriter::new(dir.path());
            writer.set("migrate", "MIGRATE", "Migrating SQLite schema (v18 → v20)…");
            let read = read_startup_progress(dir.path()).expect("progress readable");
            assert_eq!(read.phase, "migrate");
            assert_eq!(read.tag, "MIGRATE");
            assert_eq!(read.detail, "Migrating SQLite schema (v18 → v20)…");
            assert_eq!(read.pid, std::process::id());
        }
        assert!(read_startup_progress(dir.path()).is_none());
    }
}
