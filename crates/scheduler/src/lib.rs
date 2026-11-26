pub mod timers;
use anyhow::Result;
use canban_types::{
    AgentRun, Config, ExecutionTrigger, Lifecycle, Profile, Project, TerminalEntry, now,
};
/// Resolve an explicit profile ID, otherwise the global default.
pub fn select(config: &Config, explicit: Option<&str>) -> Result<Profile> {
    select_in_project(config, None, explicit)
}
/// Resolve a profile ID in an optional Project context (checking `enabled` and `agent_overrides`).
pub fn select_in_project(
    config: &Config,
    project: Option<&Project>,
    explicit: Option<&str>,
) -> Result<Profile> {
    let id = explicit.unwrap_or(&config.default_agent);
    let profile = config
        .profiles
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| anyhow::anyhow!("Unknown agent @{id}"))?;
    if !profile.enabled {
        anyhow::bail!("Agent @{id} is disabled; enable it in Settings before dispatching");
    }
    Ok(canban_profiles::apply_project_override(
        config, project, profile,
    ))
}
/// Mentions select their profile; automatic stages use their reference or default.
pub fn select_for_stage(
    config: &Config,
    stage_profile: Option<&str>,
    explicit: Option<&str>,
) -> Result<Profile> {
    select_for_stage_in_project(config, None, stage_profile, explicit)
}
pub fn select_for_stage_in_project(
    config: &Config,
    project: Option<&Project>,
    stage_profile: Option<&str>,
    explicit: Option<&str>,
) -> Result<Profile> {
    select_in_project(config, project, explicit.or(stage_profile))
}
#[allow(clippy::too_many_arguments)]
pub fn agent_run(
    project: &str,
    card: &str,
    number: u32,
    stage: &str,
    trigger: impl Into<ExecutionTrigger>,
    profile: Profile,
    instruction: String,
    on_completed: Option<String>,
) -> AgentRun {
    agent_run_at(
        project,
        card,
        number,
        stage,
        trigger,
        profile,
        instruction,
        on_completed,
        now(),
    )
}

/// Pure clock-injected variant of [`agent_run`].
#[allow(clippy::too_many_arguments)]
pub fn agent_run_at(
    project: &str,
    card: &str,
    number: u32,
    stage: &str,
    trigger: impl Into<ExecutionTrigger>,
    profile: Profile,
    instruction: String,
    on_completed: Option<String>,
    now_ms: u64,
) -> AgentRun {
    AgentRun {
        workflow_run_id: None,
        workflow_step: None,
        resume_session: false,
        trigger: trigger.into(),
        stage: stage.into(),
        profile,
        instruction,
        on_completed,
        require_human_review: false,
        workspace: None,
        directory: None,
        lifecycle: Lifecycle::default(),
        project_id: project.into(),
        card_id: card.into(),
        card_number: 0,
        number,
        state: "queued".into(),
        core_id: None,
        terminal: None,
        cwd: String::new(),
        input_path: String::new(),
        output_path: None,
        observed: None,
        error: None,
        started_at: None,
        updated_at: now_ms,
        provider_session_id: None,
        close_state: String::new(),
        turn_ended_at: None,
        response_path: None,
        completed_at: None,
        completion_turn: None,
        close_requested_at: None,
        connection_state: String::new(),
        pending: None,
        applied_env: Default::default(),
        applied_env_fingerprint: String::new(),
    }
}

/// This is an automatic admission budget, not a cap on retained terminals.
/// Verified waits and idle states release slots; unknown launches reserve them.
pub fn active_count(snapshot: &canban_types::Snapshot) -> usize {
    active_count_at(snapshot, canban_types::now())
}

/// Pure clock-injected variant of [`active_count`].
pub fn active_count_at(snapshot: &canban_types::Snapshot, at: u64) -> usize {
    let bound = snapshot
        .terminals
        .iter()
        .filter_map(|t| t.agent.as_ref())
        .filter(|a| {
            (a.terminal.is_some()
                && !a.finished()
                && (a.close_state == "closing"
                    || !matches!(
                        a.runtime_status_at(at),
                        "closed" | "blocked" | "idle" | "done" | "error"
                    )))
                || (a.terminal.is_none() && matches!(a.state.as_str(), "starting" | "unknown"))
        })
        .count();
    bound
        + snapshot
            .configuration_sessions
            .iter()
            .filter(|s| {
                matches!(
                    s.state.as_str(),
                    "starting" | "unknown" | "running" | "working"
                )
            })
            .count()
        + snapshot
            .merge_batches
            .iter()
            .filter(|b| {
                matches!(
                    b.state.as_str(),
                    "starting" | "unknown" | "running" | "working"
                )
            })
            .count()
        + snapshot
            .scheduled_runs
            .iter()
            .filter(|r| {
                !r.creates_card
                    && r.terminal_id.is_some()
                    && matches!(
                        r.state.as_str(),
                        "starting" | "unknown" | "running" | "working"
                    )
            })
            .count()
}
pub fn automatic(agent: &AgentRun) -> bool {
    agent.trigger.is_automatic()
}
/// Pure clock-injected idle hand-off eligibility check.
pub fn can_hand_off_idle_at(
    snapshot: &canban_types::Snapshot,
    entry: &TerminalEntry,
    now_ms: u64,
) -> bool {
    let Some(agent) = entry.agent.as_ref() else {
        return false;
    };
    agent.pending.is_some()
        && agent.terminal.is_some()
        && agent.close_state.is_empty()
        && !agent.finished()
        && agent.connection_state == "connected"
        && matches!(agent.runtime_status_at(now_ms), "idle" | "done")
        && matches!(agent.state.as_str(), "completed" | "backlog")
        && !snapshot
            .questions
            .iter()
            .any(|q| q.terminal_id.as_deref() == Some(entry.id.as_str()) && q.state != "replied")
}

/// True when `entry` holds a live, completed, idle PTY that can accept a queued `pending`
/// assignment in-place without spawning a new PTY.
pub fn can_continue_in_place(snapshot: &canban_types::Snapshot, entry: &TerminalEntry) -> bool {
    can_continue_in_place_at(snapshot, entry, canban_types::now())
}

/// Pure clock-injected variant of [`can_continue_in_place`].
pub fn can_continue_in_place_at(
    snapshot: &canban_types::Snapshot,
    entry: &TerminalEntry,
    now_ms: u64,
) -> bool {
    if !can_hand_off_idle_at(snapshot, entry, now_ms) {
        return false;
    }
    let agent = entry.agent.as_ref().unwrap();
    let pending = agent.pending.as_ref().unwrap();
    let expected_directory = agent.directory.as_deref().or_else(|| {
        snapshot
            .projects
            .iter()
            .find(|p| p.id == agent.project_id)
            .map(|p| p.directory.as_str())
    });
    let expected_env = pending
        .workflow_run_id
        .as_deref()
        .and_then(|id| snapshot.workflow_runs.iter().find(|r| r.id == id))
        .map(|run| run.custom_env_for_step(pending.workflow_step.as_deref()))
        .unwrap_or_default();
    agent.directory.as_deref() == expected_directory
        && agent.profile.id == pending.profile.id
        && agent.profile.command_name() == pending.profile.command_name()
        && agent.applied_env == expected_env
}
fn queued_meta(agent: &AgentRun) -> Option<(&str, &ExecutionTrigger, Option<&str>, u64)> {
    if let Some(p) = &agent.pending {
        Some((
            &p.stage,
            &p.trigger,
            p.workflow_run_id.as_deref(),
            p.queued_at,
        ))
    } else if agent.state == "queued" {
        Some((
            &agent.stage,
            &agent.trigger,
            agent.workflow_run_id.as_deref(),
            agent.updated_at,
        ))
    } else {
        None
    }
}
pub fn eligible(snapshot: &canban_types::Snapshot, entry: &TerminalEntry) -> bool {
    eligible_at(snapshot, entry, canban_types::now())
}

/// Pure clock-injected variant of [`eligible`].
pub fn eligible_at(snapshot: &canban_types::Snapshot, entry: &TerminalEntry, now_ms: u64) -> bool {
    let Some(agent) = entry.agent.as_ref() else {
        return false;
    };
    let Some((stage, trigger, workflow_run_id, _)) = queued_meta(agent) else {
        return false;
    };
    let Some(card) = snapshot.cards.iter().find(|c| c.id == agent.card_id) else {
        return false;
    };
    if card.removed_at.is_some()
        || card.archived_at.is_some()
        || (trigger.is_automatic()
            && (card.resolved()
                || card.human_gate.is_some()
                || card.attention.is_some()
                || card.stage != stage))
    {
        return false;
    }
    if trigger.is_automatic()
        && let Some(id) = workflow_run_id
        && !snapshot.workflow_runs.iter().any(|r| {
            r.id == id
                && r.active()
                && r.terminal_id.as_deref() == Some(entry.id.as_str())
                && r.state == "waiting_agent"
        })
    {
        return false;
    }
    if agent.pending.is_some() {
        can_hand_off_idle_at(snapshot, entry, now_ms)
            || agent.finished()
            || agent.terminal.is_none()
    } else {
        (agent.finished() || agent.terminal.is_none()) && agent.state != "unknown"
    }
}
pub fn next(snapshot: &canban_types::Snapshot, max_agents: usize) -> Option<TerminalEntry> {
    next_at(snapshot, max_agents, canban_types::now())
}

/// Pure clock-injected variant of [`next`].
pub fn next_at(
    snapshot: &canban_types::Snapshot,
    max_agents: usize,
    now_ms: u64,
) -> Option<TerminalEntry> {
    if active_count_at(snapshot, now_ms) >= max_agents {
        return None;
    }
    snapshot
        .terminals
        .iter()
        .filter(|t| eligible_at(snapshot, t, now_ms))
        .min_by_key(|t| {
            let a = t.agent.as_ref().unwrap();
            let (stage, _, _, queued_at) = queued_meta(a).unwrap();
            let depth = snapshot
                .projects
                .iter()
                .find(|p| p.id == a.project_id)
                .and_then(|p| p.workflow.stages.iter().position(|s| s.id == stage))
                .unwrap_or(0);
            (std::cmp::Reverse(depth), queued_at)
        })
        .cloned()
}

/// Workflow runs to release from `admit` gates, in admission order.
pub fn admissions(snapshot: &canban_types::Snapshot, max_agents: usize) -> Vec<String> {
    admissions_at(snapshot, max_agents, canban_types::now())
}

/// Pure clock-injected variant of [`admissions`].
pub fn admissions_at(
    snapshot: &canban_types::Snapshot,
    max_agents: usize,
    now_ms: u64,
) -> Vec<String> {
    use canban_types::WorkflowStep;
    let card = |id: &str| snapshot.cards.iter().find(|c| c.id == id);
    let queued = snapshot
        .terminals
        .iter()
        .filter(|t| eligible_at(snapshot, t, now_ms))
        .count()
        + snapshot
            .merge_batches
            .iter()
            .filter(|b| b.state == "queued")
            .count()
        + snapshot
            .scheduled_runs
            .iter()
            .filter(|r| !r.creates_card && r.state == "queued")
            .count();
    let mut free = max_agents.saturating_sub(active_count_at(snapshot, now_ms) + queued);
    // (project, workflow, gate step, column) -> admitted Cards still in that column.
    let mut ready = std::collections::BTreeMap::<(String, String, String, String), u32>::new();
    for run in snapshot.workflow_runs.iter().filter(|r| r.active()) {
        let gate = &run.inputs["ready_gate"];
        let (Some(step), Some(column)) = (gate["step"].as_str(), gate["column"].as_str()) else {
            continue;
        };
        if run
            .card_id
            .as_deref()
            .and_then(card)
            .is_some_and(|c| c.active() && c.stage == column)
        {
            *ready
                .entry((
                    run.project_id.clone(),
                    run.workflow_id.clone(),
                    step.into(),
                    column.into(),
                ))
                .or_default() += 1;
        }
    }
    let mut waiting: Vec<_> = snapshot
        .workflow_runs
        .iter()
        .filter(|r| r.active() && r.state == "waiting_capacity")
        .filter_map(|run| {
            let c = run.card_id.as_deref().and_then(card)?;
            if !c.active() || c.resolved() || c.human_gate.is_some() || c.attention.is_some() {
                return None;
            }
            let Some(WorkflowStep::Admit {
                agent_slot,
                max_ready,
                ..
            }) = run.step.as_ref().and_then(|k| run.definition.steps.get(k))
            else {
                return None;
            };
            let mut peers: Vec<_> = snapshot
                .cards
                .iter()
                .filter(|p| p.active() && p.project_id == c.project_id && p.stage == c.stage)
                .collect();
            peers.sort_by(|a, b| a.board_cmp(b));
            let position = peers
                .iter()
                .position(|p| p.id == c.id)
                .unwrap_or(usize::MAX);
            Some((position, run.created_at, run, c, *agent_slot, *max_ready))
        })
        .collect();
    waiting.sort_by(|a, b| (a.0, a.1, &a.2.id).cmp(&(b.0, b.1, &b.2.id)));
    let mut admitted = Vec::new();
    for (_, _, run, c, agent_slot, max_ready) in waiting {
        if agent_slot && free == 0 {
            continue;
        }
        let key = (
            run.project_id.clone(),
            run.workflow_id.clone(),
            run.step.clone().unwrap_or_default(),
            c.stage.clone(),
        );
        if let Some(limit) = max_ready
            && ready.get(&key).copied().unwrap_or(0) >= limit
        {
            continue;
        }
        if agent_slot {
            free -= 1;
        }
        if max_ready.is_some() {
            *ready.entry(key).or_default() += 1;
        }
        admitted.push(run.id.clone());
    }
    admitted
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_selection_uses_explicit_then_stage_then_global_default() {
        let config = Config::default();
        assert_eq!(
            select_for_stage(&config, Some("reviewer"), Some("claude"))
                .unwrap()
                .id,
            "claude"
        );
        assert_eq!(
            select_for_stage(&config, Some("reviewer"), None)
                .unwrap()
                .id,
            "reviewer"
        );
        assert_eq!(
            select_for_stage(&config, None, None).unwrap().id,
            config.default_agent
        );
        assert!(select_for_stage(&config, Some("missing"), None).is_err());
        assert!(select_for_stage(&config, Some("reviewer"), Some("missing")).is_err());

        let mut disabled = config.clone();
        disabled
            .profiles
            .iter_mut()
            .find(|p| p.id == "claude")
            .unwrap()
            .enabled = false;
        assert!(select_for_stage(&disabled, None, Some("claude")).is_err());

        let dir = tempfile::tempdir().unwrap();
        let resolved = canban_profiles::resolve(config, dir.path()).unwrap();
        let mut proj = Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: std::collections::BTreeMap::from([(
                "developer".into(),
                canban_types::ProjectAgentOverride {
                    agent: Some("agy".into()),
                    model: Some("gemini-4-carbon-low".into()),
                    effort: Some("low".into()),
                    orchestrator: None,
                },
            )]),
            tags: Default::default(),
            id: "p".into(),
            name: "P".into(),
            directory: "/tmp".into(),
            workflow: Default::default(),
            target_branch: None,
            created_at: 0,
        };
        let selected = select_in_project(&resolved, Some(&proj), Some("developer")).unwrap();
        assert_eq!(selected.id, "developer");
        assert_eq!(selected.command()[0], "agy");
        assert_eq!(selected.model.as_deref(), Some("gemini-4-carbon-low"));
        assert_eq!(selected.effort.as_deref(), Some("low"));
        proj.agent_overrides.clear();
        let fallback = select_in_project(&resolved, Some(&proj), Some("developer")).unwrap();
        assert_eq!(fallback.command()[0], "codex");
    }
}
