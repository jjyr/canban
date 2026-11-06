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
        state: "queued".into(),
        attempt: 0,
        created_at: now(),
    }
}
pub fn ready(config: &Config, pending: &Request, requests: &[Request]) -> bool {
    let running: Vec<_> = requests
        .iter()
        .filter(|r| matches!(r.state.as_str(), "starting" | "running" | "working"))
        .collect();
    running.len() < config.capacity
        && running
            .iter()
            .filter(|r| r.profile.id == pending.profile.id)
            .count()
            < pending.profile.capacity
}
