//! Startup Herdr Core (`hc`) discovery, health check, and official installer lifecycle.
use super::*;
use std::{
    io::{BufRead, BufReader},
    process::Stdio,
};

pub const DEFAULT_INSTALL_URL: &str = "https://releases.herdr.dev/core/install.sh";
pub const DISPLAY_INSTALL_COMMAND: &str =
    "curl -fsSL https://releases.herdr.dev/core/install.sh | sh";
const MAX_INSTALL_LOGS: usize = 60;

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct HerdrInstallState {
    pub phase: String,
    pub logs: Vec<String>,
    pub error: Option<String>,
}

impl Default for HerdrInstallState {
    fn default() -> Self {
        Self {
            phase: "idle".into(),
            logs: Vec::new(),
            error: None,
        }
    }
}

fn push_install_log(
    state: &Arc<Mutex<HerdrInstallState>>,
    server: Option<&std::sync::Weak<AppServer>>,
    line: impl Into<String>,
) {
    let cleaned = strip_ansi(&line.into());
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return;
    }
    {
        let mut guard = state.lock().unwrap();
        guard.logs.push(trimmed.to_owned());
        if guard.logs.len() > MAX_INSTALL_LOGS {
            let drain = guard.logs.len() - MAX_INSTALL_LOGS;
            guard.logs.drain(0..drain);
        }
    }
    if let Some(s) = server.and_then(std::sync::Weak::upgrade) {
        s.emit_herdr_install();
    }
}

fn load_raw_agents_config(home: &Path) -> Result<Config> {
    let path = home.join("config/agents.json");
    let bytes = std::fs::read(&path)?;
    let mut value: Value = serde_json::from_slice(&bytes)?;
    let _ = canban_profiles::migrate(&mut value)?;
    Ok(serde_json::from_value(value)?)
}

impl AppServer {
    pub(super) fn herdr_install_snapshot(&self) -> Value {
        let state = self.herdr_install.lock().unwrap().clone();
        serde_json::to_value(state)
            .unwrap_or_else(|_| json!({"phase":"idle","logs":[],"error":null}))
    }

    pub(super) fn herdr_check(&self, p: Value) -> Result<Value> {
        if p["probe"] == false {
            return Ok(json!({
                "install": self.herdr_install_snapshot(),
            }));
        }
        let raw = self.load_unmigrated_config()?;
        let configured_hc = raw.hc.trim().to_owned();
        let default_hc = matches!(configured_hc.as_str(), "" | "hc" | "herdr-core");
        let lookup_name = if configured_hc.is_empty() {
            "hc"
        } else {
            configured_hc.as_str()
        };
        let resolved_path =
            canban_profiles::find_local_executable_with_env(lookup_name, &raw.hc_env)
                .map(|path| path.to_string_lossy().into_owned());

        let Some(resolved_hc) = resolved_path else {
            let error = if default_hc {
                "Herdr Core (`hc`) was not found in ~/.local/bin, /opt/homebrew/bin, /usr/local/bin, or PATH.".to_owned()
            } else {
                format!(
                    "Configured Herdr Core executable `{configured_hc}` does not exist on this machine."
                )
            };
            return Ok(json!({
                "installed": false,
                "ready": false,
                "configured_hc": if configured_hc.is_empty() { "hc" } else { configured_hc.as_str() },
                "resolved_hc": Value::Null,
                "default_hc": default_hc,
                "install_command": DISPLAY_INSTALL_COMMAND,
                "core_id": Value::Null,
                "version": Value::Null,
                "error": error,
                "install": self.herdr_install_snapshot(),
            }));
        };

        // Refresh in-memory resolved config if `hc` newly resolved on disk so both
        // `Client::status()` and the background observer immediately use the binary.
        if let Ok((next_config, migration)) = Self::read_config(&self.home) {
            let _ = Self::publish_migration(&self.home, migration);
            let changed = {
                let mut active = self.config.write().unwrap();
                let changed = active.hc != next_config.hc
                    || active.hc_session != next_config.hc_session
                    || active.hc_env != next_config.hc_env;
                *active = next_config;
                changed
            };
            if changed {
                self.wake_herdr();
                self.wake_runtime();
                self.emit("configuration", None);
            }
        }

        let mut probe_config = self.config();
        probe_config.hc = resolved_hc.clone();
        match self.herdr_client_with_config(probe_config).status() {
            Ok(status) => {
                self.wake_herdr();
                self.wake_runtime();
                let core_id = status["core"]["id"]
                    .as_str()
                    .or_else(|| status["id"].as_str())
                    .map(str::to_owned);
                let version = status["core"]["version"]
                    .as_str()
                    .or_else(|| status["version"].as_str())
                    .map(str::to_owned);
                Ok(json!({
                    "installed": true,
                    "ready": true,
                    "configured_hc": if configured_hc.is_empty() { "hc" } else { configured_hc.as_str() },
                    "resolved_hc": resolved_hc,
                    "default_hc": default_hc,
                    "install_command": DISPLAY_INSTALL_COMMAND,
                    "core_id": core_id,
                    "version": version,
                    "error": Value::Null,
                    "install": self.herdr_install_snapshot(),
                }))
            }
            Err(err) => Ok(json!({
                "installed": true,
                "ready": false,
                "configured_hc": if configured_hc.is_empty() { "hc" } else { configured_hc.as_str() },
                "resolved_hc": resolved_hc,
                "default_hc": default_hc,
                "install_command": DISPLAY_INSTALL_COMMAND,
                "core_id": Value::Null,
                "version": Value::Null,
                "error": format!("Herdr Core binary was found at `{resolved_hc}`, but failed to respond: {err:#}"),
                "install": self.herdr_install_snapshot(),
            })),
        }
    }

    pub(super) fn herdr_install(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Installing Herdr Core requires an explicit user action"
        );
        let wait = p["wait"] == true;
        let changed = {
            let mut guard = self.herdr_install.lock().unwrap();
            if guard.phase == "installing" {
                if !wait {
                    return Ok(serde_json::to_value(guard.clone())?);
                }
                false
            } else {
                guard.phase = "installing".into();
                guard.logs = vec!["Starting Herdr Core installer…".into()];
                guard.error = None;
                true
            }
        };
        if changed {
            self.emit_herdr_install();
        }

        let home = self.home.clone();
        let install_state = Arc::clone(&self.herdr_install);
        let weak = self.self_weak.get().cloned().unwrap_or_default();
        if wait {
            run_herdr_installer(&home, &install_state, Some(weak));
            return self.herdr_check(json!({}));
        }
        self.spawn_blocking_bg(move || {
            run_herdr_installer(&home, &install_state, Some(weak));
        });
        Ok(self.herdr_install_snapshot())
    }
}

fn notify_install(server: Option<&std::sync::Weak<AppServer>>) {
    if let Some(s) = server.and_then(std::sync::Weak::upgrade) {
        s.emit_herdr_install();
    }
}

fn run_herdr_installer(
    home: &Path,
    install_state: &Arc<Mutex<HerdrInstallState>>,
    server: Option<std::sync::Weak<AppServer>>,
) {
    let raw = match load_raw_agents_config(home) {
        Ok(cfg) => cfg,
        Err(err) => {
            {
                let mut guard = install_state.lock().unwrap();
                guard.phase = "failed".into();
                guard.error = Some(format!("Could not read configuration: {err:#}"));
            }
            notify_install(server.as_ref());
            return;
        }
    };

    let url = raw
        .hc_env
        .get("CANBAN_HERDR_INSTALL_URL")
        .cloned()
        .or_else(|| std::env::var("CANBAN_HERDR_INSTALL_URL").ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_INSTALL_URL.into());

    push_install_log(
        install_state,
        server.as_ref(),
        format!("Downloading installer from {url}"),
    );

    let mut cmd = canban_exec::command("/bin/sh");
    cmd.arg("-c")
        .arg(
            "set -eu; curl -fsSL --connect-timeout 15 --max-time 300 \"$CANBAN_HERDR_INSTALL_SCRIPT_URL\" | sh",
        )
        .env("CANBAN_HERDR_INSTALL_SCRIPT_URL", &url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut path_entries: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default();
    if let Some(user_home) = raw
        .hc_env
        .get("HOME")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("HOME"))
    {
        let local_bin = std::path::PathBuf::from(&user_home).join(".local/bin");
        if !path_entries.contains(&local_bin) {
            path_entries.push(local_bin);
        }
    }
    for sys in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ] {
        let p = std::path::PathBuf::from(sys);
        if !path_entries.contains(&p) {
            path_entries.push(p);
        }
    }
    if let Ok(joined) = std::env::join_paths(path_entries) {
        cmd.env("PATH", joined);
    }
    canban_herdr::apply_default_terminal_env_to_command(&mut cmd, &raw.hc_env);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            {
                let mut guard = install_state.lock().unwrap();
                guard.phase = "failed".into();
                guard.error = Some(format!("Could not start installer process: {err}"));
            }
            notify_install(server.as_ref());
            return;
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let status = std::thread::scope(|scope| {
        if let Some(stdout) = stdout {
            scope.spawn(|| {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    push_install_log(install_state, server.as_ref(), line);
                }
            });
        }
        if let Some(stderr) = stderr {
            scope.spawn(|| {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    push_install_log(install_state, server.as_ref(), line);
                }
            });
        }
        child.wait()
    });

    match status {
        Ok(exit) if exit.success() => {
            push_install_log(
                install_state,
                server.as_ref(),
                "Verifying Herdr Core runtime connection…",
            );
            let configured_hc = raw.hc.trim();
            let lookup_name = if configured_hc.is_empty() {
                "hc"
            } else {
                configured_hc
            };
            let Some(resolved) =
                canban_profiles::find_local_executable_with_env(lookup_name, &raw.hc_env)
            else {
                {
                    let mut guard = install_state.lock().unwrap();
                    guard.phase = "failed".into();
                    guard.error = Some(format!(
                        "Installer finished, but `{lookup_name}` was still not found on disk."
                    ));
                }
                notify_install(server.as_ref());
                return;
            };
            let mut probe_config = raw;
            probe_config.hc = resolved.to_string_lossy().into_owned();
            match canban_herdr::Client::new(probe_config).status() {
                Ok(status_val) => {
                    let version = status_val["core"]["version"]
                        .as_str()
                        .or_else(|| status_val["version"].as_str())
                        .unwrap_or("installed");
                    push_install_log(
                        install_state,
                        server.as_ref(),
                        format!("Herdr Core ready ({version})"),
                    );
                    let endpoint = home.join("app.sock");
                    if endpoint.exists() {
                        let _ = rpc(&endpoint, "herdr.check", json!({}));
                    }
                    {
                        let mut guard = install_state.lock().unwrap();
                        guard.phase = "succeeded".into();
                        guard.error = None;
                    }
                    notify_install(server.as_ref());
                }
                Err(err) => {
                    {
                        let mut guard = install_state.lock().unwrap();
                        guard.phase = "failed".into();
                        guard.error = Some(format!(
                            "Installed `{}` could not start Herdr Core: {err:#}",
                            resolved.display()
                        ));
                    }
                    notify_install(server.as_ref());
                }
            }
        }
        Ok(exit) => {
            let code = exit
                .code()
                .map(|c| format!("exit code {c}"))
                .unwrap_or_else(|| "terminated by signal".into());
            {
                let mut guard = install_state.lock().unwrap();
                let last_line = guard.logs.last().cloned().unwrap_or_default();
                guard.phase = "failed".into();
                guard.error = Some(if last_line.is_empty() {
                    format!("Herdr Core installer failed ({code}).")
                } else {
                    format!("Herdr Core installer failed ({code}): {last_line}")
                });
            }
            notify_install(server.as_ref());
        }
        Err(err) => {
            {
                let mut guard = install_state.lock().unwrap();
                guard.phase = "failed".into();
                guard.error = Some(format!("Installer wait error: {err}"));
            }
            notify_install(server.as_ref());
        }
    }
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        if ch != '\r' {
            out.push(ch);
        }
    }
    out
}
