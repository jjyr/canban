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
                let home = self.home.clone();
                let Some((target_ver, target_api, cli_for_restart)) = apply_daemon_update(
                    &home,
                    &self.cli,
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
                drop(operation);

                // Schedule graceful daemon restart right after flushing the RPC response so
                // `systemd --user` / `launchd` (`Restart=always` / `KeepAlive`) or standalone
                // `canban serve` boots the newly installed binary cleanly.
                let stop_flag_home = self.home.clone();
                let expected_pid = std::process::id();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(120));
                    if canban_updater::worker::systemd_manages_home(&stop_flag_home)
                        && std::process::Command::new("systemctl")
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

fn is_local_ui_running(bundle: &Path, home: &Path) -> bool {
    let ui_bin = canban_updater::bundle_ui(bundle);
    let ui_canon = std::fs::canonicalize(&ui_bin).unwrap_or_else(|_| ui_bin.clone());
    let home_canon = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());

    #[cfg(target_os = "linux")]
    {
        let default_home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|h| h.join(".canban"))
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p));
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return false;
        };
        for entry in entries.flatten() {
            let pid_os = entry.file_name();
            let Some(pid_str) = pid_os.to_str() else {
                continue;
            };
            if !pid_str.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid_str}/stat"))
                && let Some(rparen) = stat.rfind(')')
                && stat[rparen + 1..].trim_start().starts_with('Z')
            {
                continue;
            }
            let exe_matches = std::fs::read_link(format!("/proc/{pid_str}/exe"))
                .ok()
                .is_some_and(|exe| {
                    let raw = exe.to_string_lossy();
                    let clean = Path::new(raw.strip_suffix(" (deleted)").unwrap_or(&raw));
                    clean == ui_bin || clean == ui_canon
                });
            let cmdline_matches = !exe_matches
                && std::fs::read(format!("/proc/{pid_str}/cmdline"))
                    .ok()
                    .is_some_and(|bytes| {
                        bytes.split(|&b| b == 0).any(|arg| {
                            !arg.is_empty()
                                && std::str::from_utf8(arg).ok().is_some_and(|s| {
                                    let p = Path::new(s);
                                    p == ui_bin || p == ui_canon
                                })
                        })
                    });
            if !exe_matches && !cmdline_matches {
                continue;
            }
            if let Ok(env_bytes) = std::fs::read(format!("/proc/{pid_str}/environ")) {
                let mut proc_home: Option<PathBuf> = None;
                for kv in env_bytes.split(|&b| b == 0) {
                    if let Ok(s) = std::str::from_utf8(kv)
                        && let Some(val) = s.strip_prefix("CANBAN_HOME=")
                        && !val.is_empty()
                    {
                        let p = PathBuf::from(val);
                        proc_home = Some(std::fs::canonicalize(&p).unwrap_or(p));
                        break;
                    }
                }
                match proc_home {
                    Some(ph) if ph == home_canon => return true,
                    Some(_) => continue,
                    None if default_home.as_ref() == Some(&home_canon) => return true,
                    None => continue,
                }
            }
            return true;
        }
        false
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = home_canon;
        let ui_str = ui_bin.to_string_lossy();
        let ui_canon_str = ui_canon.to_string_lossy();
        let Ok(out) = std::process::Command::new("ps")
            .args(["-ww", "-x", "-o", "state=,command="])
            .output()
        else {
            return false;
        };
        if !out.status.success() {
            return false;
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('Z') {
                continue;
            }
            let Some((_, cmd)) = trimmed.split_once(char::is_whitespace) else {
                continue;
            };
            let cmd = cmd.trim();
            if !cmd.contains("canban-ui") {
                continue;
            }
            if cmd == ui_str
                || cmd == ui_canon_str
                || cmd.starts_with(&format!("{ui_str} "))
                || cmd.starts_with(&format!("{ui_canon_str} "))
                || (Path::new(cmd).exists()
                    && std::fs::canonicalize(cmd).ok().as_deref() == Some(ui_canon.as_path()))
                || cmd.split_whitespace().any(|tok| {
                    tok == ui_str
                        || tok == ui_canon_str
                        || (tok.ends_with("/canban-ui")
                            && std::fs::canonicalize(tok).ok().as_deref()
                                == Some(ui_canon.as_path()))
                })
            {
                return true;
            }
        }
        false
    }
}

fn apply_daemon_update(
    home: &Path,
    cli_path: &Path,
    current: &canban_updater::Build,
    target_version: Option<&str>,
    gh_client: Option<&canban_github::Client>,
) -> Result<Option<(String, u32, PathBuf)>> {
    // Coexistence guard: if `cli_path` (which resolves `/proc/self/exe` through `~/.local/bin/canban`
    // symlinks!) belongs to a full Canban desktop bundle (`~/.local/opt/canban` or `Canban.app`),
    // NEVER overwrite `bin/canban` alone with a CLI-only tarball, which would break DESKTOP_API
    // parity with sibling `canban-ui` and `canban-updater`.
    if let Ok(target_bundle) = canban_updater::worker::bundle_root(cli_path) {
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
        let token = canban_types::token_id();
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

    let resolved_cli = if std::fs::symlink_metadata(cli_path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        std::fs::canonicalize(cli_path).unwrap_or_else(|_| cli_path.to_path_buf())
    } else {
        cli_path.to_path_buf()
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
    let tmp_bin = parent.join(format!(".canban-next-{}", canban_types::id()));
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
        let mut live_ui = std::process::Command::new(opt.join("canban/bin/canban-ui"))
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
        let outcome = apply_daemon_update(&home, &bin_links.join("canban"), &current, None, None);
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
}
