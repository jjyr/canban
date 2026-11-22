use anyhow::{Result, ensure};
use canban_types::{Card, Project, Workflow, id, now};
pub mod github;
mod lifecycle;
pub mod order;
pub use lifecycle::remove;
/// One transactional insertion path for local creation and external intake.
pub fn create(
    db: canban_storage::Db<'_>,
    project: &Project,
    title: &str,
    body: &str,
    actor: &str,
) -> Result<Card> {
    create_at_with_id(db, project, title, body, actor, None, None)
}
/// Create a Card with an optional client-supplied identifier (`1..=64` `[A-Za-z0-9_-]`).
pub fn create_with_id(
    db: canban_storage::Db<'_>,
    project: &Project,
    title: &str,
    body: &str,
    actor: &str,
    explicit_id: Option<&str>,
) -> Result<Card> {
    create_at_with_id(db, project, title, body, actor, None, explicit_id)
}
/// Intake can supply an append rank from its transaction-local Board index.
pub(crate) fn create_at(
    db: canban_storage::Db<'_>,
    project: &Project,
    title: &str,
    body: &str,
    actor: &str,
    rank: Option<i64>,
) -> Result<Card> {
    create_at_with_id(db, project, title, body, actor, rank, None)
}
fn create_at_with_id(
    db: canban_storage::Db<'_>,
    project: &Project,
    title: &str,
    body: &str,
    actor: &str,
    rank: Option<i64>,
    explicit_id: Option<&str>,
) -> Result<Card> {
    if let Some(custom_id) = explicit_id {
        ensure!(
            canban_types::is_valid_client_id(custom_id),
            "Invalid Card ID: expected 1..=64 ASCII alphanumeric, '_' or '-' characters"
        );
        ensure!(
            db.get::<Card>(custom_id).is_err(),
            "Card '{custom_id}' already exists"
        );
    }
    let number = db.counter(&format!("cards:{}", project.id))?;
    let mut c = card(project, number, title, body)?;
    if let Some(custom_id) = explicit_id {
        c.id = custom_id.to_owned();
    }
    if let Some(rank) = rank {
        c.board_order = Some(rank);
    } else {
        order::place(db, &mut c, None)?;
    }
    db.save(&c)?;
    db.append(canban_timeline::event(
        &c.project_id,
        &c.id,
        "card.created",
        actor,
        "Card created",
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
        detected_github: None,
        github_disconnected: false,
        delivery_mode: Default::default(),
        agent_overrides: Default::default(),
        tags: Default::default(),
        id: id(),
        name: name.trim().into(),
        directory: std::fs::canonicalize(path)?.to_string_lossy().into_owned(),
        workflow,
        target_branch: None,
        created_at: now(),
    })
}
pub fn card(project: &Project, number: u64, title: &str, body: &str) -> Result<Card> {
    ensure!(!title.trim().is_empty(), "Card title is required");
    Ok(Card {
        schedule_source: None,
        source_trust: Some("local".into()),
        hold_auto_merge: false,
        github_links: Vec::new(),
        created_by: String::new(),
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
        board_order: None,
        title: title.trim().into(),
        body: body.into(),
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
        resolution: None,
        created_at: now(),
    })
}
pub fn check_revision(card: &Card, expected: u64) -> Result<()> {
    ensure!(
        card.revision == expected,
        "revision_conflict: expected revision {expected}, current revision {}; refresh and retry",
        card.revision
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_with_id_accepts_valid_id_and_rejects_invalid_or_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = canban_storage::Store::open(dir.path()).unwrap();
        let repo = dir.path().join("repo");
        let p = project("Test", repo.to_str().unwrap(), Workflow::default()).unwrap();
        store.write(|db| db.save(&p)).unwrap();

        let created = store
            .write(|db| {
                create_with_id(db, &p, "Client ID Card", "Body", "user", Some("abc_12-XYZ"))
            })
            .unwrap();
        assert_eq!(created.id, "abc_12-XYZ");
        assert_eq!(created.number, 1);

        let dup = store
            .write(|db| create_with_id(db, &p, "Duplicate", "", "user", Some("abc_12-XYZ")))
            .unwrap_err();
        assert!(dup.to_string().contains("already exists"));

        for bad in ["", "bad/id", "has space", &"a".repeat(65)] {
            let err = store
                .write(|db| create_with_id(db, &p, "Bad", "", "user", Some(bad)))
                .unwrap_err();
            assert!(err.to_string().contains("Invalid Card ID"), "{bad}: {err}");
        }
    }
}
