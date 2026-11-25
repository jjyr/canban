use anyhow::{Context, Result, ensure};
use canban_types::Config;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};

pub(crate) const MAX_JSON_FRAME_BYTES: u64 = 48 * 1024 * 1024;

pub(crate) fn write_json_line<W: Write + ?Sized>(
    writer: &mut W,
    value: &Value,
) -> std::io::Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

pub(crate) fn read_bounded_json_line_into(
    reader: &mut impl BufRead,
    line: &mut String,
    max_bytes: u64,
) -> std::io::Result<()> {
    line.clear();
    let n = reader.by_ref().take(max_bytes + 1).read_line(line)?;
    if line.len() as u64 > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Herdr frame too large",
        ));
    }
    if n == 0 || !line.ends_with('\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "Herdr disconnected before completing frame",
        ));
    }
    Ok(())
}

pub(crate) fn read_bounded_json_line(
    reader: &mut impl BufRead,
    max_bytes: u64,
) -> std::io::Result<String> {
    let mut line = String::new();
    read_bounded_json_line_into(reader, &mut line, max_bytes)?;
    Ok(line)
}

/// Default Ghostty terminal identity injected when starting or launching Herdr PTYs
/// unless explicitly overridden by caller `env` or configured `hc_env`.
pub const DEFAULT_TERMINAL_IDENTITY_ENV: [(&str, &str); 3] = [
    ("TERM", "xterm-256color"),
    ("TERM_PROGRAM", "ghostty"),
    ("COLORTERM", "truecolor"),
];

/// Merge default Ghostty terminal identity + `hc_env` defaults into a launch environment map
/// without overwriting any key already set by the caller (`env` > `hc_env` > defaults).
///
/// `PATH` in `hc_env` is intentionally deferred to `merge_local_path_env` so remote-machine
/// launches do not receive the local host's `PATH`.
pub fn merge_default_terminal_env(
    env: &mut BTreeMap<String, String>,
    hc_env: &BTreeMap<String, String>,
) {
    for (k, v) in hc_env {
        if k == "PATH" || env.contains_key(k) {
            continue;
        }
        let is_empty_identity =
            v.is_empty() && DEFAULT_TERMINAL_IDENTITY_ENV.iter().any(|(ik, _)| *ik == k);
        if !is_empty_identity {
            env.insert(k.clone(), v.clone());
        }
    }
    for (k, default_v) in DEFAULT_TERMINAL_IDENTITY_ENV {
        if !env.contains_key(k) {
            env.insert(k.to_owned(), default_v.to_owned());
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

impl TerminalSize {
    pub const MIN_COLS: u16 = 20;
    pub const MAX_COLS: u16 = 500;
    pub const MIN_ROWS: u16 = 5;
    pub const MAX_ROWS: u16 = 200;

    pub fn normalized(cols: u16, rows: u16) -> Option<Self> {
        if cols < 2 || rows < 1 {
            return None;
        }
        Some(Self {
            cols: cols.clamp(Self::MIN_COLS, Self::MAX_COLS),
            rows: rows.clamp(Self::MIN_ROWS, Self::MAX_ROWS),
        })
    }
}

/// Control-plane subscription returned by an injectable [`HerdrBackend`].
pub struct BackendSubscription {
    pub core: String,
    pub snapshot: Value,
    pub receiver: std::sync::mpsc::Receiver<Result<Value>>,
}

/// Injectable Herdr Core backend seam for deterministic simulation testing.
pub trait HerdrBackend: Send + Sync + 'static {
    fn call(&self, method: &str, params: Value) -> Result<Value>;
    fn subscribe(&self, wake: Arc<dyn Fn() + Send + Sync>) -> Result<BackendSubscription>;
    fn activity(&self, core: &str, terminal: &str) -> crate::activity::Activity;
}

#[derive(Clone)]
pub struct Client {
    pub config: Config,
    pub backend: Option<Arc<dyn HerdrBackend>>,
    runner: Arc<dyn canban_exec::CommandRunner>,
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
/// Merge local host environment into `env` according to `policy` using an explicit
/// [`canban_exec::CommandRunner`]. Production [`canban_exec::SystemRunner`] delegates
/// unchanged to [`merge_local_env_with_policy`]; non-system runners (such as `SimExec` in DST)
/// read purely from `runner.shell_env()` without spawning a login shell probe.
pub fn merge_local_env_with_policy_with_runner(
    env: &mut BTreeMap<String, String>,
    hc_env: &BTreeMap<String, String>,
    policy: LocalEnvPolicy,
    runner: &dyn canban_exec::CommandRunner,
) {
    if runner.is_system() {
        merge_local_env_with_policy(env, hc_env, policy);
        return;
    }
    let snapshot = runner.shell_env();
    if policy == LocalEnvPolicy::InheritHostShell
        && !hc_env.contains_key("CANBAN_HERDR_SEARCH_PATHS")
    {
        merge_shell_env_snapshot(env, hc_env, &snapshot);
    }
    if !env.get("PATH").is_some_and(|v| !v.is_empty())
        && !hc_env.contains_key("CANBAN_HERDR_SEARCH_PATHS")
        && let Some(path) = canban_exec::merged_path_string_from_snapshot(&snapshot, hc_env)
    {
        env.insert("PATH".into(), path);
    }
}

impl Client {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            backend: None,
            runner: canban_exec::system_runner(),
        }
    }
    pub fn with_backend(config: Config, backend: Option<Arc<dyn HerdrBackend>>) -> Self {
        Self {
            config,
            backend,
            runner: canban_exec::system_runner(),
        }
    }
    pub fn with_backend_and_runner(
        config: Config,
        backend: Option<Arc<dyn HerdrBackend>>,
        runner: Arc<dyn canban_exec::CommandRunner>,
    ) -> Self {
        Self {
            config,
            backend,
            runner,
        }
    }
    pub fn with_runner(mut self, runner: Arc<dyn canban_exec::CommandRunner>) -> Self {
        self.runner = runner;
        self
    }
    pub fn runner(&self) -> &Arc<dyn canban_exec::CommandRunner> {
        &self.runner
    }
    /// hc connect speaks the public JSON-RPC protocol and resolves the installed
    /// core's socket/config without linking Herdr's private implementation.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        if let Some(backend) = &self.backend {
            return backend.call(method, params);
        }
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
        self.launch_with_policy(argv, cwd, env, labels, LocalEnvPolicy::MinimalPathOnly)
    }
    pub fn launch_with_policy(
        &self,
        argv: Vec<String>,
        cwd: &str,
        env: BTreeMap<String, String>,
        labels: BTreeMap<String, String>,
        env_policy: LocalEnvPolicy,
    ) -> Result<Value> {
        self.launch_with_policy_and_unset(argv, cwd, env, labels, env_policy, &BTreeSet::new())
    }
    pub fn launch_with_policy_and_unset(
        &self,
        argv: Vec<String>,
        cwd: &str,
        env: BTreeMap<String, String>,
        labels: BTreeMap<String, String>,
        env_policy: LocalEnvPolicy,
        unset_keys: &BTreeSet<String>,
    ) -> Result<Value> {
        self.launch_with_policy_unset_and_size(argv, cwd, env, labels, env_policy, unset_keys, None)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn launch_with_policy_unset_and_size(
        &self,
        argv: Vec<String>,
        cwd: &str,
        env: BTreeMap<String, String>,
        labels: BTreeMap<String, String>,
        env_policy: LocalEnvPolicy,
        unset_keys: &BTreeSet<String>,
        size: Option<TerminalSize>,
    ) -> Result<Value> {
        self.launch_with_policy_unset_size_and_runner(
            argv,
            cwd,
            env,
            labels,
            env_policy,
            unset_keys,
            size,
            self.runner.as_ref(),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn launch_with_policy_unset_size_and_runner(
        &self,
        argv: Vec<String>,
        cwd: &str,
        mut env: BTreeMap<String, String>,
        labels: BTreeMap<String, String>,
        env_policy: LocalEnvPolicy,
        unset_keys: &BTreeSet<String>,
        size: Option<TerminalSize>,
        runner: &dyn canban_exec::CommandRunner,
    ) -> Result<Value> {
        ensure!(!argv.is_empty(), "Agent profile has no launch command");
        merge_default_terminal_env(&mut env, &self.config.hc_env);
        merge_local_env_with_policy_with_runner(&mut env, &self.config.hc_env, env_policy, runner);
        for k in unset_keys {
            env.remove(k);
        }
        let mut p = json!({"argv":argv,"cwd":cwd,"env":env,"labels":labels,"no_parent":true,"no_notify":true,"outlive_parent":true,"wait":false});
        if let Some(sz) = size {
            p["size"] = json!({ "cols": sz.cols, "rows": sz.rows });
        }
        self.call("terminal.run", p)
    }
    pub fn terminals(&self) -> Result<Value> {
        self.call("terminal.list", json!({"all":true}))
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
pub use canban_exec::shell_env;
pub mod stream;
pub mod transport;

pub use shell_env::{
    LocalEnvPolicy, ShellEnvSnapshot, apply_local_path_env_to_command,
    invalidate_local_shell_env_cache, invalidate_local_shell_path_cache, is_denied_shell_env_key,
    local_merged_path_string, local_search_paths, local_shell_env_snapshot,
    merge_local_env_with_policy, merge_local_path_env, merge_shell_env_snapshot,
    probe_shell_env_with, probe_shell_path_with, probe_shell_snapshot_with,
    refresh_local_shell_env, refresh_local_shell_path_if_stale,
};

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
        assert_eq!(
            caller_env.get("CUSTOM_FLAG").map(String::as_str),
            Some("1"),
            "non-identity hc_env variables must be forwarded into launch env"
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

    #[test]
    fn merge_default_terminal_env_forwards_hc_env_without_overwriting_caller_or_injecting_path() {
        let mut env = BTreeMap::from([
            ("CALLER_ONLY".into(), "keep-caller".into()),
            ("SHARED".into(), "from-caller".into()),
        ]);
        let hc_env = BTreeMap::from([
            ("SHARED".into(), "from-hc".into()),
            ("HTTP_PROXY".into(), "http://127.0.0.1:7890".into()),
            ("CARGO_HOME".into(), "/Users/demo/.cargo".into()),
            ("PATH".into(), "/local/only/bin".into()),
        ]);

        merge_default_terminal_env(&mut env, &hc_env);

        assert_eq!(
            env.get("CALLER_ONLY").map(String::as_str),
            Some("keep-caller")
        );
        assert_eq!(env.get("SHARED").map(String::as_str), Some("from-caller"));
        assert_eq!(
            env.get("HTTP_PROXY").map(String::as_str),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            env.get("CARGO_HOME").map(String::as_str),
            Some("/Users/demo/.cargo")
        );
        assert!(
            !env.contains_key("PATH"),
            "PATH must remain deferred to merge_local_path_env so remote machines stay isolated"
        );
        assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
    }

    #[test]
    fn launch_with_policy_unset_and_size_includes_size_only_when_specified() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock-hc-run-size");
        let log_path = dir.path().join("calls.jsonl");
        std::fs::write(
            &script,
            format!(
                r#"#!/usr/bin/python3
import sys, json
log_path = {:?}
for line in sys.stdin:
    req = json.loads(line)
    m = req["method"]
    if m == "hello":
        res = {{"generation": 1}}
    elif m == "terminal.run":
        with open(log_path, "a") as f:
            f.write(json.dumps(req["params"]) + "\n")
        res = {{"address": "t1"}}
    else:
        res = {{}}
    print(json.dumps({{"jsonrpc": "2.0", "id": req["id"], "result": res}}), flush=True)
"#,
                log_path
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let client = Client::new(Config {
            hc: script.to_string_lossy().into_owned(),
            hc_session: "unit-run-size".into(),
            ..Default::default()
        });
        let cwd = dir.path().to_string_lossy().into_owned();
        client
            .launch_with_policy_unset_and_size(
                vec!["/bin/sh".into()],
                &cwd,
                BTreeMap::new(),
                BTreeMap::new(),
                LocalEnvPolicy::MinimalPathOnly,
                &BTreeSet::new(),
                Some(TerminalSize {
                    cols: 112,
                    rows: 36,
                }),
            )
            .unwrap();
        client
            .launch_with_policy_and_unset(
                vec!["/bin/sh".into()],
                &cwd,
                BTreeMap::new(),
                BTreeMap::new(),
                LocalEnvPolicy::MinimalPathOnly,
                &BTreeSet::new(),
            )
            .unwrap();

        let recorded = std::fs::read_to_string(&log_path).unwrap();
        let lines: Vec<Value> = recorded
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["size"], json!({"cols": 112, "rows": 36}));
        assert!(lines[1].get("size").is_none());
    }

    #[test]
    fn client_with_runner_merges_shell_env_without_probing_host_shell() {
        use std::sync::Mutex;

        struct StubRunner {
            snapshot: ShellEnvSnapshot,
        }

        impl canban_exec::CommandRunner for StubRunner {
            fn run(&self, _spec: &canban_exec::CommandSpec) -> Result<canban_exec::ExecOutput> {
                anyhow::bail!("unexpected command run")
            }
            fn find_executable(&self, _name: &str) -> Option<std::path::PathBuf> {
                None
            }
            fn shell_env(&self) -> ShellEnvSnapshot {
                self.snapshot.clone()
            }
            fn hostname(&self) -> String {
                "stub-host".into()
            }
        }

        struct RecordingBackend {
            calls: Mutex<Vec<Value>>,
        }

        impl HerdrBackend for RecordingBackend {
            fn call(&self, _method: &str, params: Value) -> Result<Value> {
                self.calls.lock().unwrap().push(params);
                Ok(json!({ "address": "t-stub-1" }))
            }
            fn subscribe(&self, _wake: Arc<dyn Fn() + Send + Sync>) -> Result<BackendSubscription> {
                anyhow::bail!("unused")
            }
            fn activity(&self, _core: &str, _terminal: &str) -> crate::activity::Activity {
                crate::activity::Activity::default()
            }
        }

        let runner: Arc<dyn canban_exec::CommandRunner> = Arc::new(StubRunner {
            snapshot: ShellEnvSnapshot {
                shell: "/bin/stubsh".into(),
                vars: BTreeMap::from([
                    ("PATH".into(), "/stub/bin:/usr/bin".into()),
                    ("STUB_HOST_VAR".into(), "from-runner".into()),
                    ("DYLD_INSERT_LIBRARIES".into(), "/bad.dylib".into()),
                ]),
                path_entries: vec![
                    std::path::PathBuf::from("/stub/bin"),
                    std::path::PathBuf::from("/usr/bin"),
                ],
                full_env_captured: true,
            },
        });
        let backend = Arc::new(RecordingBackend {
            calls: Mutex::new(Vec::new()),
        });
        let client =
            Client::with_backend_and_runner(Config::default(), Some(backend.clone()), runner);

        // InheritHostShell pulls non-denied vars and PATH exclusively from runner.shell_env().
        client
            .launch_with_policy_unset_and_size(
                vec!["/bin/echo".into(), "hi".into()],
                "/tmp",
                BTreeMap::from([("CALLER_VAR".into(), "1".into())]),
                BTreeMap::new(),
                LocalEnvPolicy::InheritHostShell,
                &BTreeSet::new(),
                None,
            )
            .unwrap();

        // MinimalPathOnly pulls PATH only and never pulls STUB_HOST_VAR.
        client
            .launch_with_policy_unset_and_size(
                vec!["/bin/echo".into(), "hi".into()],
                "/tmp",
                BTreeMap::new(),
                BTreeMap::new(),
                LocalEnvPolicy::MinimalPathOnly,
                &BTreeSet::new(),
                None,
            )
            .unwrap();

        let calls = backend.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        let env0 = &calls[0]["env"];
        assert_eq!(env0["PATH"].as_str(), Some("/stub/bin:/usr/bin"));
        assert_eq!(env0["STUB_HOST_VAR"].as_str(), Some("from-runner"));
        assert_eq!(env0["CALLER_VAR"].as_str(), Some("1"));
        assert!(env0.get("DYLD_INSERT_LIBRARIES").is_none());

        let env1 = &calls[1]["env"];
        assert_eq!(env1["PATH"].as_str(), Some("/stub/bin:/usr/bin"));
        assert!(env1.get("STUB_HOST_VAR").is_none());
    }
}
