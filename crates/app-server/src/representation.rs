//! Agent-facing response contracts. Presentation never enters mutation fingerprints.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

const CARD_FIELDS: &[&str] = &[
    "id",
    "project_id",
    "number",
    "title",
    "body",
    "stage",
    "revision",
    "created_at",
    "created_by",
    "assignee",
    "content",
    "source_trust",
    "schedule_source",
    "github_links",
    "hold_auto_merge",
    "removed_at",
    "pull_request",
    "pull_request_history",
    "delivery_mode",
    "workspace",
    "workspace_history",
    "board_order",
    "context_notes",
    "attention",
    "archived_at",
    "archive",
    "archive_error",
    "worktree",
    "branch",
    "base_commit",
    "human_gate",
    "delivery",
    "resolution",
];
const CARD_SUMMARY: &[&str] = &["id", "number", "title", "stage", "revision"];
const EVENT_FIELDS: &[&str] = &[
    "id",
    "project_id",
    "card_id",
    "sequence",
    "kind",
    "actor",
    "created_at",
    "body",
    "message",
    "data",
];
const EVENT_SUMMARY: &[&str] = &[
    "id",
    "card_id",
    "sequence",
    "kind",
    "actor",
    "created_at",
    "body",
];

pub fn card_response(method: &str) -> bool {
    matches!(
        method,
        "card.get"
            | "card.show"
            | "card.list"
            | "card.create"
            | "card.update"
            | "card.move"
            | "card.adopt"
            | "card.restore"
            | "card.resolve"
            | "card.reopen"
            | "card.context.update"
            | "card.hold_auto_merge"
    )
}

/// Validate before dispatch, including before any side effect or handoff enqueue.
pub fn validate(method: &str, p: &Value) -> Result<()> {
    ensure!(p.is_object(), "Parameters must be an object");
    if let Some(view) = p.get("view") {
        ensure!(view == "full", "invalid_view: use view=full or omit view");
    }
    if let Some(fields) = p.get("fields") {
        ensure!(
            p.get("view").is_none(),
            "fields and view are mutually exclusive"
        );
        let allowed = if card_response(method) && method != "card.adopt" {
            CARD_FIELDS
        } else if matches!(method, "timeline.list" | "comment.add" | "comment.update") {
            EVENT_FIELDS
        } else {
            anyhow::bail!("fields_not_supported: {method}");
        };
        let fields = fields
            .as_array()
            .context("fields must be an array of field names")?;
        ensure!(!fields.is_empty(), "fields must not be empty");
        for field in fields {
            let field = field.as_str().context("fields must contain strings")?;
            ensure!(
                allowed.contains(&field),
                "unknown_field: {field}; allowed: {}",
                allowed.join(",")
            );
        }
    }
    Ok(())
}

fn select(value: &Value, fields: &[&str]) -> Value {
    Value::Object(
        fields
            .iter()
            .map(|key| ((*key).into(), value[*key].clone()))
            .collect(),
    )
}

fn project(value: Value, p: &Value, defaults: &[&str]) -> Value {
    if p["view"] == "full" {
        value
    } else if let Some(fields) = p["fields"].as_array() {
        let fields = fields.iter().filter_map(Value::as_str).collect::<Vec<_>>();
        select(&value, &fields)
    } else {
        select(&value, defaults)
    }
}

fn event(mut value: Value) -> Value {
    if value["kind"]
        .as_str()
        .is_some_and(|k| k.starts_with("comment."))
    {
        let body = value["data"]
            .get("body")
            .cloned()
            .unwrap_or_else(|| value["message"].clone());
        value["body"] = body;
        value.as_object_mut().unwrap().remove("message");
        if let Some(data) = value["data"].as_object_mut() {
            data.remove("body");
        }
    }
    value
}

pub(super) fn render(method: &str, p: &Value, mut result: Value) -> Value {
    if method == "card.list" {
        if let Some(items) = result["items"].as_array_mut() {
            for item in items {
                *item = project(item.take(), p, CARD_SUMMARY);
            }
        }
        return result;
    }
    if method == "timeline.list" {
        if let Some(events) = result["events"].as_array_mut() {
            for item in events {
                *item = project(event(item.take()), p, EVENT_SUMMARY);
            }
        }
        return result;
    }
    if matches!(method, "comment.add" | "comment.update") {
        if p["view"] == "full" || p.get("fields").is_some() {
            let mut view = project(event(result.clone()), p, EVENT_SUMMARY);
            // Control information cannot be hidden by projection.
            view["accepted"] = json!(true);
            view["id"] = result["id"].clone();
            view["card_id"] = result["card_id"].clone();
            view["sequence"] = result["sequence"].clone();
            if let Some(revision) = result.get("card_revision") {
                view["card_revision"] = revision.clone();
            }
            result = view;
        } else {
            result = select(&result, &["id", "card_id", "sequence", "card_revision"]);
            result["accepted"] = json!(true);
        }
    } else if method == "card.adopt" && p["view"] != "full" {
        let card = &result["card"];
        let mut receipt = select(card, &["id", "number", "stage", "revision"]);
        receipt["adopted"] = result["adopted"].clone();
        receipt["workflow_run_id"] = result["workflow_run"]["id"].clone();
        receipt["accepted"] = json!(true);
        result = receipt;
    } else if card_response(method) {
        let read = matches!(method, "card.get" | "card.show");
        let controls = select(&result, &["id", "number", "stage", "revision"]);
        result = project(
            result,
            p,
            if read {
                CARD_SUMMARY
            } else {
                &["id", "number", "stage", "revision"]
            },
        );
        if !read {
            for (key, value) in controls.as_object().unwrap() {
                result[key] = value.clone();
            }
            result["accepted"] = json!(true);
        }
    }
    // Reports/replies already have small, semantically distinct receipts. Preserve them.
    if !matches!(method, "card.get" | "card.show")
        && (card_response(method)
            || matches!(
                method,
                "comment.add"
                    | "comment.update"
                    | "comment.delete"
                    | "execution.report"
                    | "agent.reply"
                    | "batch.report"
                    | "schedule.report"
            ))
        && let Some(key) = p.get("request_key")
    {
        result["request_key"] = key.clone();
    }
    result
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CardCursor {
    scope: String,
    generation: String,
    number: u64,
    id: String,
}

fn limit(p: &Value, default: u64) -> Result<usize> {
    let n = p
        .get("limit")
        .map(|v| v.as_u64().context("limit must be an integer"))
        .transpose()?
        .unwrap_or(default);
    ensure!((1..=500).contains(&n), "limit must be between 1 and 500");
    Ok(n as usize)
}

impl super::AppServer {
    pub(super) fn card_page(&self, p: &Value) -> Result<Value> {
        let store = self.store.lock().unwrap();
        let db = store.db();
        let project = p
            .get("project_id")
            .filter(|v| !v.is_null())
            .map(|v| {
                super::resolve_project_id(db, v.as_str().context("project_id must be a string")?)
            })
            .transpose()?;
        let stage = p
            .get("stage")
            .map(|v| v.as_str().context("stage must be a string"))
            .transpose()?;
        let removed = p["include_removed"] == true;
        let limit = limit(p, 50)?;
        let scope = canban_context::digest(&serde_json::to_vec(&json!([project, stage, removed]))?);
        // Read only membership/revisions for consistency; never deserialize all Card bodies.
        let filter = "(?1 IS NULL OR project_id=?1) AND (?2 IS NULL OR state=?2) AND (?3 OR json_extract(data,'$.removed_at') IS NULL)";
        let mut q = db.conn.prepare(&format!(
            "SELECT id,json_extract(data,'$.revision') FROM cards WHERE {filter} ORDER BY number,id"
        ))?;
        let metadata = q
            .query_map(rusqlite::params![project, stage, removed], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let generation = canban_context::digest(&serde_json::to_vec(&metadata)?);
        let cursor = p
            .get("cursor")
            .filter(|v| !v.is_null())
            .map(|v| -> Result<CardCursor> {
                let c: CardCursor =
                    serde_json::from_str(v.as_str().context("cursor must be a string")?)
                        .context("invalid_cursor")?;
                ensure!(
                    c.scope == scope,
                    "cursor_scope_mismatch: use the original filters"
                );
                ensure!(
                    c.generation == generation,
                    "cursor_stale: restart the listing and deduplicate by id"
                );
                Ok(c)
            })
            .transpose()?;
        let (after, id) = cursor
            .as_ref()
            .map(|c| (c.number, c.id.as_str()))
            .unwrap_or((0, ""));
        let mut q = db.conn.prepare(&format!("SELECT number,id,data FROM cards WHERE {filter} AND (number>?4 OR (number=?4 AND id>?5)) ORDER BY number,id LIMIT ?6"))?;
        let mut rows = q
            .query_map(
                rusqlite::params![project, stage, removed, after, id, limit + 1],
                |r| {
                    Ok((
                        r.get::<_, u64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let more = rows.len() > limit;
        rows.truncate(limit);
        let next = if more {
            let last = rows.last().unwrap();
            Some(serde_json::to_string(&CardCursor {
                scope,
                generation,
                number: last.0,
                id: last.1.clone(),
            })?)
        } else {
            None
        };
        let items = rows
            .into_iter()
            .map(|(_, _, data)| serde_json::from_str::<Value>(&data))
            .collect::<serde_json::Result<Vec<_>>>()?;
        Ok(json!({"items":items,"next_cursor":next}))
    }

    pub(super) fn timeline_page(&self, p: &Value) -> Result<Value> {
        let after_param = p
            .get("after_sequence")
            .filter(|v| !v.is_null())
            .map(|v| v.as_u64().context("after_sequence must be an integer"))
            .transpose()?;
        let before_param = p
            .get("before_sequence")
            .filter(|v| !v.is_null())
            .map(|v| v.as_u64().context("before_sequence must be an integer"))
            .transpose()?;
        ensure!(
            after_param.is_none() || before_param.is_none(),
            "after_sequence and before_sequence are mutually exclusive"
        );
        let tail = p
            .get("tail")
            .filter(|v| !v.is_null())
            .map(|v| v.as_bool().context("tail must be a boolean"))
            .transpose()?
            .unwrap_or(false);
        let limit = limit(p, 50)?;
        let kinds = p
            .get("kinds")
            .filter(|v| !v.is_null())
            .map(|v| -> Result<String> {
                let kinds = v.as_array().context("kinds must be an array")?;
                ensure!(
                    kinds.iter().all(Value::is_string),
                    "kinds must contain strings"
                );
                Ok(serde_json::to_string(kinds)?)
            })
            .transpose()?;

        let (mut events, next_cursor, last_sequence, highwater, has_more_before, oldest_sequence) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let card = super::resolve_card(db, p)?.id;
            let highwater: u64 = db.conn.query_row(
                "SELECT COALESCE(MAX(number),0) FROM events WHERE card_id=?1",
                [&card],
                |r| r.get(0),
            )?;
            if let Some(before) = before_param {
                let mut q = db.conn.prepare(
                    "SELECT data FROM events WHERE card_id=?1 AND number<?2 AND (?3 IS NULL OR json_extract(data,'$.kind') IN (SELECT value FROM json_each(?3))) ORDER BY number DESC LIMIT ?4",
                )?;
                let mut events = q
                    .query_map(rusqlite::params![card, before, kinds, limit + 1], |r| {
                        r.get::<_, String>(0)
                    })?
                    .map(|r| Ok(serde_json::from_str::<canban_types::Event>(&r?)?))
                    .collect::<Result<Vec<_>>>()?;
                let has_more_before = events.len() > limit;
                events.truncate(limit);
                events.reverse();
                let oldest_sequence = events.first().map(|e| e.sequence);
                let last_sequence = events.last().map(|e| e.sequence).unwrap_or(0);
                (
                    events,
                    None,
                    last_sequence,
                    highwater,
                    has_more_before,
                    oldest_sequence,
                )
            } else if tail && after_param.is_none() {
                let mut q = db.conn.prepare(
                    "SELECT data FROM events WHERE card_id=?1 AND number<=?2 AND (?3 IS NULL OR json_extract(data,'$.kind') IN (SELECT value FROM json_each(?3))) ORDER BY number DESC LIMIT ?4",
                )?;
                let mut events = q
                    .query_map(rusqlite::params![card, highwater, kinds, limit + 1], |r| {
                        r.get::<_, String>(0)
                    })?
                    .map(|r| Ok(serde_json::from_str::<canban_types::Event>(&r?)?))
                    .collect::<Result<Vec<_>>>()?;
                let has_more_before = events.len() > limit;
                events.truncate(limit);
                events.reverse();
                let oldest_sequence = events.first().map(|e| e.sequence);
                (
                    events,
                    None,
                    highwater,
                    highwater,
                    has_more_before,
                    oldest_sequence,
                )
            } else {
                let after = after_param.unwrap_or(0);
                let mut q = db.conn.prepare(
                    "SELECT data FROM events WHERE card_id=?1 AND number>?2 AND number<=?3 AND (?4 IS NULL OR json_extract(data,'$.kind') IN (SELECT value FROM json_each(?4))) ORDER BY number LIMIT ?5",
                )?;
                let mut events = q
                    .query_map(
                        rusqlite::params![card, after, highwater, kinds, limit + 1],
                        |r| r.get::<_, String>(0),
                    )?
                    .map(|r| Ok(serde_json::from_str::<canban_types::Event>(&r?)?))
                    .collect::<Result<Vec<_>>>()?;
                let more = events.len() > limit;
                events.truncate(limit);
                let progress = if more {
                    events.last().unwrap().sequence
                } else {
                    highwater.max(after)
                };
                let oldest_sequence = events.first().map(|e| e.sequence);
                let has_more_before = if after > 0 {
                    let bound = oldest_sequence.unwrap_or(after.saturating_add(1));
                    db.conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM events WHERE card_id=?1 AND number<?2 AND (?3 IS NULL OR json_extract(data,'$.kind') IN (SELECT value FROM json_each(?3))))",
                        rusqlite::params![card, bound, kinds],
                        |r| r.get::<_, i64>(0),
                    )? != 0
                } else {
                    false
                };
                (
                    events,
                    more.then_some(progress),
                    progress,
                    highwater,
                    has_more_before,
                    oldest_sequence,
                )
            }
        };

        if let Ok(client) = self.github_cached_client() {
            crate::github::projection::enrich_pr_events(&client, &mut events);
        }
        let events: Vec<Value> = events
            .into_iter()
            .map(serde_json::to_value)
            .collect::<serde_json::Result<Vec<_>>>()?;
        Ok(json!({
            "events": events,
            "next_cursor": next_cursor,
            "last_sequence": last_sequence,
            "highwater": highwater,
            "has_more_before": has_more_before,
            "oldest_sequence": oldest_sequence,
        }))
    }
}
