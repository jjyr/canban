//! Local, stable product destinations. Search never starts or resumes an Agent.
use canban_types::*;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Project(String),
    Card(String),
    Note(String),
    Agent(String),
    Terminal(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub kind: &'static str,
    pub target: Target,
    pub fields: Vec<String>,
}
pub fn candidates(s: &Snapshot) -> Vec<Candidate> {
    let project = |id: &str| {
        s.projects
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Unknown Project".into())
    };
    let mut rows = Vec::new();
    for p in &s.projects {
        rows.push(Candidate {
            id: format!("project:{}", p.id),
            title: p.name.clone(),
            detail: format!("Project · {}", p.directory),
            kind: "Project",
            target: Target::Project(p.id.clone()),
            fields: vec![p.name.clone(), p.directory.clone()],
        });
    }
    for c in &s.cards {
        let title = format!("#{} {}", c.number, c.title);
        let owner = project(&c.project_id);
        rows.push(Candidate {
            id: format!("card:{}", c.id),
            title: title.clone(),
            detail: format!(
                "Card · {owner} · {}{}",
                c.stage,
                if c.archived_at.is_some() {
                    " · Archived"
                } else {
                    ""
                }
            ),
            kind: "Card",
            target: Target::Card(c.id.clone()),
            fields: vec![
                title,
                owner,
                c.tags.join(" "),
                c.branch.clone().unwrap_or_default(),
            ],
        });
    }
    for n in &s.notes {
        let owner = project(&n.project_id);
        rows.push(Candidate {
            id: format!("note:{}", n.id),
            title: n.title.clone(),
            detail: format!("Note · {owner}"),
            kind: "Note",
            target: Target::Note(n.id.clone()),
            fields: vec![n.title.clone(), owner],
        });
    }
    for row in canban_notifications::overview(s) {
        let Some(req) = &row.request else {
            continue;
        };
        let title = format!(
            "@{} · #{} {}",
            req.profile.id, row.card.number, row.card.title
        );
        rows.push(Candidate {
            id: format!("agent:{}", row.id),
            title: title.clone(),
            detail: format!(
                "Agent · {} · {} · {}",
                row.project_name, row.card.stage, row.group
            ),
            kind: "Agent",
            target: Target::Agent(row.id),
            fields: vec![
                title,
                row.project_name,
                req.profile.command_name(),
                row.group,
            ],
        });
    }
    for e in crate::terminals::entries(s)
        .into_iter()
        .filter(|e| e.saved.conversation_id.is_none())
    {
        rows.push(Candidate {
            id: format!("terminal:{}", e.saved.id),
            title: e.saved.title(),
            detail: format!("Terminal · {}", e.project_name),
            kind: "Terminal",
            target: Target::Terminal(e.saved.id.clone()),
            fields: vec![
                e.saved.title(),
                e.project_name.clone(),
                e.cwd().unwrap_or_default(),
            ],
        });
    }
    for row in &mut rows {
        row.fields.push(row.kind.into());
    }
    rows
}
/// Multi-term, Unicode-safe subsequence matching. Exact/prefix/contiguous title
/// matches lead, then close subsequences; secondary fields keep a lower weight.
fn score_field(field: &str, term: &str) -> Option<usize> {
    if field == term {
        return Some(0);
    }
    if field.starts_with(term) {
        return Some(
            5 + field
                .chars()
                .count()
                .saturating_sub(term.chars().count())
                .min(20),
        );
    }
    if let Some(at) = field.find(term) {
        return Some(30 + field[..at].chars().count().min(50));
    }
    let mut needle = term.chars();
    let mut wanted = needle.next()?;
    let mut first = None;
    for (i, ch) in field.chars().enumerate() {
        if ch == wanted {
            first.get_or_insert(i);
            if let Some(next) = needle.next() {
                wanted = next;
            } else {
                return Some(
                    90 + first.unwrap_or(0).min(50)
                        + (i - first.unwrap_or(0) + 1 - term.chars().count()).min(200),
                );
            }
        }
    }
    None
}
pub fn ranked(
    rows: &[Candidate],
    query: &str,
    recent: &[String],
    projects_only: bool,
) -> Vec<Candidate> {
    let terms: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut scored = rows
        .iter()
        .filter(|r| !projects_only || r.kind == "Project")
        .filter_map(|row| {
            let mut total = 0;
            for term in &terms {
                let score = row
                    .fields
                    .iter()
                    .enumerate()
                    .filter_map(|(i, f)| {
                        score_field(&f.to_lowercase(), term)
                            .map(|v| v + if i == 0 { 0 } else { 60 })
                    })
                    .min()?;
                total += score;
            }
            let visit = recent
                .iter()
                .position(|id| id == &row.id)
                .unwrap_or(usize::MAX);
            Some((total, visit, row))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.title.to_lowercase().cmp(&b.2.title.to_lowercase()))
            .then(a.2.detail.to_lowercase().cmp(&b.2.detail.to_lowercase()))
            .then(a.2.id.cmp(&b.2.id))
    });
    scored.into_iter().map(|(_, _, r)| r.clone()).collect()
}
#[cfg(test)]
mod tests;
