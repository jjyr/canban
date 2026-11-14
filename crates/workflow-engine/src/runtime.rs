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
fn log(
    db: Db<'_>,
    c: &Card,
    kind: &str,
    actor: &str,
    message: &str,
    terminal_id: Option<&str>,
    mut data: Value,
) -> Result<()> {
    if let Some(tid) = terminal_id
        && data.get("terminal_id").is_none()
    {
        data["terminal_id"] = json!(tid);
    }
    db.append(canban_timeline::event(
        &c.project_id,
        &c.id,
        kind,
        actor,
        message,
        data,
    ))?;
    Ok(())
}
fn attention(db: Db<'_>, card: &mut Card, message: &str) -> Result<()> {
    card.attention = Some(message.into());
    if card.human_gate.is_none() {
        card.human_gate = Some(HumanGate {
            reason: message.into(),
            terminal_id: None,
            next_stage: None,
        });
    }
    db.save(&canban_notifications::attention(
        &card.project_id,
        &card.id,
        message,
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
fn record(db: Db<'_>, run: &WorkflowRun, kind: &str, message: &str) -> Result<()> {
    let card: Card = db.get(&run.card_id)?;
    log(
        db,
        &card,
        kind,
        if kind == "workflow.approved" {
            "user"
        } else {
            "system"
        },
        message,
        run.terminal_id.as_deref(),
        json!({"workflow_run_id":run.id,"step":run.step,"visit":run.visit,"state":run.state}),
    )
}
fn next(run: &mut WorkflowRun, to: Option<String>) {
    if let Some(step) = &run.step {
        run.history
            .push(json!({"step":step,"visit":run.visit,"event":"completed","at":now()}));
    }
    run.step = to;
    run.terminal_id = None;
    run.error = None;
    run.state = "running".into();
    run.visit += 1;
    run.updated_at = now();
}

pub fn stop_card(db: Db<'_>, card: &str, state: &str) -> Result<()> {
    for mut run in db
        .list::<WorkflowRun>()?
        .into_iter()
        .filter(|r| r.card_id == card && r.active())
    {
        run.state = state.into();
        run.updated_at = now();
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
        db.save(&run)?;
        record(db, &run, "workflow.stopped", state)?;
    }
    Ok(())
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
    ensure!(card.active(), "Restore the Card before starting a workflow");
    ensure!(!card.merged(), "Merged Cards do not start new workflows");
    ensure!(
        card.human_gate.is_none(),
        "Resolve the existing human decision first"
    );
    ensure!(
        !db.list::<WorkflowRun>()?
            .iter()
            .any(|r| r.card_id == card.id && r.active()),
        "Card already has an active workflow"
    );
    let step = if let Some(s) = flow.entries.get(entry) {
        s.clone()
    } else if flow.steps.contains_key(entry) {
        entry.to_owned()
    } else {
        bail!("Unknown workflow entry")
    };
    let mut run = WorkflowRun {
        id: id(),
        project_id: card.project_id.clone(),
        card_id: card.id.clone(),
        workflow_id: flow.id.clone(),
        definition_revision: revision,
        definition: flow,
        state: "running".into(),
        step: Some(step),
        visit: 1,
        terminal_id: None,
        trigger_event_id: event,
        input_digest: digest(db, card),
        inputs: json!({"title":card.title,"body":card.body,"content":card.content,"legacy":legacy}),
        outputs: BTreeMap::new(),
        history: Vec::new(),
        error: None,
        created_at: now(),
        updated_at: now(),
    };
    db.save(&run)?;
    record(db, &run, "workflow.started", entry)?;
    drive(db, config, card, &mut run, queue)?;
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
    ensure!(card.active(), "Restore the Card before starting a workflow");
    ensure!(!card.merged(), "Merged Cards do not start new workflows");
    ensure!(
        card.human_gate.is_none(),
        "Resolve the existing human decision first"
    );
    ensure!(
        !db.list::<WorkflowRun>()?
            .iter()
            .any(|r| r.card_id == card.id && r.active()),
        "Card already has an active workflow"
    );
    let step = if let Some(s) = flow.entries.get(entry) {
        s.clone()
    } else if flow.steps.contains_key(entry) {
        entry.to_owned()
    } else {
        bail!("Unknown workflow entry")
    };
    let mut run = WorkflowRun {
        id: id(),
        project_id: card.project_id.clone(),
        card_id: card.id.clone(),
        workflow_id: flow.id.clone(),
        definition_revision: revision,
        definition: flow,
        state: "running".into(),
        step: Some(step),
        visit: 1,
        terminal_id: None,
        trigger_event_id: event,
        input_digest: digest(db, card),
        inputs: json!({"title":card.title,"body":card.body,"content":card.content,"legacy":legacy,"adopted":true}),
        outputs: BTreeMap::new(),
        history: Vec::new(),
        error: None,
        created_at: now(),
        updated_at: now(),
    };
    db.save(&run)?;
    record(db, &run, "workflow.started", entry)?;
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
                    .push(json!({"step":key,"visit":run.visit,"at":now(),"event":"entered"}));
                let mut card_val = serde_json::to_value(&*card)?;
                if let Some(obj) = card_val.as_object_mut() {
                    obj.insert("source_trust".into(), json!(card.trust_level()));
                }
                let context = json!({"inputs":run.inputs,"outputs":run.outputs,"card":card_val});
                next(
                    &mut run,
                    Some(if condition_matches(context.pointer(&pointer), &equals) {
                        then
                    } else {
                        otherwise
                    }),
                );
            }
            WorkflowStep::Notify { message, next: to } => {
                run.history
                    .push(json!({"step":key,"visit":run.visit,"at":now(),"event":"entered"}));
                db.save(&canban_notifications::attention(
                    &card.project_id,
                    &card.id,
                    &message,
                ))?;
                record(db, &run, "workflow.notified", &message)?;
                next(&mut run, to);
            }
            WorkflowStep::Admit { next: to, .. } => {
                // An already-live session holds its terminal; bypass capacity admission.
                run.history
                    .push(json!({"step":key,"visit":run.visit,"at":now(),"event":"entered"}));
                next(&mut run, to);
            }
            WorkflowStep::Agent {
                next: continuation, ..
            } => {
                run.history
                    .push(json!({"step":key,"visit":run.visit,"at":now(),"event":"entered"}));
                let step_ref = format!("{key}:{}", run.visit);
                if let Some(entry_ref) = adopted_terminal.take() {
                    if let Some(agent) = &mut entry_ref.agent {
                        agent.workflow_run_id = Some(run.id.clone());
                        agent.workflow_step = Some(step_ref);
                        agent.stage = card.stage.clone();
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
                run.updated_at = now();
                db.save(&run)?;
                db.save(card)?;
                claimed = true;
                break;
            }
            _ => break,
        }
    }
    if !claimed {
        drive(db, config, card, &mut run, queue)?;
    }
    Ok(run)
}
fn drive(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    queue: Queue,
) -> Result<()> {
    while run.state == "running" {
        if let Some(removed) = run.inputs["stop_after_step"].as_str()
            && run.step.as_deref() != Some(removed)
        {
            run.step = None;
            run.state = "cancelled".into();
            record(db, run, "workflow.stopped", "Step removed from workflow")?;
            break;
        }
        if run.visit > run.definition.max_steps {
            run.state = "failed".into();
            run.error = Some("Workflow execution limit reached".into());
            attention(db, card, run.error.as_deref().unwrap())?;
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
            .push(json!({"step":key,"visit":run.visit,"at":now(),"event":"entered"}));
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
                let stage = Stage {
                    id: card.stage.clone(),
                    name: card.stage.clone(),
                    execute: true,
                    profile,
                    instruction: format!("{instruction}{rework_note}{prior_steps}"),
                    on_completed: None,
                    require_human_review: false,
                };
                let trigger = if run.inputs["legacy"] == true {
                    ExecutionTrigger::StageEntry
                } else {
                    ExecutionTrigger::Workflow
                };
                if let Some(mut entry) =
                    queue(db, config, card, &project, &stage, trigger, None, None)?
                {
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
                run.inputs["script_started_at"] = json!(now());
                run.input_digest = digest(db, card);
                run.state = "waiting_script".into();
                record(db, run, "workflow.script_started", &format!("Running script {key}"))?;
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
                    log(
                        db,
                        card,
                        "card.stage_changed",
                        "system",
                        &format!("{} → {}", from, card.stage),
                        run.terminal_id.as_deref(),
                        json!({"from":from,"to":card.stage,"entry_id":id(),"reason":"workflow_completed","workflow_run_id":run.id,"step":key}),
                    )?;
                }
                next(run, to);
            }
            WorkflowStep::Approval { title, .. } => {
                attention(db, card, &title)?;
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
                record(db, run, "workflow.waiting_for_human", &title)?;
            }
            WorkflowStep::Deliver {
                target_branch,
                strategy,
                sync_upstream,
                next: to,
                ..
            } => {
                if card.hold_auto_merge {
                    let title = "Auto-merge held by user; approve or resume to merge";
                    attention(db, card, title)?;
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
                    record(db, run, "workflow.waiting_for_human", title)?;
                } else {
                    if run.inputs["queued_at"].as_u64().is_none() {
                        run.inputs["queued_at"] = json!(now());
                    }
                    if let Some(obj) = run.inputs.as_object_mut() {
                        obj.remove("queue_pause_reason");
                    }
                    run.inputs["pending_next"] = json!(to);
                    run.inputs["target_branch"] = json!(target_branch);
                    run.inputs["strategy"] = json!(strategy);
                    run.inputs["sync_upstream"] = json!(sync_upstream);
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
                next(
                    run,
                    Some(if condition_matches(context.pointer(&pointer), &equals) {
                        then
                    } else {
                        otherwise
                    }),
                );
            }
            WorkflowStep::Notify { message, next: to } => {
                db.save(&canban_notifications::attention(
                    &card.project_id,
                    &card.id,
                    &message,
                ))?;
                record(db, run, "workflow.notified", &message)?;
                next(run, to);
            }
            WorkflowStep::Admit { next: to, .. } => {
                // The AppServer admission pump releases runs in Board order.
                run.inputs["pending_next"] = json!(to);
                run.state = "waiting_capacity".into();
            }
            WorkflowStep::Complete => {
                run.history
                    .push(json!({"step":key,"visit":run.visit,"event":"completed","at":now()}));
                run.step = None;
                run.state = "completed".into();
            }
        }
    }
    run.updated_at = now();
    db.save(run)?;
    db.save(card)?;
    if run.state == "completed" {
        record(db, run, "workflow.completed", "Workflow completed")?;
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
    let agent = terminal.and_then(|t| t.agent.as_ref());
    let run_opt: Option<WorkflowRun> = if let Some(a) = agent
        && let Some(id) = &a.workflow_run_id
    {
        Some(db.get(id)?)
    } else if let Some(id) = &report.workflow_run_id {
        Some(db.get(id)?)
    } else {
        db.list::<WorkflowRun>()?.into_iter().find(|r| {
            r.card_id == card.id
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
                run.history.push(json!({"step":key,"visit":run.visit,"event":"returned_to_backlog","report_id":report.id,"at":now()}));
            }
            run.state = "cancelled".into();
            run.updated_at = now();
            db.save(&run)?;
        } else if matches!(report.state.as_str(), "blocked" | "failed") {
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
        attention(db, card, run.error.as_deref().unwrap())?;
        db.save(&run)?;
        record(db, &run, "workflow.stale", "Input changed")?;
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
            json!({"step":key,"visit":run.visit,"event":"conflict_resolved","report_id":report.id,"at":now()}),
        );
        let to = run.definition.steps[&key].next().map(str::to_owned);
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
        run.visit += 1;
        run.terminal_id = term_id;
        run.error = None;
        run.state = "running".into();
        return drive(db, config, card, &mut run, queue);
    }
    run.outputs.insert(key.clone(),json!({"report_id":report.id,"path":report.path,"message":report.message,"terminal_id":term_id,"run_number":run_num}));
    run.history.push(
        json!({"step":key,"visit":run.visit,"event":"completed","report_id":report.id,"at":now()}),
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
    next(&mut run, to);
    // Keep the completing terminal as decision context when the next node is approval or delivery.
    if run.step.as_ref().is_some_and(|k| {
        matches!(
            run.definition.steps[k],
            WorkflowStep::Approval { .. } | WorkflowStep::Deliver { .. }
        )
    }) {
        run.terminal_id = term_id;
    }
    drive(db, config, card, &mut run, queue)
}

/// Compatibility callers (Review/merge) resolve the same run, never a stage executor.
pub fn advance(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    terminal: Option<&TerminalEntry>,
    queue: Queue,
) -> Result<()> {
    let agent = terminal.and_then(|t| t.agent.as_ref());
    let run_opt: Option<WorkflowRun> = if let Some(a) = agent
        && let Some(id) = &a.workflow_run_id
    {
        Some(db.get(id)?)
    } else {
        db.list::<WorkflowRun>()?
            .into_iter()
            .find(|r| r.card_id == card.id && r.active())
    };
    if let Some(mut run) = run_opt {
        if !run.active() {
            return Ok(());
        }
        if run.state == "waiting_approval" {
            return approve_with_feedback(db, config, card, &mut run, false, None, queue);
        }
        if run.state == "blocked" && agent.is_none_or(|a| a.state == "completed") {
            ensure!(
                digest(db, card) == run.input_digest,
                "Task changed; request rework instead"
            );
            let to = run.inputs["pending_next"].as_str().map(str::to_owned);
            next(&mut run, to);
            return drive(db, config, card, &mut run, queue);
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
    approve_with_feedback(db, config, card, run, rework, None, queue)
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
                on_failure: Some(to), ..
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
            next: to,
            ..
        } = &run.definition.steps[&key]
        {
            run.history.push(json!({"step":key,"visit":run.visit,"decision":"approved","outputs":run.outputs,"at":now()}));
            card.human_gate = None;
            card.attention = None;
            card.hold_auto_merge = false;
            card.revision += 1;
            run.input_digest = digest(db, card);
            if let Some(obj) = run.inputs.as_object_mut() {
                obj.remove("queue_pause_reason");
            }
            run.inputs["queued_at"] = json!(now());
            run.inputs["pending_next"] = json!(to);
            run.inputs["target_branch"] = json!(target_branch);
            run.inputs["strategy"] = json!(strategy);
            run.inputs["sync_upstream"] = json!(sync_upstream);
            run.state = "waiting_delivery".into();
            run.updated_at = now();
            db.save(run)?;
            db.save(card)?;
            record(db, run, "workflow.approved", "Human approved delivery")?;
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
            run.history.push(json!({"step":key,"visit":run.visit,"decision":"retry","outputs":run.outputs,"at":now()}));
            card.human_gate = None;
            card.attention = None;
            card.revision += 1;
            run.error = None;
            run.input_digest = digest(db, card);
            run.step = Some(key.clone());
            run.terminal_id = None;
            run.state = "running".into();
            run.visit += 1;
            run.updated_at = now();
            record(db, run, "workflow.approved", "Retrying script")?;
            return drive(db, config, card, run, queue);
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
            } else if run.inputs["rework_feedback"].as_str().is_none_or(|s| s.trim().is_empty())
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
    run.history.push(json!({"step":key,"visit":run.visit,"decision":if rework{"rework"}else{"approved"},"feedback":clean_feedback,"outputs":run.outputs,"at":now()}));
    card.human_gate = None;
    card.attention = None;
    card.revision += 1;
    run.input_digest = digest(db, card);
    next(run, to);
    record(
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
    )?;
    drive(db, config, card, run, queue)
}
pub fn complete_delivery(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    output: Value,
    queue: Queue,
) -> Result<()> {
    let key = run.step.clone().context("Missing Deliver step")?;
    if let Some(obj) = run.inputs.as_object_mut() {
        obj.remove("queued_at");
        obj.remove("queue_pause_reason");
    }
    run.outputs.insert(key.clone(), output);
    let to = run.inputs["pending_next"].as_str().map(str::to_owned);
    next(run, to);
    drive(db, config, card, run, queue)
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
    ensure!(
        run.state == "waiting_script",
        "Workflow is not running a script"
    );
    let key = run.step.clone().context("Missing Script step")?;
    ensure!(
        matches!(run.definition.steps.get(&key), Some(WorkflowStep::Script { .. })),
        "Current step is not a Script step"
    );
    merge_env_into_run(run, exported_env);
    if let Some(obj) = run.inputs.as_object_mut() {
        obj.remove("script_started_at");
        obj.remove("script_terminal");
    }
    run.outputs.insert(key, output);
    let to = run.inputs["pending_next"].as_str().map(str::to_owned);
    next(run, to);
    drive(db, config, card, run, queue)
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
        .push(json!({"step":key,"visit":run.visit,"event":"failed","outputs":output,"at":now()}));
    run.outputs.insert(key.clone(), output.clone());
    log(
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
        run.updated_at = now();
        return drive(db, config, card, run, queue);
    }

    attention(db, card, summary)?;
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
    run.updated_at = now();
    db.save(run)?;
    db.save(card)?;
    record(db, run, "workflow.waiting_for_human", summary)?;
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
    record(db, run, "workflow.admitted", "Capacity available")?;
    next(run, to);
    drive(db, config, card, run, queue)
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
    let Some(mut entry) = queue(
        db,
        config,
        card,
        &project,
        &stage,
        ExecutionTrigger::Workflow,
        None,
        None,
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
        json!({"step":key,"visit":run.visit,"decision":"merge_conflict","profile_id":agent_id,"files":files,"at":now()}),
    );
    card.human_gate = None;
    card.attention = None;
    card.revision += 1;
    run.terminal_id = Some(entry.id.clone());
    run.state = "waiting_agent".into();
    run.error = None;
    run.input_digest = digest(db, card);
    run.updated_at = now();
    log(
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
    attention(db, card, reason)?;
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
    run.updated_at = now();
    db.save(run)?;
    db.save(card)?;
    record(db, run, "workflow.waiting_for_human", reason)?;
    Ok(())
}

pub fn initialize(db: Db<'_>, config: &Config) -> Result<()> {
    let _ = (db, config);
    Ok(())
}

pub fn reconcile(db: Db<'_>) -> Result<()> {
    for mut run in db.list::<WorkflowRun>()?.into_iter().filter(|r| r.active()) {
        let card: Card = db.get(&run.card_id)?;
        if !card.active() {
            stop_card(db, &card.id, "cancelled")?;
            continue;
        }
        if let Some(id) = &run.terminal_id
            && let Ok(entry) = db.get::<TerminalEntry>(id)
            && let Some(agent) = &entry.agent
        {
            if matches!(run.state.as_str(), "waiting_agent" | "failed")
                && agent.pending.is_none()
                && matches!(agent.state.as_str(), "failed" | "unknown" | "needs_report")
            {
                run.state = if agent.state == "failed" {
                    "failed"
                } else {
                    "waiting_agent"
                }
                .into();
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
