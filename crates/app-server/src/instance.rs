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
        #[cfg(target_os = "macos")]
        {
            self.directory.join("CanbanDev.app")
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.directory.join("CanbanDev")
        }
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
        let out = canban_exec::raw_command("ps")
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
    #[cfg(target_os = "linux")]
    let executable = {
        let exe_path = std::fs::read_link("/proc/self/exe")
            .or_else(|_| std::env::current_exe())
            .context("Could not resolve development process executable")?;
        let raw = exe_path.to_string_lossy();
        raw.strip_suffix(" (deleted)").unwrap_or(&raw).to_owned()
    };
    #[cfg(not(target_os = "linux"))]
    let executable = ps("comm")?;
    let path = instance.directory.join(format!("{role}.json"));
    canban_context::atomic(
        &path,
        &serde_json::to_vec(&serde_json::json!({
            "pid":std::process::id(), "identity":{"start":ps("lstart")?, "command":ps("command")?, "executable":executable}
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
    let resolved = if std::fs::symlink_metadata(executable)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        std::fs::canonicalize(executable).unwrap_or_else(|_| executable.to_path_buf())
    } else {
        executable.to_path_buf()
    };

    let (bundle, file, is_named_dev, is_macos_app) = if resolved
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        == Some("MacOS")
        && resolved
            .parent()
            .and_then(Path::parent)
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            == Some("Contents")
    {
        let Some(b) = resolved
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
        else {
            return Ok(None);
        };
        if b.extension().and_then(|s| s.to_str()) != Some("app") {
            return Ok(None);
        }
        (
            b.to_path_buf(),
            b.join("Contents/Resources/dev-instance.json"),
            b.file_name().and_then(|s| s.to_str()) == Some("CanbanDev.app"),
            true,
        )
    } else if resolved
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        == Some("bin")
    {
        let Some(b) = resolved.parent().and_then(Path::parent) else {
            return Ok(None);
        };
        if b.extension().and_then(|s| s.to_str()) == Some("app") {
            return Ok(None);
        }
        (
            b.to_path_buf(),
            b.join("share/dev-instance.json"),
            b.file_name().and_then(|s| s.to_str()) == Some("CanbanDev"),
            false,
        )
    } else {
        return Ok(None);
    };

    if !file.exists() {
        ensure!(
            !is_named_dev,
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
    let expected_bundle = if is_macos_app {
        instance.directory.join("CanbanDev.app")
    } else {
        instance.directory.join("CanbanDev")
    };
    ensure!(
        expected_bundle == bundle,
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
            directory: directory.clone(),
            home: tmp.path().join("home"),
            app_id: "com.canban.dev.test".into(),
        };
        let mac_bundle = directory.join("CanbanDev.app");
        std::fs::create_dir_all(mac_bundle.join("Contents/Resources")).unwrap();
        std::fs::write(
            mac_bundle.join("Contents/Resources/dev-instance.json"),
            serde_json::to_vec(&instance).unwrap(),
        )
        .unwrap();
        assert_eq!(
            read(&mac_bundle.join("Contents/MacOS/canban-ui"))
                .unwrap()
                .unwrap()
                .home,
            instance.home
        );
        let other = tmp.path().join("copy.app");
        std::fs::create_dir_all(other.join("Contents/Resources")).unwrap();
        std::fs::copy(
            mac_bundle.join("Contents/Resources/dev-instance.json"),
            other.join("Contents/Resources/dev-instance.json"),
        )
        .unwrap();
        assert!(read(&other.join("Contents/MacOS/canban-ui")).is_err());
        std::fs::remove_file(mac_bundle.join("Contents/Resources/dev-instance.json")).unwrap();
        assert!(read(&mac_bundle.join("Contents/MacOS/canban-ui")).is_err());
        assert!(
            read(&tmp.path().join("target/debug/canban"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn linux_directory_development_bundle_binding_and_standalone_cli_coexist() {
        let tmp = tempfile::tempdir().unwrap();
        let directory = tmp.path().join("branch-linux");
        let instance = Development {
            checkout: tmp.path().into(),
            branch: "feature/linux".into(),
            key: "linuxkey".into(),
            directory: directory.clone(),
            home: tmp.path().join("home"),
            app_id: "com.canban.dev.linuxkey".into(),
        };
        let linux_bundle = directory.join("CanbanDev");
        std::fs::create_dir_all(linux_bundle.join("bin")).unwrap();
        std::fs::create_dir_all(linux_bundle.join("share")).unwrap();
        std::fs::write(
            linux_bundle.join("share/dev-instance.json"),
            serde_json::to_vec(&instance).unwrap(),
        )
        .unwrap();
        assert_eq!(
            read(&linux_bundle.join("bin/canban-ui"))
                .unwrap()
                .unwrap()
                .home,
            instance.home
        );
        assert_eq!(
            read(&linux_bundle.join("bin/canban"))
                .unwrap()
                .unwrap()
                .app_id,
            instance.app_id
        );

        // Moved Linux dev bundle is rejected.
        let moved = tmp.path().join("CanbanDev");
        std::fs::create_dir_all(moved.join("bin")).unwrap();
        std::fs::create_dir_all(moved.join("share")).unwrap();
        std::fs::copy(
            linux_bundle.join("share/dev-instance.json"),
            moved.join("share/dev-instance.json"),
        )
        .unwrap();
        assert!(read(&moved.join("bin/canban-ui")).is_err());

        // Missing dev-instance.json inside a `CanbanDev` directory errors explicitly.
        std::fs::remove_file(linux_bundle.join("share/dev-instance.json")).unwrap();
        assert!(read(&linux_bundle.join("bin/canban-ui")).is_err());

        // Release Linux desktop bundle (`~/.local/opt/canban/bin/canban`) and
        // standalone headless CLI (`~/.local/bin/canban`) return Ok(None).
        assert!(
            read(&tmp.path().join(".local/opt/canban/bin/canban"))
                .unwrap()
                .is_none()
        );
        assert!(
            read(&tmp.path().join(".local/bin/canban"))
                .unwrap()
                .is_none()
        );

        // `register` records full executable identity without hardcoded /bin/ps.
        let _reg = register(&instance, "server").unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("server.json")).unwrap()).unwrap();
        assert_eq!(saved["pid"].as_u64(), Some(std::process::id().into()));
        assert!(!saved["identity"]["start"].as_str().unwrap().is_empty());
        assert!(!saved["identity"]["executable"].as_str().unwrap().is_empty());
    }
}
