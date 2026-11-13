//! Home binding for generated development bundles, shared by desktop and CLI.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Development {
    pub checkout: PathBuf,
    pub branch: String,
    pub key: String,
    pub directory: PathBuf,
    pub home: PathBuf,
    pub app_id: String,
}

impl Development {
    pub fn bundle(&self) -> PathBuf {
        self.directory.join("CanbanDev.app")
    }

    pub fn check_home(&self, home: &Path) -> Result<()> {
        ensure!(
            home == self.home,
            "Development app is bound to {}; select another home with mise run dev",
            self.home.display()
        );
        Ok(())
    }
}

/// A PID alone is insufficient: the controller also checks launch time and executable.
pub struct Registration {
    path: PathBuf,
    _lock: Option<std::fs::File>,
}

pub fn register(instance: &Development, role: &str) -> Result<Registration> {
    std::fs::create_dir_all(&instance.directory)?;
    let lock = if role == "ui" {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(instance.directory.join("ui.lock"))?;
        file.try_lock()
            .context("This development instance already has a UI")?;
        Some(file)
    } else {
        None
    };
    let ps = |field: &str| -> Result<String> {
        let out = std::process::Command::new("/bin/ps")
            .args([
                "-ww",
                "-p",
                &std::process::id().to_string(),
                "-o",
                &format!("{field}="),
            ])
            .output()?;
        ensure!(
            out.status.success(),
            "Could not record development process identity"
        );
        Ok(String::from_utf8_lossy(&out.stdout).trim().into())
    };
    let path = instance.directory.join(format!("{role}.json"));
    canban_context::atomic(
        &path,
        &serde_json::to_vec(&serde_json::json!({
            "pid":std::process::id(), "identity":{"start":ps("lstart")?, "command":ps("command")?, "executable":ps("comm")?}
        }))?,
    )?;
    Ok(Registration { path, _lock: lock })
}

impl Drop for Registration {
    fn drop(&mut self) {
        if std::fs::read(&self.path)
            .ok()
            .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok())
            .is_some_and(|v| v["pid"].as_u64() == Some(std::process::id().into()))
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub fn bundled() -> Result<Option<Development>> {
    read(&std::env::current_exe()?)
}

pub fn read_executable(executable: &Path) -> Result<Option<Development>> {
    read(executable)
}

fn read(executable: &Path) -> Result<Option<Development>> {
    let Some(bundle) = executable
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
    else {
        return Ok(None);
    };
    if bundle.extension().and_then(|s| s.to_str()) != Some("app") {
        return Ok(None);
    }
    let file = bundle.join("Contents/Resources/dev-instance.json");
    if !file.exists() {
        ensure!(
            bundle.file_name().and_then(|s| s.to_str()) != Some("CanbanDev.app"),
            "Development app lacks an instance binding; rebuild with mise run dev"
        );
        return Ok(None);
    }
    let instance: Development = serde_json::from_slice(&std::fs::read(file)?)
        .context("Invalid development instance binding")?;
    ensure!(
        instance.home.is_absolute() && instance.directory.is_absolute(),
        "Development paths must be absolute"
    );
    ensure!(
        instance.bundle() == bundle,
        "Development app was moved; start its instance with mise run dev"
    );
    ensure!(
        instance.app_id.starts_with("com.canban.dev.")
            && instance
                .app_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c)),
        "Invalid development application identity"
    );
    Ok(Some(instance))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn development_binding_survives_launch_without_shell_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let directory = tmp.path().join("branch");
        let instance = Development {
            checkout: tmp.path().into(),
            branch: "feature/editor".into(),
            key: "key".into(),
            directory,
            home: tmp.path().join("home"),
            app_id: "com.canban.dev.test".into(),
        };
        let bundle = instance.bundle();
        std::fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
        std::fs::write(
            bundle.join("Contents/Resources/dev-instance.json"),
            serde_json::to_vec(&instance).unwrap(),
        )
        .unwrap();
        assert_eq!(
            read(&bundle.join("Contents/MacOS/canban-ui"))
                .unwrap()
                .unwrap()
                .home,
            instance.home
        );
        let other = tmp.path().join("copy.app");
        std::fs::create_dir_all(other.join("Contents/Resources")).unwrap();
        std::fs::copy(
            bundle.join("Contents/Resources/dev-instance.json"),
            other.join("Contents/Resources/dev-instance.json"),
        )
        .unwrap();
        assert!(read(&other.join("Contents/MacOS/canban-ui")).is_err());
        std::fs::remove_file(bundle.join("Contents/Resources/dev-instance.json")).unwrap();
        assert!(read(&bundle.join("Contents/MacOS/canban-ui")).is_err());
        assert!(
            read(&tmp.path().join("target/debug/canban"))
                .unwrap()
                .is_none()
        );
    }
}
