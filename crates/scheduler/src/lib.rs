use anyhow::{Result, bail, ensure};
use canban_types::{Config, Profile, Request, id, now};
pub fn select(config: &Config, tags: &[String], explicit: Option<&str>) -> Result<Profile> {
    if let Some(id) = explicit {
        return config
            .profiles
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown agent @{id}"));
    }
    let matches: Vec<_> = config
        .profiles
        .iter()
        .filter(|p| tags.iter().any(|t| t == &p.id || p.tags.contains(t)))
        .collect();
    if matches.len() > 1 {
        bail!("Multiple agents match the card tags; choose one routing tag");
    }
    if let Some(p) = matches.first() {
        return Ok((*p).clone());
    }
    ensure!(tags.is_empty(), "No agent matches the card tags");
    select(config, &[], Some(&config.default_agent))
}
/// Explicit mentions select a profile directly. Automatic stages with a profile
/// reference keep that role even when the Card has routing tags. Legacy stages
/// without a reference retain tag/default routing.
pub fn select_for_stage(
    config: &Config,
    tags: &[String],
    stage_profile: Option<&str>,
    explicit: Option<&str>,
) -> Result<Profile> {
    select(config, tags, explicit.or(stage_profile))
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
                && !matches!(
                    a.runtime_status(),
                    "closed" | "blocked" | "idle" | "done" | "error"
                ))
                || (a.terminal.is_none() && matches!(a.state.as_str(), "starting" | "unknown"))
        })
        .count();
    bound
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
    request.trigger == "stage_entry"
}
pub fn eligible(snapshot: &canban_types::Snapshot, request: &Request) -> bool {
    if request.state != "queued" {
        return false;
    }
    let Some(card) = snapshot.cards.iter().find(|c| c.id == request.card_id) else {
        return false;
    };
    if card.archived_at.is_some()
        || (automatic(request)
            && (card.human_gate.is_some()
                || card.attention.is_some()
                || card.stage != request.stage))
    {
        return false;
    }
    !snapshot.attempts.iter().any(|a| {
        a.card_id == request.card_id
            && snapshot
                .requests
                .iter()
                .any(|r| r.id == a.request_id && r.profile.id == request.profile.id)
            && ((a.terminal.is_some()
                && a.observed.as_ref().is_none_or(|v| v["state"] != "finished"))
                || a.state == "unknown")
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
