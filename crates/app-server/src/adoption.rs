//! Optional takeover of an already-running terminal; workflow adoption is independent.
use super::*;

pub(super) struct AdoptTerminal {
    source: TerminalEntry,
    observed: Value,
}

impl AppServer {
    /// Resolve the CLI's inherited Herdr identity before entering the DB transaction.
    pub(super) fn prepare_adopt_terminal(&self, p: &Value) -> Option<AdoptTerminal> {
        let prepare = || -> Result<AdoptTerminal> {
            let address = p["runtime_terminal"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .context("Caller terminal identity is unavailable")?;
            ensure!(
                address.len() < 512 && !address.contains('/'),
                "Caller terminal identity is not a local Core address"
            );
            let config = self.config();
            if let Some(session) = p["runtime_session"].as_str() {
                ensure!(
                    session == config.hc_session,
                    "Caller uses a different Core session"
                );
            }
            let (core, records) = self.runtime_inventory()?;
            let observed = records
                .into_iter()
                .find(|r| r["address"] == address)
                .context("Caller terminal is absent from the Core inventory")?;
            ensure!(observed["state"] == "live", "Caller terminal is not live");
            let candidates: Vec<_> = self
                .store
                .lock()
                .unwrap()
                .db()
                .list::<TerminalEntry>()?
                .into_iter()
                .filter(|t| {
                    if let Some(a) = &t.agent {
                        a.core_id.as_deref() == Some(&core)
                            && a.terminal.as_deref() == Some(address)
                    } else {
                        t.runtime.as_ref().is_some_and(|r| {
                            r.core_id.as_deref() == Some(&core)
                                && r.terminal.as_deref() == Some(address)
                        })
                    }
                })
                .collect();
            ensure!(
                candidates.len() == 1,
                "Caller terminal binding is missing or ambiguous"
            );
            let source = candidates.into_iter().next().unwrap();
            Ok(AdoptTerminal { source, observed })
        };
        match prepare() {
            Ok(value) => Some(value),
            Err(error) => {
                eprintln!("Canban adopt terminal: skipped: {error:#}");
                None
            }
        }
    }
}

/// A savepoint keeps optional takeover failures from partially changing ownership.
pub(super) fn bind(
    db: Db<'_>,
    config: &Config,
    card: &Card,
    p: &Value,
    prepared: &AdoptTerminal,
) -> Option<TerminalEntry> {
    let attempt = || -> Result<TerminalEntry> {
        let source: TerminalEntry = db.get(&prepared.source.id)?;
        ensure!(
            source.runtime.as_ref().map(|r| (&r.core_id, &r.terminal))
                == prepared
                    .source
                    .runtime
                    .as_ref()
                    .map(|r| (&r.core_id, &r.terminal))
                && source
                    .agent
                    .as_ref()
                    .map(|a| (&a.core_id, &a.terminal, a.number))
                    == prepared
                        .source
                        .agent
                        .as_ref()
                        .map(|a| (&a.core_id, &a.terminal, a.number)),
            "Caller terminal changed during adoption"
        );
        let requested = p["profile_id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(a) = &source.agent {
            ensure!(
                a.card_id == card.id
                    && !a.finished()
                    && a.close_state.is_empty()
                    && requested.is_none_or(|id| a.profile.id == id),
                "Caller terminal belongs to another execution"
            );
            return Ok(source);
        }
        ensure!(
            source.conversation_id.is_none(),
            "Caller terminal has another owner"
        );
        ensure!(
            source
                .origin_card_id
                .as_deref()
                .is_none_or(|id| id == card.id),
            "Caller terminal was opened for another Card"
        );
        let runtime = source
            .runtime
            .as_ref()
            .context("Caller runtime disappeared")?;
        ensure!(
            runtime.state == "running",
            "Caller terminal is closing or finished"
        );
        let project: Project = db.get(&card.project_id)?;
        let profile = canban_scheduler::select_in_project(config, Some(&project), requested)?;
        let engine = profile.command_name();
        let observed_engine = canban_profiles::identify_herdr_agent(&prepared.observed);
        ensure!(
            source.agent_engine.as_deref() == Some(engine.as_str())
                || source.agent_engine.as_deref() == profile.preset.as_deref()
                    && profile.preset.is_some()
                || observed_engine == Some(engine.as_str()),
            "Caller Agent does not match the selected Profile"
        );
        let cwd =
            canban_herdr::cwd(&prepared.observed).context("Caller directory is unavailable")?;
        let workspace = card.workspace.clone().unwrap_or_else(|| WorkspaceBinding {
            directory: project.directory.clone(),
            path: card
                .worktree
                .clone()
                .unwrap_or_else(|| project.directory.clone()),
            branch: card.branch.clone(),
            base_commit: card.base_commit.clone(),
            target_branch: project.target_branch.clone(),
        });
        ensure!(
            path_within(&workspace.path, cwd),
            "Caller terminal is outside the Card workspace"
        );
        if let Some(caller_cwd) = p["cwd"].as_str() {
            ensure!(
                path_within(&workspace.path, caller_cwd),
                "Caller directory is outside the Card workspace"
            );
        }
        let context = sessions::conversation(db, card, &profile)?;
        ensure!(
            !db.list::<WorkflowRun>()?
                .iter()
                .any(|w| w.card_id.as_deref() == Some(card.id.as_str())
                    && w.active()
                    && w.state == "waiting_agent"
                    && w.terminal_id.as_deref().is_some_and(|id| id != context.id)),
            "Workflow is waiting for another execution"
        );
        let mut entry: TerminalEntry = db.get(&context.id)?;
        ensure!(
            entry
                .agent
                .as_ref()
                .is_none_or(|a| a.finished() || a.state == "queued" && a.terminal.is_none()),
            "Card/Profile already has another live execution"
        );
        let number = u32::try_from(db.counter(&format!("card-attempt:{}", card.id))?)?;
        let mut run = canban_scheduler::agent_run(
            &card.project_id,
            &card.id,
            number,
            p["stage"].as_str().unwrap_or("develop"),
            ExecutionTrigger::Interactive,
            profile,
            String::new(),
            None,
        );
        run.state = "running".into();
        run.card_number = card.number;
        run.core_id = runtime.core_id.clone();
        run.terminal = runtime.terminal.clone();
        run.cwd = cwd.to_owned();
        run.directory = Some(workspace.directory.clone());
        run.workspace = Some(workspace);
        run.provider_session_id = context.provider_session_id;
        run.observed = Some(canban_herdr::observation(&prepared.observed));
        run.connection_state = "connected".into();
        run.started_at = Some(now());
        run.lifecycle.observed_at = Some(now());
        run.lifecycle.last_interaction_at = Some(now());
        entry.agent = Some(run);
        entry.runtime = None;
        entry.name = source.name.clone();
        entry.number = source.number;
        // Keep the independent entry as ended history, without a competing PTY binding.
        let mut detached = source.clone();
        // Terminal numbers are unique. Transfer the live number to the Card entry.
        detached.number = db.counter("terminal-number")?;
        if let Some(runtime) = &mut detached.runtime {
            runtime.state = "finished".into();
            runtime.terminal = None;
            runtime.agent_status = None;
            runtime.agent_turn = None;
            runtime.observed_agent = None;
            runtime.waiting = None;
        }
        db.save(&detached)?;
        db.save(&entry)?;
        Ok(entry)
    };
    let result = (|| -> Result<TerminalEntry> {
        db.conn.execute_batch("SAVEPOINT adopt_terminal")?;
        match attempt() {
            Ok(entry) => {
                db.conn.execute_batch("RELEASE adopt_terminal")?;
                Ok(entry)
            }
            Err(error) => {
                db.conn
                    .execute_batch("ROLLBACK TO adopt_terminal; RELEASE adopt_terminal")?;
                Err(error)
            }
        }
    })();
    match result {
        Ok(entry) => Some(entry),
        Err(error) => {
            eprintln!("Canban adopt terminal: skipped: {error:#}");
            None
        }
    }
}
