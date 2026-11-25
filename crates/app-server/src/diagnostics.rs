//! End-to-end server, runtime, toolchain, storage, and log diagnostics RPCs.
use super::*;

fn probe_tool(
    name: &str,
    version_arg: &str,
    env_map: &std::collections::BTreeMap<String, String>,
    runner: &dyn canban_exec::CommandRunner,
) -> Value {
    let Some(resolved) = canban_profiles::find_local_executable_with_runner(name, env_map, runner)
    else {
        return json!({
            "available": false,
            "path": Value::Null,
            "version": Value::Null,
            "error": format!("{name} not found in ~/.local/bin, /opt/homebrew/bin, /usr/local/bin, or PATH"),
        });
    };
    let mut spec = canban_exec::CommandSpec::new(resolved.to_string_lossy()).arg(version_arg);
    for (k, v) in env_map {
        spec = spec.env(k, v);
    }
    let version = runner.run(&spec).ok().and_then(|out| {
        let text = if out.stdout.is_empty() {
            String::from_utf8_lossy(&out.stderr).into_owned()
        } else {
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        text.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_owned)
    });
    json!({
        "available": true,
        "path": resolved.to_string_lossy(),
        "version": version,
        "error": Value::Null,
    })
}

fn tail_file(path: &Path, max_lines: usize, max_bytes: u64) -> Value {
    if !path.exists() {
        return json!({
            "exists": false,
            "path": path,
            "size_bytes": 0,
            "lines": [],
        });
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return json!({
            "exists": false,
            "path": path,
            "size_bytes": 0,
            "lines": [],
        });
    };
    let size = meta.len();
    let Ok(mut file) = std::fs::File::open(path) else {
        return json!({
            "exists": true,
            "path": path,
            "size_bytes": size,
            "lines": [],
        });
    };
    use std::io::{Seek, SeekFrom};
    if size > max_bytes {
        let _ = file.seek(SeekFrom::Start(size - max_bytes));
    }
    let mut buf = Vec::new();
    let _ = file.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let mut all_lines: Vec<&str> = text.lines().collect();
    if size > max_bytes && !all_lines.is_empty() {
        // Drop partial leading line after byte seek.
        all_lines.remove(0);
    }
    let start = all_lines.len().saturating_sub(max_lines);
    let kept: Vec<String> = all_lines[start..].iter().map(|s| (*s).to_owned()).collect();
    let joined = kept.join("\n");
    json!({
        "exists": true,
        "path": path,
        "size_bytes": size,
        "lines": kept,
        "text": joined,
    })
}

impl AppServer {
    pub(super) fn diagnostics_call(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "diagnostics.status" => self.diagnostics_status(),
            "diagnostics.logs" => self.diagnostics_logs(&p),
            _ => bail!("Unknown diagnostics operation: {method}"),
        }
    }

    pub(super) fn diagnostics_status(&self) -> Result<Value> {
        if self.exec_runner.is_system() {
            canban_herdr::refresh_local_shell_path_if_stale(std::time::Duration::from_secs(2));
        }
        let config = self.config();
        let build = canban_updater::Build::current();
        let uptime_ms = self.now_ms().saturating_sub(self.started_at);
        let listen_cfg = network::load_listen_config(&self.home);
        let bound_tcp = self.bound_tcp.lock().unwrap().clone();
        let has_token = network::read_server_token(&self.home).is_some();
        let paired_devices_count = self.device_store.list().len();

        let db_path = self.home.join("canban.sqlite");
        let wal_path = self.home.join("canban.sqlite-wal");
        let db_size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
        let wal_size = std::fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);

        let (project_count, active_cards, active_terminals, journal_mode, db_healthy) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let projects = db.list::<Project>().map(|v| v.len()).unwrap_or(0);
            let cards = db
                .list::<Card>()
                .map(|v| v.into_iter().filter(|c| c.active()).count())
                .unwrap_or(0);
            let terms = db
                .list::<TerminalEntry>()
                .map(|v| {
                    v.into_iter()
                        .filter(|t| {
                            t.runtime.as_ref().is_some_and(|r| r.state != "finished")
                                || t.agent.as_ref().is_some_and(|a| !a.finished())
                        })
                        .count()
                })
                .unwrap_or(0);
            let jmode: String = db
                .conn
                .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                .unwrap_or_else(|_| "unknown".into());
            let quick: String = db
                .conn
                .query_row("PRAGMA quick_check(1)", [], |r| r.get(0))
                .unwrap_or_else(|e| e.to_string());
            (
                projects,
                cards,
                terms,
                jmode,
                quick.eq_ignore_ascii_case("ok"),
            )
        };

        let herdr = self.herdr_check(json!({"probe": true})).unwrap_or_else(
            |e| json!({"installed": false, "ready": false, "error": e.to_string()}),
        );
        let (core_id, obs_connected, obs_terminals) = {
            let inv = self.inventory.lock().unwrap();
            (
                inv.core.clone(),
                inv.error.is_none() && inv.last_at > 0,
                inv.records.len(),
            )
        };

        let git = probe_tool(
            "git",
            "--version",
            &config.hc_env,
            self.exec_runner.as_ref(),
        );
        let python3 = probe_tool(
            "python3",
            "--version",
            &config.hc_env,
            self.exec_runner.as_ref(),
        );
        let tailscale_ip = network::detect_tailscale_ipv4().ok();
        let service_mgr = network::service_status(&self.home);

        let mut profile_checks = Vec::new();
        for prof in config.profiles.iter().filter(|pr| pr.enabled) {
            let cmd_name = prof
                .command()
                .first()
                .map(String::as_str)
                .unwrap_or_default()
                .to_owned();
            let resolved = if cmd_name.is_empty() {
                None
            } else {
                canban_profiles::find_local_executable_with_runner(
                    &cmd_name,
                    &config.hc_env,
                    self.exec_runner.as_ref(),
                )
            };
            profile_checks.push(json!({
                "id": prof.id,
                "identity": prof.is_identity(),
                "command": cmd_name,
                "available": resolved.is_some(),
                "resolved_path": resolved.map(|p| p.to_string_lossy().into_owned()),
            }));
        }

        let gh = self.github.status();

        Ok(json!({
            "checked_at": self.now_ms(),
            "server": {
                "server_id": self.identity,
                "hostname": network::hostname_with(self.exec_runner.as_ref()),
                "pid": std::process::id(),
                "uptime_ms": uptime_ms,
                "home": self.home,
                "socket": self.endpoint(),
                "listen_config": listen_cfg,
                "bound_tcp": bound_tcp,
                "auth_token_configured": has_token || paired_devices_count > 0,
                "legacy_token_configured": has_token,
                "paired_device_count": paired_devices_count,
                "api_version": API_VERSION,
                "build": build,
            },
            "storage": {
                "healthy": db_healthy,
                "journal_mode": journal_mode,
                "db_path": db_path,
                "db_size_bytes": db_size,
                "wal_size_bytes": wal_size,
                "project_count": project_count,
                "active_card_count": active_cards,
                "active_terminal_count": active_terminals,
            },
            "herdr": {
                "check": herdr,
                "core_id": core_id,
                "observer_connected": obs_connected,
                "observed_terminals": obs_terminals,
                "session": config.hc_session,
            },
            "toolchain": {
                "git": git,
                "python3": python3,
                "tailscale": {
                    "available": tailscale_ip.is_some(),
                    "ipv4": tailscale_ip,
                },
                "service_manager": service_mgr,
                "profiles": profile_checks,
            },
            "github": {
                "state": gh.state,
                "login": gh.login,
                "error": gh.error,
            },
        }))
    }

    pub(super) fn diagnostics_logs(&self, p: &Value) -> Result<Value> {
        let source = p["source"].as_str().unwrap_or("server");
        let lines = p["lines"].as_u64().unwrap_or(160).clamp(1, 2000) as usize;
        let path = match source {
            "server" => self.home.join("server.log"),
            "updater" => self.home.join("updates/updater.log"),
            other => bail!("Unsupported log source: {other}"),
        };
        Ok(tail_file(&path, lines, 256 * 1024))
    }

    pub(super) fn host_probe_payload(&self) -> Value {
        json!({
            "server_id": self.identity,
            "hostname": network::hostname_with(self.exec_runner.as_ref()),
            "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            "version": env!("CARGO_PKG_VERSION"),
            "api_version": API_VERSION,
            "build": canban_updater::Build::current(),
            "auth_required": true,
        })
    }
}
