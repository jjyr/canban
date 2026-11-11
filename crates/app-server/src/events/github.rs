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

pub(crate) fn observe(
    db: Db<'_>,
    repository: &str,
    number: u64,
    kind: &str,
    section: &str,
    value: &Value,
    collection_known: bool,
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
            metadata.insert(
                field.into(),
                json!(canban_context::digest(&serde_json::to_vec(v)?)),
            );
        }
    }
    metadata.insert("observed_state".into(), value["state"].clone());
    metadata.insert("merged_state".into(), value["merged"].clone());
    metadata.insert("updated_at".into(), value["updated_at"].clone());

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
            !matches!(k.as_str(), "updated_at" | "observed_state" | "merged_state")
                && before.get(*k) != new.get(*k)
        })
        .cloned()
        .collect();
    let mut kinds = Vec::new();
    if old.is_none() {
        kinds.push(if section == "overview" || !collection_known {
            "discovered"
        } else if section == "reviews" {
            "review_submitted"
        } else if section == "checks" {
            "check_updated"
        } else {
            "comment_created"
        });
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
    let cards = db.list::<Card>()?;
    let related: Vec<_> = cards
        .iter()
        .filter_map(|c| {
            let relation =
                if c.github_issue().is_some_and(|s| {
                    s.repository.eq_ignore_ascii_case(repository) && s.number == number
                }) {
                    "source"
                } else if c.pull_request.as_ref().is_some_and(|s| {
                    s.repository.eq_ignore_ascii_case(repository) && s.number == number
                }) {
                    "delivery"
                } else if c.github_links.iter().any(|s| {
                    s.repository.eq_ignore_ascii_case(repository) && s.number == Some(number)
                }) {
                    "reference"
                } else {
                    return None;
                };
            Some(json!({"card_id":c.id,"project_id":c.project_id,"kind":relation}))
        })
        .collect();
    for action in kinds {
        db.append(canban_timeline::event("","",&format!("github.{kind}.{action}"),&format!("github:{}",value["user"]["id"]),"GitHub source observation",None,json!({"origin":"github","subject":object,"source_kind":format!("github.{kind}"),"repository":repository,"number":number,"item_id":value["id"],"section":section,"reply_to":value["in_reply_to_id"],"changed_fields":changed,"revision":canban_context::digest(&serde_json::to_vec(&new)?),"related_cards":related,"occurred_at":value["updated_at"],"observation":"poll","baseline":old.is_none()&&!collection_known})))?;
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
        self.store.lock().unwrap().write(|db| {
            if section == "overview" {
                observe(db, &repo, number, kind, section, data, false)?;
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
                    observe(db, &repo, number, kind, section, item, known)?;
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
pub(crate) fn complete_collection(
    db: Db<'_>,
    repo: &str,
    number: u64,
    kind: &str,
    section: &str,
    items: &[Value],
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
        observe(db, repo, number, kind, section, value, old.is_some())?;
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
    for removed in before.into_iter().filter(|id| !ids.contains(id)) {
        db.append(canban_timeline::event("","",&format!("github.{kind}.{}",if section=="checks"{"check_removed"}else if section=="reviews"{"review_removed"}else{"comment_deleted"}),"github","Remote collection item removed",None,json!({"origin":"github","repository":repo,"number":number,"subject":subject(db,repo,kind,number)?,"section":section,"item_id":removed,"related_cards":related,"observation":"complete_poll"})))?;
    }
    db.conn.execute("INSERT INTO source_observations(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![key,serde_json::to_string(&ids)?])?;
    Ok(())
}
