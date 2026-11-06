use anyhow::{Result, bail, ensure};
use canban_types::{Stage, Workflow};
pub fn validate(flow: &Workflow) -> Result<()> {
    ensure!(!flow.name.trim().is_empty(), "Workflow name is required");
    let mut ids = std::collections::HashSet::new();
    for stage in &flow.stages {
        ensure!(
            !stage.id.is_empty() && ids.insert(&stage.id),
            "Stage IDs must be nonempty and unique"
        );
    }
    let backlog = stage(flow, "backlog")?;
    ensure!(!backlog.execute, "Backlog cannot execute agents");
    for stage in &flow.stages {
        if let Some(next) = &stage.on_completed {
            self::stage(flow, next)?;
            ensure!(
                next != &stage.id,
                "Self-looping completion is not supported"
            );
        }
    }
    // Completion cycles would otherwise create an unbounded automatic workflow.
    for start in &flow.stages {
        let mut seen = std::collections::HashSet::new();
        let mut cursor = Some(start.id.as_str());
        while let Some(s) = cursor {
            ensure!(seen.insert(s), "Completion callbacks contain a cycle");
            cursor = stage(flow, s)?.on_completed.as_deref();
        }
    }
    Ok(())
}
pub fn stage<'a>(flow: &'a Workflow, id: &str) -> Result<&'a Stage> {
    if let Some(s) = flow.stages.iter().find(|s| s.id == id) {
        Ok(s)
    } else {
        bail!("Unknown stage: {id}")
    }
}
