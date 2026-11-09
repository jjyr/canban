use anyhow::{Result, ensure};
use canban_types::{Card, Project, Workflow, id, now};
pub mod github;
mod lifecycle;
pub use lifecycle::remove;
/// One transactional insertion path for local creation and external intake.
pub fn create(
    db: canban_storage::Db<'_>,
    project: &Project,
    title: &str,
    body: &str,
    tags: Vec<String>,
    actor: &str,
) -> Result<Card> {
    let number = db.counter(&format!("cards:{}", project.id))?;
    let c = card(project, number, title, body, tags)?;
    db.save(&c)?;
    db.append(canban_timeline::event(
        &c.project_id,
        &c.id,
        "card.created",
        actor,
        "Card created",
        None,
        serde_json::json!({"title":c.title,"body":c.body}),
    ))?;
    Ok(c)
}
pub fn project(name: &str, directory: &str, workflow: Workflow) -> Result<Project> {
    ensure!(!name.trim().is_empty(), "Project name is required");
    ensure!(
        !directory.trim().is_empty(),
        "Project directory is required"
    );
    let path = std::path::Path::new(directory);
    ensure!(path.is_absolute(), "Project directory must be absolute");
    std::fs::create_dir_all(path)?;
    Ok(Project {
        github: None,
        delivery_mode: Default::default(),
        location: None,
        id: id(),
        name: name.trim().into(),
        directory: std::fs::canonicalize(path)?.to_string_lossy().into_owned(),
        workflow,
        target_branch: None,
        created_at: now(),
    })
}
pub fn card(
    project: &Project,
    number: u64,
    title: &str,
    body: &str,
    tags: Vec<String>,
) -> Result<Card> {
    ensure!(!title.trim().is_empty(), "Card title is required");
    Ok(Card {
        content: Default::default(),
        removed_at: None,
        pull_request: None,
        pull_request_history: Vec::new(),
        delivery_mode: None,
        workspace: None,
        workspace_history: Vec::new(),
        id: id(),
        project_id: project.id.clone(),
        number,
        title: title.trim().into(),
        body: body.into(),
        tags,
        context_notes: None,
        stage: "backlog".into(),
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
        created_at: now(),
    })
}
pub fn check_revision(card: &Card, expected: u64) -> Result<()> {
    ensure!(
        card.revision == expected,
        "revision_conflict: card changed; refresh and retry"
    );
    Ok(())
}
