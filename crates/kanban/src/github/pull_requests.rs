//! PR discovery/eligibility policy. AppServer only schedules and applies this reconciliation.
use super::{Change, Sync};
use anyhow::{Context, Result};
use canban_github::Client;
use canban_types::*;
use serde_json::Value;
pub fn fetch(client: &Client, project: &Project, cards: &[Card], login: &str) -> Result<Sync> {
    let source = project
        .github
        .as_ref()
        .context("Project has no GitHub source")?;
    let mut numbers: std::collections::BTreeSet<u64> = cards
        .iter()
        .filter(|c| c.project_id == project.id && c.removed_at.is_none() && c.is_github_pr())
        .filter_map(|c| {
            c.github_issue()
                .filter(|s| s.repository_id == source.repository_id)
                .map(|s| s.number)
        })
        .collect();
    if source.pr_intake != GitHubPrIntake::Off {
        let collaborators = if source.pr_intake == GitHubPrIntake::Collaborators {
            Some(client.collaborators(&source.repository)?)
        } else {
            None
        };
        let prs: Vec<Value> = client.pages(&format!(
            "/repos/{}/pulls?state=open&sort=updated&direction=desc",
            source.repository
        ))?;
        for pr in prs {
            if pr["draft"] == true && !source.include_drafts {
                continue;
            }
            let author = pr["user"]["login"].as_str().unwrap_or("");
            if !source.authors.is_empty()
                && !source
                    .authors
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(author))
            {
                continue;
            }
            if !source.labels.is_empty()
                && !source.labels.iter().any(|l| {
                    pr["labels"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|v| v["name"] == *l)
                })
            {
                continue;
            }
            let match_user = |field: &str| {
                pr[field].as_array().into_iter().flatten().any(|u| {
                    u["login"]
                        .as_str()
                        .is_some_and(|l| l.eq_ignore_ascii_case(login))
                })
            };
            let eligible = match source.pr_intake {
                GitHubPrIntake::Off => false,
                GitHubPrIntake::All => true,
                GitHubPrIntake::Collaborators => collaborators
                    .as_ref()
                    .is_some_and(|us| us.iter().any(|u| Some(u.id) == pr["user"]["id"].as_u64())),
                GitHubPrIntake::ReviewRequested => match_user("requested_reviewers"),
                GitHubPrIntake::Assigned => match_user("assignees"),
                GitHubPrIntake::Authored => author.eq_ignore_ascii_case(login),
            };
            if eligible
                && let Some(n) = pr["number"].as_u64()
                && !cards.iter().any(|c| {
                    c.project_id == project.id
                        && c.pull_request.as_ref().is_some_and(|p| {
                            p.repository_id == source.repository_id && p.number == n
                        })
                })
            {
                numbers.insert(n);
            }
        }
    }
    let mut changes = Vec::new();
    for number in numbers {
        let issue = client.item(&source.repository, number)?;
        let _: Value = client.get(&format!("/repos/{}/pulls/{number}", source.repository))?;
        changes.push(Change {
            issue,
            comments: Vec::new(),
        });
    }
    Ok(Sync { changes })
}
