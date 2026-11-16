//! Activate the already installed App using the updater's journal/locks/readiness.
//! No release discovery, asset download or App-bundle replacement occurs here.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Handoff {
    pub peer: Process,
    pub backup: PathBuf,
}

pub fn prepare(
    home: &Path,
    cli: &Path,
    peer: &serde_json::Value,
    ui_pid: u32,
    app_id: String,
) -> Result<()> {
    let _operation = lock(home, "operation.lock")?;
    ensure!(
        !handoff::active(home),
        "A backend update is already in progress"
    );
    let home = home.canonicalize()?;
    let cli = cli.canonicalize()?;
    let target = bundle(&cli)?;
    ensure!(
        !crate::bundle_dev_instance(&target).exists(),
        "Use mise run dev to restart a development instance"
    );
    let output = Command::new(&cli)
        .arg("build-info")
        .env_remove("CANBAN_ENDPOINT")
        .output()?;
    ensure!(
        output.status.success(),
        "Installed CLI build information is unavailable"
    );
    let build: Build = serde_json::from_slice(&output.stdout)?;
    let current = handoff::rpc(&home.join("app.sock"), "app.ping", json!({}))?;
    ensure!(
        current["pid"] == peer["pid"],
        "Backend changed; reopen the App before updating"
    );
    ensure!(
        PathBuf::from(
            current["home"]
                .as_str()
                .context("Backend did not report its home")?
        )
        .canonicalize()?
            == home,
        "Backend belongs to another home"
    );
    ensure!(
        backend_order(&current, &build).is_lt(),
        "Backend is no longer older; reopen the App"
    );
    let pid = u32::try_from(
        current["pid"]
            .as_u64()
            .context("Backend did not report its PID")?,
    )?;
    let old_server = process(pid)?;
    let ui = process(ui_pid)?;
    ensure!(
        ui.identity.contains(
            crate::bundle_ui(&target)
                .to_str()
                .context("Invalid UI path")?
        ),
        "Update must be started by this installed App"
    );
    let schema_before = canban_storage::migrations::database_version(&home)?;
    ensure!(
        schema_before <= build.schema,
        "Database is newer than this App; use a newer Canban App"
    );
    let token = canban_types::token_id();
    let job = home.join("updates/jobs").join(&token);
    std::fs::create_dir_all(&job)?;
    let helper = job.join("canban-updater");
    std::fs::copy(crate::bundle_updater(&target), &helper)?;
    let j = Journal {
        local: Some(Handoff {
            peer: old_server,
            backup: job.join("database-before.sqlite"),
        }),
        token,
        phase: "prepared".into(),
        home: home.clone(),
        target: target.clone(),
        staged: target,
        previous: job.join("unused-app-backup"),
        build,
        schema_before,
        before: serde_json::from_value(current["build"].clone()).ok(),
        ui: Some(ui),
        app_id,
        error: None,
        server: None,
        candidate_ui: None,
        recovery_ui: None,
        systemd_managed: false,
    };
    j.save()?;
    state(&j, "installing")?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join("updates/updater.log"))?;
    use std::os::unix::process::CommandExt;
    Command::new(helper)
        .arg("--home")
        .arg(home)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()?;
    Ok(())
}

fn restore_data(j: &Journal) -> Result<()> {
    let local = j.local.as_ref().context("Missing local handoff")?;
    let _ = super::restore_snapshot(&j.home, &local.backup, j.schema_before)?;
    Ok(())
}

fn install(j: &mut Journal) -> Result<()> {
    j.phase = "stopping".into();
    j.save()?;
    if let Some(ui) = &j.ui {
        stop_process(ui)?;
    }
    let old = j
        .local
        .as_ref()
        .context("Missing local handoff")?
        .peer
        .clone();
    match handoff::rpc(&j.home.join("app.sock"), "app.ping", json!({})) {
        Ok(ping) => {
            ensure!(
                ping["pid"].as_u64() == Some(old.pid.into())
                    && process(old.pid)?.identity == old.identity,
                "Backend changed; refusing to stop another process"
            );
            if !j.systemd_managed && super::systemd_manages_home(&j.home) {
                j.systemd_managed = true;
                j.save()?;
            }
            if j.systemd_managed {
                super::stop_systemd_service()?;
                let _ = handoff::rpc(
                    &j.home.join("app.sock"),
                    "app.shutdown",
                    json!({"expected_pid":old.pid}),
                );
            } else {
                handoff::rpc(
                    &j.home.join("app.sock"),
                    "app.shutdown",
                    json!({"expected_pid":old.pid}),
                )?;
            }
        }
        Err(error) => ensure!(
            !process(old.pid).is_ok_and(|p| p.identity == old.identity),
            "Old backend is not responding: {error:#}"
        ),
    }
    wait_unlock(&j.home)?;
    let schema = canban_storage::migrations::database_version(&j.home)?;
    ensure!(
        schema <= j.build.schema,
        "Database is newer than the installed App"
    );
    j.schema_before = schema;
    if schema < j.build.schema {
        j.phase = "migrating".into();
        j.save()?;
        let backup = j.local.as_ref().unwrap().backup.clone();
        super::capture_snapshot(&j.home, &backup)?;
        database(j, &cli(j), j.build.schema)?;
    } else {
        let backup = j.local.as_ref().unwrap().backup.clone();
        super::capture_snapshot(&j.home, &backup)?;
    }
    j.phase = "starting".into();
    j.save()?;
    boot(j, &j.build.clone())?;
    j.phase = "complete".into();
    j.error = None;
    j.save()?;
    let mut saved = load_state(&j.home);
    saved.phase = "current".into();
    saved.error = None;
    saved.available = None;
    save_state(&j.home, &saved)?;
    super::prune_update_artifacts(j);
    let _ = handoff::flush(&j.home);
    Ok(())
}

pub(super) fn run(j: &mut Journal, recover: bool) -> Result<()> {
    if recover || j.phase != "prepared" {
        if let Some(ui) = j.recovery_ui.take() {
            stop_process(&ui)?;
        }
        stop_children(j);
        restore_data(j)?;
        // Refresh expected Build from the current installed target bundle so recovering
        // after installing a newer Canban.app adopts its schema and API version.
        if let Ok(output) = Command::new(cli(j))
            .arg("build-info")
            .env_remove("CANBAN_ENDPOINT")
            .output()
            && output.status.success()
            && let Ok(current_build) = serde_json::from_slice::<Build>(&output.stdout)
        {
            j.build = current_build;
        }
        j.server = None;
        j.candidate_ui = None;
        j.ui = None;
        j.phase = "prepared".into();
        j.save()?;
    }
    if let Err(error) = install(j) {
        let before_data = j.phase == "stopping";
        j.error = Some(format!("{error:#}"));
        if before_data {
            j.phase = "rolled_back".into();
            j.save()?;
            state(j, "error")?;
            super::prune_update_artifacts(j);
            // Reopen the startup prompt; no database or installed files changed.
            if j.ui.is_none() || j.ui.as_ref().is_some_and(|ui| process(ui.pid).is_err()) {
                show_recovery(j);
            }
            return Err(error);
        }
        stop_children(j);
        if let Err(restore) = restore_data(j) {
            j.error = Some(format!("{error:#}; restoring data failed: {restore:#}"));
        }
        j.phase = "manual_recovery".into();
        j.save()?;
        state(j, "manual_recovery")?;
        show_recovery(j);
        return Err(error);
    }
    Ok(())
}
