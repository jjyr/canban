use anyhow::{Result, ensure};
use canban_types::{Card, Project, Workflow, id, now};
pub mod github;
mod lifecycle;
pub mod order;
pub use lifecycle::{remove, remove_at};
/// One transactional insertion path for local creation and external intake.
pub fn create(
    db: canban_storage::Db<'_>,
    project: &Project,
    title: &str,
    body: &str,
    actor: &str,
) -> Result<Card> {
    create_at(db, project, title, body, actor, now())
}
/// Pure clock-injected variant of [`create`].
pub fn create_at(
    db: canban_storage::Db<'_>,
    project: &Project,
    title: &str,
    body: &str,
    actor: &str,
    now_ms: u64,
) -> Result<Card> {
    create_inner(db, project, title, body, actor, None, None, now_ms)
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
    create_with_id_at(db, project, title, body, actor, explicit_id, now())
}
/// Pure clock-injected variant of [`create_with_id`].
pub fn create_with_id_at(
    db: canban_storage::Db<'_>,
    project: &Project,
    title: &str,
    body: &str,
    actor: &str,
    explicit_id: Option<&str>,
    now_ms: u64,
) -> Result<Card> {
    create_inner(db, project, title, body, actor, None, explicit_id, now_ms)
}
/// Intake can supply an append rank from its transaction-local Board index.
pub(crate) fn create_ranked_at(
    db: canban_storage::Db<'_>,
    project: &Project,
    title: &str,
    body: &str,
    actor: &str,
    rank: Option<i64>,
    now_ms: u64,
) -> Result<Card> {
    create_inner(db, project, title, body, actor, rank, None, now_ms)
}
#[allow(clippy::too_many_arguments)]
fn create_inner(
    db: canban_storage::Db<'_>,
    project: &Project,
    title: &str,
    body: &str,
    actor: &str,
    rank: Option<i64>,
    explicit_id: Option<&str>,
    now_ms: u64,
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
    let mut c = card_at(project, number, title, body, now_ms)?;
    if let Some(custom_id) = explicit_id {
        c.id = custom_id.to_owned();
    }
    if let Some(rank) = rank {
        c.board_order = Some(rank);
    } else {
        order::place(db, &mut c, None)?;
    }
    db.save(&c)?;
    db.append(canban_timeline::event_at(
        &c.project_id,
        &c.id,
        "card.created",
        actor,
        "Card created",
        serde_json::json!({"title":c.title,"body":c.body}),
        now_ms,
    ))?;
    Ok(c)
}
pub fn project(name: &str, directory: &str, workflow: Workflow) -> Result<Project> {
    project_at(name, directory, workflow, now())
}
/// Pure clock-injected variant of [`project`].
pub fn project_at(name: &str, directory: &str, workflow: Workflow, now_ms: u64) -> Result<Project> {
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
        created_at: now_ms,
    })
}
pub fn card(project: &Project, number: u64, title: &str, body: &str) -> Result<Card> {
    card_at(project, number, title, body, now())
}
/// Pure clock-injected variant of [`card`].
pub fn card_at(
    project: &Project,
    number: u64,
    title: &str,
    body: &str,
    now_ms: u64,
) -> Result<Card> {
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
        created_at: now_ms,
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

    #[test]
    fn clock_injected_project_card_create_remove_and_github_sync_use_2023_epoch() {
        const T0: u64 = 1_700_000_000_000;
        let dir = tempfile::tempdir().unwrap();
        let mut store = canban_storage::Store::open(dir.path()).unwrap();
        let repo = dir.path().join("repo");
        let mut p = project_at("Test", repo.to_str().unwrap(), Workflow::default(), T0).unwrap();
        assert_eq!(p.created_at, T0);
        p.github = Some(canban_types::GitHubSource {
            repository: "org/repo".into(),
            repository_id: 7,
            intake: canban_types::GitHubIntake::All,
            pr_intake: canban_types::GitHubPrIntake::Off,
            include_drafts: false,
            paused: false,
            authors: vec![],
            labels: vec![],
            generation: "gen-1".into(),
            synced_at: None,
            error: None,
        });
        store.write(|db| db.save(&p)).unwrap();

        let mut created = store
            .write(|db| create_at(db, &p, "Local Card", "Body", "user", T0 + 1_000))
            .unwrap();
        assert_eq!(created.created_at, T0 + 1_000);

        store
            .write(|db| remove_at(db, &mut created, T0 + 2_000))
            .unwrap();
        assert_eq!(created.removed_at, Some(T0 + 2_000));

        let issue = canban_github::models::Issue {
            id: 101,
            node_id: "I_101".into(),
            number: 10,
            title: "Issue 10".into(),
            body: Some("Details".into()),
            state: "open".into(),
            html_url: "https://github.com/org/repo/issues/10".into(),
            user: canban_github::models::User {
                id: 1,
                login: "alice".into(),
            },
            labels: vec![],
            updated_at: "2023-11-14T22:13:20Z".into(),
            pull_request: None,
        };
        store
            .write(|db| {
                github::apply_at(
                    db,
                    &p,
                    "gen-1",
                    github::Sync {
                        changes: vec![github::Change {
                            issue,
                            comments: vec![],
                        }],
                    },
                    T0 + 3_000,
                )
            })
            .unwrap();

        let updated_proj: Project = store.db().get(&p.id).unwrap();
        assert_eq!(
            updated_proj.github.as_ref().unwrap().synced_at,
            Some(T0 + 3_000)
        );
        let events: Vec<canban_types::Event> = store.db().list().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].created_at, T0 + 1_000);
        assert_eq!(events[1].created_at, T0 + 2_000);
        assert_eq!(events[2].created_at, T0 + 3_000);
    }
}
