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
    Value::Object(out)
}
impl Client {
    /// hc connect speaks the public JSON-RPC protocol and resolves the installed
    /// core's socket/config without linking Herdr's private implementation.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let mut child = Command::new(&self.config.hc)
            .arg("connect")
            .env("HC_SESSION", &self.config.hc_session)
            .envs(&self.config.hc_env)
            .env_remove("HC_TERMINAL_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Could not start hc connect; configure the installed hc executable")?;
        let mut input = child.stdin.take().unwrap();
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","id":1,"method":"hello","params":{"generation":1}})
        )?;
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","id":2,"method":method,"params":params})
        )?;
        input.flush()?;
        let output = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Herdr disconnected before replying")));
                        break;
                    }
                    Ok(_) => {
                        if let Ok(value) = serde_json::from_str::<Value>(&line)
                            && value["id"] == 2
                        {
                            let _ = tx.send(Ok(value));
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e.into()));
                        break;
                    }
                }
            }
        });
        let reply = rx.recv_timeout(Duration::from_secs(if method == "terminal.run" {
            45
        } else {
            12
        }));
        drop(input);
        let _ = child.kill();
        let _ = child.wait();
        let reply = reply.context("Herdr reply timed out; launch effects may be unknown")??;
        ensure!(
            reply.get("error").is_none(),
            "Herdr {method}: {}",
            reply["error"]
        );
        reply
            .get("result")
            .cloned()
            .context("Herdr reply has no result")
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
        ensure!(!argv.is_empty(), "Agent profile has no launch command");
        let result=self.call("terminal.run",json!({"argv":argv,"cwd":cwd,"env":env,"labels":labels,"no_parent":true,"no_notify":true,"outlive_parent":true,"wait":false}))?;
        Ok(result)
    }
    pub fn get(&self, terminal: &str) -> Result<Value> {
        self.call("terminal.get", json!({"terminal":terminal}))
    }
    pub fn read(&self, terminal: &str) -> Result<Value> {
        self.call("terminal.read", json!({"terminal":terminal,"lines":160}))
    }
}

pub mod stream;
