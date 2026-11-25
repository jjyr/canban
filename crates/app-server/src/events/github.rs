//! Metadata-only remote observations. Clearing response caches never resets identity.
use super::*;

fn subject(db: Db<'_>, repo: &str, kind: &str, number: u64) -> Result<String> {
    let repo_id = db
        .list::<Project>()?
        .into_iter()
        .filter_map(|p| p.github)
        .find(|s| s.repository.eq_ignore_ascii_case(repo))
        .map(|s| s.repository_id);
    Ok(format!(
        "github.com:{}:{kind}:{number}",
        repo_id
            .map(|v| v.to_string())
            .unwrap_or_else(|| repo.to_lowercase())
    ))
}

type Relations = std::collections::BTreeMap<(String, u64), Vec<Value>>;
fn relation_index(cards: &[Card]) -> Relations {
    let mut result = Relations::new();
    for c in cards {
        let mut keys = std::collections::BTreeMap::new();
        if let Some(s) = c.github_issue() {
            keys.insert((s.repository.to_lowercase(), s.number), "source");
        }
        if let Some(s) = &c.pull_request {
            keys.entry((s.repository.to_lowercase(), s.number))
                .or_insert("delivery");
        }
        for s in &c.github_links {
            if let Some(number) = s.number {
                keys.entry((s.repository.to_lowercase(), number))
                    .or_insert("reference");
            }
        }
        for (key, kind) in keys {
            result
                .entry(key)
                .or_default()
                .push(json!({"card_id":c.id,"project_id":c.project_id,"kind":kind}));
        }
    }
    result
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct RepoWatermark {
    pub baseline_at: String,
    pub issues_since: String,
    pub comments_since: String,
}

pub(crate) fn format_iso8601_utc(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    // Howard Hinnant's civil_from_days algorithm (exact UTC proleptic Gregorian).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

pub(crate) fn load_repo_watermark(db: Db<'_>, repository: &str) -> Result<Option<RepoWatermark>> {
    let key = format!("repo-watermark:{}", repository.to_ascii_lowercase());
    let raw: Option<String> = db
        .conn
        .query_row(
            "SELECT data FROM source_observations WHERE id=?1",
            [&key],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
}

pub(crate) fn save_repo_watermark(
    db: Db<'_>,
    repository: &str,
    watermark: &RepoWatermark,
) -> Result<()> {
    let key = format!("repo-watermark:{}", repository.to_ascii_lowercase());
    db.conn.execute(
        "INSERT INTO source_observations(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
        params![key, serde_json::to_string(watermark)?],
    )?;
    Ok(())
}

/// Records a remote GitHub item ID (`comment` or `review`) written by Canban so polling
/// marks it `self_action: true` even when shell `GH_TOKEN` uses a different account.
pub(crate) fn record_written_receipt(
    db: Db<'_>,
    repository: &str,
    kind: &str,
    item_id: u64,
    now_ms: u64,
) -> Result<()> {
    if item_id == 0 {
        return Ok(());
    }
    let key = format!(
        "receipt:{}:{kind}:{item_id}",
        repository.to_ascii_lowercase()
    );
    db.conn.execute(
        "INSERT INTO source_observations(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
        params![key, json!({"recorded_at": now_ms}).to_string()],
    )?;
    Ok(())
}

pub(crate) fn has_written_receipt(db: Db<'_>, repository: &str, kind: &str, item_id: u64) -> bool {
    if item_id == 0 {
        return false;
    }
    let key = format!(
        "receipt:{}:{kind}:{item_id}",
        repository.to_ascii_lowercase()
    );
    db.conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM source_observations WHERE id=?1)",
            [&key],
            |r| r.get::<_, bool>(0),
        )
        .unwrap_or(false)
}

fn canonical_actor(value: &Value) -> (String, Option<&str>) {
    let login = value["user"]["login"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let actor = match login {
        Some(l) => format!("github:{l}"),
        None => "github".into(),
    };
    (actor, login)
}

fn parse_issue_number_from_url(url: &str) -> Option<u64> {
    url.trim_end_matches('/').rsplit('/').next()?.parse().ok()
}

/// Reuse one relation input for a synchronization transaction rather than loading
/// every Card again for each source observation.
pub(crate) fn observe_batch(
    db: Db<'_>,
    repository: &str,
    items: &[Value],
    kind: Option<&str>,
    now_ms: u64,
) -> Result<()> {
    observe_batch_with_context(db, repository, items, kind, None, None, now_ms)
}

pub(crate) fn observe_batch_with_context(
    db: Db<'_>,
    repository: &str,
    items: &[Value],
    kind: Option<&str>,
    baseline_at: Option<&str>,
    current_login: Option<&str>,
    now_ms: u64,
) -> Result<()> {
    let cards = relation_index(&db.list::<Card>()?);
    for item in items {
        let Some(number) = item["number"].as_u64() else {
            continue;
        };
        observe_with_cards(
            db,
            repository,
            number,
            kind.unwrap_or(if item.get("pull_request").is_some() {
                "pr"
            } else {
                "issue"
            }),
            "overview",
            item,
            baseline_at.is_some(),
            Some(&cards),
            baseline_at,
            current_login,
            now_ms,
        )?;
    }
    Ok(())
}

pub(crate) fn observe_repo_comments_batch(
    db: Db<'_>,
    repository: &str,
    comments: &[Value],
    collection_known: bool,
    current_login: Option<&str>,
    now_ms: u64,
) -> Result<()> {
    let all_cards = db.list::<Card>()?;
    let cards = relation_index(&all_cards);
    for comment in comments {
        let Some(issue_url) = comment["issue_url"].as_str() else {
            continue;
        };
        let Some(number) = parse_issue_number_from_url(issue_url) else {
            continue;
        };
        let is_pr = comment["html_url"]
            .as_str()
            .is_some_and(|u| u.contains("/pull/"))
            || all_cards.iter().any(|c| {
                c.is_github_pr()
                    && c.github_issue().is_some_and(|s| {
                        s.repository.eq_ignore_ascii_case(repository) && s.number == number
                    })
            });
        let kind = if is_pr { "pr" } else { "issue" };
        observe_with_cards(
            db,
            repository,
            number,
            kind,
            "discussion",
            comment,
            collection_known,
            Some(&cards),
            None,
            current_login,
            now_ms,
        )?;
    }
    Ok(())
}

const CANONICAL_OBSERVATION_VERSION: u64 = 2;

/// Normalize fields whose JSON representation varies between GitHub REST endpoints
/// and compact internal projections (`models::Issue`, `GitHubIssueSnapshot`) or
/// embeds volatile nested repo counters (`head.repo.pushed_at`, `open_issues_count`).
fn canonicalize_observed_field(field: &str, value: &Value) -> Value {
    match field {
        "labels" => {
            if let Some(arr) = value.as_array() {
                let mut names: Vec<String> = arr
                    .iter()
                    .filter_map(|item| {
                        item.as_str()
                            .or_else(|| item.get("name").and_then(Value::as_str))
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                    })
                    .collect();
                names.sort();
                names.dedup();
                json!(names)
            } else {
                value.clone()
            }
        }
        "assignees" | "requested_reviewers" => {
            if let Some(arr) = value.as_array() {
                let mut logins: Vec<String> = arr
                    .iter()
                    .filter_map(|item| {
                        item.as_str()
                            .or_else(|| item.get("login").and_then(Value::as_str))
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_ascii_lowercase())
                    })
                    .collect();
                logins.sort();
                logins.dedup();
                json!(logins)
            } else {
                value.clone()
            }
        }
        "head" => {
            if let Some(sha) = value
                .as_str()
                .or_else(|| value.get("sha").and_then(Value::as_str))
            {
                json!(sha)
            } else {
                value.clone()
            }
        }
        "body" if value.is_null() => json!(""),
        "merged" if value.is_null() => json!(false),
        _ => value.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn observe_with_cards(
    db: Db<'_>,
    repository: &str,
    number: u64,
    kind: &str,
    section: &str,
    value: &Value,
    collection_known: bool,
    cards: Option<&Relations>,
    baseline_at: Option<&str>,
    current_login: Option<&str>,
    now_ms: u64,
) -> Result<()> {
    let object = subject(db, repository, kind, number)?;
    let item = if section == "overview" {
        object.clone()
    } else {
        format!(
            "{object}:{section}:{}",
            value["id"]
                .as_u64()
                .context("Remote item has no stable ID")?
        )
    };
    let mut metadata = serde_json::Map::new();
    for field in [
        "title",
        "body",
        "state",
        "labels",
        "assignees",
        "draft",
        "merged",
        "head",
        "requested_reviewers",
        "conclusion",
        "status",
        "submitted_at",
        "in_reply_to_id",
    ] {
        if let Some(v) = value.get(field) {
            let canonical = canonicalize_observed_field(field, v);
            metadata.insert(
                field.into(),
                json!(canban_context::digest(&serde_json::to_vec(&canonical)?)),
            );
        }
    }
    metadata.insert("observed_state".into(), value["state"].clone());
    metadata.insert("merged_state".into(), value["merged"].clone());
    metadata.insert("updated_at".into(), value["updated_at"].clone());
    metadata.insert(
        "canonical_version".into(),
        json!(CANONICAL_OBSERVATION_VERSION),
    );

    let old: Option<String> = db
        .conn
        .query_row(
            "SELECT data FROM source_observations WHERE id=?1",
            [&item],
            |r| r.get(0),
        )
        .optional()?;
    let before: Value = old
        .as_ref()
        .map(|s| serde_json::from_str(s))
        .transpose()?
        .unwrap_or(Value::Null);
    let migrating_canonical = old.is_some()
        && before.get("canonical_version").and_then(Value::as_u64)
            != Some(CANONICAL_OBSERVATION_VERSION);
    // Sparse Issue projections cannot erase PR-only fields observed earlier.
    let mut merged = before.as_object().cloned().unwrap_or_default();
    for (key, value) in metadata {
        if !value.is_null() || !merged.contains_key(&key) {
            merged.insert(key, value);
        }
    }
    let new = Value::Object(merged);
    if new == before {
        return Ok(());
    }
    // Retain later observations if an older in-flight fetch returns last.
    if before["updated_at"]
        .as_str()
        .zip(new["updated_at"].as_str())
        .is_some_and(|(a, b)| a > b)
    {
        return Ok(());
    }
    let changed: Vec<_> = new
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| {
            !matches!(
                k.as_str(),
                "updated_at" | "observed_state" | "merged_state" | "canonical_version"
            ) && !(migrating_canonical
                && matches!(
                    k.as_str(),
                    "labels" | "assignees" | "requested_reviewers" | "head" | "body"
                ))
                && before.get(*k) != new.get(*k)
        })
        .cloned()
        .collect();
    let mut kinds = Vec::new();
    let mut is_baseline = old.is_none() && !collection_known;
    if old.is_none() {
        if section == "overview" {
            let opened = kind == "issue"
                && baseline_at
                    .zip(value["created_at"].as_str())
                    .is_some_and(|(baseline, created)| created >= baseline);
            if opened {
                kinds.push("opened");
                is_baseline = false;
            } else {
                kinds.push("discovered");
                is_baseline = true;
            }
        } else if !collection_known {
            kinds.push("discovered");
        } else if section == "reviews" {
            kinds.push("review_submitted");
        } else if section == "checks" {
            kinds.push("check_updated");
        } else {
            kinds.push("comment_created");
        }
    } else if !changed.is_empty() {
        if section != "overview" {
            kinds.push(if section == "reviews" && value["state"] == "DISMISSED" {
                "review_dismissed"
            } else if section == "reviews" {
                "review_updated"
            } else if section == "checks" {
                "check_updated"
            } else {
                "comment_edited"
            });
        } else {
            if changed.iter().any(|f| f == "state") {
                kinds.push(if value["state"] == "closed" {
                    "closed"
                } else {
                    "reopened"
                });
            }
            if value["merged"] == true && before["merged_state"] != true {
                kinds.push("merged");
            }
            for (field, event) in [
                ("head", "head_changed"),
                ("draft", "draft_changed"),
                ("labels", "labels_changed"),
                ("assignees", "assignees_changed"),
                ("requested_reviewers", "review_requested"),
            ] {
                if changed.iter().any(|f| f == field) {
                    kinds.push(event);
                }
            }
            if changed.iter().any(|f| f == "title" || f == "body") {
                kinds.push("edited");
            }
        }
    }
    let loaded;
    let cards = if let Some(cards) = cards {
        cards
    } else {
        loaded = relation_index(&db.list::<Card>()?);
        &loaded
    };
    let related = cards
        .get(&(repository.to_lowercase(), number))
        .cloned()
        .unwrap_or_default();
    let (actor, actor_login) = canonical_actor(value);
    let receipt_kind = if section == "reviews" {
        "review"
    } else {
        "comment"
    };
    let item_id = value["id"].as_u64().unwrap_or(0);
    let self_action = actor_login
        .is_some_and(|login| current_login.is_some_and(|cur| cur.eq_ignore_ascii_case(login)))
        || (section != "overview" && has_written_receipt(db, repository, receipt_kind, item_id));
    for action in kinds {
        // Avoid double-firing PR wildcard rules when a PR Card exists and `github/delivery.rs`
        // already emits canonical Card-scoped PR activity events (`updated`, `state_changed`,
        // `commented`, `reviewed`, `ci_*`).
        if kind == "pr"
            && !related.is_empty()
            && matches!(
                action,
                "comment_created"
                    | "review_submitted"
                    | "head_changed"
                    | "closed"
                    | "reopened"
                    | "draft_changed"
                    | "check_updated"
            )
        {
            continue;
        }
        let url = value["html_url"].as_str().unwrap_or("");
        db.append(canban_timeline::event_at(
            "",
            "",
            &format!("github.{kind}.{action}"),
            &actor,
            "GitHub source observation",
            json!({
                "origin": "github",
                "subject": object,
                "source_kind": format!("github.{kind}"),
                "repository": repository,
                "number": number,
                "url": url,
                "item_id": value["id"],
                "section": section,
                "reply_to": value["in_reply_to_id"],
                "changed_fields": changed,
                "revision": canban_context::digest(&serde_json::to_vec(&new)?),
                "related_cards": related,
                "occurred_at": value["updated_at"],
                "actor_login": actor_login,
                "self_action": self_action,
                "observation": "poll",
                "baseline": is_baseline,
            }),
            now_ms,
        ))?;
    }
    db.conn.execute("INSERT INTO source_observations(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![item,serde_json::to_string(&new)?])?;
    Ok(())
}
impl AppServer {
    pub(crate) fn observe_github_response(&self, path: &str, data: &Value) -> Result<()> {
        let parts: Vec<_> = path
            .split('?')
            .next()
            .unwrap_or(path)
            .trim_matches('/')
            .split('/')
            .collect();
        if parts.len() < 5 || parts[0] != "repos" || !matches!(parts[3], "issues" | "pulls") {
            return Ok(());
        }
        let Ok(number) = parts[4].parse::<u64>() else {
            return Ok(());
        };
        let repo = format!("{}/{}", parts[1], parts[2]);
        let kind = if parts[3] == "pulls"
            || data.get("pull_request").is_some()
            || self
                .store
                .lock()
                .unwrap()
                .db()
                .list::<Card>()?
                .iter()
                .any(|c| {
                    c.is_github_pr()
                        && c.github_issue().is_some_and(|s| {
                            s.repository.eq_ignore_ascii_case(&repo) && s.number == number
                        })
                }) {
            "pr"
        } else {
            "issue"
        };
        let section = if parts.len() == 5 {
            "overview"
        } else {
            match parts[5] {
                "comments" => {
                    if parts[3] == "pulls" {
                        "threads"
                    } else {
                        "discussion"
                    }
                }
                "reviews" => "reviews",
                _ => return Ok(()),
            }
        };
        let current_login = self.github.status().login;
        let now_ms = self.now_ms();
        self.store.lock().unwrap().write(|db| {
            if section == "overview" {
                observe_with_cards(
                    db,
                    &repo,
                    number,
                    kind,
                    section,
                    data,
                    false,
                    None,
                    None,
                    current_login.as_deref(),
                    now_ms,
                )?;
            } else {
                let key = format!("collection:{path}");
                let known = db.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM source_observations WHERE id=?1)",
                    [&key],
                    |r| r.get::<_, bool>(0),
                )?;
                let complete_key = format!("complete:{repo}:{kind}:{number}:{section}");
                let known = known
                    || db.conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM source_observations WHERE id=?1)",
                        [complete_key],
                        |r| r.get::<_, bool>(0),
                    )?;
                for item in data.as_array().context("Expected GitHub collection")? {
                    observe_with_cards(
                        db,
                        &repo,
                        number,
                        kind,
                        section,
                        item,
                        known,
                        None,
                        None,
                        current_login.as_deref(),
                        now_ms,
                    )?;
                }
                db.conn.execute(
                    "INSERT OR IGNORE INTO source_observations(id,data) VALUES(?1,'{}')",
                    [key],
                )?;
            }
            Ok(())
        })?;
        if section == "overview" && kind == "pr" && data["merged"] == true {
            let _ = self.reconcile_remote_pr_merge(&repo, number, data);
        }
        self.wake_runtime();
        Ok(())
    }
}

/// Only a complete successful scan can establish deletion or a collection baseline.
#[cfg(test)]
pub(crate) fn complete_collection(
    db: Db<'_>,
    repo: &str,
    number: u64,
    kind: &str,
    section: &str,
    items: &[Value],
) -> Result<()> {
    complete_collection_with_login(db, repo, number, kind, section, items, None, now())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn complete_collection_with_login(
    db: Db<'_>,
    repo: &str,
    number: u64,
    kind: &str,
    section: &str,
    items: &[Value],
    current_login: Option<&str>,
    now_ms: u64,
) -> Result<()> {
    let key = format!("complete:{repo}:{kind}:{number}:{section}");
    let old: Option<String> = db
        .conn
        .query_row(
            "SELECT data FROM source_observations WHERE id=?1",
            [&key],
            |r| r.get(0),
        )
        .optional()?;
    let before: Vec<u64> = old
        .as_ref()
        .map(|s| serde_json::from_str(s))
        .transpose()?
        .unwrap_or_default();
    let ids: Vec<_> = items.iter().filter_map(|v| v["id"].as_u64()).collect();
    for value in items {
        observe_with_cards(
            db,
            repo,
            number,
            kind,
            section,
            value,
            old.is_some(),
            None,
            None,
            current_login,
            now_ms,
        )?;
    }
    let related: Vec<_> = db
        .list::<Card>()?
        .iter()
        .filter(|c| {
            c.github_issue()
                .is_some_and(|s| s.repository.eq_ignore_ascii_case(repo) && s.number == number)
        })
        .map(|c| json!({"card_id":c.id,"project_id":c.project_id,"kind":"source"}))
        .collect();
    let changed = old.is_none() || ids != before;
    for removed in before.into_iter().filter(|id| !ids.contains(id)) {
        db.append(canban_timeline::event_at("","",&format!("github.{kind}.{}",if section=="checks"{"check_removed"}else if section=="reviews"{"review_removed"}else{"comment_deleted"}),"github","Remote collection item removed",json!({"origin":"github","repository":repo,"number":number,"subject":subject(db,repo,kind,number)?,"section":section,"item_id":removed,"related_cards":related,"observation":"complete_poll"}), now_ms))?;
    }
    if changed {
        db.conn.execute("INSERT INTO source_observations(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![key,serde_json::to_string(&ids)?])?;
    }
    Ok(())
}
