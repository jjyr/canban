//! PR projection and delivery validation, independent of Git/runtime IO.
use anyhow::{Context, Result, ensure};
use canban_types::*;
use serde_json::Value;
pub fn project(value: &Value, old: &GitHubPr) -> Result<GitHubPr> {
    let mut pr = old.clone();
    pr.number = value["number"].as_u64().context("Invalid PR number")?;
    pr.url = value["html_url"].as_str().context("Invalid PR URL")?.into();
    ensure!(
        pr.url == format!("https://github.com/{}/pull/{}", pr.repository, pr.number),
        "PR repository does not match delivery"
    );
    pr.head = value["head"]["sha"]
        .as_str()
        .context("Invalid PR head")?
        .into();
    ensure!(
        value["base"]["ref"].as_str() == Some(pr.base_branch.as_str()),
        "PR target branch changed; inspect delivery before continuing"
    );
    pr.state = value["state"].as_str().unwrap_or("unknown").into();
    pr.merged = value["merged"].as_bool().unwrap_or(false) || value["merged_at"].is_string();
    Ok(pr)
}

fn is_https_url(url: &str) -> bool {
    url.starts_with("https://")
}

/// Aggregates GitHub check-runs and legacy commit statuses using fast-fail semantics:
/// as soon as any check has failed, `state` becomes `"failure"` even while others run.
pub fn summarize_checks(
    pr_url: &str,
    head_sha: &str,
    check_runs: &[Value],
    combined_status: &Value,
) -> GitHubPrChecks {
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut running = 0u32;
    let mut failed_checks = Vec::new();

    for run in check_runs {
        let name = run["name"].as_str().unwrap_or("Check").to_owned();
        let status = run["status"].as_str().unwrap_or("queued");
        let conclusion = run["conclusion"].as_str();
        let url = run["html_url"]
            .as_str()
            .or_else(|| run["details_url"].as_str())
            .filter(|u| is_https_url(u))
            .map(str::to_owned);
        if status == "completed" {
            if matches!(conclusion, Some("success" | "neutral" | "skipped")) {
                passed += 1;
            } else {
                failed += 1;
                failed_checks.push(GitHubPrCheck {
                    name,
                    status: status.to_owned(),
                    conclusion: conclusion.map(str::to_owned),
                    url,
                });
            }
        } else {
            running += 1;
        }
    }

    if let Some(statuses) = combined_status["statuses"].as_array()
        && !statuses.is_empty()
    {
        for st in statuses {
            let state = st["state"].as_str().unwrap_or("pending");
            let name = st["context"].as_str().unwrap_or("Status").to_owned();
            let url = st["target_url"]
                .as_str()
                .filter(|u| is_https_url(u))
                .map(str::to_owned);
            match state {
                "success" => passed += 1,
                "pending" => running += 1,
                _ => {
                    failed += 1;
                    failed_checks.push(GitHubPrCheck {
                        name,
                        status: "completed".into(),
                        conclusion: Some(state.to_owned()),
                        url,
                    });
                }
            }
        }
    } else if combined_status["total_count"].as_u64().unwrap_or(0) > 0 {
        match combined_status["state"].as_str().unwrap_or("pending") {
            "success" => passed += 1,
            "pending" => running += 1,
            other => {
                failed += 1;
                failed_checks.push(GitHubPrCheck {
                    name: "Commit status".into(),
                    status: "completed".into(),
                    conclusion: Some(other.to_owned()),
                    url: None,
                });
            }
        }
    }

    let total = passed + failed + running;
    let state = if total == 0 {
        "none"
    } else if failed > 0 {
        "failure"
    } else if running > 0 {
        "running"
    } else {
        "success"
    };
    let url =
        (!pr_url.trim().is_empty()).then(|| format!("{}/checks", pr_url.trim_end_matches('/')));

    GitHubPrChecks {
        state: state.into(),
        head_sha: head_sha.to_owned(),
        url,
        total,
        passed,
        failed,
        running,
        failed_checks,
    }
}
pub fn validate_merge<'a>(card: &'a Card, head: &str) -> Result<&'a GitHubPr> {
    ensure!(card.active(), "Restore the Card before delivery");
    let pr = card
        .pull_request
        .as_ref()
        .context("Publish a delivery PR first")?;
    ensure!(
        pr.number > 0 && pr.state == "open" && !pr.merged,
        "Delivery PR is not open"
    );
    ensure!(
        pr.head == head,
        "PR head changed; refresh and review before merging"
    );
    Ok(pr)
}
// Remote discussion is read from cache; it is never copied into the durable Timeline.
pub fn record_activity(
    _db: canban_storage::Db<'_>,
    _card: &Card,
    _comments: &[canban_github::models::Comment],
    _reviews: &[Value],
) -> Result<()> {
    Ok(())
}
