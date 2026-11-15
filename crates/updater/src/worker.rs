//! Detached installer. The journal survives every bundle/process transition.
use super::*;
use anyhow::{bail, ensure};
use serde_json::json;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
pub mod local;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub identity: String,
}
pub fn process(pid: u32) -> Result<Process> {
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "lstart=,command="])
        .output()?;
    ensure!(output.status.success(), "Process {pid} is unavailable");
    let identity = String::from_utf8(output.stdout)?.trim().to_owned();
    ensure!(!identity.is_empty(), "Process is gone");
    Ok(Process { pid, identity })
}
fn stop_process(p: &Process) -> Result<()> {
    if process(p.pid).is_ok_and(|v| v.identity == p.identity) {
        ensure!(
            Command::new("/bin/kill")
                .args(["-TERM", &p.pid.to_string()])
                .status()?
                .success(),
            "Could not close old UI"
        );
        let until = Instant::now() + Duration::from_secs(15);
        while process(p.pid).is_ok_and(|v| v.identity == p.identity) {
            ensure!(Instant::now() < until, "Old UI did not close");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default)]
    pub local: Option<local::Handoff>,
    pub token: String,
    pub phase: String,
    pub home: PathBuf,
    pub target: PathBuf,
    pub staged: PathBuf,
    pub previous: PathBuf,
    pub build: Build,
    pub schema_before: u32,
    #[serde(default)]
    pub before: Option<Build>,
    pub ui: Option<Process>,
    pub app_id: String,
    pub error: Option<String>,
    pub server: Option<Process>,
    pub candidate_ui: Option<Process>,
    #[serde(default)]
    pub recovery_ui: Option<Process>,
}
impl Journal {
    pub(crate) fn save(&self) -> Result<()> {
        atomic(
            &self.home.join("updates/install.json"),
            &serde_json::to_vec_pretty(self)?,
        )
    }
}
fn bundle(cli: &Path) -> Result<PathBuf> {
    let path = cli
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .context("CLI is not in an App bundle")?;
    ensure!(
        path.extension().and_then(|s| s.to_str()) == Some("app"),
        "Update requires a packaged App"
    );
    Ok(path.to_path_buf())
}
/// Called on a detached background thread after the install intent is accepted.
pub fn prepare_locked(
    home: &Path,
    cli: &Path,
    ui: Option<Process>,
    app_id: String,
    _operation: File,
    client: Option<canban_github::Client>,
) -> Result<()> {
    ensure!(!handoff::active(home), "An upgrade is already pending");
    let release = load_state(home)
        .available
        .context("Check for an update first")?;
    let target = bundle(cli)?;
    let staged = feed::stage(home, &release, client.as_ref())?;
    let token = canban_types::token_id();
    let parent = target.parent().context("App parent missing")?;
    // Copy into the target's filesystem before shutdown, so bundle switches are renames.
    let candidate = parent.join(format!(".Canban-next-{token}.app"));
    copy_bundle(&staged, &candidate)?;
    crate::quarantine::prepare(&candidate)?;
    let helper = home
        .join("updates/jobs")
        .join(&token)
        .join("canban-updater");
    std::fs::create_dir_all(helper.parent().unwrap())?;
    std::fs::copy(cli.with_file_name("canban-updater"), &helper)?;
    let before: Build = serde_json::from_value(
        handoff::rpc(&home.join("app.sock"), "app.ping", json!({}))?["build"].clone(),
    )?;
    let schema_before = before.schema;
    let j = Journal {
        local: None,
        token: token.clone(),
        phase: "prepared".into(),
        home: home.into(),
        target: target.clone(),
        staged: candidate,
        previous: parent.join(format!(".Canban-previous-{token}.app")),
        build: Build {
            version: release.manifest.version,
            commit: release.manifest.commit,
            schema: release.manifest.schema,
            api: release.manifest.api,
            platform: release.manifest.platform,
        },
        schema_before,
        before: Some(before),
        ui,
        app_id,
        error: None,
        server: None,
        candidate_ui: None,
        recovery_ui: None,
    };
    j.save()?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join("updates/updater.log"))?;
    use std::os::unix::process::CommandExt;
    Command::new(helper)
        .arg("--home")
        .arg(home)
        .stdout(log.try_clone()?)
        .stderr(log)
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()?;
    Ok(())
}
fn copy_bundle(source: &Path, target: &Path) -> Result<()> {
    ensure!(!target.exists(), "Staged App already exists");
    std::fs::create_dir(target)?;
    for item in std::fs::read_dir(source)? {
        let item = item?;
        let to = target.join(item.file_name());
        let kind = item.file_type()?;
        if kind.is_dir() {
            copy_bundle(&item.path(), &to)?;
        } else {
            ensure!(kind.is_file(), "App contains a link or special file");
            std::fs::copy(item.path(), &to)?;
            File::open(&to)?.sync_all()?;
        }
    }
    File::open(target)?.sync_all()?;
    Ok(())
}
fn cli(j: &Journal) -> PathBuf {
    j.target.join("Contents/MacOS/canban")
}
pub(super) fn snapshot_path(j: &Journal) -> PathBuf {
    if let Some(local) = &j.local {
        local.backup.clone()
    } else {
        j.home
            .join("updates/jobs")
            .join(&j.token)
            .join("database-before.sqlite")
    }
}
pub(super) fn capture_snapshot(home: &Path, backup: &Path) -> Result<()> {
    if !home.join("canban.sqlite").exists() || backup.exists() {
        return Ok(());
    }
    if let Some(parent) = backup.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = backup.with_extension("pending");
    if temporary.exists() {
        std::fs::remove_file(&temporary)?;
    }
    let _service = service_lock(home)?;
    canban_storage::migrations::backup(home, &temporary)?;
    std::fs::rename(&temporary, backup)?;
    if let Some(parent) = backup.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}
pub(super) fn restore_snapshot(home: &Path, backup: &Path, expected_schema: u32) -> Result<bool> {
    if !backup.exists() {
        return Ok(false);
    }
    let _service = service_lock(home)?;
    let staged = backup.with_extension("restore");
    if staged.exists() {
        std::fs::remove_file(&staged)?;
    }
    std::fs::copy(backup, &staged)?;
    File::open(&staged)?.sync_all()?;
    for suffix in ["-wal", "-shm"] {
        let path = home.join(format!("canban.sqlite{suffix}"));
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    std::fs::rename(staged, home.join("canban.sqlite"))?;
    File::open(home)?.sync_all()?;
    ensure!(
        canban_storage::migrations::database_version(home)? == expected_schema,
        "Database snapshot restoration failed"
    );
    Ok(true)
}
/// Removes transient staging/session files, deletes the committed pre-upgrade SQLite
/// snapshot, and prunes historical updater job/bundle directories so disk usage stays O(1).
pub(super) fn prune_update_artifacts(j: &Journal) {
    let backup = snapshot_path(j);
    let _ = std::fs::remove_file(&backup);
    let _ = std::fs::remove_file(backup.with_extension("pending"));
    let _ = std::fs::remove_file(backup.with_extension("restore"));

    let updates = j.home.join("updates");
    if let Ok(entries) = std::fs::read_dir(&updates) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with("ui-ready-") || name.starts_with("ui-saved-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let staging = updates.join("staging");
    if let Ok(entries) = std::fs::read_dir(&staging) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    let jobs = updates.join("jobs");
    if let Ok(entries) = std::fs::read_dir(&jobs) {
        for entry in entries.flatten() {
            if entry.file_name().to_str() != Some(j.token.as_str()) {
                let path = entry.path();
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(path);
                } else {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
    if j.local.is_none()
        && let Some(parent) = j.target.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        let keep_prev = format!(".Canban-previous-{}.app", j.token);
        let keep_failed = format!(".Canban-failed-{}.app", j.token);
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let is_managed_backup = (name.starts_with(".Canban-previous-")
                || name.starts_with(".Canban-failed-")
                || name.starts_with(".Canban-next-"))
                && name.ends_with(".app");
            if is_managed_backup && name != keep_prev && name != keep_failed {
                let path = entry.path();
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(path);
                } else {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
}
fn database(j: &Journal, binary: &Path, target: u32) -> Result<()> {
    let out = Command::new(binary)
        .arg("--home")
        .arg(&j.home)
        .args(["database", "--target", &target.to_string()])
        .env("CANBAN_UPGRADE_TOKEN", &j.token)
        .env_remove("CANBAN_ENDPOINT")
        .output()?;
    ensure!(
        out.status.success(),
        "Database migration to {target} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    ensure!(
        canban_storage::migrations::database_version(&j.home)? == target,
        "Database migration did not reach schema {target}"
    );
    Ok(())
}
fn wait_unlock(home: &Path) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        if service_lock(home).is_ok() {
            return Ok(());
        }
        ensure!(
            Instant::now() < until,
            "AppServer did not release its database/service lock"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn environment(cmd: &mut Command, j: &Journal) {
    cmd.env("CANBAN_HOME", &j.home)
        .env("CANBAN_UPGRADE_TOKEN", &j.token)
        .env("CANBAN_APP_ID", &j.app_id)
        .env_remove("CANBAN_ENDPOINT")
        .env_remove("CANBAN_CARD")
        .env_remove("CARD_ID")
        .env_remove("CANBAN_PROJECT")
        .env_remove("CANBAN_PROFILE_ID");
}
fn spawn_server(j: &Journal) -> Result<Child> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(j.home.join("server.log"))?;
    let mut cmd = Command::new(cli(j));
    cmd.arg("--home")
        .arg(&j.home)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    environment(&mut cmd, j);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    Ok(cmd.spawn()?)
}
fn spawn_ui(j: &Journal) -> Result<Child> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(j.home.join("updates/ui.log"))?;
    let mut cmd = Command::new(j.target.join("Contents/MacOS/canban-ui"));
    cmd.stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    environment(&mut cmd, j);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    Ok(cmd.spawn()?)
}
fn health(j: &Journal, server: &mut Child, expected: &Build) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(30);
    let endpoint = j.home.join("app.sock");
    loop {
        ensure!(
            server.try_wait()?.is_none(),
            "Candidate AppServer exited before readiness"
        );
        if let Ok(ping) = handoff::rpc(&endpoint, "app.ping", json!({})) {
            ensure!(
                ping["pid"].as_u64() == Some(server.id() as u64),
                "A different service answered candidate readiness"
            );
            let build: Build = serde_json::from_value(ping["build"].clone())?;
            ensure!(build == *expected, "AppServer build identity mismatch");
            handoff::rpc(&endpoint, "app.state", json!({}))
                .context("Candidate AppServer failed initial app.state check")?;
            return Ok(());
        }
        ensure!(
            Instant::now() < until,
            "Candidate AppServer readiness timed out"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn state(j: &Journal, phase: &str) -> Result<()> {
    let mut s = load_state(&j.home);
    s.phase = phase.into();
    s.error = j.error.clone();
    save_state(&j.home, &s)
}
/// Re-running a journal after interruption chooses recovery, never blindly
/// replays physical installation steps.
pub fn run(home: &Path, recover: bool) -> Result<()> {
    let _upgrade = lock(home, "upgrade.lock")?;
    let mut j = handoff::journal(home)?;
    ensure!(j.home == home, "Upgrade journal home mismatch");
    if matches!(j.phase.as_str(), "complete" | "rolled_back") {
        return Ok(());
    }
    ensure!(
        j.phase != "manual_recovery" || recover,
        "Manual recovery required: {}",
        j.error.as_deref().unwrap_or("unknown")
    );
    if j.local.is_some() {
        return local::run(&mut j, recover);
    }
    if recover && matches!(j.phase.as_str(), "prepared" | "stopping") {
        let _ = std::fs::remove_dir_all(&j.staged);
        j.phase = "rolled_back".into();
        j.error = Some("Interrupted upgrade cancelled before database migration".into());
        j.save()?;
        state(&j, "error")?;
        prune_update_artifacts(&j);
        return Ok(());
    }
    if recover && j.phase == "manual_recovery" {
        j.phase = "recovering".into();
        j.save()?;
    }
    let result = if recover || j.phase != "prepared" {
        rollback(&mut j)
    } else {
        install(&mut j)
    };
    if let Err(error) = result {
        if j.phase == "complete" {
            return Err(error);
        }
        j.error = Some(format!("{error:#}"));
        j.save()?;
        if !recover && j.phase == "stopping" {
            // No migration/bundle switch has begun. Keep a responsive old UI
            // and service in place instead of creating a second authority.
            let _ = std::fs::remove_dir_all(&j.staged);
            j.phase = "rolled_back".into();
            j.save()?;
            state(&j, "error")?;
            prune_update_artifacts(&j);
            if j.ui.as_ref().is_some_and(|ui| process(ui.pid).is_err()) {
                let _ = spawn_ui(&j);
            }
            return Ok(());
        }

        if recover || j.phase == "manual_recovery" {
            j.phase = "manual_recovery".into();
            j.save()?;
            state(&j, "manual_recovery")?;
            stop_children(&j);
            show_recovery(&mut j);
            return Err(error);
        }
        if let Err(recovery) = rollback(&mut j) {
            j.phase = "manual_recovery".into();
            j.error = Some(format!(
                "{}; recovery failed: {recovery:#}",
                j.error.unwrap_or_default()
            ));
            j.save()?;
            state(&j, "manual_recovery")?;
            stop_children(&j);
            show_recovery(&mut j);
            bail!("{}", j.error.unwrap_or_default());
        }
    }
    Ok(())
}
fn install(j: &mut Journal) -> Result<()> {
    state(j, "installing")?;
    j.phase = "stopping".into();
    j.save()?;
    if let Some(ui) = &j.ui {
        let until = Instant::now() + Duration::from_secs(15);
        while !j
            .home
            .join("updates")
            .join(format!("ui-saved-{}", j.token))
            .exists()
        {
            ensure!(
                process(ui.pid).is_ok_and(|v| v.identity == ui.identity),
                "Old UI exited before saving its session"
            );
            ensure!(
                Instant::now() < until,
                "Old UI did not preserve its session"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        ensure!(
            process(ui.pid).is_ok_and(|p| p.identity == ui.identity),
            "Source UI closed during download; retry from the current window"
        );
        stop_process(ui)?;
    }
    handoff::rpc(&j.home.join("app.sock"), "app.shutdown", json!({}))?;
    wait_unlock(&j.home)?;
    if j.home.join("canban.sqlite").exists() {
        j.schema_before = canban_storage::migrations::database_version(&j.home)?;
    }
    j.phase = "migrating".into();
    j.save()?;
    capture_snapshot(&j.home, &snapshot_path(j))?;
    database(j, &j.staged.join("Contents/MacOS/canban"), j.build.schema)?;
    j.phase = "swapping".into();
    j.save()?;
    std::fs::rename(&j.target, &j.previous)?;
    std::fs::rename(&j.staged, &j.target)?;
    #[cfg(target_os = "macos")]
    refresh_bundle_registration(&j.target);
    File::open(j.target.parent().context("App parent missing")?)?.sync_all()?;
    j.phase = "starting".into();
    j.save()?;
    boot(j, &j.build.clone())?;
    j.phase = "complete".into();
    j.error = None;
    j.save()?;
    state(j, "current")?;
    let mut s = load_state(&j.home);
    s.available = None;
    s.progress = 0;
    save_state(&j.home, &s)?;
    prune_update_artifacts(j);
    // Journal commit opens normal business operations before flushing durable reports.
    let _ = std::fs::remove_file(j.home.join("updates/ui-session.json"));
    if let Err(e) = handoff::flush(&j.home) {
        eprintln!("Queued Agent reports await retry: {e:#}");
    }
    Ok(())
}
fn boot(j: &mut Journal, expected: &Build) -> Result<()> {
    let mut server = spawn_server(j)?;
    j.server = Some(process(server.id())?);
    j.save()?;
    health(j, &mut server, expected)?;
    j.server = Some(process(server.id())?);
    j.save()?;
    let ready = j.home.join("updates").join(format!("ui-ready-{}", j.token));
    let _ = std::fs::remove_file(&ready);
    let mut ui = spawn_ui(j)?;
    j.candidate_ui = Some(process(ui.id())?);
    j.save()?;
    let until = Instant::now() + Duration::from_secs(30);
    let ready = j.home.join("updates").join(format!("ui-ready-{}", j.token));
    loop {
        ensure!(
            ui.try_wait()?.is_none(),
            "Candidate UI exited before readiness"
        );
        ensure!(
            server.try_wait()?.is_none(),
            "Candidate service exited during UI startup"
        );
        if ready.exists() {
            let b: Build = serde_json::from_slice(&std::fs::read(&ready)?)?;
            ensure!(b == *expected, "UI version identity mismatch");
            break;
        }
        ensure!(Instant::now() < until, "Candidate UI readiness timed out");
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
fn rollback(j: &mut Journal) -> Result<()> {
    if let Some(ui) = j.recovery_ui.take() {
        stop_process(&ui)?;
    }
    if let Some(ui) = &j.candidate_ui {
        stop_process(ui)?;
    }
    // Never shut down an unrelated process found at a reused socket/PID.
    if let Some(server) = &j.server
        && process(server.pid).is_ok_and(|p| p.identity == server.identity)
    {
        let _ = handoff::rpc(&j.home.join("app.sock"), "app.shutdown", json!({}));
    }
    // A crash may have happened before the original service acknowledged shutdown.
    if j.server.is_none() {
        let _ = handoff::rpc(&j.home.join("app.sock"), "app.shutdown", json!({}));
    }
    wait_unlock(&j.home)?;
    let candidate = if j.previous.exists() && j.target.exists() {
        cli(j)
    } else {
        j.staged.join("Contents/MacOS/canban")
    };
    if j.home.join("canban.sqlite").exists() {
        let current_schema = canban_storage::migrations::database_version(&j.home)?;
        // If schema 16+ short-ID path rewrites already ran, run candidate down first so
        // sync_paths_down restores on-disk Card/archive directories before restoring SQLite.
        if current_schema != j.schema_before && (j.schema_before < 16 && current_schema >= 16) {
            let _ = database(j, &candidate, j.schema_before);
        }
        let backup = snapshot_path(j);
        if backup.exists() {
            match restore_snapshot(&j.home, &backup, j.schema_before) {
                Ok(_) => {}
                Err(snapshot_err) => {
                    if canban_storage::migrations::database_version(&j.home)? != j.schema_before {
                        database(j, &candidate, j.schema_before).with_context(|| {
                            format!("snapshot restore failed ({snapshot_err:#}) and fallback down migration failed")
                        })?;
                    } else {
                        return Err(snapshot_err);
                    }
                }
            }
        } else if canban_storage::migrations::database_version(&j.home)? != j.schema_before {
            database(j, &candidate, j.schema_before)?;
        }
    }
    if j.previous.exists() {
        if j.target.exists() {
            let failed = j
                .target
                .with_file_name(format!(".Canban-failed-{}.app", j.token));
            ensure!(
                !failed.exists(),
                "Failed App preservation path already exists"
            );
            std::fs::rename(&j.target, failed)?;
        }
        std::fs::rename(&j.previous, &j.target)?;
        #[cfg(target_os = "macos")]
        refresh_bundle_registration(&j.target);
        File::open(j.target.parent().context("App parent missing")?)?.sync_all()?;
    }
    j.phase = "restoring".into();
    j.save()?;
    let before = if let Some(build) = j.before.clone() {
        build
    } else {
        let output = Command::new(cli(j)).arg("build-info").output()?;
        ensure!(
            output.status.success(),
            "Restored CLI identity is unavailable"
        );
        serde_json::from_slice(&output.stdout)?
    };
    boot(j, &before)?;
    j.phase = "rolled_back".into();
    j.save()?;
    state(j, "error")?;
    prune_update_artifacts(j);
    let _ = std::fs::remove_file(j.home.join("updates/ui-session.json"));
    if let Err(e) = handoff::flush(&j.home) {
        eprintln!("Queued Agent reports await retry: {e:#}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn refresh_bundle_registration(bundle: &Path) {
    if let Ok(file) = File::open(bundle) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
    let lsregister = Path::new(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
    );
    if lsregister.is_file() {
        let _ = Command::new(lsregister).arg("-f").arg(bundle).output();
    }
}

fn stop_children(j: &Journal) {
    if let Some(ui) = &j.candidate_ui {
        let _ = stop_process(ui);
    }
    if let Some(server) = &j.server
        && process(server.pid).is_ok_and(|p| p.identity == server.identity)
    {
        let _ = handoff::rpc(&j.home.join("app.sock"), "app.shutdown", json!({}));
        let _ = wait_unlock(&j.home);
    }
}
fn show_recovery(j: &mut Journal) {
    let source = if j.previous.exists() {
        &j.previous
    } else {
        &j.target
    };
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(j.home.join("updates/ui.log"));
    if let Ok(log) = log {
        let mut command = Command::new(source.join("Contents/MacOS/canban-ui"));
        environment(&mut command, j);
        command
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap_or(log));
        if let Ok(child) = command.spawn() {
            j.recovery_ui = process(child.id()).ok();
            let _ = j.save();
        }
    }
}
#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn interrupted_preparation_cancels_without_opening_or_restoring_data() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(home.join("canban.sqlite"), b"must not open this file").unwrap();
        let j = Journal {
            local: None,
            token: "fixture".into(),
            phase: "prepared".into(),
            home: home.into(),
            target: home.join("Canban.app"),
            staged: home.join("next.app"),
            previous: home.join("old.app"),
            build: Build::current(),
            schema_before: 8,
            before: None,
            ui: None,
            app_id: "test.app".into(),
            error: None,
            server: None,
            candidate_ui: None,
            recovery_ui: None,
        };
        j.save().unwrap();
        run(home, true).unwrap();
        assert_eq!(handoff::journal(home).unwrap().phase, "rolled_back");
        assert_eq!(
            std::fs::read(home.join("canban.sqlite")).unwrap(),
            b"must not open this file"
        );
    }

    #[test]
    fn snapshot_capture_restores_same_schema_or_migrated_data_and_cleans_wal() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        drop(canban_storage::Store::open(home).unwrap());
        let schema = canban_storage::migrations::database_version(home).unwrap();
        {
            let conn = rusqlite::Connection::open(home.join("canban.sqlite")).unwrap();
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE sentinel(value TEXT);
                 INSERT INTO sentinel VALUES('before-upgrade');",
            )
            .unwrap();
        }
        let backup = home.join("updates/jobs/tok-1/database-before.sqlite");
        capture_snapshot(home, &backup).unwrap();
        assert!(backup.is_file());

        // Simulate destructive candidate migration / same-schema data mutation + WAL residue.
        {
            let conn = rusqlite::Connection::open(home.join("canban.sqlite")).unwrap();
            conn.execute_batch(
                "DELETE FROM sentinel;
                 INSERT INTO sentinel VALUES('corrupted-by-candidate');
                 PRAGMA user_version = 999;",
            )
            .unwrap();
            // Keep uncheckpointed WAL pages around before closing.
        }
        std::fs::write(home.join("canban.sqlite-wal"), b"stale-wal-bytes").unwrap();
        std::fs::write(home.join("canban.sqlite-shm"), b"stale-shm-bytes").unwrap();

        assert!(restore_snapshot(home, &backup, schema).unwrap());
        assert!(!home.join("canban.sqlite-wal").exists());
        assert!(!home.join("canban.sqlite-shm").exists());
        assert_eq!(
            canban_storage::migrations::database_version(home).unwrap(),
            schema
        );
        let conn = rusqlite::Connection::open(home.join("canban.sqlite")).unwrap();
        let val: String = conn
            .query_row("SELECT value FROM sentinel", [], |r| r.get(0))
            .unwrap();
        assert_eq!(val, "before-upgrade");
    }

    #[test]
    fn prune_update_artifacts_bounds_snapshots_jobs_staging_and_bundle_backups() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let home = root.join("home");
        let apps = root.join("apps");
        std::fs::create_dir_all(home.join("updates/jobs/old-tok")).unwrap();
        std::fs::create_dir_all(home.join("updates/jobs/cur-tok")).unwrap();
        std::fs::create_dir_all(home.join("updates/staging/dl-1")).unwrap();
        std::fs::create_dir_all(apps.join("Canban.app")).unwrap();
        std::fs::create_dir_all(apps.join(".Canban-previous-old-tok.app")).unwrap();
        std::fs::create_dir_all(apps.join(".Canban-failed-old-tok.app")).unwrap();
        std::fs::create_dir_all(apps.join(".Canban-next-old-tok.app")).unwrap();
        std::fs::create_dir_all(apps.join(".Canban-previous-cur-tok.app")).unwrap();
        std::fs::create_dir_all(apps.join("Unrelated.app")).unwrap();

        std::fs::write(home.join("updates/jobs/old-tok/canban-updater"), b"old").unwrap();
        std::fs::write(
            home.join("updates/jobs/old-tok/database-before.sqlite"),
            b"old-db",
        )
        .unwrap();
        std::fs::write(home.join("updates/jobs/cur-tok/canban-updater"), b"cur").unwrap();
        std::fs::write(
            home.join("updates/jobs/cur-tok/database-before.sqlite"),
            b"cur-db",
        )
        .unwrap();
        std::fs::write(home.join("updates/ui-ready-old-tok"), b"{}").unwrap();
        std::fs::write(home.join("updates/ui-ready-cur-tok"), b"{}").unwrap();
        std::fs::write(home.join("updates/ui-saved-cur-tok"), b"ready").unwrap();
        std::fs::write(home.join("updates/staging/dl-1/download.tar.zst"), b"zst").unwrap();

        let j = Journal {
            local: None,
            token: "cur-tok".into(),
            phase: "complete".into(),
            home: home.clone(),
            target: apps.join("Canban.app"),
            staged: apps.join(".Canban-next-cur-tok.app"),
            previous: apps.join(".Canban-previous-cur-tok.app"),
            build: Build::current(),
            schema_before: 19,
            before: None,
            ui: None,
            app_id: "com.canban.test".into(),
            error: None,
            server: None,
            candidate_ui: None,
            recovery_ui: None,
        };
        prune_update_artifacts(&j);

        assert!(
            !home
                .join("updates/jobs/cur-tok/database-before.sqlite")
                .exists()
        );
        assert!(home.join("updates/jobs/cur-tok/canban-updater").is_file());
        assert!(!home.join("updates/jobs/old-tok").exists());
        assert!(!home.join("updates/ui-ready-old-tok").exists());
        assert!(!home.join("updates/ui-ready-cur-tok").exists());
        assert!(!home.join("updates/ui-saved-cur-tok").exists());
        assert!(
            std::fs::read_dir(home.join("updates/staging"))
                .unwrap()
                .next()
                .is_none()
        );
        assert!(!apps.join(".Canban-previous-old-tok.app").exists());
        assert!(!apps.join(".Canban-failed-old-tok.app").exists());
        assert!(!apps.join(".Canban-next-old-tok.app").exists());
        assert!(apps.join(".Canban-previous-cur-tok.app").is_dir());
        assert!(apps.join("Canban.app").is_dir());
        assert!(apps.join("Unrelated.app").is_dir());
    }
}
