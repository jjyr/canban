//! Workflow-configured scheduled tasks (`ScheduleState` & `ScheduledRun`),
//! supporting both Card-creating automation and Cardless Terminal Agent execution
//! with automatic post-report PTY reclamation.

use super::*;
use canban_scheduler::timers::{
    ScheduleAction, compute_next_run_at, evaluate_schedules, next_schedule_deadline,
    next_scheduled_run_at,
};
use canban_types::cron::CronTimeZone;
use std::collections::{BTreeMap, BTreeSet};

/// Settle delay before forcibly reclaiming a completed Cardless scheduled PTY
/// when Herdr has not yet emitted `"idle"` / `"done"` (`5s`).
pub(super) const SCHEDULE_RECLAIM_SETTLE_MS: u64 = 5_000;

/// Maximum wait after sending a graceful exit command (`ExitAction::Input`) before
/// falling back to `terminal.kill` so completed scheduled terminals never linger (`10s`).
pub(super) const SCHEDULE_GRACEFUL_EXIT_TIMEOUT_MS: u64 = 10_000;

/// Maximum number of retained `ScheduledRun` rows per schedule so history stays bounded.
const MAX_RUNS_PER_SCHEDULE: usize = 50;

pub(super) fn schedule_state_id(project_id: &str, workflow_id: &str, rule_id: &str) -> String {
    format!("schedule:{project_id}:{workflow_id}:{rule_id}")
}

fn profile_agent_engine(prof: &Profile) -> String {
    let cmd = prof.command_name();
    match cmd.as_str() {
        "codex" | "claude" => cmd,
        _ => prof.preset.clone().unwrap_or(cmd),
    }
}

fn expand_schedule_placeholders(
    template: &str,
    tz: CronTimeZone,
    scheduled_at: u64,
    run_number: u64,
    workflow_name: &str,
    rule_id: &str,
) -> String {
    let (date, dt) = tz.format_timestamp(scheduled_at);
    template
        .replace("{{date}}", &date)
        .replace("{{datetime}}", &dt)
        .replace("{{run_number}}", &run_number.to_string())
        .replace("{{workflow}}", workflow_name)
        .replace("{{rule}}", rule_id)
}

/// Synchronizes `ScheduleState` rows in SQLite with a scope's published `WorkflowSource` collection
/// (`scope_id` is either a `Project.id` or `GLOBAL_PROJECT_ID`).
pub(super) fn sync_scope_schedules(
    db: Db<'_>,
    scope_id: &str,
    project_opt: Option<&Project>,
    active_sources: &[WorkflowSource],
    now_ms: u64,
) -> Result<()> {
    let existing: Vec<ScheduleState> = db
        .list::<ScheduleState>()?
        .into_iter()
        .filter(|s| s.project_id == scope_id)
        .collect();
    let mut keep_ids = BTreeSet::new();

    for source in active_sources {
        let rev: WorkflowRevision = db.get(&source.revision_id)?;
        let workflow_name = if rev.definition.name.trim().is_empty() {
            source.filename.trim_end_matches(".yaml").to_owned()
        } else {
            rev.definition.name.clone()
        };
        for (rule_id, rule) in &source.rules {
            if !rule.is_schedule() {
                continue;
            }
            let Some(cron) = rule
                .cron
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let sid = schedule_state_id(scope_id, &source.workflow_id, rule_id);
            keep_ids.insert(sid.clone());

            let enabled = source.enabled && rule.enabled;
            let timezone = rule
                .timezone
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let target_column = rule.card.as_ref().and_then(|c| {
                let col = c
                    .column
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                col.or_else(|| {
                    project_opt.map(|p| {
                        p.workflow
                            .stages
                            .first()
                            .map(|s| s.id.clone())
                            .unwrap_or_else(|| "backlog".into())
                    })
                })
            });
            let profile_id = if rule.card.is_none() {
                canban_workflow_engine::source::schedule_agent_profile(&rev.definition, rule_id)
            } else {
                None
            };
            let starts_with_script = rule.card.is_none()
                && canban_workflow_engine::source::schedule_starts_with_script(
                    &rev.definition,
                    rule_id,
                );

            let prev = existing.iter().find(|s| s.id == sid);
            let schedule_changed = prev.is_none_or(|p| {
                p.cron != cron || p.timezone != timezone || (!p.enabled && enabled)
            });
            let next_run_at = if !enabled {
                None
            } else if schedule_changed || prev.and_then(|p| p.next_run_at).is_none() {
                compute_next_run_at(cron, timezone.as_deref(), now_ms)
            } else {
                prev.and_then(|p| p.next_run_at)
            };

            let state = ScheduleState {
                host_id: String::new(),
                id: sid,
                project_id: scope_id.to_owned(),
                filename: source.filename.clone(),
                workflow_id: source.workflow_id.clone(),
                workflow_name: workflow_name.clone(),
                rule_id: rule_id.clone(),
                cron: cron.to_owned(),
                timezone,
                overlap: rule.overlap,
                enabled,
                creates_card: rule.card.is_some(),
                starts_with_script,
                target_column,
                profile_id,
                next_run_at,
                last_scheduled_at: prev.and_then(|p| p.last_scheduled_at),
                last_run_id: prev.and_then(|p| p.last_run_id.clone()),
                queued_triggers: if enabled {
                    prev.map_or(0, |p| p.queued_triggers)
                } else {
                    0
                },
                run_count: prev.map_or(0, |p| p.run_count),
                updated_at: now_ms,
            };
            db.save(&state)?;
        }
    }

    for old in existing {
        if !keep_ids.contains(&old.id) {
            db.conn
                .execute("DELETE FROM schedule_states WHERE id=?1", [&old.id])?;
        }
    }
    Ok(())
}

/// Reconciles persisted schedule timers and in-flight runs on `AppServer::open` startup:
/// - Advances any past `next_run_at <= now` directly to the next future occurrence (never replays missed offline ticks).
/// - Marks any interrupted `"starting"` Cardless run without a terminal receipt as `"unknown"`.
pub(super) fn reconcile_startup(db: Db<'_>, now_ms: u64) -> Result<()> {
    for mut state in db.list::<ScheduleState>()? {
        if !state.enabled {
            if state.next_run_at.is_some() || state.queued_triggers > 0 {
                state.next_run_at = None;
                state.queued_triggers = 0;
                state.updated_at = now_ms;
                db.save(&state)?;
            }
            continue;
        }
        let needs_advance = state.next_run_at.is_none_or(|due| due <= now_ms);
        if needs_advance {
            state.next_run_at = compute_next_run_at(&state.cron, state.timezone.as_deref(), now_ms);
            state.updated_at = now_ms;
            db.save(&state)?;
        }
    }
    for mut run in db.list::<ScheduledRun>()? {
        if !run.creates_card && run.state == "starting" {
            let has_terminal = run
                .terminal_id
                .as_deref()
                .and_then(|tid| db.get::<TerminalEntry>(tid).ok())
                .and_then(|t| t.runtime)
                .and_then(|r| r.terminal)
                .is_some();
            if has_terminal {
                run.state = "running".into();
            } else {
                run.state = "unknown".into();
                run.error = Some(
                    "AppServer restarted while scheduled Agent launch was in flight; not automatically replayed"
                        .into(),
                );
            }
            db.save(&run)?;
        }
    }
    Ok(())
}

fn trim_schedule_history(db: Db<'_>, schedule_id: &str) -> Result<()> {
    let mut runs: Vec<ScheduledRun> = db
        .list::<ScheduledRun>()?
        .into_iter()
        .filter(|r| r.schedule_id == schedule_id && !r.active())
        .collect();
    if runs.len() <= MAX_RUNS_PER_SCHEDULE {
        return Ok(());
    }
    runs.sort_by(|a, b| {
        b.scheduled_at
            .cmp(&a.scheduled_at)
            .then_with(|| b.number.cmp(&a.number))
    });
    for stale in runs.into_iter().skip(MAX_RUNS_PER_SCHEDULE) {
        db.conn
            .execute("DELETE FROM scheduled_runs WHERE id=?1", [&stale.id])?;
    }
    Ok(())
}

/// Fires one scheduled task invocation (either cron tick or manual `schedule.trigger`),
/// creating either a scheduled Board Card + `WorkflowRun` or a queued Cardless `ScheduledRun`
/// (with lazy `TerminalEntry` creation when `start_step` is an `Agent` step).
#[allow(clippy::too_many_arguments)]
pub(super) fn fire_schedule(
    db: Db<'_>,
    home: &Path,
    config: &Config,
    schedule_id: &str,
    scheduled_at: u64,
    trigger_kind: &str,
    allow_disabled_manual: bool,
    now_ms: u64,
    id_gen: &dyn canban_types::IdGen,
) -> Result<(ScheduledRun, Option<String>)> {
    let mut state: ScheduleState = db
        .get(schedule_id)
        .with_context(|| format!("Schedule not found: {schedule_id}"))?;
    let is_global = is_global_project_id(&state.project_id);
    let project_opt: Option<Project> = if is_global {
        None
    } else {
        Some(db.get(&state.project_id)?)
    };
    let source = db
        .list::<WorkflowSource>()?
        .into_iter()
        .find(|s| s.project_id == state.project_id && s.workflow_id == state.workflow_id)
        .context("Scheduled Workflow source is no longer active")?;
    let rule = source
        .rules
        .get(&state.rule_id)
        .cloned()
        .context("Scheduled trigger rule no longer exists")?;
    ensure!(
        rule.is_schedule(),
        "Rule `{}` is no longer a scheduled trigger",
        state.rule_id
    );
    if !allow_disabled_manual {
        ensure!(
            source.enabled && rule.enabled && state.enabled,
            "Schedule is disabled"
        );
    }

    let rev = super::workflow::published(db, &state.workflow_id)?;
    let start_step = rev
        .definition
        .entries
        .get(&state.rule_id)
        .cloned()
        .or_else(|| {
            let s = rule.start_at.trim();
            (!s.is_empty()).then(|| s.to_owned())
        });
    let run_number = db.counter(&format!("schedule-run:{}", state.id))?;
    let short = id_gen.next_id();
    let run_id = format!("scheduled:{}:{short}", state.project_id);
    let tz = CronTimeZone::parse(rule.timezone.as_deref()).unwrap_or(CronTimeZone::Local);

    let (scheduled_run, created_card_id) = if let Some(card_cfg) = &rule.card {
        let project = project_opt
            .as_ref()
            .context("Global schedules cannot create Board Cards")?;
        let target_column = card_cfg
            .column
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                project
                    .workflow
                    .stages
                    .first()
                    .map(|s| s.id.clone())
                    .unwrap_or_else(|| "backlog".into())
            });
        ensure!(
            project
                .workflow
                .stages
                .iter()
                .any(|s| s.id == target_column),
            "Target column `{target_column}` does not exist in Project `{}`",
            project.name
        );
        let default_title = format!("{} ({{{{date}}}})", state.workflow_name);
        let raw_title = card_cfg
            .title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&default_title);
        let title = expand_schedule_placeholders(
            raw_title,
            tz,
            scheduled_at,
            run_number,
            &state.workflow_name,
            &state.rule_id,
        );
        let body = card_cfg
            .body
            .as_deref()
            .map(|b| {
                expand_schedule_placeholders(
                    b,
                    tz,
                    scheduled_at,
                    run_number,
                    &state.workflow_name,
                    &state.rule_id,
                )
            })
            .unwrap_or_default();

        let card_num = db.counter(&format!("cards:{}", project.id))?;
        let mut card = canban_kanban::card_at(project, card_num, title.trim(), &body, now_ms)?;
        card.id = id_gen.next_id();
        card.stage = target_column;
        card.created_by = format!("schedule:{}", state.workflow_name);
        card.schedule_source = Some(ScheduleSourceRef {
            schedule_id: state.id.clone(),
            workflow_id: state.workflow_id.clone(),
            workflow_name: state.workflow_name.clone(),
            filename: state.filename.clone(),
            rule_id: state.rule_id.clone(),
            cron: state.cron.clone(),
            run_id: run_id.clone(),
            run_number,
            scheduled_at,
        });
        canban_kanban::order::place(db, &mut card, None)?;
        db.save(&card)?;
        // `origin: "schedule"` keeps `card.column_entered` local subscriptions from double-starting a second WorkflowRun.
        log(
            db,
            &card,
            "card.created",
            "schedule",
            &format!(
                "Scheduled Card created by {} ({})",
                state.workflow_name, state.rule_id
            ),
            None,
            json!({
                "title": card.title,
                "stage": card.stage,
                "origin": "schedule",
                "reason": "scheduled",
                "schedule_id": state.id,
                "scheduled_run_id": run_id,
                "workflow_id": state.workflow_id,
                "rule_id": state.rule_id,
                "run_number": run_number,
                "trigger_kind": trigger_kind,
            }),
            now_ms,
        )?;

        let wf_run =
            super::workflow::start_scheduled(db, config, &mut card, rev, &state.rule_id, now_ms)?;
        let initial_state = if wf_run.active() {
            "running".to_owned()
        } else {
            wf_run.state.clone()
        };
        let completed_at = (!wf_run.active()).then_some(now_ms);
        let run = ScheduledRun {
            host_id: String::new(),
            id: run_id,
            schedule_id: state.id.clone(),
            project_id: project.id.clone(),
            workflow_id: state.workflow_id.clone(),
            workflow_name: state.workflow_name.clone(),
            filename: state.filename.clone(),
            rule_id: state.rule_id.clone(),
            cron: state.cron.clone(),
            number: run_number,
            trigger_kind: trigger_kind.to_owned(),
            creates_card: true,
            card_id: Some(card.id.clone()),
            card_number: Some(card.number),
            workflow_run_id: Some(wf_run.id),
            start_step: start_step.clone(),
            current_step: wf_run.step,
            profile_id: None,
            provider_session_id: None,
            terminal_id: wf_run.terminal_id,
            cwd: project.directory.clone(),
            state: initial_state,
            summary: Some(format!("Created Card #{} ({})", card.number, card.title)),
            report_path: None,
            output_path: None,
            error: wf_run.error,
            steps: Vec::new(),
            scheduled_at,
            started_at: Some(now_ms),
            completed_at,
            close_requested_at: None,
            updated_at: now_ms,
        };
        db.save(&run)?;
        (run, Some(card.id))
    } else {
        // Lazy admission: only pre-allocate a TerminalEntry and Agent profile when `start_step`
        // directly invokes an `Agent` step. Script-starting runs execute with 0 PTY/slot cost
        // and only create a TerminalEntry if execution transitions to an `Agent` step.
        let start_step_def = start_step
            .as_deref()
            .and_then(|sid| rev.definition.steps.get(sid));
        let starts_with_agent = matches!(start_step_def, Some(WorkflowStep::Agent { .. }));
        let step_prof = match start_step_def {
            Some(WorkflowStep::Agent { profile, .. }) => profile.clone(),
            _ => None,
        };
        let selected_profile = if starts_with_agent {
            Some(canban_scheduler::select_in_project(
                config,
                project_opt.as_ref(),
                step_prof.as_deref(),
            )?)
        } else {
            None
        };
        let terminal_id = if let Some(prof) = selected_profile.as_ref() {
            let entry = TerminalEntry {
                id: run_id.clone(),
                conversation_id: None,
                number: db.counter("terminal-number")?,
                name: Some(format!("{} #{}", state.workflow_name, run_number)),
                origin_project_id: Some(
                    project_opt
                        .as_ref()
                        .map_or_else(|| GLOBAL_PROJECT_ID.to_owned(), |p| p.id.clone()),
                ),
                origin_card_id: None,
                agent_engine: Some(profile_agent_engine(prof)),
                created_at: now_ms,
                runtime: Some(TerminalRuntime {
                    core_id: None,
                    terminal: None,
                    state: "queued".into(),
                    connection: "connecting".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                    observed_agent: None,
                    waiting: None,
                    command: None,
                }),
                agent: None,
            };
            db.save(&entry)?;
            Some(run_id.clone())
        } else {
            None
        };
        let run = ScheduledRun {
            host_id: String::new(),
            id: run_id,
            schedule_id: state.id.clone(),
            project_id: state.project_id.clone(),
            workflow_id: state.workflow_id.clone(),
            workflow_name: state.workflow_name.clone(),
            filename: state.filename.clone(),
            rule_id: state.rule_id.clone(),
            cron: state.cron.clone(),
            number: run_number,
            trigger_kind: trigger_kind.to_owned(),
            creates_card: false,
            card_id: None,
            card_number: None,
            workflow_run_id: None,
            start_step: start_step.clone(),
            current_step: start_step,
            profile_id: selected_profile.map(|p| p.id),
            provider_session_id: None,
            terminal_id,
            cwd: project_opt
                .as_ref()
                .map(|p| p.directory.clone())
                .unwrap_or_else(|| home.to_string_lossy().into_owned()),
            state: "queued".into(),
            summary: None,
            report_path: None,
            output_path: None,
            error: None,
            steps: Vec::new(),
            scheduled_at,
            started_at: None,
            completed_at: None,
            close_requested_at: None,
            updated_at: now_ms,
        };
        db.save(&run)?;
        (run, None)
    };

    state.run_count = state.run_count.saturating_add(1);
    state.last_scheduled_at = Some(scheduled_at);
    state.last_run_id = Some(scheduled_run.id.clone());
    state.updated_at = now_ms;
    db.save(&state)?;
    trim_schedule_history(db, &state.id)?;
    Ok((scheduled_run, created_card_id))
}

fn record_skipped_overlap(
    db: Db<'_>,
    schedule_id: &str,
    scheduled_at: u64,
    next_run_at: Option<u64>,
    now_ms: u64,
    id_gen: &dyn canban_types::IdGen,
) -> Result<()> {
    let mut state: ScheduleState = db.get(schedule_id)?;
    let run_number = db.counter(&format!("schedule-run:{}", state.id))?;
    let run_id = format!("scheduled:{}:{}", state.project_id, id_gen.next_id());
    let run = ScheduledRun {
        host_id: String::new(),
        id: run_id.clone(),
        schedule_id: state.id.clone(),
        project_id: state.project_id.clone(),
        workflow_id: state.workflow_id.clone(),
        workflow_name: state.workflow_name.clone(),
        filename: state.filename.clone(),
        rule_id: state.rule_id.clone(),
        cron: state.cron.clone(),
        number: run_number,
        trigger_kind: "cron".into(),
        creates_card: state.creates_card,
        card_id: None,
        card_number: None,
        workflow_run_id: None,
        start_step: None,
        current_step: None,
        profile_id: state.profile_id.clone(),
        provider_session_id: None,
        terminal_id: None,
        cwd: String::new(),
        state: "skipped".into(),
        summary: Some("Skipped due to active previous run (overlap: skip)".into()),
        report_path: None,
        output_path: None,
        error: None,
        steps: Vec::new(),
        scheduled_at,
        started_at: Some(now_ms),
        completed_at: Some(now_ms),
        close_requested_at: None,
        updated_at: now_ms,
    };
    db.save(&run)?;
    state.next_run_at = next_run_at;
    state.last_scheduled_at = Some(scheduled_at);
    state.last_run_id = Some(run_id);
    state.run_count = state.run_count.saturating_add(1);
    state.updated_at = now_ms;
    db.save(&state)?;
    trim_schedule_history(db, &state.id)?;
    Ok(())
}

impl AppServer {
    /// Returns the earliest wakeup deadline across enabled schedules and settling Cardless PTYs.
    pub(super) fn schedule_deadline(&self, snapshot: &Snapshot) -> Option<u64> {
        let now_ms = self.now_ms();
        let mut earliest = next_schedule_deadline(&snapshot.schedule_states);
        let mut consider = |at: u64| {
            earliest = Some(earliest.map_or(at, |cur| cur.min(at)));
        };
        for run in &snapshot.scheduled_runs {
            if run.creates_card {
                continue;
            }
            let Some(tid) = run.terminal_id.as_deref() else {
                continue;
            };
            let Some(rt) = snapshot
                .terminals
                .iter()
                .find(|t| t.id == tid)
                .and_then(|t| t.runtime.as_ref())
            else {
                continue;
            };
            if rt.state == "finished" {
                continue;
            }
            if matches!(run.state.as_str(), "completed" | "failed") {
                if let Some(req_at) = run.close_requested_at {
                    consider(req_at.saturating_add(SCHEDULE_GRACEFUL_EXIT_TIMEOUT_MS + 1));
                } else if !self.presence.is_protected(tid, None, now_ms)
                    && self.presence.last_viewed_at(tid, None, now_ms) == 0
                    && !self
                        .presence
                        .get_scheduled_track(tid)
                        .is_some_and(|t| t.interactive_followup)
                    && let Some(done_at) = run.completed_at
                {
                    consider(done_at.saturating_add(SCHEDULE_RECLAIM_SETTLE_MS + 1));
                }
            }
        }
        earliest
    }

    pub(super) fn cancel_scheduled_run_from_terminal_close(
        &self,
        db: Db<'_>,
        run: &mut ScheduledRun,
        item: &mut TerminalEntry,
    ) -> Result<()> {
        if run.state == "queued" {
            item.runtime = Some(TerminalRuntime {
                core_id: None,
                terminal: None,
                state: "finished".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
                command: None,
            });
            db.save(item)?;
        }
        run.state = "cancelled".into();
        run.error = Some("Scheduled Agent terminal closed by user".into());
        run.completed_at = Some(self.now_ms());
        db.save(run)?;
        Ok(())
    }

    /// Reconciles Card-mode scheduled runs, fires due cron timers, launches queued Cardless
    /// scheduled runs within `scheduler.max_agents`, and automatically reclaims finished Cardless PTYs.
    pub(super) fn pump_schedules(&self) -> Result<()> {
        self.reconcile_card_scheduled_runs()?;
        self.evaluate_and_fire_due_schedules()?;
        self.observe_and_reclaim_cardless_schedules()?;
        self.launch_queued_cardless_schedules()?;
        Ok(())
    }

    fn reconcile_card_scheduled_runs(&self) -> Result<()> {
        let snapshot = self.snapshot()?;
        let mut updates = Vec::new();
        for run in snapshot
            .scheduled_runs
            .iter()
            .filter(|r| r.creates_card && r.active())
        {
            let wf = run
                .workflow_run_id
                .as_deref()
                .and_then(|id| snapshot.workflow_runs.iter().find(|w| w.id == id));
            let card = run
                .card_id
                .as_deref()
                .and_then(|id| snapshot.cards.iter().find(|c| c.id == id));
            let (next_state, next_step, next_terminal, next_err, done) = match wf {
                Some(w) if w.active() => {
                    let st = if w.state == "blocked" || card.is_some_and(|c| c.human_gate.is_some())
                    {
                        "blocked"
                    } else {
                        "running"
                    };
                    (
                        st.to_owned(),
                        w.step.clone(),
                        w.terminal_id.clone(),
                        w.error.clone(),
                        false,
                    )
                }
                Some(w) => (
                    w.state.clone(),
                    w.step.clone(),
                    w.terminal_id.clone(),
                    w.error.clone(),
                    true,
                ),
                None => (
                    "cancelled".to_owned(),
                    run.current_step.clone(),
                    run.terminal_id.clone(),
                    None,
                    true,
                ),
            };
            if run.state != next_state
                || run.current_step != next_step
                || run.terminal_id != next_terminal
                || (done && run.completed_at.is_none())
            {
                updates.push((
                    run.id.clone(),
                    next_state,
                    next_step,
                    next_terminal,
                    next_err,
                    done,
                ));
            }
        }
        if !updates.is_empty() {
            let now_ms = self.now_ms();
            self.store.lock().unwrap().write(|db| {
                for (rid, st, step, tid, err, done) in updates {
                    if let Ok(mut cur) = db.get::<ScheduledRun>(&rid) {
                        cur.state = st;
                        cur.current_step = step;
                        if tid.is_some() {
                            cur.terminal_id = tid;
                        }
                        if err.is_some() {
                            cur.error = err;
                        }
                        if done && cur.completed_at.is_none() {
                            cur.completed_at = Some(now_ms);
                        }
                        db.save(&cur)?;
                    }
                }
                Ok(())
            })?;
            self.emit("schedules", None);
        }
        Ok(())
    }

    fn evaluate_and_fire_due_schedules(&self) -> Result<()> {
        let snapshot = self.snapshot()?;
        let now_ms = self.now_ms();
        let actions = evaluate_schedules(&snapshot, now_ms);
        if actions.is_empty() {
            return Ok(());
        }
        let config = self.config();
        let mut emitted_cards = Vec::new();
        let mut changed = false;
        self.store.lock().unwrap().write(|db| {
            let tick_ms = self.now_ms();
            for action in actions {
                match action {
                    ScheduleAction::AdvanceOnly {
                        schedule_id,
                        next_run_at,
                        clear_queued,
                    } => {
                        if let Ok(mut st) = db.get::<ScheduleState>(&schedule_id) {
                            st.next_run_at = next_run_at;
                            if clear_queued {
                                st.queued_triggers = 0;
                            }
                            st.updated_at = tick_ms;
                            db.save(&st)?;
                            changed = true;
                        }
                    }
                    ScheduleAction::SkipOverlap {
                        schedule_id,
                        scheduled_at,
                        next_run_at,
                    } => {
                        record_skipped_overlap(
                            db,
                            &schedule_id,
                            scheduled_at,
                            next_run_at,
                            tick_ms,
                            self.id_gen.as_ref(),
                        )?;
                        changed = true;
                    }
                    ScheduleAction::QueueOverlap {
                        schedule_id,
                        next_run_at,
                    } => {
                        if let Ok(mut st) = db.get::<ScheduleState>(&schedule_id) {
                            st.queued_triggers = 1;
                            st.next_run_at = next_run_at;
                            st.updated_at = tick_ms;
                            db.save(&st)?;
                            changed = true;
                        }
                    }
                    ScheduleAction::Trigger {
                        schedule_id,
                        scheduled_at,
                        next_run_at,
                        consume_queued,
                    } => {
                        if let Ok(mut st) = db.get::<ScheduleState>(&schedule_id) {
                            if consume_queued {
                                st.queued_triggers = 0;
                            } else {
                                st.next_run_at = next_run_at;
                            }
                            st.updated_at = tick_ms;
                            db.save(&st)?;
                        }
                        if let Ok((_, card_id)) = fire_schedule(
                            db,
                            &self.home,
                            &config,
                            &schedule_id,
                            scheduled_at,
                            "cron",
                            false,
                            tick_ms,
                            self.id_gen.as_ref(),
                        ) {
                            if let Some(cid) = card_id {
                                emitted_cards.push(cid);
                            }
                            changed = true;
                        }
                    }
                }
            }
            events::pump_at_with_prune_state(
                db,
                &config,
                self.now_ms(),
                Some(&self.last_cardless_prune_at),
            )?;
            Ok(())
        })?;
        if changed {
            let _ = self.pump_admissions();
            self.request_deliveries();
            self.request_scripts();
            for cid in emitted_cards {
                self.emit("application", Some(cid));
            }
            self.emit("schedules", None);
        }
        Ok(())
    }

    fn observe_and_reclaim_cardless_schedules(&self) -> Result<()> {
        let snapshot = self.snapshot()?;
        let now_ms = self.now_ms();
        let client = self.herdr_client();
        for run in snapshot.scheduled_runs.iter().filter(|r| !r.creates_card) {
            let Some(tid) = run.terminal_id.as_deref() else {
                continue;
            };
            let Some(entry) = snapshot.terminals.iter().find(|t| t.id == tid) else {
                continue;
            };
            let Some(rt) = entry.runtime.as_ref() else {
                continue;
            };

            if rt.state == "finished" {
                if matches!(run.state.as_str(), "running" | "working" | "blocked") {
                    let out_path = self.capture_scheduled_output(&run.id, entry).ok().flatten();
                    self.store.lock().unwrap().write(|db| {
                        let mut cur: ScheduledRun = db.get(&run.id)?;
                        if matches!(cur.state.as_str(), "running" | "working" | "blocked") {
                            let was_blocked = cur.state == "blocked";
                            cur.state = "failed".into();
                            cur.error = Some(if was_blocked {
                                "Scheduled Agent terminal exited while waiting for decision".into()
                            } else {
                                "Scheduled Agent exited without reporting an outcome".into()
                            });
                            cur.completed_at = Some(now_ms);
                            if let Some(step) = cur.steps.iter_mut().rev().find(|s| {
                                s.kind == "agent"
                                    && matches!(s.state.as_str(), "running" | "blocked")
                            }) {
                                step.state = "failed".into();
                            }
                            if out_path.is_some() {
                                cur.output_path = out_path;
                            }
                            db.save(&cur)?;
                        }
                        Ok(())
                    })?;
                    self.emit("schedules", None);
                }
                continue;
            }

            if matches!(run.state.as_str(), "running" | "working") {
                let next_state = match rt.agent_status.as_deref() {
                    Some("blocked") => "blocked",
                    Some("working") => "working",
                    _ => "running",
                };
                if run.state != next_state {
                    let wait_msg = rt.waiting.as_ref().map(|w| {
                        if w.message.trim().is_empty() {
                            w.title.clone()
                        } else {
                            format!("{} — {}", w.title, w.message)
                        }
                    });
                    self.store.lock().unwrap().write(|db| {
                        let mut cur: ScheduledRun = db.get(&run.id)?;
                        if matches!(cur.state.as_str(), "running" | "working") {
                            cur.state = next_state.into();
                            if next_state == "blocked" {
                                if cur.summary.is_none()
                                    && let Some(msg) = wait_msg
                                {
                                    cur.summary = Some(msg);
                                }
                                if let Some(step) = cur
                                    .steps
                                    .iter_mut()
                                    .rev()
                                    .find(|s| s.kind == "agent" && s.state == "running")
                                {
                                    step.state = "blocked".into();
                                }
                            }
                            db.save(&cur)?;
                        }
                        Ok(())
                    })?;
                    self.emit("schedules", None);
                }
                continue;
            }

            if run.state == "blocked" {
                // Keep the live PTY open while waiting on human decision (`⌘I Needs you`).
                // Only clear back to `working` if Herdr observes active work again before report.
                if rt.agent_status.as_deref() == Some("working") && run.completed_at.is_none() {
                    self.store.lock().unwrap().write(|db| {
                        let mut cur: ScheduledRun = db.get(&run.id)?;
                        if cur.state == "blocked" && cur.completed_at.is_none() {
                            cur.state = "working".into();
                            if let Some(step) = cur
                                .steps
                                .iter_mut()
                                .rev()
                                .find(|s| s.kind == "agent" && s.state == "blocked")
                            {
                                step.state = "running".into();
                            }
                            db.save(&cur)?;
                        }
                        Ok(())
                    })?;
                    self.emit("schedules", None);
                }
                continue;
            }

            if !matches!(run.state.as_str(), "completed" | "failed") {
                continue;
            }
            let Some(addr) = rt.terminal.as_deref() else {
                self.mark_scheduled_terminal_finished(tid);
                continue;
            };
            let protected = self.presence.is_protected(tid, None, now_ms);
            let last_viewed = self.presence.last_viewed_at(tid, None, now_ms);
            let track = self.presence.observe_scheduled_runtime(
                tid,
                rt.agent_status.as_deref(),
                rt.agent_turn,
                protected || last_viewed > 0,
                now_ms,
            );
            if matches!(
                rt.agent_status.as_deref(),
                Some("working" | "running" | "blocked")
            ) && (track.interactive_followup
                || track.settled_once
                || protected
                || last_viewed > 0)
            {
                if run.close_requested_at.is_some() || rt.state == "closing" {
                    let _ = self.store.lock().unwrap().write(|db| {
                        let mut cur: ScheduledRun = db.get(&run.id)?;
                        cur.close_requested_at = None;
                        db.save(&cur)?;
                        if let Ok(mut e) = db.get::<TerminalEntry>(tid)
                            && let Some(r) = &mut e.runtime
                            && r.state == "closing"
                        {
                            r.state = "running".into();
                            db.save(&e)?;
                        }
                        Ok(())
                    });
                    self.emit("schedules", None);
                    self.emit("terminals", None);
                }
                continue;
            }
            let core_matches = rt
                .core_id
                .as_deref()
                .is_some_and(|cid| self.verified_core_id().ok().as_deref() == Some(cid));

            if let Some(req_at) = run.close_requested_at {
                if now_ms.saturating_sub(req_at) >= SCHEDULE_GRACEFUL_EXIT_TIMEOUT_MS {
                    if run.output_path.is_none()
                        && let Ok(Some(rel)) = self.capture_scheduled_output(&run.id, entry)
                    {
                        let _ = self.store.lock().unwrap().write(|db| {
                            let mut cur: ScheduledRun = db.get(&run.id)?;
                            cur.output_path = Some(rel);
                            db.save(&cur)
                        });
                    }
                    if core_matches {
                        let _ = client.call("terminal.kill", json!({ "terminal": addr }));
                    }
                    self.mark_scheduled_terminal_finished(tid);
                    self.emit("schedules", None);
                }
                continue;
            }

            if protected {
                continue;
            }

            let idle_minutes = self.config().scheduler.idle_minutes;
            if track.interactive_followup {
                if idle_minutes == 0 {
                    continue;
                }
                let settled = matches!(rt.agent_status.as_deref(), Some("idle" | "done" | "error"))
                    || rt.agent_status.is_none();
                if !settled {
                    continue;
                }
                let anchor = track
                    .idle_since
                    .unwrap_or(0)
                    .max(last_viewed)
                    .max(run.completed_at.unwrap_or(0));
                if now_ms.saturating_sub(anchor) < idle_minutes.saturating_mul(60_000) {
                    continue;
                }
            } else if last_viewed > 0 {
                if idle_minutes == 0 {
                    continue;
                }
                let settled = matches!(rt.agent_status.as_deref(), Some("idle" | "done" | "error"))
                    || (rt.agent_status.is_none()
                        && run.completed_at.is_some_and(|at| {
                            now_ms.saturating_sub(at) >= SCHEDULE_RECLAIM_SETTLE_MS
                        }));
                if !settled {
                    continue;
                }
                let anchor = last_viewed.max(run.completed_at.unwrap_or(0));
                if now_ms.saturating_sub(anchor) < crate::lifecycle::COMPLETED_IDLE_RECLAIM_MS {
                    continue;
                }
            } else {
                let settled = matches!(rt.agent_status.as_deref(), Some("idle" | "done" | "error"))
                    || run
                        .completed_at
                        .is_some_and(|at| now_ms.saturating_sub(at) >= SCHEDULE_RECLAIM_SETTLE_MS);
                if !settled {
                    continue;
                }
            }

            if run.output_path.is_none()
                && let Ok(Some(rel)) = self.capture_scheduled_output(&run.id, entry)
            {
                let _ = self.store.lock().unwrap().write(|db| {
                    let mut cur: ScheduledRun = db.get(&run.id)?;
                    cur.output_path = Some(rel);
                    db.save(&cur)
                });
            }

            if !core_matches {
                self.mark_scheduled_terminal_finished(tid);
                continue;
            }

            let exit_input = run
                .profile_id
                .as_deref()
                .and_then(|pid| {
                    self.config()
                        .profiles
                        .into_iter()
                        .find(|p| p.id == pid)
                        .and_then(|p| p.exit)
                })
                .and_then(|ex| match ex {
                    ExitAction::Input { text } => Some(text),
                    ExitAction::Manual => None,
                });

            if let Some(text) = exit_input
                && matches!(rt.agent_status.as_deref(), Some("idle" | "done"))
                && client
                    .call("terminal.send", json!({ "terminal": addr, "text": text }))
                    .is_ok()
            {
                let _ = self.store.lock().unwrap().write(|db| {
                    let mut cur: ScheduledRun = db.get(&run.id)?;
                    cur.close_requested_at = Some(now_ms);
                    db.save(&cur)?;
                    if let Ok(mut e) = db.get::<TerminalEntry>(tid)
                        && let Some(r) = &mut e.runtime
                    {
                        r.state = "closing".into();
                        db.save(&e)?;
                    }
                    Ok(())
                });
                self.emit("schedules", None);
                self.emit("terminals", None);
                continue;
            }

            let _ = client.call("terminal.kill", json!({ "terminal": addr }));
            self.mark_scheduled_terminal_finished(tid);
            self.emit("schedules", None);
        }
        Ok(())
    }

    fn mark_scheduled_terminal_finished(&self, terminal_id: &str) {
        self.presence.remove_scheduled_track(terminal_id);
        let _ = self.store.lock().unwrap().write(|db| {
            if let Ok(mut entry) = db.get::<TerminalEntry>(terminal_id) {
                if let Some(rt) = &mut entry.runtime {
                    rt.state = "finished".into();
                    rt.agent_status = None;
                    rt.agent_turn = None;
                    rt.observed_agent = None;
                    rt.waiting = None;
                }
                db.save(&entry)?;
            }
            Ok(())
        });
        self.emit("terminals", None);
    }

    fn capture_scheduled_output(
        &self,
        schedule_run_id: &str,
        entry: &TerminalEntry,
    ) -> Result<Option<String>> {
        let Some(rt) = entry.runtime.as_ref() else {
            return Ok(None);
        };
        let Some(addr) = rt.terminal.as_deref() else {
            return Ok(None);
        };
        let client = self.herdr_client();
        if rt.core_id.as_deref() != self.verified_core_id().ok().as_deref() {
            return Ok(None);
        }
        let value = client.read(addr)?;
        let Some(text) = value["text"].as_str() else {
            return Ok(None);
        };
        let run: ScheduledRun = self.store.lock().unwrap().db().get(schedule_run_id)?;
        let dir = self.schedule_run_dir(&self.home, &run);
        let path = dir.join("output.txt");
        canban_context::atomic(&path, text.as_bytes())?;
        Ok(Some(canban_context::relative(&self.home, &path)?))
    }

    pub(super) fn schedule_run_dir(&self, home: &Path, run: &ScheduledRun) -> PathBuf {
        if is_global_project_id(&run.project_id) {
            home.join("global").join("schedules").join(run.short_id())
        } else {
            home.join("projects")
                .join(&run.project_id)
                .join("schedules")
                .join(run.short_id())
        }
    }
}

impl AppServer {
    fn launch_queued_cardless_schedules(&self) -> Result<()> {
        let snapshot = self.snapshot()?;
        let Some(queued) =
            next_scheduled_run_at(&snapshot, self.config().scheduler.max_agents, self.now_ms())
        else {
            return Ok(());
        };
        let _gate = self.terminal_gate.lock().unwrap();
        let admitted = self.store.lock().unwrap().write(|db| {
            let current = db.snapshot()?;
            let mut run: ScheduledRun = db.get(&queued.id)?;
            if run.state != "queued" || run.creates_card {
                return Ok(None);
            }
            let now_ms = self.now_ms();
            if run.terminal_id.is_some()
                && canban_scheduler::active_count_at(&current, now_ms)
                    >= self.config().scheduler.max_agents
            {
                return Ok(None);
            }
            let project_opt: Option<Project> = if is_global_project_id(&run.project_id) {
                None
            } else {
                Some(db.get(&run.project_id)?)
            };
            let rev: WorkflowRevision = super::workflow::published(db, &run.workflow_id)?;
            run.state = "starting".into();
            if run.started_at.is_none() {
                run.started_at = Some(now_ms);
            }
            run.updated_at = now_ms;
            db.save(&run)?;
            if let Some(tid) = run.terminal_id.as_deref()
                && let Ok(mut entry) = db.get::<TerminalEntry>(tid)
                && let Some(rt) = &mut entry.runtime
            {
                rt.state = "starting".into();
                rt.connection = "connecting".into();
                db.save(&entry)?;
            }
            Ok(Some((run, project_opt, rev)))
        })?;
        let Some((mut run, project_opt, rev)) = admitted else {
            return Ok(());
        };

        if let Err(e) = self.execute_cardless_scheduled_run(&mut run, project_opt.as_ref(), &rev) {
            let receipt_uncertain = e.downcast_ref::<crate::LaunchReceiptUncertain>().is_some();
            let err_msg = format!("{e:#}");
            self.store.lock().unwrap().write(|db| {
                let mut cur: ScheduledRun = db.get(&run.id)?;
                if !matches!(cur.state.as_str(), "completed" | "failed" | "blocked") {
                    if receipt_uncertain {
                        cur.state = "unknown".into();
                        cur.error = Some(err_msg.clone());
                    } else {
                        cur.state = "failed".into();
                        cur.error = Some(err_msg.clone());
                        cur.completed_at = Some(self.now_ms());
                    }
                    db.save(&cur)?;
                }
                if let Some(tid) = cur.terminal_id.as_deref()
                    && let Ok(mut entry) = db.get::<TerminalEntry>(tid)
                    && let Some(rt) = &mut entry.runtime
                    && rt.terminal.is_none()
                {
                    if receipt_uncertain {
                        rt.state = "unknown".into();
                        rt.connection = "unavailable".into();
                    } else {
                        rt.state = "finished".into();
                    }
                    rt.error = Some(err_msg);
                    db.save(&entry)?;
                }
                Ok(())
            })?;
            self.emit("schedules", None);
            self.emit("terminals", None);
        }
        Ok(())
    }

    fn execute_cardless_scheduled_run(
        &self,
        run: &mut ScheduledRun,
        project: Option<&Project>,
        rev: &WorkflowRevision,
    ) -> Result<()> {
        let directory = project
            .map(|p| p.directory.clone())
            .unwrap_or_else(|| self.home.to_string_lossy().into_owned());
        let ops = self.ops();
        let run_dir = self.schedule_run_dir(&self.home, run);
        std::fs::create_dir_all(run_dir.join("reports"))?;

        let handoff_path = run_dir.join("handoff.json");
        let (mut shared_env, mut script_notes) = if handoff_path.exists()
            && let Ok(raw) = std::fs::read_to_string(&handoff_path)
            && let Ok(v) = serde_json::from_str::<Value>(&raw)
        {
            let env = serde_json::from_value::<BTreeMap<String, String>>(v["env"].clone())
                .unwrap_or_else(|_| rev.definition.env.clone());
            let notes =
                serde_json::from_value::<Vec<String>>(v["notes"].clone()).unwrap_or_default();
            (env, notes)
        } else {
            (rev.definition.env.clone(), Vec::<String>::new())
        };
        let mut step_cursor = run.current_step.clone().or_else(|| run.start_step.clone());
        let mut visited_failure = BTreeSet::<String>::new();

        for _ in 0..32 {
            let Some(step_id) = step_cursor.clone() else {
                let summary = if script_notes.is_empty() {
                    "Scheduled script workflow completed".to_owned()
                } else {
                    script_notes.join("; ")
                };
                self.complete_script_only_scheduled_run(run, "completed", Some(summary), None)?;
                return Ok(());
            };
            let step = rev
                .definition
                .steps
                .get(&step_id)
                .with_context(|| format!("Unknown scheduled step `{step_id}`"))?;
            match step {
                WorkflowStep::Complete => {
                    let summary = if script_notes.is_empty() {
                        "Scheduled workflow completed".to_owned()
                    } else {
                        script_notes.join("; ")
                    };
                    self.complete_script_only_scheduled_run(run, "completed", Some(summary), None)?;
                    return Ok(());
                }
                WorkflowStep::Notify { message, next } => {
                    script_notes.push(message.clone());
                    run.steps.push(ScheduledStepRecord {
                        step_id: step_id.clone(),
                        kind: "notify".into(),
                        state: "completed".into(),
                        summary: Some(message.clone()),
                        ..Default::default()
                    });
                    step_cursor = next.clone();
                }
                WorkflowStep::Script {
                    run: body,
                    shell,
                    working_directory,
                    timeout_minutes,
                    timeout_seconds,
                    continue_on_error,
                    on_failure,
                    env: step_env,
                    next,
                } => {
                    let step_started_at = self.now_ms();
                    run.current_step = Some(step_id.clone());
                    run.state = "running".into();
                    let _ = self.store.lock().unwrap().write(|db| {
                        let mut cur: ScheduledRun = db.get(&run.id)?;
                        cur.state = "running".into();
                        cur.current_step = Some(step_id.clone());
                        db.save(&cur)
                    });
                    self.emit("schedules", None);

                    let base = Path::new(&directory);
                    let script_cwd = crate::scripts::resolve_working_directory(
                        &ops.host,
                        base,
                        working_directory.as_deref(),
                    )?;
                    let slug = crate::scripts::sanitized_step_slug(&step_id);
                    let scripts_dir = run_dir.join("scripts");
                    std::fs::create_dir_all(&scripts_dir)?;
                    let script_file = scripts_dir.join(format!("{slug}.sh"));
                    let env_file = scripts_dir.join(format!("{slug}.env"));
                    let output_file = scripts_dir.join(format!("{slug}.output"));
                    let log_file = scripts_dir.join(format!("{slug}.log"));
                    canban_context::atomic(&script_file, body.as_bytes())?;
                    canban_context::atomic(&env_file, b"")?;
                    canban_context::atomic(&output_file, b"")?;

                    let mut protected =
                        self.base_schedule_env(project, run, &run_dir, Some(&step_id));
                    protected.insert("CANBAN_ENV".into(), env_file.to_string_lossy().into_owned());
                    protected.insert(
                        "CANBAN_OUTPUT".into(),
                        output_file.to_string_lossy().into_owned(),
                    );
                    let mut wf_env = shared_env.clone();
                    for (k, v) in step_env {
                        wf_env.insert(k.clone(), v.clone());
                    }
                    let resolved_env = self.resolve_runtime_env(
                        None,
                        project,
                        &protected,
                        Some(&wf_env),
                        Some("local"),
                    )?;
                    let argv = crate::scripts::shell_argv(shell.as_deref(), &script_file)?;
                    let timeout_secs =
                        crate::scripts::script_timeout_secs(*timeout_minutes, *timeout_seconds);
                    let cfg = self.config();
                    let env_policy = canban_exec::shell_env::LocalEnvPolicy::for_source_trust(
                        cfg.terminal_env_mode.is_inherit(),
                        Some("local"),
                    );
                    let (exit_code, timed_out) =
                        crate::scripts::run_local_process_with_policy_unset_and_runner(
                            &argv,
                            &script_cwd,
                            &resolved_env.env,
                            &cfg.hc_env,
                            env_policy,
                            &resolved_env.unset_keys,
                            &log_file,
                            timeout_secs,
                            self.exec_runner.as_ref(),
                        )?;
                    let duration_ms = self.now_ms().saturating_sub(step_started_at);
                    if let Ok(raw_env) = std::fs::read_to_string(&env_file)
                        && let Ok(parsed) = crate::scripts::parse_key_value_file(&raw_env)
                    {
                        for (k, v) in parsed {
                            shared_env.insert(k, v);
                        }
                    }
                    let raw_log_text = std::fs::read_to_string(&log_file).unwrap_or_default();
                    let log_text =
                        crate::env::mask_secrets(&raw_log_text, &resolved_env.secret_values);
                    if log_text != raw_log_text {
                        let _ = canban_context::atomic(&log_file, log_text.as_bytes());
                    }
                    let rel_log = canban_context::relative(&self.home, &log_file)?;
                    let ok = !timed_out && exit_code == Some(0);
                    let tail = crate::scripts::log_tail(&log_text);
                    let last_line = log_text
                        .lines()
                        .rev()
                        .map(str::trim)
                        .find(|l| !l.is_empty())
                        .map(str::to_owned);
                    if ok || *continue_on_error {
                        let note = last_line
                            .clone()
                            .unwrap_or_else(|| format!("Script `{step_id}` succeeded"));
                        script_notes.push(note.clone());
                        run.output_path = Some(rel_log.clone());
                        run.steps.push(ScheduledStepRecord {
                            step_id: step_id.clone(),
                            kind: "script".into(),
                            state: if ok { "completed" } else { "failed" }.into(),
                            profile_id: None,
                            exit_code,
                            duration_ms: Some(duration_ms),
                            log_path: Some(rel_log),
                            summary: Some(note),
                        });
                        step_cursor = next.clone();
                    } else if let Some(target) = on_failure
                        && visited_failure.insert(step_id.clone())
                    {
                        script_notes.push(format!(
                            "Script `{step_id}` failed (exit {exit_code:?}):\n```\n{tail}\n```"
                        ));
                        run.output_path = Some(rel_log.clone());
                        run.steps.push(ScheduledStepRecord {
                            step_id: step_id.clone(),
                            kind: "script".into(),
                            state: "failed".into(),
                            profile_id: None,
                            exit_code,
                            duration_ms: Some(duration_ms),
                            log_path: Some(rel_log),
                            summary: last_line.or_else(|| {
                                Some(format!("Exited with code {}", exit_code.unwrap_or(-1)))
                            }),
                        });
                        step_cursor = Some(target.clone());
                    } else {
                        let err = if timed_out {
                            format!("Scheduled script `{step_id}` timed out after {timeout_secs}s")
                        } else {
                            format!("Scheduled script `{step_id}` failed with exit {exit_code:?}")
                        };
                        run.output_path = Some(rel_log.clone());
                        run.steps.push(ScheduledStepRecord {
                            step_id: step_id.clone(),
                            kind: "script".into(),
                            state: "failed".into(),
                            profile_id: None,
                            exit_code,
                            duration_ms: Some(duration_ms),
                            log_path: Some(rel_log),
                            summary: Some(err.clone()),
                        });
                        self.complete_script_only_scheduled_run(run, "failed", None, Some(err))?;
                        return Ok(());
                    }
                }
                WorkflowStep::Agent {
                    profile: step_prof,
                    instruction,
                    env: step_env,
                    ..
                } => {
                    // Lazy admission: if this run started as a `script` without a TerminalEntry,
                    // allocate the TerminalEntry now and verify `scheduler.max_agents` capacity.
                    if run.terminal_id.is_none() {
                        let config = self.config();
                        let prof = canban_scheduler::select_in_project(
                            &config,
                            project,
                            step_prof.as_deref().or(run.profile_id.as_deref()),
                        )?;
                        let mut deferred = false;
                        self.store.lock().unwrap().write(|db| {
                            let snap = db.snapshot()?;
                            let now_ms = self.now_ms();
                            let slot_free = canban_scheduler::active_count_at(&snap, now_ms)
                                < config.scheduler.max_agents;
                            let entry = TerminalEntry {
                                id: run.id.clone(),
                                conversation_id: None,
                                number: db.counter("terminal-number")?,
                                name: Some(format!("{} #{}", run.workflow_name, run.number)),
                                origin_project_id: Some(project.map_or_else(
                                    || GLOBAL_PROJECT_ID.to_owned(),
                                    |p| p.id.clone(),
                                )),
                                origin_card_id: None,
                                agent_engine: Some(profile_agent_engine(&prof)),
                                created_at: now_ms,
                                runtime: Some(TerminalRuntime {
                                    core_id: None,
                                    terminal: None,
                                    state: if slot_free { "starting" } else { "queued" }.into(),
                                    connection: "connecting".into(),
                                    error: None,
                                    agent_status: None,
                                    agent_turn: None,
                                    observed_agent: None,
                                    waiting: None,
                                    command: None,
                                }),
                                agent: None,
                            };
                            db.save(&entry)?;
                            let mut cur: ScheduledRun = db.get(&run.id)?;
                            cur.terminal_id = Some(run.id.clone());
                            cur.profile_id = Some(prof.id.clone());
                            cur.current_step = Some(step_id.clone());
                            cur.steps = run.steps.clone();
                            cur.output_path = run.output_path.clone();
                            if !slot_free {
                                cur.state = "queued".into();
                                deferred = true;
                            }
                            db.save(&cur)?;
                            *run = cur;
                            Ok(())
                        })?;
                        if deferred {
                            let _ = canban_context::atomic(
                                &handoff_path,
                                &serde_json::to_vec(&json!({
                                    "env": shared_env,
                                    "notes": script_notes,
                                }))?,
                            );
                            self.emit("schedules", None);
                            self.emit("terminals", None);
                            return Ok(());
                        }
                    }
                    return self.launch_cardless_agent_step(
                        run,
                        project,
                        &directory,
                        &run_dir,
                        &step_id,
                        step_prof.as_deref(),
                        Some(instruction.as_str()),
                        &shared_env,
                        step_env,
                        &script_notes,
                    );
                }
                other => {
                    bail!(
                        "Step `{step_id}` ({other:?}) requires a Card; add `card:` to the schedule rule"
                    );
                }
            }
        }
        bail!("Scheduled workflow exceeded maximum step transitions")
    }
}

impl AppServer {
    #[allow(clippy::too_many_arguments)]
    fn launch_cardless_agent_step(
        &self,
        run: &mut ScheduledRun,
        project: Option<&Project>,
        directory: &str,
        run_dir: &Path,
        step_id: &str,
        step_prof: Option<&str>,
        instruction: Option<&str>,
        shared_env: &BTreeMap<String, String>,
        step_env: &BTreeMap<String, String>,
        script_notes: &[String],
    ) -> Result<()> {
        let config = self.config();
        let prof = canban_scheduler::select_in_project(
            &config,
            project,
            step_prof.or(run.profile_id.as_deref()),
        )?;
        let session_id = if prof.command().iter().any(|s| s.contains("{session_id}")) {
            Some(self.next_uuid())
        } else {
            None
        };
        let mut protected = self.base_schedule_env(project, run, run_dir, Some(step_id));
        protected.insert("CANBAN_PROFILE_ID".into(), prof.id.clone());
        protected.insert(
            "CANBAN_SESSION_ID".into(),
            session_id.clone().unwrap_or_default(),
        );
        let mut wf_env = shared_env.clone();
        for (k, v) in step_env {
            wf_env.insert(k.clone(), v.clone());
        }

        let prompt = self.render_cardless_schedule_prompt(
            project,
            directory,
            run,
            &prof,
            step_id,
            instruction,
            script_notes,
        );
        canban_context::atomic(&run_dir.join("prompt.md"), prompt.as_bytes())?;
        canban_context::atomic(
            &run_dir.join("context.json"),
            &serde_json::to_vec_pretty(&json!({
                "schedule_run_id": run.id,
                "schedule_id": run.schedule_id,
                "scope": if project.is_some() { "project" } else { "global" },
                "project": project.map(|p| json!({ "id": p.id, "name": p.name, "directory": directory })).unwrap_or(Value::Null),
                "cwd": directory,
                "workflow_id": run.workflow_id,
                "workflow_name": run.workflow_name,
                "rule_id": run.rule_id,
                "cron": run.cron,
                "run_number": run.number,
                "step_id": step_id,
                "profile_id": prof.id,
                "instruction": instruction.unwrap_or_default(),
            }))?,
        )?;

        let mut argv = canban_profiles::command(
            &prof,
            session_id.as_deref(),
            true,
            &[
                ("home", &self.home.to_string_lossy()),
                ("cwd", directory),
                ("project_dir", directory),
                ("cli", &self.cli.to_string_lossy()),
                ("prompt", &prompt),
            ],
        )?;
        let is_herdr_template = prof
            .preset
            .as_deref()
            .is_some_and(|s| s.starts_with("herdr:"));
        if !is_herdr_template {
            argv[0] = canban_profiles::executable_with_runner(
                &argv[0],
                &config.hc_env,
                self.exec_runner.as_ref(),
            );
        }

        let core_id = self.verified_core_id()?;
        let mut labels = BTreeMap::from([
            ("canban.schedule_run".into(), run.id.clone()),
            ("canban.terminal".into(), run.id.clone()),
        ]);
        if let Some(proj) = project {
            labels.insert("canban.project".into(), proj.id.clone());
        } else {
            labels.insert("canban.scope".into(), "global".into());
        }
        let receipt = self
            .launch_herdr_with_env(crate::env::HerdrLaunchRequest {
                profile: Some(&prof),
                project,
                protected_context: &protected,
                workflow_env: Some(&wf_env),
                source_trust: Some("local"),
                argv,
                cwd: directory.to_owned(),
                labels,
                size: None,
            })?
            .receipt;
        let terminal_address = strp(&receipt, "address")?.to_owned();
        {
            let mut inv = self.inventory.lock().unwrap();
            if inv.core == core_id {
                inv.records
                    .entry(terminal_address.clone())
                    .or_insert_with(|| {
                        let mut seeded = receipt.clone();
                        seeded["address"] = json!(terminal_address);
                        if seeded["state"].is_null() {
                            seeded["state"] = json!("live");
                        }
                        if seeded["cwd"].is_null() {
                            seeded["cwd"] = json!(directory);
                        }
                        seeded
                    });
            }
        }
        let prev_out = run.output_path.clone();
        let mut steps = run.steps.clone();
        if !steps
            .iter()
            .any(|s| s.step_id == step_id && s.kind == "agent")
        {
            steps.push(ScheduledStepRecord {
                step_id: step_id.to_owned(),
                kind: "agent".into(),
                state: "running".into(),
                profile_id: Some(prof.id.clone()),
                ..Default::default()
            });
        }
        self.store.lock().unwrap().write(|db| {
            let mut cur: ScheduledRun = db.get(&run.id)?;
            cur.current_step = Some(step_id.to_owned());
            cur.profile_id = Some(prof.id.clone());
            if cur.provider_session_id.is_none() {
                cur.provider_session_id = session_id.clone();
            }
            if prev_out.is_some() && cur.output_path.is_none() {
                cur.output_path = prev_out;
            }
            cur.steps = steps;
            if cur.state == "starting" {
                cur.state = "running".into();
            }
            cur.cwd = directory.to_owned();
            db.save(&cur)?;

            let tid = cur.terminal_id.clone().unwrap_or_else(|| run.id.clone());
            let mut entry: TerminalEntry = db.get(&tid)?;
            entry.agent_engine = Some(profile_agent_engine(&prof));
            entry.runtime = Some(TerminalRuntime {
                core_id: Some(core_id),
                terminal: Some(terminal_address),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
                command: None,
            });
            db.save(&entry)
        })?;
        self.emit("schedules", None);
        self.emit("terminals", None);
        ensure!(
            receipt
                .get("first_message")
                .is_none_or(|v| v["sent"] != false),
            "Herdr did not deliver the initial scheduled prompt; inspect Terminal"
        );
        Ok(())
    }

    fn complete_script_only_scheduled_run(
        &self,
        run: &ScheduledRun,
        state: &str,
        summary: Option<String>,
        error: Option<String>,
    ) -> Result<()> {
        let out_path = run.output_path.clone();
        let steps = run.steps.clone();
        self.store.lock().unwrap().write(|db| {
            let mut cur: ScheduledRun = db.get(&run.id)?;
            cur.state = state.into();
            cur.summary = summary;
            cur.error = error;
            cur.steps = steps;
            if out_path.is_some() {
                cur.output_path = out_path;
            }
            cur.completed_at = Some(self.now_ms());
            db.save(&cur)?;
            if let Some(tid) = cur.terminal_id.as_deref()
                && let Ok(mut entry) = db.get::<TerminalEntry>(tid)
                && let Some(rt) = &mut entry.runtime
            {
                rt.state = "finished".into();
                db.save(&entry)?;
            }
            Ok(())
        })?;
        self.emit("schedules", None);
        self.emit("terminals", None);
        Ok(())
    }

    fn base_schedule_env(
        &self,
        project: Option<&Project>,
        run: &ScheduledRun,
        run_dir: &Path,
        step_id: Option<&str>,
    ) -> BTreeMap<String, String> {
        let mut env = BTreeMap::from([
            ("CANBAN_CLI".into(), self.cli.to_string_lossy().into_owned()),
            (
                "CANBAN_HOME".into(),
                self.home.to_string_lossy().into_owned(),
            ),
            (
                "CANBAN_ENDPOINT".into(),
                self.endpoint().to_string_lossy().into_owned(),
            ),
            ("CANBAN_SCHEDULE_ID".into(), run.schedule_id.clone()),
            ("CANBAN_SCHEDULE_RUN_ID".into(), run.id.clone()),
            ("CANBAN_WORKFLOW".into(), run.workflow_id.clone()),
            (
                "CANBAN_RUN_DIR".into(),
                run_dir.to_string_lossy().into_owned(),
            ),
        ]);
        if let Some(proj) = project {
            env.insert("CANBAN_SCOPE".into(), "project".into());
            env.insert("CANBAN_PROJECT".into(), proj.id.clone());
            env.insert("CANBAN_PROJECT_DIR".into(), proj.directory.clone());
            env.insert("CANBAN_WORKSPACE".into(), proj.directory.clone());
        } else {
            env.insert("CANBAN_SCOPE".into(), "global".into());
            env.insert(
                "CANBAN_WORKSPACE".into(),
                self.home.to_string_lossy().into_owned(),
            );
        }
        if let Some(sid) = step_id {
            env.insert("CANBAN_STEP_ID".into(), sid.to_owned());
        }
        env
    }

    #[allow(clippy::too_many_arguments)]
    fn render_cardless_schedule_prompt(
        &self,
        project: Option<&Project>,
        directory: &str,
        run: &ScheduledRun,
        profile: &Profile,
        step_id: &str,
        instruction: Option<&str>,
        script_notes: &[String],
    ) -> String {
        let scope_line = match project {
            Some(proj) => format!(
                "You are running as scheduled Terminal Agent `@{}` for Project `{}` (`{}`).",
                profile.id, proj.name, proj.id
            ),
            None => format!(
                "You are running as a host-level Global scheduled Terminal Agent `@{}`.",
                profile.id
            ),
        };
        let mut out = format!(
            "# Scheduled Task: {} (#{})\n\n\
             {scope_line}\n\
             This scheduled run operates directly in `{}` without creating a Kanban Card.\n\n\
             - Schedule Rule: `{}` (`{}`)\n\
             - Workflow: `{}` (step `{}`)\n\
             - Trigger: `{}`\n\
             - Run ID: `{}`\n",
            run.workflow_name,
            run.number,
            directory,
            run.rule_id,
            run.cron,
            run.workflow_id,
            step_id,
            run.trigger_kind,
            run.id,
        );
        if !script_notes.is_empty() {
            out.push_str("\n## Preceding Step Context\n");
            for note in script_notes {
                out.push_str("- ");
                out.push_str(note);
                out.push('\n');
            }
        }
        out.push_str("\n## Task Instructions\n");
        let body = instruction
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(
                "Execute the scheduled maintenance or inspection task described by this workflow.",
            );
        out.push_str(body);
        out.push_str(
            "\n\n## Reporting Outcome\n\
             - If you need human confirmation or a decision before proceeding, call `report --state blocked --message '...'` (this keeps your live terminal open and notifies the user in `Needs you` so they can reply here).\n\
             - When finished, report your final outcome via the injected CLI so Canban records the run summary and reclaims this Agent session cleanly:\n\
             ```sh\n\
             \"$CANBAN_CLI\" report --state <completed|blocked|failed> --message 'Concise summary' [--file \"$CANBAN_RUN_DIR/report.md\"]\n\
             ```\n\
             Keep temporary or report files under `$CANBAN_RUN_DIR` (`$CANBAN_RUN_DIR/report.md`).\n",
        );
        out
    }
}

impl AppServer {
    pub(super) fn schedule_bind(&self, p: Value) -> Result<Value> {
        let run_id = p["schedule_run_id"]
            .as_str()
            .or_else(|| p["terminal_id"].as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .context("Missing schedule_run_id")?
            .to_owned();
        let session = strp(&p, "session_id")?.trim().to_owned();
        ensure!(
            !session.is_empty() && session.len() < 512,
            "Invalid provider session ID"
        );
        self.mutate("schedule.bind", &p, |db| {
            let mut run: ScheduledRun = db.get(&run_id)?;
            run.provider_session_id = Some(session.clone());
            db.save(&run)?;
            Ok(json!({ "bound": true, "schedule_run_id": run.id, "session_id": session }))
        })
    }

    pub(super) fn schedule_report(&self, p: Value) -> Result<Value> {
        let state = strp(&p, "state")?;
        ensure!(
            ["working", "completed", "blocked", "failed"].contains(&state),
            "invalid_report: scheduled Terminal Agent supports working | completed | blocked | failed"
        );
        let run_id = p["schedule_run_id"]
            .as_str()
            .or_else(|| {
                p["terminal_id"]
                    .as_str()
                    .filter(|s| s.trim().starts_with("scheduled:"))
            })
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .context("Missing schedule_run_id")?
            .to_owned();
        let key = strp(&p, "request_key")?;
        let fingerprint = canban_context::digest(&serde_json::to_vec(&p)?);

        let run_check: ScheduledRun = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            if let Some(old) = db
                .list::<Report>()?
                .into_iter()
                .find(|r| r.request_key == key)
            {
                ensure!(old.fingerprint == fingerprint, "request_key_conflict");
                return Ok(json!({ "accepted": true, "duplicate": true }));
            }
            db.get(&run_id)?
        };
        ensure!(
            !run_check.creates_card,
            "Card-backed scheduled runs report against their Card"
        );

        let report_id = self.next_id();
        let report_path = if let Some(body) = p["body"].as_str() {
            let path = self
                .schedule_run_dir(&self.home, &run_check)
                .join("reports")
                .join(format!("{report_id}.md"));
            canban_context::atomic(&path, body.as_bytes())?;
            Some(canban_context::relative(&self.home, &path)?)
        } else {
            None
        };
        let message = p["message"].as_str().unwrap_or("").trim().to_owned();

        let res = self.mutate("schedule.report", &p, |db| {
            if let Some(old) = db
                .list::<Report>()?
                .into_iter()
                .find(|r| r.request_key == key)
            {
                ensure!(old.fingerprint == fingerprint, "request_key_conflict");
                return Ok(json!({ "accepted": true, "duplicate": true }));
            }
            let mut cur: ScheduledRun = db.get(&run_id)?;
            let profile_label = cur
                .profile_id
                .clone()
                .or_else(|| {
                    p["profile_id"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| self.config().default_agent);
            let actor = format!("agent:{profile_label}");
            let now_ms = self.now_ms();
            let report = Report {
                id: report_id.clone(),
                terminal_id: cur.terminal_id.clone(),
                workflow_run_id: None,
                project_id: cur.project_id.clone(),
                card_id: String::new(),
                actor,
                state: state.to_owned(),
                message: message.clone(),
                path: report_path.clone(),
                request_key: key.to_owned(),
                fingerprint,
                created_at: now_ms,
            };
            db.save(&report)?;

            cur.state = state.to_owned();
            if !message.is_empty() {
                cur.summary = Some(message.clone());
            }
            if report_path.is_some() {
                cur.report_path = report_path.clone();
            }
            if let Some(step) = cur.steps.iter_mut().rev().find(|s| s.kind == "agent") {
                step.state = match state {
                    "working" => "running".into(),
                    other => other.to_owned(),
                };
                if !message.is_empty() {
                    step.summary = Some(message.clone());
                }
            }
            if state == "working" {
                cur.error = None;
            } else if state == "blocked" {
                // Keep PTY alive when Agent reports `blocked` so human can respond in Terminal.
                cur.completed_at = None;
                cur.close_requested_at = None;
                if !message.is_empty() {
                    cur.error = Some(message);
                }
            } else {
                cur.completed_at = Some(now_ms);
                if state == "failed" && !message.is_empty() {
                    cur.error = Some(message);
                } else if state == "completed" {
                    cur.error = None;
                }
            }
            db.save(&cur)?;
            Ok(json!({
                "accepted": true,
                "duplicate": false,
                "schedule_run_id": cur.id,
                "recorded_state": cur.state,
            }))
        })?;
        self.emit("schedules", None);
        self.wake_runtime();
        Ok(res)
    }

    pub(super) fn schedule_call(&self, method: &str, p: Value) -> Result<Value> {
        let resolve_filter_scope = |db: Db<'_>, params: &Value| -> Result<Option<String>> {
            if params["scope"]
                .as_str()
                .is_some_and(|s| s.trim().eq_ignore_ascii_case("global"))
            {
                return Ok(Some(GLOBAL_PROJECT_ID.to_owned()));
            }
            params["project_id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|raw| super::resolve_workflow_scope(db, raw))
                .transpose()
        };

        match method {
            "schedule.bind" => self.schedule_bind(p),
            "schedule.report" => self.schedule_report(p),
            "schedule.list" => {
                let store = self.store.lock().unwrap();
                let db = store.db();
                let filter_pid = resolve_filter_scope(db, &p)?;
                let mut schedules: Vec<ScheduleState> = db
                    .list::<ScheduleState>()?
                    .into_iter()
                    .filter(|s| filter_pid.as_deref().is_none_or(|pid| s.project_id == pid))
                    .collect();
                schedules.sort_by(|a, b| {
                    is_global_project_id(&b.project_id)
                        .cmp(&is_global_project_id(&a.project_id))
                        .then_with(|| a.project_id.cmp(&b.project_id))
                        .then_with(|| a.filename.cmp(&b.filename))
                        .then_with(|| a.rule_id.cmp(&b.rule_id))
                });
                let mut runs: Vec<ScheduledRun> = db
                    .list::<ScheduledRun>()?
                    .into_iter()
                    .filter(|r| filter_pid.as_deref().is_none_or(|pid| r.project_id == pid))
                    .collect();
                runs.sort_by(|a, b| {
                    b.scheduled_at
                        .cmp(&a.scheduled_at)
                        .then_with(|| b.number.cmp(&a.number))
                });
                Ok(json!({ "schedules": schedules, "runs": runs }))
            }
            "schedule.runs" => {
                let store = self.store.lock().unwrap();
                let db = store.db();
                let filter_pid = resolve_filter_scope(db, &p)?;
                let filter_sid = p["schedule_id"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let limit = p["limit"].as_u64().unwrap_or(50).clamp(1, 200) as usize;
                let mut runs: Vec<ScheduledRun> = db
                    .list::<ScheduledRun>()?
                    .into_iter()
                    .filter(|r| filter_pid.as_deref().is_none_or(|pid| r.project_id == pid))
                    .filter(|r| filter_sid.is_none_or(|sid| r.schedule_id == sid))
                    .collect();
                runs.sort_by(|a, b| {
                    b.scheduled_at
                        .cmp(&a.scheduled_at)
                        .then_with(|| b.number.cmp(&a.number))
                });
                runs.truncate(limit);
                Ok(json!({ "runs": runs }))
            }
            "schedule.create" => {
                ensure!(
                    p["execution"].is_null(),
                    "Creating a schedule requires a user action"
                );
                let res = self.mutate(method, &p, |db| {
                    let config = self.config();
                    let scope_id = resolve_filter_scope(db, &p)?
                        .unwrap_or_else(|| GLOBAL_PROJECT_ID.to_owned());
                    let is_global = is_global_project_id(&scope_id);
                    let now_ms = self.now_ms();
                    let project_opt: Option<Project> = if is_global {
                        authoring::initialize_global(&self.home, db, &config, now_ms)?;
                        None
                    } else {
                        let proj: Project = db.get(&scope_id)?;
                        authoring::initialize(&self.home, db, &config, &proj, now_ms)?;
                        Some(proj)
                    };

                    let name = strp(&p, "name")?.trim().to_owned();
                    ensure!(!name.is_empty(), "Schedule name is required");
                    let cron = strp(&p, "cron")?.trim().to_owned();
                    ensure!(!cron.is_empty(), "Cron expression is required");
                    let timezone = p["timezone"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("local"))
                        .map(str::to_owned);
                    let overlap = match p["overlap"].as_str().map(str::trim) {
                        Some(raw) if !raw.is_empty() => ScheduleOverlap::parse(raw)
                            .with_context(|| format!("Unknown overlap policy `{raw}`"))?,
                        _ => ScheduleOverlap::Skip,
                    };
                    let enabled = p["enabled"].as_bool().unwrap_or(true);
                    let mode_raw = p["mode"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or("agent");
                    let shell = p["shell"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let working_directory = p["working_directory"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let timeout_minutes = p["timeout_minutes"].as_u64().map(|v| v as u32);
                    let profile = p["profile_id"]
                        .as_str()
                        .or_else(|| p["profile"].as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let instruction = p["instruction"].as_str().unwrap_or("").trim().to_owned();
                    let script_body = p["script"].as_str().unwrap_or("").trim().to_owned();

                    let mode = match mode_raw {
                        "script" => {
                            ensure!(!script_body.is_empty(), "Script body is required");
                            canban_workflow_engine::source::ScheduledWorkflowMode::ScriptOnly {
                                run: script_body,
                                shell,
                                working_directory,
                                timeout_minutes,
                            }
                        }
                        "script_agent" => {
                            ensure!(!script_body.is_empty(), "Script body is required");
                            canban_workflow_engine::source::ScheduledWorkflowMode::ScriptWithAgentFallback {
                                run: script_body,
                                shell,
                                working_directory,
                                timeout_minutes,
                                profile,
                                instruction,
                            }
                        }
                        "agent" => {
                            canban_workflow_engine::source::ScheduledWorkflowMode::AgentOnly {
                                profile,
                                instruction,
                            }
                        }
                        "card" => {
                            ensure!(
                                !is_global,
                                "Global schedules cannot create Board Cards; select a Project"
                            );
                            canban_workflow_engine::source::ScheduledWorkflowMode::CreateCard {
                                column: p["card_column"]
                                    .as_str()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_owned),
                                title: p["card_title"]
                                    .as_str()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_owned),
                                body: p["card_body"]
                                    .as_str()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_owned),
                                profile,
                                instruction,
                            }
                        }
                        other => bail!(
                            "Unknown schedule mode `{other}`; use script | script_agent | agent | card"
                        ),
                    };

                    let current = authoring::scope_template_files(db, &scope_id)?;
                    let existing_names: BTreeSet<String> =
                        current.iter().map(|f| f.filename.clone()).collect();
                    let filename = if let Some(explicit) = p["filename"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        ensure!(
                            !existing_names.contains(explicit),
                            "Workflow file `{explicit}` already exists"
                        );
                        explicit.to_owned()
                    } else {
                        let slug: String = name
                            .to_ascii_lowercase()
                            .chars()
                            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                            .collect::<String>()
                            .split('-')
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join("-");
                        let stem = if slug.is_empty() {
                            "scheduled".to_owned()
                        } else {
                            format!("scheduled-{}", &slug[..slug.len().min(36)])
                        };
                        let mut cand = format!("{stem}.yaml");
                        let mut idx = 2usize;
                        while existing_names.contains(&cand) {
                            cand = format!("{stem}-{idx}.yaml");
                            idx += 1;
                        }
                        cand
                    };

                    let short = self.next_id();
                    let wf_id = if is_global {
                        format!("global:schedule-{short}")
                    } else {
                        format!("schedule:{scope_id}:{short}")
                    };
                    let doc = canban_workflow_engine::source::build_scheduled_workflow_document(
                        canban_workflow_engine::source::ScheduledWorkflowSpec {
                            workflow_id: wf_id.clone(),
                            name,
                            scope: if is_global {
                                WorkflowScope::Global
                            } else {
                                WorkflowScope::Project
                            },
                            cron,
                            timezone,
                            overlap,
                            enabled,
                            mode,
                        },
                    );
                    let yaml = canban_workflow_engine::source::encode(&doc)?;
                    let mut inputs: Vec<Value> = current
                        .into_iter()
                        .map(|f| json!({"filename": f.filename, "source": f.source}))
                        .collect();
                    inputs.push(json!({"filename": filename, "source": yaml}));

                    if let Some(proj) = project_opt.as_ref() {
                        authoring::publish_collection(
                            &self.home, db, proj, &config, &inputs, now_ms,
                        )?;
                    } else {
                        authoring::publish_global_collection(
                            &self.home, db, &config, &inputs, now_ms,
                        )?;
                    }

                    let created = db
                        .list::<ScheduleState>()?
                        .into_iter()
                        .find(|s| {
                            s.project_id == scope_id
                                && s.workflow_id == wf_id
                                && s.rule_id == "schedule"
                        })
                        .context("Created schedule state missing")?;
                    Ok(json!({ "schedule": created, "filename": filename }))
                })?;
                self.emit("schedules", None);
                self.wake_runtime();
                Ok(res)
            }
            "schedule.trigger" => {
                ensure!(
                    p["execution"].is_null(),
                    "Triggering a schedule requires a user action"
                );
                let mut created_card = None;
                let res = self.mutate(method, &p, |db| {
                    let schedule_id = if let Some(sid) = p["schedule_id"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        sid.to_owned()
                    } else {
                        let pid =
                            resolve_filter_scope(db, &p)?.context("Missing project_id or scope")?;
                        let rule_id = strp(&p, "rule_id")?;
                        if let Some(wf_id) = p["workflow_id"]
                            .as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                        {
                            schedule_state_id(&pid, wf_id, rule_id)
                        } else {
                            let filename = strp(&p, "filename")?;
                            let src = db
                                .list::<WorkflowSource>()?
                                .into_iter()
                                .find(|s| s.project_id == pid && s.filename == filename)
                                .context("Workflow source not found")?;
                            schedule_state_id(&pid, &src.workflow_id, rule_id)
                        }
                    };
                    let config = self.config();
                    let now_ms = self.now_ms();
                    let (run, card_id) = fire_schedule(
                        db,
                        &self.home,
                        &config,
                        &schedule_id,
                        now_ms,
                        "manual",
                        true,
                        now_ms,
                        self.id_gen.as_ref(),
                    )?;
                    created_card = card_id;
                    Ok(serde_json::to_value(run)?)
                })?;
                if let Some(cid) = created_card {
                    self.emit("application", Some(cid));
                }
                self.emit("schedules", None);
                self.wake_runtime();
                Ok(res)
            }
            "schedule.toggle" => {
                ensure!(
                    p["execution"].is_null(),
                    "Toggling a schedule requires a user action"
                );
                let enabled = p["enabled"]
                    .as_bool()
                    .context("Missing boolean field: enabled")?;
                let res = self.mutate(method, &p, |db| {
                    let (pid, filename, rule_id) = if let Some(sid) = p["schedule_id"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        let st: ScheduleState = db.get(sid)?;
                        (st.project_id, st.filename, st.rule_id)
                    } else {
                        (
                            resolve_filter_scope(db, &p)?.context("Missing project_id or scope")?,
                            strp(&p, "filename")?.to_owned(),
                            strp(&p, "rule_id")?.to_owned(),
                        )
                    };
                    let config = self.config();
                    let now_ms = self.now_ms();
                    let project_opt = if is_global_project_id(&pid) {
                        authoring::initialize_global(&self.home, db, &config, now_ms)?;
                        None
                    } else {
                        let proj: Project = db.get(&pid)?;
                        authoring::initialize(&self.home, db, &config, &proj, now_ms)?;
                        Some(proj)
                    };
                    let current = authoring::scope_template_files(db, &pid)?;
                    let mut found = false;
                    let mut inputs = Vec::with_capacity(current.len());
                    for file in current {
                        if file.filename == filename {
                            let mut doc = canban_workflow_engine::source::parse(&file.source)?;
                            if enabled {
                                doc.enabled = true;
                            }
                            let rule = doc.on.get_mut(&rule_id).with_context(|| {
                                format!("Rule `{rule_id}` not found in {filename}")
                            })?;
                            ensure!(rule.is_schedule(), "Rule `{rule_id}` is not a schedule");
                            rule.enabled = enabled;
                            found = true;
                            inputs.push(json!({
                                "filename": file.filename,
                                "source": canban_workflow_engine::source::encode(&doc)?,
                            }));
                        } else {
                            inputs.push(json!({
                                "filename": file.filename,
                                "source": file.source,
                            }));
                        }
                    }
                    ensure!(found, "Workflow file `{filename}` not found");
                    if let Some(project) = project_opt.as_ref() {
                        authoring::publish_collection(
                            &self.home, db, project, &config, &inputs, now_ms,
                        )?;
                    } else {
                        authoring::publish_global_collection(
                            &self.home, db, &config, &inputs, now_ms,
                        )?;
                    }
                    let updated = db
                        .list::<ScheduleState>()?
                        .into_iter()
                        .find(|s| {
                            s.project_id == pid && s.filename == filename && s.rule_id == rule_id
                        })
                        .context("Updated schedule state missing")?;
                    Ok(serde_json::to_value(updated)?)
                })?;
                self.emit("schedules", None);
                self.wake_runtime();
                Ok(res)
            }
            "schedule.delete" => {
                ensure!(
                    p["execution"].is_null(),
                    "Deleting a schedule requires a user action"
                );
                let res = self.mutate(method, &p, |db| {
                    let (pid, filename, rule_id) = if let Some(sid) = p["schedule_id"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        let st: ScheduleState = db.get(sid)?;
                        (st.project_id, st.filename, st.rule_id)
                    } else {
                        (
                            resolve_filter_scope(db, &p)?
                                .context("Missing project_id or scope")?,
                            strp(&p, "filename")?.to_owned(),
                            strp(&p, "rule_id")?.to_owned(),
                        )
                    };
                    let config = self.config();
                    let now_ms = self.now_ms();
                    let project_opt = if is_global_project_id(&pid) {
                        authoring::initialize_global(&self.home, db, &config, now_ms)?;
                        None
                    } else {
                        let proj: Project = db.get(&pid)?;
                        authoring::initialize(&self.home, db, &config, &proj, now_ms)?;
                        Some(proj)
                    };
                    let current = authoring::scope_template_files(db, &pid)?;
                    let total_files = current.len();
                    let mut found = false;
                    let mut inputs = Vec::with_capacity(current.len());
                    for file in current {
                        if file.filename == filename {
                            let mut doc = canban_workflow_engine::source::parse(&file.source)?;
                            ensure!(
                                doc.on.remove(&rule_id).is_some(),
                                "Rule `{rule_id}` not found in {filename}"
                            );
                            found = true;
                            if doc.on.is_empty() {
                                if !is_global_project_id(&pid) && total_files <= 1 {
                                    bail!(
                                        "Cannot remove the last trigger rule in `{filename}` when it is the Project's only Workflow file"
                                    );
                                }
                                // Drop the now-empty standalone scheduled workflow file.
                                continue;
                            }
                            inputs.push(json!({
                                "filename": file.filename,
                                "source": canban_workflow_engine::source::encode(&doc)?,
                            }));
                        } else {
                            inputs.push(json!({
                                "filename": file.filename,
                                "source": file.source,
                            }));
                        }
                    }
                    ensure!(found, "Workflow file `{filename}` not found");
                    if let Some(project) = project_opt.as_ref() {
                        authoring::publish_collection(
                            &self.home, db, project, &config, &inputs, now_ms,
                        )?;
                    } else {
                        authoring::publish_global_collection(
                            &self.home, db, &config, &inputs, now_ms,
                        )?;
                    }
                    Ok(json!({ "deleted": true, "project_id": pid, "filename": filename, "rule_id": rule_id }))
                })?;
                self.emit("schedules", None);
                self.wake_runtime();
                Ok(res)
            }
            "schedule.read_artifact" => {
                let run_id = strp(&p, "schedule_run_id")?;
                let path = strp(&p, "path")?;
                let run: ScheduledRun = self.store.lock().unwrap().db().get(run_id)?;
                let root = self.schedule_run_dir(&self.home, &run);
                let full = self.home.join(path);
                ensure!(
                    full.starts_with(&root)
                        && !Path::new(path)
                            .components()
                            .any(|c| matches!(c, std::path::Component::ParentDir)),
                    "Artifact path is outside this scheduled run"
                );
                let bytes = std::fs::read(&full)
                    .with_context(|| format!("Scheduled artifact not found: {path}"))?;
                Ok(json!({
                    "body": String::from_utf8_lossy(&bytes).into_owned()
                }))
            }
            _ => bail!("Unknown schedule operation: {method}"),
        }
    }
}
