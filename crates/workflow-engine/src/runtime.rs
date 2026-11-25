//! One durable interpreter for converted stage templates and user definitions.
use anyhow::{Context, Result, bail, ensure};
use canban_storage::Db;
use canban_types::*;
use serde_json::{Value, json};

fn condition_matches(actual: Option<&Value>, expected: &Value) -> bool {
    let Some(actual) = actual else {
        return expected.is_null();
    };
    if actual == expected {
        return true;
    }
    match (actual, expected) {
        (Value::String(s), Value::Bool(b)) | (Value::Bool(b), Value::String(s)) => {
            (s == "true" && *b) || (s == "false" && !*b)
        }
        (Value::String(s), Value::Number(n)) | (Value::Number(n), Value::String(s)) => {
            *s == n.to_string()
        }
        _ => false,
    }
}

fn merge_env_into_run(run: &mut WorkflowRun, exported_env: &BTreeMap<String, String>) {
    if exported_env.is_empty() {
        return;
    }
    let mut map = run
        .inputs
        .get("env")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    for (k, v) in exported_env {
        map.insert(k.clone(), json!(v));
    }
    run.inputs["env"] = Value::Object(map);
}
pub type Queue = for<'a> fn(
    Db<'a>,
    &Config,
    &mut Card,
    &Project,
    &Stage,
    ExecutionTrigger,
    Option<&str>,
    Option<String>,
) -> Result<Option<TerminalEntry>>;

/// Clock-injected queue adapter for `*_at` workflow runtime entrypoints.
pub type QueueAt = for<'a> fn(
    Db<'a>,
    &Config,
    &mut Card,
    &Project,
    &Stage,
    ExecutionTrigger,
    Option<&str>,
    Option<String>,
    u64,
) -> Result<Option<TerminalEntry>>;

/// Clock-aware queue callback accepted by `*_at` workflow runtime entrypoints.
///
/// Implemented automatically for any [`QueueAt`] function pointer or matching closure.
pub trait QueueHandler {
    #[allow(clippy::too_many_arguments)]
    fn enqueue(
        &mut self,
        db: Db<'_>,
        config: &Config,
        card: &mut Card,
        project: &Project,
        stage: &Stage,
        trigger: ExecutionTrigger,
        explicit: Option<&str>,
        context: Option<String>,
        now_ms: u64,
    ) -> Result<Option<TerminalEntry>>;
}

impl<F> QueueHandler for F
where
    F: for<'a> FnMut(
        Db<'a>,
        &Config,
        &mut Card,
        &Project,
        &Stage,
        ExecutionTrigger,
        Option<&str>,
        Option<String>,
        u64,
    ) -> Result<Option<TerminalEntry>>,
{
    fn enqueue(
        &mut self,
        db: Db<'_>,
        config: &Config,
        card: &mut Card,
        project: &Project,
        stage: &Stage,
        trigger: ExecutionTrigger,
        explicit: Option<&str>,
        context: Option<String>,
        now_ms: u64,
    ) -> Result<Option<TerminalEntry>> {
        self(
            db, config, card, project, stage, trigger, explicit, context, now_ms,
        )
    }
}

struct LegacyQueue(Queue);

impl QueueHandler for LegacyQueue {
    fn enqueue(
        &mut self,
        db: Db<'_>,
        config: &Config,
        card: &mut Card,
        project: &Project,
        stage: &Stage,
        trigger: ExecutionTrigger,
        explicit: Option<&str>,
        context: Option<String>,
        _now_ms: u64,
    ) -> Result<Option<TerminalEntry>> {
        (self.0)(db, config, card, project, stage, trigger, explicit, context)
    }
}

#[allow(clippy::too_many_arguments)]
fn log_at(
    db: Db<'_>,
    c: &Card,
    kind: &str,
    actor: &str,
    message: &str,
    terminal_id: Option<&str>,
    mut data: Value,
    now_ms: u64,
) -> Result<()> {
    if let Some(tid) = terminal_id
        && data.get("terminal_id").is_none()
    {
        data["terminal_id"] = json!(tid);
    }
    db.append(canban_timeline::event_at(
        &c.project_id,
        &c.id,
        kind,
        actor,
        message,
        data,
        now_ms,
    ))?;
    Ok(())
}
fn attention_at(db: Db<'_>, card: &mut Card, message: &str, now_ms: u64) -> Result<()> {
    card.attention = Some(message.into());
    if card.human_gate.is_none() {
        card.human_gate = Some(HumanGate {
            reason: message.into(),
            terminal_id: None,
            next_stage: None,
        });
    }
    db.save(&canban_notifications::attention_at(
        &card.project_id,
        &card.id,
        message,
        now_ms,
    ))?;
    Ok(())
}
use std::collections::BTreeMap;

pub fn latest(db: Db<'_>, id: &str) -> Result<WorkflowRevision> {
    db.list::<WorkflowRevision>()?
        .into_iter()
        .filter(|w| w.workflow_id == id)
        .max_by_key(|w| w.revision)
        .context("Workflow not found")
}
fn digest(db: Db<'_>, card: &Card) -> String {
    card_digest(db, card)
}
pub fn card_digest(db: Db<'_>, card: &Card) -> String {
    let source = card.github_issue().and_then(|r| {
        let key = format!(
            "github.com:{}:{}:{}",
            r.repository_id,
            if card.is_github_pr() { "pr" } else { "issue" },
            r.number
        );
        db.conn
            .query_row(
                "SELECT data FROM source_observations WHERE id=?1",
                [key],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|v| json!([v["title"], v["body"], v["head"]]))
    });
    canban_context::digest(
        &serde_json::to_vec(&json!([card.title, card.body, card.content, source])).unwrap(),
    )
}
fn record_at(db: Db<'_>, run: &WorkflowRun, kind: &str, message: &str, now_ms: u64) -> Result<()> {
    let actor = if kind == "workflow.approved" {
        "user"
    } else {
        "system"
    };
    let mut data =
        json!({"workflow_run_id":run.id,"step":run.step,"visit":run.visit,"state":run.state});
    if let Some(card_id) = &run.card_id {
        let card: Card = db.get(card_id)?;
        log_at(
            db,
            &card,
            kind,
            actor,
            message,
            run.terminal_id.as_deref(),
            data,
            now_ms,
        )
    } else {
        if let Some(tid) = run.terminal_id.as_deref() {
            data["terminal_id"] = json!(tid);
        }
        db.append(canban_timeline::event_at(
            &run.project_id,
            "",
            kind,
            actor,
            message,
            data,
            now_ms,
        ))?;
        Ok(())
    }
}
fn next_at(run: &mut WorkflowRun, to: Option<String>, now_ms: u64) {
    if let Some(step) = &run.step {
        run.history
            .push(json!({"step":step,"visit":run.visit,"event":"completed","at":now_ms}));
    }
    run.step = to;
    run.terminal_id = None;
    run.error = None;
    run.state = "running".into();
    run.visit += 1;
    run.updated_at = now_ms;
}

pub fn stop_card(db: Db<'_>, card: &str, state: &str) -> Result<()> {
    stop_card_at(db, card, state, now())
}

/// Pure clock-injected variant of [`stop_card`].
pub fn stop_card_at(db: Db<'_>, card: &str, state: &str, now_ms: u64) -> Result<()> {
    for mut run in db
        .list::<WorkflowRun>()?
        .into_iter()
        .filter(|r| r.card_id.as_deref() == Some(card) && r.active())
    {
        stop_active_run_at(db, &mut run, state, now_ms)?;
    }
    Ok(())
}

/// Stops a single active `WorkflowRun` (card-bound or cardless) by ID.
pub fn stop_run(db: Db<'_>, run_id: &str, state: &str) -> Result<()> {
    stop_run_at(db, run_id, state, now())
}

/// Pure clock-injected variant of [`stop_run`].
pub fn stop_run_at(db: Db<'_>, run_id: &str, state: &str, now_ms: u64) -> Result<()> {
    let mut run: WorkflowRun = db.get(run_id)?;
    if !run.active() {
        return Ok(());
    }
    stop_active_run_at(db, &mut run, state, now_ms)
}

fn stop_active_run_at(db: Db<'_>, run: &mut WorkflowRun, state: &str, now_ms: u64) -> Result<()> {
    run.state = state.into();
    run.updated_at = now_ms;
    for mut entry in db.list::<TerminalEntry>()? {
        let mut changed = false;
        if let Some(a) = &mut entry.agent {
            if a.workflow_run_id.as_deref() == Some(&run.id) && a.state == "queued" {
                a.state = "cancelled".into();
                changed = true;
            }
            if a.pending
                .as_ref()
                .is_some_and(|p| p.workflow_run_id.as_deref() == Some(&run.id))
            {
                a.pending = None;
                changed = true;
            }
        }
        if changed {
            db.save(&entry)?;
        }
    }
    db.save(run)?;
    record_at(db, run, "workflow.stopped", state, now_ms)?;
    Ok(())
}

fn build_run_inputs(
    db: Db<'_>,
    card: &Card,
    legacy: bool,
    adopted: bool,
    event_id: Option<&str>,
) -> Value {
    let mut inputs = json!({
        "title": card.title,
        "body": card.body,
        "content": card.content,
        "legacy": legacy,
    });
    if adopted {
        inputs["adopted"] = json!(true);
    }
    if let Some(eid) = event_id
        && let Ok(ev) = db.get::<Event>(eid)
    {
        inputs["trigger_event"] = json!({
            "id": ev.id,
            "kind": ev.kind,
            "actor": ev.actor,
            "message": ev.message,
            "data": ev.data,
        });
    }
    inputs
}

fn trigger_event_note(inputs: &Value) -> String {
    let Some(ev) = inputs.get("trigger_event").filter(|v| v.is_object()) else {
        return String::new();
    };
    let kind = ev["kind"].as_str().unwrap_or("");
    if !matches!(kind, "github.pr.ci_failed" | "github.pr.ci_succeeded") {
        return String::new();
    }
    let data = &ev["data"];
    let number = data["number"].as_u64().unwrap_or(0);
    let url = data["url"].as_str().unwrap_or("");
    let checks_url = data["checks_url"].as_str().unwrap_or("");
    let sha = data["head_sha"].as_str().unwrap_or("");
    let short_sha: String = sha.chars().take(10).collect();
    let mut lines = vec![format!(
        "Triggered by `{kind}` on PR #{number} ({short_sha}): {}",
        ev["message"].as_str().unwrap_or("")
    )];
    if !url.is_empty() {
        lines.push(format!("- PR: {url}"));
    }
    if !checks_url.is_empty() {
        lines.push(format!("- Checks: {checks_url}"));
    }
    if let Some(failed) = data["failed_checks"].as_array() {
        for item in failed.iter().take(10) {
            let name = item["name"].as_str().unwrap_or("check");
            let conclusion = item["conclusion"].as_str().unwrap_or("failure");
            if let Some(check_url) = item["url"].as_str().filter(|u| !u.is_empty()) {
                lines.push(format!(
                    "- Failed check `{name}` ({conclusion}): {check_url}"
                ));
            } else {
                lines.push(format!("- Failed check `{name}` ({conclusion})"));
            }
        }
    }
    format!("\n\n{}", lines.join("\n"))
}

#[allow(
    clippy::too_many_arguments,
    reason = "Transaction-scoped entry captures definition, causality and queue adapter"
)]
pub fn start(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    flow: WorkflowDefinition,
    revision: u64,
    entry: &str,
    event: Option<String>,
    legacy: bool,
    queue: Queue,
) -> Result<WorkflowRun> {
    start_at(
        db,
        config,
        card,
        flow,
        revision,
        entry,
        event,
        legacy,
        LegacyQueue(queue),
        now(),
    )
}

/// Pure clock-injected variant of [`start`].
#[allow(
    clippy::too_many_arguments,
    reason = "Transaction-scoped entry captures definition, causality, queue adapter and clock"
)]
pub fn start_at(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    flow: WorkflowDefinition,
    revision: u64,
    entry: &str,
    event: Option<String>,
    legacy: bool,
    mut queue: impl QueueHandler,
    now_ms: u64,
) -> Result<WorkflowRun> {
    ensure!(card.active(), "Restore the Card before starting a workflow");
    ensure!(
        !card.resolved(),
        "Resolved Cards do not start new workflows"
    );
    ensure!(
        card.human_gate.is_none(),
        "Resolve the existing human decision first"
    );
    ensure!(
        !db.list::<WorkflowRun>()?
            .iter()
            .any(|r| r.card_id.as_deref() == Some(card.id.as_str()) && r.active()),
        "Card already has an active workflow"
    );
    let step = if let Some(s) = flow.entries.get(entry) {
        s.clone()
    } else if flow.steps.contains_key(entry) {
        entry.to_owned()
    } else {
        bail!("Unknown workflow entry")
    };
    let inputs = build_run_inputs(db, card, legacy, false, event.as_deref());
    let mut run = WorkflowRun {
        id: id(),
        project_id: card.project_id.clone(),
        card_id: Some(card.id.clone()),
        workflow_id: flow.id.clone(),
        definition_revision: revision,
        definition: flow,
        state: "running".into(),
        step: Some(step),
        visit: 1,
        terminal_id: None,
        trigger_event_id: event,
        input_digest: digest(db, card),
        inputs,
        outputs: BTreeMap::new(),
        history: Vec::new(),
        error: None,
        created_at: now_ms,
        updated_at: now_ms,
    };
    db.save(&run)?;
    record_at(db, &run, "workflow.started", entry, now_ms)?;
    drive_at(db, config, card, &mut run, &mut queue, now_ms)?;
    Ok(run)
}

/// Starts `entry` by claiming the first `Agent` step reached directly for the caller's
/// Card Terminal or already-running Agent terminal (evaluating leading `Condition` / `Notify`
/// steps without enqueuing a duplicate Agent run). If the entry path pauses or finishes
/// at a non-`Agent` step (`Approval`, `Deliver`, `Admit`, `Complete`), it drives normally.
#[allow(
    clippy::too_many_arguments,
    reason = "Transaction-scoped adoption captures definition, optional live terminal and queue adapter"
)]
pub fn start_adopted(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    flow: WorkflowDefinition,
    revision: u64,
    entry: &str,
    event: Option<String>,
    legacy: bool,
    adopted_terminal: Option<&mut TerminalEntry>,
    queue: Queue,
) -> Result<WorkflowRun> {
    start_adopted_at(
        db,
        config,
        card,
        flow,
        revision,
        entry,
        event,
        legacy,
        adopted_terminal,
        LegacyQueue(queue),
        now(),
    )
}

/// Pure clock-injected variant of [`start_adopted`].
#[allow(
    clippy::too_many_arguments,
    reason = "Transaction-scoped adoption captures definition, optional live terminal, queue adapter and clock"
)]
pub fn start_adopted_at(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    flow: WorkflowDefinition,
    revision: u64,
    entry: &str,
    event: Option<String>,
    legacy: bool,
    adopted_terminal: Option<&mut TerminalEntry>,
    mut queue: impl QueueHandler,
    now_ms: u64,
) -> Result<WorkflowRun> {
    ensure!(card.active(), "Restore the Card before starting a workflow");
    ensure!(
        !card.resolved(),
        "Resolved Cards do not start new workflows"
    );
    ensure!(
        card.human_gate.is_none(),
        "Resolve the existing human decision first"
    );
    ensure!(
        !db.list::<WorkflowRun>()?
            .iter()
            .any(|r| r.card_id.as_deref() == Some(card.id.as_str()) && r.active()),
        "Card already has an active workflow"
    );
    let step = if let Some(s) = flow.entries.get(entry) {
        s.clone()
    } else if flow.steps.contains_key(entry) {
        entry.to_owned()
    } else {
        bail!("Unknown workflow entry")
    };
    let inputs = build_run_inputs(db, card, legacy, true, event.as_deref());
    let mut run = WorkflowRun {
        id: id(),
        project_id: card.project_id.clone(),
        card_id: Some(card.id.clone()),
        workflow_id: flow.id.clone(),
        definition_revision: revision,
        definition: flow,
        state: "running".into(),
        step: Some(step),
        visit: 1,
        terminal_id: None,
        trigger_event_id: event,
        input_digest: digest(db, card),
        inputs,
        outputs: BTreeMap::new(),
        history: Vec::new(),
        error: None,
        created_at: now_ms,
        updated_at: now_ms,
    };
    db.save(&run)?;
    record_at(db, &run, "workflow.started", entry, now_ms)?;
    let mut claimed = false;
    let mut adopted_terminal = adopted_terminal;
    while run.state == "running" {
        if run.visit > run.definition.max_steps {
            break;
        }
        let Some(key) = run.step.clone() else {
            break;
        };
        let Some(step) = run.definition.steps.get(&key).cloned() else {
            break;
        };
        match step {
            WorkflowStep::Condition {
                pointer,
                equals,
                then,
                otherwise,
            } => {
                run.history
                    .push(json!({"step":key,"visit":run.visit,"at":now_ms,"event":"entered"}));
                let mut card_val = serde_json::to_value(&*card)?;
                if let Some(obj) = card_val.as_object_mut() {
                    obj.insert("source_trust".into(), json!(card.trust_level()));
                }
                let context = json!({"inputs":run.inputs,"outputs":run.outputs,"card":card_val});
                next_at(
                    &mut run,
                    Some(if condition_matches(context.pointer(&pointer), &equals) {
                        then
                    } else {
                        otherwise
                    }),
                    now_ms,
                );
            }
            WorkflowStep::Notify { message, next: to } => {
                run.history
                    .push(json!({"step":key,"visit":run.visit,"at":now_ms,"event":"entered"}));
                db.save(&canban_notifications::attention_at(
                    &card.project_id,
                    &card.id,
                    &message,
                    now_ms,
                ))?;
                record_at(db, &run, "workflow.notified", &message, now_ms)?;
                next_at(&mut run, to, now_ms);
            }
            WorkflowStep::Admit { next: to, .. } => {
                // An already-live session holds its terminal; bypass capacity admission.
                run.history
                    .push(json!({"step":key,"visit":run.visit,"at":now_ms,"event":"entered"}));
                next_at(&mut run, to, now_ms);
            }
            WorkflowStep::Agent {
                next: continuation, ..
            } => {
                run.history
                    .push(json!({"step":key,"visit":run.visit,"at":now_ms,"event":"entered"}));
                let step_ref = format!("{key}:{}", run.visit);
                if let Some(entry_ref) = adopted_terminal.take() {
                    if let Some(agent) = &mut entry_ref.agent {
                        if matches!(
                            agent.state.as_str(),
                            "completed" | "backlog" | "blocked" | "failed"
                        ) {
                            agent.number = agent.number.saturating_add(1).max(1);
                            agent.state = "running".into();
                            agent.error = None;
                            agent.completed_at = None;
                            agent.completion_turn = None;
                            agent.turn_ended_at = None;
                            agent.close_requested_at = None;
                            agent.close_state.clear();
                            agent.lifecycle.close_reason.clear();
                            agent.lifecycle.interactive_followup = false;
                        }
                        if agent.trigger.is_comment() {
                            agent.trigger = ExecutionTrigger::Interactive;
                        }
                        agent.workflow_run_id = Some(run.id.clone());
                        agent.workflow_step = Some(step_ref);
                        agent.stage = card.stage.clone();
                        agent.updated_at = now_ms;
                        if legacy {
                            let following = run.definition.steps[&key]
                                .next()
                                .and_then(|n| run.definition.steps.get(n));
                            agent.require_human_review =
                                matches!(following, Some(WorkflowStep::Approval { .. }));
                            let movement = match following {
                                Some(WorkflowStep::Approval { next, .. }) => {
                                    next.as_ref().and_then(|n| run.definition.steps.get(n))
                                }
                                other => other,
                            };
                            agent.on_completed = match movement {
                                Some(WorkflowStep::Move { column, .. }) => Some(column.clone()),
                                _ => None,
                            };
                        }
                    }
                    db.save(entry_ref)?;
                    run.terminal_id = Some(entry_ref.id.clone());
                }
                run.state = "waiting_agent".into();
                run.inputs["title"] = json!(card.title);
                run.inputs["body"] = json!(card.body);
                run.input_digest = digest(db, card);
                run.inputs["pending_next"] = json!(continuation);
                run.updated_at = now_ms;
                db.save(&run)?;
                db.save(card)?;
                claimed = true;
                break;
            }
            _ => break,
        }
    }
    if !claimed {
        drive_at(db, config, card, &mut run, &mut queue, now_ms)?;
    }
    Ok(run)
}
fn drive_at<Q: QueueHandler>(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    queue: &mut Q,
    now_ms: u64,
) -> Result<()> {
    while run.state == "running" {
        if let Some(removed) = run.inputs["stop_after_step"].as_str()
            && run.step.as_deref() != Some(removed)
        {
            run.step = None;
            run.state = "cancelled".into();
            record_at(
                db,
                run,
                "workflow.stopped",
                "Step removed from workflow",
                now_ms,
            )?;
            break;
        }
        if run.visit > run.definition.max_steps {
            run.state = "failed".into();
            run.error = Some("Workflow execution limit reached".into());
            attention_at(db, card, run.error.as_deref().unwrap(), now_ms)?;
            break;
        }
        let Some(key) = run.step.clone() else {
            run.state = "completed".into();
            break;
        };
        let step = run
            .definition
            .steps
            .get(&key)
            .context("Workflow step not found")?
            .clone();
        run.history
            .push(json!({"step":key,"visit":run.visit,"at":now_ms,"event":"entered"}));
        match step {
            WorkflowStep::Agent {
                profile,
                instruction,
                next: continuation,
                ..
            } => {
                let project: Project = db.get(&card.project_id)?;
                let rework_label = match run.inputs["rework_origin"].as_str() {
                    Some("merge_conflict") => "Merge conflict to resolve before delivery",
                    Some("script_failure") => "Workflow script failure to fix",
                    _ => "Human rework feedback for this visit",
                };
                let rework_note = run.inputs["rework_feedback"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| format!("\n\n{rework_label}:\n{}", s.trim()))
                    .unwrap_or_default();
                if run.inputs.get("rework_feedback").is_some() {
                    run.inputs["rework_feedback"] = Value::Null;
                    run.inputs["rework_origin"] = Value::Null;
                }
                let prior_steps = if run.outputs.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n\nEarlier workflow step outputs:\n{}",
                        serde_json::to_string(&run.outputs)?
                    )
                };
                let trigger_note = trigger_event_note(&run.inputs);
                let stage = Stage {
                    id: card.stage.clone(),
                    name: card.stage.clone(),
                    execute: true,
                    profile,
                    instruction: format!("{instruction}{rework_note}{trigger_note}{prior_steps}"),
                    on_completed: None,
                    require_human_review: false,
                };
                let trigger = if run.inputs["legacy"] == true {
                    ExecutionTrigger::StageEntry
                } else {
                    ExecutionTrigger::Workflow
                };
                if let Some(mut entry) = queue.enqueue(
                    db, config, card, &project, &stage, trigger, None, None, now_ms,
                )? {
                    let step_ref = format!("{key}:{}", run.visit);
                    if let Some(agent) = &mut entry.agent {
                        let (require_human_review, on_completed) = if run.inputs["legacy"] == true {
                            let following = run.definition.steps[&key]
                                .next()
                                .and_then(|n| run.definition.steps.get(n));
                            let req_rev = matches!(following, Some(WorkflowStep::Approval { .. }));
                            let movement = match following {
                                Some(WorkflowStep::Approval { next, .. }) => {
                                    next.as_ref().and_then(|n| run.definition.steps.get(n))
                                }
                                other => other,
                            };
                            let on_done = match movement {
                                Some(WorkflowStep::Move { column, .. }) => Some(column.clone()),
                                _ => None,
                            };
                            (req_rev, on_done)
                        } else {
                            (false, None)
                        };
                        if let Some(p) = &mut agent.pending {
                            p.workflow_run_id = Some(run.id.clone());
                            p.workflow_step = Some(step_ref.clone());
                            if run.inputs["legacy"] == true {
                                p.require_human_review = require_human_review;
                                p.on_completed = on_completed;
                            }
                        } else {
                            agent.workflow_run_id = Some(run.id.clone());
                            agent.workflow_step = Some(step_ref.clone());
                            if run.inputs["legacy"] == true {
                                agent.require_human_review = require_human_review;
                                agent.on_completed = on_completed;
                            }
                        }
                    }
                    db.save(&entry)?;
                    db.conn.execute("UPDATE events SET data=json_set(data,'$.data.workflow_run_id',?2,'$.data.workflow_step',?3,'$.data.origin','workflow') WHERE json_extract(data,'$.data.terminal_id')=?1 AND json_extract(data,'$.kind')='execution.queued'",[entry.id.as_str(),run.id.as_str(),step_ref.as_str()])?;
                    run.terminal_id = Some(entry.id);
                    run.state = "waiting_agent".into();
                    run.inputs["title"] = json!(card.title);
                    run.inputs["body"] = json!(card.body);
                    run.input_digest = digest(db, card);
                    run.inputs["pending_next"] = json!(continuation);
                } else {
                    run.state = "blocked".into();
                    run.error = card.attention.clone();
                }
            }
            WorkflowStep::Script { next: to, .. } => {
                run.error = None;
                if let Some(obj) = run.inputs.as_object_mut() {
                    obj.remove("script_terminal");
                }
                run.inputs["pending_next"] = json!(to);
                run.inputs["script_started_at"] = json!(now_ms);
                run.input_digest = digest(db, card);
                run.state = "waiting_script".into();
                record_at(
                    db,
                    run,
                    "workflow.script_started",
                    &format!("Running script {key}"),
                    now_ms,
                )?;
            }
            WorkflowStep::Move { column, next: to } => {
                ensure!(
                    card.human_gate.is_none(),
                    "Human decision blocks automatic movement"
                );
                if card.stage != column {
                    let from = card.stage.clone();
                    card.stage = column;
                    card.board_order = None;
                    card.revision += 1;
                    canban_kanban::order::place(db, card, None)?;
                    db.save(card)?;
                    log_at(
                        db,
                        card,
                        "card.stage_changed",
                        "system",
                        &format!("{} → {}", from, card.stage),
                        run.terminal_id.as_deref(),
                        json!({"from":from,"to":card.stage,"entry_id":id(),"reason":"workflow_completed","workflow_run_id":run.id,"step":key}),
                        now_ms,
                    )?;
                }
                next_at(run, to, now_ms);
            }
            WorkflowStep::Approval { title, .. } => {
                attention_at(db, card, &title, now_ms)?;
                card.human_gate = Some(HumanGate {
                    reason: title.clone(),
                    terminal_id: run.terminal_id.clone(),
                    next_stage: run.definition.steps[&key].next().and_then(|k| {
                        match &run.definition.steps[k] {
                            WorkflowStep::Move { column, .. } => Some(column.clone()),
                            _ => None,
                        }
                    }),
                });
                card.revision += 1;
                db.save(card)?;
                run.state = "waiting_approval".into();
                record_at(db, run, "workflow.waiting_for_human", &title, now_ms)?;
            }
            WorkflowStep::Deliver {
                target_branch,
                strategy,
                sync_upstream,
                batch_size,
                test_command,
                next: to,
                ..
            } => {
                if card.hold_auto_merge {
                    let title = "Auto-merge held by user; approve or resume to merge";
                    attention_at(db, card, title, now_ms)?;
                    card.human_gate = Some(HumanGate {
                        reason: title.into(),
                        terminal_id: run.terminal_id.clone(),
                        next_stage: None,
                    });
                    card.revision += 1;
                    db.save(card)?;
                    if let Some(obj) = run.inputs.as_object_mut() {
                        obj.remove("queued_at");
                        obj.remove("queue_pause_reason");
                    }
                    run.state = "waiting_approval".into();
                    record_at(db, run, "workflow.waiting_for_human", title, now_ms)?;
                } else {
                    if run.inputs["queued_at"].as_u64().is_none() {
                        run.inputs["queued_at"] =
                            json!(next_merge_queued_at_ms(db, &card.project_id, now_ms)?);
                    }
                    if let Some(obj) = run.inputs.as_object_mut() {
                        obj.remove("queue_pause_reason");
                    }
                    run.inputs["pending_next"] = json!(to);
                    run.inputs["target_branch"] = json!(target_branch);
                    run.inputs["strategy"] = json!(strategy);
                    run.inputs["sync_upstream"] = json!(sync_upstream);
                    run.inputs["batch_size"] = json!(batch_size);
                    run.inputs["test_command"] = json!(test_command);
                    run.state = "waiting_delivery".into();
                }
            }
            WorkflowStep::Condition {
                pointer,
                equals,
                then,
                otherwise,
            } => {
                let mut card_val = serde_json::to_value(&*card)?;
                if let Some(obj) = card_val.as_object_mut() {
                    obj.insert("source_trust".into(), json!(card.trust_level()));
                }
                let context = json!({"inputs":run.inputs,"outputs":run.outputs,"card":card_val});
                next_at(
                    run,
                    Some(if condition_matches(context.pointer(&pointer), &equals) {
                        then
                    } else {
                        otherwise
                    }),
                    now_ms,
                );
            }
            WorkflowStep::Notify { message, next: to } => {
                db.save(&canban_notifications::attention_at(
                    &card.project_id,
                    &card.id,
                    &message,
                    now_ms,
                ))?;
                record_at(db, run, "workflow.notified", &message, now_ms)?;
                next_at(run, to, now_ms);
            }
            WorkflowStep::Admit { next: to, .. } => {
                // The AppServer admission pump releases runs in Board order.
                run.inputs["pending_next"] = json!(to);
                run.state = "waiting_capacity".into();
            }
            WorkflowStep::Complete => {
                run.history
                    .push(json!({"step":key,"visit":run.visit,"event":"completed","at":now_ms}));
                run.step = None;
                run.state = "completed".into();
            }
        }
    }
    run.updated_at = now_ms;
    db.save(run)?;
    db.save(card)?;
    if run.state == "completed" {
        record_at(db, run, "workflow.completed", "Workflow completed", now_ms)?;
    }
    Ok(())
}

pub fn reported(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    terminal: Option<&TerminalEntry>,
    report: &Report,
    queue: Queue,
) -> Result<()> {
    reported_at(
        db,
        config,
        card,
        terminal,
        report,
        LegacyQueue(queue),
        now(),
    )
}

/// Pure clock-injected variant of [`reported`].
pub fn reported_at(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    terminal: Option<&TerminalEntry>,
    report: &Report,
    mut queue: impl QueueHandler,
    now_ms: u64,
) -> Result<()> {
    let agent = terminal.and_then(|t| t.agent.as_ref());
    let run_opt: Option<WorkflowRun> = if let Some(a) = agent
        && let Some(id) = &a.workflow_run_id
    {
        Some(db.get(id)?)
    } else if let Some(id) = &report.workflow_run_id {
        Some(db.get(id)?)
    } else {
        db.list::<WorkflowRun>()?.into_iter().find(|r| {
            r.card_id.as_deref() == Some(card.id.as_str())
                && r.active()
                && matches!(r.state.as_str(), "waiting_agent" | "blocked")
        })
    };
    let Some(mut run) = run_opt else {
        return Ok(());
    };
    if !run.active() {
        return Ok(());
    }
    if let (Some(run_tid), Some(t)) = (&run.terminal_id, terminal)
        && run_tid != &t.id
    {
        return Ok(());
    }
    let term_id = terminal
        .map(|t| t.id.clone())
        .or_else(|| report.terminal_id.clone());
    let run_num = agent.map(|a| a.number);
    let on_done = agent.and_then(|a| a.on_completed.clone());
    if report.state != "completed" {
        if report.state == "backlog" {
            if let Some(key) = run.step.clone() {
                run.outputs.insert(
                    key.clone(),
                    json!({"report_id":report.id,"path":report.path,"message":report.message,"terminal_id":term_id,"run_number":run_num,"state":"backlog"}),
                );
                run.history.push(json!({"step":key,"visit":run.visit,"event":"returned_to_backlog","report_id":report.id,"at":now_ms}));
            }
            run.state = "cancelled".into();
            run.updated_at = now_ms;
            db.save(&run)?;
        } else if matches!(report.state.as_str(), "blocked" | "failed") {
            if run.step_is_deliver()
                && let Some(obj) = run.inputs.as_object_mut()
            {
                obj.remove("queued_at");
                obj.remove("queue_pause_reason");
            }
            run.state = "blocked".into();
            run.error = Some(report.message.clone());
            db.save(&run)?;
        }
        return Ok(());
    }
    if card.archived_at.is_some() || card.removed_at.is_some() {
        run.state = "cancelled".into();
        db.save(&run)?;
        return Ok(());
    }
    if digest(db, card) != run.input_digest {
        run.state = "stale".into();
        run.error = Some("Task content changed; rework or stop this workflow".into());
        attention_at(db, card, run.error.as_deref().unwrap(), now_ms)?;
        db.save(&run)?;
        record_at(db, &run, "workflow.stale", "Input changed", now_ms)?;
        return Ok(());
    }
    let key = run.step.clone().context("Missing Agent step")?;
    if matches!(
        run.definition.steps.get(&key),
        Some(WorkflowStep::Deliver { .. })
    ) {
        run.outputs.insert(
            key.clone(),
            json!({"report_id":report.id,"path":report.path,"message":report.message,"terminal_id":term_id,"run_number":run_num,"state":"conflict_resolved"}),
        );
        run.history.push(
            json!({"step":key,"visit":run.visit,"event":"conflict_resolved","report_id":report.id,"at":now_ms}),
        );
        let to = run.definition.steps[&key].next().map(str::to_owned);
        if let Some(gate) = &mut card.human_gate {
            if gate.terminal_id.is_none() {
                gate.terminal_id = term_id.clone();
            }
            gate.next_stage = on_done;
            if let Some(obj) = run.inputs.as_object_mut() {
                obj.remove("queued_at");
                obj.remove("queue_pause_reason");
            }
            run.state = "blocked".into();
            run.inputs["pending_next"] = json!(to);
            db.save(&run)?;
            return Ok(());
        }
        run.visit += 1;
        run.terminal_id = term_id;
        run.error = None;
        run.state = "running".into();
        return drive_at(db, config, card, &mut run, &mut queue, now_ms);
    }
    run.outputs.insert(key.clone(),json!({"report_id":report.id,"path":report.path,"message":report.message,"terminal_id":term_id,"run_number":run_num}));
    run.history.push(
        json!({"step":key,"visit":run.visit,"event":"completed","report_id":report.id,"at":now_ms}),
    );
    let to = run.definition.steps[&key].next().map(str::to_owned);
    // A previously reported block must still be approved even if the Agent later completes.
    if let Some(gate) = &mut card.human_gate {
        if gate.terminal_id.is_none() {
            gate.terminal_id = term_id.clone();
        }
        gate.next_stage = on_done;
        run.state = "blocked".into();
        run.inputs["pending_next"] = json!(to);
        db.save(&run)?;
        return Ok(());
    }
    next_at(&mut run, to, now_ms);
    // Keep the completing terminal as decision context when the next node is approval or delivery.
    if run.step.as_ref().is_some_and(|k| {
        matches!(
            run.definition.steps[k],
            WorkflowStep::Approval { .. } | WorkflowStep::Deliver { .. }
        )
    }) {
        run.terminal_id = term_id;
    }
    drive_at(db, config, card, &mut run, &mut queue, now_ms)
}

/// Compatibility callers (Review/merge) resolve the same run, never a stage executor.
pub fn advance(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    terminal: Option<&TerminalEntry>,
    queue: Queue,
) -> Result<()> {
    advance_at(db, config, card, terminal, LegacyQueue(queue), now())
}

/// Pure clock-injected variant of [`advance`].
pub fn advance_at(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    terminal: Option<&TerminalEntry>,
    mut queue: impl QueueHandler,
    now_ms: u64,
) -> Result<()> {
    let agent = terminal.and_then(|t| t.agent.as_ref());
    let run_opt: Option<WorkflowRun> = if let Some(a) = agent
        && let Some(id) = &a.workflow_run_id
    {
        Some(db.get(id)?)
    } else {
        db.list::<WorkflowRun>()?
            .into_iter()
            .find(|r| r.card_id.as_deref() == Some(card.id.as_str()) && r.active())
    };
    if let Some(mut run) = run_opt {
        if !run.active() {
            return Ok(());
        }
        if run.state == "waiting_approval" {
            return approve_with_feedback_at(
                db, config, card, &mut run, false, None, queue, now_ms,
            );
        }
        if run.state == "blocked" && agent.is_none_or(|a| a.state == "completed") {
            ensure!(
                digest(db, card) == run.input_digest,
                "Task changed; request rework instead"
            );
            if run.step_is_deliver() {
                if let Some(obj) = run.inputs.as_object_mut() {
                    obj.remove("queue_pause_reason");
                }
                run.inputs["queued_at"] =
                    json!(next_merge_queued_at_ms(db, &card.project_id, now_ms)?);
                run.visit += 1;
                run.error = None;
                run.state = "running".into();
                return drive_at(db, config, card, &mut run, &mut queue, now_ms);
            }
            let to = run.inputs["pending_next"].as_str().map(str::to_owned);
            next_at(&mut run, to, now_ms);
            return drive_at(db, config, card, &mut run, &mut queue, now_ms);
        }
    }
    Ok(())
}
pub fn approve(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    rework: bool,
    queue: Queue,
) -> Result<()> {
    approve_at(db, config, card, run, rework, LegacyQueue(queue), now())
}

/// Pure clock-injected variant of [`approve`].
pub fn approve_at(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    rework: bool,
    queue: impl QueueHandler,
    now_ms: u64,
) -> Result<()> {
    approve_with_feedback_at(db, config, card, run, rework, None, queue, now_ms)
}
pub fn approve_with_feedback(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    rework: bool,
    feedback: Option<&str>,
    queue: Queue,
) -> Result<()> {
    approve_with_feedback_at(
        db,
        config,
        card,
        run,
        rework,
        feedback,
        LegacyQueue(queue),
        now(),
    )
}

/// Pure clock-injected variant of [`approve_with_feedback`].
#[allow(clippy::too_many_arguments)]
pub fn approve_with_feedback_at(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    rework: bool,
    feedback: Option<&str>,
    mut queue: impl QueueHandler,
    now_ms: u64,
) -> Result<()> {
    ensure!(
        matches!(run.state.as_str(), "waiting_approval" | "stale"),
        "Workflow is not awaiting a decision"
    );
    let key = run.step.clone().context("No pending step")?;
    let is_script = matches!(&run.definition.steps[&key], WorkflowStep::Script { .. });
    let to = if rework {
        match &run.definition.steps[&key] {
            WorkflowStep::Approval {
                rework: Some(to), ..
            }
            | WorkflowStep::Deliver {
                rework: Some(to), ..
            } => Some(to.clone()),
            WorkflowStep::Script {
                on_failure: Some(to),
                ..
            } => Some(to.clone()),
            WorkflowStep::Agent { .. } if run.state == "stale" => Some(key.clone()),
            _ => bail!("No rework path configured"),
        }
    } else {
        let skip_digest = matches!(
            &run.definition.steps[&key],
            WorkflowStep::Script { .. } | WorkflowStep::Deliver { .. }
        );
        ensure!(
            skip_digest || digest(db, card) == run.input_digest,
            "Approval is stale; request changes instead"
        );
        ensure!(
            run.state == "waiting_approval",
            "Stale work cannot be approved"
        );
        if let WorkflowStep::Deliver {
            target_branch,
            strategy,
            sync_upstream,
            max_conflict_attempts,
            next: to,
            ..
        } = &run.definition.steps[&key]
        {
            run.history.push(json!({"step":key,"visit":run.visit,"decision":"approved","outputs":run.outputs,"at":now_ms}));
            card.human_gate = None;
            card.attention = None;
            card.hold_auto_merge = false;
            card.revision += 1;
            run.input_digest = digest(db, card);
            if let Some(obj) = run.inputs.as_object_mut() {
                obj.remove("queue_pause_reason");
            }
            if *max_conflict_attempts > 0
                && run.inputs["conflict_attempts"].as_u64().unwrap_or(0)
                    >= u64::from(*max_conflict_attempts)
            {
                run.inputs["conflict_attempts"] = json!(max_conflict_attempts - 1);
            }
            run.inputs["queued_at"] = json!(next_merge_queued_at_ms(db, &card.project_id, now_ms)?);
            run.inputs["pending_next"] = json!(to);
            run.inputs["target_branch"] = json!(target_branch);
            run.inputs["strategy"] = json!(strategy);
            run.inputs["sync_upstream"] = json!(sync_upstream);
            run.state = "waiting_delivery".into();
            run.updated_at = now_ms;
            db.save(run)?;
            db.save(card)?;
            record_at(
                db,
                run,
                "workflow.approved",
                "Human approved delivery",
                now_ms,
            )?;
            return Ok(());
        }
        if is_script {
            if let Some(obj) = run.inputs.as_object_mut() {
                obj.remove("script_terminal");
                obj.remove("rework_feedback");
                obj.remove("rework_origin");
            }
            let mut failures = run
                .inputs
                .get("script_failures")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            failures.insert(key.clone(), json!(0));
            run.inputs["script_failures"] = Value::Object(failures);
            run.history.push(json!({"step":key,"visit":run.visit,"decision":"retry","outputs":run.outputs,"at":now_ms}));
            card.human_gate = None;
            card.attention = None;
            card.revision += 1;
            run.error = None;
            run.input_digest = digest(db, card);
            run.step = Some(key.clone());
            run.terminal_id = None;
            run.state = "running".into();
            run.visit += 1;
            run.updated_at = now_ms;
            record_at(db, run, "workflow.approved", "Retrying script", now_ms)?;
            return drive_at(db, config, card, run, &mut queue, now_ms);
        }
        run.definition.steps[&key].next().map(str::to_owned)
    };
    let clean_feedback = feedback.map(str::trim).filter(|s| !s.is_empty());
    if rework {
        if is_script {
            if let Some(obj) = run.inputs.as_object_mut() {
                obj.remove("script_terminal");
            }
            let mut failures = run
                .inputs
                .get("script_failures")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            failures.insert(key.clone(), json!(0));
            run.inputs["script_failures"] = Value::Object(failures);
            run.inputs["rework_origin"] = json!("script_failure");
            if let Some(note) = clean_feedback {
                run.inputs["rework_feedback"] = json!(note);
            } else if run.inputs["rework_feedback"]
                .as_str()
                .is_none_or(|s| s.trim().is_empty())
                && let Some(out) = run.outputs.get(&key)
            {
                let summary = out["summary"].as_str().unwrap_or("Script failed");
                let log_path = out["path"].as_str().unwrap_or("");
                let note = if log_path.is_empty() {
                    summary.to_owned()
                } else {
                    format!("{summary}\nLog: {log_path}")
                };
                run.inputs["rework_feedback"] = json!(note);
            }
        } else if let Some(note) = clean_feedback {
            run.inputs["rework_feedback"] = json!(note);
            run.inputs["rework_origin"] = Value::Null;
        }
    }
    if rework && let Some(obj) = run.inputs.as_object_mut() {
        obj.remove("queued_at");
        obj.remove("queue_pause_reason");
    }
    run.history.push(json!({"step":key,"visit":run.visit,"decision":if rework{"rework"}else{"approved"},"feedback":clean_feedback,"outputs":run.outputs,"at":now_ms}));
    card.human_gate = None;
    card.attention = None;
    card.revision += 1;
    run.input_digest = digest(db, card);
    next_at(run, to, now_ms);
    record_at(
        db,
        run,
        "workflow.approved",
        if rework {
            if let Some(note) = clean_feedback {
                note
            } else {
                "Changes requested"
            }
        } else {
            "Human approved"
        },
        now_ms,
    )?;
    drive_at(db, config, card, run, &mut queue, now_ms)
}
pub fn complete_delivery(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    output: Value,
    queue: Queue,
) -> Result<()> {
    complete_delivery_at(db, config, card, run, output, LegacyQueue(queue), now())
}

/// Pure clock-injected variant of [`complete_delivery`].
pub fn complete_delivery_at(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    mut output: Value,
    mut queue: impl QueueHandler,
    now_ms: u64,
) -> Result<()> {
    let key = run.step.clone().context("Missing Deliver step")?;
    let queued_at = run.inputs["queued_at"].as_u64();
    if let Some(obj) = run.inputs.as_object_mut() {
        obj.remove("queued_at");
        obj.remove("queue_pause_reason");
    }
    if let Some(q) = queued_at
        && let Some(out_obj) = output.as_object_mut()
    {
        out_obj.entry("queued_at".to_owned()).or_insert(json!(q));
    }
    run.outputs.insert(key.clone(), output);
    let to = run.inputs["pending_next"].as_str().map(str::to_owned);
    next_at(run, to, now_ms);
    drive_at(db, config, card, run, &mut queue, now_ms)
}

/// Records successful (or `continue_on_error`) completion of the active `Script` step,
/// persists `$CANBAN_ENV` and `$CANBAN_OUTPUT` into run context, and continues execution.
pub fn complete_script(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    output: Value,
    exported_env: &BTreeMap<String, String>,
    queue: Queue,
) -> Result<()> {
    complete_script_at(
        db,
        config,
        card,
        run,
        output,
        exported_env,
        LegacyQueue(queue),
        now(),
    )
}

/// Pure clock-injected variant of [`complete_script`].
#[allow(clippy::too_many_arguments)]
pub fn complete_script_at(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    output: Value,
    exported_env: &BTreeMap<String, String>,
    mut queue: impl QueueHandler,
    now_ms: u64,
) -> Result<()> {
    ensure!(
        run.state == "waiting_script",
        "Workflow is not running a script"
    );
    let key = run.step.clone().context("Missing Script step")?;
    ensure!(
        matches!(
            run.definition.steps.get(&key),
            Some(WorkflowStep::Script { .. })
        ),
        "Current step is not a Script step"
    );
    merge_env_into_run(run, exported_env);
    if let Some(obj) = run.inputs.as_object_mut() {
        obj.remove("script_started_at");
        obj.remove("script_terminal");
    }
    run.outputs.insert(key, output);
    let to = run.inputs["pending_next"].as_str().map(str::to_owned);
    next_at(run, to, now_ms);
    drive_at(db, config, card, run, &mut queue, now_ms)
}

/// Records failure of the active `Script` step: jumps back to `on_failure` at most once per
/// run when configured, or pauses in `waiting_approval` for human intervention on repeat or
/// unconfigured failure.
#[allow(
    clippy::too_many_arguments,
    reason = "Transaction-scoped script failure carries output, exported env, summary, and rework feedback"
)]
pub fn fail_script(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    output: Value,
    exported_env: &BTreeMap<String, String>,
    summary: &str,
    detail_feedback: &str,
    queue: Queue,
) -> Result<()> {
    fail_script_at(
        db,
        config,
        card,
        run,
        output,
        exported_env,
        summary,
        detail_feedback,
        LegacyQueue(queue),
        now(),
    )
}

/// Pure clock-injected variant of [`fail_script`].
#[allow(
    clippy::too_many_arguments,
    reason = "Transaction-scoped script failure carries output, exported env, summary, rework feedback, and clock"
)]
pub fn fail_script_at(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    output: Value,
    exported_env: &BTreeMap<String, String>,
    summary: &str,
    detail_feedback: &str,
    mut queue: impl QueueHandler,
    now_ms: u64,
) -> Result<()> {
    ensure!(
        run.state == "waiting_script",
        "Workflow is not running a script"
    );
    let key = run.step.clone().context("Missing Script step")?;
    let on_failure = match run.definition.steps.get(&key) {
        Some(WorkflowStep::Script { on_failure, .. }) => on_failure.clone(),
        _ => bail!("Current step is not a Script step"),
    };
    merge_env_into_run(run, exported_env);
    if let Some(obj) = run.inputs.as_object_mut() {
        obj.remove("script_started_at");
        obj.remove("script_terminal");
    }
    run.history
        .push(json!({"step":key,"visit":run.visit,"event":"failed","outputs":output,"at":now_ms}));
    run.outputs.insert(key.clone(), output.clone());
    log_at(
        db,
        card,
        "workflow.script_failed",
        "system",
        summary,
        None,
        json!({
            "workflow_run_id": run.id,
            "step": key,
            "path": output["path"],
            "exit_code": output["exit_code"],
            "timed_out": output["timed_out"],
        }),
        now_ms,
    )?;

    let mut failures = run
        .inputs
        .get("script_failures")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let prior = failures.get(&key).and_then(|v| v.as_u64()).unwrap_or(0);
    failures.insert(key.clone(), json!(prior + 1));
    run.inputs["script_failures"] = Value::Object(failures);

    if let Some(target) = on_failure
        && prior == 0
    {
        run.inputs["rework_origin"] = json!("script_failure");
        run.inputs["rework_feedback"] = json!(detail_feedback);
        run.step = Some(target);
        run.terminal_id = None;
        run.error = None;
        run.state = "running".into();
        run.visit += 1;
        run.updated_at = now_ms;
        return drive_at(db, config, card, run, &mut queue, now_ms);
    }

    attention_at(db, card, summary, now_ms)?;
    card.human_gate = Some(HumanGate {
        reason: summary.into(),
        terminal_id: run.terminal_id.clone(),
        next_stage: None,
    });
    card.revision += 1;
    run.state = "waiting_approval".into();
    run.error = Some(summary.into());
    run.inputs["rework_feedback"] = json!(detail_feedback);
    run.input_digest = digest(db, card);
    run.updated_at = now_ms;
    db.save(run)?;
    db.save(card)?;
    record_at(db, run, "workflow.waiting_for_human", summary, now_ms)?;
    Ok(())
}
/// Releases a run waiting at an `admit` gate. The caller (admission pump) decides
/// capacity and Board order; this only records the decision and continues.
pub fn admit(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    queue: Queue,
) -> Result<()> {
    admit_at(db, config, card, run, LegacyQueue(queue), now())
}

/// Pure clock-injected variant of [`admit`].
pub fn admit_at(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    mut queue: impl QueueHandler,
    now_ms: u64,
) -> Result<()> {
    ensure!(
        run.state == "waiting_capacity",
        "Workflow is not waiting for capacity"
    );
    let key = run.step.clone().context("Missing Admit step")?;
    let Some(WorkflowStep::Admit {
        max_ready,
        next: to,
        ..
    }) = run.definition.steps.get(&key).cloned()
    else {
        bail!("Current step is not an Admit gate");
    };
    if max_ready.is_some() {
        run.inputs["ready_gate"] = json!({"step": key, "column": card.stage});
    }
    record_at(db, run, "workflow.admitted", "Capacity available", now_ms)?;
    next_at(run, to, now_ms);
    drive_at(db, config, card, run, &mut queue, now_ms)
}

/// Dispatches a delivery merge conflict directly to the Deliver step's conflict
/// Agent (`profile`, defaulting to `@merger` when enabled) so it resolves the
/// conflict in the Card worktree while keeping the run at the `Deliver` step.
/// Returns `false` (caller should hold for a human) when the attempt budget is
/// spent or the conflict Agent cannot be queued.
#[allow(
    clippy::too_many_arguments,
    reason = "Transaction-scoped conflict context plus queue adapter"
)]
pub fn rework_conflict(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    target_branch: &str,
    target_head: &str,
    files: &[String],
    queue: Queue,
) -> Result<bool> {
    rework_conflict_at(
        db,
        config,
        card,
        run,
        target_branch,
        target_head,
        files,
        LegacyQueue(queue),
        now(),
    )
}

/// Pure clock-injected variant of [`rework_conflict`].
#[allow(
    clippy::too_many_arguments,
    reason = "Transaction-scoped conflict context plus queue adapter and clock"
)]
pub fn rework_conflict_at(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    target_branch: &str,
    target_head: &str,
    files: &[String],
    mut queue: impl QueueHandler,
    now_ms: u64,
) -> Result<bool> {
    if run.state != "waiting_delivery" {
        return Ok(false);
    }
    let key = run.step.clone().context("Missing Deliver step")?;
    let Some(WorkflowStep::Deliver {
        profile,
        strategy,
        next: continuation,
        rework,
        max_conflict_attempts,
        ..
    }) = run.definition.steps.get(&key).cloned()
    else {
        return Ok(false);
    };
    let attempts = run.inputs["conflict_attempts"].as_u64().unwrap_or(0) as u32;
    if attempts >= max_conflict_attempts {
        return Ok(false);
    }
    let conflict_profile = profile
        .filter(|p| !p.trim().is_empty())
        .or_else(|| {
            config
                .profiles
                .iter()
                .any(|p| p.id == "merger" && p.enabled)
                .then(|| "merger".to_owned())
        })
        .or_else(|| {
            rework
                .as_ref()
                .and_then(|to| match run.definition.steps.get(to) {
                    Some(WorkflowStep::Agent {
                        profile: Some(p), ..
                    }) if config.profiles.iter().any(|x| x.id == *p && x.enabled) => {
                        Some(p.clone())
                    }
                    _ => None,
                })
        });
    let files_summary = if files.is_empty() {
        "(inspect `git status` in the Card worktree)".to_owned()
    } else {
        files.join(", ")
    };
    let resolution_steps = match strategy {
        MergeStrategy::Rebase => format!(
            "The worktree may already be mid-rebase onto `{target_branch}` ({target_head}). \
             Resolve every conflict while preserving both this Card's intent and the target branch changes, \
             stage resolved files, and run `GIT_EDITOR=true git rebase --continue` until the rebase completes \
             (if no rebase is in progress, run `GIT_EDITOR=true git rebase {target_branch}`). \
             Keep clean, semantic commit messages rather than raw SHA merge commits, and run the relevant builds and tests."
        ),
        MergeStrategy::Squash | MergeStrategy::Merge => format!(
            "The worktree may have an in-progress merge of `{target_branch}` ({target_head}). \
             Resolve every conflict while preserving both this Card's intent and the target branch changes, \
             run the relevant builds and tests, and commit the resolved worktree with a semantic message \
             such as `git commit -m \"chore(merge): integrate {target_branch} into card #{}\"` \
             (if no merge is in progress, run `git merge --no-commit --no-ff {target_branch}` first).",
            card.number
        ),
    };
    let instruction = format!(
        "Merge conflict to resolve before delivery ({strategy_name} strategy):\n\
         Queue merge found conflicts integrating `{target_branch}` ({target_head}) into this Card worktree.\n\
         Conflicted files: {files_summary}\n\
         {resolution_steps} \
         Do not push; Queue merge will retry delivery automatically after you report `--state completed` (or report `--state blocked` if human judgment is required).",
        strategy_name = strategy.as_str(),
    );
    let project: Project = db.get(&card.project_id)?;
    let stage = Stage {
        id: card.stage.clone(),
        name: card.stage.clone(),
        execute: true,
        profile: conflict_profile,
        instruction,
        on_completed: None,
        require_human_review: false,
    };
    let Some(mut entry) = queue.enqueue(
        db,
        config,
        card,
        &project,
        &stage,
        ExecutionTrigger::Workflow,
        None,
        None,
        now_ms,
    )?
    else {
        return Ok(false);
    };
    run.visit += 1;
    let step_ref = format!("{key}:{}", run.visit);
    let mut agent_id = String::new();
    if let Some(agent) = &mut entry.agent {
        if let Some(p) = &mut agent.pending {
            p.workflow_run_id = Some(run.id.clone());
            p.workflow_step = Some(step_ref.clone());
            agent_id = p.profile.id.clone();
        } else {
            agent.workflow_run_id = Some(run.id.clone());
            agent.workflow_step = Some(step_ref.clone());
            agent_id = agent.profile.id.clone();
        }
    }
    db.save(&entry)?;
    db.conn.execute(
        "UPDATE events SET data=json_set(data,'$.data.workflow_run_id',?2,'$.data.workflow_step',?3,'$.data.origin','workflow') WHERE json_extract(data,'$.data.terminal_id')=?1 AND json_extract(data,'$.kind')='execution.queued'",
        [entry.id.as_str(), run.id.as_str(), step_ref.as_str()],
    )?;
    if let Some(obj) = run.inputs.as_object_mut() {
        obj.remove("queue_pause_reason");
    }
    run.inputs["conflict_attempts"] = json!(attempts + 1);
    run.inputs["title"] = json!(card.title);
    run.inputs["body"] = json!(card.body);
    run.inputs["pending_next"] = json!(continuation);
    run.outputs.insert(
        key.clone(),
        json!({"state":"conflict","target_branch":target_branch,"target_head":target_head,"files":files,"conflict_round":attempts + 1,"profile_id":agent_id}),
    );
    run.history.push(
        json!({"step":key,"visit":run.visit,"decision":"merge_conflict","profile_id":agent_id,"files":files,"at":now_ms}),
    );
    card.human_gate = None;
    card.attention = None;
    card.revision += 1;
    run.terminal_id = Some(entry.id.clone());
    run.state = "waiting_agent".into();
    run.error = None;
    run.input_digest = digest(db, card);
    run.updated_at = now_ms;
    log_at(
        db,
        card,
        "review.merge_conflict",
        "system",
        &format!(
            "Merge conflict with {target_branch}; @{agent_id} will resolve it (round {}/{max_conflict_attempts})",
            attempts + 1
        ),
        Some(&entry.id),
        json!({"workflow_run_id":run.id,"target_branch":target_branch,"target_head":target_head,"files":files,"conflict_round":attempts + 1,"profile_id":agent_id}),
        now_ms,
    )?;
    db.save(run)?;
    db.save(card)?;
    Ok(true)
}

pub fn hold_delivery(
    db: Db<'_>,
    card: &mut Card,
    run: &mut WorkflowRun,
    reason: &str,
) -> Result<()> {
    hold_delivery_at(db, card, run, reason, now())
}

/// Pure clock-injected variant of [`hold_delivery`].
pub fn hold_delivery_at(
    db: Db<'_>,
    card: &mut Card,
    run: &mut WorkflowRun,
    reason: &str,
    now_ms: u64,
) -> Result<()> {
    attention_at(db, card, reason, now_ms)?;
    card.human_gate = Some(HumanGate {
        reason: reason.into(),
        terminal_id: run.terminal_id.clone(),
        next_stage: None,
    });
    card.revision += 1;
    if let Some(obj) = run.inputs.as_object_mut() {
        obj.remove("queued_at");
        obj.remove("queue_pause_reason");
    }
    run.state = "waiting_approval".into();
    run.updated_at = now_ms;
    db.save(run)?;
    db.save(card)?;
    record_at(db, run, "workflow.waiting_for_human", reason, now_ms)?;
    Ok(())
}

/// Computes a strictly increasing `queued_at` ticket for a Project's FIFO merge queue
/// so newly queued or retried deliveries always join behind existing queued/active entries.
pub fn next_merge_queued_at(db: Db<'_>, project_id: &str) -> Result<u64> {
    next_merge_queued_at_ms(db, project_id, now())
}

/// Pure clock-injected variant of [`next_merge_queued_at`].
pub fn next_merge_queued_at_ms(db: Db<'_>, project_id: &str, now_ms: u64) -> Result<u64> {
    let max_existing = db
        .list::<WorkflowRun>()?
        .into_iter()
        .filter(|r| r.project_id == project_id && r.active() && r.step_is_deliver())
        .filter_map(|r| r.inputs["queued_at"].as_u64())
        .max()
        .unwrap_or(0);
    Ok(now_ms.max(max_existing.saturating_add(1)))
}

/// Re-queues a failed, blocked, or stopped `Deliver` workflow run at the tail of its
/// Project's FIFO merge queue (`waiting_delivery`) rather than bypassing active queue members
/// or re-launching `@merger` out of order.
pub fn requeue_delivery(db: Db<'_>, card: &mut Card, run: &mut WorkflowRun) -> Result<()> {
    requeue_delivery_at(db, card, run, now())
}

/// Pure clock-injected variant of [`requeue_delivery`].
pub fn requeue_delivery_at(
    db: Db<'_>,
    card: &mut Card,
    run: &mut WorkflowRun,
    now_ms: u64,
) -> Result<()> {
    ensure!(run.active(), "Workflow already ended");
    let key = run.step.clone().context("Missing Deliver step")?;
    let WorkflowStep::Deliver {
        target_branch,
        strategy,
        sync_upstream,
        batch_size,
        test_command,
        max_conflict_attempts,
        next: to,
        ..
    } = run
        .definition
        .steps
        .get(&key)
        .cloned()
        .context("Current step is not Deliver")?
    else {
        bail!("Current step is not Deliver");
    };
    ensure!(
        matches!(
            run.state.as_str(),
            "failed" | "blocked" | "waiting_agent" | "waiting_batch"
        ),
        "Delivery is not retryable from state {}",
        run.state
    );
    if run.state == "waiting_batch" {
        let active_batch = db.list::<MergeBatch>()?.into_iter().any(|b| {
            b.active()
                && b.items
                    .iter()
                    .any(|item| !item.evicted && item.workflow_run_id == run.id)
        });
        ensure!(
            !active_batch,
            "Card is currently owned by an active merge batch"
        );
    }
    let mut handled_terminal = false;
    let mut refund_attempt = true;
    if let Some(tid) = &run.terminal_id
        && let Ok(mut entry) = db.get::<TerminalEntry>(tid)
        && let Some(agent) = &mut entry.agent
    {
        ensure!(
            !matches!(
                agent.state.as_str(),
                "queued" | "starting" | "running" | "working"
            ),
            "End the running Agent before retrying auto-merge"
        );
        ensure!(
            !(agent.state == "completed" && run.state != "blocked"),
            "Completed conflict Agent is awaiting automatic delivery"
        );
        ensure!(
            agent.terminal.is_none()
                || agent
                    .observed
                    .as_ref()
                    .is_some_and(|v| matches!(v["state"].as_str(), Some("finished" | "missing"))),
            "End the running Agent before retrying auto-merge"
        );
        handled_terminal = true;
        if matches!(agent.state.as_str(), "completed" | "backlog") {
            refund_attempt = false;
        }
        let mut entry_changed = false;
        if matches!(agent.state.as_str(), "blocked" | "needs_report" | "unknown")
            || !agent.finished()
        {
            agent.state = "cancelled".into();
            entry_changed = true;
        }
        if agent.pending.is_some() {
            agent.pending = None;
            entry_changed = true;
        }
        if entry_changed {
            agent.updated_at = now_ms;
            db.save(&entry)?;
        }
    }
    if run.state == "waiting_agent" && !handled_terminal {
        bail!("End the running Agent before retrying auto-merge");
    }
    if refund_attempt && max_conflict_attempts > 0 {
        let attempts = run.inputs["conflict_attempts"].as_u64().unwrap_or(0);
        if attempts > 0 {
            let refunded = (attempts - 1).min(u64::from(max_conflict_attempts - 1));
            run.inputs["conflict_attempts"] = json!(refunded);
        }
    }
    card.hold_auto_merge = false;
    card.human_gate = None;
    card.attention = None;
    card.revision += 1;
    run.input_digest = digest(db, card);
    if let Some(obj) = run.inputs.as_object_mut() {
        obj.remove("queue_pause_reason");
    }
    run.inputs["queued_at"] = json!(next_merge_queued_at_ms(db, &card.project_id, now_ms)?);
    run.inputs["pending_next"] = json!(to);
    run.inputs["target_branch"] = json!(target_branch);
    run.inputs["strategy"] = json!(strategy);
    run.inputs["sync_upstream"] = json!(sync_upstream);
    run.inputs["batch_size"] = json!(batch_size);
    run.inputs["test_command"] = json!(test_command);
    run.terminal_id = None;
    run.error = None;
    run.state = "waiting_delivery".into();
    run.updated_at = now_ms;
    run.history.push(json!({
        "step": key,
        "visit": run.visit,
        "decision": "retry_queued",
        "at": now_ms
    }));
    db.save(run)?;
    db.save(card)?;
    record_at(
        db,
        run,
        "workflow.delivery_requeued",
        "Queued for auto-merge",
        now_ms,
    )?;
    Ok(())
}

fn build_cardless_inputs(db: Db<'_>, event_id: Option<&str>) -> Value {
    let mut inputs = json!({
        "cardless": true,
    });
    if let Some(eid) = event_id
        && let Ok(ev) = db.get::<Event>(eid)
    {
        inputs["trigger_event"] = json!({
            "id": ev.id,
            "kind": ev.kind,
            "actor": ev.actor,
            "message": ev.message,
            "data": ev.data,
        });
    }
    inputs
}

/// Starts a cardless `WorkflowRun` (`card_id: None`) for a repo-level event or schedule trigger,
/// advancing through synchronous steps (`Condition`, `Notify`, `Complete`) until it pauses at
/// `Script` (`waiting_script`) or `Agent` (`waiting_agent`), completes, or fails.
pub fn start_cardless(
    db: Db<'_>,
    project_id: &str,
    flow: WorkflowDefinition,
    revision: u64,
    entry: &str,
    event: Option<String>,
) -> Result<WorkflowRun> {
    start_cardless_at(db, project_id, flow, revision, entry, event, now())
}

/// Pure clock-injected variant of [`start_cardless`].
pub fn start_cardless_at(
    db: Db<'_>,
    project_id: &str,
    flow: WorkflowDefinition,
    revision: u64,
    entry: &str,
    event: Option<String>,
    now_ms: u64,
) -> Result<WorkflowRun> {
    let step = if let Some(s) = flow.entries.get(entry) {
        s.clone()
    } else if flow.steps.contains_key(entry) {
        entry.to_owned()
    } else {
        bail!("Unknown workflow entry")
    };
    let inputs = build_cardless_inputs(db, event.as_deref());
    let input_digest = canban_context::digest(&serde_json::to_vec(&inputs).unwrap_or_default());
    let mut run = WorkflowRun {
        id: id(),
        project_id: project_id.to_owned(),
        card_id: None,
        workflow_id: flow.id.clone(),
        definition_revision: revision,
        definition: flow,
        state: "running".into(),
        step: Some(step),
        visit: 1,
        terminal_id: None,
        trigger_event_id: event,
        input_digest,
        inputs,
        outputs: BTreeMap::new(),
        history: Vec::new(),
        error: None,
        created_at: now_ms,
        updated_at: now_ms,
    };
    db.save(&run)?;
    record_at(db, &run, "workflow.started", entry, now_ms)?;
    drive_cardless_at(db, &mut run, now_ms)?;
    Ok(run)
}

/// Advances a cardless `WorkflowRun` through `Script`, `Condition`, `Agent`, `Notify`, and
/// `Complete` steps. Card-only steps (`Move`, `Approval`, `Deliver`, `Admit`) fail with a clear
/// error.
pub fn drive_cardless(db: Db<'_>, run: &mut WorkflowRun) -> Result<()> {
    drive_cardless_at(db, run, now())
}

/// Pure clock-injected variant of [`drive_cardless`].
pub fn drive_cardless_at(db: Db<'_>, run: &mut WorkflowRun, now_ms: u64) -> Result<()> {
    ensure!(
        run.card_id.is_none(),
        "drive_cardless requires a cardless WorkflowRun"
    );
    while run.state == "running" {
        if let Some(removed) = run.inputs["stop_after_step"].as_str()
            && run.step.as_deref() != Some(removed)
        {
            run.step = None;
            run.state = "cancelled".into();
            record_at(
                db,
                run,
                "workflow.stopped",
                "Step removed from workflow",
                now_ms,
            )?;
            break;
        }
        if run.visit > run.definition.max_steps {
            run.state = "failed".into();
            run.error = Some("Workflow execution limit reached".into());
            break;
        }
        let Some(key) = run.step.clone() else {
            run.state = "completed".into();
            break;
        };
        let step = run
            .definition
            .steps
            .get(&key)
            .context("Workflow step not found")?
            .clone();
        run.history
            .push(json!({"step":key,"visit":run.visit,"at":now_ms,"event":"entered"}));
        match step {
            WorkflowStep::Script { next: to, .. } => {
                run.error = None;
                if let Some(obj) = run.inputs.as_object_mut() {
                    obj.remove("script_terminal");
                }
                run.inputs["pending_next"] = json!(to);
                run.inputs["script_started_at"] = json!(now_ms);
                run.state = "waiting_script".into();
                record_at(
                    db,
                    run,
                    "workflow.script_started",
                    &format!("Running script {key}"),
                    now_ms,
                )?;
            }
            WorkflowStep::Condition {
                pointer,
                equals,
                then,
                otherwise,
            } => {
                let context = json!({"inputs":run.inputs,"outputs":run.outputs,"card":Value::Null});
                next_at(
                    run,
                    Some(if condition_matches(context.pointer(&pointer), &equals) {
                        then
                    } else {
                        otherwise
                    }),
                    now_ms,
                );
            }
            WorkflowStep::Agent {
                profile,
                instruction,
                next: continuation,
                ..
            } => {
                let rework_label = match run.inputs["rework_origin"].as_str() {
                    Some("script_failure") => "Workflow script failure to fix",
                    _ => "Feedback for this visit",
                };
                let rework_note = run.inputs["rework_feedback"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| format!("\n\n{rework_label}:\n{}", s.trim()))
                    .unwrap_or_default();
                if run.inputs.get("rework_feedback").is_some() {
                    run.inputs["rework_feedback"] = Value::Null;
                    run.inputs["rework_origin"] = Value::Null;
                }
                let prior_steps = if run.outputs.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n\nEarlier workflow step outputs:\n{}",
                        serde_json::to_string(&run.outputs)?
                    )
                };
                let trigger_note = trigger_event_note(&run.inputs);
                let full_instruction =
                    format!("{instruction}{rework_note}{trigger_note}{prior_steps}");
                if let Some(obj) = run.inputs.as_object_mut() {
                    obj.remove("agent_terminal");
                }
                run.inputs["pending_next"] = json!(continuation);
                run.inputs["pending_agent"] = json!({
                    "profile": profile,
                    "instruction": full_instruction,
                });
                run.state = "waiting_agent".into();
                record_at(
                    db,
                    run,
                    "workflow.agent_queued",
                    &format!("Queued cardless agent step {key}"),
                    now_ms,
                )?;
            }
            WorkflowStep::Notify { message, next: to } => {
                db.save(&canban_notifications::attention_at(
                    &run.project_id,
                    "",
                    &message,
                    now_ms,
                ))?;
                record_at(db, run, "workflow.notified", &message, now_ms)?;
                next_at(run, to, now_ms);
            }
            WorkflowStep::Complete => {
                run.history
                    .push(json!({"step":key,"visit":run.visit,"event":"completed","at":now_ms}));
                run.step = None;
                run.state = "completed".into();
            }
            other => {
                let kind = match other {
                    WorkflowStep::Move { .. } => "card.move",
                    WorkflowStep::Approval { .. } => "approval",
                    WorkflowStep::Deliver { .. } => "deliver",
                    WorkflowStep::Admit { .. } => "admit",
                    _ => "step",
                };
                let err = format!(
                    "Step `{key}` ({kind}) requires a Card and cannot run in a cardless workflow"
                );
                run.state = "failed".into();
                run.error = Some(err.clone());
                run.updated_at = now_ms;
                db.save(run)?;
                record_at(db, run, "workflow.failed", &err, now_ms)?;
                bail!("{err}");
            }
        }
    }
    run.updated_at = now_ms;
    db.save(run)?;
    if run.state == "completed" {
        record_at(db, run, "workflow.completed", "Workflow completed", now_ms)?;
    }
    Ok(())
}

/// Records successful (or `continue_on_error`) completion of the active `Script` step in a
/// cardless `WorkflowRun`, merges `$CANBAN_ENV` and `$CANBAN_OUTPUT`, and drives forward.
pub fn complete_cardless_script(
    db: Db<'_>,
    run: &mut WorkflowRun,
    output: Value,
    exported_env: &BTreeMap<String, String>,
) -> Result<()> {
    complete_cardless_script_at(db, run, output, exported_env, now())
}

/// Pure clock-injected variant of [`complete_cardless_script`].
pub fn complete_cardless_script_at(
    db: Db<'_>,
    run: &mut WorkflowRun,
    output: Value,
    exported_env: &BTreeMap<String, String>,
    now_ms: u64,
) -> Result<()> {
    ensure!(
        run.card_id.is_none(),
        "complete_cardless_script requires a cardless WorkflowRun"
    );
    ensure!(
        run.state == "waiting_script",
        "Workflow is not running a script"
    );
    let key = run.step.clone().context("Missing Script step")?;
    ensure!(
        matches!(
            run.definition.steps.get(&key),
            Some(WorkflowStep::Script { .. })
        ),
        "Current step is not a Script step"
    );
    merge_env_into_run(run, exported_env);
    if let Some(obj) = run.inputs.as_object_mut() {
        obj.remove("script_started_at");
        obj.remove("script_terminal");
    }
    run.outputs.insert(key, output);
    let to = run.inputs["pending_next"].as_str().map(str::to_owned);
    next_at(run, to, now_ms);
    drive_cardless_at(db, run, now_ms)
}

/// Records failure of the active `Script` step in a cardless `WorkflowRun`: jumps to `on_failure`
/// at most once per run when configured, or marks the run `failed` otherwise.
pub fn fail_cardless_script(
    db: Db<'_>,
    run: &mut WorkflowRun,
    output: Value,
    exported_env: &BTreeMap<String, String>,
    summary: &str,
    detail_feedback: &str,
) -> Result<()> {
    fail_cardless_script_at(
        db,
        run,
        output,
        exported_env,
        summary,
        detail_feedback,
        now(),
    )
}

/// Pure clock-injected variant of [`fail_cardless_script`].
pub fn fail_cardless_script_at(
    db: Db<'_>,
    run: &mut WorkflowRun,
    output: Value,
    exported_env: &BTreeMap<String, String>,
    summary: &str,
    detail_feedback: &str,
    now_ms: u64,
) -> Result<()> {
    ensure!(
        run.card_id.is_none(),
        "fail_cardless_script requires a cardless WorkflowRun"
    );
    ensure!(
        run.state == "waiting_script",
        "Workflow is not running a script"
    );
    let key = run.step.clone().context("Missing Script step")?;
    let on_failure = match run.definition.steps.get(&key) {
        Some(WorkflowStep::Script { on_failure, .. }) => on_failure.clone(),
        _ => bail!("Current step is not a Script step"),
    };
    merge_env_into_run(run, exported_env);
    if let Some(obj) = run.inputs.as_object_mut() {
        obj.remove("script_started_at");
        obj.remove("script_terminal");
    }
    run.history
        .push(json!({"step":key,"visit":run.visit,"event":"failed","outputs":output,"at":now_ms}));
    run.outputs.insert(key.clone(), output.clone());
    db.append(canban_timeline::event_at(
        &run.project_id,
        "",
        "workflow.script_failed",
        "system",
        summary,
        json!({
            "workflow_run_id": run.id,
            "step": key,
            "path": output["path"],
            "exit_code": output["exit_code"],
            "timed_out": output["timed_out"],
        }),
        now_ms,
    ))?;

    let mut failures = run
        .inputs
        .get("script_failures")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let prior = failures.get(&key).and_then(|v| v.as_u64()).unwrap_or(0);
    failures.insert(key.clone(), json!(prior + 1));
    run.inputs["script_failures"] = Value::Object(failures);

    if let Some(target) = on_failure
        && prior == 0
    {
        run.inputs["rework_origin"] = json!("script_failure");
        run.inputs["rework_feedback"] = json!(detail_feedback);
        run.step = Some(target);
        run.terminal_id = None;
        run.error = None;
        run.state = "running".into();
        run.visit += 1;
        run.updated_at = now_ms;
        return drive_cardless_at(db, run, now_ms);
    }

    run.state = "failed".into();
    run.error = Some(summary.into());
    run.inputs["rework_feedback"] = json!(detail_feedback);
    run.updated_at = now_ms;
    db.save(run)?;
    record_at(db, run, "workflow.failed", summary, now_ms)?;
    Ok(())
}

/// Records completion (or non-completed termination) of the active `Agent` step in a cardless
/// `WorkflowRun` and drives forward when `report_state == "completed"`.
pub fn complete_cardless_agent(
    db: Db<'_>,
    run: &mut WorkflowRun,
    output: Value,
    report_state: &str,
) -> Result<()> {
    complete_cardless_agent_at(db, run, output, report_state, now())
}

/// Pure clock-injected variant of [`complete_cardless_agent`].
pub fn complete_cardless_agent_at(
    db: Db<'_>,
    run: &mut WorkflowRun,
    output: Value,
    report_state: &str,
    now_ms: u64,
) -> Result<()> {
    ensure!(
        run.card_id.is_none(),
        "complete_cardless_agent requires a cardless WorkflowRun"
    );
    ensure!(
        matches!(run.state.as_str(), "waiting_agent" | "running"),
        "Workflow is not awaiting an Agent step"
    );
    let key = run.step.clone().context("Missing Agent step")?;
    ensure!(
        matches!(
            run.definition.steps.get(&key),
            Some(WorkflowStep::Agent { .. })
        ),
        "Current step is not an Agent step"
    );
    if let Some(obj) = run.inputs.as_object_mut() {
        obj.remove("pending_agent");
        obj.remove("agent_terminal");
    }
    run.outputs.insert(key.clone(), output.clone());
    if report_state != "completed" {
        run.history.push(json!({
            "step": key,
            "visit": run.visit,
            "event": report_state,
            "outputs": output,
            "at": now_ms
        }));
        run.state = if report_state == "backlog" {
            "cancelled".into()
        } else {
            report_state.into()
        };
        run.error = output["message"].as_str().map(str::to_owned);
        run.updated_at = now_ms;
        db.save(run)?;
        return Ok(());
    }
    let to = run.inputs["pending_next"].as_str().map(str::to_owned);
    next_at(run, to, now_ms);
    drive_cardless_at(db, run, now_ms)
}

pub fn initialize(db: Db<'_>, config: &Config) -> Result<()> {
    let _ = (db, config);
    Ok(())
}

pub fn reconcile(db: Db<'_>) -> Result<()> {
    reconcile_at(db, now())
}

/// Pure clock-injected variant of [`reconcile`].
pub fn reconcile_at(db: Db<'_>, now_ms: u64) -> Result<()> {
    for mut run in db.list::<WorkflowRun>()?.into_iter().filter(|r| r.active()) {
        if let Some(card_id) = &run.card_id {
            let card: Card = db.get(card_id)?;
            if !card.active() {
                stop_card_at(db, &card.id, "cancelled", now_ms)?;
                continue;
            }
        }
        if let Some(id) = &run.terminal_id
            && let Ok(entry) = db.get::<TerminalEntry>(id)
            && let Some(agent) = &entry.agent
        {
            if matches!(run.state.as_str(), "waiting_agent" | "failed")
                && agent.pending.is_none()
                && matches!(agent.state.as_str(), "failed" | "unknown" | "needs_report")
            {
                let next_state = if agent.state == "failed" {
                    "failed"
                } else {
                    "waiting_agent"
                };
                if next_state == "failed"
                    && run.step_is_deliver()
                    && let Some(obj) = run.inputs.as_object_mut()
                {
                    obj.remove("queued_at");
                    obj.remove("queue_pause_reason");
                }
                run.state = next_state.into();
                run.error = Some(format!(
                    "Execution {}: inspect or explicitly retry",
                    agent.state
                ));
                db.save(&run)?;
            } else if run.state == "failed" && (agent.state == "queued" || agent.pending.is_some())
            {
                run.state = "waiting_agent".into();
                run.error = None;
                db.save(&run)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod cardless_runtime_tests {
    use super::*;
    use canban_storage::Store;

    fn temp_store() -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("canban-wf-cardless-{}", id()));
        let store = Store::open(&dir).unwrap();
        (store, dir)
    }

    fn seed_project(store: &mut Store) -> Project {
        let project = Project {
            id: "proj-1".into(),
            name: "Demo".into(),
            directory: "/tmp/demo".into(),
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: DeliveryMode::default(),
            agent_overrides: BTreeMap::new(),
            tags: Vec::new(),
            workflow: Workflow::default(),
            target_branch: None,
            created_at: now(),
        };
        store.write(|db| db.save(&project)).unwrap();
        project
    }

    #[test]
    fn cardless_script_condition_agent_notify_complete_pipeline_works() {
        let (mut store, dir) = temp_store();
        let project = seed_project(&mut store);
        let yaml = r#"schema: 11
id: issue:reply
name: Issue Auto-Reply
on:
  comment:
    event: github.issue.comment_created
    start_at: filter
    cardless: true
    match:
      exclude_self: true
steps:
  - id: filter
    run: echo ok
    next: check
  - id: check
    uses: condition
    with:
      pointer: /outputs/filter/matched
      equals: true
      then: reply
      otherwise: skip_notify
  - id: reply
    uses: agent
    with:
      profile: developer
      instruction: Reply to issue comment
    next: done
  - id: skip_notify
    uses: notify
    with:
      message: Skipped non-matching comment
    next: done
  - id: done
    uses: complete
"#;
        let flow = crate::source::parse(yaml).unwrap().compile().unwrap();

        // Trigger event saved in DB
        let ev = store
            .write(|db| {
                db.append(canban_timeline::event(
                    "",
                    "",
                    "github.issue.comment_created",
                    "github:alice",
                    "help!",
                    json!({"repository": "org/repo", "number": 42}),
                ))
            })
            .unwrap();

        // 1. Matched branch: script -> condition(true) -> agent -> complete
        let mut run = store
            .write(|db| {
                start_cardless(
                    db,
                    &project.id,
                    flow.clone(),
                    1,
                    "comment",
                    Some(ev.id.clone()),
                )
            })
            .unwrap();
        assert!(run.is_cardless());
        assert_eq!(run.state, "waiting_script");
        assert_eq!(run.step.as_deref(), Some("filter"));
        assert_eq!(run.trigger_event_id.as_deref(), Some(ev.id.as_str()));
        assert_eq!(run.inputs["trigger_event"]["data"]["number"], 42);

        // Reload from SQLite to verify NULL card_id roundtrip
        let reloaded: WorkflowRun = store.db().get(&run.id).unwrap();
        assert_eq!(reloaded.card_id, None);
        assert_eq!(reloaded.state, "waiting_script");

        // Reconcile must not fail on cardless run
        store.write(reconcile).unwrap();

        // Complete script with matched=true -> Condition routes to `reply` (Agent)
        let mut exported_env = BTreeMap::new();
        exported_env.insert("ISSUE_NUM".into(), "42".into());
        store
            .write(|db| {
                complete_cardless_script(db, &mut run, json!({"matched": true}), &exported_env)
            })
            .unwrap();
        assert_eq!(run.state, "waiting_agent");
        assert_eq!(run.step.as_deref(), Some("reply"));
        assert_eq!(run.inputs["env"]["ISSUE_NUM"], "42");
        assert_eq!(run.inputs["pending_agent"]["profile"], "developer");

        // Complete agent -> Complete
        store
            .write(|db| complete_cardless_agent(db, &mut run, json!({"ok": true}), "completed"))
            .unwrap();
        assert_eq!(run.state, "completed");
        assert!(!run.active());

        // 2. Non-matched branch: script -> condition(false) -> notify -> complete
        let mut run2 = store
            .write(|db| start_cardless(db, &project.id, flow.clone(), 1, "comment", None))
            .unwrap();
        store
            .write(|db| {
                complete_cardless_script(db, &mut run2, json!({"matched": false}), &BTreeMap::new())
            })
            .unwrap();
        assert_eq!(run2.state, "completed");
        let notices: Vec<Notice> = store.db().list().unwrap();
        assert!(
            notices
                .iter()
                .any(|n| n.card_id.is_empty() && n.message == "Skipped non-matching comment")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cardless_run_rejects_card_only_steps_with_clear_error() {
        let (mut store, dir) = temp_store();
        let project = seed_project(&mut store);
        for (name, step) in [
            (
                "card.move",
                WorkflowStep::Move {
                    column: "develop".into(),
                    next: None,
                },
            ),
            (
                "approval",
                WorkflowStep::Approval {
                    title: "Approve".into(),
                    next: None,
                    rework: None,
                },
            ),
            (
                "deliver",
                WorkflowStep::Deliver {
                    profile: None,
                    target_branch: None,
                    strategy: MergeStrategy::Rebase,
                    sync_upstream: true,
                    batch_size: DEFAULT_MERGE_BATCH_SIZE,
                    test_command: None,
                    next: None,
                    rework: None,
                    max_conflict_attempts: DEFAULT_MAX_CONFLICT_ATTEMPTS,
                },
            ),
            (
                "admit",
                WorkflowStep::Admit {
                    agent_slot: true,
                    max_ready: None,
                    next: None,
                },
            ),
        ] {
            let mut entries = BTreeMap::new();
            entries.insert("main".into(), "s1".into());
            let mut steps = BTreeMap::new();
            steps.insert("s1".into(), step);
            let flow = WorkflowDefinition {
                id: format!("wf-{name}"),
                name: "Test".into(),
                scope: WorkflowScope::Project,
                columns: Vec::new(),
                entries,
                steps,
                env: BTreeMap::new(),
                workspace: WorkspaceSettings::default(),
                max_steps: 20,
            };
            let err = store
                .write(|db| start_cardless(db, &project.id, flow, 1, "main", None))
                .unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains(name) && msg.contains("requires a Card"),
                "unexpected error for {name}: {msg}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn complete_cardless_agent_clears_agent_terminal_between_consecutive_agent_steps() {
        let (mut store, dir) = temp_store();
        let project = seed_project(&mut store);
        let yaml = r#"schema: 11
id: multi:agent
name: Multi-Agent Cardless
on:
  comment:
    event: github.issue.comment_created
    start_at: triage
    cardless: true
steps:
  - id: triage
    uses: agent
    with:
      profile: reviewer
      instruction: Triage the incoming comment
    next: respond
  - id: respond
    uses: agent
    with:
      profile: developer
      instruction: Draft the response
    next: done
  - id: done
    uses: complete
"#;
        let flow = crate::source::parse(yaml).unwrap().compile().unwrap();

        let mut run = store
            .write(|db| start_cardless(db, &project.id, flow.clone(), 1, "comment", None))
            .unwrap();
        assert_eq!(run.state, "waiting_agent");
        assert_eq!(run.step.as_deref(), Some("triage"));
        assert!(run.inputs.get("agent_terminal").is_none());

        // Simulate `pump_cardless_agents` recording a launched terminal for step 1.
        run.inputs["agent_terminal"] = json!({
            "address": "local/term-1",
            "step_id": "triage",
            "visit": run.visit,
            "started_at": now(),
        });
        store.write(|db| db.save(&run)).unwrap();

        // Completing step 1 must clear `agent_terminal` so step 2 starts cleanly.
        store
            .write(|db| {
                complete_cardless_agent(
                    db,
                    &mut run,
                    json!({"terminal": "local/term-1", "exit_code": 0}),
                    "completed",
                )
            })
            .unwrap();
        assert_eq!(run.state, "waiting_agent");
        assert_eq!(run.step.as_deref(), Some("respond"));
        assert!(
            run.inputs.get("agent_terminal").is_none(),
            "stale agent_terminal leaked into subsequent agent step: {:?}",
            run.inputs
        );
        assert_eq!(run.inputs["pending_agent"]["profile"], "developer");

        // Reload from SQLite to confirm cleared state was persisted.
        let reloaded: WorkflowRun = store.db().get(&run.id).unwrap();
        assert!(reloaded.inputs.get("agent_terminal").is_none());

        // Simulate launching step 2 and failing it -> `agent_terminal` is also cleared.
        run.inputs["agent_terminal"] = json!({
            "address": "local/term-2",
            "step_id": "respond",
            "visit": run.visit,
            "started_at": now(),
        });
        store
            .write(|db| {
                complete_cardless_agent(
                    db,
                    &mut run,
                    json!({"terminal": "local/term-2", "exit_code": 1, "message": "failed"}),
                    "failed",
                )
            })
            .unwrap();
        assert_eq!(run.state, "failed");
        assert!(run.inputs.get("agent_terminal").is_none());
        assert!(run.inputs.get("pending_agent").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workflow_runtime_at_stamps_all_timestamps_under_2023_epoch_without_wall_clock_leaks() {
        const T0: u64 = 1_700_000_000_000;
        const T_MAX: u64 = T0 + 100_000;

        let (mut store, dir) = temp_store();
        let project = Project {
            id: "proj-clock".into(),
            name: "Clock Demo".into(),
            directory: "/tmp/clock-demo".into(),
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: DeliveryMode::default(),
            agent_overrides: BTreeMap::new(),
            tags: Vec::new(),
            workflow: Workflow::default(),
            target_branch: None,
            created_at: T0,
        };
        store.write(|db| db.save(&project)).unwrap();

        let mut card = store
            .write(|db| canban_kanban::create_at(db, &project, "Clock Card", "Body", "user", T0))
            .unwrap();
        let config = Config::default();

        // Card-bound workflow covering Notify -> Script -> Approval -> Deliver -> Move -> Complete.
        let mut entries = BTreeMap::new();
        entries.insert("start".into(), "notify".into());
        let mut steps = BTreeMap::new();
        steps.insert(
            "notify".into(),
            WorkflowStep::Notify {
                message: "Started".into(),
                next: Some("check".into()),
            },
        );
        steps.insert(
            "check".into(),
            WorkflowStep::Script {
                run: "echo ok".into(),
                shell: None,
                working_directory: None,
                timeout_minutes: None,
                timeout_seconds: Some(60),
                continue_on_error: false,
                env: BTreeMap::new(),
                on_failure: None,
                next: Some("approve".into()),
            },
        );
        steps.insert(
            "approve".into(),
            WorkflowStep::Approval {
                title: "Approve delivery".into(),
                next: Some("deliver".into()),
                rework: None,
            },
        );
        steps.insert(
            "deliver".into(),
            WorkflowStep::Deliver {
                profile: None,
                target_branch: Some("main".into()),
                strategy: MergeStrategy::Rebase,
                sync_upstream: false,
                batch_size: DEFAULT_MERGE_BATCH_SIZE,
                test_command: None,
                next: Some("move_review".into()),
                rework: None,
                max_conflict_attempts: DEFAULT_MAX_CONFLICT_ATTEMPTS,
            },
        );
        steps.insert(
            "move_review".into(),
            WorkflowStep::Move {
                column: "review".into(),
                next: Some("done".into()),
            },
        );
        steps.insert("done".into(), WorkflowStep::Complete);
        let flow = WorkflowDefinition {
            id: "wf-clock".into(),
            name: "Clock Flow".into(),
            scope: WorkflowScope::Project,
            columns: Vec::new(),
            entries,
            steps,
            env: BTreeMap::new(),
            workspace: WorkspaceSettings::default(),
            max_steps: 20,
        };

        let noop_queue: QueueAt = |_, _, _, _, _, _, _, _, _| Ok(None);

        let mut run = store
            .write(|db| {
                start_at(
                    db,
                    &config,
                    &mut card,
                    flow.clone(),
                    1,
                    "start",
                    None,
                    false,
                    noop_queue,
                    T0 + 1_000,
                )
            })
            .unwrap();
        assert_eq!(run.state, "waiting_script");
        assert_eq!(run.created_at, T0 + 1_000);
        assert_eq!(run.updated_at, T0 + 1_000);
        assert_eq!(run.inputs["script_started_at"], json!(T0 + 1_000));

        store
            .write(|db| {
                complete_script_at(
                    db,
                    &config,
                    &mut card,
                    &mut run,
                    json!({"ok": true}),
                    &BTreeMap::new(),
                    noop_queue,
                    T0 + 2_000,
                )
            })
            .unwrap();
        assert_eq!(run.state, "waiting_approval");
        assert_eq!(run.updated_at, T0 + 2_000);

        store
            .write(|db| {
                approve_with_feedback_at(
                    db,
                    &config,
                    &mut card,
                    &mut run,
                    false,
                    None,
                    noop_queue,
                    T0 + 3_000,
                )
            })
            .unwrap();
        assert_eq!(run.state, "waiting_delivery");
        assert_eq!(run.updated_at, T0 + 3_000);
        assert_eq!(run.inputs["queued_at"], json!(T0 + 3_000));

        store
            .write(|db| hold_delivery_at(db, &mut card, &mut run, "Hold", T0 + 4_000))
            .unwrap();
        assert_eq!(run.state, "waiting_approval");
        assert_eq!(run.updated_at, T0 + 4_000);

        store
            .write(|db| {
                approve_with_feedback_at(
                    db,
                    &config,
                    &mut card,
                    &mut run,
                    false,
                    None,
                    noop_queue,
                    T0 + 5_000,
                )
            })
            .unwrap();
        assert_eq!(run.state, "waiting_delivery");
        assert_eq!(run.inputs["queued_at"], json!(T0 + 5_000));

        store
            .write(|db| {
                complete_delivery_at(
                    db,
                    &config,
                    &mut card,
                    &mut run,
                    json!({"state": "merged"}),
                    noop_queue,
                    T0 + 6_000,
                )
            })
            .unwrap();
        assert_eq!(run.state, "completed");
        assert_eq!(run.updated_at, T0 + 6_000);
        assert_eq!(card.stage, "review");

        // Cardless workflow via `*_at` also stamps strictly within 2023 epoch.
        let yaml = r#"schema: 11
id: clock:cardless
name: Clock Cardless
on:
  tick:
    event: github.issue.comment_created
    start_at: filter
    cardless: true
steps:
  - id: filter
    run: echo ok
    next: note
  - id: note
    uses: notify
    with:
      message: Done
    next: end
  - id: end
    uses: complete
"#;
        let cardless_flow = crate::source::parse(yaml).unwrap().compile().unwrap();
        let mut crun = store
            .write(|db| {
                start_cardless_at(db, &project.id, cardless_flow, 1, "tick", None, T0 + 7_000)
            })
            .unwrap();
        assert_eq!(crun.created_at, T0 + 7_000);
        assert_eq!(crun.inputs["script_started_at"], json!(T0 + 7_000));
        store
            .write(|db| {
                complete_cardless_script_at(
                    db,
                    &mut crun,
                    json!({"ok": true}),
                    &BTreeMap::new(),
                    T0 + 8_000,
                )
            })
            .unwrap();
        assert_eq!(crun.state, "completed");
        assert_eq!(crun.updated_at, T0 + 8_000);

        // Verify zero wall-clock leaks across all history entries, timeline events, and notices.
        for hist in run.history.iter().chain(crun.history.iter()) {
            let at = hist["at"].as_u64().expect("history entry must carry `at`");
            assert!(
                (T0..=T_MAX).contains(&at),
                "wall-clock leak in history: {hist}"
            );
        }
        for ev in store.db().list::<Event>().unwrap() {
            assert!(
                (T0..=T_MAX).contains(&ev.created_at),
                "wall-clock leak in event {}: {}",
                ev.kind,
                ev.created_at
            );
        }
        for notice in store.db().list::<Notice>().unwrap() {
            assert!(
                (T0..=T_MAX).contains(&notice.created_at),
                "wall-clock leak in notice {}: {}",
                notice.message,
                notice.created_at
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
