//! One durable interpreter for converted stage templates and user definitions.
use anyhow::{Context, Result, bail, ensure};
use canban_storage::Db;
use canban_types::*;
use serde_json::{Value, json};
pub type Queue = for<'a> fn(
    Db<'a>,
    &Config,
    &mut Card,
    &Project,
    &Stage,
    RequestTrigger,
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
            } => {
                let project: Project = db.get(&card.project_id)?;
                let rework_label = if run.inputs["rework_origin"] == "merge_conflict" {
                    "Merge conflict to resolve before delivery"
                } else {
                    "Human rework feedback for this visit"
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
                    RequestTrigger::StageEntry
                } else {
                    RequestTrigger::Workflow
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
                sync_upstream,
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
        if report.state == "backlog" {
            if let Some(key) = run.step.clone() {
                run.outputs.insert(
                    key.clone(),
                    json!({"report_id":report.id,"path":report.path,"message":report.message,"request_id":request.id,"attempt":report.attempt,"state":"backlog"}),
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
            run.inputs["pending_next"] = json!(to);
            run.inputs["target_branch"] = json!(target_branch);
            run.inputs["sync_upstream"] = json!(sync_upstream);
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

/// Routes a delivery merge conflict to the Deliver step's rework target so an
/// Agent resolves it in the Card worktree. Returns `false` (caller should hold
/// for a human) when no rework path exists or the attempt budget is spent.
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
        rework: Some(to),
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
    let note = format!(
        "Queue merge found conflicts integrating `{target_branch}` ({target_head}) into this Card worktree.\n\
         Conflicted files: {}\n\
         The worktree may still be mid-merge. Resolve every conflict while preserving both this Card's intent and the target branch changes, \
         run the relevant tests, and commit the merge (if no merge is in progress, run `git merge {target_branch}` first). \
         Do not push; Queue merge will retry delivery automatically after you report completion.",
        files.join(", ")
    );
    run.inputs["conflict_attempts"] = json!(attempts + 1);
    run.inputs["rework_feedback"] = json!(note);
    run.inputs["rework_origin"] = json!("merge_conflict");
    run.outputs.insert(
        key.clone(),
        json!({"state":"conflict","target_branch":target_branch,"target_head":target_head,"files":files,"attempt":attempts + 1}),
    );
    run.history.push(
        json!({"step":key,"visit":run.visit,"decision":"merge_conflict","files":files,"at":now()}),
    );
    card.human_gate = None;
    card.attention = None;
    card.revision += 1;
    run.input_digest = digest(db, card);
    log(
        db,
        card,
        "review.merge_conflict",
        "system",
        &format!(
            "Merge conflict with {target_branch}; Agent will resolve it (attempt {}/{max_conflict_attempts})",
            attempts + 1
        ),
        None,
        json!({"workflow_run_id":run.id,"target_branch":target_branch,"target_head":target_head,"files":files,"attempt":attempts + 1}),
    )?;
    next(run, Some(to));
    drive(db, config, card, run, queue)?;
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
