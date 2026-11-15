use anyhow::{Context, Result, ensure};
use canban_types::Config;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    time::Duration,
};

/// Default Ghostty terminal identity injected when starting or launching Herdr PTYs
/// unless explicitly overridden by caller `env` or configured `hc_env`.
pub const DEFAULT_TERMINAL_IDENTITY_ENV: [(&str, &str); 3] = [
    ("TERM", "xterm-256color"),
    ("TERM_PROGRAM", "ghostty"),
    ("COLORTERM", "truecolor"),
];

/// Merge default Ghostty terminal identity + `hc_env` defaults into a launch environment map
/// without overwriting any key already set by the caller (`env` > `hc_env` > defaults).
pub fn merge_default_terminal_env(
    env: &mut BTreeMap<String, String>,
    hc_env: &BTreeMap<String, String>,
) {
    for (k, default_v) in DEFAULT_TERMINAL_IDENTITY_ENV {
        if !env.contains_key(k) {
            let v = hc_env
                .get(k)
                .filter(|s| !s.is_empty())
                .map(String::as_str)
                .unwrap_or(default_v);
            env.insert(k.to_owned(), v.to_owned());
        }
    }
}

/// Apply default Ghostty terminal identity + `hc_env` onto a spawned `Command`
/// so GUI-launched processes (`Finder` / `LaunchServices`) advertise Ghostty consistently
/// while still honoring explicit `hc_env` overrides.
pub fn apply_default_terminal_env_to_command(cmd: &mut Command, hc_env: &BTreeMap<String, String>) {
    for (k, default_v) in DEFAULT_TERMINAL_IDENTITY_ENV {
        if !hc_env.get(k).is_some_and(|v| !v.is_empty()) {
            cmd.env(k, default_v);
        }
    }
    for (k, v) in hc_env {
        let is_empty_identity =
            v.is_empty() && DEFAULT_TERMINAL_IDENTITY_ENV.iter().any(|(ik, _)| *ik == k);
        if !is_empty_identity {
            cmd.env(k, v);
        }
    }
}

#[derive(Clone)]
pub struct Client {
    pub config: Config,
}
/// Retain runtime facts, not argv/env copies of the potentially very long prompt.
pub fn observation(value: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in [
        "address",
        "state",
        "status",
        "status_ms",
        "exit_code",
        "signal",
        "reason",
        "pid",
        "turn",
        "cwd",
        "finished_ms",
    ] {
        if let Some(v) = value.get(key) {
            out.insert(key.into(), v.clone());
        }
    }
    if let Some(cwd) = value["foreground"]["cwd"].as_str() {
        out.insert("foreground".into(), json!({"cwd":cwd}));
    }
    Value::Object(out)
}

/// Observed foreground directory, with the Core's terminal directory fallback.
pub fn cwd(value: &Value) -> Option<&str> {
    value["foreground"]["cwd"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| value["cwd"].as_str().filter(|s| !s.is_empty()))
}
impl Client {
    /// hc connect speaks the public JSON-RPC protocol and resolves the installed
    /// core's socket/config without linking Herdr's private implementation.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        pool::call(&self.config, method, params)
    }
    pub fn status(&self) -> Result<Value> {
        self.call("server.status", json!({}))
    }
    pub fn launch(
        &self,
        argv: Vec<String>,
        cwd: &str,
        env: BTreeMap<String, String>,
        labels: BTreeMap<String, String>,
    ) -> Result<Value> {
        self.launch_on("local", argv, cwd, env, labels)
    }
    pub fn launch_on(
        &self,
        machine: &str,
        argv: Vec<String>,
        cwd: &str,
        mut env: BTreeMap<String, String>,
        labels: BTreeMap<String, String>,
    ) -> Result<Value> {
        ensure!(!argv.is_empty(), "Agent profile has no launch command");
        merge_default_terminal_env(&mut env, &self.config.hc_env);
        let mut p = json!({"argv":argv,"cwd":cwd,"env":env,"labels":labels,"no_parent":true,"no_notify":true,"outlive_parent":true,"wait":false});
        if machine != "local" {
            p["on"] = json!(machine);
        }
        let mut result = self.call("terminal.run", p)?;
        // Stable owner prefix survives route rename and cannot follow a reused alias.
        if machine != "local" {
            let address = result["address"].as_str().context("No terminal address")?;
            let local = address.rsplit('/').next().unwrap();
            result["address"] = json!(format!("{machine}/{local}"));
        }
        Ok(result)
    }
    pub fn terminals(&self) -> Result<Value> {
        let mut list = self.call("terminal.list", json!({"all":true}))?;
        let machines = self
            .call("machine.list", json!({}))
            .unwrap_or(json!({"machines":[]}));
        if let Some(rows) = list["terminals"].as_array_mut() {
            for row in rows {
                if let Some((prefix, local)) =
                    row["address"].as_str().and_then(|v| v.split_once('/'))
                    && let Some(m) = machines["machines"]
                        .as_array()
                        .and_then(|ms| ms.iter().find(|m| m["name"] == prefix))
                {
                    row["address"] = json!(format!(
                        "{}/{local}",
                        m["id"].as_str().context("No owner ID")?
                    ));
                }
            }
        }
        Ok(list)
    }
    pub fn get(&self, terminal: &str) -> Result<Value> {
        self.call("terminal.get", json!({"terminal":terminal}))
    }
    pub fn read(&self, terminal: &str) -> Result<Value> {
        self.call("terminal.read", json!({"terminal":terminal,"lines":160}))
    }
    pub fn read_screen(&self, terminal: &str) -> Result<Value> {
        self.call("terminal.read", json!({"terminal":terminal,"lines":0}))
    }
}

pub mod activity;
pub mod events;
pub mod host;
mod pool;
pub mod refine;
pub mod stream;
pub mod transport;

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn cmd_env_map(cmd: &Command) -> BTreeMap<String, Option<String>> {
        cmd.get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|val: &OsStr| val.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn default_ghostty_identity_is_injected_and_respects_hc_env_and_caller_overrides() {
        // 1. Empty env + empty hc_env -> defaults injected.
        let mut env = BTreeMap::new();
        let hc_env = BTreeMap::new();
        merge_default_terminal_env(&mut env, &hc_env);
        assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
        assert_eq!(env.get("TERM_PROGRAM").map(String::as_str), Some("ghostty"));
        assert_eq!(env.get("COLORTERM").map(String::as_str), Some("truecolor"));

        // 2. hc_env overrides defaults when non-empty, while empty hc_env entries fall back to defaults,
        //    and explicit caller env overrides both hc_env and defaults.
        let mut caller_env = BTreeMap::from([("TERM_PROGRAM".into(), "kitty".into())]);
        let custom_hc_env = BTreeMap::from([
            ("TERM".into(), "xterm-ghostty".into()),
            ("TERM_PROGRAM".into(), "wezterm".into()),
            ("COLORTERM".into(), String::new()),
            ("CUSTOM_FLAG".into(), "1".into()),
        ]);
        merge_default_terminal_env(&mut caller_env, &custom_hc_env);
        assert_eq!(
            caller_env.get("TERM").map(String::as_str),
            Some("xterm-ghostty")
        );
        assert_eq!(
            caller_env.get("TERM_PROGRAM").map(String::as_str),
            Some("kitty")
        );
        assert_eq!(
            caller_env.get("COLORTERM").map(String::as_str),
            Some("truecolor")
        );

        // 3. transport::command injects default Ghostty identity for GUI-launched hc processes
        //    and respects non-empty config.hc_env overrides.
        let default_cfg = Config::default();
        let default_cmd_envs = cmd_env_map(&transport::command(&default_cfg));
        assert_eq!(
            default_cmd_envs.get("TERM"),
            Some(&Some("xterm-256color".into()))
        );
        assert_eq!(
            default_cmd_envs.get("TERM_PROGRAM"),
            Some(&Some("ghostty".into()))
        );
        assert_eq!(
            default_cmd_envs.get("COLORTERM"),
            Some(&Some("truecolor".into()))
        );

        let override_cfg = Config {
            hc_env: custom_hc_env,
            ..Config::default()
        };
        let override_cmd_envs = cmd_env_map(&transport::command(&override_cfg));
        assert_eq!(
            override_cmd_envs.get("TERM"),
            Some(&Some("xterm-ghostty".into()))
        );
        assert_eq!(
            override_cmd_envs.get("TERM_PROGRAM"),
            Some(&Some("wezterm".into()))
        );
        assert_eq!(
            override_cmd_envs.get("COLORTERM"),
            Some(&Some("truecolor".into()))
        );
        assert_eq!(
            override_cmd_envs.get("CUSTOM_FLAG"),
            Some(&Some("1".into()))
        );
    }
}
