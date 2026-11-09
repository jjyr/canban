//! GitHub-to-Card rules and transactional reconciliation; provider IO is separate.
pub mod delivery;
use anyhow::{Result, ensure};
use canban_github::{
    Client,
    models::{Comment, Issue},
};
use canban_storage::Db;
use canban_types::*;
use serde_json::json;
use std::collections::HashSet;
pub struct Change {
    pub issue: Issue,
    pub comments: Vec<Comment>,
}
pub struct Sync {
    pub changes: Vec<Change>,
}
pub fn cache_id(project: &str, issue: u64) -> String {
    format!("{project}:{issue}")
}
pub fn fetch(
    client: &Client,
    project: &Project,
    cards: &[Card],
    cache: &[GitHubIssueSnapshot],
) -> Result<Sync> {
    let source = project
        .github
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No GitHub source"))?;
    let authors = if source.intake == GitHubIntake::Collaborators {
        Some(
            client
                .collaborators(&source.repository)?
                .into_iter()
                .map(|u| u.id)
                .collect::<HashSet<_>>(),
        )
    } else {
        None
    };
    let mut issues = client.issues(&source.repository)?;
    for card in cards
        .iter()
        .filter(|c| c.project_id == project.id && c.removed_at.is_none())
    {
        if let Some(r) = card.github_issue()
            && r.repository_id == source.repository_id
            && !issues.iter().any(|i| i.id == r.issue_id)
        {
            issues.push(client.issue(&source.repository, r.number)?);
        }
    }
    let mut changes = Vec::new();
    for issue in issues {
        let existing = cards.iter().find(|c| {
            c.project_id == project.id
                && c.github_issue().is_some_and(|s| {
                    s.issue_id == issue.id && s.repository_id == source.repository_id
                })
        });
        if existing.is_some_and(|c| c.removed_at.is_some()) {
            continue;
        }
        if existing.is_none()
            && (issue.state != "open"
                || authors
                    .as_ref()
                    .is_some_and(|a| !a.contains(&issue.user.id)))
        {
            continue;
        }
        if cache.iter().any(|c| {
            c.project_id == project.id
                && c.source.issue_id == issue.id
                && c.updated_at == issue.updated_at
        }) {
            continue;
        }
        let comments = client.comments(&source.repository, issue.number)?;
        changes.push(Change { issue, comments });
    }
    Ok(Sync { changes })
}
pub fn apply(db: Db<'_>, project: &Project, expected: &str, sync: Sync) -> Result<()> {
    let current: Project = db.get(&project.id)?;
    ensure!(
        current
            .github
            .as_ref()
            .is_some_and(|s| s.generation == expected),
        "GitHub source changed; discarded stale synchronization"
    );
    for c in sync.changes {
        let removed = db.list::<Card>()?.iter().any(|card| {
            card.project_id == project.id
                && card.removed_at.is_some()
                && card.github_issue().is_some_and(|s| {
                    s.issue_id == c.issue.id
                        && s.repository_id == project.github.as_ref().unwrap().repository_id
                })
        });
        if !removed {
            upsert(db, &current, c, false, "")?;
        }
    }
    let mut current = current;
    let source = current.github.as_mut().unwrap();
    source.synced_at = Some(now());
    source.error = None;
    db.save(&current)?;
    Ok(())
}
pub fn upsert(
    db: Db<'_>,
    project: &Project,
    change: Change,
    manual: bool,
    instructions: &str,
) -> Result<Card> {
    let config = project
        .github
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Connect this Project to GitHub first"))?;
    ensure!(
        change.issue.pull_request.is_none(),
        "Select an Issue, not a pull request"
    );
    let i = change.issue;
    let source = GitHubIssueRef {
        repository: config.repository.clone(),
        repository_id: config.repository_id,
        issue_id: i.id,
        number: i.number,
        url: i.html_url.clone(),
    };
    let existing = db.list::<Card>()?.into_iter().find(|c| {
        c.project_id == project.id
            && c.github_issue()
                .is_some_and(|s| s.issue_id == i.id && s.repository_id == source.repository_id)
    });
    let mut card = if let Some(mut c) = existing {
        if c.removed_at.is_some() {
            ensure!(manual, "Issue was removed locally");
            c.removed_at = None;
            c.revision += 1;
            event(
                db,
                &c,
                "card.restored",
                "user",
                "Issue restored to Board",
                json!({}),
            )?;
        }
        c
    } else {
        let mut c = crate::create(
            db,
            project,
            &i.title,
            instructions,
            if manual { "user" } else { "github" },
        )?;
        c.content = CardContent::GitHubIssue {
            source: source.clone(),
        };
        c
    };
    let snapshot = GitHubIssueSnapshot {
        id: cache_id(&project.id, i.id),
        project_id: project.id.clone(),
        source,
        title: i.title.clone(),
        body: i.body.unwrap_or_default(),
        state: i.state,
        author_id: i.user.id,
        author_login: i.user.login,
        labels: i.labels.into_iter().map(|l| l.name).collect(),
        updated_at: i.updated_at,
        synced_at: now(),
    };
    let old = db.get::<GitHubIssueSnapshot>(&snapshot.id).ok();
    if old
        .as_ref()
        .is_some_and(|old| old.updated_at > snapshot.updated_at)
    {
        // A slower overlapping sync must not replace a newer source snapshot.
        db.save(&card)?;
        return Ok(card);
    }
    if old
        .as_ref()
        .is_none_or(|o| o.updated_at != snapshot.updated_at)
    {
        card.title = i.title;
        card.revision += 1;
        event(
            db,
            &card,
            "github.issue_synced",
            "github",
            &format!("Issue #{} · {}", i.number, snapshot.state),
            json!({"source":snapshot.source,"state":snapshot.state,"updated_at":snapshot.updated_at}),
        )?;
    }
    db.save(&snapshot)?;
    db.save(&card)?;
    let known = db.list::<Event>()?;
    for comment in change.comments {
        let key = format!("{}:{}", comment.id, comment.updated_at);
        if known.iter().any(|e| {
            e.card_id == card.id && e.kind == "github.comment" && e.data["external_revision"] == key
        }) {
            continue;
        }
        event(
            db,
            &card,
            "github.comment",
            &format!("github:{}", comment.user.login),
            &comment.body,
            json!({"external_revision":key,"comment_id":comment.id,"url":comment.html_url}),
        )?;
    }
    Ok(card)
}
fn event(
    db: Db<'_>,
    c: &Card,
    kind: &str,
    actor: &str,
    text: &str,
    data: serde_json::Value,
) -> Result<()> {
    db.append(canban_timeline::event(
        &c.project_id,
        &c.id,
        kind,
        actor,
        text,
        None,
        data,
    ))?;
    Ok(())
}
pub fn remove(db: Db<'_>, card: &mut Card) -> Result<()> {
    super::remove(db, card)
}

#[cfg(test)]
mod tests;
