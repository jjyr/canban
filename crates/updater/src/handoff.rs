//! Startup fencing and a durable, idempotent Agent-report outbox.
use super::*;
use anyhow::{bail, ensure};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader},
    os::unix::net::UnixStream,
    path::PathBuf,
};
pub fn journal(home: &Path) -> Result<worker::Journal> {
    Ok(serde_json::from_slice(&std::fs::read(
        home.join("updates/install.json"),
    )?)?)
}
pub fn active(home: &Path) -> bool {
    match std::fs::read(home.join("updates/install.json")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Ok(bytes) => serde_json::from_slice::<worker::Journal>(&bytes).map_or(true, |j| {
            !matches!(j.phase.as_str(), "complete" | "rolled_back")
        }),
        Err(_) => true,
    }
}
pub fn authorized(home: &Path) -> bool {
    journal(home)
        .is_ok_and(|j| std::env::var("CANBAN_UPGRADE_TOKEN").ok().as_deref() == Some(&j.token))
}
/// Held through database open/service-lock acquisition. The updater owns the
/// exclusive side; only its explicitly tokened candidate can bypass it.
pub fn startup_guard(home: &Path) -> Result<Option<File>> {
    std::fs::create_dir_all(home.join("updates"))?;
    if authorized(home) {
        ensure!(
            journal(home)?.phase != "manual_recovery",
            "Upgrade requires manual recovery"
        );
        return Ok(None);
    }
    ensure!(
        !active(home),
        "Canban upgrade is pending; inspect updates/install.json or run canban-updater --home HOME --recover"
    );
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.join("updates/upgrade.lock"))?;
    f.try_lock_shared()
        .context("Canban is upgrading; wait for the replacement service")?;
    Ok(Some(f))
}
pub fn rpc(endpoint: &Path, method: &str, params: Value) -> Result<Value> {
    let mut s = UnixStream::connect(endpoint)?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    s.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;
    s.write_all(
        format!(
            "{}\n",
            json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
        )
        .as_bytes(),
    )?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line)?;
    let r: Value = serde_json::from_str(&line)?;
    if let Some(error) = r.get("error") {
        bail!("RPC {method}: {error}");
    }
    Ok(r["result"].clone())
}
#[derive(Serialize, Deserialize)]
struct Pending {
    method: String,
    params: Value,
}
pub fn enqueue(home: &Path, method: &str, params: &Value) -> Result<PathBuf> {
    ensure!(
        matches!(
            method,
            "execution.report" | "batch.report" | "schedule.report" | "agent.reply"
        ),
        "Only idempotent Agent reports/replies can be queued"
    );
    ensure!(
        params["request_key"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "A stable request key is required"
    );
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&Pending {
        method: method.into(),
        params: params.clone(),
    })?;
    let path = home
        .join("updates/outbox")
        .join(format!("{:x}.json", Sha256::digest(&bytes)));
    atomic(&path, &bytes)?;
    Ok(path)
}
pub fn flush(home: &Path) -> Result<()> {
    flush_older_than(home, std::time::Duration::ZERO)
}

/// Periodic background flush skips fresh files (<2s old) that may currently be
/// in-flight inside a live `canban` CLI process, avoiding double RPC sends while
/// still flushing anything left behind by an interrupted upgrade within seconds.
pub fn flush_settled(home: &Path) -> Result<()> {
    flush_older_than(home, std::time::Duration::from_secs(2))
}

fn flush_older_than(home: &Path, min_age: std::time::Duration) -> Result<()> {
    if active(home) {
        return Ok(());
    }
    let Ok(_lock) = lock(home, "outbox.lock") else {
        return Ok(());
    };
    let dir = home.join("updates/outbox");
    if !dir.exists() {
        return Ok(());
    }
    for item in std::fs::read_dir(&dir)?.take(32) {
        let path = item?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if !min_age.is_zero()
            && let Ok(meta) = std::fs::metadata(&path)
            && let Ok(modified) = meta.modified()
            && modified.elapsed().is_ok_and(|age| age < min_age)
        {
            continue;
        }
        let Ok(raw) = std::fs::read(&path) else {
            continue;
        };
        let value: Pending = serde_json::from_slice(&raw)?;
        match rpc(&home.join("app.sock"), &value.method, value.params) {
            Ok(_) => {
                let _ = std::fs::remove_file(path);
            }
            Err(e) => {
                if e.to_string().starts_with("RPC ") && !active(home) && path.exists() {
                    atomic(&path.with_extension("rejected"), e.to_string().as_bytes())?;
                    let _ = std::fs::rename(&path, path.with_extension("pending-manual"));
                }
                break;
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_is_stable_and_rejects_physical_mutations() {
        let t = tempfile::tempdir().unwrap();
        let p = json!({"request_key":"same","body":"retained"});
        let a = enqueue(t.path(), "execution.report", &p).unwrap();
        assert_eq!(a, enqueue(t.path(), "execution.report", &p).unwrap());
        assert!(enqueue(t.path(), "review.merge", &p).is_err());
        assert_eq!(std::fs::read_dir(a.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn flush_settled_skips_inflight_outbox_entries_until_settled() {
        let t = tempfile::tempdir().unwrap();
        let p = json!({"request_key":"inflight","body":"retained"});
        let path = enqueue(t.path(), "execution.report", &p).unwrap();
        // Without a listening socket, an immediate `flush` would fail to connect,
        // whereas `flush_settled` leaves fresh (<2s) in-flight CLI files untouched.
        flush_settled(t.path()).unwrap();
        assert!(path.is_file());
    }
}

fn spawn_recover(
    home: &Path,
    guard: File,
    mut j: worker::Journal,
    bundled_updater: Option<&Path>,
    ui_pid: Option<u32>,
    use_bundled: bool,
    record_ui: bool,
) -> Result<()> {
    if let Some(pid) = ui_pid.filter(|_| record_ui) {
        j.recovery_ui = worker::process(pid).ok();
        j.save()?;
    }
    let job_dir = home.join("updates/jobs").join(&j.token);
    std::fs::create_dir_all(&job_dir)?;
    let helper = if let Some(src) = bundled_updater.filter(|p| use_bundled && p.is_file()) {
        let dest = job_dir.join("canban-updater-recover");
        let _ = std::fs::remove_file(&dest);
        std::fs::copy(src, &dest)?;
        dest
    } else {
        let saved = job_dir.join("canban-updater");
        ensure!(
            saved.is_file(),
            "Saved updater helper is missing at {}",
            saved.display()
        );
        saved
    };
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join("updates/updater.log"))?;
    drop(guard);
    use std::os::unix::process::CommandExt;
    canban_exec::raw_command(helper)
        .arg("--home")
        .arg(home)
        .arg("--recover")
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()?;
    Ok(())
}

/// A normal launch can resume an interrupted installer. The worker's exclusive
/// lock elects one recovery process; a manual-recovery journal is never retried.
/// Returns `Ok(true)` when recovery replaces the launching UI via a candidate window.
pub fn resume_with(
    home: &Path,
    bundled_updater: Option<&Path>,
    ui_pid: Option<u32>,
) -> Result<bool> {
    if authorized(home) || !active(home) {
        return Ok(false);
    }
    let j = journal(home)?;
    ensure!(
        j.phase != "manual_recovery",
        "Upgrade requires manual recovery; inspect updates/updater.log"
    );
    let Ok(guard) = lock(home, "upgrade.lock") else {
        return Ok(false);
    };
    let replaces_ui = j.local.is_some() || !matches!(j.phase.as_str(), "prepared" | "stopping");
    let use_bundled = j.local.is_some();
    spawn_recover(
        home,
        guard,
        j,
        bundled_updater,
        ui_pid,
        use_bundled,
        replaces_ui,
    )?;
    Ok(replaces_ui)
}

/// A normal launch can resume an interrupted installer. The worker's exclusive
/// lock elects one recovery process; a manual-recovery journal is never retried.
pub fn resume(home: &Path) -> Result<()> {
    resume_with(home, None, None).map(|_| ())
}

/// Explicitly trigger `--recover` from the native Recovery window.
/// Copies the latest `canban-updater` into the job directory so bundle renames stay safe,
/// and registers `ui_pid` as `recovery_ui` so the worker closes the Recovery window cleanly.
pub fn recover(home: &Path, bundled_updater: Option<&Path>, ui_pid: Option<u32>) -> Result<()> {
    let guard = lock(home, "upgrade.lock")?;
    let j = journal(home)?;
    spawn_recover(home, guard, j, bundled_updater, ui_pid, true, true)
}

#[cfg(test)]
mod guard_tests {
    use super::*;
    #[test]
    fn upgrade_and_startup_locks_are_mutually_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let exclusive = lock(dir.path(), "upgrade.lock").unwrap();
        assert!(startup_guard(dir.path()).is_err());
        drop(exclusive);
        let shared = startup_guard(dir.path()).unwrap();
        assert!(lock(dir.path(), "upgrade.lock").is_err());
        drop(shared);
        assert!(lock(dir.path(), "upgrade.lock").is_ok());
    }
}

#[cfg(test)]
mod journal_tests {
    use super::*;

    fn sample_local_journal(home: &Path, token: &str, phase: &str) -> worker::Journal {
        worker::Journal {
            local: Some(worker::local::Handoff {
                peer: worker::Process {
                    pid: 1,
                    identity: "old".into(),
                },
                backup: home.join("updates/backup.sqlite"),
            }),
            token: token.into(),
            phase: phase.into(),
            home: home.to_path_buf(),
            target: home.join("Canban.app"),
            staged: home.join("updates/staged"),
            previous: home.join("updates/previous"),
            build: Build::current(),
            schema_before: Build::current().schema,
            before: None,
            ui: None,
            app_id: "com.canban.test".into(),
            error: None,
            server: None,
            candidate_ui: None,
            recovery_ui: None,
            systemd_managed: false,
        }
    }

    #[test]
    fn corrupt_upgrade_state_fails_closed_before_database_open() {
        let dir = tempfile::tempdir().unwrap();
        atomic(&dir.path().join("updates/install.json"), b"invalid journal").unwrap();
        assert!(active(dir.path()));
        assert!(startup_guard(dir.path()).is_err());
        assert!(!dir.path().join("canban.sqlite").exists());
    }

    #[test]
    fn enqueue_accepts_batch_report_alongside_execution_report_and_agent_reply() {
        let dir = tempfile::tempdir().unwrap();
        let path = enqueue(
            dir.path(),
            "batch.report",
            &serde_json::json!({
                "batch_id": "merge-queue:proj:1234",
                "state": "completed",
                "message": "Resolved conflicts",
                "request_key": "req-batch-1",
            }),
        )
        .unwrap();
        assert!(path.is_file());
    }

    #[test]
    fn resume_with_returns_false_when_upgrade_lock_is_held() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("updates")).unwrap();
        sample_local_journal(dir.path(), "tok-locked", "starting")
            .save()
            .unwrap();
        let _guard = lock(dir.path(), "upgrade.lock").unwrap();
        assert!(!resume_with(dir.path(), None, None).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn resume_with_copies_bundled_updater_and_records_recovery_ui_for_local_handoff() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join("updates")).unwrap();
        sample_local_journal(home, "tok-local", "starting")
            .save()
            .unwrap();

        let marker = home.join("updates/ran.args");
        let bundled = home.join("bundled-canban-updater");
        std::fs::write(
            &bundled,
            format!("#!/bin/sh\necho \"$@\" > \"{}\"\n", marker.display()),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&bundled).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bundled, perms).unwrap();

        let replaces_ui = resume_with(home, Some(&bundled), Some(std::process::id())).unwrap();
        assert!(replaces_ui);

        let copied = home.join("updates/jobs/tok-local/canban-updater-recover");
        assert!(copied.is_file());
        let saved = journal(home).unwrap();
        assert_eq!(saved.recovery_ui.map(|p| p.pid), Some(std::process::id()));

        let mut args = String::new();
        for _ in 0..40 {
            if let Ok(content) = std::fs::read_to_string(&marker)
                && !content.is_empty()
            {
                args = content;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(args.contains("--home"), "unexpected marker args: {args:?}");
        assert!(
            args.contains("--recover"),
            "unexpected marker args: {args:?}"
        );
    }
}
