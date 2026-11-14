//! Unified local/remote execution of Workflow `Script` steps and shared multi-layer `env` resolution.
use super::*;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_SCRIPT_TIMEOUT_SECS: u64 = 3600;
const LOG_TAIL_LINES: usize = 40;
const LOG_TAIL_MAX_CHARS: usize = 4000;

/// Context fields needed to compute Layer 1 (`CANBAN_*`) through Layer 4 (`step.env`) variables.
pub(crate) struct StepEnvContext<'a> {
    pub cli: &'a Path,
    pub exec_home: &'a Path,
    pub endpoint: &'a str,
    pub project: &'a Project,
    pub card: &'a Card,
    pub workspace: Option<&'a WorkspaceBinding>,
    pub run_dir: &'a Path,
    pub workflow_run: Option<&'a WorkflowRun>,
    pub step_ref: Option<&'a str>,
    pub script_env_file: Option<&'a Path>,
    pub script_output_file: Option<&'a Path>,
}

/// Builds the merged environment map for an `agent` or `script` step:
/// Layer 1 (`CANBAN_*` context) < Layer 2 (workflow `env`) < Layer 3 (`$CANBAN_ENV`) < Layer 4 (step `env`).
/// Returns `(full_env, custom_env_only)` where `custom_env_only` captures Layers 2–4 for PTY tracking.
pub(crate) fn build_step_env(
    ctx: StepEnvContext<'_>,
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut env = BTreeMap::new();
    let workspace_dir = ctx
        .workspace
        .map(|w| w.path.as_str())
        .or(ctx.card.worktree.as_deref())
        .filter(|s| !s.is_empty())
        .unwrap_or(&ctx.project.directory);
    let branch = ctx
        .workspace
        .and_then(|w| w.branch.as_deref())
        .or(ctx.card.branch.as_deref())
        .unwrap_or("");
    let base_branch = ctx
        .workspace
        .and_then(|w| w.target_branch.as_deref())
        .or(ctx.project.target_branch.as_deref())
        .or_else(|| {
            ctx.workflow_run
                .and_then(|r| r.definition.workspace.base_branch.as_deref())
        })
        .unwrap_or("");

    env.insert("CANBAN_CLI".into(), ctx.cli.to_string_lossy().into_owned());
    env.insert(
        "CANBAN_HOME".into(),
        ctx.exec_home.to_string_lossy().into_owned(),
    );
    env.insert("CANBAN_ENDPOINT".into(), ctx.endpoint.to_owned());
    env.insert("CANBAN_PROJECT".into(), ctx.project.id.clone());
    env.insert("CANBAN_PROJECT_DIR".into(), ctx.project.directory.clone());
    env.insert("CANBAN_WORKSPACE".into(), workspace_dir.to_owned());
    env.insert("CANBAN_CARD".into(), ctx.card.number.to_string());
    env.insert("CARD_ID".into(), ctx.card.id.clone());
    env.insert("CANBAN_CARD_TITLE".into(), ctx.card.title.clone());
    env.insert("CANBAN_BRANCH".into(), branch.to_owned());
    env.insert("CANBAN_BASE_BRANCH".into(), base_branch.to_owned());
    env.insert(
        "CANBAN_SOURCE_TRUST".into(),
        ctx.card.trust_level().to_owned(),
    );
    env.insert("CANBAN_STAGE".into(), ctx.card.stage.clone());
    env.insert(
        "CANBAN_RUN_DIR".into(),
        ctx.run_dir.to_string_lossy().into_owned(),
    );

    if let Some(run) = ctx.workflow_run {
        env.insert("CANBAN_WORKFLOW".into(), run.workflow_id.clone());
        env.insert("CANBAN_RUN_ID".into(), run.id.clone());
        if let Some(step_id) = run.resolve_step_id(ctx.step_ref) {
            env.insert("CANBAN_STEP_ID".into(), step_id.to_owned());
        }
    }
    if let Some(path) = ctx.script_env_file {
        env.insert("CANBAN_ENV".into(), path.to_string_lossy().into_owned());
    }
    if let Some(path) = ctx.script_output_file {
        env.insert("CANBAN_OUTPUT".into(), path.to_string_lossy().into_owned());
    }

    let custom = ctx
        .workflow_run
        .map(|run| run.custom_env_for_step(ctx.step_ref))
        .unwrap_or_default();
    for (k, v) in &custom {
        env.insert(k.clone(), v.clone());
    }
    (env, custom)
}

/// Parses GitHub Actions compatible `KEY=VALUE` and `KEY<<DELIMITER ... DELIMITER` files.
pub(crate) fn parse_key_value_file(contents: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let lines: Vec<&str> = contents.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        if let Some((raw_key, delim)) = line.split_once("<<")
            && !delim.is_empty()
            && !raw_key.contains('=')
        {
            let key = raw_key.trim();
            validate_env_key(key)?;
            let mut body = Vec::new();
            i += 1;
            let mut closed = false;
            while i < lines.len() {
                if lines[i] == delim {
                    closed = true;
                    i += 1;
                    break;
                }
                body.push(lines[i]);
                i += 1;
            }
            ensure!(closed, "Unterminated multiline delimiter `{delim}` for `{key}`");
            out.insert(key.to_owned(), body.join("\n"));
            continue;
        }
        let Some((raw_key, value)) = line.split_once('=') else {
            bail!("Invalid environment/output line (expected KEY=VALUE): `{line}`");
        };
        let key = raw_key.trim();
        validate_env_key(key)?;
        out.insert(key.to_owned(), value.to_owned());
        i += 1;
    }
    Ok(out)
}

fn validate_env_key(key: &str) -> Result<()> {
    ensure!(
        !key.is_empty() && !key.contains('=') && !key.contains('\0') && !key.chars().any(char::is_whitespace),
        "Invalid environment/output key `{key}`"
    );
    Ok(())
}

/// Resolves the command vector executing `script_path` under the configured `shell`.
fn shell_argv(shell: Option<&str>, script_path: &Path) -> Result<Vec<String>> {
    let path_str = script_path.to_string_lossy().into_owned();
    let Some(raw) = shell.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(vec![
            "/bin/sh".into(),
            "-c".into(),
            "if command -v bash >/dev/null 2>&1; then exec bash -eo pipefail \"$1\"; else exec sh -e \"$1\"; fi".into(),
            "canban-script".into(),
            path_str,
        ]);
    };
    let lower = raw.to_ascii_lowercase();
    match lower.as_str() {
        "bash" => Ok(vec!["bash".into(), "-eo".into(), "pipefail".into(), path_str]),
        "sh" => Ok(vec!["sh".into(), "-e".into(), path_str]),
        "zsh" => Ok(vec!["zsh".into(), "-eo".into(), "pipefail".into(), path_str]),
        "python" | "python3" => Ok(vec!["python3".into(), path_str]),
        _ => {
            let mut parts: Vec<String> = raw.split_whitespace().map(str::to_owned).collect();
            ensure!(!parts.is_empty(), "Empty shell specification");
            let mut substituted = false;
            for part in &mut parts {
                if part.contains("{0}") {
                    *part = part.replace("{0}", &path_str);
                    substituted = true;
                }
            }
            if !substituted {
                parts.push(path_str);
            }
            Ok(parts)
        }
    }
}

fn wrapped_herdr_argv(log_path: &Path, inner: &[String]) -> Vec<String> {
    let mut argv = vec![
        "/bin/sh".into(),
        "-c".into(),
        "exec >\"$1\" 2>&1; shift; exec \"$@\"".into(),
        "canban-script".into(),
        log_path.to_string_lossy().into_owned(),
    ];
    argv.extend(inner.iter().cloned());
    argv
}

fn sanitized_step_slug(step_id: &str) -> String {
    let s: String = step_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' })
        .collect();
    if s.is_empty() { "script".into() } else { s }
}

fn resolve_working_directory(
    host: &canban_herdr::host::Host,
    base: &Path,
    rel: Option<&str>,
) -> Result<PathBuf> {
    let Some(rel) = rel.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(base.to_path_buf());
    };
    let p = Path::new(rel);
    ensure!(
        !p.is_absolute()
            && !rel.starts_with('/')
            && !rel.starts_with('\\')
            && !rel.starts_with('~')
            && p.components()
                .all(|c| !matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_))),
        "Script working_directory `{rel}` must remain relative to the workspace"
    );
    let target = base.join(p);
    ensure!(
        host.is_dir(&target),
        "Script working_directory does not exist: {}",
        target.display()
    );
    Ok(target)
}

fn read_host_text(host: &canban_herdr::host::Host, path: &Path) -> Result<String> {
    if !host.remote() {
        return match std::fs::read(path) {
            Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(e.into()),
        };
    }
    if !host.exists(path) {
        return Ok(String::new());
    }
    let out = host.run(Path::new("/"), &["cat", &path.to_string_lossy()], None)?;
    ensure!(
        out.status.success(),
        "Failed reading remote file {}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn log_tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(LOG_TAIL_LINES);
    let joined = lines[start..].join("\n");
    if joined.len() <= LOG_TAIL_MAX_CHARS {
        return joined;
    }
    let mut cutoff = joined.len().saturating_sub(LOG_TAIL_MAX_CHARS);
    while cutoff < joined.len() && !joined.is_char_boundary(cutoff) {
        cutoff += 1;
    }
    format!("...\n{}", &joined[cutoff..])
}

struct PreparedScript {
    machine: String,
    exec_home: PathBuf,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    argv: Vec<String>,
    script_file: PathBuf,
    env_file: PathBuf,
    output_file: PathBuf,
    remote_log_file: PathBuf,
    timeout_secs: u64,
    continue_on_error: bool,
}

fn script_timeout_secs(timeout_minutes: Option<u32>, timeout_seconds: Option<u64>) -> u64 {
    timeout_seconds
        .or_else(|| timeout_minutes.map(|m| u64::from(m).saturating_mul(60)))
        .unwrap_or(DEFAULT_SCRIPT_TIMEOUT_SECS)
        .max(1)
}

fn collect_script_outputs(
    log_rel_path: &str,
    summary: &str,
    exit_code: Option<i64>,
    timed_out: bool,
    continue_on_error: bool,
    ok: bool,
    outputs: &BTreeMap<String, String>,
) -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in outputs {
        map.insert(k.clone(), json!(v));
    }
    map.insert("path".into(), json!(log_rel_path));
    map.insert("summary".into(), json!(summary));
    map.insert("exit_code".into(), json!(exit_code));
    map.insert("timed_out".into(), json!(timed_out));
    map.insert(
        "outcome".into(),
        json!(if ok {
            "success"
        } else if continue_on_error {
            "failed_continued"
        } else {
            "failed"
        }),
    );
    Value::Object(map)
}

/// Best-effort termination for any active scripts on `card_id` using an already-held DB lock.
pub(crate) fn kill_card_scripts_db(db: Db<'_>, config: &Config, card_id: &str) {
    let Ok(all) = db.list::<WorkflowRun>() else {
        return;
    };
    let client = canban_herdr::Client {
        config: config.clone(),
    };
    for run in all
        .into_iter()
        .filter(|r| r.card_id == card_id && r.state == "waiting_script")
    {
        if let Some(addr) = run
            .inputs
            .get("script_terminal")
            .and_then(|v| v["address"].as_str())
        {
            let _ = client.call("terminal.kill", json!({ "terminal": addr }));
        }
    }
}

struct ScriptOutcome<'a> {
    run_id: &'a str,
    step_id: &'a str,
    visit: u32,
    machine: &'a str,
    env_file: &'a Path,
    output_file: &'a Path,
    script_file: &'a Path,
    raw_log_path: Option<&'a Path>,
    fallback_text: &'a str,
    exit_code: Option<i64>,
    timed_out: bool,
    timeout_secs: u64,
    continue_on_error: bool,
}

impl AppServer {
    pub(super) fn wake_scripts(&self) {
        if let Some(tx) = self.script_wake.lock().unwrap().as_ref() {
            let _ = tx.try_send(());
        }
    }

    /// Wakes the background script worker in daemon mode; runs synchronously for in-process callers.
    pub(crate) fn request_scripts(&self) {
        if let Some(tx) = self.script_wake.lock().unwrap().clone() {
            let _ = tx.try_send(());
        } else {
            let _ = self.pump_scripts();
        }
    }

    pub(crate) fn pump_scripts(&self) -> Result<bool> {
        if self.observer_running.load(Ordering::SeqCst) {
            self.pump_scripts_herdr()
        } else {
            self.pump_scripts_inline()
        }
    }

    fn prepare_script(&self, run: &WorkflowRun, step_id: &str) -> Result<PreparedScript> {
        let Some(WorkflowStep::Script {
            run: command_body,
            shell,
            working_directory,
            timeout_minutes,
            timeout_seconds,
            continue_on_error,
            ..
        }) = run.definition.steps.get(step_id)
        else {
            bail!("Workflow step `{step_id}` is not a script");
        };

        let (project, card) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let card: Card = db.get(&run.card_id)?;
            ensure!(card.active(), "Card was archived before running script");
            let project: Project = db.get(&run.project_id)?;
            (project, card)
        };

        let location = card
            .workspace
            .as_ref()
            .map(|w| w.location.clone())
            .unwrap_or_else(|| project.work_location());
        if self.observer_running.load(Ordering::SeqCst) {
            self.resolve_machine(&location.machine, true)?;
        }
        let ops = self.ops(&location.machine);
        let remote = location.machine != "local";
        let exec_home = self.execution_home(&location.machine)?;

        let mut bound = project.clone();
        bound.workflow.workspace = run.definition.workspace.clone();
        bound.directory = location.directory.clone();
        bound.location = Some(location.clone());

        let mut prepared = card.clone();
        let (ws_path, workspace) = if let Some(existing) = &card.workspace
            && existing.location == location
        {
            let path = if bound.workflow.workspace.create_worktree || card.worktree.is_some() {
                ops.resume(&exec_home, &bound, &prepared)?
            } else {
                PathBuf::from(&existing.path)
            };
            (path, Some(existing.clone()))
        } else {
            let path = ops.prepare(&exec_home, &bound, &prepared)?;
            self.git_changed(&location.machine);
            prepared.worktree = Some(path.to_string_lossy().into_owned());
            let info = ops.info(&bound, &prepared).ok();
            let binding = WorkspaceBinding {
                location: location.clone(),
                path: path.to_string_lossy().into_owned(),
                branch: info.as_ref().and_then(|i| i.branch.clone()),
                base_commit: info.as_ref().and_then(|i| i.base_commit.clone()),
                target_branch: bound.target_branch.clone(),
            };
            self.store.lock().unwrap().write(|db| {
                let mut current: Card = db.get(&card.id)?;
                if current.workspace.as_ref() != Some(&binding) {
                    if let Some(old) = current.workspace.replace(binding.clone())
                        && old.location != binding.location
                    {
                        current.workspace_history.push(old);
                        current.delivery = None;
                    }
                    current.worktree = Some(binding.path.clone());
                    current.branch = binding.branch.clone();
                    current.base_commit = binding.base_commit.clone();
                    log(
                        db,
                        &current,
                        "workspace.prepared",
                        "system",
                        "Card workspace prepared",
                        None,
                        json!({"workspace": binding}),
                    )?;
                    db.save(&current)?;
                }
                Ok(())
            })?;
            (path, Some(binding))
        };

        let cwd = resolve_working_directory(&ops.host, &ws_path, working_directory.as_deref())?;
        let scripts_dir = canban_context::card_dir(&exec_home, &card.project_id, &card.id).join("scripts");
        ops.host.mkdir(&scripts_dir)?;

        let slug = sanitized_step_slug(step_id);
        let stem = format!("{slug}-v{}-{}", run.visit, id());
        let script_file = scripts_dir.join(format!("{stem}.sh"));
        let env_file = scripts_dir.join(format!("{stem}.env"));
        let output_file = scripts_dir.join(format!("{stem}.output"));
        let remote_log_file = scripts_dir.join(format!("{stem}.raw.log"));

        ops.host.write(&script_file.to_string_lossy(), command_body)?;
        ops.host.write(&env_file.to_string_lossy(), "")?;
        ops.host.write(&output_file.to_string_lossy(), "")?;
        ops.host.write(&remote_log_file.to_string_lossy(), "")?;

        let endpoint = if remote {
            format!("spool:{}", exec_home.join("rpc").display())
        } else {
            self.endpoint().to_string_lossy().into_owned()
        };
        let cli = if remote {
            exec_home.join("bin/canban")
        } else {
            self.cli.clone()
        };
        if remote {
            self.stage_remote(&ops.host, &exec_home, &card)?;
        }

        let (env, _) = build_step_env(StepEnvContext {
            cli: &cli,
            exec_home: &exec_home,
            endpoint: &endpoint,
            project: &bound,
            card: &card,
            workspace: workspace.as_ref(),
            run_dir: &scripts_dir,
            workflow_run: Some(run),
            step_ref: Some(step_id),
            script_env_file: Some(&env_file),
            script_output_file: Some(&output_file),
        });
        let argv = shell_argv(shell.as_deref(), &script_file)?;

        Ok(PreparedScript {
            machine: location.machine,
            exec_home,
            cwd,
            env,
            argv,
            script_file,
            env_file,
            output_file,
            remote_log_file,
            timeout_secs: script_timeout_secs(*timeout_minutes, *timeout_seconds),
            continue_on_error: *continue_on_error,
        })
    }

    fn finalize_prepared_script(
        &self,
        run: &WorkflowRun,
        step_id: &str,
        prep: &PreparedScript,
        fallback_text: &str,
        exit_code: Option<i64>,
        timed_out: bool,
    ) -> Result<()> {
        self.finalize_script_run(ScriptOutcome {
            run_id: &run.id,
            step_id,
            visit: run.visit,
            machine: &prep.machine,
            env_file: &prep.env_file,
            output_file: &prep.output_file,
            script_file: &prep.script_file,
            raw_log_path: Some(&prep.remote_log_file),
            fallback_text,
            exit_code,
            timed_out,
            timeout_secs: prep.timeout_secs,
            continue_on_error: prep.continue_on_error,
        })
    }

    fn finalize_script_run(&self, out: ScriptOutcome<'_>) -> Result<()> {
        let host = canban_herdr::host::Host::new(self.config(), out.machine);
        let raw_log = out
            .raw_log_path
            .and_then(|p| read_host_text(&host, p).ok())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| out.fallback_text.to_owned());
        let env_raw = read_host_text(&host, out.env_file).unwrap_or_default();
        let out_raw = read_host_text(&host, out.output_file).unwrap_or_default();

        // Best-effort scratch file cleanup on execution host.
        for f in [
            Some(out.env_file),
            Some(out.output_file),
            Some(out.script_file),
            out.raw_log_path,
        ]
        .into_iter()
        .flatten()
        {
            if !host.remote() {
                let _ = std::fs::remove_file(f);
            } else {
                let _ = host.run(Path::new("/"), &["rm", "-f", &f.to_string_lossy()], None);
            }
        }

        let parse_env_res = parse_key_value_file(&env_raw);
        let parse_out_res = parse_key_value_file(&out_raw);

        let config = self.config();
        let card_id = self.store.lock().unwrap().write(|db| {
            let mut current_run: WorkflowRun = db.get(out.run_id)?;
            if !current_run.active()
                || current_run.state != "waiting_script"
                || current_run.step.as_deref() != Some(out.step_id)
                || current_run.visit != out.visit
            {
                return Ok(None);
            }
            let mut card: Card = db.get(&current_run.card_id)?;
            if !card.active() {
                return Ok(None);
            }
            let slug = sanitized_step_slug(out.step_id);
            let local_log = canban_context::card_dir(&self.home, &card.project_id, &card.id)
                .join("scripts")
                .join(format!("{slug}-v{}.log", out.visit));
            let mut log_body = raw_log.clone();
            if out.timed_out {
                log_body.push_str(&format!(
                    "\n[canban] Script timed out after {}s\n",
                    out.timeout_secs
                ));
            }
            if let Err(ref e) = parse_env_res {
                log_body.push_str(&format!("\n[canban] Failed to parse $CANBAN_ENV: {e:#}\n"));
            }
            if let Err(ref e) = parse_out_res {
                log_body.push_str(&format!("\n[canban] Failed to parse $CANBAN_OUTPUT: {e:#}\n"));
            }
            canban_context::atomic(&local_log, log_body.as_bytes())?;
            let log_rel = canban_context::relative(&self.home, &local_log)?;

            let parse_error = parse_env_res
                .as_ref()
                .err()
                .or_else(|| parse_out_res.as_ref().err())
                .map(|e| format!("{e:#}"));
            let exported_env = parse_env_res.as_ref().ok().cloned().unwrap_or_default();
            let outputs = parse_out_res.as_ref().ok().cloned().unwrap_or_default();

            let succeeded = !out.timed_out && out.exit_code == Some(0) && parse_error.is_none();
            let summary = if let Some(err) = &parse_error {
                format!("Script `{}` produced invalid env/output: {err}", out.step_id)
            } else if out.timed_out {
                format!("Script `{}` timed out after {}s", out.step_id, out.timeout_secs)
            } else if out.exit_code == Some(0) {
                format!("Script `{}` succeeded", out.step_id)
            } else {
                let code = out
                    .exit_code
                    .map_or_else(|| "signal".into(), |c| c.to_string());
                format!("Script `{}` failed with exit code {code}", out.step_id)
            };

            let output_val = collect_script_outputs(
                &log_rel,
                &summary,
                out.exit_code,
                out.timed_out,
                out.continue_on_error,
                succeeded,
                &outputs,
            );
            let cid = card.id.clone();

            if succeeded || out.continue_on_error {
                canban_workflow_engine::runtime::complete_script(
                    db,
                    &config,
                    &mut card,
                    &mut current_run,
                    output_val,
                    &exported_env,
                    enqueue,
                )?;
            } else {
                let code_desc = if out.timed_out {
                    format!("timed out after {}s", out.timeout_secs)
                } else {
                    out.exit_code
                        .map_or_else(|| "killed by signal".into(), |c| c.to_string())
                };
                let tail = log_tail(&log_body);
                let detail = if tail.trim().is_empty() {
                    format!(
                        "Step: {}\nExit code: {code_desc}\nLog: {log_rel}",
                        out.step_id
                    )
                } else {
                    format!(
                        "Step: {}\nExit code: {code_desc}\nLog: {log_rel}\n\nLast output lines:\n{tail}",
                        out.step_id
                    )
                };
                canban_workflow_engine::runtime::fail_script(
                    db,
                    &config,
                    &mut card,
                    &mut current_run,
                    output_val,
                    &exported_env,
                    &summary,
                    &detail,
                    enqueue,
                )?;
            }
            Ok(Some(cid))
        })?;

        if let Some(cid) = card_id {
            let _ = self.pump_admissions();
            self.emit("application", Some(cid));
            self.request_deliveries();
            self.wake_runtime();
        }
        Ok(())
    }

    fn pump_scripts_herdr(&self) -> Result<bool> {
        let runs: Vec<WorkflowRun> = {
            let store = self.store.lock().unwrap();
            store
                .db()
                .list::<WorkflowRun>()?
                .into_iter()
                .filter(|r| r.active() && r.state == "waiting_script")
                .collect()
        };
        if runs.is_empty() {
            return Ok(false);
        }
        let client = canban_herdr::Client {
            config: self.config(),
        };

        for run in runs {
            let Some(step_id) = run.step.clone() else {
                continue;
            };
            let Some(WorkflowStep::Script {
                timeout_minutes,
                timeout_seconds,
                continue_on_error,
                ..
            }) = run.definition.steps.get(&step_id).cloned()
            else {
                continue;
            };
            let timeout_secs = script_timeout_secs(timeout_minutes, timeout_seconds);

            if let Some(meta) = run
                .inputs
                .get("script_terminal")
                .filter(|v| v.is_object())
            {
                let Some(addr) = meta["address"].as_str() else {
                    continue;
                };
                let started_at = meta["started_at"].as_u64().unwrap_or(run.updated_at);
                let machine = meta["machine"].as_str().unwrap_or("local");
                let env_file = PathBuf::from(meta["env_file"].as_str().unwrap_or(""));
                let out_file = PathBuf::from(meta["output_file"].as_str().unwrap_or(""));
                let script_file = PathBuf::from(meta["script_file"].as_str().unwrap_or(""));
                let raw_log_file = PathBuf::from(meta["raw_log_file"].as_str().unwrap_or(""));
                let expected_core = meta["core_id"].as_str();
                let finalize_meta =
                    |fallback_text: &str, exit_code: Option<i64>, timed_out: bool| {
                        let _ = self.finalize_script_run(ScriptOutcome {
                            run_id: &run.id,
                            step_id: &step_id,
                            visit: run.visit,
                            machine,
                            env_file: &env_file,
                            output_file: &out_file,
                            script_file: &script_file,
                            raw_log_path: Some(&raw_log_file),
                            fallback_text,
                            exit_code,
                            timed_out,
                            timeout_secs,
                            continue_on_error,
                        });
                    };

                if let Ok(status) = client.status()
                    && let Some(cid) = expected_core
                    && status["core"]["id"].as_str() != Some(cid)
                {
                    finalize_meta(
                        "Herdr Core identity changed before the script completed",
                        None,
                        false,
                    );
                    continue;
                }

                let state_val = match client.get(addr) {
                    Ok(v) => v,
                    Err(_) => {
                        finalize_meta("Herdr no longer retains the script terminal", None, false);
                        continue;
                    }
                };

                let elapsed_ms = now().saturating_sub(started_at);
                let is_finished = state_val["state"] == "finished";
                let timed_out = !is_finished && elapsed_ms >= timeout_secs.saturating_mul(1000);
                if timed_out {
                    let _ = client.call("terminal.kill", json!({ "terminal": addr }));
                    let scrollback = client
                        .call("terminal.read", json!({ "terminal": addr, "lines": 2000 }))
                        .ok()
                        .and_then(|v| v["text"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    finalize_meta(&scrollback, None, true);
                    continue;
                }
                if is_finished {
                    let scrollback = client
                        .call("terminal.read", json!({ "terminal": addr, "lines": 2000 }))
                        .ok()
                        .and_then(|v| v["text"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    let exit_code = state_val["exit_code"].as_i64();
                    finalize_meta(&scrollback, exit_code, false);
                }
                continue;
            }

            let prep = match self.prepare_script(&run, &step_id) {
                Ok(p) => p,
                Err(e) => {
                    let msg = format!("Failed to prepare script `{step_id}`: {e:#}");
                    let _ = self.finalize_script_error_only(&run.id, &step_id, run.visit, &msg, continue_on_error);
                    continue;
                }
            };
            let core_id = client
                .status()
                .ok()
                .and_then(|s| s["core"]["id"].as_str().map(str::to_owned));
            let labels = BTreeMap::from([
                ("canban.project".into(), run.project_id.clone()),
                ("canban.card_id".into(), run.card_id.clone()),
                ("canban.script_run".into(), run.id.clone()),
                ("canban.script_step".into(), step_id.clone()),
            ]);
            let wrapped = wrapped_herdr_argv(&prep.remote_log_file, &prep.argv);
            match client.launch_on(
                &prep.machine,
                wrapped,
                &prep.cwd.to_string_lossy(),
                prep.env.clone(),
                labels,
            ) {
                Ok(launched) => {
                    let addr = launched["address"].as_str().unwrap_or("").to_owned();
                    let _ = self.store.lock().unwrap().write(|db| {
                        let mut current: WorkflowRun = db.get(&run.id)?;
                        if current.state == "waiting_script"
                            && current.step.as_deref() == Some(&step_id)
                            && current.visit == run.visit
                        {
                            current.inputs["script_terminal"] = json!({
                                "address": addr,
                                "core_id": core_id,
                                "machine": prep.machine,
                                "exec_home": prep.exec_home,
                                "env_file": prep.env_file,
                                "output_file": prep.output_file,
                                "script_file": prep.script_file,
                                "raw_log_file": prep.remote_log_file,
                                "started_at": now(),
                            });
                            current.updated_at = now();
                            db.save(&current)?;
                        }
                        Ok(())
                    });
                    self.emit("application", Some(run.card_id.clone()));
                }
                Err(e) => {
                    let msg = format!("Failed to launch script `{step_id}`: {e:#}");
                    let _ = self.finalize_prepared_script(&run, &step_id, &prep, &msg, None, false);
                }
            }
        }
        let has_waiting = self
            .store
            .lock()
            .unwrap()
            .db()
            .list::<WorkflowRun>()?
            .into_iter()
            .any(|r| r.active() && r.state == "waiting_script");
        Ok(has_waiting)
    }

    /// Synchronous in-process runner for unit tests (`AppServer::open` without background daemons).
    fn pump_scripts_inline(&self) -> Result<bool> {
        for _ in 0..256 {
            let next_run = {
                let store = self.store.lock().unwrap();
                store
                    .db()
                    .list::<WorkflowRun>()?
                    .into_iter()
                    .find(|r| r.active() && r.state == "waiting_script")
            };
            let Some(run) = next_run else {
                return Ok(false);
            };
            let Some(step_id) = run.step.clone() else {
                break;
            };
            let continue_on_error = matches!(
                run.definition.steps.get(&step_id),
                Some(WorkflowStep::Script { continue_on_error: true, .. })
            );
            let prep = match self.prepare_script(&run, &step_id) {
                Ok(p) => p,
                Err(e) => {
                    let msg = format!("Failed to prepare script `{step_id}`: {e:#}");
                    self.finalize_script_error_only(&run.id, &step_id, run.visit, &msg, continue_on_error)?;
                    continue;
                }
            };
            let (exit_code, timed_out) = run_local_process(
                &prep.argv,
                &prep.cwd,
                &prep.env,
                &prep.remote_log_file,
                prep.timeout_secs,
            )?;
            self.finalize_prepared_script(&run, &step_id, &prep, "", exit_code, timed_out)?;
        }
        let has_waiting = self
            .store
            .lock()
            .unwrap()
            .db()
            .list::<WorkflowRun>()?
            .into_iter()
            .any(|r| r.active() && r.state == "waiting_script");
        Ok(has_waiting)
    }

    fn finalize_script_error_only(
        &self,
        run_id: &str,
        step_id: &str,
        visit: u32,
        summary: &str,
        continue_on_error: bool,
    ) -> Result<()> {
        let config = self.config();
        let card_id = self.store.lock().unwrap().write(|db| {
            let mut current_run: WorkflowRun = db.get(run_id)?;
            if !current_run.active()
                || current_run.state != "waiting_script"
                || current_run.step.as_deref() != Some(step_id)
                || current_run.visit != visit
            {
                return Ok(None);
            }
            let mut card: Card = db.get(&current_run.card_id)?;
            let slug = sanitized_step_slug(step_id);
            let local_log = canban_context::card_dir(&self.home, &card.project_id, &card.id)
                .join("scripts")
                .join(format!("{slug}-v{visit}.log"));
            canban_context::atomic(&local_log, summary.as_bytes())?;
            let log_rel = canban_context::relative(&self.home, &local_log)?;
            let output_val = collect_script_outputs(
                &log_rel,
                summary,
                None,
                false,
                continue_on_error,
                false,
                &BTreeMap::new(),
            );
            let cid = card.id.clone();
            if continue_on_error {
                canban_workflow_engine::runtime::complete_script(
                    db,
                    &config,
                    &mut card,
                    &mut current_run,
                    output_val,
                    &BTreeMap::new(),
                    enqueue,
                )?;
            } else {
                canban_workflow_engine::runtime::fail_script(
                    db,
                    &config,
                    &mut card,
                    &mut current_run,
                    output_val,
                    &BTreeMap::new(),
                    summary,
                    summary,
                    enqueue,
                )?;
            }
            Ok(Some(cid))
        })?;
        if let Some(cid) = card_id {
            let _ = self.pump_admissions();
            self.emit("application", Some(cid));
            self.request_deliveries();
            self.wake_runtime();
        }
        Ok(())
    }
}

fn run_local_process(
    argv: &[String],
    cwd: &Path,
    env: &BTreeMap<String, String>,
    log_path: &Path,
    timeout_secs: u64,
) -> Result<(Option<i64>, bool)> {
    ensure!(!argv.is_empty(), "Empty script command");
    let stdout_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path)?;
    let stderr_file = stdout_file.try_clone()?;

    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));

    let mut child = cmd.spawn().with_context(|| format!("Failed spawning `{}`", argv[0]))?;
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status.code().map(i64::from), false));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok((None, true));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
