use anyhow::Result;
use canban_types::{Config, Profile, Project, Request, id, now};
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
pub fn request(
    project: &str,
    card: &str,
    stage: &str,
    trigger: &str,
    profile: Profile,
    instruction: String,
    on_completed: Option<String>,
) -> Request {
    Request {
        workflow_run_id: None,
        workflow_step: None,
        location: None,
        resume_from: None,
        id: id(),
        project_id: project.into(),
        card_id: card.into(),
        stage: stage.into(),
        trigger: trigger.into(),
        profile,
        instruction,
        on_completed,
        require_human_review: false,
        state: "queued".into(),
        attempt: 0,
        created_at: now(),
    }
}

/// This is an automatic admission budget, not a cap on retained terminals.
/// Verified waits and idle states release slots; unknown launches reserve them.
pub fn active_count(snapshot: &canban_types::Snapshot) -> usize {
    let bound = snapshot
        .attempts
        .iter()
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
            .requests
            .iter()
            .filter(|r| {
                matches!(r.state.as_str(), "starting" | "unknown")
                    && !snapshot
                        .attempts
                        .iter()
                        .any(|a| a.request_id == r.id && a.number == r.attempt)
            })
            .count()
}
pub fn automatic(request: &Request) -> bool {
    matches!(request.trigger.as_str(), "stage_entry" | "workflow")
}
/// A completed, idle invocation may take another assignment in the same environment.
pub fn retained<'a>(
    snapshot: &'a canban_types::Snapshot,
    request: &Request,
) -> Option<&'a canban_types::Attempt> {
    let location = request.location.clone().or_else(|| {
        snapshot
            .projects
            .iter()
            .find(|p| p.id == request.project_id)
            .map(|p| p.work_location())
    });
    snapshot.attempts.iter().rev().find(|a| {
        a.card_id == request.card_id
            && a.terminal.is_some()
            && a.close_state.is_empty()
            && !a.finished()
            && a.location == location
            && a.connection_state == "connected"
            && matches!(a.runtime_status(), "idle" | "done")
            && snapshot.requests.iter().any(|r| {
                r.id == a.request_id
                    && r.state == "completed"
                    && r.profile.id == request.profile.id
                    && r.profile.command() == request.profile.command()
                    && r.profile.resume_command() == request.profile.resume_command()
            })
            && !snapshot
                .questions
                .iter()
                .any(|q| q.request_id == a.request_id && q.state != "replied")
    })
}
pub fn eligible(snapshot: &canban_types::Snapshot, request: &Request) -> bool {
    if request.state != "queued" {
        return false;
    }
    let Some(card) = snapshot.cards.iter().find(|c| c.id == request.card_id) else {
        return false;
    };
    if card.removed_at.is_some()
        || card.archived_at.is_some()
        || (automatic(request)
            && (card.human_gate.is_some()
                || card.attention.is_some()
                || card.stage != request.stage))
    {
        return false;
    }
    if automatic(request)
        && let Some(id) = &request.workflow_run_id
        && !snapshot.workflow_runs.iter().any(|r| {
            r.id == *id
                && r.active()
                && r.request_id.as_deref() == Some(&request.id)
                && r.state == "waiting_agent"
        })
    {
        return false;
    }
    let retained = retained(snapshot, request).map(|a| a.id.as_str());
    !snapshot.attempts.iter().any(|a| {
        a.card_id == request.card_id
            && snapshot
                .requests
                .iter()
                .any(|r| r.id == a.request_id && r.profile.id == request.profile.id)
            && retained != Some(a.id.as_str())
            && ((!a.finished() && a.terminal.is_some()) || a.state == "unknown")
    })
}
pub fn next(snapshot: &canban_types::Snapshot, max_agents: usize) -> Option<Request> {
    if active_count(snapshot) >= max_agents {
        return None;
    }
    snapshot
        .requests
        .iter()
        .filter(|r| eligible(snapshot, r))
        .min_by_key(|r| {
            let depth = snapshot
                .projects
                .iter()
                .find(|p| p.id == r.project_id)
                .and_then(|p| p.workflow.stages.iter().position(|s| s.id == r.stage))
                .unwrap_or(0);
            (std::cmp::Reverse(depth), r.created_at)
        })
        .cloned()
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
            delivery_mode: Default::default(),
            agent_overrides: std::collections::BTreeMap::from([("developer".into(), "agy".into())]),
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
        proj.agent_overrides.clear();
        let fallback = select_in_project(&resolved, Some(&proj), Some("developer")).unwrap();
        assert_eq!(fallback.command()[0], "codex");
    }
}
