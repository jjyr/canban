use super::*;
use canban_github::models::{Label, User};
use canban_storage::Store;
fn fixture() -> (tempfile::TempDir, Store, Project) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let mut project =
        crate::project("test", dir.path().to_str().unwrap(), Workflow::default()).unwrap();
    project.github = Some(GitHubSource {
        repository: "owner/repo".into(),
        repository_id: 1,
        generation: "generation-a".into(),
        intake: GitHubIntake::All,
        synced_at: None,
        error: None,
    });
    store.write(|db| db.save(&project)).unwrap();
    (dir, store, project)
}
fn change(number: u64) -> Change {
    Change {
        issue: Issue {
            id: number + 100,
            node_id: format!("I_{number}"),
            number,
            title: format!("Issue {number}"),
            body: Some("Remote task description".into()),
            state: "open".into(),
            html_url: format!("https://github.com/owner/repo/issues/{number}"),
            user: User {
                id: 7,
                login: "author".into(),
            },
            labels: vec![Label {
                name: "codex".into(),
            }],
            updated_at: "2026-11-09T00:00:00Z".into(),
            pull_request: None,
        },
        comments: vec![Comment {
            id: number,
            body: "@codex remote text must never run an Agent".into(),
            user: User {
                id: 7,
                login: "author".into(),
            },
            html_url: "https://github.com/owner/repo/issues/1#issuecomment-1".into(),
            updated_at: "2026-11-09T00:00:00Z".into(),
        }],
    }
}
#[test]
fn intake_uses_local_numbering_without_dispatch_or_routing_labels() {
    let (_dir, mut store, p) = fixture();
    store
        .write(|db| {
            crate::create(db, &p, "Local", "local body", "user")?;
            apply(
                db,
                &p,
                "generation-a",
                Sync {
                    changes: vec![change(80)],
                },
            )
        })
        .unwrap();
    let snapshot = store.snapshot().unwrap();
    let c = snapshot
        .cards
        .iter()
        .find(|c| c.github_issue().is_some())
        .unwrap();
    assert_eq!(c.number, 2);
    assert_eq!(c.github_issue().unwrap().number, 80);
    assert_eq!(c.stage, "backlog");
    assert!(snapshot.requests.is_empty());
    assert!(snapshot.conversations.is_empty());
    assert!(snapshot.events.iter().any(|e| e.kind == "github.comment"));
}
#[test]
fn repeated_sync_and_manual_add_preserve_identity_and_deduplicate_events() {
    let (_dir, mut store, p) = fixture();
    let first = store
        .write(|db| upsert(db, &p, change(1), true, "Local requirements"))
        .unwrap();
    let second = store
        .write(|db| {
            upsert(
                db,
                &p,
                change(1),
                true,
                "Do not overwrite existing instructions",
            )
        })
        .unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(second.body, "Local requirements");
    assert_eq!(store.snapshot().unwrap().cards.len(), 1);
    assert_eq!(
        store
            .db()
            .list::<Event>()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "github.comment")
            .count(),
        1
    );
    let mut duplicate = second.clone();
    duplicate.id = id();
    duplicate.number = 90;
    assert!(
        store.write(|db| db.save(&duplicate)).is_err(),
        "source uniqueness must also hold at the database boundary"
    );
}
#[test]
fn local_removal_suppresses_sync_and_explicit_readd_restores_history() {
    let (_dir, mut store, p) = fixture();
    let mut c = store
        .write(|db| upsert(db, &p, change(1), true, "Keep context"))
        .unwrap();
    store.write(|db| remove(db, &mut c)).unwrap();
    store
        .write(|db| {
            apply(
                db,
                &p,
                "generation-a",
                Sync {
                    changes: vec![change(1)],
                },
            )
        })
        .unwrap();
    assert!(store.snapshot().unwrap().cards.is_empty());
    let restored = store
        .write(|db| upsert(db, &p, change(1), true, ""))
        .unwrap();
    assert_eq!(restored.id, c.id);
    assert_eq!(restored.number, c.number);
    assert_eq!(restored.body, "Keep context");
    assert_eq!(store.snapshot().unwrap().cards.len(), 1);
    assert!(
        store
            .db()
            .list::<Event>()
            .unwrap()
            .iter()
            .any(|e| e.kind == "card.removed")
    );
}
#[test]
fn stale_source_sync_is_discarded_without_partial_import() {
    let (_dir, mut store, p) = fixture();
    let result = store.write(|db| {
        apply(
            db,
            &p,
            "old-generation",
            Sync {
                changes: vec![change(1), change(2)],
            },
        )
    });
    assert!(result.is_err());
    assert!(store.snapshot().unwrap().cards.is_empty());
    assert!(store.db().list::<GitHubIssueSnapshot>().unwrap().is_empty());
}
#[test]
fn remote_closure_keeps_local_stage_instructions_and_history() {
    let (_dir, mut store, p) = fixture();
    let mut c = store
        .write(|db| upsert(db, &p, change(1), true, "Local requirements"))
        .unwrap();
    c.stage = "review".into();
    store.write(|db| db.save(&c)).unwrap();
    let mut update = change(1);
    update.issue.state = "closed".into();
    update.issue.title = "Updated remotely".into();
    update.issue.updated_at = "2026-11-09T01:00:00Z".into();
    let updated = store.write(|db| upsert(db, &p, update, false, "")).unwrap();
    assert_eq!(updated.stage, "review");
    assert_eq!(updated.body, "Local requirements");
    assert_eq!(updated.title, "Updated remotely");
    assert!(updated.archived_at.is_none());
}
#[test]
fn rejects_pull_request_as_source() {
    let (_dir, mut store, p) = fixture();
    let mut c = change(1);
    c.issue.pull_request = Some(json!({"url":"https://api.github.com/"}));
    assert!(store.write(|db| upsert(db, &p, c, true, "")).is_err());
    assert!(store.snapshot().unwrap().cards.is_empty());
}
#[test]
fn older_overlapping_sync_cannot_overwrite_newer_issue_content() {
    let (_dir, mut store, p) = fixture();
    let mut newer = change(1);
    newer.issue.title = "Newer content".into();
    newer.issue.updated_at = "2026-11-09T02:00:00Z".into();
    let card = store.write(|db| upsert(db, &p, newer, false, "")).unwrap();
    let retained = store
        .write(|db| upsert(db, &p, change(1), false, ""))
        .unwrap();
    assert_eq!(retained.title, "Newer content");
    assert_eq!(retained.revision, card.revision);
    assert_eq!(
        store
            .db()
            .get::<GitHubIssueSnapshot>(&cache_id(&p.id, 101))
            .unwrap()
            .title,
        "Newer content"
    );
}
