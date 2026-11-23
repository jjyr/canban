//! Unified local command construction and user shell `PATH` resolution.
#![allow(clippy::disallowed_methods)]

pub mod shell_env;

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};

pub use shell_env::{
    LocalEnvPolicy, ShellEnvSnapshot, apply_local_path_env_to_command,
    invalidate_local_shell_env_cache, invalidate_local_shell_path_cache, is_denied_shell_env_key,
    local_merged_path_string, local_search_paths, local_shell_env_snapshot,
    merge_local_env_with_policy, merge_local_path_env, merge_shell_env_snapshot,
    probe_shell_env_with, probe_shell_path_with, probe_shell_snapshot_with,
    refresh_local_shell_env, refresh_local_shell_path_if_stale,
};

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
}
