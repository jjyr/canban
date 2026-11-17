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
        runtime_home: None,
        workspace: None,
        location: None,
        owner_core_id: None,
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
        updated_at: now(),
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
    }
}

/// This is an automatic admission budget, not a cap on retained terminals.
/// Verified waits and idle states release slots; unknown launches reserve them.
pub fn active_count(snapshot: &canban_types::Snapshot) -> usize {
    let bound = snapshot
        .terminals
        .iter()
        .filter_map(|t| t.agent.as_ref())
        .filter(|a| {
            (a.terminal.is_some()
                && !a.finished()
                && (a.close_state == "closing"
                    || !matches!(
                        a.runtime_status(),
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
fn can_hand_off_idle(snapshot: &canban_types::Snapshot, entry: &TerminalEntry) -> bool {
    let Some(agent) = entry.agent.as_ref() else {
        return false;
    };
    agent.pending.is_some()
        && agent.terminal.is_some()
        && agent.close_state.is_empty()
        && !agent.finished()
        && agent.connection_state == "connected"
        && matches!(agent.runtime_status(), "idle" | "done")
        && matches!(agent.state.as_str(), "completed" | "backlog")
        && !snapshot
            .questions
            .iter()
            .any(|q| q.terminal_id.as_deref() == Some(entry.id.as_str()) && q.state != "replied")
}

/// True when `entry` holds a live, completed, idle PTY that can accept a queued `pending`
/// assignment in-place without spawning a new PTY.
pub fn can_continue_in_place(snapshot: &canban_types::Snapshot, entry: &TerminalEntry) -> bool {
    if !can_hand_off_idle(snapshot, entry) {
        return false;
    }
    let agent = entry.agent.as_ref().unwrap();
    let pending = agent.pending.as_ref().unwrap();
    let expected_location = agent.location.clone().or_else(|| {
        snapshot
            .projects
            .iter()
            .find(|p| p.id == agent.project_id)
            .map(|p| p.work_location())
    });
    let expected_env = pending
        .workflow_run_id
        .as_deref()
        .and_then(|id| snapshot.workflow_runs.iter().find(|r| r.id == id))
        .map(|run| run.custom_env_for_step(pending.workflow_step.as_deref()))
        .unwrap_or_default();
    agent.location == expected_location
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
            && (card.merged()
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
        can_hand_off_idle(snapshot, entry) || agent.finished() || agent.terminal.is_none()
    } else {
        (agent.finished() || agent.terminal.is_none()) && agent.state != "unknown"
    }
}
pub fn next(snapshot: &canban_types::Snapshot, max_agents: usize) -> Option<TerminalEntry> {
    if active_count(snapshot) >= max_agents {
        return None;
    }
    snapshot
        .terminals
        .iter()
        .filter(|t| eligible(snapshot, t))
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
    use canban_types::WorkflowStep;
    let card = |id: &str| snapshot.cards.iter().find(|c| c.id == id);
    let queued = snapshot
        .terminals
        .iter()
        .filter(|t| eligible(snapshot, t))
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
    let mut free = max_agents.saturating_sub(active_count(snapshot) + queued);
    // (project, workflow, gate step, column) -> admitted Cards still in that column.
    let mut ready = std::collections::BTreeMap::<(String, String, String, String), u32>::new();
    for run in snapshot.workflow_runs.iter().filter(|r| r.active()) {
        let gate = &run.inputs["ready_gate"];
        let (Some(step), Some(column)) = (gate["step"].as_str(), gate["column"].as_str()) else {
            continue;
        };
        if card(&run.card_id).is_some_and(|c| c.active() && c.stage == column) {
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
            let c = card(&run.card_id)?;
            if !c.active() || c.merged() || c.human_gate.is_some() || c.attention.is_some() {
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
                },
            )]),
            tags: Default::default(),
            id: "p".into(),
            name: "P".into(),
            directory: "/tmp".into(),
            location: None,
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

    #[test]
    fn merge_batches_count_against_active_budget_and_reserve_queued_slots() {
        let mut a1 = agent_run(
            "p1",
            "c1",
            1,
            "develop",
            "auto",
            Profile::default(),
            "go".into(),
            None,
        );
        a1.state = "queued".into();
        let t1 = TerminalEntry {
            id: "t1".into(),
            conversation_id: Some("conv1".into()),
            number: 1,
            name: None,
            origin_project_id: None,
            origin_card_id: None,
            agent_engine: None,
            runtime: None,
            agent: Some(a1),
            created_at: 1,
        };

        let batch = canban_types::MergeBatch {
            id: "merge-queue:p1:abcd1234".into(),
            project_id: "p1".into(),
            profile_id: "merger".into(),
            provider_session_id: None,
            target_branch: "main".into(),
            target_head: "abc".into(),
            strategy: canban_types::MergeStrategy::Rebase,
            sync_upstream: true,
            test_command: None,
            worktree: "/tmp/mq".into(),
            state: "running".into(),
            failure_kind: None,
            conflict_card_number: None,
            conflict_files: vec![],
            test_output: None,
            items: vec![canban_types::MergeBatchItem {
                card_id: "c3".into(),
                card_number: 3,
                workflow_run_id: "r3".into(),
                branch: "canban/card-3".into(),
                worktree: "/tmp/c3".into(),
                base_head: "def".into(),
                integrated_commit: None,
                evicted: false,
            }],
            prompt: "resolve".into(),
            error: None,
            queued_at: None,
            started_at: None,
            created_at: 1,
            updated_at: 1,
        };

        let make_card = |id: &str, number: u64| canban_types::Card {
            schedule_source: None,
            source_trust: None,
            hold_auto_merge: false,
            github_links: vec![],
            created_by: String::new(),
            content: Default::default(),
            removed_at: None,
            pull_request: None,
            pull_request_history: vec![],
            delivery_mode: None,
            workspace: None,
            workspace_history: vec![],
            id: id.into(),
            project_id: "p1".into(),
            number,
            title: format!("Card {number}"),
            body: String::new(),
            board_order: None,
            context_notes: None,
            stage: "develop".into(),
            revision: 1,
            attention: None,
            archived_at: None,
            archive: None,
            archive_error: None,
            worktree: None,
            branch: None,
            base_commit: None,
            human_gate: None,
            delivery: None,
            created_at: 1,
        };

        let mut def = canban_types::WorkflowDefinition {
            id: "dev".into(),
            name: "Dev".into(),
            columns: vec![],
            entries: Default::default(),
            steps: Default::default(),
            env: Default::default(),
            workspace: Default::default(),
            max_steps: 100,
        };
        def.steps.insert(
            "admit".into(),
            canban_types::WorkflowStep::Admit {
                agent_slot: true,
                max_ready: None,
                next: None,
            },
        );
        let wf2 = canban_types::WorkflowRun {
            id: "wf2".into(),
            project_id: "p1".into(),
            card_id: "c2".into(),
            workflow_id: "dev".into(),
            definition_revision: 1,
            definition: def,
            state: "waiting_capacity".into(),
            step: Some("admit".into()),
            visit: 1,
            terminal_id: None,
            trigger_event_id: None,
            input_digest: String::new(),
            inputs: Default::default(),
            outputs: Default::default(),
            history: vec![],
            error: None,
            created_at: 2,
            updated_at: 2,
        };

        let mut snap = canban_types::Snapshot {
            cards: vec![make_card("c1", 1), make_card("c2", 2)],
            terminals: vec![t1.clone()],
            workflow_runs: vec![wf2],
            merge_batches: vec![batch],
            ..Default::default()
        };
        assert_eq!(active_count(&snap), 1);
        assert!(next(&snap, 1).is_none());
        assert_eq!(next(&snap, 2).map(|t| t.id), Some("t1".into()));
        assert!(admissions(&snap, 2).is_empty());
        assert_eq!(admissions(&snap, 3), vec!["wf2".to_string()]);

        // A queued merge batch reserves capacity ahead of ready-gate admissions.
        snap.merge_batches[0].state = "queued".into();
        assert_eq!(active_count(&snap), 0);
        assert!(admissions(&snap, 2).is_empty());
        assert_eq!(admissions(&snap, 3), vec!["wf2".to_string()]);
    }
}
