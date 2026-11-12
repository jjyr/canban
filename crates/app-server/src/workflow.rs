//! AppServer workflow operations; interpretation lives in canban-workflow-engine.
use super::*;
use canban_workflow_engine::runtime as engine;
fn latest(db: Db<'_>, id: &str) -> Result<WorkflowRevision> {
    engine::latest(db, id)
}
pub(super) fn stop_card(db: Db<'_>, card: &str, state: &str) -> Result<()> {
    engine::stop_card(db, card, state)
}
pub(super) fn initialize(db: Db<'_>, config: &Config) -> Result<()> {
    engine::initialize(db, config)
}
pub(super) fn reported(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    request: &Request,
    report: &Report,
) -> Result<()> {
    engine::reported(db, config, card, request, report, enqueue)
}
pub(super) fn advance(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    request: &Request,
) -> Result<()> {
    engine::advance(db, config, card, request, enqueue)
}
fn approve(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    run: &mut WorkflowRun,
    rework: bool,
    feedback: Option<&str>,
) -> Result<()> {
    engine::approve_with_feedback(db, config, card, run, rework, feedback, enqueue)
}
#[allow(
    clippy::too_many_arguments,
    reason = "Thin entry adapter forwards the captured workflow invocation"
)]
fn start(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    flow: WorkflowDefinition,
    revision: u64,
    entry: &str,
    event: Option<String>,
    legacy: bool,
) -> Result<WorkflowRun> {
    engine::start(
        db, config, card, flow, revision, entry, event, legacy, enqueue,
    )
}
pub(super) fn published(db: Db<'_>, id: &str) -> Result<WorkflowRevision> {
    latest(db, id)
}
pub(super) fn start_event(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    w: WorkflowRevision,
    entry: &str,
    event: &str,
) -> Result<WorkflowRun> {
    start(
        db,
        config,
        card,
        w.definition,
        w.revision,
        entry,
        Some(event.into()),
        w.workflow_id.starts_with("delivery:"),
    )
}
pub(super) fn reconcile(db: Db<'_>) -> Result<()> {
    engine::reconcile(db)
}
impl AppServer {
    pub(super) fn workflow_call(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "workflow.template" => Ok(json!(self.default_workflow()?)),
            "workflow.schema" => Ok(
                json!({"schema_version":1,"authoring_format":"YAML 1.2","yaml":{"schema":1,"id":"Workflow ID","name":"Display name","enabled":"optional boolean, default true","columns":[{"id":"column ID (e.g. backlog, design, develop, qa, review)","name":"Column display name"}],"on":{"rule_id":{"event":"manual | card.created | card.column_entered | card.column_left | ...","match":"EventSelector without types; Project scope implicit","start_at":"step ID","enabled":"optional boolean"}},"steps":[{"id":"stable step ID","name":"optional display name","uses":"agent | card.move | approval | deliver | condition | notify | complete","with":"arguments as described in step_types below","next":"optional explicit successor; defaults to next item","on_changes_requested":"optional approval/deliver rework target"}]},"source_operations":{"workflow.sources":{"project_id":"Project ID"},"workflow.check / workflow.apply":{"project_id":"Project ID","revision":"collection revision from workflow.sources","sources":[{"filename":"name.yaml","source":"original YAML"}]},"workflow.prompt":{"project_id":"Project ID"},"configuration.open":{"project_id":"Project ID","profile_id":"Profile ID","prompt":"reviewed Prompt","fresh":"explicit new conversation"}},"definition":{"id":"string","name":"string","columns":[{"id":"string","name":"string"}],"entries":{"entry":"step_id"},"steps":{"step_id":{"type":"agent|move|approval|deliver|condition|notify|complete"}},"max_steps":"1..1000"},"step_types":{"agent":{"profile":"optional Profile ID","instruction":"string","next":"optional step ID"},"move":{"column":"column ID","next":"optional step ID"},"approval":{"title":"string","next":"optional step ID","rework":"optional step ID"},"deliver":{"target_branch":"optional target branch","sync_upstream":"optional boolean, default true (false merges locally without pushing upstream)","next":"optional step ID","rework":"optional step ID"},"condition":{"pointer":"JSON pointer into inputs/outputs/card (e.g. /card/source_trust)","equals":"JSON value (e.g. external_pr | collaborator | local)","then":"step ID","otherwise":"step ID"},"notify":{"message":"string","next":"optional step ID"},"complete":{}},"operations":["workflow.list","workflow.get","workflow.validate","workflow.publish","workflow.bind","workflow.start","workflow.runs","workflow.run","workflow.decide","workflow.stop","card.hold_auto_merge","event.list","subscription.save","subscription.list","subscription.deliveries","subscription.ack"],"parameters":{"workflow.validate":{"project_id":"Project ID","definition":"WorkflowDefinition"},"workflow.publish":{"project_id":"Project ID","definition":"WorkflowDefinition","revision":"current definition revision; 0 creates","request_key":"optional idempotency key"},"workflow.bind":{"project_id":"Project ID","workflow_id":"published ID","revision":"current binding revision; 0 creates"},"workflow.start":{"card_id":"Card ID","workflow_id":"published ID","entry":"entry name","request_key":"optional idempotency key"},"workflow.get":{"workflow_id":"published ID","revision":"optional historical revision"},"workflow.run":{"run_id":"WorkflowRun ID"},"workflow.runs":{"card_id":"optional Card ID"},"workflow.decide":{"run_id":"WorkflowRun ID","visit":"current run.visit","revision":"current Card.revision","decision":"approve | rework","feedback":"optional rework instructions","request_key":"optional idempotency key"},"workflow.stop":{"run_id":"WorkflowRun ID"},"card.hold_auto_merge":{"card_id":"Card ID","revision":"current Card.revision","hold":"optional boolean; toggles when omitted"},"subscription.save":{"subscription_id":"omit for creation","revision":"current revision; 0 creates","enabled":"boolean; default false","selector":"EventSelector","target":"SubscriptionTarget"},"subscription.deliveries":{"subscription_id":"optional subscription ID"},"subscription.ack":{"delivery_id":"consumer receipt ID"},"subscription.retry":{"delivery_id":"failed receipt ID"},"event.list":{"after":"journal offset; default 0","limit":"1..500","selector":"optional EventSelector"}},"selector_fields":["project_id","card_id","repository","subject","types","column","changed_fields","actor","origin","source_kind","reason","reply_to","include_related","include_initial"],"subscription_targets":[{"type":"consumer"},{"type":"workflow","workflow_id":"published ID","entry":"entry name","card_id":"optional fixed Card ID"}],"workspace":{"create_worktree":"boolean","sync_base":"boolean","base_branch":"optional branch","remote":"remote name"},"activation":"YAML apply atomically publishes sources, optional Board columns, and named triggers. Future receipts/runs use the new revision. Existing runs and captured receipts retain theirs; independent subscriptions and manual starts remain explicit."}),
            ),
            "workflow.list" => Ok(json!(
                self.store
                    .lock()
                    .unwrap()
                    .db()
                    .list::<WorkflowRevision>()?
                    .into_iter()
                    .filter(
                        |w| p["project_id"].as_str().is_none_or(|v| v == w.project_id)
                            && p["workflow_id"]
                                .as_str()
                                .is_none_or(|id| id == w.workflow_id)
                    )
                    .collect::<Vec<_>>()
            )),
            "workflow.get" => {
                let store = self.store.lock().unwrap();
                let w = if let Some(rev) = p["revision"].as_u64() {
                    store
                        .db()
                        .get::<WorkflowRevision>(&format!("{}:{rev}", strp(&p, "workflow_id")?))?
                } else {
                    latest(store.db(), strp(&p, "workflow_id")?)?
                };
                Ok(json!(w))
            }
            "workflow.runs" => Ok(json!(
                self.store
                    .lock()
                    .unwrap()
                    .db()
                    .list::<WorkflowRun>()?
                    .into_iter()
                    .filter(|r| p["card_id"].as_str().is_none_or(|v| v == r.card_id))
                    .collect::<Vec<_>>()
            )),
            "workflow.run" => Ok(json!(
                self.store
                    .lock()
                    .unwrap()
                    .db()
                    .get::<WorkflowRun>(strp(&p, "run_id")?)?
            )),
            "workflow.validate" | "workflow.publish" => {
                let flow: WorkflowDefinition = serde_json::from_value(p["definition"].clone())?;
                let project: Project = self
                    .store
                    .lock()
                    .unwrap()
                    .db()
                    .get(strp(&p, "project_id")?)?;
                canban_workflow_engine::validate_definition(
                    &flow,
                    &project
                        .workflow
                        .stages
                        .iter()
                        .map(|s| s.id.clone())
                        .collect::<Vec<_>>(),
                    &self
                        .config()
                        .profiles
                        .iter()
                        .map(|a| a.id.clone())
                        .collect::<Vec<_>>(),
                )?;
                if method == "workflow.validate" {
                    return Ok(json!({"valid":true}));
                }
                self.mutate(method, &p, |db| {
                    let old = latest(db, &flow.id).ok();
                    ensure!(
                        old.as_ref().is_none_or(|w| w.project_id == project.id),
                        "Workflow belongs to another Project"
                    );
                    let revision = old.as_ref().map_or(0, |w| w.revision);
                    ensure!(
                        p["revision"].as_u64() == Some(revision),
                        "revision_conflict: supply current workflow revision (0 for creation)"
                    );
                    let w = super::authoring::import(
                        &self.home,
                        db,
                        &self.config(),
                        &project,
                        &flow,
                        revision,
                    )?;
                    Ok(json!(w))
                })
            }
            "workflow.bind" => self.mutate(method, &p, |db| {
                let project: Project = db.get(strp(&p, "project_id")?)?;
                let flow = latest(db, strp(&p, "workflow_id")?)?;
                ensure!(flow.project_id == project.id, "Wrong Project");
                let old = db.get::<WorkflowBinding>(&project.id).ok();
                ensure!(
                    p["revision"].as_u64() == Some(old.as_ref().map_or(0, |v| v.revision)),
                    "revision_conflict"
                );
                super::authoring::bind(
                    &self.home,
                    db,
                    &self.config(),
                    &project,
                    &flow.workflow_id,
                )?;
                let b = WorkflowBinding {
                    id: project.id.clone(),
                    project_id: project.id,
                    workflow_id: flow.workflow_id,
                    revision: old.map_or(1, |b| b.revision + 1),
                };
                db.save(&b)?;
                Ok(json!(b))
            }),
            "workflow.start" => self.mutate(method, &p, |db| {
                let mut card: Card = db.get(strp(&p, "card_id")?)?;
                let w = latest(db, strp(&p, "workflow_id")?)?;
                ensure!(card.project_id == w.project_id, "Wrong Project");
                if let Some(source) = db
                    .list::<WorkflowSource>()?
                    .iter()
                    .find(|s| s.workflow_id == w.workflow_id)
                {
                    ensure!(
                        source.enabled
                            && source
                                .rules
                                .get(strp(&p, "entry")?)
                                .is_some_and(|r| r.enabled && r.event == "manual"),
                        "This Workflow entry is disabled or is not a manual trigger"
                    );
                }
                Ok(json!(start(
                    db,
                    &self.config(),
                    &mut card,
                    w.definition,
                    w.revision,
                    strp(&p, "entry")?,
                    None,
                    false
                )?))
            }),
            "workflow.stop" => self.mutate(method, &p, |db| {
                let run: WorkflowRun = db.get(strp(&p, "run_id")?)?;
                ensure!(
                    p["execution"].is_null(),
                    "Stopping a workflow requires a user invocation"
                );
                ensure!(run.active(), "Workflow already ended");
                stop_card(db, &run.card_id, "cancelled")?;
                let mut card: Card = db.get(&run.card_id)?;
                card.human_gate = None;
                card.attention = None;
                card.revision += 1;
                db.save(&card)?;
                Ok(json!({"stopped":true,"live_agents_terminated":false}))
            }),
            "workflow.decide" => {
                ensure!(
                    p["execution"].is_null(),
                    "Human decisions require a user invocation"
                );
                self.mutate(method, &p, |db| {
                    let mut run: WorkflowRun = db.get(strp(&p, "run_id")?)?;
                    ensure!(
                        p["visit"].as_u64() == Some(run.visit as u64),
                        "Decision version changed"
                    );
                    let mut card: Card = db.get(&run.card_id)?;
                    revise(&card, &p)?;
                    let decision = strp(&p, "decision")?;
                    ensure!(matches!(decision, "approve" | "rework"), "Invalid decision");
                    approve(
                        db,
                        &self.config(),
                        &mut card,
                        &mut run,
                        decision == "rework",
                        p["feedback"].as_str(),
                    )?;
                    Ok(json!(run))
                })
            }
            _ => bail!("Unknown workflow operation {method}"),
        }
    }
    pub(super) fn continue_card(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Human review must be approved by the user, not an Agent invocation"
        );
        let config = self.config();
        self.mutate("card.continue", &p, |db| {
            let mut card: Card = db.get(strp(&p, "card_id")?)?;
            revise(&card, &p)?;
            ensure!(card.active(), "Restore the Card before continuing");
            let approval_step = db
                .list::<WorkflowRun>()?
                .iter()
                .any(|r| r.card_id == card.id && r.active() && r.state == "waiting_approval");
            let gate = card
                .human_gate
                .clone()
                .context("Card is not waiting for human review")?;
            if let Some(run) = db
                .list::<WorkflowRun>()?
                .into_iter()
                .find(|r| r.card_id == card.id && r.active())
            {
                let mut run = run;
                if run.state == "waiting_approval" {
                    approve(db, &config, &mut card, &mut run, false, None)?;
                } else if let Some(id) = gate.request_id {
                    let mut req: Request = db.get(&id)?;
                    if req.state == "completed" {
                        card.human_gate = None;
                        card.attention = None;
                        advance(db, &config, &mut card, &req)?;
                    } else {
                        ensure!(
                            !db.list::<Attempt>()?.iter().any(|a| a.request_id == req.id
                                && a.terminal.is_some()
                                && a.observed.as_ref().is_none_or(|v| v["state"] != "finished")),
                            "Answer the live Agent first, or end it before retrying"
                        );
                        ensure!(
                            matches!(req.state.as_str(), "blocked" | "failed" | "needs_report"),
                            "Inspect the uncertain execution before retrying"
                        );
                        req.state = "queued".into();
                        db.save(&req)?;
                        run.state = "waiting_agent".into();
                        db.save(&run)?;
                        card.human_gate = None;
                        card.attention = None;
                    }
                } else {
                    stop_card(db, &card.id, "superseded")?;
                    card.human_gate = None;
                    card.attention = None;
                    let stage = card.stage.clone();
                    if let Some(source) = db.list::<WorkflowSource>()?.into_iter().find(|s| {
                        s.project_id == card.project_id && s.enabled && s.rules.contains_key(&stage)
                    }) && let Ok(w) = published(db, &source.workflow_id)
                    {
                        start(
                            db,
                            &config,
                            &mut card,
                            w.definition,
                            w.revision,
                            &stage,
                            None,
                            false,
                        )?;
                    }
                }
            } else {
                card.human_gate = None;
                card.attention = None;
                if let Some(id) = gate.request_id {
                    let mut req: Request = db.get(&id)?;
                    ensure!(
                        req.state != "unknown",
                        "Inspect uncertain launch before retrying"
                    );
                    if matches!(req.state.as_str(), "blocked" | "failed" | "needs_report") {
                        ensure!(
                            !db.list::<Attempt>()?.iter().any(|a| a.request_id == req.id
                                && a.terminal.is_some()
                                && a.observed.as_ref().is_none_or(|v| v["state"] != "finished")),
                            "Answer the live Agent first, or end it before retrying"
                        );
                        req.state = "queued".into();
                        db.save(&req)?;
                    }
                }
            }
            card.revision += 1;
            if !approval_step {
                log(
                    db,
                    &card,
                    "workflow.approved",
                    "user",
                    "Human reviewed and continued the workflow",
                    None,
                    json!({}),
                )?;
            }
            db.save(&card)?;
            Ok(json!(card))
        })
    }
}
