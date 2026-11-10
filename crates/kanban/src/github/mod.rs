//! GitHub-to-Card rules and transactional reconciliation; provider IO is separate.
pub mod delivery;
pub mod pull_requests;
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
    let mut issues = if source.intake == GitHubIntake::Off {
        Vec::new()
    } else {
        client.issues(&source.repository)?
    };
    for card in cards
        .iter()
        .filter(|c| c.project_id == project.id && c.removed_at.is_none())
    {
        if let Some(r) = card.github_issue()
            && r.repository_id == source.repository_id
            && !issues.iter().any(|i| i.id == r.issue_id)
        {
            issues.push(client.item(&source.repository, r.number)?);
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
                || (!source.authors.is_empty()
                    && !source
                        .authors
                        .iter()
                        .any(|a| a.eq_ignore_ascii_case(&issue.user.login)))
                || (!source.labels.is_empty()
                    && !source
                        .labels
                        .iter()
                        .any(|l| issue.labels.iter().any(|v| &v.name == l)))
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
        changes.push(Change {
            issue,
            comments: Vec::new(),
        });
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
    let i = change.issue;
    let is_pr = i.pull_request.is_some();
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
    if existing.is_none()
        && is_pr
        && let Some(card) = db.list::<Card>()?.into_iter().find(|c| {
            c.project_id == project.id
                && c.pull_request.as_ref().is_some_and(|pr| {
                    pr.repository_id == source.repository_id && pr.number == source.number
                })
        })
    {
        return Ok(card);
    }
    let card = if let Some(mut c) = existing {
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
            &format!("{}#{}", source.repository, source.number),
            instructions,
            if manual { "user" } else { "github" },
        )?;
        c.content = if is_pr {
            CardContent::GitHubPr {
                source: source.clone(),
            }
        } else {
            CardContent::GitHubIssue {
                source: source.clone(),
            }
        };
        c
    };
    // Source data belongs to the disposable provider cache. Persist only workflow ownership.
    db.save(&card)?;
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
