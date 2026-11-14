//! AppServer workflow operations; interpretation lives in canban-workflow-engine.
use super::*;
use canban_workflow_engine::runtime as engine;
fn latest(db: Db<'_>, id: &str) -> Result<WorkflowRevision> {
    engine::latest(db, id)
}
pub(super) fn stop_card(db: Db<'_>, config: &Config, card: &str, state: &str) -> Result<()> {
    crate::scripts::kill_card_scripts_db(db, config, card);
    engine::stop_card(db, card, state)
}
pub(super) fn initialize(db: Db<'_>, config: &Config) -> Result<()> {
    engine::initialize(db, config)
}
pub(super) fn reported(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    terminal: Option<&TerminalEntry>,
    report: &Report,
) -> Result<()> {
    engine::reported(db, config, card, terminal, report, enqueue)
}
pub(super) fn advance(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    terminal: Option<&TerminalEntry>,
) -> Result<()> {
    engine::advance(db, config, card, terminal, enqueue)
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
pub(super) fn adopt_stage(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    terminal: Option<&mut TerminalEntry>,
    stage: &str,
    event: Option<String>,
) -> Result<Option<WorkflowRun>> {
    let mut sources: Vec<_> = db
        .list::<WorkflowSource>()?
        .into_iter()
        .filter(|s| s.project_id == card.project_id && s.enabled)
        .collect();
    sources.sort_by(|a, b| {
        (a.filename != "development.yaml")
            .cmp(&(b.filename != "development.yaml"))
            .then_with(|| a.filename.cmp(&b.filename))
    });
    for source in sources {
        let matched_entry = source
            .rules
            .iter()
            .find(|(_, r)| {
                r.enabled
                    && r.event == "card.column_entered"
                    && r.selector.column.as_deref() == Some(stage)
                    && r.selector.origin.as_deref().is_none_or(|o| o == "local")
            })
            .map(|(k, _)| k.clone())
            .or_else(|| {
                source
                    .rules
                    .get(stage)
                    .filter(|r| {
                        r.enabled
                            && r.event == "card.column_entered"
                            && r.selector.column.as_deref().is_none_or(|c| c == stage)
                    })
                    .map(|_| stage.to_owned())
            });
        if let Some(entry) = matched_entry {
            let w = published(db, &source.workflow_id)?;
            let run = engine::start_adopted(
                db,
                config,
                card,
                w.definition,
                w.revision,
                &entry,
                event,
                w.workflow_id.starts_with("delivery:"),
                terminal,
                enqueue,
            )?;
            return Ok(Some(run));
        }
    }
    Ok(None)
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
    /// Releases workflow runs waiting at `admit` gates when capacity allows.
    /// Runs before scheduler dispatch so admitted Agent work is queued in Board order.
    pub(crate) fn pump_admissions(&self) -> Result<()> {
        let config = self.config();
        let admitted = self.store.lock().unwrap().write(|db| {
            let snapshot = db.snapshot()?;
            let ids = canban_scheduler::admissions(&snapshot, config.scheduler.max_agents);
            let mut cards = Vec::new();
            for id in ids {
                let mut run: WorkflowRun = db.get(&id)?;
                if run.state != "waiting_capacity" {
                    continue;
                }
                let mut card: Card = db.get(&run.card_id)?;
                engine::admit(db, &config, &mut card, &mut run, enqueue)?;
                cards.push(card.id);
            }
            Ok(cards)
        })?;
        for card in admitted {
            self.emit("application", Some(card));
        }
        Ok(())
    }
    pub(super) fn workflow_call(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "workflow.schema" => Ok(
                json!({"schema_version":canban_workflow_engine::migrations::CURRENT,"supported_schemas":[canban_workflow_engine::migrations::BASELINE,canban_workflow_engine::migrations::CURRENT],"authoring_format":"YAML 1.2","yaml":{"schema":canban_workflow_engine::migrations::CURRENT,"id":"Workflow ID","name":"Display name","enabled":"optional boolean, default true","env":"optional { KEY: VALUE } map applied to every `script` and `agent` step (overrides host env and built-in `CANBAN_*` context, overridden by `$CANBAN_ENV` and step `env`)","columns":[{"id":"column ID (e.g. backlog, design, develop, qa, review)","name":"Column display name"}],"on":{"rule_id":{"event":"manual | card.created | card.column_entered | card.column_left | ...","match":"EventSelector without types; Project scope implicit","start_at":"step ID","enabled":"optional boolean"}},"steps":[{"id":"stable step ID","name":"optional display name","uses":"agent | script | card.move | approval | deliver | admit | condition | notify | complete (optional when top-level `run` is given)","run":"shorthand shell command (equivalent to `uses: script` + `with.run`; cannot be combined with non-script `uses` or `with.run`)","shell":"optional shorthand shell for `script` steps (default bash -eo pipefail; supports sh, zsh, python3, or custom command)","working_directory":"optional relative path within Card worktree (or Project directory)","timeout_minutes":"optional integer script timeout (minutes)","timeout_seconds":"optional integer script timeout (seconds)","continue_on_error":"optional boolean for `script` steps, default false","on_failure":"optional target step ID jumped to ONCE on first non-zero script failure; second failure pauses in human review","env":"optional { KEY: VALUE } map applied on this `script` or `agent` step (highest precedence)","with":"arguments as described in step_types below","next":"optional explicit successor; defaults to next item; card.move may use $entry (target column entry rule start, or end of run)","on_changes_requested":"optional approval/deliver rework target","stage":"optional Board column this step belongs to (Studio Stage); inferred when absent","enabled":"optional boolean, default true; false skips the step at compile time and references continue at its successor (a disabled condition follows otherwise)"}]},"source_operations":{"workflow.sources":{"project_id":"Project ID"},"workflow.check / workflow.apply":{"project_id":"Project ID","revision":"collection revision from workflow.sources","sources":[{"filename":"name.yaml","source":"original YAML"}]},"workflow.prompt":{"project_id":"Project ID"},"configuration.open":{"project_id":"Project ID","profile_id":"optional Profile ID; defaults to config.default_agent","prompt":"optional reviewed Prompt","fresh":"explicit new conversation","auto_fresh":"start fresh when an ended session lacks exact resume"}},"step_types":{"agent":{"profile":"optional Profile ID","instruction":"optional string; uses built-in role/step default prompt when omitted","env":"optional { KEY: VALUE } map layered after workflow env and $CANBAN_ENV","next":"optional step ID"},"script":{"run":"command or multi-line script body (either top-level `run:` shorthand or `uses: script` + `with.run`)","shell":"optional interpreter (default bash with -eo pipefail, fallback sh -e; supports zsh, python3, etc.)","working_directory":"optional relative path inside the Card worktree (or Project dir)","timeout_minutes":"optional integer timeout in minutes (default 60)","timeout_seconds":"optional integer timeout in seconds","continue_on_error":"optional boolean, default false; proceed even on non-zero exit","on_failure":"optional target step ID jumped to ONCE on first failure (carries exit code and log tail in rework feedback); second consecutive failure pauses for human action","env":"optional { KEY: VALUE } map layered on top of workflow env and $CANBAN_ENV","outputs":"writes KEY=VALUE pairs or multiline delimiters to $CANBAN_OUTPUT (stored at outputs.<step_id>.<key>) and $CANBAN_ENV (shared with subsequent script & agent steps)","next":"optional step ID"},"card.move":{"column":"column ID, or $next for the column after the step's stage","next":"optional step ID or $entry"},"approval":{"title":"string","next":"optional step ID","rework":"optional step ID"},"deliver":{"profile":"optional Profile ID for automatic merge-conflict / batch-failure resolution (defaults to @merger when enabled)","target_branch":"optional target branch","strategy":"optional: rebase (default) | squash | merge","sync_upstream":"optional boolean, default true (false merges locally without pushing upstream)","batch_size":"optional integer 1..50, default 5; combines up to N queued Cards into one integration + test + push batch","test_command":"optional non-empty shell command executed once per integrated batch before fast-forward and push","max_conflict_attempts":"optional integer, default 3; merge conflicts or failed batch checks automatically route to @merger (or deliver.with.profile) up to this many times per run, then pause for a human; 0 disables","next":"optional step ID","rework":"optional step ID (human Request changes target)"},"admit":{"agent_slot":"optional boolean, default true; wait for a free automatic Agent slot (scheduler.max_agents)","max_ready":"optional integer 1..100; at most this many Cards that passed this gate may remain in its column","next":"optional step ID","note":"waiting runs are admitted in Board order within their column"},"condition":{"pointer":"JSON pointer into inputs/outputs/card (e.g. /card/source_trust or /outputs/<step_id>/<key>)","equals":"JSON value (e.g. external_pr | collaborator | local)","then":"step ID","otherwise":"step ID"},"notify":{"message":"string","next":"optional step ID"},"complete":{}},"operations":["workflow.sources","workflow.check","workflow.apply","workflow.prompt","workflow.list","workflow.get","workflow.start","workflow.runs","workflow.run","workflow.decide","workflow.deliver","workflow.stop","card.hold_auto_merge","event.list","subscription.save","subscription.list","subscription.deliveries","subscription.ack"],"parameters":{"workflow.sources":{"project_id":"Project ID"},"workflow.check":{"project_id":"Project ID","revision":"collection revision from workflow.sources","sources":[{"filename":"name.yaml","source":"original YAML"}]},"workflow.apply":{"project_id":"Project ID","revision":"collection revision from workflow.sources","sources":[{"filename":"name.yaml","source":"original YAML"}],"request_key":"optional idempotency key"},"workflow.start":{"card_id":"Card ID","workflow_id":"published ID","entry":"entry name","request_key":"optional idempotency key"},"workflow.get":{"workflow_id":"published ID","revision":"optional historical revision"},"workflow.run":{"run_id":"WorkflowRun ID"},"workflow.runs":{"card_id":"optional Card ID"},"workflow.decide":{"run_id":"WorkflowRun ID","visit":"current run.visit","revision":"current Card.revision","decision":"approve | rework","feedback":"optional rework instructions","request_key":"optional idempotency key"},"workflow.deliver":{"card_id":"Card ID","revision":"current Card.revision","request_key":"optional idempotency key"},"workflow.stop":{"run_id":"WorkflowRun ID"},"card.hold_auto_merge":{"card_id":"Card ID","revision":"current Card.revision","hold":"optional boolean; toggles when omitted"},"subscription.save":{"subscription_id":"omit for creation","revision":"current revision; 0 creates","enabled":"boolean; default false","selector":"EventSelector","target":"SubscriptionTarget"},"subscription.deliveries":{"subscription_id":"optional subscription ID"},"subscription.ack":{"delivery_id":"consumer receipt ID"},"subscription.retry":{"delivery_id":"failed receipt ID"},"event.list":{"after":"journal offset; default 0","limit":"1..500","selector":"optional EventSelector"}},"selector_fields":["project_id","card_id","repository","subject","types","column","changed_fields","actor","origin","source_kind","reason","reply_to","include_related","include_initial"],"subscription_targets":[{"type":"consumer"},{"type":"workflow","workflow_id":"published ID","entry":"entry name","card_id":"optional fixed Card ID"}],"workspace":{"create_worktree":"boolean","sync_base":"boolean","base_branch":"optional branch","remote":"remote name"},"activation":"YAML apply atomically publishes sources, optional Board columns, and named triggers. Older schema versions upgrade automatically via linear workflow migrations. Future receipts/runs use the new revision; existing runs and captured receipts retain theirs."}),
            ),
            "workflow.list" => {
                let store = self.store.lock().unwrap();
                let db = store.db();
                let project_id = p["project_id"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|raw| super::resolve_project_id(db, raw))
                    .transpose()?;
                Ok(json!(
                    db.list::<WorkflowRevision>()?
                        .into_iter()
                        .filter(|w| {
                            project_id.as_deref().is_none_or(|id| id == w.project_id)
                                && p["workflow_id"]
                                    .as_str()
                                    .is_none_or(|id| id == w.workflow_id)
                        })
                        .collect::<Vec<_>>()
                ))
            }
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
                stop_card(db, &self.config(), &run.card_id, "cancelled")?;
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
                let res = self.mutate(method, &p, |db| {
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
                })?;
                if matches!(
                    res["state"].as_str(),
                    Some("waiting_delivery" | "waiting_script")
                ) {
                    self.request_deliveries();
                    self.request_scripts();
                    if let Some(id) = res["id"].as_str() {
                        let store = self.store.lock().unwrap();
                        if let Ok(updated) = store.db().get::<WorkflowRun>(id) {
                            return Ok(json!(updated));
                        }
                    }
                }
                Ok(res)
            }
            "workflow.deliver" => {
                ensure!(
                    p["execution"].is_null(),
                    "Triggering auto-merge requires a user invocation"
                );
                let res = self.mutate(method, &p, |db| {
                    let mut card: Card = db.get(strp(&p, "card_id")?)?;
                    revise(&card, &p)?;
                    ensure!(card.active(), "Restore the Card before merging");
                    let config = self.config();
                    if let Some(mut run) = db
                        .list::<WorkflowRun>()?
                        .into_iter()
                        .find(|r| r.card_id == card.id && r.active())
                    {
                        let is_deliver = run.step.as_ref().is_some_and(|k| {
                            matches!(
                                run.definition.steps.get(k),
                                Some(WorkflowStep::Deliver { .. })
                            )
                        });
                        if is_deliver && run.state == "waiting_approval" {
                            approve(db, &config, &mut card, &mut run, false, None)?;
                            return Ok(json!(run));
                        }
                        if is_deliver && run.state == "waiting_delivery" {
                            if card.hold_auto_merge || run.inputs["queued_at"].as_u64().is_none() {
                                run.inputs["queued_at"] =
                                    json!(engine::next_merge_queued_at(db, &card.project_id)?);
                                run.updated_at = now();
                                db.save(&run)?;
                            }
                            card.hold_auto_merge = false;
                            card.human_gate = None;
                            card.attention = None;
                            card.revision += 1;
                            db.save(&card)?;
                            return Ok(json!(run));
                        }
                        if is_deliver
                            && matches!(run.state.as_str(), "failed" | "blocked" | "waiting_agent")
                        {
                            engine::requeue_delivery(db, &mut card, &mut run)?;
                            return Ok(json!(run));
                        }
                        bail!("Card workflow is currently active at another step");
                    }
                    // No active workflow run: find the latest enabled workflow on this Project
                    // containing a `Deliver` step and start directly from it.
                    let enabled_ids: std::collections::BTreeSet<String> = db
                        .list::<WorkflowSource>()?
                        .into_iter()
                        .filter(|s| s.project_id == card.project_id && s.enabled)
                        .map(|s| s.workflow_id)
                        .collect();
                    let mut latest_by_wf = std::collections::BTreeMap::new();
                    for w in db
                        .list::<WorkflowRevision>()?
                        .into_iter()
                        .filter(|w| w.project_id == card.project_id)
                    {
                        if enabled_ids.is_empty() || enabled_ids.contains(&w.workflow_id) {
                            latest_by_wf.insert(w.workflow_id.clone(), w);
                        }
                    }
                    let (w, step_id) = latest_by_wf
                        .into_values()
                        .find_map(|w| {
                            let stage_entry = w
                                .definition
                                .entries
                                .get(&card.stage)
                                .filter(|k| {
                                    matches!(
                                        w.definition.steps.get(*k),
                                        Some(WorkflowStep::Deliver { .. })
                                    )
                                })
                                .cloned();
                            let any_deliver = stage_entry.or_else(|| {
                                w.definition
                                    .steps
                                    .iter()
                                    .find(|(_, st)| matches!(st, WorkflowStep::Deliver { .. }))
                                    .map(|(k, _)| k.clone())
                            });
                            any_deliver.map(|k| (w, k))
                        })
                        .context(
                            "No Queue merge (deliver) step configured in this Project's workflow",
                        )?;
                    card.hold_auto_merge = false;
                    card.human_gate = None;
                    card.attention = None;
                    card.revision += 1;
                    db.save(&card)?;
                    let legacy = w.workflow_id.starts_with("delivery:");
                    let run = start(
                        db,
                        &config,
                        &mut card,
                        w.definition,
                        w.revision,
                        &step_id,
                        None,
                        legacy,
                    )?;
                    Ok(json!(run))
                })?;
                self.request_deliveries();
                self.request_scripts();
                if let Some(id) = res["id"].as_str() {
                    let store = self.store.lock().unwrap();
                    if let Ok(updated) = store.db().get::<WorkflowRun>(id) {
                        return Ok(json!(updated));
                    }
                }
                Ok(res)
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
        let res = self.mutate("card.continue", &p, |db| {
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
                } else if let Some(tid) = gate.terminal_id {
                    let mut prev_entry: TerminalEntry = db.get(&tid)?;
                    let prev = prev_entry
                        .agent
                        .as_ref()
                        .context("Terminal has no Agent assignment")?;
                    if prev.state == "completed" {
                        card.human_gate = None;
                        card.attention = None;
                        advance(db, &config, &mut card, Some(&prev_entry))?;
                    } else if run.step_is_deliver() {
                        engine::requeue_delivery(db, &mut card, &mut run)?;
                    } else {
                        ensure!(
                            prev.terminal.is_none()
                                || prev.observed.as_ref().is_some_and(|v| {
                                    matches!(v["state"].as_str(), Some("finished" | "missing"))
                                }),
                            "Answer the live Agent first, or end it before retrying"
                        );
                        ensure!(
                            matches!(
                                prev.state.as_str(),
                                "blocked" | "failed" | "needs_report" | "missing"
                            ),
                            "Inspect the uncertain execution before retrying"
                        );
                        crate::sessions::retry_agent(db, &card, &mut prev_entry)?;
                        run.terminal_id = Some(prev_entry.id);
                        run.state = "waiting_agent".into();
                        db.save(&run)?;
                        card.human_gate = None;
                        card.attention = None;
                    }
                } else {
                    stop_card(db, &config, &card.id, "superseded")?;
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
                if let Some(tid) = gate.terminal_id {
                    let mut prev_entry: TerminalEntry = db.get(&tid)?;
                    if let Some(prev) = &prev_entry.agent {
                        ensure!(
                            prev.state != "unknown",
                            "Inspect uncertain launch before retrying"
                        );
                        if matches!(
                            prev.state.as_str(),
                            "blocked" | "failed" | "needs_report" | "missing"
                        ) {
                            ensure!(
                                prev.terminal.is_none()
                                    || prev
                                        .observed
                                        .as_ref()
                                        .is_some_and(|v| v["state"] == "finished"),
                                "Answer the live Agent first, or end it before retrying"
                            );
                            crate::sessions::retry_agent(db, &card, &mut prev_entry)?;
                        }
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
        })?;
        self.request_deliveries();
        self.request_scripts();
        if let Some(id) = res["id"].as_str() {
            let store = self.store.lock().unwrap();
            if let Ok(updated) = store.db().get::<Card>(id) {
                return Ok(json!(updated));
            }
        }
        Ok(res)
    }
}
