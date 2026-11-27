use super::*;
impl AppServer {
    /// Remote fields are an ephemeral projection. Opening a Board never causes HTTP calls.
    pub(crate) fn project_github(&self, snapshot: &mut Snapshot) {
        snapshot.github_issues.clear();
        let client = self.github_cached_client().ok();
        for card in &mut snapshot.cards {
            if let Some(source) = card.github_issue().cloned() {
                card.title = format!("{}#{}", source.repository, source.number);
                let issue = client.as_ref().and_then(|client| {
                    client.cached::<Value>(&format!(
                        "/repos/{}/issues/{}",
                        source.repository, source.number
                    ))
                });
                let remote = if card.is_github_pr() {
                    client
                        .as_ref()
                        .and_then(|client| {
                            client.cached::<Value>(&format!(
                                "/repos/{}/pulls/{}",
                                source.repository, source.number
                            ))
                        })
                        .or(issue)
                } else {
                    issue
                };
                if let Some(value) = remote {
                    card.title = value["title"].as_str().unwrap_or(&card.title).to_owned();
                    let pr = if card.is_github_pr() {
                        client.as_ref().and_then(|client| {
                            client.cached::<Value>(&format!(
                                "/repos/{}/pulls/{}",
                                source.repository, source.number
                            ))
                        })
                    } else {
                        None
                    };
                    let state = if pr.as_ref().is_some_and(|p| p["merged"] == true)
                        || card.delivery.is_some()
                    {
                        "merged"
                    } else if pr.as_ref().is_some_and(|p| p["draft"] == true) {
                        "draft"
                    } else {
                        value["state"].as_str().unwrap_or("unknown")
                    };
                    let pr_head_sha = pr
                        .as_ref()
                        .and_then(|p| p["head"]["sha"].as_str())
                        .map(str::to_owned);
                    let pr_checks = if card.is_github_pr() {
                        client
                            .as_ref()
                            .and_then(|client| {
                                client.cached::<Value>(&format!(
                                    "/repos/{}/pulls/{}/canban-summary",
                                    source.repository, source.number
                                ))
                            })
                            .and_then(|summary| {
                                serde_json::from_value::<GitHubPrChecks>(
                                    summary["checks_summary"].clone(),
                                )
                                .ok()
                            })
                    } else {
                        None
                    };
                    snapshot.github_issues.push(GitHubIssueSnapshot {
                        id: format!("{}:{}", card.project_id, source.issue_id),
                        project_id: card.project_id.clone(),
                        source,
                        title: card.title.clone(),
                        body: value["body"].as_str().unwrap_or("").into(),
                        state: state.into(),
                        author_id: value["user"]["id"].as_u64().unwrap_or(0),
                        author_login: value["user"]["login"].as_str().unwrap_or("").into(),
                        labels: value["labels"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|l| l["name"].as_str().map(str::to_owned))
                            .collect(),
                        assignees: value["assignees"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|l| l["login"].as_str().map(str::to_owned))
                            .collect(),
                        updated_at: value["updated_at"].as_str().unwrap_or("").into(),
                        synced_at: 0,
                        pr_head_sha,
                        pr_checks,
                    });
                }
            }
            if let Some(pr) = &mut card.pull_request {
                if let Some(value) = client.as_ref().and_then(|client| {
                    client.cached::<Value>(&format!("/repos/{}/pulls/{}", pr.repository, pr.number))
                }) && let Ok(projected) = integration::delivery::project(&value, pr)
                {
                    *pr = projected;
                }
                if let Some(summary) = client.as_ref().and_then(|client| {
                    client.cached::<Value>(&format!(
                        "/repos/{}/pulls/{}/canban-summary",
                        pr.repository, pr.number
                    ))
                }) {
                    pr.checks = summary["checks"].as_str().unwrap_or("").into();
                    pr.checks_summary =
                        serde_json::from_value(summary["checks_summary"].clone()).ok();
                    pr.review_summary = summary["reviews"].as_str().unwrap_or("").into();
                }
                if card.delivery.as_ref().is_some_and(|d| {
                    d.request_key == format!("github:{}:{}", pr.repository_id, pr.number)
                }) {
                    pr.merged = true;
                    pr.state = "closed".into();
                }
            }
        }
    }
}

pub(crate) fn enrich_pr_events(client: &canban_github::Client, events: &mut [Event]) {
    let mut activity_cache: std::collections::HashMap<(String, u64), Option<Value>> =
        std::collections::HashMap::new();
    for ev in events {
        if !matches!(
            ev.kind.as_str(),
            "github.pr.commented" | "github.pr.reviewed"
        ) {
            continue;
        }
        let Some(repo) = ev.data["repository"].as_str().map(str::to_owned) else {
            continue;
        };
        let Some(number) = ev.data["number"].as_u64() else {
            continue;
        };
        let key = (repo.to_lowercase(), number);
        let cached = activity_cache
            .entry(key)
            .or_insert_with(|| {
                client.cached::<Value>(&format!("/repos/{repo}/pulls/{number}/canban-activity"))
            })
            .as_ref();
        let Some(activity) = cached else {
            continue;
        };
        let (body, raw_ts) = match ev.kind.as_str() {
            "github.pr.commented" => {
                let section = if ev.data["kind"].as_str() == Some("review_comment") {
                    "review_comments"
                } else {
                    "issue_comments"
                };
                let entry = ev.data["comment_id"]
                    .as_u64()
                    .map(|cid| &activity[section][cid.to_string()]);
                (
                    entry.and_then(|v| v["body"].as_str()),
                    entry.and_then(|v| v["created_at"].as_str()),
                )
            }
            "github.pr.reviewed" => {
                let entry = ev.data["review_id"]
                    .as_u64()
                    .map(|rid| &activity["reviews"][rid.to_string()]);
                (
                    entry.and_then(|v| v["body"].as_str()),
                    entry.and_then(|v| {
                        v["submitted_at"]
                            .as_str()
                            .or_else(|| v["created_at"].as_str())
                    }),
                )
            }
            _ => (None, None),
        };
        if let Some(body) = body.filter(|b| !b.trim().is_empty()) {
            ev.data["projected_body"] = json!(body);
        }
        if let Some(occurred_at) = raw_ts.and_then(canban_notifications::parse_rfc3339_ms) {
            ev.data["occurred_at"] = json!(occurred_at);
            ev.created_at = occurred_at;
        }
    }
}
