//! Unified local/remote execution of Workflow `Script` steps and shared multi-layer `env` resolution.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
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
    pub card: Option<&'a Card>,
    pub workspace: Option<&'a WorkspaceBinding>,
    pub run_dir: &'a Path,
    pub workflow_run: Option<&'a WorkflowRun>,
    pub step_ref: Option<&'a str>,
    pub script_env_file: Option<&'a Path>,
    pub script_output_file: Option<&'a Path>,
}

pub(crate) fn cardless_run_dir(home: &Path, project_id: &str, run_id: &str) -> PathBuf {
    if is_global_project_id(project_id) {
        home.join("workflow-runs").join(run_id)
    } else {
        home.join("projects")
            .join(project_id)
            .join("workflow-runs")
            .join(run_id)
    }
}

pub(crate) fn cardless_run_source_trust(run: &WorkflowRun) -> Option<&str> {
    let Some(ev) = run.inputs.get("trigger_event").filter(|v| v.is_object()) else {
        return Some("local");
    };
    if let Some(trust) = ev["data"]["source_trust"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        return Some(trust);
    }
    if ev["data"]["origin"].as_str() == Some("github")
        || ev["kind"]
            .as_str()
            .is_some_and(|k| k.starts_with("github."))
    {
        if ev["data"]["self_action"] == true {
            Some("collaborator")
        } else {
            Some("external")
        }
    } else {
        Some("local")
    }
}

/// Shared dummy Project used for Global (`__global__`) cardless workflows.
pub(crate) fn global_workflow_project(home: &Path) -> Project {
    Project {
        id: GLOBAL_PROJECT_ID.into(),
        name: "Global".into(),
        directory: home.to_string_lossy().into_owned(),
        target_branch: None,
        workflow: Workflow::default(),
        github: None,
        detected_github: None,
        github_disconnected: false,
        delivery_mode: DeliveryMode::default(),
        agent_overrides: BTreeMap::new(),
        tags: Vec::new(),
        created_at: 0,
    }
}

/// Builds protected `CANBAN_*` context environment variables for an `agent` or `script` step.
pub(crate) fn build_step_context_env(ctx: StepEnvContext<'_>) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let workspace_dir = ctx
        .workspace
        .map(|w| w.path.as_str())
        .or_else(|| ctx.card.and_then(|c| c.worktree.as_deref()))
        .filter(|s| !s.is_empty())
        .unwrap_or(&ctx.project.directory);
    let branch = ctx
        .workspace
        .and_then(|w| w.branch.as_deref())
        .or_else(|| ctx.card.and_then(|c| c.branch.as_deref()))
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
    let source_trust = ctx
        .card
        .map(Card::trust_level)
        .or_else(|| ctx.workflow_run.and_then(cardless_run_source_trust))
        .unwrap_or("local");

    env.insert("CANBAN_CLI".into(), ctx.cli.to_string_lossy().into_owned());
    env.insert(
        "CANBAN_HOME".into(),
        ctx.exec_home.to_string_lossy().into_owned(),
    );
    env.insert("CANBAN_ENDPOINT".into(), ctx.endpoint.to_owned());
    env.insert("CANBAN_PROJECT".into(), ctx.project.id.clone());
    env.insert("CANBAN_PROJECT_DIR".into(), ctx.project.directory.clone());
    env.insert("CANBAN_WORKSPACE".into(), workspace_dir.to_owned());
    if let Some(card) = ctx.card {
        env.insert("CANBAN_CARD".into(), card.number.to_string());
        env.insert("CARD_ID".into(), card.id.clone());
        env.insert("CANBAN_CARD_TITLE".into(), card.title.clone());
        env.insert("CANBAN_STAGE".into(), card.stage.clone());
    }
    env.insert("CANBAN_BRANCH".into(), branch.to_owned());
    env.insert("CANBAN_BASE_BRANCH".into(), base_branch.to_owned());
    env.insert("CANBAN_SOURCE_TRUST".into(), source_trust.to_owned());
    env.insert(
        "CANBAN_RUN_DIR".into(),
        ctx.run_dir.to_string_lossy().into_owned(),
    );

    if let Some(gh) = &ctx.project.github
        && !gh.repository.is_empty()
    {
        env.insert("CANBAN_GITHUB_REPO".into(), gh.repository.clone());
    }

    if let Some(card) = ctx.card {
        if let Some(pr) = &card.pull_request {
            if !pr.repository.is_empty() {
                env.insert("CANBAN_GITHUB_REPO".into(), pr.repository.clone());
            }
            if pr.number > 0 {
                env.insert("CANBAN_PR_NUMBER".into(), pr.number.to_string());
            }
            if !pr.url.is_empty() {
                env.insert("CANBAN_PR_URL".into(), pr.url.clone());
            }
            if !pr.checks.is_empty() {
                env.insert("CANBAN_PR_CHECKS".into(), pr.checks.clone());
            }
            if let Some(url) = pr
                .checks_summary
                .as_ref()
                .and_then(|s| s.url.as_deref())
                .filter(|u| !u.is_empty())
            {
                env.insert("CANBAN_PR_CHECKS_URL".into(), url.to_owned());
            }
        }
        if let Some(source) = card.github_issue() {
            if !source.repository.is_empty() {
                env.entry("CANBAN_GITHUB_REPO".into())
                    .or_insert_with(|| source.repository.clone());
            }
            if card.is_github_pr() {
                if source.number > 0 {
                    env.entry("CANBAN_PR_NUMBER".into())
                        .or_insert_with(|| source.number.to_string());
                }
                if !source.url.is_empty() {
                    env.entry("CANBAN_PR_URL".into())
                        .or_insert_with(|| source.url.clone());
                }
            } else {
                if source.number > 0 {
                    env.entry("CANBAN_ISSUE_NUMBER".into())
                        .or_insert_with(|| source.number.to_string());
                }
                if !source.url.is_empty() {
                    env.entry("CANBAN_ISSUE_URL".into())
                        .or_insert_with(|| source.url.clone());
                }
            }
        }
    }

    if let Some(run) = ctx.workflow_run {
        env.insert("CANBAN_WORKFLOW".into(), run.workflow_id.clone());
        env.insert("CANBAN_RUN_ID".into(), run.id.clone());
        if let Some(step_id) = run.resolve_step_id(ctx.step_ref) {
            env.insert("CANBAN_STEP_ID".into(), step_id.to_owned());
        }
        if let Some(ev) = run.inputs.get("trigger_event").filter(|v| v.is_object()) {
            let kind = ev["kind"].as_str().unwrap_or("");
            if !kind.is_empty() {
                env.insert("CANBAN_TRIGGER_EVENT".into(), kind.to_owned());
            }
            let data = &ev["data"];
            if let Some(repo) = data["repository"].as_str().filter(|s| !s.is_empty()) {
                env.entry("CANBAN_GITHUB_REPO".into())
                    .or_insert_with(|| repo.to_owned());
            }
            let is_issue_event = kind.starts_with("github.issue.")
                || data["source_kind"].as_str() == Some("github.issue");
            let is_pr_event =
                kind.starts_with("github.pr.") || data["source_kind"].as_str() == Some("github.pr");
            if is_issue_event {
                if let Some(num) = data["number"].as_u64() {
                    env.entry("CANBAN_ISSUE_NUMBER".into())
                        .or_insert_with(|| num.to_string());
                }
                if let Some(url) = data["url"].as_str().filter(|s| !s.is_empty()) {
                    env.entry("CANBAN_ISSUE_URL".into())
                        .or_insert_with(|| url.to_owned());
                }
            } else if is_pr_event {
                if let Some(num) = data["number"].as_u64() {
                    env.entry("CANBAN_PR_NUMBER".into())
                        .or_insert_with(|| num.to_string());
                }
                if let Some(url) = data["url"].as_str().filter(|s| !s.is_empty()) {
                    env.entry("CANBAN_PR_URL".into())
                        .or_insert_with(|| url.to_owned());
                }
                if let Some(url) = data["checks_url"].as_str().filter(|s| !s.is_empty()) {
                    env.entry("CANBAN_PR_CHECKS_URL".into())
                        .or_insert_with(|| url.to_owned());
                }
            }
            if let Some(comment_id) = data["comment_id"]
                .as_u64()
                .or_else(|| data["item_id"].as_u64())
            {
                env.entry("CANBAN_GITHUB_ITEM_ID".into())
                    .or_insert_with(|| comment_id.to_string());
            }
        }
    }
    if let Some(path) = ctx.script_env_file {
        env.insert("CANBAN_ENV".into(), path.to_string_lossy().into_owned());
    }
    if let Some(path) = ctx.script_output_file {
        env.insert("CANBAN_OUTPUT".into(), path.to_string_lossy().into_owned());
    }
    env
}

/// Builds the merged environment map for an `agent` or `script` step:
/// Layer 1 (`CANBAN_*` context) < Layer 2 (workflow `env`) < Layer 3 (`$CANBAN_ENV`) < Layer 4 (step `env`).
/// Returns `(full_env, custom_env_only)` where `custom_env_only` captures Layers 2–4 for PTY tracking.
#[allow(dead_code)]
pub(crate) fn build_step_env(
    ctx: StepEnvContext<'_>,
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut env = build_step_context_env(StepEnvContext {
        cli: ctx.cli,
        exec_home: ctx.exec_home,
        endpoint: ctx.endpoint,
        project: ctx.project,
        card: ctx.card,
        workspace: ctx.workspace,
        run_dir: ctx.run_dir,
        workflow_run: ctx.workflow_run,
        step_ref: ctx.step_ref,
        script_env_file: ctx.script_env_file,
        script_output_file: ctx.script_output_file,
    });
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
    let lines: Vec<&str> = contents
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
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
            ensure!(
                closed,
                "Unterminated multiline delimiter `{delim}` for `{key}`"
            );
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
        !key.is_empty()
            && !key.contains('=')
            && !key.contains('\0')
            && !key.chars().any(char::is_whitespace),
        "Invalid environment/output key `{key}`"
    );
    Ok(())
}

/// Resolves the command vector executing `script_path` under the configured `shell`.
pub(crate) fn shell_argv(shell: Option<&str>, script_path: &Path) -> Result<Vec<String>> {
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
        "bash" => Ok(vec![
            "bash".into(),
            "-eo".into(),
            "pipefail".into(),
            path_str,
        ]),
        "sh" => Ok(vec!["sh".into(), "-e".into(), path_str]),
        "zsh" => Ok(vec![
            "zsh".into(),
            "-eo".into(),
            "pipefail".into(),
            path_str,
        ]),
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

pub(crate) fn sanitized_step_slug(step_id: &str) -> String {
    let s: String = step_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() { "script".into() } else { s }
}

pub(crate) fn resolve_working_directory(
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
            && p.components().all(|c| !matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )),
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

fn read_local_text(path: &Path) -> Result<String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn log_tail(text: &str) -> String {
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
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    unset_keys: BTreeSet<String>,
    env_policy: canban_exec::shell_env::LocalEnvPolicy,
    argv: Vec<String>,
    script_file: PathBuf,
    env_file: PathBuf,
    output_file: PathBuf,
    raw_log_file: PathBuf,
    timeout_secs: u64,
    continue_on_error: bool,
}

pub(crate) fn script_timeout_secs(
    timeout_minutes: Option<u32>,
    timeout_seconds: Option<u64>,
) -> u64 {
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
pub(crate) fn kill_card_scripts_db(db: Db<'_>, client: &canban_herdr::Client, card_id: &str) {
    let Ok(all) = db.list::<WorkflowRun>() else {
        return;
    };
    for run in all
        .into_iter()
        .filter(|r| r.card_id.as_deref() == Some(card_id) && r.state == "waiting_script")
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

/// Best-effort termination for any active script on `run_id` using an already-held DB lock.
pub(crate) fn kill_run_scripts_db(db: Db<'_>, client: &canban_herdr::Client, run_id: &str) {
    let Ok(run) = db.get::<WorkflowRun>(run_id) else {
        return;
    };
    if run.state != "waiting_script" {
        return;
    }
    if let Some(addr) = run
        .inputs
        .get("script_terminal")
        .and_then(|v| v["address"].as_str())
    {
        let _ = client.call("terminal.kill", json!({ "terminal": addr }));
    }
}

struct ScriptOutcome<'a> {
    run_id: &'a str,
    step_id: &'a str,
    visit: u32,
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
        if let Some(notify) = self.script_wake.lock().unwrap().as_ref() {
            notify.notify_one();
        }
    }

    /// Wakes the background script worker in daemon mode; runs synchronously for in-process callers.
    pub(crate) fn request_scripts(&self) {
        if let Some(notify) = self.script_wake.lock().unwrap().clone() {
            notify.notify_one();
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
            let card = if let Some(card_id) = run.card_id.as_deref() {
                let card: Card = db.get(card_id)?;
                ensure!(card.active(), "Card was archived before running script");
                Some(card)
            } else {
                None
            };
            let project = if is_global_project_id(&run.project_id) {
                global_workflow_project(&self.home)
            } else {
                db.get::<Project>(&run.project_id)?
            };
            (project, card)
        };

        let directory = card
            .as_ref()
            .and_then(|c| c.workspace.as_ref().map(|w| w.directory.clone()))
            .unwrap_or_else(|| project.directory.clone());
        let ops = self.ops();

        let mut bound = project.clone();
        bound.workflow.workspace = run.definition.workspace.clone();
        bound.directory = directory.clone();

        let (ws_path, workspace) = if let Some(card) = &card {
            let mut prepared = card.clone();
            if let Some(existing) = &card.workspace
                && existing.directory == directory
            {
                let path = if bound.workflow.workspace.create_worktree || card.worktree.is_some() {
                    ops.resume(&self.home, &bound, &prepared)?
                } else {
                    PathBuf::from(&existing.path)
                };
                (path, Some(existing.clone()))
            } else {
                let pr_tracking = self.prepare_pr_card_source(&prepared, &mut bound, &ops)?;
                let path = ops.prepare(&self.home, &bound, &prepared)?;
                let path_str = path.to_string_lossy().into_owned();
                if card.worktree.as_deref() != Some(path_str.as_str()) {
                    self.git_changed();
                }
                prepared.worktree = Some(path_str.clone());
                let (branch, base_commit) = ops.workspace_head_meta(&path, &prepared);
                if let Some(target) = &pr_tracking
                    && path != Path::new(&directory)
                    && let Some(branch) = branch.as_deref().filter(|b| !b.is_empty())
                {
                    ops.configure_pr_tracking(&path, branch, target)?;
                }
                let binding = WorkspaceBinding {
                    directory: directory.clone(),
                    path: path_str,
                    branch,
                    base_commit,
                    target_branch: bound.target_branch.clone(),
                };
                self.store.lock().unwrap().write(|db| {
                    let mut current: Card = db.get(&card.id)?;
                    if current.workspace.as_ref() != Some(&binding) {
                        if let Some(old) = current.workspace.replace(binding.clone())
                            && old.directory != binding.directory
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
                            self.now_ms(),
                        )?;
                        db.save(&current)?;
                    }
                    Ok(())
                })?;
                (path, Some(binding))
            }
        } else {
            (PathBuf::from(&directory), None)
        };

        let cwd = resolve_working_directory(&ops.host, &ws_path, working_directory.as_deref())?;
        let scripts_dir = if let Some(card) = &card {
            canban_context::card_dir(&self.home, &card.project_id, card.number).join("scripts")
        } else {
            cardless_run_dir(&self.home, &run.project_id, &run.id).join("scripts")
        };
        std::fs::create_dir_all(&scripts_dir)?;

        let slug = sanitized_step_slug(step_id);
        let stem = format!("{slug}-v{}-{}", run.visit, self.next_id());
        let script_file = scripts_dir.join(format!("{stem}.sh"));
        let env_file = scripts_dir.join(format!("{stem}.env"));
        let output_file = scripts_dir.join(format!("{stem}.output"));
        let raw_log_file = scripts_dir.join(format!("{stem}.raw.log"));

        canban_context::atomic_with_durability(
            &script_file,
            command_body.as_bytes(),
            self.fs_durability,
        )?;
        canban_context::atomic_with_durability(&env_file, b"", self.fs_durability)?;
        canban_context::atomic_with_durability(&output_file, b"", self.fs_durability)?;
        canban_context::atomic_with_durability(&raw_log_file, b"", self.fs_durability)?;

        let protected = build_step_context_env(StepEnvContext {
            cli: &self.cli,
            exec_home: &self.home,
            endpoint: &self.endpoint().to_string_lossy(),
            project: &bound,
            card: card.as_ref(),
            workspace: workspace.as_ref(),
            run_dir: &scripts_dir,
            workflow_run: Some(run),
            step_ref: Some(step_id),
            script_env_file: Some(&env_file),
            script_output_file: Some(&output_file),
        });
        let config = self.config();
        let source_trust = card
            .as_ref()
            .map(Card::trust_level)
            .or_else(|| cardless_run_source_trust(run));
        let wf_env = run.custom_env_for_step(Some(step_id));
        let resolved =
            self.resolve_runtime_env(None, Some(&bound), &protected, Some(&wf_env), source_trust)?;
        let env_policy = canban_exec::shell_env::LocalEnvPolicy::for_source_trust(
            config.terminal_env_mode.is_inherit(),
            source_trust,
        );
        let argv = shell_argv(shell.as_deref(), &script_file)?;

        Ok(PreparedScript {
            cwd,
            env: resolved.env,
            unset_keys: resolved.unset_keys,
            env_policy,
            argv,
            script_file,
            env_file,
            output_file,
            raw_log_file,
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
            env_file: &prep.env_file,
            output_file: &prep.output_file,
            script_file: &prep.script_file,
            raw_log_path: Some(&prep.raw_log_file),
            fallback_text,
            exit_code,
            timed_out,
            timeout_secs: prep.timeout_secs,
            continue_on_error: prep.continue_on_error,
        })
    }

    fn finalize_script_run(&self, out: ScriptOutcome<'_>) -> Result<()> {
        let raw_log = out
            .raw_log_path
            .and_then(|p| read_local_text(p).ok())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| out.fallback_text.to_owned());
        let env_raw = read_local_text(out.env_file).unwrap_or_default();
        let out_raw = read_local_text(out.output_file).unwrap_or_default();

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
            let _ = std::fs::remove_file(f);
        }

        let parse_env_res = parse_key_value_file(&env_raw);
        let parse_out_res = parse_key_value_file(&out_raw);

        let config = self.config();
        let finalized: Option<Option<String>> = self.store.lock().unwrap().write(|db| {
            let mut current_run: WorkflowRun = db.get(out.run_id)?;
            if !current_run.active()
                || current_run.state != "waiting_script"
                || current_run.step.as_deref() != Some(out.step_id)
                || current_run.visit != out.visit
            {
                return Ok(None);
            }
            let slug = sanitized_step_slug(out.step_id);
            let mut card_opt = if let Some(card_id) = current_run.card_id.as_deref() {
                let card: Card = db.get(card_id)?;
                if !card.active() {
                    return Ok(None);
                }
                Some(card)
            } else {
                None
            };
            let project_for_env = if is_global_project_id(&current_run.project_id) {
                global_workflow_project(&self.home)
            } else {
                db.get::<Project>(&current_run.project_id)?
            };
            let source_trust = card_opt
                .as_ref()
                .map(Card::trust_level)
                .or_else(|| cardless_run_source_trust(&current_run));
            let wf_env = current_run.custom_env_for_step(Some(out.step_id));
            let resolved_secrets = crate::env::resolve_effective_env(
                db,
                &self.home,
                &config,
                None,
                Some(&project_for_env),
                &BTreeMap::new(),
                Some(&wf_env),
                source_trust,
            )?;
            let local_log = if let Some(card) = &card_opt {
                canban_context::card_dir(&self.home, &card.project_id, card.number)
                    .join("scripts")
                    .join(format!("{slug}-v{}.log", out.visit))
            } else {
                cardless_run_dir(&self.home, &current_run.project_id, &current_run.id)
                    .join("scripts")
                    .join(format!("{slug}-v{}.log", out.visit))
            };
            let mut log_body = raw_log.clone();
            if !resolved_secrets.skipped_untrusted_secrets.is_empty() {
                log_body.push_str(&format!(
                    "\n[canban] Skipped secret(s) for untrusted source ({}): {}\n",
                    source_trust.unwrap_or("external"),
                    resolved_secrets.skipped_untrusted_secrets.join(", ")
                ));
            }
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
            let log_body = crate::env::mask_secrets(&log_body, &resolved_secrets.secret_values);
            canban_context::atomic_with_durability(
                &local_log,
                log_body.as_bytes(),
                self.fs_durability,
            )?;
            let log_rel = canban_context::relative(&self.home, &local_log)?;

            let parse_error = parse_env_res
                .as_ref()
                .err()
                .or_else(|| parse_out_res.as_ref().err())
                .map(|e| format!("{e:#}"));
            let exported_env = parse_env_res.as_ref().ok().cloned().unwrap_or_default();
            let raw_outputs = parse_out_res.as_ref().ok().cloned().unwrap_or_default();
            let outputs: BTreeMap<String, String> = raw_outputs
                .into_iter()
                .map(|(k, v)| (k, crate::env::mask_secrets(&v, &resolved_secrets.secret_values)))
                .collect();

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
            let summary = crate::env::mask_secrets(&summary, &resolved_secrets.secret_values);

            let output_val = collect_script_outputs(
                &log_rel,
                &summary,
                out.exit_code,
                out.timed_out,
                out.continue_on_error,
                succeeded,
                &outputs,
            );
            let cid = card_opt.as_ref().map(|c| c.id.clone());
            let now_ms = self.now_ms();

            if let Some(card) = &mut card_opt {
                if succeeded || out.continue_on_error {
                    canban_workflow_engine::runtime::complete_script_at(
                        db,
                        &config,
                        card,
                        &mut current_run,
                        output_val,
                        &exported_env,
                        enqueue,
                        now_ms,
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
                    canban_workflow_engine::runtime::fail_script_at(
                        db,
                        &config,
                        card,
                        &mut current_run,
                        output_val,
                        &exported_env,
                        &summary,
                        &detail,
                        enqueue,
                        now_ms,
                    )?;
                }
            } else if succeeded || out.continue_on_error {
                canban_workflow_engine::runtime::complete_cardless_script_at(
                    db,
                    &mut current_run,
                    output_val,
                    &exported_env,
                    now_ms,
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
                canban_workflow_engine::runtime::fail_cardless_script_at(
                    db,
                    &mut current_run,
                    output_val,
                    &exported_env,
                    &summary,
                    &detail,
                    now_ms,
                )?;
            }
            Ok(Some(cid))
        })?;

        if let Some(cid) = finalized {
            let _ = self.pump_admissions();
            self.emit("application", cid);
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
        let client = self.herdr_client();

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

            if let Some(meta) = run.inputs.get("script_terminal").filter(|v| v.is_object()) {
                let Some(addr) = meta["address"].as_str() else {
                    continue;
                };
                let started_at = meta["started_at"].as_u64().unwrap_or(run.updated_at);
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

                if let Ok(actual_core) = self.verified_core_id()
                    && let Some(cid) = expected_core
                    && actual_core != cid
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

                let elapsed_ms = self.now_ms().saturating_sub(started_at);
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
                    let _ = self.finalize_script_error_only(
                        &run.id,
                        &step_id,
                        run.visit,
                        &msg,
                        continue_on_error,
                    );
                    continue;
                }
            };
            let core_id = self.verified_core_id().ok();
            let mut labels = BTreeMap::from([
                ("canban.project".into(), run.project_id.clone()),
                ("canban.script_run".into(), run.id.clone()),
                ("canban.script_step".into(), step_id.clone()),
            ]);
            if let Some(cid) = &run.card_id {
                labels.insert("canban.card_id".into(), cid.clone());
            }
            let wrapped = wrapped_herdr_argv(&prep.raw_log_file, &prep.argv);
            match client.launch_with_policy_unset_and_size(
                wrapped,
                &prep.cwd.to_string_lossy(),
                prep.env.clone(),
                labels,
                prep.env_policy,
                &prep.unset_keys,
                Some(self.resolve_launch_terminal_size(None)),
            ) {
                Ok(launched) => {
                    let addr = launched["address"].as_str().unwrap_or("").to_owned();
                    let _ = self.store.lock().unwrap().write(|db| {
                        let mut current: WorkflowRun = db.get(&run.id)?;
                        if current.state == "waiting_script"
                            && current.step.as_deref() == Some(&step_id)
                            && current.visit == run.visit
                        {
                            let started_at = self.now_ms();
                            current.inputs["script_terminal"] = json!({
                                "address": addr,
                                "core_id": core_id,
                                "env_file": prep.env_file,
                                "output_file": prep.output_file,
                                "script_file": prep.script_file,
                                "raw_log_file": prep.raw_log_file,
                                "started_at": started_at,
                            });
                            current.updated_at = started_at;
                            db.save(&current)?;
                        }
                        Ok(())
                    });
                    self.emit("application", run.card_id.clone());
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
                Some(WorkflowStep::Script {
                    continue_on_error: true,
                    ..
                })
            );
            let prep = match self.prepare_script(&run, &step_id) {
                Ok(p) => p,
                Err(e) => {
                    let msg = format!("Failed to prepare script `{step_id}`: {e:#}");
                    self.finalize_script_error_only(
                        &run.id,
                        &step_id,
                        run.visit,
                        &msg,
                        continue_on_error,
                    )?;
                    continue;
                }
            };
            let hc_env = self.config().hc_env;
            let (exit_code, timed_out) = run_local_process_with_policy_unset_and_runner(
                &prep.argv,
                &prep.cwd,
                &prep.env,
                &hc_env,
                prep.env_policy,
                &prep.unset_keys,
                &prep.raw_log_file,
                prep.timeout_secs,
                self.exec_runner.as_ref(),
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
        let finalized: Option<Option<String>> = self.store.lock().unwrap().write(|db| {
            let mut current_run: WorkflowRun = db.get(run_id)?;
            if !current_run.active()
                || current_run.state != "waiting_script"
                || current_run.step.as_deref() != Some(step_id)
                || current_run.visit != visit
            {
                return Ok(None);
            }
            let slug = sanitized_step_slug(step_id);
            let mut card_opt = if let Some(card_id) = current_run.card_id.as_deref() {
                Some(db.get::<Card>(card_id)?)
            } else {
                None
            };
            let local_log = if let Some(card) = &card_opt {
                canban_context::card_dir(&self.home, &card.project_id, card.number)
                    .join("scripts")
                    .join(format!("{slug}-v{visit}.log"))
            } else {
                cardless_run_dir(&self.home, &current_run.project_id, &current_run.id)
                    .join("scripts")
                    .join(format!("{slug}-v{visit}.log"))
            };
            canban_context::atomic_with_durability(
                &local_log,
                summary.as_bytes(),
                self.fs_durability,
            )?;
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
            let cid = card_opt.as_ref().map(|c| c.id.clone());
            let now_ms = self.now_ms();
            if let Some(card) = &mut card_opt {
                if continue_on_error {
                    canban_workflow_engine::runtime::complete_script_at(
                        db,
                        &config,
                        card,
                        &mut current_run,
                        output_val,
                        &BTreeMap::new(),
                        enqueue,
                        now_ms,
                    )?;
                } else {
                    canban_workflow_engine::runtime::fail_script_at(
                        db,
                        &config,
                        card,
                        &mut current_run,
                        output_val,
                        &BTreeMap::new(),
                        summary,
                        summary,
                        enqueue,
                        now_ms,
                    )?;
                }
            } else if continue_on_error {
                canban_workflow_engine::runtime::complete_cardless_script_at(
                    db,
                    &mut current_run,
                    output_val,
                    &BTreeMap::new(),
                    now_ms,
                )?;
            } else {
                canban_workflow_engine::runtime::fail_cardless_script_at(
                    db,
                    &mut current_run,
                    output_val,
                    &BTreeMap::new(),
                    summary,
                    summary,
                    now_ms,
                )?;
            }
            Ok(Some(cid))
        })?;
        if let Some(cid) = finalized {
            let _ = self.pump_admissions();
            self.emit("application", cid);
            self.request_deliveries();
            self.wake_runtime();
        }
        Ok(())
    }
}

#[allow(dead_code)]
pub(crate) fn run_local_process_with_policy(
    argv: &[String],
    cwd: &Path,
    env: &BTreeMap<String, String>,
    hc_env: &BTreeMap<String, String>,
    env_policy: canban_exec::shell_env::LocalEnvPolicy,
    log_path: &Path,
    timeout_secs: u64,
) -> Result<(Option<i64>, bool)> {
    run_local_process_with_policy_and_unset(
        argv,
        cwd,
        env,
        hc_env,
        env_policy,
        &BTreeSet::new(),
        log_path,
        timeout_secs,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_local_process_with_policy_and_unset(
    argv: &[String],
    cwd: &Path,
    env: &BTreeMap<String, String>,
    hc_env: &BTreeMap<String, String>,
    env_policy: canban_exec::shell_env::LocalEnvPolicy,
    unset_keys: &BTreeSet<String>,
    log_path: &Path,
    timeout_secs: u64,
) -> Result<(Option<i64>, bool)> {
    run_local_process_with_policy_unset_and_runner(
        argv,
        cwd,
        env,
        hc_env,
        env_policy,
        unset_keys,
        log_path,
        timeout_secs,
        &canban_exec::SystemRunner,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_local_process_with_policy_unset_and_runner(
    argv: &[String],
    cwd: &Path,
    env: &BTreeMap<String, String>,
    hc_env: &BTreeMap<String, String>,
    env_policy: canban_exec::shell_env::LocalEnvPolicy,
    unset_keys: &BTreeSet<String>,
    log_path: &Path,
    timeout_secs: u64,
    runner: &dyn canban_exec::CommandRunner,
) -> Result<(Option<i64>, bool)> {
    ensure!(!argv.is_empty(), "Empty script command");
    let mut merged_env = env.clone();
    canban_herdr::merge_local_env_with_policy_with_runner(
        &mut merged_env,
        hc_env,
        env_policy,
        runner,
    );
    for key in unset_keys {
        merged_env.remove(key);
    }

    if !runner.is_system() {
        use std::io::Write as _;
        let mut env_remove = vec![
            "GIT_DIR".into(),
            "GIT_WORK_TREE".into(),
            "GIT_INDEX_FILE".into(),
        ];
        env_remove.extend(unset_keys.iter().cloned());
        let spec = canban_exec::CommandSpec {
            program: argv[0].clone(),
            args: argv[1..].to_vec(),
            cwd: Some(cwd.to_path_buf()),
            env_set: merged_env,
            env_remove,
            stdin: None,
        };
        let out = runner.run(&spec)?;
        let mut log_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(log_path)?;
        log_file.write_all(&out.stdout)?;
        log_file.write_all(&out.stderr)?;
        return Ok((Some(i64::from(out.status_code)), false));
    }

    let stdout_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path)?;
    let stderr_file = stdout_file.try_clone()?;

    let mut cmd = canban_exec::command(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    for key in unset_keys {
        cmd.env_remove(key);
    }
    cmd.envs(&merged_env)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));

    let mut child = cmd
        .spawn()
        .with_context(|| format!("Failed spawning `{}`", argv[0]))?;
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
