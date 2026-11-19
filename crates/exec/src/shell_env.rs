//! User login/interactive shell `PATH` discovery, caching, and local process environment merging.
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
const INTERACTIVE_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const LOGIN_PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const FAILED_PROBE_RETRY_TTL: Duration = Duration::from_secs(15);

#[derive(Clone)]
struct CachedShellPath {
    shell: String,
    fetched_at: Instant,
    succeeded: bool,
    entries: Vec<PathBuf>,
}

static SHELL_PATH_CACHE: OnceLock<RwLock<Option<CachedShellPath>>> = OnceLock::new();
static SHELL_PROBE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static BACKGROUND_REFRESH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

fn cache_lock() -> &'static RwLock<Option<CachedShellPath>> {
    SHELL_PATH_CACHE.get_or_init(|| RwLock::new(None))
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
        .name("shell-path-refresh".into())
        .spawn(move || {
            let _flight_guard = RefreshFlightGuard;
            let _probe_guard = probe_lock().lock().unwrap_or_else(|e| e.into_inner());
            let probed = probe_shell_path_with(&shell);
            let Some(entries) = probed else {
                // Keep the last known good PATH entries when a background refresh fails.
                return;
            };
            if let Ok(mut guard) = cache_lock().write() {
                *guard = Some(CachedShellPath {
                    shell,
                    fetched_at: Instant::now(),
                    succeeded: true,
                    entries,
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

fn run_shell_probe(shell: &str, flags: &str, timeout: Duration) -> Option<Vec<PathBuf>> {
    let script = shell_probe_command(shell);
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
                return extract_marked_path(&buf);
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

/// Probe `shell` with interactive+login flags first (`-ilc`), falling back to login (`-lc`).
pub fn probe_shell_path_with(shell: &str) -> Option<Vec<PathBuf>> {
    run_shell_probe(shell, "-ilc", INTERACTIVE_PROBE_TIMEOUT)
        .or_else(|| run_shell_probe(shell, "-lc", LOGIN_PROBE_TIMEOUT))
}

fn is_cache_fresh(entry: &CachedShellPath, shell: &str, max_age: Duration) -> bool {
    if entry.shell != shell {
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
) -> Option<Vec<PathBuf>> {
    let guard = cache_lock().read().ok()?;
    let cached = guard.as_ref()?;
    if is_cache_fresh(cached, shell, max_age) {
        return Some(cached.entries.clone());
    }
    if allow_stale_while_revalidate && cached.shell == shell && cached.succeeded {
        let entries = cached.entries.clone();
        drop(guard);
        spawn_background_shell_refresh(shell.to_owned());
        return Some(entries);
    }
    None
}

fn cached_shell_path_entries(
    max_age: Duration,
    allow_stale_while_revalidate: bool,
) -> Vec<PathBuf> {
    let shell = preferred_shell();
    if let Some(entries) =
        lookup_cached_or_trigger_refresh(&shell, max_age, allow_stale_while_revalidate)
    {
        return entries;
    }

    let _probe_guard = probe_lock().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entries) =
        lookup_cached_or_trigger_refresh(&shell, max_age, allow_stale_while_revalidate)
    {
        return entries;
    }

    let probed = probe_shell_path_with(&shell);
    let succeeded = probed.is_some();
    let entries = probed.unwrap_or_default();
    if let Ok(mut guard) = cache_lock().write() {
        *guard = Some(CachedShellPath {
            shell,
            fetched_at: Instant::now(),
            succeeded,
            entries: entries.clone(),
        });
    }
    entries
}

/// Refresh cached local Shell `PATH` if older than `max_age` (skipped under `CANBAN_HERDR_SEARCH_PATHS`).
pub fn refresh_local_shell_path_if_stale(max_age: Duration) {
    if std::env::var_os("CANBAN_HERDR_SEARCH_PATHS").is_some() {
        return;
    }
    let _ = cached_shell_path_entries(max_age, false);
}

/// Drop the cached local Shell `PATH` so the next lookup or launch probes freshly.
pub fn invalidate_local_shell_path_cache() {
    if let Ok(mut guard) = cache_lock().write() {
        *guard = None;
    }
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
        if let Ok(mut guard) = cache_lock().write() {
            *guard = Some(CachedShellPath {
                shell: shell.clone(),
                fetched_at: Instant::now() - Duration::from_secs(3600),
                succeeded: true,
                entries: vec![PathBuf::from("/stale/cached/bin")],
            });
        }
        let started = Instant::now();
        let hit = lookup_cached_or_trigger_refresh(&shell, DEFAULT_CACHE_TTL, true);
        assert_eq!(hit, Some(vec![PathBuf::from("/stale/cached/bin")]));
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
}
