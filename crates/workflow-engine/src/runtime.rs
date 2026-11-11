//! One durable interpreter for converted stage templates and user definitions.
use anyhow::{Context, Result, bail, ensure};
use canban_storage::Db;
use canban_types::legacy_definition;
use canban_types::*;
use serde_json::{Value, json};
pub type Queue = for<'a> fn(
    Db<'a>,
    &Config,
    &mut Card,
    &Project,
    &Stage,
    &str,
    Option<&str>,
    Option<String>,
) -> Result<Option<Request>>;
fn log(
    db: Db<'_>,
    c: &Card,
    kind: &str,
    actor: &str,
    message: &str,
    request: Option<String>,
    data: Value,
) -> Result<()> {
    db.append(canban_timeline::event(
        &c.project_id,
        &c.id,
        kind,
        actor,
        message,
        request,
        data,
    ))?;
    Ok(())
}
fn attention(db: Db<'_>, card: &mut Card, message: &str) -> Result<()> {
    card.attention = Some(message.into());
    if card.human_gate.is_none() {
        card.human_gate = Some(HumanGate {
            reason: message.into(),
            request_id: None,
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
fn definition(db: Db<'_>, project: &Project) -> Result<(WorkflowDefinition, u64)> {
    if let Ok(binding) = db.get::<WorkflowBinding>(&project.id) {
        let w = latest(db, &binding.workflow_id)?;
        return Ok((w.definition, w.revision));
    }
    Ok((
        legacy_definition(&project.workflow, format!("delivery:{}", project.id)),
        0,
    ))
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
        run.request_id.clone(),
        json!({"workflow_run_id":run.id,"step":run.step,"visit":run.visit,"state":run.state}),
    )
}
fn next(run: &mut WorkflowRun, to: Option<String>) {
    if let Some(step) = &run.step {
        run.history
            .push(json!({"step":step,"visit":run.visit,"event":"completed","at":now()}));
    }
    run.step = to;
    run.request_id = None;
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
        for mut req in db
            .list::<Request>()?
            .into_iter()
            .filter(|r| r.workflow_run_id.as_deref() == Some(&run.id) && r.state == "queued")
        {
            req.state = "cancelled".into();
            db.save(&req)?;
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
    let step = flow
        .entries
        .get(entry)
        .context("Unknown workflow entry")?
        .clone();
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
        request_id: None,
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
pub fn enter(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    entry: &str,
    queue: Queue,
) -> Result<()> {
    let project: Project = db.get(&card.project_id)?;
    if db
        .list::<WorkflowSource>()?
        .iter()
        .any(|f| f.project_id == project.id)
    {
        return Ok(());
    }
    let (flow, revision) = definition(db, &project)?;
    if flow.entries.contains_key(entry) {
        start(db, config, card, flow, revision, entry, None, true, queue)?;
    }
    Ok(())
}
fn drive(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    queue: Queue,
) -> Result<()> {
    while run.state == "running" {
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
            } => {
                let project: Project = db.get(&card.project_id)?;
                let rework_note = run.inputs["rework_feedback"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| format!("\n\nHuman rework feedback for this visit:\n{}", s.trim()))
                    .unwrap_or_default();
                if run.inputs.get("rework_feedback").is_some() {
                    run.inputs["rework_feedback"] = Value::Null;
                }
                let stage = Stage {
                    id: card.stage.clone(),
                    name: card.stage.clone(),
                    execute: true,
                    profile,
                    instruction: format!(
                        "{}{}\n\nWorkflow {} revision {}, step {}, visit {}.\nEarlier step results (read report files/resources when needed):\n{}",
                        instruction,
                        rework_note,
                        run.workflow_id,
                        run.definition_revision,
                        key,
                        run.visit,
                        serde_json::to_string(&run.outputs)?
                    ),
                    on_completed: None,
                    require_human_review: false,
                };
                let trigger = if run.inputs["legacy"] == true {
                    "stage_entry"
                } else {
                    "workflow"
                };
                if let Some(mut req) =
                    queue(db, config, card, &project, &stage, trigger, None, None)?
                {
                    req.workflow_run_id = Some(run.id.clone());
                    req.workflow_step = Some(format!("{key}:{}", run.visit));
                    // Retain legacy snapshots for backward recovery; the engine owns progression.
                    if run.inputs["legacy"] == true {
                        let following = run.definition.steps[&key]
                            .next()
                            .and_then(|n| run.definition.steps.get(n));
                        req.require_human_review =
                            matches!(following, Some(WorkflowStep::Approval { .. }));
                        let movement = match following {
                            Some(WorkflowStep::Approval { next, .. }) => {
                                next.as_ref().and_then(|n| run.definition.steps.get(n))
                            }
                            other => other,
                        };
                        req.on_completed = match movement {
                            Some(WorkflowStep::Move { column, .. }) => Some(column.clone()),
                            _ => None,
                        };
                    }
                    db.save(&req)?;
                    db.conn.execute("UPDATE events SET data=json_set(data,'$.data.workflow_run_id',?2,'$.data.workflow_step',?3,'$.data.origin','workflow') WHERE json_extract(data,'$.request_id')=?1 AND json_extract(data,'$.kind')='execution.queued'",[req.id.as_str(),run.id.as_str(),req.workflow_step.as_deref().unwrap_or("")])?;
                    run.request_id = Some(req.id);
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
                        run.request_id.clone(),
                        json!({"from":from,"to":card.stage,"entry_id":id(),"reason":"workflow_completed","workflow_run_id":run.id,"step":key}),
                    )?;
                }
                next(run, to);
            }
            WorkflowStep::Approval { title, .. } => {
                attention(db, card, &title)?;
                card.human_gate = Some(HumanGate {
                    reason: title.clone(),
                    request_id: run.request_id.clone(),
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
                next: to,
                ..
            } => {
                if card.hold_auto_merge {
                    let title = "Auto-merge held by user; approve or resume to merge";
                    attention(db, card, title)?;
                    card.human_gate = Some(HumanGate {
                        reason: title.into(),
                        request_id: run.request_id.clone(),
                        next_stage: None,
                    });
                    card.revision += 1;
                    db.save(card)?;
                    run.state = "waiting_approval".into();
                    record(db, run, "workflow.waiting_for_human", title)?;
                } else {
                    run.inputs["pending_next"] = json!(to);
                    run.inputs["target_branch"] = json!(target_branch);
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
                    Some(if context.pointer(&pointer) == Some(&equals) {
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
    request: &Request,
    report: &Report,
    queue: Queue,
) -> Result<()> {
    let Some(id) = &request.workflow_run_id else {
        return Ok(());
    };
    let mut run: WorkflowRun = db.get(id)?;
    if !run.active() || run.request_id.as_deref() != Some(&request.id) {
        return Ok(());
    }
    if report.state != "completed" {
        if matches!(report.state.as_str(), "blocked" | "failed") {
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
    run.outputs.insert(key.clone(),json!({"report_id":report.id,"path":report.path,"message":report.message,"request_id":request.id,"attempt":report.attempt}));
    run.history.push(
        json!({"step":key,"visit":run.visit,"event":"completed","report_id":report.id,"at":now()}),
    );
    let to = run.definition.steps[&key].next().map(str::to_owned);
    // A previously reported block must still be approved even if the Agent later completes.
    if let Some(gate) = &mut card.human_gate {
        if gate.request_id.is_none() {
            gate.request_id = Some(request.id.clone());
        }
        gate.next_stage = request.on_completed.clone();
        run.state = "blocked".into();
        run.inputs["pending_next"] = json!(to);
        db.save(&run)?;
        return Ok(());
    }
    next(&mut run, to);
    // Keep the completing request as decision context when the next node is approval or delivery.
    if run.step.as_ref().is_some_and(|k| {
        matches!(
            run.definition.steps[k],
            WorkflowStep::Approval { .. } | WorkflowStep::Deliver { .. }
        )
    }) {
        run.request_id = Some(request.id.clone());
    }
    drive(db, config, card, &mut run, queue)
}

/// Compatibility callers (Review/merge) resolve the same run, never a stage executor.
pub fn advance(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    request: &Request,
    queue: Queue,
) -> Result<()> {
    if let Some(id) = &request.workflow_run_id {
        let mut run: WorkflowRun = db.get(id)?;
        if !run.active() {
            return Ok(());
        }
        if run.state == "waiting_approval" {
            return approve_with_feedback(db, config, card, &mut run, false, None, queue);
        }
        if run.state == "blocked" && request.state == "completed" {
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
    let to = if rework {
        match &run.definition.steps[&key] {
            WorkflowStep::Approval {
                rework: Some(to), ..
            }
            | WorkflowStep::Deliver {
                rework: Some(to), ..
            } => Some(to.clone()),
            WorkflowStep::Agent { .. } if run.state == "stale" => Some(key.clone()),
            _ => bail!("No rework path configured"),
        }
    } else {
        ensure!(
            digest(db, card) == run.input_digest,
            "Approval is stale; request changes instead"
        );
        ensure!(
            run.state == "waiting_approval",
            "Stale work cannot be approved"
        );
        if let WorkflowStep::Deliver {
            target_branch,
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
            run.inputs["pending_next"] = json!(to);
            run.inputs["target_branch"] = json!(target_branch);
            run.state = "waiting_delivery".into();
            run.updated_at = now();
            db.save(run)?;
            db.save(card)?;
            record(db, run, "workflow.approved", "Human approved delivery")?;
            return Ok(());
        }
        run.definition.steps[&key].next().map(str::to_owned)
    };
    let clean_feedback = feedback.map(str::trim).filter(|s| !s.is_empty());
    if rework && let Some(note) = clean_feedback {
        run.inputs["rework_feedback"] = json!(note);
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
    run.outputs.insert(key.clone(), output);
    let to = run.inputs["pending_next"].as_str().map(str::to_owned);
    next(run, to);
    drive(db, config, card, run, queue)
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
        request_id: run.request_id.clone(),
        next_stage: None,
    });
    card.revision += 1;
    run.state = "waiting_approval".into();
    run.updated_at = now();
    db.save(run)?;
    db.save(card)?;
    record(db, run, "workflow.waiting_for_human", reason)?;
    Ok(())
}

pub fn initialize(db: Db<'_>, config: &Config) -> Result<()> {
    // Adopt existing active/queued assignments and gates once, preserving all runtime IDs.
    for mut req in db.list::<Request>()?.into_iter().rev().filter(|r| {
        r.trigger == "stage_entry"
            && r.workflow_run_id.is_none()
            && matches!(
                r.state.as_str(),
                "queued"
                    | "starting"
                    | "running"
                    | "working"
                    | "blocked"
                    | "failed"
                    | "unknown"
                    | "completed"
            )
    }) {
        let card: Card = db.get(&req.card_id)?;
        let gated = card
            .human_gate
            .as_ref()
            .is_some_and(|g| g.request_id.as_deref() == Some(&req.id));
        if !card.active() || card.stage != req.stage || (req.state == "completed" && !gated) {
            continue;
        }
        if db
            .list::<WorkflowRun>()?
            .iter()
            .any(|r| r.card_id == card.id && r.active())
        {
            continue;
        }
        let project: Project = db.get(&card.project_id)?;
        let mut captured = project.workflow.clone();
        if let Some(stage) = captured.stages.iter_mut().find(|s| s.id == req.stage) {
            stage.execute = true;
            stage.profile = Some(req.profile.id.clone());
            stage.instruction = req.instruction.clone();
            stage.on_completed = req.on_completed.clone();
            stage.require_human_review = req.require_human_review;
        }
        let mut flow = legacy_definition(&captured, format!("delivery:{}", project.id));
        // The assignment snapshot wins over subsequently edited stage settings.
        let key = format!("agent:{}", req.stage);
        if let Some(WorkflowStep::Agent {
            profile,
            instruction,
            ..
        }) = flow.steps.get_mut(&key)
        {
            *profile = Some(req.profile.id.clone());
            *instruction = req.instruction.clone();
        }
        let approval = format!("approval:{}", req.stage);
        let completed = req.state == "completed";
        if completed && !flow.steps.contains_key(&approval) {
            flow.steps.insert(
                approval.clone(),
                WorkflowStep::Approval {
                    title: card.human_gate.as_ref().unwrap().reason.clone(),
                    next: flow
                        .steps
                        .get(&key)
                        .and_then(|s| s.next())
                        .map(str::to_owned),
                    rework: None,
                },
            );
        }
        let run = WorkflowRun {
            id: id(),
            project_id: card.project_id.clone(),
            card_id: card.id.clone(),
            workflow_id: flow.id.clone(),
            definition_revision: 0,
            definition: flow,
            state: if completed {
                "waiting_approval".into()
            } else if gated {
                "blocked".into()
            } else {
                "waiting_agent".into()
            },
            step: Some(if completed { approval } else { key }),
            visit: 1,
            request_id: Some(req.id.clone()),
            trigger_event_id: None,
            input_digest: digest(db, &card),
            inputs: json!({"legacy":true,"title":card.title,"body":card.body}),
            outputs: BTreeMap::new(),
            history: vec![json!({"event":"adopted","request_id":req.id})],
            error: None,
            created_at: req.created_at,
            updated_at: now(),
        };
        req.workflow_run_id = Some(run.id.clone());
        req.workflow_step = run.step.clone();
        db.save(&run)?;
        db.save(&req)?;
        db.save(&card)?;
    }
    let _ = config;
    Ok(())
}

pub fn reconcile(db: Db<'_>) -> Result<()> {
    for mut run in db.list::<WorkflowRun>()?.into_iter().filter(|r| r.active()) {
        let card: Card = db.get(&run.card_id)?;
        if !card.active() {
            stop_card(db, &card.id, "cancelled")?;
            continue;
        }
        if let Some(id) = &run.request_id {
            let req: Request = db.get(id)?;
            if matches!(run.state.as_str(), "waiting_agent" | "failed")
                && matches!(req.state.as_str(), "failed" | "unknown" | "needs_report")
            {
                run.state = if req.state == "failed" {
                    "failed"
                } else {
                    "waiting_agent"
                }
                .into();
                run.error = Some(format!(
                    "Execution {}: inspect or explicitly retry",
                    req.state
                ));
                db.save(&run)?;
            } else if run.state == "failed" && req.state == "queued" {
                run.state = "waiting_agent".into();
                run.error = None;
                db.save(&run)?;
            }
        }
    }
    Ok(())
}
