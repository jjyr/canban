//! Detached installer. The journal survives every bundle/process transition.
use super::*;
use anyhow::{bail, ensure};
use serde_json::json;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
pub mod local;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub identity: String,
}
pub fn process(pid: u32) -> Result<Process> {
    let output = canban_exec::raw_command("ps")
        .args(["-ww", "-p", &pid.to_string(), "-o", "lstart=,command="])
        .output()?;
    ensure!(output.status.success(), "Process {pid} is unavailable");
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut identity = String::from_utf8(output.stdout)?.trim().to_owned();
    ensure!(!identity.is_empty(), "Process is gone");
    #[cfg(target_os = "linux")]
    if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
        let raw = exe.to_string_lossy();
        let clean = raw.strip_suffix(" (deleted)").unwrap_or(&raw);
        if !clean.is_empty() && !identity.contains(clean) {
            identity.push_str(" exe=");
            identity.push_str(clean);
        }
    }
    Ok(Process { pid, identity })
}
fn stop_process(p: &Process) -> Result<()> {
    if process(p.pid).is_ok_and(|v| v.identity == p.identity) {
        ensure!(
            canban_exec::raw_command("kill")
                .args(["-TERM", &p.pid.to_string()])
                .status()?
                .success(),
            "Could not close old UI"
        );
        let until = Instant::now() + Duration::from_secs(15);
        while process(p.pid).is_ok_and(|v| v.identity == p.identity) {
            ensure!(Instant::now() < until, "Old UI did not close");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(())
}

pub fn strip_deleted_suffix(path: &Path) -> PathBuf {
    if let Some(s) = path.to_str()
        && let Some(stripped) = s.strip_suffix(" (deleted)")
    {
        return PathBuf::from(stripped);
    }
    let lossy = path.to_string_lossy();
    if let Some(stripped) = lossy.strip_suffix(" (deleted)") {
        return PathBuf::from(stripped);
    }
    path.to_path_buf()
}

/// If `path` points inside a retired/staged backup bundle directory created by an earlier
/// in-place installation swap (e.g. `.canban-install-*/canban-previous/bin/canban` or
/// `.canban-previous-*/bin/canban`), map it to the corresponding binary inside sibling `canban/`
/// (or `Canban.app`).
pub fn map_retired_bundle_binary(path: &Path) -> Option<PathBuf> {
    let clean = strip_deleted_suffix(path);
    let comps: Vec<std::path::Component<'_>> = clean.components().collect();
    let mut prefix = PathBuf::new();
    for (idx, comp) in comps.iter().enumerate() {
        let std::path::Component::Normal(os) = comp else {
            prefix.push(comp.as_os_str());
            continue;
        };
        if let Some(name) = os.to_str() {
            if name.starts_with(".canban-install-") {
                let rest = &comps[idx + 1..];
                let Some(std::path::Component::Normal(inner_os)) = rest.first() else {
                    return None;
                };
                let inner_name = inner_os.to_str()?;
                let tail: PathBuf = rest[1..].iter().map(|c| c.as_os_str()).collect();
                if tail.as_os_str().is_empty() {
                    return None;
                }
                if inner_name.ends_with(".app") {
                    return Some(prefix.join("Canban.app").join(tail));
                }
                if matches!(inner_name, "canban-previous" | "canban" | "stage") {
                    return Some(prefix.join("canban").join(tail));
                }
            }
            let is_linux_retired = (name.starts_with(".canban-previous-")
                || name.starts_with(".canban-prev-")
                || name.starts_with(".canban-failed-")
                || name.starts_with(".canban-next-")
                || name == "canban-previous")
                && !name.ends_with(".app");
            if is_linux_retired {
                let tail: PathBuf = comps[idx + 1..].iter().map(|c| c.as_os_str()).collect();
                if tail.as_os_str().is_empty() {
                    return None;
                }
                return Some(prefix.join("canban").join(tail));
            }
            let is_macos_retired = (name.starts_with(".Canban-previous-")
                || name.starts_with(".Canban-failed-")
                || name.starts_with(".Canban-next-")
                || name == "Canban-previous.app")
                && name.ends_with(".app");
            if is_macos_retired {
                let tail: PathBuf = comps[idx + 1..].iter().map(|c| c.as_os_str()).collect();
                if tail.as_os_str().is_empty() {
                    return None;
                }
                return Some(prefix.join("Canban.app").join(tail));
            }
        }
        prefix.push(os);
    }
    None
}

fn canonicalize_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn default_canban_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| canonicalize_or_self(&h.join(".canban")))
}

#[cfg(not(target_os = "linux"))]
fn cmd_contains_bounded(cmd: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = cmd.as_bytes();
    let mut offset = 0;
    while let Some(pos) = cmd[offset..].find(needle) {
        let start = offset + pos;
        let end = start + needle.len();
        let before_ok = start == 0 || bytes[start - 1].is_ascii_whitespace();
        let after_ok = end == bytes.len() || bytes[end].is_ascii_whitespace();
        if before_ok && after_ok {
            return true;
        }
        offset = start + 1;
    }
    false
}

#[cfg(not(target_os = "linux"))]
fn extract_env_var<'a>(tail: &'a str, key: &str) -> Option<&'a str> {
    let bytes = tail.as_bytes();
    let needle = format!("{key}=");
    let mut start = None;
    let mut offset = 0;
    while let Some(pos) = tail[offset..].find(&needle) {
        let idx = offset + pos;
        if idx == 0 || bytes[idx - 1].is_ascii_whitespace() {
            start = Some(idx + needle.len());
            break;
        }
        offset = idx + 1;
    }
    let val_start = start?;
    let rest = &tail[val_start..];
    let rest_bytes = rest.as_bytes();
    for (i, &b) in rest_bytes.iter().enumerate() {
        if b.is_ascii_whitespace() {
            let after_ws = rest[i..].trim_start();
            if let Some(eq_idx) = after_ws.find('=') {
                let ident = &after_ws[..eq_idx];
                if !ident.is_empty()
                    && (ident.as_bytes()[0].is_ascii_alphabetic() || ident.as_bytes()[0] == b'_')
                    && ident
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_')
                {
                    return Some(rest[..i].trim_end());
                }
            }
        }
    }
    Some(rest.trim_end())
}

#[cfg(not(target_os = "linux"))]
fn ps_command(pid: u32, include_env: bool) -> Option<String> {
    let mut args = vec!["-ww"];
    if include_env {
        args.push("-E");
    }
    let pid_str = pid.to_string();
    args.extend(["-p", &pid_str, "-o", "command="]);
    let out = canban_exec::raw_command("ps").args(&args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!s.is_empty()).then_some(s)
}

#[cfg(not(target_os = "linux"))]
fn env_matches_home(pid: u32, home: &Path, home_canon: &Path) -> bool {
    let Some(plain) = ps_command(pid, false) else {
        return false;
    };
    let Some(with_env) = ps_command(pid, true) else {
        return true;
    };
    let tail = with_env
        .strip_prefix(&plain)
        .map(str::trim_start)
        .unwrap_or("");
    if tail.is_empty() || !tail.contains('=') {
        return true;
    }
    if let Some(val) = extract_env_var(tail, "CANBAN_HOME") {
        let p = PathBuf::from(val);
        return p == home || canonicalize_or_self(&p) == home_canon;
    }
    default_canban_home().is_none_or(|d| d == home_canon)
}

/// Find a live local `canban-ui` process belonging to `bundle` and targeting `home`.
pub fn find_local_ui(bundle: &Path, home: &Path) -> Option<Process> {
    let ui_bin = crate::bundle_ui(bundle);
    let ui_canon = canonicalize_or_self(&ui_bin);
    let home_canon = canonicalize_or_self(home);
    let self_pid = std::process::id();

    #[cfg(target_os = "linux")]
    {
        let default_home = default_canban_home();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return None;
        };
        let mut best: Option<Process> = None;
        for entry in entries.flatten() {
            let pid_os = entry.file_name();
            let Some(pid_str) = pid_os.to_str() else {
                continue;
            };
            if !pid_str.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let Ok(pid) = pid_str.parse::<u32>() else {
                continue;
            };
            if pid == self_pid {
                continue;
            }
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid_str}/stat"))
                && let Some(rparen) = stat.rfind(')')
                && stat[rparen + 1..].trim_start().starts_with('Z')
            {
                continue;
            }
            let exe_matches = std::fs::read_link(format!("/proc/{pid_str}/exe"))
                .ok()
                .is_some_and(|exe| {
                    let clean = strip_deleted_suffix(&exe);
                    let mapped = map_retired_bundle_binary(&clean);
                    clean == ui_bin
                        || clean == ui_canon
                        || mapped.as_deref() == Some(ui_bin.as_path())
                        || mapped.as_deref() == Some(ui_canon.as_path())
                });
            let cmdline_matches = !exe_matches
                && std::fs::read(format!("/proc/{pid_str}/cmdline"))
                    .ok()
                    .is_some_and(|bytes| {
                        bytes.split(|&b| b == 0).any(|arg| {
                            !arg.is_empty()
                                && std::str::from_utf8(arg).ok().is_some_and(|s| {
                                    let p = Path::new(s);
                                    let mapped = map_retired_bundle_binary(p);
                                    p == ui_bin
                                        || p == ui_canon
                                        || mapped.as_deref() == Some(ui_bin.as_path())
                                        || mapped.as_deref() == Some(ui_canon.as_path())
                                })
                        })
                    });
            if !exe_matches && !cmdline_matches {
                continue;
            }
            if let Ok(env_bytes) = std::fs::read(format!("/proc/{pid_str}/environ")) {
                let mut proc_home: Option<PathBuf> = None;
                for kv in env_bytes.split(|&b| b == 0) {
                    if let Ok(s) = std::str::from_utf8(kv)
                        && let Some(val) = s.strip_prefix("CANBAN_HOME=")
                        && !val.is_empty()
                    {
                        proc_home = Some(canonicalize_or_self(Path::new(val)));
                        break;
                    }
                }
                match proc_home {
                    Some(ph) if ph == home_canon => {}
                    Some(_) => continue,
                    None if default_home.as_ref() == Some(&home_canon) => {}
                    None => continue,
                }
            }
            if let Ok(proc_info) = process(pid)
                && best.as_ref().is_none_or(|cur| proc_info.pid > cur.pid)
            {
                best = Some(proc_info);
            }
        }
        best
    }

    #[cfg(not(target_os = "linux"))]
    {
        let ui_str = ui_bin.to_string_lossy();
        let ui_canon_str = ui_canon.to_string_lossy();
        let Ok(out) = canban_exec::raw_command("ps")
            .args(["-ww", "-x", "-o", "pid=,state=,command="])
            .output()
        else {
            return None;
        };
        if !out.status.success() {
            return None;
        }
        let mut best: Option<Process> = None;
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some((pid_str, rest)) = trimmed.split_once(char::is_whitespace) else {
                continue;
            };
            let Ok(pid) = pid_str.trim().parse::<u32>() else {
                continue;
            };
            if pid == self_pid {
                continue;
            }
            let rest = rest.trim_start();
            let Some((state, cmd)) = rest.split_once(char::is_whitespace) else {
                continue;
            };
            if state.starts_with('Z') {
                continue;
            }
            let cmd = cmd.trim();
            if !cmd.contains("canban-ui") {
                continue;
            }
            let cmd_matches = cmd_contains_bounded(cmd, &ui_str)
                || cmd_contains_bounded(cmd, &ui_canon_str)
                || cmd.split_whitespace().any(|tok| {
                    tok == ui_str
                        || tok == ui_canon_str
                        || (tok.ends_with("/canban-ui")
                            && std::fs::canonicalize(tok).ok().as_deref()
                                == Some(ui_canon.as_path()))
                });
            if !cmd_matches || !env_matches_home(pid, home, &home_canon) {
                continue;
            }
            if let Ok(proc_info) = process(pid)
                && best.as_ref().is_none_or(|cur| proc_info.pid > cur.pid)
            {
                best = Some(proc_info);
            }
        }
        best
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default)]
    pub local: Option<local::Handoff>,
    pub token: String,
    pub phase: String,
    pub home: PathBuf,
    pub target: PathBuf,
    pub staged: PathBuf,
    pub previous: PathBuf,
    pub build: Build,
    pub schema_before: u32,
    #[serde(default)]
    pub before: Option<Build>,
    pub ui: Option<Process>,
    pub app_id: String,
    pub error: Option<String>,
    pub server: Option<Process>,
    pub candidate_ui: Option<Process>,
    #[serde(default)]
    pub recovery_ui: Option<Process>,
    #[serde(default)]
    pub systemd_managed: bool,
}
impl Journal {
    pub(crate) fn save(&self) -> Result<()> {
        atomic(
            &self.home.join("updates/install.json"),
            &serde_json::to_vec_pretty(self)?,
        )
    }
}
pub fn bundle_root(cli: &Path) -> Result<PathBuf> {
    let resolved = if std::fs::symlink_metadata(cli)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        std::fs::canonicalize(cli).unwrap_or_else(|_| cli.to_path_buf())
    } else {
        cli.to_path_buf()
    };
    if resolved
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        == Some("MacOS")
        && resolved
            .parent()
            .and_then(Path::parent)
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            == Some("Contents")
    {
        let path = resolved
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .context("CLI is not in an App bundle")?;
        ensure!(crate::is_app_bundle(path), "Update requires a packaged App");
        return Ok(path.to_path_buf());
    }
    if resolved
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        == Some("bin")
        && let Some(dir) = resolved.parent().and_then(Path::parent)
        && !crate::is_app_bundle(dir)
        && (dir.join("bundle.json").is_file()
            || (dir.join("bin/canban-ui").is_file() && dir.join("bin/canban-updater").is_file()))
    {
        return Ok(dir.to_path_buf());
    }
    let Some(path) = resolved
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
    else {
        bail!("CLI is not in an App bundle");
    };
    ensure!(crate::is_app_bundle(path), "Update requires a packaged App");
    bail!("Update requires a packaged App")
}
fn bundle(cli: &Path) -> Result<PathBuf> {
    bundle_root(cli)
}
/// Called on a detached background thread after the install intent is accepted.
pub fn prepare_locked(
    home: &Path,
    cli: &Path,
    ui: Option<Process>,
    app_id: String,
    _operation: File,
    client: Option<canban_github::Client>,
) -> Result<()> {
    ensure!(!handoff::active(home), "An upgrade is already pending");
    let release = load_state(home)
        .available
        .context("Check for an update first")?;
    let target = bundle(cli)?;
    let staged = feed::stage(home, &release, client.as_ref())?;
    let stage_job = staged.parent().map(Path::to_path_buf);
    let token = canban_types::token_id();
    let parent = target.parent().context("App parent missing")?;
    // Copy into the target's filesystem before shutdown, so bundle switches are renames.
    let candidate = parent.join(crate::next_bundle_name(&target, &token));
    let job_dir = home.join("updates/jobs").join(&token);
    let mut journal_written = false;
    let prep = (|| -> Result<()> {
        copy_bundle(&staged, &candidate)?;
        if crate::is_app_bundle(&candidate) {
            crate::quarantine::prepare(&candidate)?;
        } else {
            crate::feed::verify_bundle_tree(&candidate)?;
        }
        let helper = job_dir.join("canban-updater");
        std::fs::create_dir_all(&job_dir)?;
        std::fs::copy(crate::bundle_updater(&target), &helper)?;
        let before: Build = serde_json::from_value(
            handoff::rpc(&home.join("app.sock"), "app.ping", json!({}))?["build"].clone(),
        )?;
        let schema_before = before.schema;
        let ui = match ui {
            Some(orig) if process(orig.pid).is_ok_and(|p| p.identity == orig.identity) => {
                Some(orig)
            }
            Some(_) => find_local_ui(&target, home),
            None => None,
        };
        let j = Journal {
            local: None,
            token: token.clone(),
            phase: "prepared".into(),
            home: home.into(),
            target: target.clone(),
            staged: candidate.clone(),
            previous: parent.join(crate::previous_bundle_name(&target, &token)),
            build: Build {
                version: release.manifest.version,
                commit: release.manifest.commit,
                schema: release.manifest.schema,
                api: release.manifest.api,
                platform: release.manifest.platform,
            },
            schema_before,
            before: Some(before),
            ui,
            app_id,
            error: None,
            server: None,
            candidate_ui: None,
            recovery_ui: None,
            systemd_managed: false,
        };
        j.save()?;
        journal_written = true;
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.join("updates/updater.log"))?;
        use std::os::unix::process::CommandExt;
        canban_exec::raw_command(helper)
            .arg("--home")
            .arg(home)
            .stdout(log.try_clone()?)
            .stderr(log)
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()?;
        Ok(())
    })();
    if let Some(dir) = &stage_job {
        let _ = std::fs::remove_dir_all(dir);
    }
    if let Err(err) = prep {
        let _ = std::fs::remove_dir_all(&candidate);
        let _ = std::fs::remove_dir_all(&job_dir);
        if journal_written
            && let Ok(mut j) = handoff::journal(home)
            && j.token == token
        {
            j.phase = "rolled_back".into();
            j.error = Some(format!("{err:#}"));
            let _ = j.save();
        }
        return Err(err);
    }
    Ok(())
}
/// Atomically replace an installed desktop bundle (`Canban.app` or Linux `canban/` dir)
/// with a verified `staged` bundle directory in the same parent filesystem.
pub fn replace_bundle_in_place(target: &Path, staged: &Path, token: &str) -> Result<()> {
    let parent = target.parent().context("App parent missing")?;
    let candidate = parent.join(crate::next_bundle_name(target, token));
    let previous = parent.join(crate::previous_bundle_name(target, token));
    copy_bundle(staged, &candidate)?;
    if crate::is_app_bundle(&candidate) {
        crate::quarantine::prepare(&candidate)?;
    } else {
        crate::feed::verify_bundle_tree(&candidate)?;
    }
    std::fs::rename(target, &previous).with_context(|| {
        format!(
            "Could not move existing bundle {} aside for atomic swap",
            target.display()
        )
    })?;
    if let Err(err) = std::fs::rename(&candidate, target) {
        let _ = std::fs::rename(&previous, target);
        let _ = std::fs::remove_dir_all(&candidate);
        return Err(err).with_context(|| {
            format!(
                "Could not activate updated desktop bundle at {}",
                target.display()
            )
        });
    }
    #[cfg(target_os = "macos")]
    if crate::is_app_bundle(target) {
        refresh_bundle_registration(target);
    }
    File::open(parent)?.sync_all()?;
    let _ = std::fs::remove_dir_all(&previous);
    if let Some(stage_job) = staged.parent() {
        let _ = std::fs::remove_dir_all(stage_job);
    }
    Ok(())
}
fn copy_bundle(source: &Path, target: &Path) -> Result<()> {
    ensure!(!target.exists(), "Staged App already exists");
    std::fs::create_dir(target)?;
    for item in std::fs::read_dir(source)? {
        let item = item?;
        let to = target.join(item.file_name());
        let kind = item.file_type()?;
        if kind.is_dir() {
            copy_bundle(&item.path(), &to)?;
        } else {
            ensure!(kind.is_file(), "App contains a link or special file");
            std::fs::copy(item.path(), &to)?;
            File::open(&to)?.sync_all()?;
        }
    }
    File::open(target)?.sync_all()?;
    Ok(())
}
fn cli(j: &Journal) -> PathBuf {
    crate::bundle_cli(&j.target)
}
pub(super) fn snapshot_path(j: &Journal) -> PathBuf {
    if let Some(local) = &j.local {
        local.backup.clone()
    } else {
        j.home
            .join("updates/jobs")
            .join(&j.token)
            .join("database-before.sqlite")
    }
}
pub(super) fn capture_snapshot(home: &Path, backup: &Path) -> Result<()> {
    if !home.join("canban.sqlite").exists() || backup.exists() {
        return Ok(());
    }
    if let Some(parent) = backup.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = backup.with_extension("pending");
    if temporary.exists() {
        std::fs::remove_file(&temporary)?;
    }
    let _service = service_lock(home)?;
    canban_storage::migrations::backup(home, &temporary)?;
    std::fs::rename(&temporary, backup)?;
    if let Some(parent) = backup.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}
pub(super) fn restore_snapshot(home: &Path, backup: &Path, expected_schema: u32) -> Result<bool> {
    if !backup.exists() {
        return Ok(false);
    }
    let _service = service_lock(home)?;
    let staged = backup.with_extension("restore");
    if staged.exists() {
        std::fs::remove_file(&staged)?;
    }
    std::fs::copy(backup, &staged)?;
    File::open(&staged)?.sync_all()?;
    for suffix in ["-wal", "-shm"] {
        let path = home.join(format!("canban.sqlite{suffix}"));
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    std::fs::rename(staged, home.join("canban.sqlite"))?;
    File::open(home)?.sync_all()?;
    ensure!(
        canban_storage::migrations::database_version(home)? == expected_schema,
        "Database snapshot restoration failed"
    );
    Ok(true)
}
/// Removes transient staging/session files, deletes the committed pre-upgrade SQLite
/// snapshot, and prunes historical updater job/bundle directories so disk usage stays O(1).
pub(super) fn prune_update_artifacts(j: &Journal) {
    let backup = snapshot_path(j);
    let _ = std::fs::remove_file(&backup);
    let _ = std::fs::remove_file(backup.with_extension("pending"));
    let _ = std::fs::remove_file(backup.with_extension("restore"));

    let updates = j.home.join("updates");
    if let Ok(entries) = std::fs::read_dir(&updates) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with("ui-ready-") || name.starts_with("ui-saved-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let staging = updates.join("staging");
    if let Ok(entries) = std::fs::read_dir(&staging) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    let jobs = updates.join("jobs");
    if let Ok(entries) = std::fs::read_dir(&jobs) {
        for entry in entries.flatten() {
            if entry.file_name().to_str() != Some(j.token.as_str()) {
                let path = entry.path();
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(path);
                } else {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
    if j.local.is_none()
        && let Some(parent) = j.target.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        let keep_prev = crate::previous_bundle_name(&j.target, &j.token);
        let keep_failed = crate::failed_bundle_name(&j.target, &j.token);
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let is_macos_backup = (name.starts_with(".Canban-previous-")
                || name.starts_with(".Canban-failed-")
                || name.starts_with(".Canban-next-"))
                && name.ends_with(".app");
            let is_linux_backup = (name.starts_with(".canban-previous-")
                || name.starts_with(".canban-prev-")
                || name.starts_with(".canban-failed-")
                || name.starts_with(".canban-next-"))
                && !name.ends_with(".app");
            let is_managed_backup = if crate::is_app_bundle(&j.target) {
                is_macos_backup
            } else {
                is_linux_backup
            };
            if is_managed_backup && name != keep_prev && name != keep_failed {
                let path = entry.path();
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(path);
                } else {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
}
fn database(j: &Journal, binary: &Path, target: u32) -> Result<()> {
    let out = canban_exec::raw_command(binary)
        .arg("--home")
        .arg(&j.home)
        .args(["database", "--target", &target.to_string()])
        .env("CANBAN_UPGRADE_TOKEN", &j.token)
        .env_remove("CANBAN_ENDPOINT")
        .output()?;
    ensure!(
        out.status.success(),
        "Database migration to {target} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    ensure!(
        canban_storage::migrations::database_version(&j.home)? == target,
        "Database migration did not reach schema {target}"
    );
    Ok(())
}
fn wait_unlock(home: &Path) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        if service_lock(home).is_ok() {
            return Ok(());
        }
        ensure!(
            Instant::now() < until,
            "AppServer did not release its database/service lock"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn environment(cmd: &mut Command, j: &Journal) {
    cmd.env("CANBAN_HOME", &j.home)
        .env("CANBAN_UPGRADE_TOKEN", &j.token)
        .env("CANBAN_APP_ID", &j.app_id)
        .env_remove("CANBAN_ENDPOINT")
        .env_remove("CANBAN_CARD")
        .env_remove("CARD_ID")
        .env_remove("CANBAN_PROJECT")
        .env_remove("CANBAN_PROFILE_ID");
}
fn spawn_server(j: &Journal) -> Result<Child> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(j.home.join("server.log"))?;
    let mut cmd = canban_exec::raw_command(cli(j));
    cmd.arg("--home")
        .arg(&j.home)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    environment(&mut cmd, j);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    Ok(cmd.spawn()?)
}
fn spawn_ui(j: &Journal) -> Result<Child> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(j.home.join("updates/ui.log"))?;
    let mut cmd = canban_exec::raw_command(crate::bundle_ui(&j.target));
    cmd.stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    environment(&mut cmd, j);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    Ok(cmd.spawn()?)
}

#[cfg(any(target_os = "linux", test))]
fn tokenize_systemd_args(raw: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == '\\' && q == '"' {
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '\\' {
            if let Some(next) = chars.next() {
                cur.push(next);
            }
        } else if c.is_whitespace() {
            if !cur.is_empty() {
                tokens.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn unit_manages_home(unit_body: &str, user_home: &Path, target_home: &Path) -> bool {
    let norm = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let target_norm = norm(target_home);
    let default_norm = norm(&user_home.join(".canban"));
    let mut has_exec_start = false;
    let mut explicit_home: Option<PathBuf> = None;
    let mut env_home: Option<PathBuf> = None;
    for raw_line in unit_body.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("ExecStart=") {
            let args = tokenize_systemd_args(rest.trim());
            if args.is_empty() {
                continue;
            }
            has_exec_start = true;
            let mut iter = args.iter();
            while let Some(arg) = iter.next() {
                if arg == "--home" {
                    if let Some(val) = iter.next() {
                        explicit_home = Some(PathBuf::from(val));
                    }
                } else if let Some(val) = arg.strip_prefix("--home=") {
                    explicit_home = Some(PathBuf::from(val));
                }
            }
        } else if let Some(rest) = line.strip_prefix("Environment=") {
            for entry in tokenize_systemd_args(rest.trim()) {
                if let Some(val) = entry.strip_prefix("CANBAN_HOME=") {
                    env_home = Some(PathBuf::from(val));
                }
            }
        }
    }
    if !has_exec_start {
        return false;
    }
    if let Some(h) = explicit_home.or(env_home) {
        return norm(&h) == target_norm;
    }
    default_norm == target_norm
}

pub fn systemd_manages_home(home: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Some(user_home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return false;
        };
        let config_root = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| user_home.join(".config"));
        let unit_path = config_root.join("systemd/user/canban.service");
        let Ok(unit_body) = std::fs::read_to_string(unit_path) else {
            return false;
        };
        if !unit_manages_home(&unit_body, &user_home, home) {
            return false;
        }
        canban_exec::raw_command("systemctl")
            .args(["--user", "is-active", "--quiet", "canban.service"])
            .status()
            .is_ok_and(|s| s.success())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = home;
        false
    }
}

pub(super) fn stop_systemd_service() -> Result<()> {
    let status = canban_exec::raw_command("systemctl")
        .args(["--user", "stop", "canban.service"])
        .status()
        .context("Failed to execute systemctl --user stop canban.service")?;
    ensure!(
        status.success(),
        "systemctl --user stop canban.service failed with {status}"
    );
    Ok(())
}

struct SystemdTokenGuard;
impl SystemdTokenGuard {
    fn acquire(token: &str) -> Result<Self> {
        let status = canban_exec::raw_command("systemctl")
            .args([
                "--user",
                "set-environment",
                &format!("CANBAN_UPGRADE_TOKEN={token}"),
            ])
            .status()
            .context("Failed to set CANBAN_UPGRADE_TOKEN in systemd --user")?;
        ensure!(
            status.success(),
            "systemctl --user set-environment failed with {status}"
        );
        Ok(Self)
    }
}
impl Drop for SystemdTokenGuard {
    fn drop(&mut self) {
        let _ = canban_exec::raw_command("systemctl")
            .args(["--user", "unset-environment", "CANBAN_UPGRADE_TOKEN"])
            .status();
    }
}

fn health_systemd(j: &Journal, expected: &Build) -> Result<u32> {
    let until = Instant::now() + Duration::from_secs(30);
    let endpoint = j.home.join("app.sock");
    loop {
        if let Ok(ping) = handoff::rpc(&endpoint, "app.ping", json!({})) {
            let pid = u32::try_from(
                ping["pid"]
                    .as_u64()
                    .context("Candidate systemd AppServer did not report its PID")?,
            )?;
            let build: Build = serde_json::from_value(ping["build"].clone())?;
            ensure!(build == *expected, "AppServer build identity mismatch");
            handoff::rpc(&endpoint, "app.state", json!({}))
                .context("Candidate AppServer failed initial app.state check")?;
            return Ok(pid);
        }
        ensure!(
            Instant::now() < until,
            "Candidate systemd AppServer readiness timed out"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn health(j: &Journal, server: &mut Child, expected: &Build) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(30);
    let endpoint = j.home.join("app.sock");
    loop {
        ensure!(
            server.try_wait()?.is_none(),
            "Candidate AppServer exited before readiness"
        );
        if let Ok(ping) = handoff::rpc(&endpoint, "app.ping", json!({})) {
            ensure!(
                ping["pid"].as_u64() == Some(server.id() as u64),
                "A different service answered candidate readiness"
            );
            let build: Build = serde_json::from_value(ping["build"].clone())?;
            ensure!(build == *expected, "AppServer build identity mismatch");
            handoff::rpc(&endpoint, "app.state", json!({}))
                .context("Candidate AppServer failed initial app.state check")?;
            return Ok(());
        }
        ensure!(
            Instant::now() < until,
            "Candidate AppServer readiness timed out"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn state(j: &Journal, phase: &str) -> Result<()> {
    let mut s = load_state(&j.home);
    s.phase = phase.into();
    s.error = j.error.clone();
    save_state(&j.home, &s)
}
/// Re-running a journal after interruption chooses recovery, never blindly
/// replays physical installation steps.
pub fn run(home: &Path, recover: bool) -> Result<()> {
    let _upgrade = lock(home, "upgrade.lock")?;
    let mut j = handoff::journal(home)?;
    ensure!(j.home == home, "Upgrade journal home mismatch");
    if matches!(j.phase.as_str(), "complete" | "rolled_back") {
        return Ok(());
    }
    ensure!(
        j.phase != "manual_recovery" || recover,
        "Manual recovery required: {}",
        j.error.as_deref().unwrap_or("unknown")
    );
    if j.local.is_some() {
        return local::run(&mut j, recover);
    }
    if recover && matches!(j.phase.as_str(), "prepared" | "stopping") {
        let _ = std::fs::remove_dir_all(&j.staged);
        j.phase = "rolled_back".into();
        j.error = Some("Interrupted upgrade cancelled before database migration".into());
        j.save()?;
        state(&j, "error")?;
        prune_update_artifacts(&j);
        return Ok(());
    }
    if recover && j.phase == "manual_recovery" {
        j.phase = "recovering".into();
        j.save()?;
    }
    let result = if recover || j.phase != "prepared" {
        rollback(&mut j)
    } else {
        install(&mut j)
    };
    if let Err(error) = result {
        if j.phase == "complete" {
            return Err(error);
        }
        j.error = Some(format!("{error:#}"));
        j.save()?;
        if !recover && j.phase == "stopping" {
            // No migration/bundle switch has begun. Keep a responsive old UI
            // and service in place instead of creating a second authority.
            let _ = std::fs::remove_dir_all(&j.staged);
            j.phase = "rolled_back".into();
            j.save()?;
            state(&j, "error")?;
            prune_update_artifacts(&j);
            if j.ui.as_ref().is_some_and(|ui| process(ui.pid).is_err())
                && find_local_ui(&j.target, &j.home).is_none()
            {
                let _ = spawn_ui(&j);
            }
            return Ok(());
        }

        if recover || j.phase == "manual_recovery" {
            j.phase = "manual_recovery".into();
            j.save()?;
            state(&j, "manual_recovery")?;
            stop_children(&j);
            show_recovery(&mut j);
            return Err(error);
        }
        if let Err(recovery) = rollback(&mut j) {
            j.phase = "manual_recovery".into();
            j.error = Some(format!(
                "{}; recovery failed: {recovery:#}",
                j.error.unwrap_or_default()
            ));
            j.save()?;
            state(&j, "manual_recovery")?;
            stop_children(&j);
            show_recovery(&mut j);
            bail!("{}", j.error.unwrap_or_default());
        }
    }
    Ok(())
}
fn resolve_live_ui(j: &mut Journal) -> Result<Option<Process>> {
    match &j.ui {
        Some(ui) if process(ui.pid).is_ok_and(|p| p.identity == ui.identity) => {
            Ok(Some(ui.clone()))
        }
        Some(_) => {
            let replacement = find_local_ui(&j.target, &j.home);
            j.ui = replacement.clone();
            j.save()?;
            Ok(replacement)
        }
        None => Ok(None),
    }
}
fn install(j: &mut Journal) -> Result<()> {
    state(j, "installing")?;
    j.phase = "stopping".into();
    j.save()?;
    if resolve_live_ui(j)?.is_some() {
        let until = Instant::now() + Duration::from_secs(15);
        while !j
            .home
            .join("updates")
            .join(format!("ui-saved-{}", j.token))
            .exists()
        {
            if resolve_live_ui(j)?.is_none() {
                break;
            }
            ensure!(
                Instant::now() < until,
                "Old UI did not preserve its session"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        if let Some(ui) = resolve_live_ui(j)? {
            stop_process(&ui)?;
        }
    }
    if !j.systemd_managed && systemd_manages_home(&j.home) {
        j.systemd_managed = true;
        j.save()?;
    }
    if j.systemd_managed {
        stop_systemd_service()?;
        let _ = handoff::rpc(&j.home.join("app.sock"), "app.shutdown", json!({}));
    } else {
        handoff::rpc(&j.home.join("app.sock"), "app.shutdown", json!({}))?;
    }
    wait_unlock(&j.home)?;
    if j.home.join("canban.sqlite").exists() {
        j.schema_before = canban_storage::migrations::database_version(&j.home)?;
    }
    j.phase = "migrating".into();
    j.save()?;
    capture_snapshot(&j.home, &snapshot_path(j))?;
    database(j, &crate::bundle_cli(&j.staged), j.build.schema)?;
    j.phase = "swapping".into();
    j.save()?;
    std::fs::rename(&j.target, &j.previous)?;
    std::fs::rename(&j.staged, &j.target)?;
    #[cfg(target_os = "macos")]
    if crate::is_app_bundle(&j.target) {
        refresh_bundle_registration(&j.target);
    }
    File::open(j.target.parent().context("App parent missing")?)?.sync_all()?;
    j.phase = "starting".into();
    j.save()?;
    boot(j, &j.build.clone())?;
    j.phase = "complete".into();
    j.error = None;
    j.save()?;
    state(j, "current")?;
    let mut s = load_state(&j.home);
    s.available = None;
    s.progress = 0;
    save_state(&j.home, &s)?;
    prune_update_artifacts(j);
    // Journal commit opens normal business operations before flushing durable reports.
    let _ = std::fs::remove_file(j.home.join("updates/ui-session.json"));
    if let Err(e) = handoff::flush(&j.home) {
        eprintln!("Queued Agent reports await retry: {e:#}");
    }
    Ok(())
}
fn boot(j: &mut Journal, expected: &Build) -> Result<()> {
    let mut direct_server = if j.systemd_managed {
        let _token_guard = SystemdTokenGuard::acquire(&j.token)?;
        let status = canban_exec::raw_command("systemctl")
            .args(["--user", "start", "canban.service"])
            .status()
            .context("Failed to execute systemctl --user start canban.service")?;
        ensure!(
            status.success(),
            "systemctl --user start canban.service failed with {status}"
        );
        let pid = health_systemd(j, expected)?;
        j.server = Some(process(pid)?);
        j.save()?;
        None
    } else {
        let mut server = spawn_server(j)?;
        j.server = Some(process(server.id())?);
        j.save()?;
        health(j, &mut server, expected)?;
        j.server = Some(process(server.id())?);
        j.save()?;
        Some(server)
    };
    let ready = j.home.join("updates").join(format!("ui-ready-{}", j.token));
    let _ = std::fs::remove_file(&ready);
    let mut ui = spawn_ui(j)?;
    j.candidate_ui = Some(process(ui.id())?);
    j.save()?;
    let until = Instant::now() + Duration::from_secs(30);
    let ready = j.home.join("updates").join(format!("ui-ready-{}", j.token));
    loop {
        ensure!(
            ui.try_wait()?.is_none(),
            "Candidate UI exited before readiness"
        );
        if let Some(server) = &mut direct_server {
            ensure!(
                server.try_wait()?.is_none(),
                "Candidate service exited during UI startup"
            );
        } else if let Some(srv) = &j.server {
            ensure!(
                process(srv.pid).is_ok_and(|p| p.identity == srv.identity),
                "Candidate service exited during UI startup"
            );
        }
        if ready.exists() {
            let b: Build = serde_json::from_slice(&std::fs::read(&ready)?)?;
            ensure!(b == *expected, "UI version identity mismatch");
            break;
        }
        ensure!(Instant::now() < until, "Candidate UI readiness timed out");
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
fn rollback(j: &mut Journal) -> Result<()> {
    if let Some(ui) = j.recovery_ui.take() {
        stop_process(&ui)?;
    }
    if let Some(ui) = &j.candidate_ui {
        stop_process(ui)?;
    }
    if !j.systemd_managed && systemd_manages_home(&j.home) {
        j.systemd_managed = true;
        j.save()?;
    }
    if j.systemd_managed {
        let _ = stop_systemd_service();
    }
    // Never shut down an unrelated process found at a reused socket/PID.
    if let Some(server) = &j.server
        && process(server.pid).is_ok_and(|p| p.identity == server.identity)
    {
        let _ = handoff::rpc(&j.home.join("app.sock"), "app.shutdown", json!({}));
    }
    // A crash may have happened before the original service acknowledged shutdown.
    if j.server.is_none() {
        let _ = handoff::rpc(&j.home.join("app.sock"), "app.shutdown", json!({}));
    }
    wait_unlock(&j.home)?;
    let candidate = if j.previous.exists() && j.target.exists() {
        cli(j)
    } else {
        crate::bundle_cli(&j.staged)
    };
    if j.home.join("canban.sqlite").exists() {
        let current_schema = canban_storage::migrations::database_version(&j.home)?;
        // If schema 16+ short-ID path rewrites already ran, run candidate down first so
        // sync_paths_down restores on-disk Card/archive directories before restoring SQLite.
        if current_schema != j.schema_before && (j.schema_before < 16 && current_schema >= 16) {
            let _ = database(j, &candidate, j.schema_before);
        }
        let backup = snapshot_path(j);
        if backup.exists() {
            match restore_snapshot(&j.home, &backup, j.schema_before) {
                Ok(_) => {}
                Err(snapshot_err) => {
                    if canban_storage::migrations::database_version(&j.home)? != j.schema_before {
                        database(j, &candidate, j.schema_before).with_context(|| {
                            format!("snapshot restore failed ({snapshot_err:#}) and fallback down migration failed")
                        })?;
                    } else {
                        return Err(snapshot_err);
                    }
                }
            }
        } else if canban_storage::migrations::database_version(&j.home)? != j.schema_before {
            database(j, &candidate, j.schema_before)?;
        }
    }
    if j.previous.exists() {
        if j.target.exists() {
            let failed = j
                .target
                .with_file_name(crate::failed_bundle_name(&j.target, &j.token));
            ensure!(
                !failed.exists(),
                "Failed App preservation path already exists"
            );
            std::fs::rename(&j.target, failed)?;
        }
        std::fs::rename(&j.previous, &j.target)?;
        #[cfg(target_os = "macos")]
        if crate::is_app_bundle(&j.target) {
            refresh_bundle_registration(&j.target);
        }
        File::open(j.target.parent().context("App parent missing")?)?.sync_all()?;
    }
    j.phase = "restoring".into();
    j.save()?;
    let before = if let Some(build) = j.before.clone() {
        build
    } else {
        let output = canban_exec::raw_command(cli(j))
            .arg("build-info")
            .output()?;
        ensure!(
            output.status.success(),
            "Restored CLI identity is unavailable"
        );
        serde_json::from_slice(&output.stdout)?
    };
    boot(j, &before)?;
    j.phase = "rolled_back".into();
    j.save()?;
    state(j, "error")?;
    prune_update_artifacts(j);
    let _ = std::fs::remove_file(j.home.join("updates/ui-session.json"));
    if let Err(e) = handoff::flush(&j.home) {
        eprintln!("Queued Agent reports await retry: {e:#}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn refresh_bundle_registration(bundle: &Path) {
    if let Ok(file) = File::open(bundle) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
    let lsregister = Path::new(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
    );
    if lsregister.is_file() {
        let _ = canban_exec::raw_command(lsregister)
            .arg("-f")
            .arg(bundle)
            .output();
    }
}

fn stop_children(j: &Journal) {
    if let Some(ui) = &j.candidate_ui {
        let _ = stop_process(ui);
    }
    if j.systemd_managed {
        let _ = stop_systemd_service();
        let _ = wait_unlock(&j.home);
        return;
    }
    if let Some(server) = &j.server
        && process(server.pid).is_ok_and(|p| p.identity == server.identity)
    {
        let _ = handoff::rpc(&j.home.join("app.sock"), "app.shutdown", json!({}));
        let _ = wait_unlock(&j.home);
    }
}
fn show_recovery(j: &mut Journal) {
    let source = if j.previous.exists() {
        &j.previous
    } else {
        &j.target
    };
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(j.home.join("updates/ui.log"));
    if let Ok(log) = log {
        let mut command = canban_exec::raw_command(crate::bundle_ui(source));
        environment(&mut command, j);
        command
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap_or(log));
        if let Ok(child) = command.spawn() {
            j.recovery_ui = process(child.id()).ok();
            let _ = j.save();
        }
    }
}
#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn interrupted_preparation_cancels_without_opening_or_restoring_data() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(home.join("canban.sqlite"), b"must not open this file").unwrap();
        let j = Journal {
            local: None,
            token: "fixture".into(),
            phase: "prepared".into(),
            home: home.into(),
            target: home.join("Canban.app"),
            staged: home.join("next.app"),
            previous: home.join("old.app"),
            build: Build::current(),
            schema_before: 8,
            before: None,
            ui: None,
            app_id: "test.app".into(),
            error: None,
            server: None,
            candidate_ui: None,
            recovery_ui: None,
            systemd_managed: false,
        };
        j.save().unwrap();
        run(home, true).unwrap();
        assert_eq!(handoff::journal(home).unwrap().phase, "rolled_back");
        assert_eq!(
            std::fs::read(home.join("canban.sqlite")).unwrap(),
            b"must not open this file"
        );
    }

    #[test]
    fn snapshot_capture_restores_same_schema_or_migrated_data_and_cleans_wal() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        drop(canban_storage::Store::open(home).unwrap());
        let schema = canban_storage::migrations::database_version(home).unwrap();
        {
            let conn = rusqlite::Connection::open(home.join("canban.sqlite")).unwrap();
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE sentinel(value TEXT);
                 INSERT INTO sentinel VALUES('before-upgrade');",
            )
            .unwrap();
        }
        let backup = home.join("updates/jobs/tok-1/database-before.sqlite");
        capture_snapshot(home, &backup).unwrap();
        assert!(backup.is_file());

        // Simulate destructive candidate migration / same-schema data mutation + WAL residue.
        {
            let conn = rusqlite::Connection::open(home.join("canban.sqlite")).unwrap();
            conn.execute_batch(
                "DELETE FROM sentinel;
                 INSERT INTO sentinel VALUES('corrupted-by-candidate');
                 PRAGMA user_version = 999;",
            )
            .unwrap();
            // Keep uncheckpointed WAL pages around before closing.
        }
        std::fs::write(home.join("canban.sqlite-wal"), b"stale-wal-bytes").unwrap();
        std::fs::write(home.join("canban.sqlite-shm"), b"stale-shm-bytes").unwrap();

        assert!(restore_snapshot(home, &backup, schema).unwrap());
        assert!(!home.join("canban.sqlite-wal").exists());
        assert!(!home.join("canban.sqlite-shm").exists());
        assert_eq!(
            canban_storage::migrations::database_version(home).unwrap(),
            schema
        );
        let conn = rusqlite::Connection::open(home.join("canban.sqlite")).unwrap();
        let val: String = conn
            .query_row("SELECT value FROM sentinel", [], |r| r.get(0))
            .unwrap();
        assert_eq!(val, "before-upgrade");
    }

    #[test]
    fn prune_update_artifacts_bounds_snapshots_jobs_staging_and_bundle_backups() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let home = root.join("home");
        let apps = root.join("apps");
        std::fs::create_dir_all(home.join("updates/jobs/old-tok")).unwrap();
        std::fs::create_dir_all(home.join("updates/jobs/cur-tok")).unwrap();
        std::fs::create_dir_all(home.join("updates/staging/dl-1")).unwrap();
        std::fs::create_dir_all(apps.join("Canban.app")).unwrap();
        std::fs::create_dir_all(apps.join(".Canban-previous-old-tok.app")).unwrap();
        std::fs::create_dir_all(apps.join(".Canban-failed-old-tok.app")).unwrap();
        std::fs::create_dir_all(apps.join(".Canban-next-old-tok.app")).unwrap();
        std::fs::create_dir_all(apps.join(".Canban-previous-cur-tok.app")).unwrap();
        std::fs::create_dir_all(apps.join("Unrelated.app")).unwrap();

        std::fs::write(home.join("updates/jobs/old-tok/canban-updater"), b"old").unwrap();
        std::fs::write(
            home.join("updates/jobs/old-tok/database-before.sqlite"),
            b"old-db",
        )
        .unwrap();
        std::fs::write(home.join("updates/jobs/cur-tok/canban-updater"), b"cur").unwrap();
        std::fs::write(
            home.join("updates/jobs/cur-tok/database-before.sqlite"),
            b"cur-db",
        )
        .unwrap();
        std::fs::write(home.join("updates/ui-ready-old-tok"), b"{}").unwrap();
        std::fs::write(home.join("updates/ui-ready-cur-tok"), b"{}").unwrap();
        std::fs::write(home.join("updates/ui-saved-cur-tok"), b"ready").unwrap();
        std::fs::write(home.join("updates/staging/dl-1/download.tar.zst"), b"zst").unwrap();

        let j = Journal {
            local: None,
            token: "cur-tok".into(),
            phase: "complete".into(),
            home: home.clone(),
            target: apps.join("Canban.app"),
            staged: apps.join(".Canban-next-cur-tok.app"),
            previous: apps.join(".Canban-previous-cur-tok.app"),
            build: Build::current(),
            schema_before: 19,
            before: None,
            ui: None,
            app_id: "com.canban.test".into(),
            error: None,
            server: None,
            candidate_ui: None,
            recovery_ui: None,
            systemd_managed: false,
        };
        prune_update_artifacts(&j);

        assert!(
            !home
                .join("updates/jobs/cur-tok/database-before.sqlite")
                .exists()
        );
        assert!(home.join("updates/jobs/cur-tok/canban-updater").is_file());
        assert!(!home.join("updates/jobs/old-tok").exists());
        assert!(!home.join("updates/ui-ready-old-tok").exists());
        assert!(!home.join("updates/ui-ready-cur-tok").exists());
        assert!(!home.join("updates/ui-saved-cur-tok").exists());
        assert!(
            std::fs::read_dir(home.join("updates/staging"))
                .unwrap()
                .next()
                .is_none()
        );
        assert!(!apps.join(".Canban-previous-old-tok.app").exists());
        assert!(!apps.join(".Canban-failed-old-tok.app").exists());
        assert!(!apps.join(".Canban-next-old-tok.app").exists());
        assert!(apps.join(".Canban-previous-cur-tok.app").is_dir());
        assert!(apps.join("Canban.app").is_dir());
        assert!(apps.join("Unrelated.app").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn linux_bundle_root_resolves_symlinks_swaps_in_place_and_prunes_backups() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let opt = root.join(".local/opt");
        let bin_links = root.join(".local/bin");
        std::fs::create_dir_all(opt.join("canban/bin")).unwrap();
        std::fs::create_dir_all(&bin_links).unwrap();
        std::fs::write(
            opt.join("canban/bundle.json"),
            b"{\"app_id\":\"com.canban.desktop\"}\n",
        )
        .unwrap();
        for name in ["canban", "canban-ui", "canban-updater"] {
            std::fs::write(opt.join("canban/bin").join(name), b"v1").unwrap();
        }
        symlink(opt.join("canban/bin/canban"), bin_links.join("canban")).unwrap();

        let expected = std::fs::canonicalize(opt.join("canban")).unwrap();
        assert_eq!(
            bundle_root(&opt.join("canban/bin/canban")).unwrap(),
            opt.join("canban")
        );
        assert_eq!(bundle_root(&bin_links.join("canban")).unwrap(), expected);

        // Standalone headless CLI without bundle.json / sibling canban-ui is not a desktop bundle.
        let standalone_dir = root.join("headless/.local/bin");
        std::fs::create_dir_all(&standalone_dir).unwrap();
        std::fs::write(standalone_dir.join("canban"), b"headless").unwrap();
        assert!(bundle_root(&standalone_dir.join("canban")).is_err());

        // Atomic in-place Linux bundle replacement updates all sibling files together.
        let staged_job = root.join("home/updates/staging/job-1");
        let staged = staged_job.join("canban");
        std::fs::create_dir_all(staged.join("bin")).unwrap();
        std::fs::write(
            staged.join("bundle.json"),
            b"{\"app_id\":\"com.canban.desktop\",\"version\":\"20261116-2\"}\n",
        )
        .unwrap();
        for name in ["canban", "canban-ui", "canban-updater"] {
            std::fs::write(staged.join("bin").join(name), b"v2").unwrap();
        }
        replace_bundle_in_place(&opt.join("canban"), &staged, "tok-swap").unwrap();
        assert_eq!(std::fs::read(opt.join("canban/bin/canban")).unwrap(), b"v2");
        assert_eq!(
            std::fs::read(opt.join("canban/bin/canban-ui")).unwrap(),
            b"v2"
        );
        assert_eq!(
            std::fs::read(opt.join("canban/bin/canban-updater")).unwrap(),
            b"v2"
        );
        assert_eq!(std::fs::read(bin_links.join("canban")).unwrap(), b"v2");
        assert!(!opt.join(".canban-previous-tok-swap").exists());
        assert!(!staged_job.exists());

        // Pruning Linux backup directories preserves current token and unrelated directories.
        let home = root.join("home");
        std::fs::create_dir_all(home.join("updates/jobs/cur")).unwrap();
        std::fs::create_dir_all(opt.join(".canban-previous-old")).unwrap();
        std::fs::create_dir_all(opt.join(".canban-failed-old")).unwrap();
        std::fs::create_dir_all(opt.join(".canban-next-old")).unwrap();
        std::fs::create_dir_all(opt.join(".canban-previous-cur")).unwrap();
        std::fs::create_dir_all(opt.join("other-tool")).unwrap();
        let j = Journal {
            local: None,
            token: "cur".into(),
            phase: "complete".into(),
            home,
            target: opt.join("canban"),
            staged: opt.join(".canban-next-cur"),
            previous: opt.join(".canban-previous-cur"),
            build: Build::current(),
            schema_before: 19,
            before: None,
            ui: None,
            app_id: "com.canban.desktop".into(),
            error: None,
            server: None,
            candidate_ui: None,
            recovery_ui: None,
            systemd_managed: false,
        };
        prune_update_artifacts(&j);
        assert!(!opt.join(".canban-previous-old").exists());
        assert!(!opt.join(".canban-failed-old").exists());
        assert!(!opt.join(".canban-next-old").exists());
        assert!(opt.join(".canban-previous-cur").is_dir());
        assert!(opt.join("canban").is_dir());
        assert!(opt.join("other-tool").is_dir());
    }

    #[test]
    fn systemd_unit_home_matching_isolates_live_service_from_other_homes() {
        let dir = tempfile::tempdir().unwrap();
        let user_home = dir.path().join("user");
        let default_home = user_home.join(".canban");
        let custom_home = dir.path().join("isolated-home");
        std::fs::create_dir_all(&default_home).unwrap();
        std::fs::create_dir_all(&custom_home).unwrap();

        let generated_unit = format!(
            "[Service]\nEnvironment=\"PATH=/usr/bin\"\nExecStart=\"/home/u/.local/bin/canban\" --home \"{}\" serve\nRestart=always\n",
            default_home.display()
        );
        assert!(unit_manages_home(
            &generated_unit,
            &user_home,
            &default_home
        ));
        assert!(!unit_manages_home(
            &generated_unit,
            &user_home,
            &custom_home
        ));

        let implicit_home_unit =
            "[Service]\nExecStart=/home/u/.local/bin/canban serve --tailscale\n";
        assert!(unit_manages_home(
            implicit_home_unit,
            &user_home,
            &default_home
        ));
        assert!(!unit_manages_home(
            implicit_home_unit,
            &user_home,
            &custom_home
        ));

        let eq_flag_unit = format!(
            "[Service]\nExecStart=/usr/local/bin/canban --home={} serve\n",
            custom_home.display()
        );
        assert!(unit_manages_home(&eq_flag_unit, &user_home, &custom_home));
        assert!(!unit_manages_home(&eq_flag_unit, &user_home, &default_home));

        // Legacy Journal JSON without `systemd_managed` deserializes cleanly.
        let legacy = serde_json::json!({
            "token": "tok",
            "phase": "prepared",
            "home": default_home,
            "target": user_home.join(".local/opt/canban"),
            "staged": user_home.join(".local/opt/.canban-next-tok"),
            "previous": user_home.join(".local/opt/.canban-previous-tok"),
            "build": Build::current(),
            "schema_before": 19,
            "ui": null,
            "app_id": "com.canban.desktop",
            "error": null,
            "server": null,
            "candidate_ui": null,
        });
        let parsed: Journal = serde_json::from_value(legacy).unwrap();
        assert!(!parsed.systemd_managed);
    }

    #[cfg(unix)]
    #[test]
    fn find_local_ui_and_resolve_live_ui_rebind_across_ui_restart_and_filter_by_home() {
        use std::{io::Read as _, os::unix::fs::PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let home = root.join("home-a");
        let other_home = root.join("home-b");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&other_home).unwrap();

        let bundle = root.join("Canban.app");
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let build = Build::current();
        let cli_script = format!(
            "#!/bin/sh\nif [ \"$1\" = \"build-info\" ]; then printf '%s\\n' '{}'; exit 0; fi\nexit 1\n",
            serde_json::to_string(&build).unwrap()
        );
        std::fs::write(macos.join("canban"), cli_script).unwrap();
        std::fs::write(macos.join("canban-updater"), "#!/bin/sh\nexit 0\n").unwrap();
        // Use python3 so environment variables are visible via `ps -E` on macOS as well as `/proc` on Linux.
        std::fs::write(
            macos.join("canban-ui"),
            "#!/usr/bin/python3\nimport sys, time\nsys.stdout.write('ready\\n')\nsys.stdout.flush()\ntime.sleep(30)\n",
        )
        .unwrap();
        for name in ["canban", "canban-ui", "canban-updater"] {
            std::fs::set_permissions(macos.join(name), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        let spawn_ui = |target_home: &Path| {
            let mut child = canban_exec::raw_command(macos.join("canban-ui"))
                .env("CANBAN_HOME", target_home)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            if let Some(mut stdout) = child.stdout.take() {
                let mut buf = [0u8; 6];
                let _ = stdout.read_exact(&mut buf);
            }
            child
        };

        let mut other_ui = spawn_ui(&other_home);
        assert!(
            find_local_ui(&bundle, &home).is_none(),
            "UI running for home-b must not match home-a"
        );

        let mut old_ui = spawn_ui(&home);
        let old_proc = process(old_ui.id()).unwrap();
        let found_old = find_local_ui(&bundle, &home).expect("expected old UI for home-a");
        assert_eq!(found_old.pid, old_proc.pid);

        let mut new_ui = spawn_ui(&home);
        let new_pid = new_ui.id();
        let _ = old_ui.kill();
        let _ = old_ui.wait();

        let mut journal = Journal {
            local: None,
            token: "tok-rebind".into(),
            phase: "stopping".into(),
            home: home.clone(),
            target: bundle.clone(),
            staged: root.join(".Canban-next-tok-rebind.app"),
            previous: root.join(".Canban-previous-tok-rebind.app"),
            build,
            schema_before: canban_storage::migrations::CURRENT,
            before: None,
            ui: Some(old_proc),
            app_id: "com.canban.test".into(),
            error: None,
            server: None,
            candidate_ui: None,
            recovery_ui: None,
            systemd_managed: false,
        };
        journal.save().unwrap();

        let rebound = resolve_live_ui(&mut journal)
            .unwrap()
            .expect("replacement UI should be rebound");
        assert_eq!(rebound.pid, new_pid);
        assert_eq!(
            handoff::journal(&home).unwrap().ui.map(|p| p.pid),
            Some(new_pid)
        );

        let _ = new_ui.kill();
        let _ = new_ui.wait();
        assert!(resolve_live_ui(&mut journal).unwrap().is_none());
        assert!(handoff::journal(&home).unwrap().ui.is_none());

        let _ = other_ui.kill();
        let _ = other_ui.wait();
    }

    #[cfg(unix)]
    #[test]
    fn prepare_locked_cleans_up_staging_candidate_and_job_on_post_stage_failure() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let home = root.join("home");
        std::fs::create_dir_all(home.join("updates")).unwrap();

        let bundle = root.join(if cfg!(target_os = "macos") {
            "Canban.app"
        } else {
            "canban"
        });
        let bin_dir = if cfg!(target_os = "macos") {
            bundle.join("Contents/MacOS")
        } else {
            bundle.join("bin")
        };
        std::fs::create_dir_all(&bin_dir).unwrap();
        let build = Build::current();
        if !cfg!(target_os = "macos") {
            std::fs::write(
                bundle.join("bundle.json"),
                b"{\"app_id\":\"com.canban.desktop\"}\n",
            )
            .unwrap();
        }
        let cli_script = format!(
            "#!/bin/sh\nif [ \"$1\" = \"build-info\" ]; then printf '%s\\n' '{}'; exit 0; fi\nexit 1\n",
            serde_json::to_string(&build).unwrap()
        );
        std::fs::write(bin_dir.join("canban"), cli_script.as_bytes()).unwrap();
        std::fs::write(bin_dir.join("canban-ui"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(bin_dir.join("canban-updater"), b"#!/bin/sh\nexit 0\n").unwrap();
        for name in ["canban", "canban-ui", "canban-updater"] {
            std::fs::set_permissions(bin_dir.join(name), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        let archive = root.join("update.tar.zst");
        feed::pack(&bundle, &archive).unwrap();
        let archive_bytes = std::fs::read(&archive).unwrap();
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let served = archive_bytes.clone();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = server.accept() {
                let mut req = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut req);
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                    served.len()
                );
                let _ = stream.write_all(hdr.as_bytes());
                let _ = stream.write_all(&served);
            }
        });

        let avail = feed::Available {
            manifest: feed::Manifest {
                version: build.version.clone(),
                commit: build.commit.clone(),
                platform: build.platform.clone(),
                api: build.api,
                schema: build.schema,
                rollback_floor: 8,
                asset: format!("Canban-{}.tar.zst", build.platform),
                size: archive_bytes.len() as u64,
                sha256: feed::sha256_hex(&archive_bytes),
            },
            url: format!("http://{addr}/update.tar.zst"),
            notes: String::new(),
        };
        save_state(
            &home,
            &State {
                phase: "available".into(),
                available: Some(avail),
                ..Default::default()
            },
        )
        .unwrap();

        let op_lock = lock(&home, "operation.lock").unwrap();
        // `app.sock` does not exist, so `handoff::rpc(..., "app.ping")` fails after staging/copying.
        let err = prepare_locked(
            &home,
            &bin_dir.join("canban"),
            None,
            "com.canban.test".into(),
            op_lock,
            None,
        )
        .unwrap_err();
        let _ = handle.join();
        assert!(!err.to_string().is_empty());
        assert!(!handoff::active(&home));
        assert_eq!(
            std::fs::read_dir(home.join("updates/staging"))
                .unwrap()
                .count(),
            0
        );
        assert_eq!(
            std::fs::read_dir(home.join("updates/jobs"))
                .unwrap()
                .count(),
            0
        );
        let leftovers: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .flatten()
            .filter(|e| {
                let n = e.file_name();
                let s = n.to_string_lossy();
                s.starts_with(".Canban-next-") || s.starts_with(".canban-next-")
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "unexpected candidate leftovers: {leftovers:?}"
        );
    }
}
