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

/// Finds the best matching PR for a local Card's `branch` from a PR listing.
/// Prefers open same-repo PRs, then open fork PRs matching `push_owner`,
/// then a unique open PR on `branch`. Closed/merged PRs match only when
/// `local_head_sha` matches `pr["head"]["sha"]`.
pub fn match_branch_pr<'a>(
    prs: &'a [Value],
    repository_id: u64,
    branch: &str,
    excluded_numbers: &[u64],
    push_owner: Option<&str>,
    local_head_sha: Option<&str>,
) -> Option<&'a Value> {
    let branch = branch.trim();
    if branch.is_empty() {
        return None;
    }
    let candidates: Vec<&Value> = prs
        .iter()
        .filter(|pr| {
            pr["head"]["ref"].as_str() == Some(branch)
                && pr["number"]
                    .as_u64()
                    .is_some_and(|n| n > 0 && !excluded_numbers.contains(&n))
        })
        .collect();
    let open: Vec<&Value> = candidates
        .iter()
        .copied()
        .filter(|pr| pr["state"].as_str() == Some("open"))
        .collect();
    if let Some(same_repo) = open
        .iter()
        .copied()
        .find(|pr| pr["head"]["repo"]["id"].as_u64() == Some(repository_id))
    {
        return Some(same_repo);
    }
    if let Some(owner) = push_owner
        && let Some(fork_match) = open.iter().copied().find(|pr| {
            pr["head"]["user"]["login"]
                .as_str()
                .is_some_and(|l| l.eq_ignore_ascii_case(owner))
        })
    {
        return Some(fork_match);
    }
    if open.len() == 1 {
        return Some(open[0]);
    }
    if let Some(sha) = local_head_sha.filter(|s| !s.is_empty()) {
        return candidates
            .into_iter()
            .find(|pr| pr["head"]["sha"].as_str() == Some(sha));
    }
    None
}

/// Builds a `GitHubPr` binding from a GitHub Pull Request API object for a local Card.
pub fn build_linked_pr(
    project: &Project,
    card: &Card,
    repository_id: u64,
    repository: &str,
    pr: &Value,
) -> Option<GitHubPr> {
    let number = pr["number"].as_u64().filter(|&n| n > 0)?;
    let url = pr["html_url"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("https://github.com/{repository}/pull/{number}"));
    let head = pr["head"]["sha"].as_str().unwrap_or("").to_owned();
    let head_branch = pr["head"]["label"]
        .as_str()
        .or_else(|| pr["head"]["ref"].as_str())
        .or(card.branch.as_deref())
        .unwrap_or("")
        .to_owned();
    let base_branch = pr["base"]["ref"]
        .as_str()
        .or(project.target_branch.as_deref())
        .unwrap_or("main")
        .to_owned();
    let state = pr["state"].as_str().unwrap_or("open").to_owned();
    let merged = pr["merged"].as_bool().unwrap_or(false) || pr["merged_at"].is_string();
    let workspace = card.workspace.clone().unwrap_or_else(|| WorkspaceBinding {
        directory: project.directory.clone(),
        path: card.worktree.clone().unwrap_or_default(),
        branch: card.branch.clone(),
        base_commit: card.base_commit.clone(),
        target_branch: project.target_branch.clone(),
    });
    Some(GitHubPr {
        review_summary: String::new(),
        repository: repository.to_owned(),
        repository_id,
        number,
        url,
        state,
        head,
        head_branch,
        base_branch,
        merged,
        checks: String::new(),
        checks_summary: None,
        workspace,
    })
}
