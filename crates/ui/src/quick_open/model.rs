//! Local, stable product destinations. Search never starts or resumes an Agent.
use canban_types::*;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Project(String),
    Card(String),
    Agent(String),
    Terminal(String),
    Worktree(String),
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
            fields: vec![title, owner, c.branch.clone().unwrap_or_default()],
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
                row.project_name,
                row.card.stage,
                format_args!(
                    "{} · {}",
                    row.group,
                    crate::terminals::navigation::label(&row)
                )
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
    for wt in &s.worktrees {
        let owner = project(&wt.project_id);
        rows.push(Candidate {
            id: format!("worktree:{}", wt.id),
            title: format!("⎇ {}", wt.branch),
            detail: format!("Worktree · {owner} · base {}", wt.base_branch),
            kind: "Worktree",
            target: Target::Worktree(wt.id.clone()),
            fields: vec![
                wt.branch.clone(),
                owner,
                wt.path.clone(),
                wt.base_branch.clone(),
            ],
        });
    }
    for row in &mut rows {
        row.fields.push(row.kind.into());
    }
    rows
}
fn kind_rank(kind: &str) -> u8 {
    match kind {
        "Project" => 0,
        "Agent" => 1,
        "Card" => 2,
        "Worktree" => 3,
        "Terminal" => 4,
        _ => 5,
    }
}

fn card_number(title: &str) -> Option<(u64, &str)> {
    let rest = title.strip_prefix('#')?;
    let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let number = rest[..digits].parse::<u64>().ok()?;
    Some((number, rest[digits..].trim_start()))
}

pub fn ranked(
    rows: &[Candidate],
    query: &str,
    recent: &[String],
    projects_only: bool,
) -> Vec<Candidate> {
    let empty_query = query.trim().is_empty();
    let mut scored = rows
        .iter()
        .filter(|r| !projects_only || r.kind == "Project")
        .filter_map(|row| {
            let total = canban_ui_kit::search::score(&row.fields, query)?;
            let visit = recent
                .iter()
                .position(|id| id == &row.id)
                .unwrap_or(usize::MAX);
            let archived = row.detail.ends_with(" · Archived");
            Some((total, visit, archived, row))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then_with(|| {
                if empty_query {
                    kind_rank(a.3.kind).cmp(&kind_rank(b.3.kind))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(
                || match (card_number(&a.3.title), card_number(&b.3.title)) {
                    (Some((na, ta)), Some((nb, tb)))
                        if a.3.kind == "Card" && b.3.kind == "Card" =>
                    {
                        if empty_query {
                            nb.cmp(&na)
                                .then_with(|| ta.to_lowercase().cmp(&tb.to_lowercase()))
                        } else {
                            na.cmp(&nb)
                                .then_with(|| ta.to_lowercase().cmp(&tb.to_lowercase()))
                        }
                    }
                    _ => a.3.title.to_lowercase().cmp(&b.3.title.to_lowercase()),
                },
            )
            .then(a.3.detail.to_lowercase().cmp(&b.3.detail.to_lowercase()))
            .then(a.3.id.cmp(&b.3.id))
    });
    scored.into_iter().map(|(_, _, _, r)| r.clone()).collect()
}
#[cfg(test)]
mod tests;
