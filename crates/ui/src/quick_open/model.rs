//! Local, stable product destinations. Search never starts or resumes an Agent.
use canban_types::*;
use std::collections::BTreeSet;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Project(String),
    Card(String),
    Agent(String),
    Terminal(String),
    Worktree(String),
    Schedule(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub kind: &'static str,
    pub target: Target,
    pub fields: Vec<String>,
    pub tags: Vec<String>,
}
pub(crate) fn github_search_fields(card: &Card) -> Vec<String> {
    let mut fields = Vec::new();
    let mut push_ref = |kind: &str, repository: &str, num: u64, include_gh: bool| {
        if num == 0 {
            return;
        }
        fields.push(format!("{kind} #{num}"));
        fields.push(format!("{kind}#{num}"));
        fields.push(format!("{kind}:{num}"));
        if include_gh {
            fields.push(format!("gh#{num}"));
            fields.push(format!("gh:{num}"));
        }
        fields.push(format!("#{num}"));
        fields.push(num.to_string());
        if !repository.is_empty() {
            fields.push(format!("{repository}#{num}"));
        }
    };
    if let Some(src) = card.github_issue() {
        let kind = if card.is_github_pr() { "pr" } else { "issue" };
        push_ref(kind, &src.repository, src.number, true);
    }
    if let Some(pr) = card.pull_request.as_ref().filter(|p| p.number > 0) {
        push_ref("pr", &pr.repository, pr.number, true);
    }
    for link in &card.github_links {
        if let Some(num) = link.number.filter(|&n| n > 0) {
            let kind = if link.kind == "pr" { "pr" } else { "issue" };
            push_ref(kind, &link.repository, num, false);
        }
    }
    fields
}

pub fn candidates(s: &Snapshot) -> Vec<Candidate> {
    candidates_with_tags(s, &BTreeSet::new())
}
pub fn candidates_with_tags(s: &Snapshot, local_tags: &BTreeSet<String>) -> Vec<Candidate> {
    let known_tags = crate::tags::known_map(s, local_tags, None);
    let project = |id: &str| {
        let base = s
            .projects
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Unknown Project".into());
        let hid = s.project_host(id);
        if hid != "local" && s.has_remote_hosts() {
            format!("{base} · {}", s.host_display_name(hid))
        } else {
            base
        }
    };
    let mut rows = Vec::new();
    for p in &s.projects {
        let hid = s.project_host(&p.id);
        let host_suffix = if hid != "local" && s.has_remote_hosts() {
            if s.is_host_offline(hid) {
                format!(" · {} (offline)", s.host_display_name(hid))
            } else {
                format!(" · {}", s.host_display_name(hid))
            }
        } else {
            String::new()
        };
        rows.push(Candidate {
            id: format!("project:{}", p.id),
            title: p.name.clone(),
            detail: format!("Project{host_suffix} · {}", p.directory),
            kind: "Project",
            target: Target::Project(p.id.clone()),
            fields: vec![
                p.name.clone(),
                p.directory.clone(),
                s.host_display_name(hid).to_owned(),
            ],
            tags: Vec::new(),
        });
    }
    for c in s.cards.iter().filter(|c| c.active() && !c.merged()) {
        let title = format!("#{} {}", c.number, c.title);
        let owner = project(&c.project_id);
        let tags = crate::tags::card_tags(c, &known_tags);
        let mut fields = vec![
            title.clone(),
            owner.clone(),
            c.branch.clone().unwrap_or_default(),
        ];
        fields.extend(github_search_fields(c));
        for tag in &tags {
            fields.push(format!("#{tag}"));
            fields.push(tag.clone());
        }
        let gh_detail = if let Some(src) = c.github_issue() {
            format!(
                " · {} #{}",
                if c.is_github_pr() { "PR" } else { "Issue" },
                src.number
            )
        } else if let Some(pr) = c.pull_request.as_ref().filter(|p| p.number > 0) {
            format!(" · PR #{}", pr.number)
        } else {
            String::new()
        };
        rows.push(Candidate {
            id: format!("card:{}", c.id),
            title,
            detail: format!(
                "Card · {owner} · {}{}{gh_detail}",
                c.stage,
                crate::tags::format_summary(&tags, crate::tags::MAX_BOARD_TILE_TAGS),
            ),
            kind: "Card",
            target: Target::Card(c.id.clone()),
            fields,
            tags,
        });
    }
    for row in canban_notifications::overview(s)
        .into_iter()
        .filter(|row| row.card.active() && !row.card.merged())
    {
        let Some((profile_id, command_name)) = row
            .agent()
            .map(|att| (att.profile.id.clone(), att.profile.command_name()))
        else {
            continue;
        };
        let title = format!("@{} · #{} {}", profile_id, row.card.number, row.card.title);
        let detail = format!(
            "Agent · {} · {} · {}",
            row.project_name,
            row.card.stage,
            format_args!(
                "{} · {}",
                row.group,
                crate::terminals::navigation::label(&row)
            )
        );
        rows.push(Candidate {
            id: format!("agent:{}", row.id),
            title: title.clone(),
            detail,
            kind: "Agent",
            target: Target::Agent(row.id),
            fields: vec![title, row.project_name, command_name, row.group],
            tags: Vec::new(),
        });
    }
    for e in crate::terminals::entries(s).into_iter().filter(|e| {
        e.saved.conversation_id.is_none()
            && !e.ended()
            && e.card.as_ref().is_none_or(|c| c.active() && !c.merged())
    }) {
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
            tags: Vec::new(),
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
            tags: Vec::new(),
        });
    }
    for st in &s.schedule_states {
        let scope_label = if is_global_project_id(&st.project_id) {
            "Global · Host".to_owned()
        } else {
            project(&st.project_id)
        };
        let title = if st.rule_id == "schedule" {
            st.workflow_name.clone()
        } else {
            format!("{} / {}", st.workflow_name, st.rule_id)
        };
        let mode_label = if st.creates_card {
            "Card"
        } else if st.starts_with_script {
            if st.profile_id.is_some() {
                "Script → Agent"
            } else {
                "Script"
            }
        } else {
            "Agent"
        };
        let state_label = if s
            .scheduled_runs
            .iter()
            .any(|r| r.schedule_id == st.id && r.state == "blocked")
        {
            "Needs you"
        } else if st.enabled {
            "Active"
        } else {
            "Paused"
        };
        rows.push(Candidate {
            id: format!("schedule:{}", st.id),
            title: title.clone(),
            detail: format!(
                "Schedule · {scope_label} · {} · {mode_label} · {state_label}",
                st.cron
            ),
            kind: "Schedule",
            target: Target::Schedule(st.id.clone()),
            fields: vec![
                title,
                scope_label,
                st.cron.clone(),
                st.filename.clone(),
                mode_label.into(),
                state_label.into(),
                "cron".into(),
            ],
            tags: Vec::new(),
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
        "Schedule" => 4,
        "Terminal" => 5,
        _ => 6,
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
    let (tag_terms, other_query) = crate::tags::split_tag_query(query);
    let mut scored = rows
        .iter()
        .filter(|r| !projects_only || r.kind == "Project")
        .filter_map(|row| {
            let total = if tag_terms.is_empty() {
                canban_ui_kit::search::score(&row.fields, query)?
            } else {
                let tag_score = crate::tags::score_tags(&row.tags, &tag_terms)?;
                let other_score = if other_query.is_empty() {
                    0
                } else {
                    canban_ui_kit::search::score(&row.fields, &other_query)?
                };
                tag_score + other_score
            };
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
            .then_with(|| {
                if empty_query {
                    kind_rank(a.2.kind).cmp(&kind_rank(b.2.kind))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(
                || match (card_number(&a.2.title), card_number(&b.2.title)) {
                    (Some((na, ta)), Some((nb, tb)))
                        if a.2.kind == "Card" && b.2.kind == "Card" =>
                    {
                        if empty_query {
                            nb.cmp(&na)
                                .then_with(|| ta.to_lowercase().cmp(&tb.to_lowercase()))
                        } else {
                            na.cmp(&nb)
                                .then_with(|| ta.to_lowercase().cmp(&tb.to_lowercase()))
                        }
                    }
                    _ => a.2.title.to_lowercase().cmp(&b.2.title.to_lowercase()),
                },
            )
            .then(a.2.detail.to_lowercase().cmp(&b.2.detail.to_lowercase()))
            .then(a.2.id.cmp(&b.2.id))
    });
    scored.into_iter().map(|(_, _, r)| r.clone()).collect()
}
#[cfg(test)]
mod tests;
