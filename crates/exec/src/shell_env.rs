//! User login/interactive shell environment and `PATH` discovery, caching, and trust-scoped merging.
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Mutex, OnceLock, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const PATH_BEGIN: &str = "__CANBAN_PATH_BEGIN__";
const PATH_END: &str = "__CANBAN_PATH_END__";
const ENV_BEGIN: &str = "__CANBAN_ENV_BEGIN__";
const ENV_END: &str = "__CANBAN_ENV_END__";
const INTERACTIVE_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const LOGIN_PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const FAILED_PROBE_RETRY_TTL: Duration = Duration::from_secs(15);

/// Session-local or dangerous environment variables that must never be inherited
/// from the host user shell snapshot.
const DENIED_EXACT_KEYS: &[&str] = &[
    "PWD",
    "OLDPWD",
    "SHLVL",
    "_",
    "TERM_SESSION_ID",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TMPDIR",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "HC_SESSION",
    "HC_SCOPE",
    "HC_TERMINAL_ID",
    "HC_TERMINAL_SOCKET",
];

/// Environment variable prefixes that must never be inherited from the host user shell snapshot.
const DENIED_KEY_PREFIXES: &[&str] = &["DYLD_", "CANBAN_", "GIT_", "HERDR_", "BASH_FUNC_"];

/// Returns true if `key` is denied from host shell environment inheritance.
pub fn is_denied_shell_env_key(key: &str) -> bool {
    if key.is_empty() || key.contains(['=', '\0', '\n', '\r']) {
        return true;
    }
    DENIED_EXACT_KEYS.contains(&key)
        || DENIED_KEY_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
}

/// Captured snapshot of the local user's login/interactive shell environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellEnvSnapshot {
    pub shell: String,
    pub vars: BTreeMap<String, String>,
    pub path_entries: Vec<PathBuf>,
    /// True when full environment probing (`env -0` / shell native dump) succeeded;
    /// false when falling back to `PATH`-only probing or empty defaults.
    pub full_env_captured: bool,
}

/// Trust-scoped local environment inheritance policy for automated processes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalEnvPolicy {
    /// Inherit the filtered host shell environment snapshot (for trusted `local` /
    /// `collaborator` runs when Host Settings enables terminal env inheritance).
    InheritHostShell,
    /// Minimal environment: only merge `PATH` (for `external_pr`, untrusted external
    /// cardless events, or when Host Settings is set to Minimal).
    MinimalPathOnly,
}

impl LocalEnvPolicy {
    /// Resolve the effective policy from the Host Settings toggle and trust level.
    /// Untrusted sources always downgrade to `MinimalPathOnly`.
    pub fn resolve(inherit_host_setting: bool, trusted_source: bool) -> Self {
        if inherit_host_setting && trusted_source {
            Self::InheritHostShell
        } else {
            Self::MinimalPathOnly
        }
    }

    /// Resolve the effective policy from a Canban `source_trust` value (`"local"`,
    /// `"collaborator"`, `"external_pr"`, etc.). Only `"local"` and `"collaborator"`
    /// are treated as trusted.
    pub fn for_source_trust(inherit_host_setting: bool, source_trust: Option<&str>) -> Self {
        let trusted = matches!(source_trust, Some("local" | "collaborator"));
        Self::resolve(inherit_host_setting, trusted)
    }
}

#[derive(Clone)]
struct CachedShellEnv {
    fetched_at: Instant,
    succeeded: bool,
    snapshot: ShellEnvSnapshot,
}

static SHELL_ENV_CACHE: OnceLock<RwLock<Option<CachedShellEnv>>> = OnceLock::new();
static SHELL_PROBE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static BACKGROUND_REFRESH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

fn cache_lock() -> &'static RwLock<Option<CachedShellEnv>> {
    SHELL_ENV_CACHE.get_or_init(|| RwLock::new(None))
}

fn probe_lock() -> &'static Mutex<()> {
    SHELL_PROBE_LOCK.get_or_init(|| Mutex::new(()))
}

struct RefreshFlightGuard;
impl Drop for RefreshFlightGuard {
    fn drop(&mut self) {
        BACKGROUND_REFRESH_IN_FLIGHT.store(false, Ordering::Release);
    }
}

fn spawn_background_shell_refresh(shell: String) {
    if BACKGROUND_REFRESH_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("shell-env-refresh".into())
        .spawn(move || {
            let _flight_guard = RefreshFlightGuard;
            let _probe_guard = probe_lock().lock().unwrap_or_else(|e| e.into_inner());
            let (snapshot, succeeded) = probe_shell_snapshot_with(&shell);
            if !succeeded {
                // Keep the last known good snapshot when a background refresh fails.
                return;
            }
            if let Ok(mut guard) = cache_lock().write() {
                *guard = Some(CachedShellEnv {
                    fetched_at: Instant::now(),
                    succeeded: true,
                    snapshot,
                });
            }
        });
    if spawned.is_err() {
        BACKGROUND_REFRESH_IN_FLIGHT.store(false, Ordering::Release);
    }
}

fn preferred_shell() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        let trimmed = shell.trim();
        if !trimmed.is_empty() && (!trimmed.contains('/') || Path::new(trimmed).is_file()) {
            return trimmed.to_owned();
        }
    }
    for candidate in ["/bin/zsh", "/bin/bash", "/bin/sh"] {
        if Path::new(candidate).is_file() {
            return candidate.to_owned();
        }
    }
    "/bin/sh".to_owned()
}

fn shell_probe_command(shell: &str) -> &'static str {
    let name = Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match name {
        "fish" => "printf '__CANBAN_PATH_BEGIN__%s__CANBAN_PATH_END__' (string join ':' $PATH)",
        "nu" => "print -n $\"__CANBAN_PATH_BEGIN__($env.PATH | str join ':')__CANBAN_PATH_END__\"",
        "pwsh" | "powershell" => {
            "Write-Host -NoNewline \"__CANBAN_PATH_BEGIN__$($env:PATH)__CANBAN_PATH_END__\""
        }
        _ => "printf '__CANBAN_PATH_BEGIN__%s__CANBAN_PATH_END__' \"$PATH\"",
    }
}

fn shell_env_probe_command(shell: &str) -> &'static str {
    let name = Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match name {
        "fish" => "printf '__CANBAN_ENV_BEGIN__'; and env -0; and printf '__CANBAN_ENV_END__'",
        "nu" => "print -n '__CANBAN_ENV_BEGIN__'; ^env -0; print -n '__CANBAN_ENV_END__'",
        "pwsh" | "powershell" => {
            "[Console]::Out.Write('__CANBAN_ENV_BEGIN__'); Get-ChildItem Env: | ForEach-Object { [Console]::Out.Write(\"$($_.Name)=$($_.Value)`0\") }; [Console]::Out.Write('__CANBAN_ENV_END__')"
        }
        _ => "printf '__CANBAN_ENV_BEGIN__' && env -0 && printf '__CANBAN_ENV_END__'",
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn find_subslice_rev(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(haystack.len());
    }
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

fn extract_marked_path(bytes: &[u8]) -> Option<Vec<PathBuf>> {
    let text = String::from_utf8_lossy(bytes);
    let start = text.rfind(PATH_BEGIN)? + PATH_BEGIN.len();
    let rest = &text[start..];
    let end = rest.find(PATH_END)?;
    let raw = rest[..end].trim();
    if raw.is_empty() || raw == "%s" {
        return None;
    }
    let entries: Vec<PathBuf> = std::env::split_paths(raw)
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    (!entries.is_empty()).then_some(entries)
}

fn extract_marked_env(bytes: &[u8]) -> Option<BTreeMap<String, String>> {
    let begin = ENV_BEGIN.as_bytes();
    let end = ENV_END.as_bytes();
    let start = find_subslice_rev(bytes, begin)? + begin.len();
    let rest = &bytes[start..];
    let end_idx = find_subslice(rest, end)?;
    let payload = &rest[..end_idx];

    let mut vars = BTreeMap::new();
    for entry in payload.split(|&b| b == 0) {
        if entry.is_empty() {
            continue;
        }
        let Some(eq) = entry.iter().position(|&b| b == b'=') else {
            continue;
        };
        let Ok(key) = std::str::from_utf8(&entry[..eq]) else {
            continue;
        };
        if is_denied_shell_env_key(key) {
            continue;
        }
        let value = String::from_utf8_lossy(&entry[eq + 1..]).into_owned();
        vars.insert(key.to_owned(), value);
    }
    (!vars.is_empty()).then_some(vars)
}

fn kill_probe_process(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        const SIGKILL: i32 = 9;
        if let Ok(pgid) = i32::try_from(child.id())
            && pgid > 0
        {
            // SAFETY: `child` was spawned in its own process group (`process_group(0)`),
            // so `-pgid` terminates the shell and any grandchild holding stdout open.
            unsafe {
                let _ = kill(-pgid, SIGKILL);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run_shell_probe_bytes(
    shell: &str,
    flags: &str,
    script: &str,
    timeout: Duration,
) -> Option<Vec<u8>> {
    let mut cmd = Command::new(shell);
    cmd.arg(flags)
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = BufReader::new(stdout).read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let buf = out_handle.join().unwrap_or_default();
                if !status.success() {
                    return None;
                }
                return Some(buf);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_probe_process(&mut child);
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                kill_probe_process(&mut child);
                return None;
            }
        }
    }
}

fn run_shell_probe(shell: &str, flags: &str, timeout: Duration) -> Option<Vec<PathBuf>> {
    let script = shell_probe_command(shell);
    let buf = run_shell_probe_bytes(shell, flags, script, timeout)?;
    extract_marked_path(&buf)
}

fn run_shell_env_probe(
    shell: &str,
    flags: &str,
    timeout: Duration,
) -> Option<BTreeMap<String, String>> {
    let script = shell_env_probe_command(shell);
    let buf = run_shell_probe_bytes(shell, flags, script, timeout)?;
    extract_marked_env(&buf)
}

/// Probe `shell` with interactive+login flags first (`-ilc`), falling back to login (`-lc`).
pub fn probe_shell_path_with(shell: &str) -> Option<Vec<PathBuf>> {
    run_shell_probe(shell, "-ilc", INTERACTIVE_PROBE_TIMEOUT)
        .or_else(|| run_shell_probe(shell, "-lc", LOGIN_PROBE_TIMEOUT))
}

/// Probe `shell` for a full filtered environment snapshot with `-ilc`, falling back to `-lc`.
pub fn probe_shell_env_with(shell: &str) -> Option<BTreeMap<String, String>> {
    run_shell_env_probe(shell, "-ilc", INTERACTIVE_PROBE_TIMEOUT)
        .or_else(|| run_shell_env_probe(shell, "-lc", LOGIN_PROBE_TIMEOUT))
}

/// Probe `shell` for a full environment snapshot, falling back to `PATH`-only if full probing fails.
pub fn probe_shell_snapshot_with(shell: &str) -> (ShellEnvSnapshot, bool) {
    if let Some(mut vars) = probe_shell_env_with(shell) {
        let mut path_entries: Vec<PathBuf> = vars
            .get("PATH")
            .map(|raw| {
                std::env::split_paths(raw)
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect()
            })
            .unwrap_or_default();
        if path_entries.is_empty()
            && let Some(fallback_paths) = probe_shell_path_with(shell)
        {
            if let Ok(joined) = std::env::join_paths(&fallback_paths) {
                vars.insert("PATH".into(), joined.to_string_lossy().into_owned());
            }
            path_entries = fallback_paths;
        }
        return (
            ShellEnvSnapshot {
                shell: shell.to_owned(),
                vars,
                path_entries,
                full_env_captured: true,
            },
            true,
        );
    }

    if let Some(path_entries) = probe_shell_path_with(shell) {
        let mut vars = BTreeMap::new();
        if let Ok(joined) = std::env::join_paths(&path_entries) {
            vars.insert("PATH".into(), joined.to_string_lossy().into_owned());
        }
        return (
            ShellEnvSnapshot {
                shell: shell.to_owned(),
                vars,
                path_entries,
                full_env_captured: false,
            },
            true,
        );
    }

    (
        ShellEnvSnapshot {
            shell: shell.to_owned(),
            vars: BTreeMap::new(),
            path_entries: Vec::new(),
            full_env_captured: false,
        },
        false,
    )
}

fn is_cache_fresh(entry: &CachedShellEnv, shell: &str, max_age: Duration) -> bool {
    if entry.snapshot.shell != shell {
        return false;
    }
    let effective_ttl = if entry.succeeded {
        max_age
    } else {
        max_age.min(FAILED_PROBE_RETRY_TTL)
    };
    entry.fetched_at.elapsed() < effective_ttl
}

fn lookup_cached_or_trigger_refresh(
    shell: &str,
    max_age: Duration,
    allow_stale_while_revalidate: bool,
) -> Option<ShellEnvSnapshot> {
    let guard = cache_lock().read().ok()?;
    let cached = guard.as_ref()?;
    if is_cache_fresh(cached, shell, max_age) {
        return Some(cached.snapshot.clone());
    }
    if allow_stale_while_revalidate && cached.snapshot.shell == shell && cached.succeeded {
        let snapshot = cached.snapshot.clone();
        drop(guard);
        spawn_background_shell_refresh(shell.to_owned());
        return Some(snapshot);
    }
    None
}

fn cached_shell_env_snapshot(
    max_age: Duration,
    allow_stale_while_revalidate: bool,
) -> ShellEnvSnapshot {
    let shell = preferred_shell();
    if let Some(snapshot) =
        lookup_cached_or_trigger_refresh(&shell, max_age, allow_stale_while_revalidate)
    {
        return snapshot;
    }

    let _probe_guard = probe_lock().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(snapshot) =
        lookup_cached_or_trigger_refresh(&shell, max_age, allow_stale_while_revalidate)
    {
        return snapshot;
    }

    let (snapshot, succeeded) = probe_shell_snapshot_with(&shell);
    if let Ok(mut guard) = cache_lock().write() {
        *guard = Some(CachedShellEnv {
            fetched_at: Instant::now(),
            succeeded,
            snapshot: snapshot.clone(),
        });
    }
    snapshot
}

fn cached_shell_path_entries(
    max_age: Duration,
    allow_stale_while_revalidate: bool,
) -> Vec<PathBuf> {
    cached_shell_env_snapshot(max_age, allow_stale_while_revalidate).path_entries
}

/// Return the cached local user shell environment snapshot (with stale-while-revalidate).
/// Returns an empty snapshot under `CANBAN_HERDR_SEARCH_PATHS` for test isolation.
pub fn local_shell_env_snapshot() -> ShellEnvSnapshot {
    if std::env::var_os("CANBAN_HERDR_SEARCH_PATHS").is_some() {
        return ShellEnvSnapshot {
            shell: preferred_shell(),
            ..ShellEnvSnapshot::default()
        };
    }
    cached_shell_env_snapshot(DEFAULT_CACHE_TTL, true)
}

/// Synchronously re-probe the local user shell environment and update the cache
/// (used by Host Settings "Refresh"). Skipped under `CANBAN_HERDR_SEARCH_PATHS`.
pub fn refresh_local_shell_env() -> ShellEnvSnapshot {
    let shell = preferred_shell();
    if std::env::var_os("CANBAN_HERDR_SEARCH_PATHS").is_some() {
        return ShellEnvSnapshot {
            shell,
            ..ShellEnvSnapshot::default()
        };
    }
    let _probe_guard = probe_lock().lock().unwrap_or_else(|e| e.into_inner());
    let (snapshot, succeeded) = probe_shell_snapshot_with(&shell);
    if let Ok(mut guard) = cache_lock().write() {
        *guard = Some(CachedShellEnv {
            fetched_at: Instant::now(),
            succeeded,
            snapshot: snapshot.clone(),
        });
    }
    snapshot
}

/// Refresh cached local Shell `PATH`/env if older than `max_age` (skipped under `CANBAN_HERDR_SEARCH_PATHS`).
pub fn refresh_local_shell_path_if_stale(max_age: Duration) {
    if std::env::var_os("CANBAN_HERDR_SEARCH_PATHS").is_some() {
        return;
    }
    let _ = cached_shell_env_snapshot(max_age, false);
}

/// Drop the cached local Shell `PATH`/env so the next lookup or launch probes freshly.
pub fn invalidate_local_shell_path_cache() {
    if let Ok(mut guard) = cache_lock().write() {
        *guard = None;
    }
}

/// Alias for `invalidate_local_shell_path_cache` (clears the unified shell env/PATH cache).
pub fn invalidate_local_shell_env_cache() {
    invalidate_local_shell_path_cache();
}

fn push_unique_dir(out: &mut Vec<PathBuf>, dir: PathBuf) {
    if !dir.as_os_str().is_empty() && !out.contains(&dir) {
        out.push(dir);
    }
}

fn push_split_paths(out: &mut Vec<PathBuf>, raw: &std::ffi::OsStr) {
    for p in std::env::split_paths(raw) {
        push_unique_dir(out, p);
    }
}

fn resolve_home(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    env.get("HOME")
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .filter(|p| !p.as_os_str().is_empty())
}

fn push_home_fallbacks(out: &mut Vec<PathBuf>, home: &Path) {
    push_unique_dir(out, home.join(".local/bin"));
    push_unique_dir(out, home.join(".local/share/mise/shims"));
    push_unique_dir(out, home.join(".cargo/bin"));
    push_unique_dir(out, home.join(".bun/bin"));
    push_unique_dir(out, home.join("bin"));
}

/// Build ordered, deduplicated search directories for local binary discovery.
/// Honors `CANBAN_HERDR_SEARCH_PATHS` exclusively when present for test isolation.
pub fn local_search_paths(env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let search_override = env
        .get("CANBAN_HERDR_SEARCH_PATHS")
        .map(OsString::from)
        .or_else(|| std::env::var_os("CANBAN_HERDR_SEARCH_PATHS"));
    if let Some(override_paths) = search_override {
        let mut out = Vec::new();
        push_split_paths(&mut out, &override_paths);
        return out;
    }

    let mut out = Vec::new();
    if let Some(install_dir) = env
        .get("HC_INSTALL_DIR")
        .map(OsString::from)
        .or_else(|| std::env::var_os("HC_INSTALL_DIR"))
    {
        push_unique_dir(&mut out, PathBuf::from(install_dir));
    }
    if let Some(explicit_path) = env.get("PATH").filter(|s| !s.is_empty()) {
        push_split_paths(&mut out, OsString::from(explicit_path).as_os_str());
    }
    for dir in cached_shell_path_entries(DEFAULT_CACHE_TTL, true) {
        push_unique_dir(&mut out, dir);
    }
    if let Some(proc_path) = std::env::var_os("PATH") {
        push_split_paths(&mut out, &proc_path);
    }
    if let Some(home) = resolve_home(env) {
        push_home_fallbacks(&mut out, &home);
    }
    push_unique_dir(&mut out, PathBuf::from("/opt/homebrew/bin"));
    push_unique_dir(&mut out, PathBuf::from("/usr/local/bin"));
    out
}

/// Build the merged local `PATH` environment string for spawning local processes.
/// Returns `None` when `CANBAN_HERDR_SEARCH_PATHS` is active (leaving test process `PATH` untouched).
pub fn local_merged_path_string(hc_env: &BTreeMap<String, String>) -> Option<String> {
    if let Some(explicit) = hc_env.get("PATH").filter(|s| !s.is_empty()) {
        return Some(explicit.clone());
    }
    if hc_env.contains_key("CANBAN_HERDR_SEARCH_PATHS")
        || std::env::var_os("CANBAN_HERDR_SEARCH_PATHS").is_some()
    {
        return None;
    }

    let mut out = Vec::new();
    for dir in cached_shell_path_entries(DEFAULT_CACHE_TTL, true) {
        push_unique_dir(&mut out, dir);
    }
    if let Some(proc_path) = std::env::var_os("PATH") {
        push_split_paths(&mut out, &proc_path);
    }
    if let Some(home) = resolve_home(hc_env) {
        push_home_fallbacks(&mut out, &home);
    }
    for sys in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ] {
        push_unique_dir(&mut out, PathBuf::from(sys));
    }
    std::env::join_paths(out)
        .ok()
        .map(|s| s.to_string_lossy().into_owned())
}

/// Inject merged local `PATH` into a launch environment map unless already set by caller or `hc_env`.
pub fn merge_local_path_env(env: &mut BTreeMap<String, String>, hc_env: &BTreeMap<String, String>) {
    if env.get("PATH").is_some_and(|v| !v.is_empty()) {
        return;
    }
    if let Some(path) = local_merged_path_string(hc_env) {
        env.insert("PATH".into(), path);
    }
}

/// Merge non-denied variables from an explicit `ShellEnvSnapshot` into `env` without
/// overwriting keys already set in `env` or `hc_env` (`env` > `hc_env` > `snapshot`).
/// `PATH` is intentionally skipped here so `merge_local_path_env` applies unified `PATH` merging.
pub fn merge_shell_env_snapshot(
    env: &mut BTreeMap<String, String>,
    hc_env: &BTreeMap<String, String>,
    snapshot: &ShellEnvSnapshot,
) {
    for (k, v) in &snapshot.vars {
        if k == "PATH" || is_denied_shell_env_key(k) {
            continue;
        }
        if !env.contains_key(k) && !hc_env.contains_key(k) {
            env.insert(k.clone(), v.clone());
        }
    }
}

/// Merge local host environment into `env` according to `policy` without overwriting
/// any key already set by the caller or `hc_env` (`env` > `hc_env` > host shell snapshot).
///
/// - `LocalEnvPolicy::InheritHostShell`: merges non-denied variables from the cached host
///   shell snapshot (skipped under `CANBAN_HERDR_SEARCH_PATHS` test isolation), then merges `PATH`.
/// - `LocalEnvPolicy::MinimalPathOnly`: merges only `PATH`.
pub fn merge_local_env_with_policy(
    env: &mut BTreeMap<String, String>,
    hc_env: &BTreeMap<String, String>,
    policy: LocalEnvPolicy,
) {
    if policy == LocalEnvPolicy::InheritHostShell
        && !hc_env.contains_key("CANBAN_HERDR_SEARCH_PATHS")
        && std::env::var_os("CANBAN_HERDR_SEARCH_PATHS").is_none()
    {
        let snapshot = local_shell_env_snapshot();
        merge_shell_env_snapshot(env, hc_env, &snapshot);
    }
    merge_local_path_env(env, hc_env);
}

/// Apply merged local `PATH` onto a spawned `Command` unless `CANBAN_HERDR_SEARCH_PATHS` is active.
pub fn apply_local_path_env_to_command(cmd: &mut Command, hc_env: &BTreeMap<String, String>) {
    if let Some(path) = local_merged_path_string(hc_env) {
        cmd.env("PATH", path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn extract_marked_path_ignores_banner_and_echo_noise() {
        let raw = b"Welcome to zsh!\nprintf '__CANBAN_PATH_BEGIN__%s__CANBAN_PATH_END__' \"$PATH\"\n__CANBAN_PATH_BEGIN__/Users/demo/.cache/npm/global/bin:/opt/homebrew/bin:/usr/bin__CANBAN_PATH_END__\nbye\n";
        let parsed = extract_marked_path(raw).unwrap();
        assert_eq!(
            parsed,
            vec![
                PathBuf::from("/Users/demo/.cache/npm/global/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/bin"),
            ]
        );
    }

    #[test]
    fn probe_shell_path_falls_back_from_failing_interactive_to_login_and_enforces_timeout() {
        let dir = tempfile::tempdir().unwrap();

        // 1. Mock shell that fails on `-ilc` and succeeds on `-lc` with extra stdout noise.
        let fallback_shell = dir.path().join("mock-fallback-sh");
        std::fs::write(
            &fallback_shell,
            "#!/bin/sh\nif [ \"$1\" = \"-ilc\" ]; then exit 1; fi\necho \"noisy startup\"\nprintf '__CANBAN_PATH_BEGIN__/custom/npm/bin:/opt/homebrew/bin__CANBAN_PATH_END__'\n",
        )
        .unwrap();
        std::fs::set_permissions(&fallback_shell, std::fs::Permissions::from_mode(0o755)).unwrap();

        let probed = probe_shell_path_with(fallback_shell.to_str().unwrap()).unwrap();
        assert_eq!(
            probed,
            vec![
                PathBuf::from("/custom/npm/bin"),
                PathBuf::from("/opt/homebrew/bin"),
            ]
        );

        // 2. Mock shell that forks a hanging child -> entire process group is terminated at timeout.
        let slow_shell = dir.path().join("mock-slow-sh");
        std::fs::write(&slow_shell, "#!/bin/sh\nsleep 10 &\nwait\n").unwrap();
        std::fs::set_permissions(&slow_shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = Instant::now();
        let timed_out = run_shell_probe(
            slow_shell.to_str().unwrap(),
            "-ilc",
            Duration::from_millis(120),
        );
        assert!(timed_out.is_none());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "hanging shell probe must abort quickly"
        );
    }

    #[test]
    fn local_search_paths_honors_search_override_exclusively() {
        let env = BTreeMap::from([(
            "CANBAN_HERDR_SEARCH_PATHS".into(),
            "/tmp/isolated-a:/tmp/isolated-b".into(),
        )]);
        assert_eq!(
            local_search_paths(&env),
            vec![
                PathBuf::from("/tmp/isolated-a"),
                PathBuf::from("/tmp/isolated-b"),
            ]
        );
        assert_eq!(local_merged_path_string(&env), None);
    }

    #[test]
    fn merge_local_path_env_respects_caller_and_hc_env_overrides() {
        let mut caller_env = BTreeMap::from([("PATH".into(), "/caller/bin".into())]);
        let hc_env = BTreeMap::from([("PATH".into(), "/hc/bin".into())]);
        merge_local_path_env(&mut caller_env, &hc_env);
        assert_eq!(
            caller_env.get("PATH").map(String::as_str),
            Some("/caller/bin")
        );

        let mut empty_caller = BTreeMap::new();
        merge_local_path_env(&mut empty_caller, &hc_env);
        assert_eq!(
            empty_caller.get("PATH").map(String::as_str),
            Some("/hc/bin")
        );
    }

    #[test]
    fn merged_path_allows_env_shebang_script_to_resolve_interpreter() {
        let dir = tempfile::tempdir().unwrap();
        let node_bin = dir.path().join("mise-node/bin");
        let npm_bin = dir.path().join("npm-global/bin");
        std::fs::create_dir_all(&node_bin).unwrap();
        std::fs::create_dir_all(&npm_bin).unwrap();

        let mock_node = node_bin.join("mock_node_rt");
        std::fs::write(&mock_node, "#!/bin/sh\necho \"ok-from-node\"\n").unwrap();
        std::fs::set_permissions(&mock_node, std::fs::Permissions::from_mode(0o755)).unwrap();

        let pi_script = npm_bin.join("pi");
        std::fs::write(&pi_script, "#!/usr/bin/env mock_node_rt\n").unwrap();
        std::fs::set_permissions(&pi_script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let hc_env = BTreeMap::from([(
            "PATH".into(),
            format!("{}:{}:/usr/bin:/bin", npm_bin.display(), node_bin.display()),
        )]);
        let mut launch_env = BTreeMap::new();
        merge_local_path_env(&mut launch_env, &hc_env);

        let out = Command::new(&pi_script)
            .env_clear()
            .env("PATH", launch_env.get("PATH").unwrap())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "env shebang script failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok-from-node");
    }

    #[test]
    fn stale_successful_cache_returns_immediately_via_stale_while_revalidate() {
        let shell = preferred_shell();
        let expected_snapshot = ShellEnvSnapshot {
            shell: shell.clone(),
            vars: BTreeMap::from([
                ("PATH".into(), "/stale/cached/bin".into()),
                ("FOO".into(), "bar".into()),
            ]),
            path_entries: vec![PathBuf::from("/stale/cached/bin")],
            full_env_captured: true,
        };
        if let Ok(mut guard) = cache_lock().write() {
            *guard = Some(CachedShellEnv {
                fetched_at: Instant::now() - Duration::from_secs(3600),
                succeeded: true,
                snapshot: expected_snapshot.clone(),
            });
        }
        let started = Instant::now();
        let hit = lookup_cached_or_trigger_refresh(&shell, DEFAULT_CACHE_TTL, true);
        assert_eq!(hit, Some(expected_snapshot));
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "stale-while-revalidate must return immediately without blocking on shell probe"
        );
        // Synchronous callers (diagnostics/onboarding) still treat expired cache as stale.
        assert_eq!(
            lookup_cached_or_trigger_refresh(&shell, DEFAULT_CACHE_TTL, false),
            None
        );
    }

    #[test]
    fn denylist_blocks_dangerous_and_session_local_keys() {
        for denied in [
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "LD_AUDIT",
            "CANBAN_HOME",
            "CANBAN_CARD",
            "GIT_DIR",
            "GIT_WORK_TREE",
            "HERDR_SOCKET",
            "HC_SESSION",
            "HC_SCOPE",
            "HC_TERMINAL_ID",
            "HC_TERMINAL_SOCKET",
            "PWD",
            "OLDPWD",
            "SHLVL",
            "_",
            "TERM_SESSION_ID",
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "TMPDIR",
            "BASH_FUNC_foo%%",
            "",
            "BAD=KEY",
        ] {
            assert!(
                is_denied_shell_env_key(denied),
                "expected {denied:?} to be denied"
            );
        }
        for allowed in [
            "HOME",
            "PATH",
            "USER",
            "LANG",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "HTTP_PROXY",
            "GOPATH",
            "LDFLAGS",
        ] {
            assert!(
                !is_denied_shell_env_key(allowed),
                "expected {allowed:?} to be allowed"
            );
        }
    }

    #[test]
    fn extract_marked_env_parses_nul_delimited_vars_and_applies_denylist() {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"Welcome banner!\nprintf '__CANBAN_ENV_BEGIN__' && env -0\n");
        raw.extend_from_slice(ENV_BEGIN.as_bytes());
        raw.extend_from_slice(b"HOME=/Users/demo\0");
        raw.extend_from_slice(b"MULTILINE=line1\nline2=stillval\0");
        raw.extend_from_slice(b"DYLD_INSERT_LIBRARIES=/evil.dylib\0");
        raw.extend_from_slice(b"LD_PRELOAD=/evil.so\0");
        raw.extend_from_slice(b"CANBAN_CARD=99\0");
        raw.extend_from_slice(b"GIT_DIR=/tmp/.git\0");
        raw.extend_from_slice(b"HERDR_HOME=/tmp/h\0");
        raw.extend_from_slice(b"PWD=/tmp\0");
        raw.extend_from_slice(b"SHLVL=2\0");
        raw.extend_from_slice(b"TERM_PROGRAM=iTerm.app\0");
        raw.extend_from_slice(b"TMPDIR=/tmp/stale\0");
        raw.extend_from_slice(b"CARGO_HOME=/Users/demo/.cargo\0");
        raw.extend_from_slice(ENV_END.as_bytes());
        raw.extend_from_slice(b"\nbye\n");

        let parsed = extract_marked_env(&raw).unwrap();
        assert_eq!(parsed.get("HOME").map(String::as_str), Some("/Users/demo"));
        assert_eq!(
            parsed.get("MULTILINE").map(String::as_str),
            Some("line1\nline2=stillval")
        );
        assert_eq!(
            parsed.get("CARGO_HOME").map(String::as_str),
            Some("/Users/demo/.cargo")
        );
        assert!(!parsed.contains_key("DYLD_INSERT_LIBRARIES"));
        assert!(!parsed.contains_key("LD_PRELOAD"));
        assert!(!parsed.contains_key("CANBAN_CARD"));
        assert!(!parsed.contains_key("GIT_DIR"));
        assert!(!parsed.contains_key("HERDR_HOME"));
        assert!(!parsed.contains_key("PWD"));
        assert!(!parsed.contains_key("SHLVL"));
        assert!(!parsed.contains_key("TERM_PROGRAM"));
        assert!(!parsed.contains_key("TMPDIR"));
    }

    #[test]
    fn shell_env_probe_commands_handle_posix_and_non_posix_shells() {
        assert!(shell_env_probe_command("/bin/zsh").contains("env -0"));
        assert!(shell_env_probe_command("/bin/bash").contains("env -0"));
        assert!(shell_env_probe_command("/opt/homebrew/bin/fish").contains("and env -0"));
        assert!(shell_env_probe_command("/opt/homebrew/bin/nu").contains("^env -0"));
        assert!(shell_env_probe_command("/usr/local/bin/pwsh").contains("Get-ChildItem Env:"));
    }

    #[test]
    fn probe_shell_snapshot_captures_full_env_and_falls_back_to_path_only() {
        let dir = tempfile::tempdir().unwrap();

        // 1. Mock shell that supports full env probe on `-lc` (failing `-ilc`).
        let full_shell = dir.path().join("mock-full-env-sh");
        std::fs::write(
            &full_shell,
            "#!/bin/sh\nif [ \"$1\" = \"-ilc\" ]; then exit 1; fi\nprintf '__CANBAN_ENV_BEGIN__PATH=/custom/bin:/usr/bin\\0CUSTOM_HOST_VAR=hello\\0DYLD_LIBRARY_PATH=/bad\\0__CANBAN_ENV_END__'\n",
        )
        .unwrap();
        std::fs::set_permissions(&full_shell, std::fs::Permissions::from_mode(0o755)).unwrap();

        let (snap, ok) = probe_shell_snapshot_with(full_shell.to_str().unwrap());
        assert!(ok);
        assert!(snap.full_env_captured);
        assert_eq!(
            snap.vars.get("CUSTOM_HOST_VAR").map(String::as_str),
            Some("hello")
        );
        assert!(!snap.vars.contains_key("DYLD_LIBRARY_PATH"));
        assert_eq!(
            snap.path_entries,
            vec![PathBuf::from("/custom/bin"), PathBuf::from("/usr/bin")]
        );

        // 2. Mock shell where full env probe fails, falling back to PATH-only probe.
        let path_only_shell = dir.path().join("mock-path-only-sh");
        std::fs::write(
            &path_only_shell,
            "#!/bin/sh\ncase \"$2\" in\n  *__CANBAN_ENV_BEGIN__*) exit 1 ;;\n  *) printf '__CANBAN_PATH_BEGIN__/fallback/bin:/usr/bin__CANBAN_PATH_END__' ;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&path_only_shell, std::fs::Permissions::from_mode(0o755)).unwrap();

        let (fallback_snap, fallback_ok) =
            probe_shell_snapshot_with(path_only_shell.to_str().unwrap());
        assert!(fallback_ok);
        assert!(!fallback_snap.full_env_captured);
        assert_eq!(
            fallback_snap.path_entries,
            vec![PathBuf::from("/fallback/bin"), PathBuf::from("/usr/bin")]
        );
        assert_eq!(
            fallback_snap.vars.get("PATH").map(String::as_str),
            Some("/fallback/bin:/usr/bin")
        );
    }

    #[test]
    fn local_env_policy_enforces_trust_scoping_and_precedence() {
        assert_eq!(
            LocalEnvPolicy::for_source_trust(true, Some("local")),
            LocalEnvPolicy::InheritHostShell
        );
        assert_eq!(
            LocalEnvPolicy::for_source_trust(true, Some("collaborator")),
            LocalEnvPolicy::InheritHostShell
        );
        assert_eq!(
            LocalEnvPolicy::for_source_trust(true, Some("external_pr")),
            LocalEnvPolicy::MinimalPathOnly
        );
        assert_eq!(
            LocalEnvPolicy::for_source_trust(true, None),
            LocalEnvPolicy::MinimalPathOnly
        );
        assert_eq!(
            LocalEnvPolicy::for_source_trust(false, Some("local")),
            LocalEnvPolicy::MinimalPathOnly
        );

        let snapshot = ShellEnvSnapshot {
            shell: "/bin/zsh".into(),
            vars: BTreeMap::from([
                ("HOST_ONLY".into(), "from-shell".into()),
                ("SHARED_HC".into(), "from-shell".into()),
                ("SHARED_CALLER".into(), "from-shell".into()),
                ("PATH".into(), "/shell/bin".into()),
                ("DYLD_INSERT_LIBRARIES".into(), "/evil.dylib".into()),
            ]),
            path_entries: vec![PathBuf::from("/shell/bin")],
            full_env_captured: true,
        };
        let hc_env = BTreeMap::from([
            ("SHARED_HC".into(), "from-hc".into()),
            ("PATH".into(), "/hc/bin".into()),
        ]);
        let mut env = BTreeMap::from([("SHARED_CALLER".into(), "from-caller".into())]);
        merge_shell_env_snapshot(&mut env, &hc_env, &snapshot);

        assert_eq!(env.get("HOST_ONLY").map(String::as_str), Some("from-shell"));
        assert_eq!(
            env.get("SHARED_CALLER").map(String::as_str),
            Some("from-caller")
        );
        // hc_env key is not shadowed by snapshot; PATH is deferred to merge_local_path_env; denied key excluded.
        assert!(!env.contains_key("SHARED_HC"));
        assert!(!env.contains_key("PATH"));
        assert!(!env.contains_key("DYLD_INSERT_LIBRARIES"));

        // MinimalPathOnly only merges PATH and never pulls snapshot vars.
        let mut minimal_env = BTreeMap::new();
        merge_local_env_with_policy(&mut minimal_env, &hc_env, LocalEnvPolicy::MinimalPathOnly);
        assert_eq!(minimal_env.get("PATH").map(String::as_str), Some("/hc/bin"));
        assert_eq!(minimal_env.len(), 1);
    }
}
