use super::*;
impl AppServer {
    pub(crate) fn update_state_payload(&self) -> Value {
        json!({
            "build": canban_updater::Build::current(),
            "disabled": self.updates_disabled,
            "state": canban_updater::load_state(&self.home),
            "install": canban_updater::handoff::journal(&self.home)
                .ok()
                .map(|j| json!({"token": j.token, "phase": j.phase})),
        })
    }

    pub(super) fn update_call(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "update.state" => {
                let _ = canban_updater::recover_stale_state(&self.home);
                Ok(self.update_state_payload())
            }
            "update.check" => {
                ensure!(
                    !self.updates_disabled,
                    "Channel updates are disabled for development instances; use mise run dev to rebuild"
                );
                let home = self.home.clone();
                let github = self.github.clone();
                let weak = self.self_weak.get().cloned().unwrap_or_default();
                self.spawn_blocking_bg(move || {
                    if let Err(e) = canban_updater::feed::check(&home, github.client().ok()) {
                        eprintln!("Canban update check: {e:#}");
                    }
                    if let Some(server) = weak.upgrade() {
                        server.emit_update();
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
                let active_cli = resolve_active_cli_path(&self.home, &self.cli);
                let ui_raw = self.cli.with_file_name("canban-ui");
                let ui_active = canban_updater::worker::bundle_root(&active_cli)
                    .map(|b| canban_updater::bundle_ui(&b))
                    .unwrap_or_else(|_| active_cli.with_file_name("canban-ui"));
                let matches_raw = ui_raw
                    .to_str()
                    .is_some_and(|s| process.identity.contains(s));
                let matches_active = ui_active
                    .to_str()
                    .is_some_and(|s| process.identity.contains(s));
                ensure!(
                    matches_raw || matches_active,
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
                self.emit_update();
                let home = self.home.clone();
                let cli = active_cli;
                let github = self.github.clone();
                let weak = self.self_weak.get().cloned().unwrap_or_default();
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
                self.spawn_blocking_bg(move || {
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
                        s.progress = 0;
                        s.error = Some(format!("{e:#}"));
                        let _ = canban_updater::save_state(&home, &s);
                    }
                    if let Some(server) = weak.upgrade() {
                        server.emit_update();
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
                let home = self.home.clone();
                let Some((target_ver, target_api, cli_for_restart)) = apply_daemon_update(
                    &home,
                    &self.cli,
                    &current,
                    target_version.as_deref(),
                    gh_client.as_ref(),
                    self.id_gen.as_ref(),
                )?
                else {
                    let api_version = self.api_version();
                    return Ok(json!({
                        "updated": false,
                        "version": current.version,
                        "api_version": api_version,
                        "message": format!(
                            "Daemon is already on the latest published release ({}, API v{}).",
                            current.version, api_version
                        ),
                    }));
                };
                drop(operation);

                // Schedule graceful daemon restart right after flushing the RPC response so
                // `systemd --user` / `launchd` (`Restart=always` / `KeepAlive`) or standalone
                // `canban serve` boots the newly installed binary cleanly.
                let stop_flag_home = self.home.clone();
                let expected_pid = std::process::id();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(120));
                    if canban_updater::worker::systemd_manages_home(&stop_flag_home)
                        && canban_exec::raw_command("systemctl")
                            .args(["--user", "restart", "canban.service"])
                            .status()
                            .is_ok_and(|s| s.success())
                    {
                        return;
                    }
                    let svc = network::service_status(&stop_flag_home);
                    let home_canon = std::fs::canonicalize(&stop_flag_home)
                        .unwrap_or_else(|_| stop_flag_home.clone());
                    let launchd_matches_home = svc["unit_path"]
                        .as_str()
                        .and_then(|p| std::fs::read_to_string(p).ok())
                        .is_some_and(|body| {
                            body.contains(&*stop_flag_home.to_string_lossy())
                                || body.contains(&*home_canon.to_string_lossy())
                        });
                    if svc["kind"] == "launchd" && svc["installed"] == true && launchd_matches_home
                    {
                        let uid = canban_exec::raw_command("id")
                            .arg("-u")
                            .output()
                            .ok()
                            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                            .unwrap_or_default();
                        if !uid.is_empty()
                            && canban_exec::raw_command("launchctl")
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
                    let mut cmd = canban_exec::raw_command(&cli_for_restart);
                    cmd.arg("--home").arg(&stop_flag_home).arg("serve");
                    canban_exec::strip_herdr_terminal_scope_env(&mut cmd);
                    cmd.env_remove("CANBAN_ENDPOINT")
                        .env_remove("CANBAN_CARD")
                        .env_remove("CARD_ID")
                        .env_remove("CANBAN_PROJECT")
                        .env_remove("CANBAN_PROFILE_ID")
                        .env_remove("CANBAN_SESSION_ID")
                        .env_remove("CANBAN_RUN_DIR");
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
        self.spawn_bg(update_checks_loop(
            Arc::downgrade(self),
            self.updates_disabled,
            self.home.clone(),
            self.github.clone(),
            self.stop_receiver(),
        ));
    }
}

pub(crate) fn update_check_step(
    weak: &std::sync::Weak<AppServer>,
    home: &Path,
    github: &Arc<canban_github::auth::Service>,
    updates_disabled: bool,
    allow_auto_check: bool,
    prev_payload: Option<Value>,
    mut next_ms: u64,
) -> Option<(Option<Value>, u64, Duration)> {
    let server = weak.upgrade().filter(|s| !s.stopped())?;
    let handoff_active = canban_updater::handoff::active(home);
    let mut emitted_auto = false;
    if !handoff_active {
        let _ = canban_updater::handoff::flush_settled(home);
        let recovered = canban_updater::recover_stale_state(home).unwrap_or(false);
        let mut auto_checked = false;
        let now_ms = server.now_ms();
        if !updates_disabled && allow_auto_check && now_ms >= next_ms {
            let result = canban_updater::feed::check_automatically(home, github.client().ok());
            let delay = if result.is_ok() {
                auto_checked = true;
                canban_updater::load_state(home)
                    .automatic_delay(server.now_ms() / 1000)
                    .max(1)
            } else {
                60
            };
            next_ms = server.now_ms().saturating_add(delay.saturating_mul(1_000));
        }
        if recovered || auto_checked {
            server.emit_update();
            emitted_auto = true;
        }
    }

    let phase = canban_updater::load_state(home).phase;
    let active_op =
        handoff_active || matches!(phase.as_str(), "checking" | "downloading" | "installing");
    if active_op {
        let mut next_payload = prev_payload;
        let payload = server.update_state_payload();
        if next_payload.as_ref() != Some(&payload) {
            next_payload = Some(payload);
            if !emitted_auto {
                server.emit_update();
            }
        }
        return Some((next_payload, next_ms, Duration::from_millis(500)));
    }

    if prev_payload.is_some() && !emitted_auto {
        server.emit_update();
    }
    Some((None, next_ms, Duration::from_secs(1)))
}

async fn update_checks_loop(
    weak: std::sync::Weak<AppServer>,
    updates_disabled: bool,
    home: PathBuf,
    github: Arc<canban_github::auth::Service>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut next_ms = weak.upgrade().map_or(0, |s| s.now_ms());
    let mut last_active_payload: Option<Value> = None;
    let allow_auto_check = !cfg!(test)
        && std::env::var_os("CANBAN_UPDATES_DISABLED").is_none()
        && canban_updater::Build::current().is_release();
    while !*stop_rx.borrow() {
        let step_weak = weak.clone();
        let step_home = home.clone();
        let step_github = github.clone();
        let prev_payload = last_active_payload.clone();
        // A dead service cannot keep checking. The socket probe does not
        // acquire database state or retain an AppServer Arc during I/O.
        let step = tokio::task::spawn_blocking(move || {
            update_check_step(
                &step_weak,
                &step_home,
                &step_github,
                updates_disabled,
                allow_auto_check,
                prev_payload,
                next_ms,
            )
        })
        .await;
        let Ok(Some((next_payload, next_at, sleep_dur))) = step else {
            break;
        };
        last_active_payload = next_payload;
        next_ms = next_at;
        tokio::select! {
            biased;
            res = stop_rx.changed() => {
                if res.is_err() || *stop_rx.borrow() {
                    break;
                }
            }
            _ = tokio::time::sleep(sleep_dur) => {}
        }
    }
}

fn is_local_ui_running(bundle: &Path, home: &Path) -> bool {
    canban_updater::worker::find_local_ui(bundle, home).is_some()
}

fn strip_deleted_suffix(path: &Path) -> PathBuf {
    canban_updater::worker::strip_deleted_suffix(path)
}

/// If `path` points inside a retired/staged backup bundle directory created by an earlier
/// in-place installation swap (e.g. `.canban-install-*/canban-previous/bin/canban` or
/// `.canban-previous-*/bin/canban`), map it to the corresponding binary inside sibling `canban/`
/// (or `Canban.app`).
fn map_retired_bundle_binary(path: &Path) -> Option<PathBuf> {
    canban_updater::worker::map_retired_bundle_binary(path)
}

fn first_systemd_token(raw: &str) -> Option<String> {
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == '\\' && q == '"' {
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '\\' {
            if let Some(next) = chars.next() {
                cur.push(next);
            }
        } else if c.is_whitespace() {
            if !cur.is_empty() {
                return Some(cur);
            }
        } else {
            cur.push(c);
        }
    }
    (!cur.is_empty()).then_some(cur)
}

fn parse_systemd_exec_start_binary(unit_body: &str) -> Option<PathBuf> {
    let mut exec_bin: Option<PathBuf> = None;
    for raw_line in unit_body.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some(rest) = line.strip_prefix("ExecStart=") else {
            continue;
        };
        let rest = rest.trim();
        if rest.is_empty() {
            exec_bin = None;
            continue;
        }
        let rest = rest
            .trim_start_matches(['@', '-', ':', '+', '!'])
            .trim_start();
        if let Some(first) = first_systemd_token(rest)
            && !first.is_empty()
        {
            exec_bin = Some(PathBuf::from(first));
        }
    }
    exec_bin
}

fn systemd_exec_start_cli(home: &Path) -> Option<PathBuf> {
    if !canban_updater::worker::systemd_manages_home(home) {
        return None;
    }
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    let mut candidates = Vec::new();
    if let Some(uh) = &user_home {
        let config_root = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| uh.join(".config"));
        candidates.push(config_root.join("systemd/user/canban.service"));
    }
    if let Some(p) = network::service_status(home)["unit_path"].as_str() {
        let pb = PathBuf::from(p);
        if !candidates.contains(&pb) {
            candidates.push(pb);
        }
    }
    for unit_path in candidates {
        if let Ok(unit_body) = std::fs::read_to_string(&unit_path)
            && let Some(bin) = parse_systemd_exec_start_binary(&unit_body)
        {
            return Some(bin);
        }
    }
    None
}

fn resolve_active_cli_path(home: &Path, cli_path: &Path) -> PathBuf {
    let symlink_target = std::fs::symlink_metadata(cli_path)
        .ok()
        .filter(|m| m.file_type().is_symlink())
        .and_then(|_| std::fs::read_link(cli_path).ok())
        .map(|target| {
            if target.is_absolute() {
                target
            } else {
                cli_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(target)
            }
        });

    let retired_sibling = map_retired_bundle_binary(cli_path).or_else(|| {
        symlink_target
            .as_deref()
            .and_then(map_retired_bundle_binary)
    });

    if retired_sibling.is_none() && cli_path.exists() {
        return cli_path.to_path_buf();
    }

    if let Some(systemd_cli) = systemd_exec_start_cli(home) {
        if let Some(mapped) = map_retired_bundle_binary(&systemd_cli)
            && mapped.exists()
        {
            return mapped;
        }
        if systemd_cli.exists() {
            return systemd_cli;
        }
    }

    if let Some(sibling) = retired_sibling
        && sibling.exists()
    {
        return sibling;
    }

    let clean = strip_deleted_suffix(cli_path);
    if clean != cli_path && clean.exists() {
        return clean;
    }

    cli_path.to_path_buf()
}

fn apply_daemon_update(
    home: &Path,
    cli_path: &Path,
    current: &canban_updater::Build,
    target_version: Option<&str>,
    gh_client: Option<&canban_github::Client>,
    id_gen: &dyn canban_types::IdGen,
) -> Result<Option<(String, u32, PathBuf)>> {
    let active_cli = resolve_active_cli_path(home, cli_path);
    // Coexistence guard: if `active_cli` (which resolves `/proc/self/exe` through `~/.local/bin/canban`
    // symlinks and recovers from deleted/moved `canban-previous` paths!) belongs to a full Canban
    // desktop bundle (`~/.local/opt/canban` or `Canban.app`), NEVER overwrite `bin/canban` alone
    // with a CLI-only tarball, which would break DESKTOP_API parity with sibling `canban-ui` and
    // `canban-updater`.
    if let Ok(target_bundle) = canban_updater::worker::bundle_root(&active_cli) {
        ensure!(
            !is_local_ui_running(&target_bundle, home),
            "A Canban desktop window is currently open on this host ({}). Install the update from the desktop app or close Canban before updating the daemon remotely.",
            target_bundle.display()
        );
        let Some(release) =
            canban_updater::feed::discover_version(current, target_version, gh_client)?
        else {
            return Ok(None);
        };
        let target_ver = release.manifest.version.clone();
        let target_api = release.manifest.api;
        let staged_bundle = canban_updater::feed::stage(home, &release, gh_client)?;
        let token = id_gen.next_token();
        canban_updater::worker::replace_bundle_in_place(&target_bundle, &staged_bundle, &token)?;
        let mut state = canban_updater::load_state(home);
        state.phase = "current".into();
        state.available = None;
        state.progress = 0;
        state.error = None;
        let _ = canban_updater::save_state(home, &state);
        return Ok(Some((
            target_ver,
            target_api,
            canban_updater::bundle_cli(&target_bundle),
        )));
    }

    let resolved_cli = if std::fs::symlink_metadata(&active_cli)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        std::fs::canonicalize(&active_cli).unwrap_or_else(|_| active_cli.clone())
    } else {
        active_cli
    };
    ensure!(
        !resolved_cli.with_file_name("canban-ui").is_file(),
        "Refusing to overwrite {} with a CLI-only archive because sibling desktop binary canban-ui is present; update the full Canban desktop installation instead",
        resolved_cli.display()
    );

    let Some(release) = canban_updater::feed::discover_cli(current, target_version, gh_client)?
    else {
        return Ok(None);
    };
    let target_ver = release.manifest.version.clone();
    let target_api = release.manifest.api;
    let staged_bin = canban_updater::feed::stage_cli(home, &release, gh_client)?;
    let parent = resolved_cli
        .parent()
        .context("Daemon executable parent directory missing")?;
    let tmp_bin = parent.join(format!(".canban-next-{}", id_gen.next_id()));
    std::fs::copy(&staged_bin, &tmp_bin)
        .with_context(|| format!("Could not write updated binary to {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_bin, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp_bin, &resolved_cli).with_context(|| {
        format!(
            "Could not replace daemon binary at {}",
            resolved_cli.display()
        )
    })?;
    let _ = std::fs::remove_dir_all(
        staged_bin
            .parent()
            .unwrap_or_else(|| Path::new("/nonexistent")),
    );
    Ok(Some((target_ver, target_api, resolved_cli)))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn daemon_update_refuses_partial_overwrite_when_unbundled_sibling_ui_exists() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let flat = dir.path().join("flat");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::write(flat.join("canban"), b"v1-cli").unwrap();
        std::fs::write(flat.join("canban-ui"), b"v1-ui").unwrap();

        let err = apply_daemon_update(
            &home,
            &flat.join("canban"),
            &canban_updater::Build::current(),
            None,
            None,
            canban_types::system_id_gen().as_ref(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("sibling desktop binary canban-ui"),
            "unexpected error: {err:#}"
        );
        assert_eq!(std::fs::read(flat.join("canban")).unwrap(), b"v1-cli");
    }

    #[test]
    fn daemon_update_swaps_full_linux_desktop_bundle_via_symlink_and_guards_live_ui() {
        let _env = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let opt = dir.path().join(".local/opt");
        let bin_links = dir.path().join(".local/bin");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(opt.join("canban/bin")).unwrap();
        std::fs::create_dir_all(&bin_links).unwrap();

        std::fs::write(
            opt.join("canban/bundle.json"),
            b"{\"app_id\":\"com.canban.desktop\",\"version\":\"20261115-1\"}\n",
        )
        .unwrap();
        std::fs::write(opt.join("canban/bin/canban"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(
            opt.join("canban/bin/canban-ui"),
            b"#!/bin/sh\nprintf 'ready\\n'\nsleep 30\n",
        )
        .unwrap();
        std::fs::write(
            opt.join("canban/bin/canban-updater"),
            b"#!/bin/sh\nexit 0\n",
        )
        .unwrap();
        for name in ["canban", "canban-ui", "canban-updater"] {
            std::fs::set_permissions(
                opt.join("canban/bin").join(name),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        symlink(opt.join("canban/bin/canban"), bin_links.join("canban")).unwrap();

        // 1. While a local `canban-ui` from this bundle is running for `home`, remote daemon update is refused.
        let mut live_ui = canban_exec::raw_command(opt.join("canban/bin/canban-ui"))
            .env("CANBAN_HOME", &home)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut ready_line = String::new();
        if let Some(stdout) = live_ui.stdout.take() {
            let _ = BufReader::new(stdout).read_line(&mut ready_line);
        }
        let blocked = apply_daemon_update(
            &home,
            &bin_links.join("canban"),
            &canban_updater::Build {
                version: "20261115-1".into(),
                commit: "1111111111111111111111111111111111111111".into(),
                schema: canban_storage::migrations::CURRENT,
                api: API_VERSION,
                platform: "linux-x86_64".into(),
            },
            None,
            None,
            canban_types::system_id_gen().as_ref(),
        );
        let _ = live_ui.kill();
        let _ = live_ui.wait();
        assert!(
            blocked
                .unwrap_err()
                .to_string()
                .contains("Canban desktop window is currently open"),
        );

        // 2. When headless (no UI running), `update.daemon` via `~/.local/bin/canban` symlink
        // downloads `Canban-linux-x86_64.tar.zst` and atomically swaps ALL bundle binaries together!
        let current = canban_updater::Build {
            version: "20261115-1".into(),
            commit: "1111111111111111111111111111111111111111".into(),
            schema: canban_storage::migrations::CURRENT,
            api: API_VERSION,
            platform: "linux-x86_64".into(),
        };
        let next_build = canban_updater::Build {
            version: "20261116-5".into(),
            commit: "2222222222222222222222222222222222222222".into(),
            schema: canban_storage::migrations::CURRENT,
            api: API_VERSION + 1,
            platform: "linux-x86_64".into(),
        };

        let cand_src = dir.path().join("cand-src");
        std::fs::create_dir_all(cand_src.join("bin")).unwrap();
        std::fs::write(
            cand_src.join("bundle.json"),
            serde_json::to_vec_pretty(&json!({
                "app_id": "com.canban.desktop",
                "version": next_build.version,
                "commit": next_build.commit,
            }))
            .unwrap(),
        )
        .unwrap();
        let next_cli_script = format!(
            "#!/bin/sh\nif [ \"$1\" = \"build-info\" ]; then printf '%s\\n' '{}'; exit 0; fi\nprintf 'v2-cli'\n",
            serde_json::to_string(&next_build).unwrap()
        );
        std::fs::write(cand_src.join("bin/canban"), next_cli_script.as_bytes()).unwrap();
        std::fs::write(cand_src.join("bin/canban-ui"), b"#!/bin/sh\n# v2-ui\n").unwrap();
        std::fs::write(
            cand_src.join("bin/canban-updater"),
            b"#!/bin/sh\n# v2-updater\n",
        )
        .unwrap();
        for name in ["canban", "canban-ui", "canban-updater"] {
            std::fs::set_permissions(
                cand_src.join("bin").join(name),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let zst_path = dir.path().join("Canban-linux-x86_64.tar.zst");
        canban_updater::feed::pack(&cand_src, &zst_path).unwrap();
        let zst_bytes = std::fs::read(&zst_path).unwrap();
        let zst_sha = canban_updater::feed::sha256_hex(&zst_bytes);
        let zst_size = zst_bytes.len() as u64;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let manifest_json = serde_json::to_vec(&json!({
            "version": next_build.version,
            "commit": next_build.commit,
            "platform": "linux-x86_64",
            "api": next_build.api,
            "schema": next_build.schema,
            "rollback_floor": 8,
            "asset": "Canban-linux-x86_64.tar.zst",
            "size": zst_size,
            "sha256": zst_sha,
        }))
        .unwrap();
        let releases_json = serde_json::to_vec(&json!([{
            "draft": false,
            "tag_name": next_build.version,
            "body": "notes",
            "assets": [
                {
                    "name": "Canban-linux-x86_64.tar.zst",
                    "size": zst_size,
                    "browser_download_url": format!("http://{addr}/Canban-linux-x86_64.tar.zst"),
                },
                {
                    "name": "manifest-linux-x86_64.json",
                    "size": manifest_json.len(),
                    "browser_download_url": format!("http://{addr}/manifest-linux-x86_64.json"),
                }
            ]
        }]))
        .unwrap();

        let server_thread = std::thread::spawn(move || {
            for _ in 0..3 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let body: &[u8] = if req.contains("manifest-linux-x86_64.json") {
                    &manifest_json
                } else if req.contains("Canban-linux-x86_64.tar.zst") {
                    &zst_bytes
                } else {
                    &releases_json
                };
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(hdr.as_bytes());
                let _ = stream.write_all(body);
            }
        });

        let prev_api = std::env::var_os("CANBAN_UPDATE_API");
        unsafe {
            std::env::set_var("CANBAN_UPDATE_API", format!("http://{addr}"));
        }
        let outcome = apply_daemon_update(
            &home,
            &bin_links.join("canban"),
            &current,
            None,
            None,
            canban_types::system_id_gen().as_ref(),
        );
        match prev_api {
            Some(v) => unsafe { std::env::set_var("CANBAN_UPDATE_API", v) },
            None => unsafe { std::env::remove_var("CANBAN_UPDATE_API") },
        }
        let _ = server_thread.join();

        let (ver, api, restart_cli) = outcome.unwrap().unwrap();
        assert_eq!(ver, "20261116-5");
        assert_eq!(api, API_VERSION + 1);
        assert_eq!(
            restart_cli,
            std::fs::canonicalize(opt.join("canban"))
                .unwrap()
                .join("bin/canban")
        );
        assert_eq!(
            std::fs::read(opt.join("canban/bin/canban-ui")).unwrap(),
            b"#!/bin/sh\n# v2-ui\n"
        );
        assert_eq!(
            std::fs::read(opt.join("canban/bin/canban-updater")).unwrap(),
            b"#!/bin/sh\n# v2-updater\n"
        );
        let symlink_target = std::fs::read_to_string(bin_links.join("canban")).unwrap();
        assert!(symlink_target.contains("v2-cli"));
    }

    #[test]
    fn daemon_update_recovers_live_bundle_when_proc_self_exe_points_to_deleted_or_moved_previous() {
        let _env = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let opt = dir.path().join(".local/opt");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(opt.join("canban/bin")).unwrap();

        std::fs::write(
            opt.join("canban/bundle.json"),
            b"{\"app_id\":\"com.canban.desktop\",\"version\":\"20261115-1\"}\n",
        )
        .unwrap();
        std::fs::write(opt.join("canban/bin/canban"), b"#!/bin/sh\nprintf 'v1'\n").unwrap();
        std::fs::write(opt.join("canban/bin/canban-ui"), b"#!/bin/sh\n# v1-ui\n").unwrap();
        std::fs::write(
            opt.join("canban/bin/canban-updater"),
            b"#!/bin/sh\n# v1-updater\n",
        )
        .unwrap();
        for name in ["canban", "canban-ui", "canban-updater"] {
            std::fs::set_permissions(
                opt.join("canban/bin").join(name),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        // Verify direct path resolution for:
        // 1) Linux `/proc/self/exe` deleted install staging path (`.../.canban-install-xxx/canban-previous/bin/canban (deleted)`)
        let deleted_install_cli =
            opt.join(".canban-install-9a8b7c/canban-previous/bin/canban (deleted)");
        assert_eq!(
            resolve_active_cli_path(&home, &deleted_install_cli),
            opt.join("canban/bin/canban")
        );

        // 2) Moved `.canban-previous-<tok>/bin/canban` even when the backup directory still exists on disk!
        let moved_prev_dir = opt.join(".canban-previous-oldtok");
        std::fs::create_dir_all(moved_prev_dir.join("bin")).unwrap();
        std::fs::write(moved_prev_dir.join("bundle.json"), b"{}\n").unwrap();
        std::fs::write(moved_prev_dir.join("bin/canban"), b"#!/bin/sh\n# old\n").unwrap();
        assert_eq!(
            resolve_active_cli_path(&home, &moved_prev_dir.join("bin/canban")),
            opt.join("canban/bin/canban")
        );

        // 3) Standalone CLI binary replaced in-place (`.../canban (deleted)`)
        let standalone_dir = dir.path().join("standalone");
        std::fs::create_dir_all(&standalone_dir).unwrap();
        std::fs::write(standalone_dir.join("canban"), b"#!/bin/sh\n").unwrap();
        assert_eq!(
            resolve_active_cli_path(&home, &standalone_dir.join("canban (deleted)")),
            standalone_dir.join("canban")
        );

        // 4) systemd `ExecStart=` parser handles quotes, prefixes, and empty resets.
        assert_eq!(
            parse_systemd_exec_start_binary(
                "[Service]\nExecStart=/old/path\nExecStart=\nExecStart=-@\"/home/u/.local/bin/canban\" --home \"/home/u/.canban\" serve\n"
            ),
            Some(PathBuf::from("/home/u/.local/bin/canban"))
        );

        // 5) Full end-to-end `apply_daemon_update` when `cli_path` is an unlinked
        //    `.canban-install-xxx/canban-previous/bin/canban (deleted)` path!
        let current = canban_updater::Build {
            version: "20261115-1".into(),
            commit: "1111111111111111111111111111111111111111".into(),
            schema: canban_storage::migrations::CURRENT,
            api: API_VERSION,
            platform: "linux-x86_64".into(),
        };
        let next_build = canban_updater::Build {
            version: "20261116-9".into(),
            commit: "3333333333333333333333333333333333333333".into(),
            schema: canban_storage::migrations::CURRENT,
            api: API_VERSION + 1,
            platform: "linux-x86_64".into(),
        };

        let cand_src = dir.path().join("cand-src-prev");
        std::fs::create_dir_all(cand_src.join("bin")).unwrap();
        std::fs::write(
            cand_src.join("bundle.json"),
            serde_json::to_vec_pretty(&json!({
                "app_id": "com.canban.desktop",
                "version": next_build.version,
                "commit": next_build.commit,
            }))
            .unwrap(),
        )
        .unwrap();
        let next_cli_script = format!(
            "#!/bin/sh\nif [ \"$1\" = \"build-info\" ]; then printf '%s\\n' '{}'; exit 0; fi\nprintf 'v3-cli'\n",
            serde_json::to_string(&next_build).unwrap()
        );
        std::fs::write(cand_src.join("bin/canban"), next_cli_script.as_bytes()).unwrap();
        std::fs::write(cand_src.join("bin/canban-ui"), b"#!/bin/sh\n# v3-ui\n").unwrap();
        std::fs::write(
            cand_src.join("bin/canban-updater"),
            b"#!/bin/sh\n# v3-updater\n",
        )
        .unwrap();
        for name in ["canban", "canban-ui", "canban-updater"] {
            std::fs::set_permissions(
                cand_src.join("bin").join(name),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let zst_path = dir.path().join("Canban-linux-x86_64.tar.zst");
        canban_updater::feed::pack(&cand_src, &zst_path).unwrap();
        let zst_bytes = std::fs::read(&zst_path).unwrap();
        let zst_sha = canban_updater::feed::sha256_hex(&zst_bytes);
        let zst_size = zst_bytes.len() as u64;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let manifest_json = serde_json::to_vec(&json!({
            "version": next_build.version,
            "commit": next_build.commit,
            "platform": "linux-x86_64",
            "api": next_build.api,
            "schema": next_build.schema,
            "rollback_floor": 8,
            "asset": "Canban-linux-x86_64.tar.zst",
            "size": zst_size,
            "sha256": zst_sha,
        }))
        .unwrap();
        let releases_json = serde_json::to_vec(&json!([{
            "draft": false,
            "tag_name": next_build.version,
            "body": "notes",
            "assets": [
                {
                    "name": "Canban-linux-x86_64.tar.zst",
                    "size": zst_size,
                    "browser_download_url": format!("http://{addr}/Canban-linux-x86_64.tar.zst"),
                },
                {
                    "name": "manifest-linux-x86_64.json",
                    "size": manifest_json.len(),
                    "browser_download_url": format!("http://{addr}/manifest-linux-x86_64.json"),
                }
            ]
        }]))
        .unwrap();

        let server_thread = std::thread::spawn(move || {
            for _ in 0..3 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let body: &[u8] = if req.contains("manifest-linux-x86_64.json") {
                    &manifest_json
                } else if req.contains("Canban-linux-x86_64.tar.zst") {
                    &zst_bytes
                } else {
                    &releases_json
                };
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(hdr.as_bytes());
                let _ = stream.write_all(body);
            }
        });

        let prev_api = std::env::var_os("CANBAN_UPDATE_API");
        unsafe {
            std::env::set_var("CANBAN_UPDATE_API", format!("http://{addr}"));
        }
        let outcome = apply_daemon_update(
            &home,
            &deleted_install_cli,
            &current,
            None,
            None,
            canban_types::system_id_gen().as_ref(),
        );
        match prev_api {
            Some(v) => unsafe { std::env::set_var("CANBAN_UPDATE_API", v) },
            None => unsafe { std::env::remove_var("CANBAN_UPDATE_API") },
        }
        let _ = server_thread.join();

        let (ver, api, restart_cli) = outcome.unwrap().unwrap();
        assert_eq!(ver, "20261116-9");
        assert_eq!(api, API_VERSION + 1);
        assert_eq!(restart_cli, opt.join("canban/bin/canban"));
        assert_eq!(
            std::fs::read(opt.join("canban/bin/canban-ui")).unwrap(),
            b"#!/bin/sh\n# v3-ui\n"
        );
        assert_eq!(
            std::fs::read(opt.join("canban/bin/canban-updater")).unwrap(),
            b"#!/bin/sh\n# v3-updater\n"
        );
        // Moved backup bundle was untouched while live `canban/` bundle was upgraded!
        assert_eq!(
            std::fs::read(moved_prev_dir.join("bin/canban")).unwrap(),
            b"#!/bin/sh\n# old\n"
        );
    }
}
