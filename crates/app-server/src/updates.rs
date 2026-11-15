use super::*;
impl AppServer {
    pub(super) fn update_call(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "update.state" => Ok(
                json!({"build":canban_updater::Build::current(),"disabled":self.updates_disabled,"state":canban_updater::load_state(&self.home),"install":canban_updater::handoff::journal(&self.home).ok().map(|j|json!({"token":j.token,"phase":j.phase}))}),
            ),
            "update.check" => {
                ensure!(
                    !self.updates_disabled,
                    "Channel updates are disabled for development instances; use mise run dev to rebuild"
                );
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
                    !self.updates_disabled,
                    "Channel updates are disabled for development instances; use mise run dev to rebuild"
                );
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
            "update.daemon" => {
                ensure!(
                    p["execution"].is_null(),
                    "Updating a daemon requires an explicit user action"
                );
                ensure!(
                    !self.updates_disabled,
                    "Channel updates are disabled for development instances; use mise run dev to rebuild"
                );
                ensure!(
                    !canban_updater::handoff::active(&self.home),
                    "Upgrade is already in progress"
                );
                let operation = canban_updater::lock(&self.home, "operation.lock")?;
                let target_version = p["version"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                let current = canban_updater::Build::current();
                let gh_client = self.github.client().ok();
                let Some(release) = canban_updater::feed::discover_cli(
                    &current,
                    target_version.as_deref(),
                    gh_client.as_ref(),
                )?
                else {
                    return Ok(json!({
                        "updated": false,
                        "version": current.version,
                        "api_version": API_VERSION,
                        "message": format!(
                            "Daemon is already on the latest published release ({}, API v{}).",
                            current.version, API_VERSION
                        ),
                    }));
                };
                let target_ver = release.manifest.version.clone();
                let target_api = release.manifest.api;
                let home = self.home.clone();
                let cli_target = self.cli.clone();
                let this = self;
                let staged_bin =
                    canban_updater::feed::stage_cli(&home, &release, gh_client.as_ref())?;
                let parent = cli_target
                    .parent()
                    .context("Daemon executable parent directory missing")?;
                let tmp_bin = parent.join(format!(".canban-next-{}", canban_types::id()));
                std::fs::copy(&staged_bin, &tmp_bin).with_context(|| {
                    format!("Could not write updated binary to {}", parent.display())
                })?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&tmp_bin, std::fs::Permissions::from_mode(0o755))?;
                }
                std::fs::rename(&tmp_bin, &cli_target).with_context(|| {
                    format!(
                        "Could not replace daemon binary at {}",
                        cli_target.display()
                    )
                })?;
                let _ = std::fs::remove_dir_all(
                    staged_bin
                        .parent()
                        .unwrap_or_else(|| Path::new("/nonexistent")),
                );
                drop(operation);

                // Schedule graceful daemon restart right after flushing the RPC response so
                // `systemd --user` / `launchd` (`Restart=always` / `KeepAlive`) or standalone
                // `canban serve` boots the newly installed binary cleanly.
                let stop_flag_home = this.home.clone();
                let cli_for_restart = cli_target.clone();
                let expected_pid = std::process::id();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(120));
                    let svc = network::service_status(&stop_flag_home);
                    if svc["kind"] == "systemd-user" && svc["installed"] == true {
                        if std::process::Command::new("systemctl")
                            .args(["--user", "restart", "canban.service"])
                            .status()
                            .is_ok_and(|s| s.success())
                        {
                            return;
                        }
                    }
                    if svc["kind"] == "launchd" && svc["installed"] == true {
                        let uid = std::process::Command::new("id")
                            .arg("-u")
                            .output()
                            .ok()
                            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                            .unwrap_or_default();
                        if !uid.is_empty()
                            && std::process::Command::new("launchctl")
                                .args(["kickstart", "-k", &format!("gui/{uid}/com.canban.daemon")])
                                .status()
                                .is_ok_and(|s| s.success())
                        {
                            return;
                        }
                    }
                    // Fallback for standalone `canban serve`: request graceful shutdown via local socket, then spawn replacement.
                    let listen_cfg = network::load_listen_config(&stop_flag_home);
                    let _ = canban_updater::handoff::rpc(
                        &stop_flag_home.join("app.sock"),
                        "app.shutdown",
                        json!({ "expected_pid": expected_pid }),
                    );
                    for _ in 0..50 {
                        if UnixStream::connect(stop_flag_home.join("app.sock")).is_err() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    let mut cmd = std::process::Command::new(&cli_for_restart);
                    cmd.arg("--home").arg(&stop_flag_home).arg("serve");
                    if listen_cfg.tailscale {
                        cmd.arg("--tailscale");
                    }
                    if listen_cfg.port != 0 {
                        cmd.arg("--port").arg(listen_cfg.port.to_string());
                    }
                    if let Some(l) = &listen_cfg.listen {
                        cmd.arg("--listen").arg(l);
                    }
                    if let Ok(log) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(stop_flag_home.join("server.log"))
                        && let Ok(err_log) = log.try_clone()
                    {
                        cmd.stdout(log).stderr(err_log);
                    }
                    cmd.stdin(std::process::Stdio::null());
                    #[cfg(unix)]
                    {
                        use std::os::unix::process::CommandExt;
                        cmd.process_group(0);
                    }
                    let _ = cmd.spawn();
                });

                Ok(json!({
                    "updated": true,
                    "version": target_ver,
                    "api_version": target_api,
                    "restarting": true,
                }))
            }
            _ => bail!("Unknown update operation: {method}"),
        }
    }
    pub(super) fn start_update_checks(self: &Arc<Self>) {
        let updates_disabled = self.updates_disabled;
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
                    if !updates_disabled
                        && !cfg!(test)
                        && std::time::Instant::now() >= next
                        && std::env::var_os("CANBAN_UPDATES_DISABLED").is_none()
                    {
                        let result =
                            canban_updater::feed::check_automatically(&home, github.client().ok());
                        let delay = if result.is_ok() {
                            canban_updater::load_state(&home)
                                .automatic_delay(canban_updater::now())
                                .max(1)
                        } else {
                            60
                        };
                        next = std::time::Instant::now() + Duration::from_secs(delay);
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }
}
