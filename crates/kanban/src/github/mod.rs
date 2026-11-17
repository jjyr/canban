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
use std::collections::{BTreeMap, HashSet};
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
    let mut seen: HashSet<_> = issues.iter().map(|i| i.id).collect();
    let existing: BTreeMap<_, _> = cards
        .iter()
        .filter(|c| c.project_id == project.id)
        .filter_map(|c| {
            c.github_issue()
                .filter(|r| r.repository_id == source.repository_id)
                .map(|r| (r.issue_id, c))
        })
        .collect();
    let cached: BTreeMap<_, _> = cache
        .iter()
        .filter(|c| c.project_id == project.id && c.source.repository_id == source.repository_id)
        .map(|c| (c.source.issue_id, c.updated_at.as_str()))
        .collect();
    for card in cards
        .iter()
        .filter(|c| c.project_id == project.id && c.removed_at.is_none())
    {
        if let Some(r) = card.github_issue()
            && r.repository_id == source.repository_id
            && seen.insert(r.issue_id)
        {
            issues.push(client.item(&source.repository, r.number)?);
        }
    }
    let mut changes = Vec::new();
    for issue in issues {
        let existing = existing.get(&issue.id).copied();
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
        if cached
            .get(&issue.id)
            .is_some_and(|updated| *updated == issue.updated_at)
        {
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
    apply_batch(db, project, expected, sync, true)
}
/// Partial worker batches leave last-success unchanged until the complete sync finishes.
pub fn apply_batch(
    db: Db<'_>,
    project: &Project,
    expected: &str,
    sync: Sync,
    finish: bool,
) -> Result<()> {
    let current: Project = db.get(&project.id)?;
    ensure!(
        current
            .github
            .as_ref()
            .is_some_and(|s| s.generation == expected),
        "GitHub source changed; discarded stale synchronization"
    );
    let mut intake = Intake::load(db, &current)?;
    for c in sync.changes {
        let key = (current.github.as_ref().unwrap().repository_id, c.issue.id);
        if !intake
            .sources
            .get(&key)
            .is_some_and(|c| c.removed_at.is_some())
        {
            intake.upsert(db, &current, c, false, "")?;
        }
    }
    if finish {
        let mut current = current;
        let source = current.github.as_mut().unwrap();
        source.synced_at = Some(now());
        source.error = None;
        db.save(&current)?;
    }
    Ok(())
}
pub fn upsert(
    db: Db<'_>,
    project: &Project,
    change: Change,
    manual: bool,
    instructions: &str,
) -> Result<Card> {
    Intake::load(db, project)?.upsert(db, project, change, manual, instructions)
}
/// One project-scoped read per transaction, shared by source deduplication,
/// delivery exclusion and append ordering. Rebuilt after each bounded batch so
/// concurrent local removals/rule changes are respected by the next batch.
struct Intake {
    sources: BTreeMap<(u64, u64), Card>,
    deliveries: BTreeMap<(u64, u64), Card>,
    tail: Option<Card>,
}
impl Intake {
    fn load(db: Db<'_>, project: &Project) -> Result<Self> {
        let mut query = db
            .conn
            .prepare("SELECT data FROM cards WHERE project_id=?1 ORDER BY rowid")?;
        let mut result = Self {
            sources: BTreeMap::new(),
            deliveries: BTreeMap::new(),
            tail: None,
        };
        let stage = "backlog";
        for row in query.query_map([&project.id], |r| r.get::<_, String>(0))? {
            let c: Card = serde_json::from_str(&row?)?;
            if c.active()
                && c.stage == stage
                && result
                    .tail
                    .as_ref()
                    .is_none_or(|tail| tail.board_cmp(&c).is_lt())
            {
                result.tail = Some(c.clone());
            }
            if let Some(source) = c.github_issue() {
                result
                    .sources
                    .insert((source.repository_id, source.issue_id), c.clone());
            }
            if let Some(pr) = &c.pull_request {
                result
                    .deliveries
                    .entry((pr.repository_id, pr.number))
                    .or_insert(c);
            }
        }
        Ok(result)
    }
    fn upsert(
        &mut self,
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
        let existing = self.sources.get(&(source.repository_id, i.id)).cloned();
        if existing.is_none()
            && is_pr
            && let Some(card) = self.deliveries.get(&(source.repository_id, source.number))
        {
            return Ok(card.clone());
        }
        let created = existing.is_none();
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
            let mut c = crate::create_at(
                db,
                project,
                &format!("{}#{}", source.repository, source.number),
                instructions,
                if manual { "user" } else { "github" },
                self.tail
                    .as_ref()
                    .map_or(Some(0), |tail| tail.board_rank().checked_add(1024)),
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
            c.source_trust = Some(if is_pr {
                if matches!(
                    config.pr_intake,
                    GitHubPrIntake::Collaborators | GitHubPrIntake::Authored
                ) {
                    "collaborator".into()
                } else {
                    "external_pr".into()
                }
            } else {
                "collaborator".into()
            });
            c
        };
        // Finalize only this transaction's creation, never rewrite committed history on refresh.
        if created {
            db.conn.execute("UPDATE events SET data=json_set(data,'$.data.source_kind',?2,'$.data.reason',?3,'$.data.initial_sync',json(?4)) WHERE project_id=?5 AND card_id=?1 AND json_extract(data,'$.kind')='card.created'",[card.id.as_str(),if is_pr {"github.pr"}else{"github.issue"},if manual {"manual"}else{"automatic_intake"},if !manual && config.synced_at.is_none(){"true"}else{"false"},project.id.as_str()])?;
        }
        // Source data belongs to the disposable provider cache. Persist only workflow ownership.
        db.save(&card)?;
        if created
            && self
                .tail
                .as_ref()
                .is_none_or(|tail| tail.board_cmp(&card).is_lt())
        {
            self.tail = Some(card.clone());
        }
        self.sources
            .insert((source.repository_id, source.issue_id), card.clone());
        Ok(card)
    }
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
        data,
    ))?;
    Ok(())
}
pub fn remove(db: Db<'_>, card: &mut Card) -> Result<()> {
    super::remove(db, card)
}

#[cfg(test)]
mod tests;
