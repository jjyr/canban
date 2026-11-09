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
    pr.merged = value["merged"].as_bool().unwrap_or(false);
    Ok(pr)
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
pub fn record_activity(
    db: canban_storage::Db<'_>,
    card: &Card,
    comments: &[canban_github::models::Comment],
    reviews: &[Value],
) -> Result<()> {
    let events = db.list::<Event>()?;
    for comment in comments {
        let key = format!("pr-comment:{}:{}", comment.id, comment.updated_at);
        if events
            .iter()
            .any(|e| e.card_id == card.id && e.data["external_revision"] == key)
        {
            continue;
        }
        db.append(canban_timeline::event(
            &card.project_id,
            &card.id,
            "github.pr_comment",
            &format!("github:{}", comment.user.login),
            &comment.body,
            None,
            serde_json::json!({"external_revision":key,"url":comment.html_url}),
        ))?;
    }
    for review in reviews {
        let key = format!(
            "pr-review:{}:{}:{}",
            review["id"], review["submitted_at"], review["state"]
        );
        if events
            .iter()
            .any(|e| e.card_id == card.id && e.data["external_revision"] == key)
        {
            continue;
        }
        db.append(canban_timeline::event(
            &card.project_id,
            &card.id,
            "github.pr_review",
            &format!(
                "github:{}",
                review["user"]["login"].as_str().unwrap_or("unknown")
            ),
            &format!(
                "{}\n{}",
                review["state"].as_str().unwrap_or("Review"),
                review["body"].as_str().unwrap_or("")
            ),
            None,
            serde_json::json!({"external_revision":key,"url":review["html_url"]}),
        ))?;
    }
    Ok(())
}
