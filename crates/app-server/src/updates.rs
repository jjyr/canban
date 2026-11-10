use super::*;
impl AppServer {
    pub(super) fn update_call(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "update.state" => Ok(
                json!({"build":canban_updater::Build::current(),"state":canban_updater::load_state(&self.home),"install":canban_updater::handoff::journal(&self.home).ok().map(|j|json!({"token":j.token,"phase":j.phase}))}),
            ),
            "update.check" => {
                let home = self.home.clone();
                let github = self.github.clone();
                std::thread::spawn(move || {
                    if let Err(e) = canban_updater::feed::check(&home, github.client().ok()) {
                        eprintln!("Canban update check: {e:#}");
                    }
                });
                Ok(json!({"checking":true}))
            }
            "update.install" => {
                ensure!(
                    p["execution"].is_null(),
                    "Installing an update requires an explicit user action"
                );
                ensure!(
                    !canban_updater::handoff::active(&self.home),
                    "Upgrade is already in progress"
                );
                let ui_pid = p["ui_pid"].as_u64().context("UI process ID required")?;
                ensure!(ui_pid <= u32::MAX as u64, "Invalid UI process ID");
                let process = canban_updater::worker::process(ui_pid as u32)?;
                let ui = self.cli.with_file_name("canban-ui");
                ensure!(
                    process
                        .identity
                        .contains(ui.to_str().context("Invalid UI path")?),
                    "Only this AppServer's packaged UI can install an update"
                );
                let operation = canban_updater::lock(&self.home, "operation.lock")?;
                let mut state = canban_updater::load_state(&self.home);
                ensure!(state.available.is_some(), "Check for an update first");
                ensure!(state.available.as_ref().is_some_and(|a|Some(a.manifest.version.as_str())==p["version"].as_str()),"Available version changed; refresh before installing");
                state.phase = "downloading".into();
                state.error = None;
                state.progress = 0;
                canban_updater::save_state(&self.home, &state)?;
                let home = self.home.clone();
                let cli = self.cli.clone();
                let github = self.github.clone();
                let app_id = p["app_id"]
                    .as_str()
                    .unwrap_or("com.canban.desktop")
                    .to_owned();
                ensure!(
                    app_id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c)),
                    "Invalid App identity"
                );
                std::thread::spawn(move || {
                    if let Err(e) = canban_updater::worker::prepare_locked(
                        &home,
                        &cli,
                        Some(process),
                        app_id,
                        operation,
                        github.client().ok(),
                    ) {
                        let mut s = canban_updater::load_state(&home);
                        s.phase = "error".into();
                        s.error = Some(format!("{e:#}"));
                        let _ = canban_updater::save_state(&home, &s);
                    }
                });
                Ok(json!({"installing":true}))
            }
            _ => bail!("Unknown update operation: {method}"),
        }
    }
    pub(super) fn start_update_checks(self: &Arc<Self>) {
        let home = self.home.clone();
        let weak = Arc::downgrade(self);
        let github = self.github.clone();
        std::thread::spawn(move || {
            let mut next = std::time::Instant::now();
            loop {
                // A dead service cannot keep checking. The socket probe does not
                // acquire database state or retain an AppServer Arc during I/O.
                if weak.upgrade().is_none_or(|s| s.stopped()) {
                    break;
                }
                if !canban_updater::handoff::active(&home) {
                    let _ = canban_updater::handoff::flush(&home);
                    if std::time::Instant::now() >= next
                        && std::env::var_os("CANBAN_UPDATES_DISABLED").is_none()
                    {
                        next = std::time::Instant::now() + Duration::from_secs(3600);
                        let _ = canban_updater::feed::check(&home, github.client().ok());
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }
}
