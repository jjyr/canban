use anyhow::{Context, Result, ensure};
use canban_types::Config;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    time::Duration,
};

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
        env: BTreeMap<String, String>,
        labels: BTreeMap<String, String>,
    ) -> Result<Value> {
        ensure!(!argv.is_empty(), "Agent profile has no launch command");
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
}

pub mod activity;
pub mod events;
pub mod host;
mod pool;
pub mod stream;
pub mod transport;
