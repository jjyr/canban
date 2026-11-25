//! Unified local command construction and user shell `PATH` resolution.
#![allow(clippy::disallowed_methods)]

pub mod shell_env;

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::Arc,
};

pub use shell_env::{
    LocalEnvPolicy, ShellEnvSnapshot, apply_local_path_env_to_command,
    invalidate_local_shell_env_cache, invalidate_local_shell_path_cache, is_denied_shell_env_key,
    local_merged_path_string, local_search_paths, local_shell_env_snapshot,
    merge_local_env_with_policy, merge_local_path_env, merge_shell_env_snapshot,
    merged_path_string_from_snapshot, probe_shell_env_with, probe_shell_path_with,
    probe_shell_snapshot_with, refresh_local_shell_env, refresh_local_shell_path_if_stale,
    search_paths_from_snapshot,
};

/// Platform-independent specification for executing a local external command.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env_set: BTreeMap<String, String>,
    pub env_remove: Vec<String>,
    pub stdin: Option<Vec<u8>>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_set.insert(key.into(), value.into());
        self
    }

    pub fn env_remove(mut self, key: impl Into<String>) -> Self {
        self.env_remove.push(key.into());
        self
    }

    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }
}

/// Platform-independent output captured from a completed [`CommandSpec`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecOutput {
    pub status_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    pub fn success(&self) -> bool {
        self.status_code == 0
    }

    /// Converts into [`std::process::Output`] for callers expecting standard process output.
    pub fn into_std_output(self) -> Output {
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            let raw = if self.status_code < 0 {
                1 << 8
            } else {
                (self.status_code & 0xff) << 8
            };
            std::process::ExitStatus::from_raw(raw)
        };
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(self.status_code as u32)
        };
        Output {
            status,
            stdout: self.stdout,
            stderr: self.stderr,
        }
    }
}

/// Instance-scoped OS command/environment boundary (zero process-global state; safe for
/// parallel simulation workers). Production defaults to [`SystemRunner`].
pub trait CommandRunner: Send + Sync + 'static {
    fn run(&self, spec: &CommandSpec) -> anyhow::Result<ExecOutput>;
    fn find_executable(&self, name: &str) -> Option<PathBuf>;
    fn find_executable_with_env(
        &self,
        name: &str,
        extra_env: &BTreeMap<String, String>,
    ) -> Option<PathBuf> {
        let _ = extra_env;
        self.find_executable(name)
    }
    fn shell_env(&self) -> ShellEnvSnapshot;
    fn hostname(&self) -> String;
    /// Returns `true` for the real OS [`SystemRunner`] implementation.
    fn is_system(&self) -> bool {
        false
    }
}

/// Production [`CommandRunner`] delegating to real OS subprocesses and the cached login shell.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRunner;

pub fn system_runner() -> Arc<dyn CommandRunner> {
    Arc::new(SystemRunner)
}

impl CommandRunner for SystemRunner {
    fn is_system(&self) -> bool {
        true
    }
    fn run(&self, spec: &CommandSpec) -> anyhow::Result<ExecOutput> {
        let mut cmd = if spec.env_set.contains_key("PATH") {
            raw_command(&spec.program)
        } else {
            command_with_env(&spec.program, &spec.env_set)
        };
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        for key in &spec.env_remove {
            cmd.env_remove(key);
        }
        cmd.envs(&spec.env_set);
        let output = if let Some(input) = &spec.stdin {
            let mut child = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(input)?;
            }
            child.wait_with_output()?
        } else {
            cmd.output()?
        };
        Ok(ExecOutput {
            status_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn find_executable(&self, name: &str) -> Option<PathBuf> {
        find_executable(name, &BTreeMap::new())
    }

    fn find_executable_with_env(
        &self,
        name: &str,
        extra_env: &BTreeMap<String, String>,
    ) -> Option<PathBuf> {
        find_executable(name, extra_env)
    }

    fn shell_env(&self) -> ShellEnvSnapshot {
        local_shell_env_snapshot()
    }

    fn hostname(&self) -> String {
        command("hostname")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "canban-host".into())
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Locate an executable using `shell_env::local_search_paths(extra_env)`.
/// If `program` already contains a path separator, checks it directly.
pub fn find_executable(program: &str, extra_env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let trimmed = program.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('/') || trimmed.contains(std::path::MAIN_SEPARATOR) {
        let p = PathBuf::from(trimmed);
        return is_executable_file(&p).then_some(p);
    }
    for dir in local_search_paths(extra_env) {
        let candidate = dir.join(trimmed);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Build a local `Command` that resolves bare program names against the user's shell `PATH`
/// and injects the merged `PATH` so nested scripts (e.g. `#!/usr/bin/env node`) also succeed
/// when launched from a GUI app bundle.
pub fn command_with_env<S: AsRef<OsStr>>(
    program: S,
    extra_env: &BTreeMap<String, String>,
) -> Command {
    let raw = program.as_ref();
    let resolved = raw
        .to_str()
        .filter(|s| !s.contains('/') && !s.contains(std::path::MAIN_SEPARATOR))
        .and_then(|name| find_executable(name, extra_env));
    let mut cmd = match resolved {
        Some(path) => Command::new(path),
        None => Command::new(raw),
    };
    apply_local_path_env_to_command(&mut cmd, extra_env);
    cmd
}

/// Shorthand for `command_with_env(program, &BTreeMap::new())`.
pub fn command<S: AsRef<OsStr>>(program: S) -> Command {
    command_with_env(program, &BTreeMap::new())
}

/// Per-terminal Herdr scope environment variables that bind child processes to a hosted PTY's
/// kill scope and must be stripped when spawning long-lived daemons, UI, or control clients.
pub const HERDR_TERMINAL_SCOPE_ENV_KEYS: [&str; 3] =
    ["HC_SCOPE", "HC_TERMINAL_ID", "HC_TERMINAL_SOCKET"];

/// Remove Herdr per-terminal scope variables from `cmd` while preserving `HC_SESSION` and `HC_SOCKET`.
pub fn strip_herdr_terminal_scope_env(cmd: &mut Command) -> &mut Command {
    for key in HERDR_TERMINAL_SCOPE_ENV_KEYS {
        cmd.env_remove(key);
    }
    cmd
}

/// Build a `Command` without probing or modifying `PATH`.
/// Use only for bundled Canban binaries, explicit absolute system paths, or isolated test mocks.
pub fn raw_command<S: AsRef<OsStr>>(program: S) -> Command {
    Command::new(program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn strip_herdr_terminal_scope_env_removes_only_terminal_scope_keys() {
        let mut cmd = raw_command("true");
        cmd.env("HC_SESSION", "keep-session")
            .env("HC_SOCKET", "/tmp/keep-hc.sock")
            .env("HC_SCOPE", "drop-scope")
            .env("HC_TERMINAL_ID", "drop-tid")
            .env("HC_TERMINAL_SOCKET", "/tmp/drop-tsock");
        strip_herdr_terminal_scope_env(&mut cmd);

        let envs: BTreeMap<String, Option<String>> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|val| val.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(envs.get("HC_SESSION"), Some(&Some("keep-session".into())));
        assert_eq!(
            envs.get("HC_SOCKET"),
            Some(&Some("/tmp/keep-hc.sock".into()))
        );
        for key in HERDR_TERMINAL_SCOPE_ENV_KEYS {
            assert_eq!(envs.get(key), Some(&None), "{key} must be removed");
        }
    }

    #[test]
    fn command_with_env_resolves_bare_program_and_injects_path() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let helper = bin.join("my_helper_tool");
        std::fs::write(&helper, "#!/bin/sh\necho \"hello-exec\"\n").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();

        let extra_env =
            BTreeMap::from([("PATH".into(), format!("{}:/usr/bin:/bin", bin.display()))]);
        let out = command_with_env("my_helper_tool", &extra_env)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello-exec");
    }

    struct MockRunner {
        label: String,
        host: String,
        bins: BTreeMap<String, PathBuf>,
        env: ShellEnvSnapshot,
    }

    impl CommandRunner for MockRunner {
        fn run(&self, spec: &CommandSpec) -> anyhow::Result<ExecOutput> {
            let line = format!("{}:{}:{}", self.label, spec.program, spec.args.join(" "));
            Ok(ExecOutput {
                status_code: 0,
                stdout: line.into_bytes(),
                stderr: Vec::new(),
            })
        }

        fn find_executable(&self, name: &str) -> Option<PathBuf> {
            self.bins.get(name).cloned()
        }

        fn shell_env(&self) -> ShellEnvSnapshot {
            self.env.clone()
        }

        fn hostname(&self) -> String {
            self.host.clone()
        }
    }

    #[test]
    fn mock_command_runner_is_isolated_across_parallel_threads() {
        let handles: Vec<_> = (0..4)
            .map(|idx| {
                std::thread::spawn(move || {
                    let runner: Arc<dyn CommandRunner> = Arc::new(MockRunner {
                        label: format!("worker-{idx}"),
                        host: format!("sim-host-{idx}"),
                        bins: BTreeMap::from([(
                            "git".into(),
                            PathBuf::from(format!("/mock/{idx}/bin/git")),
                        )]),
                        env: ShellEnvSnapshot {
                            shell: "/bin/mocksh".into(),
                            vars: BTreeMap::from([
                                ("PATH".into(), format!("/mock/{idx}/bin")),
                                ("WORKER_ID".into(), idx.to_string()),
                            ]),
                            path_entries: vec![PathBuf::from(format!("/mock/{idx}/bin"))],
                            full_env_captured: true,
                        },
                    });
                    assert!(!runner.is_system());
                    assert_eq!(runner.hostname(), format!("sim-host-{idx}"));
                    assert_eq!(
                        runner.find_executable("git"),
                        Some(PathBuf::from(format!("/mock/{idx}/bin/git")))
                    );
                    assert_eq!(runner.find_executable("missing"), None);
                    assert_eq!(
                        runner.shell_env().vars.get("WORKER_ID").map(String::as_str),
                        Some(idx.to_string().as_str())
                    );
                    let spec = CommandSpec::new("git").args(["status", "--short"]);
                    let out = runner.run(&spec).unwrap();
                    assert!(out.success());
                    let std_out = out.into_std_output();
                    assert!(std_out.status.success());
                    assert_eq!(
                        String::from_utf8_lossy(&std_out.stdout),
                        format!("worker-{idx}:git:status --short")
                    );
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }
}
