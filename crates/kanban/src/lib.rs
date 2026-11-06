use anyhow::{Result, ensure};
use canban_types::{Card, Project, Workflow, id, now};
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
        id: id(),
        name: name.trim().into(),
        directory: std::fs::canonicalize(path)?.to_string_lossy().into_owned(),
        workflow,
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
