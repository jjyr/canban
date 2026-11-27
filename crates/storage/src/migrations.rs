//! Paired database-only migrations after the legacy schema-8 baseline.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
pub const BASELINE: u32 = 8;
pub const CURRENT: u32 = 24;
pub struct Migration {
    pub version: u32,
    pub up: fn(&Transaction<'_>) -> Result<()>,
    pub down: fn(&Transaction<'_>) -> Result<()>,
}
// Append paired steps here; retain down steps for the supported rollback floor.
const STEPS: &[Migration] = &[
    // Versions remain consecutive; workflow steps follow the merged schema 10.
    Migration {
        version: 9,
        up: github_up,
        down: github_down,
    },
    Migration {
        version: 10,
        up: editor_up,
        down: editor_down,
    },
    Migration {
        version: 11,
        up: workflows_up,
        down: workflows_down,
    },
    Migration {
        version: 12,
        up: authoring_up,
        down: authoring_down,
    },
    Migration {
        version: 13,
        up: managed_worktrees_up,
        down: managed_worktrees_down,
    },
    Migration {
        version: 14,
        up: remove_requests_up,
        down: remove_requests_down,
    },
    Migration {
        version: 15,
        up: remove_attempts_up,
        down: remove_attempts_down,
    },
    Migration {
        version: 16,
        up: short_ids_up,
        down: short_ids_down,
    },
    Migration {
        version: 17,
        up: merge_batches_up,
        down: merge_batches_down,
    },
    Migration {
        version: 18,
        up: drop_manual_terminal_counter_up,
        down: drop_manual_terminal_counter_down,
    },
    Migration {
        version: 19,
        up: purge_legacy_workflow_templates_up,
        down: purge_legacy_workflow_templates_down,
    },
    Migration {
        version: 20,
        up: schedules_up,
        down: schedules_down,
    },
    Migration {
        version: 21,
        up: activity_up,
        down: activity_down,
    },
    Migration {
        version: 22,
        up: remove_legacy_remote_machine_up,
        down: remove_legacy_remote_machine_down,
    },
    Migration {
        version: 23,
        up: env_secrets_up,
        down: env_secrets_down,
    },
    Migration {
        version: 24,
        up: detach_manual_terminal_project_up,
        down: detach_manual_terminal_project_down,
    },
];

fn flatten_workspace_binding(v: &mut Value) -> bool {
    let Some(obj) = v.as_object_mut() else {
        return false;
    };
    let Some(loc) = obj.remove("location") else {
        return false;
    };
    if obj
        .get("directory")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
        && let Some(dir) = loc.get("directory").and_then(Value::as_str)
    {
        obj.insert("directory".into(), Value::String(dir.to_owned()));
    }
    true
}

fn wrap_workspace_binding(v: &mut Value) -> bool {
    let Some(obj) = v.as_object_mut() else {
        return false;
    };
    if obj.contains_key("location") {
        return false;
    }
    let Some(dir) = obj.remove("directory").and_then(|d| match d {
        Value::String(s) => Some(s),
        _ => None,
    }) else {
        return false;
    };
    obj.insert(
        "location".into(),
        serde_json::json!({"machine": "local", "directory": dir}),
    );
    true
}

fn rewrite_json_table(
    tx: &Transaction<'_>,
    table: &str,
    mut transform: impl FnMut(&mut Value) -> bool,
) -> Result<()> {
    if !table_exists(tx, table)? {
        return Ok(());
    }
    let rows: Vec<(String, String)> = {
        let mut q = tx.prepare(&format!("SELECT id, data FROM {table}"))?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, raw) in rows {
        let mut value: Value = serde_json::from_str(&raw)?;
        if transform(&mut value) {
            tx.execute(
                &format!("UPDATE {table} SET data=?2 WHERE id=?1"),
                params![id, serde_json::to_string(&value)?],
            )?;
        }
    }
    Ok(())
}

fn remove_legacy_remote_machine_up(tx: &Transaction<'_>) -> Result<()> {
    rewrite_json_table(tx, "projects", |value| {
        let Some(obj) = value.as_object_mut() else {
            return false;
        };
        let Some(loc) = obj.remove("location") else {
            return false;
        };
        if obj
            .get("directory")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
            && let Some(dir) = loc.get("directory").and_then(Value::as_str)
        {
            obj.insert("directory".into(), Value::String(dir.to_owned()));
        }
        true
    })?;

    rewrite_json_table(tx, "cards", |value| {
        let Some(obj) = value.as_object_mut() else {
            return false;
        };
        let mut changed = false;
        if let Some(ws) = obj.get_mut("workspace") {
            changed |= flatten_workspace_binding(ws);
        }
        if let Some(history) = obj
            .get_mut("workspace_history")
            .and_then(Value::as_array_mut)
        {
            for ws in history {
                changed |= flatten_workspace_binding(ws);
            }
        }
        if let Some(ws) = obj
            .get_mut("pull_request")
            .and_then(|pr| pr.get_mut("workspace"))
        {
            changed |= flatten_workspace_binding(ws);
        }
        if let Some(prs) = obj
            .get_mut("pull_request_history")
            .and_then(Value::as_array_mut)
        {
            for pr in prs {
                if let Some(ws) = pr.get_mut("workspace") {
                    changed |= flatten_workspace_binding(ws);
                }
            }
        }
        changed
    })?;

    rewrite_json_table(tx, "managed_worktrees", flatten_workspace_binding)?;

    rewrite_json_table(tx, "terminals", |value| {
        let Some(obj) = value.as_object_mut() else {
            return false;
        };
        let mut changed = false;
        if let Some(rt) = obj.get_mut("runtime").and_then(Value::as_object_mut) {
            changed |= rt.remove("machine").is_some();
            changed |= rt.remove("owner_core_id").is_some();
        }
        if let Some(agent) = obj.get_mut("agent").and_then(Value::as_object_mut) {
            if let Some(loc) = agent.remove("location") {
                changed = true;
                if agent
                    .get("directory")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                    && let Some(dir) = loc.get("directory").and_then(Value::as_str)
                {
                    agent.insert("directory".into(), Value::String(dir.to_owned()));
                }
            }
            changed |= agent.remove("runtime_home").is_some();
            changed |= agent.remove("owner_core_id").is_some();
            if let Some(ws) = agent.get_mut("workspace") {
                changed |= flatten_workspace_binding(ws);
            }
        }
        changed
    })?;

    Ok(())
}

fn remove_legacy_remote_machine_down(tx: &Transaction<'_>) -> Result<()> {
    rewrite_json_table(tx, "projects", |value| {
        let Some(obj) = value.as_object_mut() else {
            return false;
        };
        if obj.contains_key("location") {
            return false;
        }
        let dir = obj
            .get("directory")
            .and_then(Value::as_str)
            .unwrap_or_default();
        obj.insert(
            "location".into(),
            serde_json::json!({"machine": "local", "directory": dir}),
        );
        true
    })?;

    rewrite_json_table(tx, "cards", |value| {
        let Some(obj) = value.as_object_mut() else {
            return false;
        };
        let mut changed = false;
        if let Some(ws) = obj.get_mut("workspace") {
            changed |= wrap_workspace_binding(ws);
        }
        if let Some(history) = obj
            .get_mut("workspace_history")
            .and_then(Value::as_array_mut)
        {
            for ws in history {
                changed |= wrap_workspace_binding(ws);
            }
        }
        if let Some(ws) = obj
            .get_mut("pull_request")
            .and_then(|pr| pr.get_mut("workspace"))
        {
            changed |= wrap_workspace_binding(ws);
        }
        if let Some(prs) = obj
            .get_mut("pull_request_history")
            .and_then(Value::as_array_mut)
        {
            for pr in prs {
                if let Some(ws) = pr.get_mut("workspace") {
                    changed |= wrap_workspace_binding(ws);
                }
            }
        }
        changed
    })?;

    rewrite_json_table(tx, "managed_worktrees", wrap_workspace_binding)?;

    rewrite_json_table(tx, "terminals", |value| {
        let Some(obj) = value.as_object_mut() else {
            return false;
        };
        let mut changed = false;
        if let Some(rt) = obj.get_mut("runtime").and_then(Value::as_object_mut) {
            if !rt.contains_key("machine") {
                rt.insert("machine".into(), Value::String("local".into()));
                changed = true;
            }
            if !rt.contains_key("owner_core_id") {
                let owner = rt.get("core_id").cloned().unwrap_or(Value::Null);
                rt.insert("owner_core_id".into(), owner);
                changed = true;
            }
        }
        if let Some(agent) = obj.get_mut("agent").and_then(Value::as_object_mut) {
            if !agent.contains_key("location") {
                let loc = match agent.remove("directory") {
                    Some(Value::String(dir)) => {
                        serde_json::json!({"machine": "local", "directory": dir})
                    }
                    _ => Value::Null,
                };
                agent.insert("location".into(), loc);
                changed = true;
            } else {
                changed |= agent.remove("directory").is_some();
            }
            if !agent.contains_key("runtime_home") {
                agent.insert("runtime_home".into(), Value::Null);
                changed = true;
            }
            if !agent.contains_key("owner_core_id") {
                let owner = agent.get("core_id").cloned().unwrap_or(Value::Null);
                agent.insert("owner_core_id".into(), owner);
                changed = true;
            }
            if let Some(ws) = agent.get_mut("workspace") {
                changed |= wrap_workspace_binding(ws);
            }
        }
        changed
    })?;

    Ok(())
}

fn env_secrets_up(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS vault_entries(\
            namespace TEXT NOT NULL,\
            scope TEXT NOT NULL,\
            key TEXT NOT NULL,\
            nonce BLOB NOT NULL,\
            ciphertext BLOB NOT NULL,\
            hint TEXT NOT NULL DEFAULT '',\
            updated_at INTEGER NOT NULL,\
            PRIMARY KEY (namespace, scope, key)\
        );\
        CREATE TABLE IF NOT EXISTS env_entries(\
            scope TEXT NOT NULL,\
            target_id TEXT NOT NULL,\
            key TEXT NOT NULL,\
            kind TEXT NOT NULL,\
            value TEXT,\
            updated_at INTEGER NOT NULL,\
            deleted INTEGER NOT NULL DEFAULT 0,\
            PRIMARY KEY (scope, target_id, key)\
        );",
    )?;
    Ok(())
}

fn env_secrets_down(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "DROP TABLE IF EXISTS env_entries;\
         DROP TABLE IF EXISTS vault_entries;",
    )?;
    Ok(())
}

fn detach_manual_terminal_project_up(tx: &Transaction<'_>) -> Result<()> {
    if !table_exists(tx, "terminals")? {
        return Ok(());
    }
    tx.execute_batch(
        "UPDATE terminals SET project_id=NULL \
         WHERE card_id IS NULL \
           AND id NOT LIKE 'merge-queue:%' \
           AND id NOT LIKE 'configuration:%' \
           AND id NOT LIKE 'scheduled:%';",
    )?;
    rewrite_json_table(tx, "terminals", |v| {
        v.as_object_mut()
            .is_some_and(|obj| obj.remove("origin_project_id").is_some())
    })?;
    Ok(())
}

fn detach_manual_terminal_project_down(_tx: &Transaction<'_>) -> Result<()> {
    Ok(())
}

fn activity_up(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS activity_segments(\
            id INTEGER PRIMARY KEY,\
            kind TEXT NOT NULL,\
            start_ms INTEGER NOT NULL,\
            end_ms INTEGER NOT NULL,\
            project_id TEXT,\
            card_id TEXT,\
            terminal_id TEXT,\
            data TEXT NOT NULL\
        );\
        CREATE INDEX IF NOT EXISTS activity_segments_time ON activity_segments(kind, start_ms);",
    )?;
    Ok(())
}

fn activity_down(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("DROP TABLE IF EXISTS activity_segments;")?;
    Ok(())
}

fn schedules_up(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS schedule_states(\
            id TEXT PRIMARY KEY,\
            project_id TEXT,\
            card_id TEXT,\
            state TEXT NOT NULL,\
            number INTEGER NOT NULL,\
            data TEXT NOT NULL\
        );\
        CREATE INDEX IF NOT EXISTS schedule_states_scope ON schedule_states(project_id, state);\
        CREATE TABLE IF NOT EXISTS scheduled_runs(\
            id TEXT PRIMARY KEY,\
            project_id TEXT,\
            card_id TEXT,\
            state TEXT NOT NULL,\
            number INTEGER NOT NULL,\
            data TEXT NOT NULL\
        );\
        CREATE INDEX IF NOT EXISTS scheduled_runs_scope ON scheduled_runs(project_id, state);",
    )?;
    Ok(())
}

fn schedules_down(tx: &Transaction<'_>) -> Result<()> {
    let active: u64 = tx.query_row(
        "SELECT count(*) FROM scheduled_runs WHERE json_extract(data,'$.state') IN ('queued','starting','unknown','running','working')",
        [],
        |r| r.get(0),
    )?;
    ensure!(active == 0, "End active scheduled runs before downgrading");
    tx.execute_batch("DROP TABLE IF EXISTS scheduled_runs; DROP TABLE IF EXISTS schedule_states;")?;
    Ok(())
}

fn purge_legacy_workflow_templates_up(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "DELETE FROM workflow_templates \
         WHERE json_type(data, '$.sources') IS NOT 'array' \
            OR json_type(data, '$.name') IS NOT 'text' \
            OR json_type(data, '$.updated_at') IS NOT 'integer';",
    )?;
    tx.execute_batch("DROP TABLE IF EXISTS workflow_bindings;")?;
    Ok(())
}

fn purge_legacy_workflow_templates_down(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS workflow_bindings(\
            id TEXT PRIMARY KEY,\
            project_id TEXT,\
            card_id TEXT,\
            state TEXT NOT NULL,\
            number INTEGER NOT NULL,\
            data TEXT NOT NULL\
        );\
        CREATE INDEX IF NOT EXISTS workflow_bindings_scope ON workflow_bindings(project_id, card_id, state);",
    )?;
    Ok(())
}

fn drop_manual_terminal_counter_up(tx: &Transaction<'_>) -> Result<()> {
    let rows: Vec<(String, String)> = {
        let mut q = tx.prepare("SELECT id, data FROM terminals")?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, raw) in rows {
        let mut value: Value = serde_json::from_str(&raw)?;
        let Some(obj) = value.as_object_mut() else {
            continue;
        };
        let manual = obj.remove("manual_number").and_then(|v| v.as_u64());
        let has_name = obj
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty());
        let is_independent = obj.get("conversation_id").is_none_or(Value::is_null);
        if is_independent
            && !has_name
            && let Some(n) = manual
        {
            let prefix = obj
                .get("agent_engine")
                .and_then(Value::as_str)
                .map(canban_types::friendly_agent_engine)
                .unwrap_or_else(|| "Terminal".into());
            obj.insert("name".into(), serde_json::json!(format!("{prefix} {n}")));
        }
        tx.execute(
            "UPDATE terminals SET data=?2 WHERE id=?1",
            params![id, serde_json::to_string(&value)?],
        )?;
    }
    tx.execute_batch("DELETE FROM counters WHERE id='terminal-manual-number';")?;
    Ok(())
}

fn drop_manual_terminal_counter_down(tx: &Transaction<'_>) -> Result<()> {
    let rows: Vec<(String, i64, String)> = {
        let mut q = tx.prepare("SELECT id, number, data FROM terminals ORDER BY number ASC")?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut seq = 0u64;
    for (id, _num, raw) in rows {
        let mut value: Value = serde_json::from_str(&raw)?;
        let Some(obj) = value.as_object_mut() else {
            continue;
        };
        if obj.get("conversation_id").is_none_or(Value::is_null) {
            seq += 1;
            obj.insert("manual_number".into(), serde_json::json!(seq));
        } else {
            obj.insert("manual_number".into(), Value::Null);
        }
        tx.execute(
            "UPDATE terminals SET data=?2 WHERE id=?1",
            params![id, serde_json::to_string(&value)?],
        )?;
    }
    if seq > 0 {
        tx.execute(
            "INSERT OR REPLACE INTO counters(id, value) VALUES('terminal-manual-number', ?1)",
            [seq],
        )?;
    }
    Ok(())
}

fn merge_batches_up(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS merge_batches(\
            id TEXT PRIMARY KEY,\
            project_id TEXT,\
            card_id TEXT,\
            state TEXT NOT NULL,\
            number INTEGER NOT NULL,\
            data TEXT NOT NULL\
        );\
        CREATE INDEX IF NOT EXISTS merge_batches_scope ON merge_batches(project_id, state);",
    )?;
    Ok(())
}

fn merge_batches_down(tx: &Transaction<'_>) -> Result<()> {
    let active: u64 = tx.query_row(
        "SELECT count(*) FROM merge_batches WHERE json_extract(data,'$.state') NOT IN ('completed','blocked','failed','cancelled')",
        [],
        |r| r.get(0),
    )?;
    ensure!(active == 0, "End active merge batches before downgrading");
    tx.execute_batch("DROP TABLE IF EXISTS merge_batches;")?;
    Ok(())
}
fn workflows_up(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("CREATE TABLE IF NOT EXISTS event_journal(offset INTEGER PRIMARY KEY AUTOINCREMENT,event_id TEXT NOT NULL UNIQUE REFERENCES events(id)); INSERT OR IGNORE INTO event_journal(event_id) SELECT id FROM events ORDER BY rowid; CREATE TABLE IF NOT EXISTS source_observations(id TEXT PRIMARY KEY,data TEXT NOT NULL);")?;
    for table in [
        "workflow_revisions",
        "workflow_bindings",
        "workflow_runs",
        "event_subscriptions",
        "event_deliveries",
    ] {
        tx.execute_batch(&format!("CREATE TABLE IF NOT EXISTS {table}(id TEXT PRIMARY KEY,project_id TEXT,card_id TEXT,state TEXT NOT NULL,number INTEGER NOT NULL,data TEXT NOT NULL); CREATE INDEX IF NOT EXISTS {table}_scope ON {table}(project_id,card_id,state);"))?;
    }
    tx.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS workflow_revision_number ON workflow_revisions(json_extract(data,'$.workflow_id'),json_extract(data,'$.revision')); CREATE UNIQUE INDEX IF NOT EXISTS event_delivery_key ON event_deliveries(json_extract(data,'$.subscription_id'),json_extract(data,'$.event_id'));")?;
    Ok(())
}
fn workflows_down(tx: &Transaction<'_>) -> Result<()> {
    let definitions: u64 =
        tx.query_row("SELECT count(*) FROM workflow_revisions", [], |r| r.get(0))?;
    ensure!(
        definitions == 0,
        "Cannot downgrade while custom workflow definitions exist; preserve/export them before selecting an older application"
    );
    // Do not allow an old binary to replay workflow-owned queued assignments.
    // Current legacy-compatible requests retain their snapshots for old recovery.
    tx.execute_batch("UPDATE requests SET data=json_set(data,'$.state','cancelled'),state='cancelled' WHERE json_extract(data,'$.workflow_run_id') IS NOT NULL AND state='queued' AND json_extract(data,'$.trigger')!='stage_entry'; DROP TABLE event_deliveries; DROP TABLE event_subscriptions; DROP TABLE workflow_runs; DROP TABLE workflow_bindings; DROP TABLE workflow_revisions; DROP TABLE source_observations; DROP TABLE event_journal;")?;
    Ok(())
}
fn github_up(tx: &Transaction<'_>) -> Result<()> {
    use rusqlite::params;
    tx.execute_batch("DELETE FROM github_issues; DELETE FROM events WHERE json_extract(data,'$.kind') IN ('github.comment','github.pr_comment','github.pr_review','github.issue_synced','github.pr_synced'); CREATE UNIQUE INDEX IF NOT EXISTS github_source_card ON cards(project_id,json_extract(data,'$.content.source.repository_id'),json_extract(data,'$.content.source.issue_id')) WHERE json_extract(data,'$.content.kind') IN ('github_issue','github_pr');")?;
    let db = super::Db { conn: tx };
    let mut source_titles = std::collections::HashMap::new();
    for c in db.list::<canban_types::Card>()? {
        db.save(&c)?;
        if let Some(source) = c.github_issue() {
            source_titles.insert(
                c.id.clone(),
                format!("{}#{}", source.repository, source.number),
            );
        }
    }
    for mut event in db
        .list::<canban_types::Event>()?
        .into_iter()
        .filter(|e| matches!(e.kind.as_str(), "card.created" | "card.updated"))
    {
        if let Some(title) = source_titles.get(&event.card_id)
            && event.data.get("title").is_some()
        {
            event.data["title"] = serde_json::Value::String(title.clone());
            db.save(&event)?;
        }
    }
    let receipts: Vec<(String, String)> = {
        let mut q = tx.prepare("SELECT id,result FROM operations")?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?
    };
    for (id, result) in receipts {
        if let Ok(card) = serde_json::from_str::<canban_types::Card>(&result) {
            tx.execute(
                "UPDATE operations SET result=?2 WHERE id=?1",
                params![id, super::Record::data(&card)?],
            )?;
        }
    }
    Ok(())
}
fn github_down(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("DROP INDEX IF EXISTS github_source_card;")?;
    Ok(())
}
fn editor_up(tx: &Transaction<'_>) -> Result<()> {
    // Both feature branches used schema 9. Retain the original shape so
    // startup failure can roll back to either old branch without losing drafts.
    tx.execute_batch("CREATE TABLE IF NOT EXISTS migration10_compat(keep_editor INTEGER NOT NULL,keep_source_index INTEGER NOT NULL); DELETE FROM migration10_compat; INSERT INTO migration10_compat SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='editor_drafts'),EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='github_source_card'); CREATE TABLE IF NOT EXISTS editor_drafts(id TEXT PRIMARY KEY,revision INTEGER NOT NULL,updated_at INTEGER NOT NULL,data TEXT NOT NULL);")?;
    github_up(tx)?;
    Ok(())
}
fn editor_down(tx: &Transaction<'_>) -> Result<()> {
    let (keep_editor, keep_index): (bool, bool) = tx.query_row(
        "SELECT keep_editor,keep_source_index FROM migration10_compat",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if !keep_editor {
        tx.execute_batch("DROP TABLE editor_drafts;")?;
    }
    if !keep_index {
        tx.execute_batch("DROP INDEX IF EXISTS github_source_card;")?;
    }
    tx.execute_batch("DROP TABLE migration10_compat;")?;
    Ok(())
}
pub fn version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}
pub fn apply(conn: &mut Connection, target: u32) -> Result<()> {
    apply_with_progress(conn, target, |_, _| {})
}
pub fn apply_with_progress(
    conn: &mut Connection,
    target: u32,
    on_step: impl FnMut(u32, u32),
) -> Result<()> {
    ensure!(
        CURRENT == STEPS.last().map_or(BASELINE, |s| s.version),
        "Migration registry and CURRENT schema disagree"
    );
    batch_with_progress(conn, STEPS, target, on_step)
}
#[cfg(test)]
fn batch(conn: &mut Connection, steps: &[Migration], target: u32) -> Result<()> {
    batch_with_progress(conn, steps, target, |_, _| {})
}
fn batch_with_progress(
    conn: &mut Connection,
    steps: &[Migration],
    target: u32,
    mut on_step: impl FnMut(u32, u32),
) -> Result<()> {
    let before = version(conn)?;
    let ceiling = steps.last().map_or(BASELINE, |s| s.version);
    ensure!(
        (BASELINE..=ceiling).contains(&target),
        "Unsupported target schema {target}; supported {BASELINE}..={ceiling}"
    );
    ensure!(
        (BASELINE..=ceiling).contains(&before),
        "Unsupported current schema {before}; supported {BASELINE}..={ceiling}"
    );
    for (i, step) in steps.iter().enumerate() {
        ensure!(
            step.version == BASELINE + i as u32 + 1,
            "Migration versions must be consecutive"
        );
    }
    if before == target {
        return Ok(());
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if target > before {
        for step in steps
            .iter()
            .filter(|s| s.version > before && s.version <= target)
        {
            on_step(step.version, target);
            (step.up)(&tx)?;
            tx.pragma_update(None, "user_version", step.version)?;
        }
    } else {
        for step in steps
            .iter()
            .rev()
            .filter(|s| s.version > target && s.version <= before)
        {
            on_step(step.version - 1, target);
            (step.down)(&tx)?;
            tx.pragma_update(None, "user_version", step.version - 1)?;
        }
    }
    let violations: u64 =
        tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })?;
    ensure!(violations == 0, "Migration violates foreign keys");
    let integrity: String = tx.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    ensure!(integrity == "ok", "Migration integrity check: {integrity}");
    tx.commit()?;
    Ok(())
}
/// Called by the candidate CLI while the updater owns the upgrade lock and no
/// AppServer owns the service lock. It never opens product/runtime services.
pub fn migrate(home: &Path, target: u32) -> Result<u32> {
    if database_version(home)? < BASELINE {
        ensure!(
            target == CURRENT,
            "Legacy databases can only be upgraded to the current schema"
        );
        // Reuse the legacy initialization steps without starting product/runtime services.
        drop(crate::Store::open(home)?);
        return database_version(home);
    }
    let mut conn = Connection::open(home.join("canban.sqlite"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    prepare(home, &conn, target)?;
    apply(&mut conn, target)?;
    let result = version(&conn)?;
    if result >= 16 {
        sync_paths(home, &conn)?;
    }
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(result)
}

/// Consistent SQLite snapshot, including committed WAL data, without mutating source.
pub fn backup(home: &Path, destination: &Path) -> Result<()> {
    let conn = Connection::open_with_flags(
        home.join("canban.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    conn.execute(
        "VACUUM INTO ?1",
        [destination.to_str().context("Invalid snapshot path")?],
    )?;
    std::fs::File::open(destination)?.sync_all()?;
    Ok(())
}
pub fn database_version(home: &Path) -> Result<u32> {
    let conn = Connection::open_with_flags(
        home.join("canban.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    version(&conn)
}

#[cfg(test)]
mod local_handoff_tests {
    use super::*;
    #[test]
    fn legacy_upgrade_uses_existing_steps_and_snapshot_includes_wal_data() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        drop(crate::Store::open(home).unwrap());
        migrate(home, BASELINE).unwrap();
        let conn = Connection::open(home.join("canban.sqlite")).unwrap();
        conn.execute_batch("DROP TABLE card_reads; PRAGMA user_version=7; CREATE TABLE keep(value TEXT); INSERT INTO keep VALUES('preserved');").unwrap();
        let snapshot = home.join("before.sqlite");
        backup(home, &snapshot).unwrap();
        conn.execute("INSERT INTO keep VALUES('later')", [])
            .unwrap();
        drop(conn);
        assert!(migrate(home, BASELINE).is_err());
        assert_eq!(database_version(home).unwrap(), 7);
        assert_eq!(migrate(home, CURRENT).unwrap(), CURRENT);
        assert_eq!(migrate(home, CURRENT).unwrap(), CURRENT);
        let old = Connection::open(snapshot).unwrap();
        assert_eq!(version(&old).unwrap(), 7);
        assert_eq!(
            old.query_row("SELECT count(*) FROM keep", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            1
        );
        let current = Connection::open(home.join("canban.sqlite")).unwrap();
        assert_eq!(
            current
                .query_row("SELECT count(*) FROM keep", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            2
        );
    }
}

/// Export legacy remote mirrors before the database-only migration transaction.
/// The user approved this cleanup; local comments, Cards and Agent history stay.
pub fn prepare(home: &Path, conn: &Connection, target: u32) -> Result<()> {
    let before = version(conn)?;
    let source_index:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='github_source_card')",[],|r|r.get(0))?;
    if target >= 9 && (before == 8 || (before == 9 && !source_index)) {
        // Preserve old mirrored bodies in a disposable export before changing the DB.
        // Existing execution artifacts/archives are historical local records and stay intact.
        let old: Vec<serde_json::Value> = {
            let mut q=conn.prepare("SELECT data FROM github_issues UNION ALL SELECT data FROM events WHERE json_extract(data,'$.kind') IN ('github.comment','github.pr_comment','github.pr_review','github.issue_synced','github.pr_synced') OR (json_extract(data,'$.kind') IN ('card.created','card.updated') AND card_id IN (SELECT id FROM cards WHERE json_extract(data,'$.content.kind') IN ('github_issue','github_pr'))) UNION ALL SELECT result FROM operations WHERE json_extract(result,'$.content.kind') IN ('github_issue','github_pr') OR json_extract(result,'$.pull_request') IS NOT NULL")?;
            q.query_map([], |r| r.get::<_, String>(0))?
                .map(|r| Ok(serde_json::from_str(&r?)?))
                .collect::<Result<_>>()?
        };
        if !old.is_empty() {
            let root = home.join("cache/github/legacy");
            std::fs::create_dir_all(&root)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
            }
            let mut f = tempfile::NamedTempFile::new_in(&root)?;
            std::io::Write::write_all(&mut f, &serde_json::to_vec(&old)?)?;
            f.as_file().sync_all()?;
            f.persist(root.join("v8.json"))?;
            std::fs::File::open(&root)?.sync_all()?;
        }
    }
    if before >= 16 && target < 16 {
        sync_paths_down(home, conn)?;
    }
    Ok(())
}

fn authoring_up(tx: &Transaction<'_>) -> Result<()> {
    for table in [
        "workflow_sources",
        "configuration_sessions",
        "workflow_templates",
    ] {
        tx.execute_batch(&format!("CREATE TABLE IF NOT EXISTS {table}(id TEXT PRIMARY KEY,project_id TEXT,card_id TEXT,state TEXT NOT NULL,number INTEGER NOT NULL,data TEXT NOT NULL);"))?;
    }
    Ok(())
}
fn authoring_down(tx: &Transaction<'_>) -> Result<()> {
    let count: u64 = tx.query_row("SELECT count(*) FROM workflow_sources", [], |r| r.get(0))?;
    ensure!(
        count == 0,
        "Cannot downgrade while YAML workflow sources exist; preserve their configuration first"
    );
    let templates: u64 =
        tx.query_row("SELECT count(*) FROM workflow_templates", [], |r| r.get(0))?;
    ensure!(
        templates == 0,
        "Cannot downgrade while saved workflow templates exist; reset or export them first"
    );
    let live: u64 = tx.query_row("SELECT count(*) FROM configuration_sessions WHERE json_extract(data,'$.state') NOT IN ('finished','cancelled')", [], |r| r.get(0))?;
    ensure!(live == 0, "End configuration sessions before downgrading");
    tx.execute_batch("DROP TABLE configuration_sessions; DROP TABLE workflow_sources; DROP TABLE workflow_templates;")?;
    Ok(())
}
fn managed_worktrees_up(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS managed_worktrees(id TEXT PRIMARY KEY,project_id TEXT,card_id TEXT,state TEXT NOT NULL,number INTEGER NOT NULL,data TEXT NOT NULL); CREATE INDEX IF NOT EXISTS managed_worktrees_scope ON managed_worktrees(project_id,card_id,state);",
    )?;
    Ok(())
}
fn managed_worktrees_down(tx: &Transaction<'_>) -> Result<()> {
    let count: u64 = tx.query_row("SELECT count(*) FROM managed_worktrees", [], |r| r.get(0))?;
    ensure!(
        count == 0,
        "Cannot downgrade while standalone managed worktrees exist; promote or delete them first"
    );
    tx.execute_batch(
        "DROP INDEX IF EXISTS managed_worktrees_scope; DROP TABLE managed_worktrees;",
    )?;
    Ok(())
}
type TableRow = (String, Option<String>, Option<String>, String, i64, String);

fn remove_requests_up(tx: &Transaction<'_>) -> Result<()> {
    use rusqlite::params;
    use serde_json::{Value, json};
    let has_requests: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='requests')",
        [],
        |r| r.get(0),
    )?;
    if !has_requests {
        return Ok(());
    }
    let req_rows: Vec<TableRow> = {
        let mut q = tx.prepare(
            "SELECT id, project_id, card_id, state, number, data FROM requests ORDER BY rowid",
        )?;
        q.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<std::result::Result<_, _>>()?
    };
    let mut req_by_id = std::collections::HashMap::<String, Value>::new();
    for (id, _, _, _, _, data) in &req_rows {
        if let Ok(v) = serde_json::from_str::<Value>(data) {
            req_by_id.insert(id.clone(), v);
        }
    }
    let att_rows: Vec<(String, String)> = {
        let mut q = tx.prepare("SELECT id, data FROM attempts ORDER BY rowid")?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?
    };
    let mut req_to_attempt = std::collections::HashMap::<String, (String, u64)>::new();
    let mut has_attempt = std::collections::HashSet::<String>::new();
    for (att_id, raw) in att_rows {
        let mut att: Value = serde_json::from_str(&raw)?;
        let req_id = att
            .as_object_mut()
            .and_then(|o| o.remove("request_id"))
            .and_then(|v| v.as_str().map(str::to_owned));
        let num = att["number"].as_u64().unwrap_or(1);
        if let Some(rid) = req_id {
            has_attempt.insert(rid.clone());
            if req_to_attempt
                .get(&rid)
                .is_none_or(|(_, old_num)| num >= *old_num)
            {
                req_to_attempt.insert(rid.clone(), (att_id.clone(), num));
            }
            if let Some(req) = req_by_id.get(&rid)
                && let Some(obj) = att.as_object_mut()
            {
                for k in [
                    "workflow_run_id",
                    "workflow_step",
                    "resume_from",
                    "trigger",
                    "stage",
                    "profile",
                    "instruction",
                    "on_completed",
                    "require_human_review",
                ] {
                    if let Some(v) = req.get(k) {
                        obj.insert(k.into(), v.clone());
                    }
                }
                if obj.get("location").is_none_or(Value::is_null)
                    && let Some(loc) = req.get("location")
                {
                    obj.insert("location".into(), loc.clone());
                }
            }
        }
        let state = att["state"].as_str().unwrap_or("").to_owned();
        tx.execute(
            "UPDATE attempts SET state=?2, number=?3, data=?4 WHERE id=?1",
            params![att_id, state, num as i64, serde_json::to_string(&att)?],
        )?;
    }
    for (rid, project_id, card_id, state, _, raw) in &req_rows {
        if has_attempt.contains(rid) {
            continue;
        }
        let req: Value = serde_json::from_str(raw)?;
        req_to_attempt.insert(rid.clone(), (rid.clone(), 0));
        let att = json!({
            "id": rid,
            "project_id": req["project_id"],
            "card_id": req["card_id"],
            "number": 0,
            "state": req["state"],
            "workflow_run_id": req.get("workflow_run_id").cloned().unwrap_or(Value::Null),
            "workflow_step": req.get("workflow_step").cloned().unwrap_or(Value::Null),
            "location": req.get("location").cloned().unwrap_or(Value::Null),
            "resume_from": req.get("resume_from").cloned().unwrap_or(Value::Null),
            "trigger": req.get("trigger").cloned().unwrap_or_else(|| json!("workflow")),
            "stage": req.get("stage").cloned().unwrap_or_else(|| json!("")),
            "profile": req.get("profile").cloned().unwrap_or_else(|| json!({})),
            "instruction": req.get("instruction").cloned().unwrap_or_else(|| json!("")),
            "on_completed": req.get("on_completed").cloned().unwrap_or(Value::Null),
            "require_human_review": req.get("require_human_review").cloned().unwrap_or(json!(false)),
            "cwd": "",
            "input_path": "",
            "created_at": req["created_at"],
        });
        tx.execute(
            "INSERT INTO attempts(id, project_id, card_id, state, number, data) VALUES(?1, ?2, ?3, ?4, 0, ?5)",
            params![rid, project_id, card_id, state, serde_json::to_string(&att)?],
        )?;
    }
    let map_req = |rid: &str| -> String {
        req_to_attempt
            .get(rid)
            .map(|(aid, _)| aid.clone())
            .unwrap_or_else(|| rid.to_owned())
    };
    let rewrite = |table: &str, f: &dyn Fn(&mut Value)| -> Result<()> {
        let rows: Vec<(String, String)> = {
            let mut q = tx.prepare(&format!("SELECT id, data FROM {table}"))?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (id, raw) in rows {
            let mut v: Value = serde_json::from_str(&raw)?;
            let before = v.clone();
            f(&mut v);
            if v != before {
                tx.execute(
                    &format!("UPDATE {table} SET data=?2 WHERE id=?1"),
                    params![id, serde_json::to_string(&v)?],
                )?;
            }
        }
        Ok(())
    };
    rewrite("workflow_runs", &|v| {
        if let Some(obj) = v.as_object_mut()
            && let Some(rid) = obj.remove("request_id")
        {
            obj.insert(
                "attempt_id".into(),
                rid.as_str()
                    .map(|s| json!(map_req(s)))
                    .unwrap_or(Value::Null),
            );
        }
    })?;
    rewrite("cards", &|v| {
        if let Some(gate) = v.get_mut("human_gate").and_then(Value::as_object_mut)
            && let Some(rid) = gate.remove("request_id")
        {
            gate.insert(
                "attempt_id".into(),
                rid.as_str()
                    .map(|s| json!(map_req(s)))
                    .unwrap_or(Value::Null),
            );
        }
    })?;
    rewrite("conversations", &|v| {
        if let Some(obj) = v.as_object_mut()
            && let Some(rid) = obj.remove("last_request_id")
        {
            obj.insert(
                "last_attempt_id".into(),
                rid.as_str()
                    .map(|s| json!(map_req(s)))
                    .unwrap_or(Value::Null),
            );
        }
    })?;
    rewrite("questions", &|v| {
        if let Some(obj) = v.as_object_mut() {
            let rid = obj.remove("request_id");
            let att = obj.remove("attempt").and_then(|n| n.as_u64());
            let aid = rid.as_ref().and_then(Value::as_str).map(|s| {
                let candidate = att.filter(|n| *n > 0).map(|n| format!("{s}:{n}"));
                if let Some(c) = candidate
                    && req_to_attempt.values().any(|(mapped, _)| mapped == &c)
                {
                    c
                } else {
                    map_req(s)
                }
            });
            obj.insert(
                "attempt_id".into(),
                aid.map(|s| json!(s)).unwrap_or(Value::Null),
            );
        }
    })?;
    rewrite("reports", &|v| {
        if let Some(obj) = v.as_object_mut() {
            let rid = obj.remove("request_id");
            let att = obj.remove("attempt").and_then(|n| n.as_u64());
            let actor = rid
                .as_ref()
                .and_then(Value::as_str)
                .and_then(|s| req_by_id.get(s))
                .and_then(|r| r["profile"]["id"].as_str())
                .map(|p| format!("agent:{p}"))
                .unwrap_or_else(|| "agent".into());
            let aid = rid.as_ref().and_then(Value::as_str).map(|s| {
                let candidate = att.filter(|n| *n > 0).map(|n| format!("{s}:{n}"));
                if let Some(c) = candidate
                    && req_to_attempt.values().any(|(mapped, _)| mapped == &c)
                {
                    c
                } else {
                    map_req(s)
                }
            });
            obj.entry("actor".to_owned())
                .or_insert_with(|| json!(actor));
            obj.insert(
                "attempt_id".into(),
                aid.map(|s| json!(s)).unwrap_or(Value::Null),
            );
        }
    })?;
    rewrite("events", &|v| {
        if let Some(obj) = v.as_object_mut()
            && let Some(rid) = obj.remove("request_id")
            && let Some(s) = rid.as_str()
            && let Some(data) = obj.get_mut("data").and_then(Value::as_object_mut)
        {
            data.entry("attempt_id".to_owned())
                .or_insert_with(|| json!(map_req(s)));
        }
    })?;
    rewrite("note_events", &|v| {
        if let Some(obj) = v.as_object_mut() {
            obj.remove("request_id");
            obj.remove("attempt");
        }
    })?;
    tx.execute_batch("DROP INDEX IF EXISTS requests_scope; DROP TABLE requests;")?;
    Ok(())
}
fn remove_requests_down(tx: &Transaction<'_>) -> Result<()> {
    use rusqlite::params;
    use serde_json::{Value, json};
    tx.execute_batch(
        "CREATE TABLE requests (id TEXT PRIMARY KEY, project_id TEXT, card_id TEXT, state TEXT NOT NULL, number INTEGER NOT NULL, data TEXT NOT NULL, FOREIGN KEY(project_id) REFERENCES projects(id), FOREIGN KEY(card_id) REFERENCES cards(id)); CREATE INDEX requests_scope ON requests(project_id,card_id,state);",
    )?;
    let att_rows: Vec<TableRow> = {
        let mut q = tx.prepare(
            "SELECT id, project_id, card_id, state, number, data FROM attempts ORDER BY rowid",
        )?;
        q.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<std::result::Result<_, _>>()?
    };
    for (aid, project_id, card_id, state, num, raw) in att_rows {
        let mut att: Value = serde_json::from_str(&raw)?;
        let req = json!({
            "id": aid,
            "project_id": att["project_id"],
            "card_id": att["card_id"],
            "workflow_run_id": att.get("workflow_run_id").cloned().unwrap_or(Value::Null),
            "workflow_step": att.get("workflow_step").cloned().unwrap_or(Value::Null),
            "location": att.get("location").cloned().unwrap_or(Value::Null),
            "resume_from": att.get("resume_from").cloned().unwrap_or(Value::Null),
            "trigger": att.get("trigger").cloned().unwrap_or_else(|| json!("workflow")),
            "stage": att.get("stage").cloned().unwrap_or_else(|| json!("")),
            "profile": att.get("profile").cloned().unwrap_or_else(|| json!({})),
            "instruction": att.get("instruction").cloned().unwrap_or_else(|| json!("")),
            "on_completed": att.get("on_completed").cloned().unwrap_or(Value::Null),
            "require_human_review": att.get("require_human_review").cloned().unwrap_or(json!(false)),
            "state": state,
            "attempt": num.max(0),
            "created_at": att["created_at"],
        });
        tx.execute(
            "INSERT INTO requests(id, project_id, card_id, state, number, data) VALUES(?1, ?2, ?3, ?4, 0, ?5)",
            params![aid, project_id, card_id, state, serde_json::to_string(&req)?],
        )?;
        if num == 0 && att["terminal"].is_null() && att["cwd"].as_str() == Some("") {
            tx.execute("DELETE FROM attempts WHERE id=?1", params![aid])?;
        } else {
            if let Some(obj) = att.as_object_mut() {
                obj.insert("request_id".into(), json!(aid));
            }
            tx.execute(
                "UPDATE attempts SET data=?2 WHERE id=?1",
                params![aid, serde_json::to_string(&att)?],
            )?;
        }
    }
    let rewrite = |table: &str, f: &dyn Fn(&mut Value)| -> Result<()> {
        let rows: Vec<(String, String)> = {
            let mut q = tx.prepare(&format!("SELECT id, data FROM {table}"))?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (id, raw) in rows {
            let mut v: Value = serde_json::from_str(&raw)?;
            let before = v.clone();
            f(&mut v);
            if v != before {
                tx.execute(
                    &format!("UPDATE {table} SET data=?2 WHERE id=?1"),
                    params![id, serde_json::to_string(&v)?],
                )?;
            }
        }
        Ok(())
    };
    rewrite("workflow_runs", &|v| {
        if let Some(obj) = v.as_object_mut() {
            let aid = obj.remove("attempt_id").unwrap_or(Value::Null);
            obj.insert("request_id".into(), aid);
        }
    })?;
    rewrite("cards", &|v| {
        if let Some(gate) = v.get_mut("human_gate").and_then(Value::as_object_mut) {
            let aid = gate.remove("attempt_id").unwrap_or(Value::Null);
            gate.insert("request_id".into(), aid);
        }
    })?;
    rewrite("conversations", &|v| {
        if let Some(obj) = v.as_object_mut() {
            let aid = obj.remove("last_attempt_id").unwrap_or(Value::Null);
            obj.insert("last_request_id".into(), aid);
        }
    })?;
    rewrite("questions", &|v| {
        if let Some(obj) = v.as_object_mut() {
            let aid = obj
                .remove("attempt_id")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            obj.insert("request_id".into(), json!(aid));
            obj.insert("attempt".into(), json!(1));
        }
    })?;
    rewrite("reports", &|v| {
        if let Some(obj) = v.as_object_mut() {
            let aid = obj
                .remove("attempt_id")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            obj.insert("request_id".into(), json!(aid));
            obj.insert("attempt".into(), json!(1));
        }
    })?;
    rewrite("events", &|v| {
        if let Some(obj) = v.as_object_mut() {
            let aid = obj
                .get("data")
                .and_then(|d| d.get("attempt_id"))
                .cloned()
                .unwrap_or(Value::Null);
            obj.insert("request_id".into(), aid);
        }
    })?;
    rewrite("note_events", &|v| {
        if let Some(obj) = v.as_object_mut() {
            obj.insert("request_id".into(), Value::Null);
            obj.insert("attempt".into(), Value::Null);
        }
    })?;
    Ok(())
}
fn remove_attempts_up(tx: &Transaction<'_>) -> Result<()> {
    use rusqlite::params;
    use serde_json::{Value, json};
    let has_attempts: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='attempts')",
        [],
        |r| r.get(0),
    )?;
    let mut attempt_to_terminal = std::collections::HashMap::<String, String>::new();
    if has_attempts {
        let att_rows: Vec<(String, Option<String>, Option<String>, String)> = {
            let mut q =
                tx.prepare("SELECT id, project_id, card_id, data FROM attempts ORDER BY rowid")?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        let rank = |att: &Value| -> u8 {
            let state = att["state"].as_str().unwrap_or("");
            let conn = att["connection_state"].as_str().unwrap_or("");
            let has_terminal = !att.get("terminal").is_none_or(Value::is_null);
            let finished = matches!(state, "continued" | "cancelled" | "superseded")
                || conn == "missing"
                || (!has_terminal && matches!(state, "failed" | "completed" | "backlog"))
                || att
                    .get("observed")
                    .is_some_and(|v| v["state"] == "finished");
            if has_terminal && !finished {
                3
            } else if matches!(state, "queued" | "starting" | "unknown") {
                2
            } else {
                1
            }
        };
        let mut grouped = std::collections::BTreeMap::<
            String,
            Vec<(String, Option<String>, Option<String>, Value)>,
        >::new();
        for (aid, proj, card_col, raw) in att_rows {
            let att: Value = serde_json::from_str(&raw)?;
            let card_id = att["card_id"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| card_col.clone())
                .unwrap_or_default();
            let profile_id = att["profile"]["id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    att["conversation_id"]
                        .as_str()
                        .and_then(|c| c.split_once(':').map(|(_, p)| p.to_owned()))
                })
                .unwrap_or_else(|| "developer".into());
            let conv_id = att["conversation_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{card_id}:{profile_id}"));
            attempt_to_terminal.insert(aid.clone(), conv_id.clone());
            grouped
                .entry(conv_id)
                .or_default()
                .push((aid, proj, card_col, att));
        }
        let next_counter = |key: &str| -> Result<i64> {
            tx.execute(
                "INSERT INTO counters(id,value) VALUES(?1,1) ON CONFLICT(id) DO UPDATE SET value=value+1",
                [key],
            )?;
            Ok(
                tx.query_row("SELECT value FROM counters WHERE id=?1", [key], |r| {
                    r.get(0)
                })?,
            )
        };
        for (conv_id, items) in grouped {
            let mut chosen_idx = 0usize;
            let mut best_rank = 0u8;
            for (idx, (_, _, _, att)) in items.iter().enumerate() {
                let r = rank(att);
                if r >= best_rank {
                    best_rank = r;
                    chosen_idx = idx;
                }
            }
            let (_, proj_col, card_col, mut chosen) = items[chosen_idx].clone();
            let project_id = chosen["project_id"]
                .as_str()
                .map(str::to_owned)
                .or(proj_col);
            let card_id = chosen["card_id"].as_str().map(str::to_owned).or(card_col);
            let profile_id = chosen["profile"]["id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| conv_id.split_once(':').map(|(_, p)| p.to_owned()))
                .unwrap_or_else(|| "developer".into());
            let provider = chosen["profile"]["preset"]
                .as_str()
                .unwrap_or("codex")
                .to_owned();
            let created_at = chosen["created_at"].as_u64().unwrap_or(0);
            let resume_session = chosen
                .get("resume_from")
                .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()));
            if let Some(obj) = chosen.as_object_mut() {
                obj.remove("id");
                obj.remove("conversation_id");
                obj.remove("resume_from");
                let created = obj
                    .remove("created_at")
                    .unwrap_or_else(|| json!(created_at));
                obj.entry("updated_at".to_owned()).or_insert(created);
                obj.insert("resume_session".into(), json!(resume_session));
                if obj.get("state").and_then(Value::as_str) == Some("continued") {
                    obj.insert("state".into(), json!("completed"));
                }
            }
            let state = chosen["state"].as_str().unwrap_or("").to_owned();
            let has_conv: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=?1)",
                [&conv_id],
                |r| r.get(0),
            )?;
            if !has_conv && let (Some(pid), Some(cid)) = (project_id.as_deref(), card_id.as_deref())
            {
                let conv = json!({
                    "id": conv_id,
                    "project_id": pid,
                    "card_id": cid,
                    "profile_id": profile_id,
                    "provider": provider,
                    "provider_session_id": chosen.get("provider_session_id").cloned().unwrap_or(Value::Null),
                    "created_at": created_at,
                });
                tx.execute(
                    "INSERT INTO conversations(id, project_id, card_id, state, number, data) VALUES(?1, ?2, ?3, '', 0, ?4)",
                    params![conv_id, pid, cid, serde_json::to_string(&conv)?],
                )?;
            }
            let existing_term: Option<(i64, String)> = tx
                .query_row(
                    "SELECT number, data FROM terminals WHERE id=?1",
                    [&conv_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();
            if let Some((num, raw_term)) = existing_term {
                let mut term: Value = serde_json::from_str(&raw_term)?;
                if let Some(obj) = term.as_object_mut() {
                    obj.insert("conversation_id".into(), json!(conv_id));
                    obj.insert("agent".into(), chosen);
                }
                tx.execute(
                    "UPDATE terminals SET project_id=?2, card_id=?3, state=?4, number=?5, data=?6 WHERE id=?1",
                    params![
                        conv_id,
                        project_id,
                        card_id,
                        state,
                        num,
                        serde_json::to_string(&term)?
                    ],
                )?;
            } else {
                let num = next_counter("terminal-number")?;
                let term = json!({
                    "id": conv_id,
                    "conversation_id": conv_id,
                    "number": num,
                    "name": Value::Null,
                    "manual_number": Value::Null,
                    "runtime": Value::Null,
                    "agent": chosen,
                    "created_at": created_at,
                });
                tx.execute(
                    "INSERT INTO terminals(id, project_id, card_id, state, number, data) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        conv_id,
                        project_id,
                        card_id,
                        state,
                        num,
                        serde_json::to_string(&term)?
                    ],
                )?;
            }
        }
    }
    let map_att = |aid: &str| -> String {
        attempt_to_terminal
            .get(aid)
            .cloned()
            .unwrap_or_else(|| aid.to_owned())
    };
    let rewrite = |table: &str, f: &dyn Fn(&mut Value)| -> Result<()> {
        let rows: Vec<(String, String)> = {
            let mut q = tx.prepare(&format!("SELECT id, data FROM {table}"))?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (id, raw) in rows {
            let mut v: Value = serde_json::from_str(&raw)?;
            let before = v.clone();
            f(&mut v);
            if v != before {
                tx.execute(
                    &format!("UPDATE {table} SET data=?2 WHERE id=?1"),
                    params![id, serde_json::to_string(&v)?],
                )?;
            }
        }
        Ok(())
    };
    rewrite("workflow_runs", &|v| {
        if let Some(obj) = v.as_object_mut()
            && let Some(aid) = obj.remove("attempt_id")
        {
            obj.insert(
                "terminal_id".into(),
                aid.as_str()
                    .map(|s| json!(map_att(s)))
                    .unwrap_or(Value::Null),
            );
        }
    })?;
    rewrite("cards", &|v| {
        if let Some(gate) = v.get_mut("human_gate").and_then(Value::as_object_mut)
            && let Some(aid) = gate.remove("attempt_id")
        {
            gate.insert(
                "terminal_id".into(),
                aid.as_str()
                    .map(|s| json!(map_att(s)))
                    .unwrap_or(Value::Null),
            );
        }
    })?;
    rewrite("conversations", &|v| {
        if let Some(obj) = v.as_object_mut() {
            obj.remove("last_attempt_id");
        }
    })?;
    rewrite("questions", &|v| {
        if let Some(obj) = v.as_object_mut()
            && let Some(aid) = obj.remove("attempt_id")
        {
            obj.insert(
                "terminal_id".into(),
                aid.as_str()
                    .map(|s| json!(map_att(s)))
                    .unwrap_or(Value::Null),
            );
        }
    })?;
    rewrite("reports", &|v| {
        if let Some(obj) = v.as_object_mut()
            && let Some(aid) = obj.remove("attempt_id")
        {
            obj.insert(
                "terminal_id".into(),
                aid.as_str()
                    .map(|s| json!(map_att(s)))
                    .unwrap_or(Value::Null),
            );
        }
    })?;
    rewrite("events", &|v| {
        if let Some(data) = v.get_mut("data").and_then(Value::as_object_mut)
            && let Some(aid) = data.remove("attempt_id")
        {
            data.insert(
                "terminal_id".into(),
                aid.as_str()
                    .map(|s| json!(map_att(s)))
                    .unwrap_or(Value::Null),
            );
        }
    })?;
    rewrite("note_events", &|v| {
        if let Some(data) = v.get_mut("data").and_then(Value::as_object_mut)
            && let Some(aid) = data.remove("attempt_id")
        {
            data.insert(
                "terminal_id".into(),
                aid.as_str()
                    .map(|s| json!(map_att(s)))
                    .unwrap_or(Value::Null),
            );
        }
    })?;
    tx.execute_batch("DROP INDEX IF EXISTS attempts_scope; DROP TABLE IF EXISTS attempts;")?;
    Ok(())
}
fn remove_attempts_down(tx: &Transaction<'_>) -> Result<()> {
    use rusqlite::params;
    use serde_json::{Value, json};
    tx.execute_batch(
        "CREATE TABLE attempts (id TEXT PRIMARY KEY, project_id TEXT, card_id TEXT, state TEXT NOT NULL, number INTEGER NOT NULL, data TEXT NOT NULL, FOREIGN KEY(project_id) REFERENCES projects(id), FOREIGN KEY(card_id) REFERENCES cards(id)); CREATE INDEX attempts_scope ON attempts(project_id,card_id,state);",
    )?;
    let term_rows: Vec<TableRow> = {
        let mut q = tx.prepare(
            "SELECT id, project_id, card_id, state, number, data FROM terminals ORDER BY rowid",
        )?;
        q.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<std::result::Result<_, _>>()?
    };
    let mut conv_with_attempt = std::collections::HashSet::<String>::new();
    for (tid, proj_col, card_col, _, term_num, raw) in term_rows {
        let mut term: Value = serde_json::from_str(&raw)?;
        let Some(mut agent) = term
            .as_object_mut()
            .and_then(|o| o.remove("agent"))
            .filter(|v| v.is_object())
        else {
            continue;
        };
        let project_id = agent["project_id"].as_str().map(str::to_owned).or(proj_col);
        let card_id = agent["card_id"].as_str().map(str::to_owned).or(card_col);
        let state = agent["state"].as_str().unwrap_or("").to_owned();
        let num = agent["number"].as_i64().unwrap_or(1);
        let created_at = agent["updated_at"]
            .as_u64()
            .or_else(|| term["created_at"].as_u64())
            .unwrap_or(0);
        if let Some(obj) = agent.as_object_mut() {
            obj.remove("updated_at");
            obj.remove("pending");
            let resume_session = obj
                .remove("resume_session")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            obj.insert("id".into(), json!(tid));
            obj.insert("conversation_id".into(), json!(tid));
            obj.insert("created_at".into(), json!(created_at));
            obj.insert(
                "resume_from".into(),
                if resume_session {
                    json!(tid)
                } else {
                    Value::Null
                },
            );
        }
        tx.execute(
            "INSERT INTO attempts(id, project_id, card_id, state, number, data) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![tid, project_id, card_id, state, num, serde_json::to_string(&agent)?],
        )?;
        conv_with_attempt.insert(tid.clone());
        tx.execute(
            "UPDATE terminals SET project_id=NULL, card_id=NULL, state='', number=?2, data=?3 WHERE id=?1",
            params![tid, term_num, serde_json::to_string(&term)?],
        )?;
    }
    let rewrite = |table: &str, f: &dyn Fn(&mut Value)| -> Result<()> {
        let rows: Vec<(String, String)> = {
            let mut q = tx.prepare(&format!("SELECT id, data FROM {table}"))?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (id, raw) in rows {
            let mut v: Value = serde_json::from_str(&raw)?;
            let before = v.clone();
            f(&mut v);
            if v != before {
                tx.execute(
                    &format!("UPDATE {table} SET data=?2 WHERE id=?1"),
                    params![id, serde_json::to_string(&v)?],
                )?;
            }
        }
        Ok(())
    };
    rewrite("workflow_runs", &|v| {
        if let Some(obj) = v.as_object_mut() {
            let tid = obj.remove("terminal_id").unwrap_or(Value::Null);
            obj.insert("attempt_id".into(), tid);
        }
    })?;
    rewrite("cards", &|v| {
        if let Some(gate) = v.get_mut("human_gate").and_then(Value::as_object_mut) {
            let tid = gate.remove("terminal_id").unwrap_or(Value::Null);
            gate.insert("attempt_id".into(), tid);
        }
    })?;
    rewrite("conversations", &|v| {
        if let Some(obj) = v.as_object_mut() {
            let id = obj
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_default();
            obj.insert(
                "last_attempt_id".into(),
                if conv_with_attempt.contains(&id) {
                    json!(id)
                } else {
                    Value::Null
                },
            );
        }
    })?;
    rewrite("questions", &|v| {
        if let Some(obj) = v.as_object_mut() {
            let tid = obj.remove("terminal_id").unwrap_or(Value::Null);
            obj.insert("attempt_id".into(), tid);
        }
    })?;
    rewrite("reports", &|v| {
        if let Some(obj) = v.as_object_mut() {
            let tid = obj.remove("terminal_id").unwrap_or(Value::Null);
            obj.insert("attempt_id".into(), tid);
        }
    })?;
    rewrite("events", &|v| {
        if let Some(data) = v.get_mut("data").and_then(Value::as_object_mut)
            && let Some(tid) = data.remove("terminal_id")
        {
            data.insert("attempt_id".into(), tid);
        }
    })?;
    rewrite("note_events", &|v| {
        if let Some(data) = v.get_mut("data").and_then(Value::as_object_mut)
            && let Some(tid) = data.remove("terminal_id")
        {
            data.insert("attempt_id".into(), tid);
        }
    })?;
    Ok(())
}

fn is_legacy_32hex(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get(0),
    )?)
}

fn allocate_short_id(
    kind: &str,
    old_id: &str,
    existing: &HashMap<(String, String), String>,
    used: &mut HashSet<String>,
) -> String {
    if let Some(prev) = existing.get(&(kind.to_owned(), old_id.to_owned())) {
        used.insert(prev.clone());
        return prev.clone();
    }
    let prefix = old_id[..8].to_ascii_lowercase();
    if used.insert(prefix.clone()) {
        return prefix;
    }
    for attempt in 1u64.. {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in kind
            .bytes()
            .chain(b":".iter().copied())
            .chain(old_id.bytes())
            .chain(attempt.to_le_bytes())
        {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
        let cand = format!("{:08x}", (h as u32) ^ ((h >> 32) as u32));
        if used.insert(cand.clone()) {
            return cand;
        }
    }
    unreachable!()
}

fn remap_composite_id(s: &str, map: &HashMap<String, String>) -> String {
    if let Some(mapped) = map.get(s) {
        return mapped.clone();
    }
    if !s.contains(':') {
        return s.to_owned();
    }
    let mut changed = false;
    let parts: Vec<&str> = s.split(':').collect();
    let out: Vec<String> = parts
        .iter()
        .map(|seg| {
            if let Some(mapped) = map.get(*seg) {
                changed = true;
                mapped.clone()
            } else {
                (*seg).to_owned()
            }
        })
        .collect();
    if changed { out.join(":") } else { s.to_owned() }
}

fn rewrite_rel_path(
    raw: &str,
    row_card_id: Option<&str>,
    id_map: &HashMap<String, String>,
    card_numbers: &HashMap<String, u64>,
    path_map: Option<&mut HashMap<String, String>>,
) -> String {
    let parts: Vec<&str> = raw.split('/').collect();
    let mut new_parts: Vec<String> = parts.iter().map(|s| (*s).to_owned()).collect();
    let mut changed = false;

    if parts.len() >= 4 && parts[0] == "projects" && parts[2] == "cards" {
        if let Some(new_pid) = id_map.get(parts[1]) {
            new_parts[1] = new_pid.clone();
            changed = true;
        }
        let num = card_numbers
            .get(parts[3])
            .copied()
            .or_else(|| row_card_id.and_then(|cid| card_numbers.get(cid).copied()));
        if let Some(num) = num {
            let num_str = num.to_string();
            if parts[3] != num_str
                && (card_numbers.contains_key(parts[3]) || row_card_id == Some(parts[3]))
            {
                new_parts[3] = num_str;
                changed = true;
            }
        }
        let rest_len = parts.len();
        if rest_len >= 6 && parts[rest_len - 2] == "reports" {
            let file = parts[rest_len - 1];
            if let Some(stem) = file.strip_suffix(".md")
                && let Some(new_id) = id_map.get(stem)
            {
                new_parts[rest_len - 1] = format!("{new_id}.md");
                changed = true;
            }
        }
        if parts.len() == 6 && parts[4] == "resources" {
            let file = parts[5];
            let (stem, suffix) = match file.find('.') {
                Some(idx) => (&file[..idx], &file[idx..]),
                None => (file, ""),
            };
            if let Some(new_id) = id_map.get(stem) {
                new_parts[5] = format!("{new_id}{suffix}");
                changed = true;
            }
        }
    } else if parts.len() >= 3 && parts[0] == "archives" {
        if let Some(new_pid) = id_map.get(parts[1]) {
            new_parts[1] = new_pid.clone();
            changed = true;
        }
        let num = card_numbers
            .get(parts[2])
            .copied()
            .or_else(|| row_card_id.and_then(|cid| card_numbers.get(cid).copied()));
        if let Some(num) = num {
            let num_str = num.to_string();
            if parts[2] != num_str
                && (card_numbers.contains_key(parts[2]) || row_card_id == Some(parts[2]))
            {
                new_parts[2] = num_str;
                changed = true;
            }
        }
    } else if parts.len() >= 2
        && parts[0] == "projects"
        && let Some(new_pid) = id_map.get(parts[1])
    {
        new_parts[1] = new_pid.clone();
        changed = true;
    }

    if !changed {
        return raw.to_owned();
    }
    let rewritten = new_parts.join("/");
    if let Some(map) = path_map {
        map.entry(raw.to_owned())
            .or_insert_with(|| rewritten.clone());
    }
    rewritten
}

const ID_JSON_KEYS: &[&str] = &[
    "id",
    "project_id",
    "card_id",
    "terminal_id",
    "conversation_id",
    "workflow_run_id",
    "revision_id",
    "report_id",
    "artifact_id",
    "event_id",
    "comment_id",
    "question_id",
    "answer_event_id",
    "trigger_event_id",
    "reply_to",
    "subscription_id",
    "run_id",
    "note_id",
    "target_id",
];
const ID_ARRAY_JSON_KEYS: &[&str] = &["linked_cards", "card_ids", "links"];
const PATH_JSON_KEYS: &[&str] = &["input_path", "output_path", "path", "reply_path", "archive"];

fn rewrite_json_fields(
    v: &mut Value,
    parent_key: Option<&str>,
    row_card_id: Option<&str>,
    id_map: &HashMap<String, String>,
    card_numbers: &HashMap<String, u64>,
    mut path_map: Option<&mut HashMap<String, String>>,
) {
    match v {
        Value::String(s) => {
            if let Some(k) = parent_key {
                if PATH_JSON_KEYS.contains(&k) {
                    *s = rewrite_rel_path(
                        s,
                        row_card_id,
                        id_map,
                        card_numbers,
                        path_map.as_deref_mut(),
                    );
                    return;
                }
                if ID_JSON_KEYS.contains(&k) {
                    let remapped = remap_composite_id(s, id_map);
                    if remapped != *s {
                        *s = remapped;
                    }
                    return;
                }
            }
            if (s.starts_with("projects/") || s.starts_with("archives/")) && s.contains('/') {
                *s = rewrite_rel_path(s, row_card_id, id_map, card_numbers, path_map);
            }
        }
        Value::Array(arr) => {
            let in_id_arr = parent_key.is_some_and(|k| ID_ARRAY_JSON_KEYS.contains(&k));
            for item in arr {
                if in_id_arr {
                    if let Value::String(s) = item {
                        let remapped = remap_composite_id(s, id_map);
                        if remapped != *s {
                            *s = remapped;
                        }
                    }
                } else {
                    rewrite_json_fields(
                        item,
                        None,
                        row_card_id,
                        id_map,
                        card_numbers,
                        path_map.as_deref_mut(),
                    );
                }
            }
        }
        Value::Object(obj) => {
            for (k, val) in obj.iter_mut() {
                if matches!(k.as_str(), "definition" | "source_yaml") {
                    continue;
                }
                rewrite_json_fields(
                    val,
                    Some(k.as_str()),
                    row_card_id,
                    id_map,
                    card_numbers,
                    path_map.as_deref_mut(),
                );
            }
        }
        _ => {}
    }
}

fn rewrite_json_exact_inverse(
    v: &mut Value,
    parent_key: Option<&str>,
    id_rev: &HashMap<String, String>,
    path_rev: &HashMap<String, String>,
) {
    match v {
        Value::String(s) => {
            if let Some(old_rel) = path_rev.get(s.as_str()) {
                *s = old_rel.clone();
                return;
            }
            if parent_key.is_some_and(|k| ID_JSON_KEYS.contains(&k)) {
                let remapped = remap_composite_id(s, id_rev);
                if remapped != *s {
                    *s = remapped;
                }
            }
        }
        Value::Array(arr) => {
            let in_id_arr = parent_key.is_some_and(|k| ID_ARRAY_JSON_KEYS.contains(&k));
            for item in arr {
                if in_id_arr {
                    if let Value::String(s) = item {
                        let remapped = remap_composite_id(s, id_rev);
                        if remapped != *s {
                            *s = remapped;
                        }
                    }
                } else {
                    rewrite_json_exact_inverse(item, None, id_rev, path_rev);
                }
            }
        }
        Value::Object(obj) => {
            for (k, val) in obj.iter_mut() {
                if matches!(k.as_str(), "definition" | "source_yaml") {
                    continue;
                }
                rewrite_json_exact_inverse(val, Some(k.as_str()), id_rev, path_rev);
            }
        }
        _ => {}
    }
}

const ENTITY_TABLES: &[(&str, &str)] = &[
    ("projects", "project"),
    ("cards", "card"),
    ("events", "event"),
    ("reports", "report"),
    ("artifacts", "artifact"),
    ("notes", "note"),
    ("notifications", "notice"),
    ("questions", "question"),
    ("workflow_revisions", "workflow_revision"),
    ("workflow_runs", "workflow_run"),
    ("workflow_bindings", "workflow_binding"),
    ("workflow_templates", "workflow_template"),
    ("event_subscriptions", "event_subscription"),
    ("event_deliveries", "event_delivery"),
    ("managed_worktrees", "managed_worktree"),
    ("github_issues", "github_issue"),
    ("note_events", "note_event"),
    ("source_observations", "source_observation"),
];

const CHILD_SCOPED_TABLES: &[(&str, &str)] = &[
    ("conversations", "conversation"),
    ("terminals", "terminal"),
    ("reports", "report"),
    ("artifacts", "artifact"),
    ("questions", "question"),
    ("notifications", "notice"),
    ("card_reads", "card_read"),
    ("github_issues", "github_issue"),
    ("notes", "note"),
    ("note_revisions", "note_revision"),
    ("note_events", "note_event"),
    ("workflow_sources", "workflow_source"),
    ("workflow_revisions", "workflow_revision"),
    ("workflow_runs", "workflow_run"),
    ("workflow_bindings", "workflow_binding"),
    ("workflow_templates", "workflow_template"),
    ("event_subscriptions", "event_subscription"),
    ("event_deliveries", "event_delivery"),
    ("configuration_sessions", "configuration_session"),
    ("managed_worktrees", "managed_worktree"),
];

type ShortIdTableRow = (String, Option<String>, Option<String>, String, i64, String);

fn load_short_id_rows(tx: &Transaction<'_>, table: &str) -> Result<Vec<ShortIdTableRow>> {
    let mut q = tx.prepare(&format!(
        "SELECT id, project_id, card_id, state, number, data FROM {table}"
    ))?;
    Ok(q.query_map([], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
        ))
    })?
    .collect::<std::result::Result<_, _>>()?)
}

fn short_ids_up(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS id_migrations(kind TEXT NOT NULL, old_id TEXT NOT NULL, new_id TEXT NOT NULL, PRIMARY KEY(kind, old_id));
         CREATE INDEX IF NOT EXISTS id_migrations_new ON id_migrations(kind, new_id);
         CREATE TABLE IF NOT EXISTS path_migrations(old_rel TEXT PRIMARY KEY, new_rel TEXT NOT NULL);",
    )?;

    let mut existing_map: HashMap<(String, String), String> = HashMap::new();
    let mut used: HashSet<String> = HashSet::new();
    {
        let mut q = tx.prepare("SELECT kind, old_id, new_id FROM id_migrations")?;
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (kind, old_id, new_id) = row?;
            used.insert(old_id.clone());
            used.insert(new_id.clone());
            existing_map.insert((kind, old_id), new_id);
        }
    }
    for (table, _) in ENTITY_TABLES
        .iter()
        .chain(CHILD_SCOPED_TABLES.iter())
        .chain(
            [&("source_observations", "source_observation")]
                .iter()
                .copied(),
        )
    {
        if !table_exists(tx, table)? {
            continue;
        }
        let mut q = tx.prepare(&format!("SELECT id FROM {table}"))?;
        for id in q.query_map([], |r| r.get::<_, String>(0))? {
            used.insert(id?);
        }
    }

    let mut new_mappings: Vec<(String, String, String)> = Vec::new();
    let mut id_map: HashMap<String, String> = HashMap::new();
    for ((_, old_id), new_id) in &existing_map {
        id_map.insert(old_id.clone(), new_id.clone());
    }

    for (table, kind) in ENTITY_TABLES {
        if !table_exists(tx, table)? {
            continue;
        }
        let ids: Vec<String> = {
            let mut q = tx.prepare(&format!("SELECT id FROM {table} ORDER BY rowid"))?;
            q.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        for old_id in ids {
            if is_legacy_32hex(&old_id) {
                let new_id = allocate_short_id(kind, &old_id, &existing_map, &mut used);
                id_map.insert(old_id.clone(), new_id.clone());
                new_mappings.push(((*kind).to_owned(), old_id, new_id));
            }
        }
    }

    // Collect card numbers (both old and new card IDs -> card number).
    let mut card_numbers: HashMap<String, u64> = HashMap::new();
    {
        let mut q = tx.prepare("SELECT id, number FROM cards")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))? {
            let (cid, num) = row?;
            card_numbers.insert(cid.clone(), num);
            if let Some(new_cid) = id_map.get(&cid) {
                card_numbers.insert(new_cid.clone(), num);
            }
        }
    }

    // Also pre-collect composite terminal IDs and question/conversation IDs so cards/events/reports can remap terminal_id accurately.
    for (table, kind) in [
        ("terminals", "terminal"),
        ("conversations", "conversation"),
        ("questions", "question"),
        ("note_revisions", "note_revision"),
        ("workflow_sources", "workflow_source"),
        ("github_issues", "github_issue"),
        ("configuration_sessions", "configuration_session"),
        ("card_reads", "card_read"),
    ] {
        if !table_exists(tx, table)? {
            continue;
        }
        let ids: Vec<String> = {
            let mut q = tx.prepare(&format!("SELECT id FROM {table} ORDER BY rowid"))?;
            q.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        for old_id in ids {
            if !is_legacy_32hex(&old_id) {
                let new_id = remap_composite_id(&old_id, &id_map);
                if new_id != old_id {
                    id_map.insert(old_id.clone(), new_id.clone());
                    new_mappings.push(((*kind).to_owned(), old_id, new_id));
                }
            }
        }
    }

    let mut path_map: HashMap<String, String> = HashMap::new();
    {
        let mut q = tx.prepare("SELECT old_rel, new_rel FROM path_migrations")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (o, n) = row?;
            path_map.insert(o, n);
        }
    }

    tx.execute_batch(
        "DROP INDEX IF EXISTS card_numbers;
         DROP INDEX IF EXISTS github_card_source;
         DROP INDEX IF EXISTS github_source_card;
         DROP INDEX IF EXISTS event_order;",
    )?;

    // 1. Insert new project rows (or update in-place if ID stayed unchanged).
    let project_rows = load_short_id_rows(tx, "projects")?;
    let mut old_project_ids = Vec::new();
    for (old_id, proj_col, card_col, state, number, raw) in project_rows {
        let new_id = id_map
            .get(&old_id)
            .cloned()
            .unwrap_or_else(|| old_id.clone());
        let mut v: Value = serde_json::from_str(&raw)?;
        rewrite_json_fields(
            &mut v,
            None,
            None,
            &id_map,
            &card_numbers,
            Some(&mut path_map),
        );
        let new_data = serde_json::to_string(&v)?;
        if new_id != old_id {
            tx.execute(
                "INSERT INTO projects(id, project_id, card_id, state, number, data) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                params![new_id, proj_col, card_col, state, number, new_data],
            )?;
            old_project_ids.push(old_id);
        } else if new_data != raw {
            tx.execute(
                "UPDATE projects SET data=?2 WHERE id=?1",
                params![old_id, new_data],
            )?;
        }
    }

    // 2. Insert new card rows (referencing new projects) or update in place.
    let card_rows = load_short_id_rows(tx, "cards")?;
    let mut old_card_ids = Vec::new();
    for (old_id, proj_col, card_col, state, number, raw) in card_rows {
        let new_id = id_map
            .get(&old_id)
            .cloned()
            .unwrap_or_else(|| old_id.clone());
        let new_proj = proj_col
            .as_ref()
            .map(|p| id_map.get(p).cloned().unwrap_or_else(|| p.clone()));
        let mut v: Value = serde_json::from_str(&raw)?;
        rewrite_json_fields(
            &mut v,
            None,
            Some(&old_id),
            &id_map,
            &card_numbers,
            Some(&mut path_map),
        );
        let new_data = serde_json::to_string(&v)?;
        if new_id != old_id {
            tx.execute(
                "INSERT INTO cards(id, project_id, card_id, state, number, data) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                params![new_id, new_proj, card_col, state, number, new_data],
            )?;
            old_card_ids.push(old_id);
        } else {
            tx.execute(
                "UPDATE cards SET project_id=?2, data=?3 WHERE id=?1",
                params![old_id, new_proj, new_data],
            )?;
        }
    }

    // 3. Migrate events + event_journal.
    let event_rows = load_short_id_rows(tx, "events")?;
    for (old_id, proj_col, card_col, state, number, raw) in event_rows {
        let new_id = id_map
            .get(&old_id)
            .cloned()
            .unwrap_or_else(|| old_id.clone());
        let new_proj = proj_col
            .as_ref()
            .map(|p| id_map.get(p).cloned().unwrap_or_else(|| p.clone()));
        let new_card = card_col
            .as_ref()
            .map(|c| id_map.get(c).cloned().unwrap_or_else(|| c.clone()));
        let mut v: Value = serde_json::from_str(&raw)?;
        rewrite_json_fields(
            &mut v,
            None,
            card_col.as_deref(),
            &id_map,
            &card_numbers,
            Some(&mut path_map),
        );
        let new_data = serde_json::to_string(&v)?;
        if new_id != old_id {
            tx.execute(
                "INSERT INTO events(id, project_id, card_id, state, number, data) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                params![new_id, new_proj, new_card, state, number, new_data],
            )?;
            tx.execute(
                "UPDATE event_journal SET event_id=?2 WHERE event_id=?1",
                params![old_id, new_id],
            )?;
            tx.execute("DELETE FROM events WHERE id=?1", [old_id])?;
        } else {
            tx.execute(
                "UPDATE events SET project_id=?2, card_id=?3, data=?4 WHERE id=?1",
                params![old_id, new_proj, new_card, new_data],
            )?;
        }
    }

    // 4. Update child-scoped tables in place.
    for (table, _) in CHILD_SCOPED_TABLES {
        if !table_exists(tx, table)? {
            continue;
        }
        let rows: Vec<(String, Option<String>, Option<String>, String)> = {
            let mut q = tx.prepare(&format!(
                "SELECT id, project_id, card_id, data FROM {table}"
            ))?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (old_id, proj_col, card_col, raw) in rows {
            let new_id = id_map
                .get(&old_id)
                .cloned()
                .unwrap_or_else(|| remap_composite_id(&old_id, &id_map));
            let new_proj = proj_col
                .as_ref()
                .map(|p| id_map.get(p).cloned().unwrap_or_else(|| p.clone()));
            let new_card = card_col
                .as_ref()
                .map(|c| id_map.get(c).cloned().unwrap_or_else(|| c.clone()));
            let mut v: Value = serde_json::from_str(&raw)?;
            rewrite_json_fields(
                &mut v,
                None,
                card_col.as_deref(),
                &id_map,
                &card_numbers,
                Some(&mut path_map),
            );
            if *table == "terminals"
                && let Some(agent) = v.get_mut("agent").and_then(Value::as_object_mut)
                && let Some(cid) = agent.get("card_id").and_then(Value::as_str)
                && let Some(&num) = card_numbers.get(cid)
            {
                agent.insert("card_number".into(), serde_json::json!(num));
            }
            let new_data = serde_json::to_string(&v)?;
            if new_id != old_id || new_proj != proj_col || new_card != card_col || new_data != raw {
                tx.execute(
                    &format!(
                        "UPDATE {table} SET id=?2, project_id=?3, card_id=?4, data=?5 WHERE id=?1"
                    ),
                    params![old_id, new_id, new_proj, new_card, new_data],
                )?;
            }
        }
    }

    // 5. Update non-scoped tables (source_observations, editor_drafts, counters, operations).
    if table_exists(tx, "source_observations")? {
        let rows: Vec<(String, String)> = {
            let mut q = tx.prepare("SELECT id, data FROM source_observations")?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (old_id, raw) in rows {
            let new_id = id_map
                .get(&old_id)
                .cloned()
                .unwrap_or_else(|| remap_composite_id(&old_id, &id_map));
            let mut v: Value = serde_json::from_str(&raw)?;
            rewrite_json_fields(
                &mut v,
                None,
                None,
                &id_map,
                &card_numbers,
                Some(&mut path_map),
            );
            let new_data = serde_json::to_string(&v)?;
            if new_id != old_id || new_data != raw {
                tx.execute(
                    "UPDATE source_observations SET id=?2, data=?3 WHERE id=?1",
                    params![old_id, new_id, new_data],
                )?;
            }
        }
    }
    if table_exists(tx, "editor_drafts")? {
        let rows: Vec<(String, String)> = {
            let mut q = tx.prepare("SELECT id, data FROM editor_drafts")?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (old_id, raw) in rows {
            let new_id = remap_composite_id(&old_id, &id_map);
            if new_id != old_id {
                new_mappings.push(("editor_draft".into(), old_id.clone(), new_id.clone()));
            }
            let mut v: Value = serde_json::from_str(&raw)?;
            rewrite_json_fields(
                &mut v,
                None,
                None,
                &id_map,
                &card_numbers,
                Some(&mut path_map),
            );
            let new_data = serde_json::to_string(&v)?;
            if new_id != old_id || new_data != raw {
                tx.execute(
                    "UPDATE editor_drafts SET id=?2, data=?3 WHERE id=?1",
                    params![old_id, new_id, new_data],
                )?;
            }
        }
    }
    if table_exists(tx, "counters")? {
        let keys: Vec<String> = {
            let mut q = tx.prepare("SELECT id FROM counters")?;
            q.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        for old_id in keys {
            let new_id = remap_composite_id(&old_id, &id_map);
            if new_id != old_id {
                new_mappings.push(("counter".into(), old_id.clone(), new_id.clone()));
                tx.execute(
                    "UPDATE counters SET id=?2 WHERE id=?1",
                    params![old_id, new_id],
                )?;
            }
        }
    }
    if table_exists(tx, "operations")? {
        let rows: Vec<(String, String)> = {
            let mut q = tx.prepare("SELECT id, result FROM operations")?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (id, raw) in rows {
            if let Ok(mut v) = serde_json::from_str::<Value>(&raw) {
                rewrite_json_fields(
                    &mut v,
                    None,
                    None,
                    &id_map,
                    &card_numbers,
                    Some(&mut path_map),
                );
                let new_res = serde_json::to_string(&v)?;
                if new_res != raw {
                    tx.execute(
                        "UPDATE operations SET result=?2 WHERE id=?1",
                        params![id, new_res],
                    )?;
                }
            }
        }
    }

    // 6. Delete superseded card and project rows, then recreate unique indexes.
    for old_cid in old_card_ids {
        tx.execute("DELETE FROM cards WHERE id=?1", [old_cid])?;
    }
    for old_pid in old_project_ids {
        tx.execute("DELETE FROM projects WHERE id=?1", [old_pid])?;
    }
    tx.execute_batch(
        "CREATE UNIQUE INDEX card_numbers ON cards(project_id, number);
         CREATE UNIQUE INDEX github_card_source ON cards(project_id, json_extract(data,'$.content.source.repository_id'), json_extract(data,'$.content.source.issue_id')) WHERE json_extract(data,'$.content.kind')='github_issue';
         CREATE UNIQUE INDEX github_source_card ON cards(project_id, json_extract(data,'$.content.source.repository_id'), json_extract(data,'$.content.source.issue_id')) WHERE json_extract(data,'$.content.kind') IN ('github_issue','github_pr');
         CREATE UNIQUE INDEX event_order ON events(card_id, number);",
    )?;

    for (kind, old_id, new_id) in new_mappings {
        tx.execute(
            "INSERT OR REPLACE INTO id_migrations(kind, old_id, new_id) VALUES(?1, ?2, ?3)",
            params![kind, old_id, new_id],
        )?;
    }
    for (old_rel, new_rel) in path_map {
        tx.execute(
            "INSERT OR REPLACE INTO path_migrations(old_rel, new_rel) VALUES(?1, ?2)",
            params![old_rel, new_rel],
        )?;
    }
    Ok(())
}

fn short_ids_down(tx: &Transaction<'_>) -> Result<()> {
    if !table_exists(tx, "id_migrations")? {
        return Ok(());
    }
    let mut rev_by_kind: HashMap<(String, String), String> = HashMap::new();
    let mut id_rev: HashMap<String, String> = HashMap::new();
    {
        let mut q = tx.prepare("SELECT kind, old_id, new_id FROM id_migrations")?;
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (kind, old_id, new_id) = row?;
            rev_by_kind.insert((kind, new_id.clone()), old_id.clone());
            id_rev.insert(new_id, old_id);
        }
    }
    let mut path_rev: HashMap<String, String> = HashMap::new();
    if table_exists(tx, "path_migrations")? {
        let mut q = tx.prepare("SELECT old_rel, new_rel FROM path_migrations")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (old_rel, new_rel) = row?;
            path_rev.insert(new_rel, old_rel);
        }
    }

    tx.execute_batch(
        "DROP INDEX IF EXISTS card_numbers;
         DROP INDEX IF EXISTS github_card_source;
         DROP INDEX IF EXISTS github_source_card;
         DROP INDEX IF EXISTS event_order;",
    )?;

    // 1. Restore old project rows.
    let project_rows = load_short_id_rows(tx, "projects")?;
    let mut drop_project_ids = Vec::new();
    for (new_id, proj_col, card_col, state, number, raw) in project_rows {
        let old_id = rev_by_kind
            .get(&("project".into(), new_id.clone()))
            .cloned()
            .unwrap_or_else(|| new_id.clone());
        let mut v: Value = serde_json::from_str(&raw)?;
        rewrite_json_exact_inverse(&mut v, None, &id_rev, &path_rev);
        let old_data = serde_json::to_string(&v)?;
        if old_id != new_id {
            tx.execute(
                "INSERT INTO projects(id, project_id, card_id, state, number, data) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                params![old_id, proj_col, card_col, state, number, old_data],
            )?;
            drop_project_ids.push(new_id);
        } else if old_data != raw {
            tx.execute(
                "UPDATE projects SET data=?2 WHERE id=?1",
                params![new_id, old_data],
            )?;
        }
    }

    // 2. Restore old card rows.
    let card_rows = load_short_id_rows(tx, "cards")?;
    let mut drop_card_ids = Vec::new();
    for (new_id, proj_col, card_col, state, number, raw) in card_rows {
        let old_id = rev_by_kind
            .get(&("card".into(), new_id.clone()))
            .cloned()
            .unwrap_or_else(|| new_id.clone());
        let old_proj = proj_col
            .as_ref()
            .map(|p| id_rev.get(p).cloned().unwrap_or_else(|| p.clone()));
        let mut v: Value = serde_json::from_str(&raw)?;
        rewrite_json_exact_inverse(&mut v, None, &id_rev, &path_rev);
        let old_data = serde_json::to_string(&v)?;
        if old_id != new_id {
            tx.execute(
                "INSERT INTO cards(id, project_id, card_id, state, number, data) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                params![old_id, old_proj, card_col, state, number, old_data],
            )?;
            drop_card_ids.push(new_id);
        } else {
            tx.execute(
                "UPDATE cards SET project_id=?2, data=?3 WHERE id=?1",
                params![new_id, old_proj, old_data],
            )?;
        }
    }

    // 3. Restore events + event_journal.
    let event_rows = load_short_id_rows(tx, "events")?;
    for (new_id, proj_col, card_col, state, number, raw) in event_rows {
        let old_id = rev_by_kind
            .get(&("event".into(), new_id.clone()))
            .cloned()
            .unwrap_or_else(|| new_id.clone());
        let old_proj = proj_col
            .as_ref()
            .map(|p| id_rev.get(p).cloned().unwrap_or_else(|| p.clone()));
        let old_card = card_col
            .as_ref()
            .map(|c| id_rev.get(c).cloned().unwrap_or_else(|| c.clone()));
        let mut v: Value = serde_json::from_str(&raw)?;
        rewrite_json_exact_inverse(&mut v, None, &id_rev, &path_rev);
        let old_data = serde_json::to_string(&v)?;
        if old_id != new_id {
            tx.execute(
                "INSERT INTO events(id, project_id, card_id, state, number, data) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                params![old_id, old_proj, old_card, state, number, old_data],
            )?;
            tx.execute(
                "UPDATE event_journal SET event_id=?2 WHERE event_id=?1",
                params![new_id, old_id],
            )?;
            tx.execute("DELETE FROM events WHERE id=?1", [new_id])?;
        } else {
            tx.execute(
                "UPDATE events SET project_id=?2, card_id=?3, data=?4 WHERE id=?1",
                params![new_id, old_proj, old_card, old_data],
            )?;
        }
    }

    // 4. Restore child tables.
    for (table, kind) in CHILD_SCOPED_TABLES {
        if !table_exists(tx, table)? {
            continue;
        }
        let rows: Vec<(String, Option<String>, Option<String>, String)> = {
            let mut q = tx.prepare(&format!(
                "SELECT id, project_id, card_id, data FROM {table}"
            ))?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (new_id, proj_col, card_col, raw) in rows {
            let old_id = rev_by_kind
                .get(&((*kind).to_owned(), new_id.clone()))
                .cloned()
                .unwrap_or_else(|| new_id.clone());
            let old_proj = proj_col
                .as_ref()
                .map(|p| id_rev.get(p).cloned().unwrap_or_else(|| p.clone()));
            let old_card = card_col
                .as_ref()
                .map(|c| id_rev.get(c).cloned().unwrap_or_else(|| c.clone()));
            let mut v: Value = serde_json::from_str(&raw)?;
            rewrite_json_exact_inverse(&mut v, None, &id_rev, &path_rev);
            if *table == "terminals"
                && let Some(agent) = v.get_mut("agent").and_then(Value::as_object_mut)
            {
                agent.remove("card_number");
            }
            let old_data = serde_json::to_string(&v)?;
            if old_id != new_id || old_proj != proj_col || old_card != card_col || old_data != raw {
                tx.execute(
                    &format!(
                        "UPDATE {table} SET id=?2, project_id=?3, card_id=?4, data=?5 WHERE id=?1"
                    ),
                    params![new_id, old_id, old_proj, old_card, old_data],
                )?;
            }
        }
    }

    if table_exists(tx, "source_observations")? {
        let rows: Vec<(String, String)> = {
            let mut q = tx.prepare("SELECT id, data FROM source_observations")?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (new_id, raw) in rows {
            let old_id = rev_by_kind
                .get(&("source_observation".into(), new_id.clone()))
                .cloned()
                .unwrap_or_else(|| new_id.clone());
            let mut v: Value = serde_json::from_str(&raw)?;
            rewrite_json_exact_inverse(&mut v, None, &id_rev, &path_rev);
            let old_data = serde_json::to_string(&v)?;
            if old_id != new_id || old_data != raw {
                tx.execute(
                    "UPDATE source_observations SET id=?2, data=?3 WHERE id=?1",
                    params![new_id, old_id, old_data],
                )?;
            }
        }
    }
    if table_exists(tx, "editor_drafts")? {
        let rows: Vec<(String, String)> = {
            let mut q = tx.prepare("SELECT id, data FROM editor_drafts")?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (new_id, raw) in rows {
            let old_id = rev_by_kind
                .get(&("editor_draft".into(), new_id.clone()))
                .cloned()
                .unwrap_or_else(|| new_id.clone());
            let mut v: Value = serde_json::from_str(&raw)?;
            rewrite_json_exact_inverse(&mut v, None, &id_rev, &path_rev);
            let old_data = serde_json::to_string(&v)?;
            if old_id != new_id || old_data != raw {
                tx.execute(
                    "UPDATE editor_drafts SET id=?2, data=?3 WHERE id=?1",
                    params![new_id, old_id, old_data],
                )?;
            }
        }
    }
    if table_exists(tx, "counters")? {
        let keys: Vec<String> = {
            let mut q = tx.prepare("SELECT id FROM counters")?;
            q.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        for new_id in keys {
            if let Some(old_id) = rev_by_kind.get(&("counter".into(), new_id.clone())) {
                tx.execute(
                    "UPDATE counters SET id=?2 WHERE id=?1",
                    params![new_id, old_id],
                )?;
            }
        }
    }
    if table_exists(tx, "operations")? {
        let rows: Vec<(String, String)> = {
            let mut q = tx.prepare("SELECT id, result FROM operations")?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (id, raw) in rows {
            if let Ok(mut v) = serde_json::from_str::<Value>(&raw) {
                rewrite_json_exact_inverse(&mut v, None, &id_rev, &path_rev);
                let old_res = serde_json::to_string(&v)?;
                if old_res != raw {
                    tx.execute(
                        "UPDATE operations SET result=?2 WHERE id=?1",
                        params![id, old_res],
                    )?;
                }
            }
        }
    }

    for new_cid in drop_card_ids {
        tx.execute("DELETE FROM cards WHERE id=?1", [new_cid])?;
    }
    for new_pid in drop_project_ids {
        tx.execute("DELETE FROM projects WHERE id=?1", [new_pid])?;
    }
    tx.execute_batch(
        "CREATE UNIQUE INDEX card_numbers ON cards(project_id, number);
         CREATE UNIQUE INDEX github_card_source ON cards(project_id, json_extract(data,'$.content.source.repository_id'), json_extract(data,'$.content.source.issue_id')) WHERE json_extract(data,'$.content.kind')='github_issue';
         CREATE UNIQUE INDEX github_source_card ON cards(project_id, json_extract(data,'$.content.source.repository_id'), json_extract(data,'$.content.source.issue_id')) WHERE json_extract(data,'$.content.kind') IN ('github_issue','github_pr');
         CREATE UNIQUE INDEX event_order ON events(card_id, number);
         DROP TABLE IF EXISTS path_migrations;
         DROP TABLE IF EXISTS id_migrations;",
    )?;
    Ok(())
}

fn move_tree(src: &Path, dst: &Path, report_map: &HashMap<String, String>) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let child_src = entry.path();
            let mut name = entry.file_name().to_string_lossy().into_owned();
            if src.file_name().is_some_and(|n| n == "reports")
                && let Some(stem) = name.strip_suffix(".md")
                && let Some(mapped) = report_map.get(stem)
            {
                name = format!("{mapped}.md");
            } else if src.file_name().is_some_and(|n| n == "resources") {
                let (stem, ext) = match name.find('.') {
                    Some(i) => (&name[..i], &name[i..]),
                    None => (name.as_str(), ""),
                };
                if let Some(mapped) = report_map.get(stem) {
                    name = format!("{mapped}{ext}");
                }
            }
            move_tree(&child_src, &dst.join(name), report_map)?;
        }
        let _ = std::fs::remove_dir(src);
    } else if !dst.exists() {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(src, dst)?;
    }
    Ok(())
}

fn sync_git_card_refs(conn: &Connection, mappings: &[(String, String)]) -> Result<()> {
    if mappings.is_empty() || !table_exists(conn, "projects")? {
        return Ok(());
    }
    let mut dirs = Vec::new();
    let mut q = conn.prepare("SELECT data FROM projects")?;
    for row in q.query_map([], |r| r.get::<_, String>(0))? {
        let v: Value = serde_json::from_str(&row?)?;
        let is_local = v["location"].is_null()
            || v["location"]["machine"].as_str().unwrap_or("local") == "local";
        if !is_local {
            continue;
        }
        if let Some(dir) = v["location"]["directory"]
            .as_str()
            .or_else(|| v["directory"].as_str())
        {
            dirs.push(dir.to_owned());
        }
    }
    let by_src: HashMap<&str, &str> = mappings
        .iter()
        .map(|(s, d)| (s.as_str(), d.as_str()))
        .collect();
    for dir in dirs {
        let repo = Path::new(&dir);
        if !repo.is_dir() {
            continue;
        }
        let Ok(out) = canban_exec::command("git")
            .args([
                "-C",
                &dir,
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/canban/cards/",
            ])
            .output()
        else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            let Some((refname, oid)) = line.split_once(' ') else {
                continue;
            };
            let Some(cid) = refname.strip_prefix("refs/canban/cards/") else {
                continue;
            };
            let Some(dst_cid) = by_src.get(cid) else {
                continue;
            };
            let dst_ref = format!("refs/canban/cards/{dst_cid}");
            let already = canban_exec::command("git")
                .args(["-C", &dir, "rev-parse", "--verify", "--quiet", &dst_ref])
                .output()
                .is_ok_and(|o| o.status.success());
            if !already {
                let _ = canban_exec::command("git")
                    .args(["-C", &dir, "update-ref", &dst_ref, oid])
                    .output();
            }
            let _ = canban_exec::command("git")
                .args(["-C", &dir, "update-ref", "-d", refname])
                .output();
        }
    }
    Ok(())
}

/// Moves on-disk Card/archive files and local Git retain refs to match schema-16 short IDs.
pub fn sync_paths(home: &Path, conn: &Connection) -> Result<()> {
    if !table_exists(conn, "id_migrations")? && !table_exists(conn, "path_migrations")? {
        return Ok(());
    }
    let mut path_pairs = Vec::new();
    if table_exists(conn, "path_migrations")? {
        let mut q = conn.prepare("SELECT old_rel, new_rel FROM path_migrations")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            path_pairs.push(row?);
        }
    }
    let mut proj_map = HashMap::new();
    let mut card_pairs = Vec::new();
    let mut card_old_to_new = HashMap::new();
    let mut file_id_map = HashMap::new();
    if table_exists(conn, "id_migrations")? {
        let mut q = conn.prepare("SELECT kind, old_id, new_id FROM id_migrations")?;
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (kind, old_id, new_id) = row?;
            match kind.as_str() {
                "project" => {
                    proj_map.insert(old_id, new_id);
                }
                "card" => {
                    card_old_to_new.insert(old_id.clone(), new_id.clone());
                    card_pairs.push((old_id, new_id));
                }
                "report" | "artifact" => {
                    file_id_map.insert(old_id, new_id);
                }
                _ => {}
            }
        }
    }
    if path_pairs.is_empty() && proj_map.is_empty() && card_pairs.is_empty() {
        return Ok(());
    }

    for (old_rel, new_rel) in &path_pairs {
        let src = home.join(old_rel);
        let dst = home.join(new_rel);
        if src.exists() && !dst.exists() {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&src, &dst)?;
        }
    }

    let mut card_info: HashMap<String, (String, u64)> = HashMap::new();
    if table_exists(conn, "cards")? {
        let mut q = conn.prepare("SELECT id, project_id, number FROM cards")?;
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u64>(2)?,
            ))
        })? {
            let (cid, pid, num) = row?;
            card_info.insert(cid, (pid, num));
        }
    }

    let projects_root = home.join("projects");
    if projects_root.is_dir() {
        for entry in std::fs::read_dir(&projects_root)? {
            let entry = entry?;
            let p_name = entry.file_name().to_string_lossy().into_owned();
            let dst_pid = proj_map
                .get(&p_name)
                .cloned()
                .unwrap_or_else(|| p_name.clone());
            if let Some(mapped_pid) = proj_map.get(&p_name) {
                let p_path = entry.path();
                if p_path.join("cards").is_dir() {
                    for card_entry in std::fs::read_dir(p_path.join("cards"))? {
                        let card_entry = card_entry?;
                        let c_name = card_entry.file_name().to_string_lossy().into_owned();
                        let cid = card_old_to_new
                            .get(&c_name)
                            .map(String::as_str)
                            .unwrap_or(&c_name);
                        let leaf = card_info
                            .get(cid)
                            .map(|(_, num)| num.to_string())
                            .unwrap_or_else(|| c_name.clone());
                        let dst = projects_root.join(mapped_pid).join("cards").join(leaf);
                        move_tree(&card_entry.path(), &dst, &file_id_map)?;
                    }
                    let _ = std::fs::remove_dir(p_path.join("cards"));
                }
                move_tree(&p_path, &projects_root.join(mapped_pid), &file_id_map)?;
            } else if entry.path().join("cards").is_dir() {
                for card_entry in std::fs::read_dir(entry.path().join("cards"))? {
                    let card_entry = card_entry?;
                    let c_name = card_entry.file_name().to_string_lossy().into_owned();
                    let cid = card_old_to_new
                        .get(&c_name)
                        .map(String::as_str)
                        .unwrap_or(&c_name);
                    if let Some((expected_pid, num)) = card_info.get(cid)
                        && *expected_pid == dst_pid
                        && c_name != num.to_string()
                    {
                        let dst = projects_root
                            .join(&dst_pid)
                            .join("cards")
                            .join(num.to_string());
                        move_tree(&card_entry.path(), &dst, &file_id_map)?;
                    }
                }
            }
        }
    }

    let archives_root = home.join("archives");
    if archives_root.is_dir() {
        for entry in std::fs::read_dir(&archives_root)? {
            let entry = entry?;
            let p_name = entry.file_name().to_string_lossy().into_owned();
            let dst_pid = proj_map
                .get(&p_name)
                .cloned()
                .unwrap_or_else(|| p_name.clone());
            if entry.path().is_dir() {
                for card_entry in std::fs::read_dir(entry.path())? {
                    let card_entry = card_entry?;
                    let c_name = card_entry.file_name().to_string_lossy().into_owned();
                    let cid = card_old_to_new
                        .get(&c_name)
                        .map(String::as_str)
                        .unwrap_or(&c_name);
                    if let Some((expected_pid, num)) = card_info.get(cid)
                        && *expected_pid == dst_pid
                    {
                        let dst = archives_root.join(&dst_pid).join(num.to_string());
                        if card_entry.path() != dst {
                            move_tree(&card_entry.path(), &dst, &file_id_map)?;
                        }
                    }
                }
                if proj_map.contains_key(&p_name) {
                    let _ = std::fs::remove_dir(entry.path());
                }
            }
        }
    }

    sync_git_card_refs(conn, &card_pairs)?;
    Ok(())
}

fn sync_paths_down(home: &Path, conn: &Connection) -> Result<()> {
    if !table_exists(conn, "id_migrations")? && !table_exists(conn, "path_migrations")? {
        return Ok(());
    }
    if table_exists(conn, "path_migrations")? {
        let mut q = conn.prepare("SELECT old_rel, new_rel FROM path_migrations")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (old_rel, new_rel) = row?;
            let src = home.join(&new_rel);
            let dst = home.join(&old_rel);
            if src.exists() && !dst.exists() {
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::rename(&src, &dst)?;
            }
        }
    }

    let mut rev_proj = HashMap::new();
    let mut rev_card = HashMap::new();
    let mut card_pairs_rev = Vec::new();
    let mut rev_files = HashMap::new();
    if table_exists(conn, "id_migrations")? {
        let mut q = conn.prepare("SELECT kind, old_id, new_id FROM id_migrations")?;
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (kind, old_id, new_id) = row?;
            match kind.as_str() {
                "project" => {
                    rev_proj.insert(new_id, old_id);
                }
                "card" => {
                    rev_card.insert(new_id.clone(), old_id.clone());
                    card_pairs_rev.push((new_id, old_id));
                }
                "report" | "artifact" => {
                    rev_files.insert(new_id, old_id);
                }
                _ => {}
            }
        }
    }

    if table_exists(conn, "cards")? {
        let mut q = conn.prepare("SELECT id, project_id, number FROM cards")?;
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u64>(2)?,
            ))
        })? {
            let (new_cid, new_pid, num) = row?;
            let old_pid = rev_proj
                .get(&new_pid)
                .cloned()
                .unwrap_or_else(|| new_pid.clone());
            let old_cid = rev_card
                .get(&new_cid)
                .cloned()
                .unwrap_or_else(|| new_cid.clone());
            let num_str = num.to_string();
            let cur_card_dir = home
                .join("projects")
                .join(&new_pid)
                .join("cards")
                .join(&num_str);
            let old_card_dir = home
                .join("projects")
                .join(&old_pid)
                .join("cards")
                .join(&old_cid);
            if cur_card_dir != old_card_dir && cur_card_dir.exists() {
                move_tree(&cur_card_dir, &old_card_dir, &rev_files)?;
            }
            let cur_arch_dir = home.join("archives").join(&new_pid).join(&num_str);
            let old_arch_dir = home.join("archives").join(&old_pid).join(&old_cid);
            if cur_arch_dir != old_arch_dir && cur_arch_dir.exists() {
                move_tree(&cur_arch_dir, &old_arch_dir, &rev_files)?;
            }
        }
    }
    for (new_pid, old_pid) in &rev_proj {
        let cur = home.join("projects").join(new_pid);
        if cur.is_dir() {
            let _ = std::fs::remove_dir(cur.join("cards"));
            move_tree(&cur, &home.join("projects").join(old_pid), &rev_files)?;
        }
        let cur_arch = home.join("archives").join(new_pid);
        if cur_arch.is_dir() {
            move_tree(&cur_arch, &home.join("archives").join(old_pid), &rev_files)?;
        }
    }

    sync_git_card_refs(conn, &card_pairs_rev)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_steps_roundtrip_and_accept_both_premerge_schema_nine_shapes() {
        let home = tempfile::tempdir().unwrap();
        let mut store = crate::Store::open(home.path()).unwrap();
        apply(&mut store.conn, 8).unwrap();
        store
            .conn
            .execute("INSERT INTO counters VALUES('retained',42)", [])
            .unwrap();
        for editor_branch in [true, false] {
            if editor_branch {
                store.conn.execute_batch("CREATE TABLE editor_drafts(id TEXT PRIMARY KEY,revision INTEGER NOT NULL,updated_at INTEGER NOT NULL,data TEXT NOT NULL); INSERT INTO editor_drafts VALUES('new-card:retained',1,0,'{\"body\":\"unsent\"}'); PRAGMA user_version=9;").unwrap();
            } else {
                apply(&mut store.conn, 9).unwrap();
            }
            apply(&mut store.conn, 10).unwrap();
            apply(&mut store.conn, 9).unwrap();
            let editor: bool = store
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='editor_drafts')",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(editor, editor_branch);
            if editor_branch {
                let data: String = store
                    .conn
                    .query_row("SELECT data FROM editor_drafts", [], |r| r.get(0))
                    .unwrap();
                assert!(data.contains("unsent"));
                store
                    .conn
                    .execute_batch("DROP TABLE editor_drafts")
                    .unwrap();
            }
            apply(&mut store.conn, 8).unwrap();
        }
        let value: u64 = store
            .conn
            .query_row("SELECT value FROM counters WHERE id='retained'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(value, 42);
    }
    fn fixture() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE cards(id INTEGER PRIMARY KEY, body TEXT); INSERT INTO cards VALUES(42,'keep'); PRAGMA user_version=8;").unwrap();
        c
    }
    fn up9(tx: &Transaction<'_>) -> Result<()> {
        tx.execute_batch(
            "ALTER TABLE cards ADD COLUMN note TEXT DEFAULT ''; UPDATE cards SET note='context';",
        )?;
        Ok(())
    }
    fn down9(tx: &Transaction<'_>) -> Result<()> {
        tx.execute_batch("ALTER TABLE cards DROP COLUMN note;")?;
        Ok(())
    }
    fn up10(tx: &Transaction<'_>) -> Result<()> {
        tx.execute_batch(
            "CREATE TABLE refs(id INTEGER REFERENCES cards(id)); INSERT INTO refs VALUES(42);",
        )?;
        Ok(())
    }
    fn down10(tx: &Transaction<'_>) -> Result<()> {
        tx.execute_batch("DROP TABLE refs;")?;
        Ok(())
    }
    fn fail(tx: &Transaction<'_>) -> Result<()> {
        tx.execute_batch("UPDATE cards SET body='must rollback';")?;
        anyhow::bail!("injected migration failure")
    }
    #[test]
    fn paired_roundtrip_keeps_representative_data() {
        let mut c = fixture();
        let steps = [
            Migration {
                version: 9,
                up: up9,
                down: down9,
            },
            Migration {
                version: 10,
                up: up10,
                down: down10,
            },
        ];
        batch(&mut c, &steps, 10).unwrap();
        assert_eq!(version(&c).unwrap(), 10);
        batch(&mut c, &steps, 8).unwrap();
        assert_eq!(version(&c).unwrap(), 8);
        assert_eq!(
            c.query_row("SELECT body FROM cards WHERE id=42", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        assert!(c.prepare("SELECT note FROM cards").is_err());
    }
    #[test]
    fn mid_up_and_down_failures_roll_back_the_entire_batch() {
        let mut c = fixture();
        let failing = [
            Migration {
                version: 9,
                up: up9,
                down: down9,
            },
            Migration {
                version: 10,
                up: fail,
                down: down10,
            },
        ];
        assert!(batch(&mut c, &failing, 10).is_err());
        assert_eq!(version(&c).unwrap(), 8);
        assert!(c.prepare("SELECT note FROM cards").is_err());
        let steps = [
            Migration {
                version: 9,
                up: up9,
                down: fail,
            },
            Migration {
                version: 10,
                up: up10,
                down: down10,
            },
        ];
        batch(&mut c, &steps, 10).unwrap();
        assert!(batch(&mut c, &steps, 8).is_err());
        assert_eq!(version(&c).unwrap(), 10);
        assert!(c.prepare("SELECT * FROM refs").is_ok());
        assert_eq!(
            c.query_row("SELECT body FROM cards", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        assert!(batch(&mut c, &steps, 7).is_err());
        assert!(batch(&mut c, &steps, 11).is_err());
    }
    #[test]
    fn managed_worktrees_schema_13_roundtrip_and_downgrade_guard() {
        let home = tempfile::tempdir().unwrap();
        let mut store = crate::Store::open(home.path()).unwrap();
        assert_eq!(version(&store.conn).unwrap(), CURRENT);
        let has_requests: bool = store
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='requests')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!has_requests);
        apply(&mut store.conn, 12).unwrap();
        assert_eq!(version(&store.conn).unwrap(), 12);
        apply(&mut store.conn, 13).unwrap();
        assert_eq!(version(&store.conn).unwrap(), 13);
        store.conn.execute(
            "INSERT INTO managed_worktrees(id,project_id,card_id,state,number,data) VALUES('wt-1','p1',NULL,'',0,'{}')",
            [],
        ).unwrap();
        let err = apply(&mut store.conn, 12).unwrap_err();
        assert!(err.to_string().contains("standalone managed worktrees"));
        assert_eq!(version(&store.conn).unwrap(), 13);
        store
            .conn
            .execute("DELETE FROM managed_worktrees WHERE id='wt-1'", [])
            .unwrap();
        apply(&mut store.conn, 12).unwrap();
        assert_eq!(version(&store.conn).unwrap(), 12);
        apply(&mut store.conn, CURRENT).unwrap();
        assert_eq!(version(&store.conn).unwrap(), CURRENT);
    }

    #[test]
    fn schema_14_merges_requests_into_attempts_and_roundtrips_with_schema_13() {
        use serde_json::{Value, json};
        let home = tempfile::tempdir().unwrap();
        let mut store = crate::Store::open(home.path()).unwrap();
        assert_eq!(version(&store.conn).unwrap(), CURRENT);

        // Downgrade empty DB to schema 13 and populate representative Request + Attempt records.
        apply(&mut store.conn, 13).unwrap();
        assert_eq!(version(&store.conn).unwrap(), 13);
        store.conn.execute(
            "INSERT INTO projects(id,project_id,card_id,state,number,data) VALUES('p1',NULL,NULL,'',0,'{\"id\":\"p1\",\"name\":\"P\",\"directory\":\"/tmp/p\",\"workflow\":{\"stages\":[]},\"created_at\":1}')",
            [],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO cards(id,project_id,card_id,state,number,data) VALUES('c1','p1',NULL,'develop',1,'{\"id\":\"c1\",\"project_id\":\"p1\",\"number\":1,\"title\":\"T\",\"body\":\"\",\"stage\":\"develop\",\"revision\":1,\"human_gate\":{\"reason\":\"Check\",\"request_id\":\"req-1\",\"next_stage\":null},\"created_at\":1}')",
            [],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO requests(id,project_id,card_id,state,number,data) VALUES('req-1','p1','c1','running',0,'{\"id\":\"req-1\",\"project_id\":\"p1\",\"card_id\":\"c1\",\"workflow_run_id\":\"run-1\",\"workflow_step\":\"develop:1\",\"trigger\":\"workflow\",\"stage\":\"develop\",\"profile\":{\"id\":\"developer\",\"name\":\"Developer\"},\"instruction\":\"Build it\",\"state\":\"running\",\"attempt\":1,\"created_at\":10}')",
            [],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO requests(id,project_id,card_id,state,number,data) VALUES('req-queued','p1','c1','queued',0,'{\"id\":\"req-queued\",\"project_id\":\"p1\",\"card_id\":\"c1\",\"trigger\":\"stage_entry\",\"stage\":\"develop\",\"profile\":{\"id\":\"reviewer\",\"name\":\"Reviewer\"},\"instruction\":\"Queued review\",\"state\":\"queued\",\"attempt\":0,\"created_at\":20}')",
            [],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO attempts(id,project_id,card_id,state,number,data) VALUES('req-1:1','p1','c1','running',1,'{\"id\":\"req-1:1\",\"request_id\":\"req-1\",\"project_id\":\"p1\",\"card_id\":\"c1\",\"number\":1,\"state\":\"running\",\"cwd\":\"/tmp/wt\",\"input_path\":\"in.md\",\"created_at\":11}')",
            [],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO workflow_runs(id,project_id,card_id,state,number,data) VALUES('run-1','p1','c1','waiting_agent',0,'{\"id\":\"run-1\",\"request_id\":\"req-1\"}')",
            [],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO conversations(id,project_id,card_id,state,number,data) VALUES('c1:developer','p1','c1','',0,'{\"id\":\"c1:developer\",\"last_request_id\":\"req-1\"}')",
            [],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO questions(id,project_id,card_id,state,number,data) VALUES('q1','p1','c1','replied',0,'{\"id\":\"q1\",\"request_id\":\"req-1\",\"attempt\":1}')",
            [],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO reports(id,project_id,card_id,state,number,data) VALUES('rep1','p1','c1','completed',0,'{\"id\":\"rep1\",\"request_id\":\"req-1\",\"attempt\":1}')",
            [],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO events(id,project_id,card_id,state,number,data) VALUES('ev1','p1','c1','execution.started',1,'{\"id\":\"ev1\",\"request_id\":\"req-1\",\"data\":{}}')",
            [],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO note_events(id,project_id,card_id,state,number,data) VALUES('nev1','p1','c1','note.updated',1,'{\"id\":\"nev1\",\"request_id\":\"req-1\",\"attempt\":1}')",
            [],
        ).unwrap();

        // Migrate 13 -> 14
        apply(&mut store.conn, 14).unwrap();
        assert_eq!(version(&store.conn).unwrap(), 14);
        let has_requests: bool = store
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='requests')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!has_requests);

        let att1_raw: String = store
            .conn
            .query_row("SELECT data FROM attempts WHERE id='req-1:1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let att1: Value = serde_json::from_str(&att1_raw).unwrap();
        assert!(att1.get("request_id").is_none());
        assert_eq!(att1["profile"]["id"], json!("developer"));
        assert_eq!(att1["instruction"], json!("Build it"));
        assert_eq!(att1["workflow_run_id"], json!("run-1"));

        let queued_raw: String = store
            .conn
            .query_row("SELECT data FROM attempts WHERE id='req-queued'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let queued: Value = serde_json::from_str(&queued_raw).unwrap();
        assert_eq!(queued["state"], json!("queued"));
        assert_eq!(queued["profile"]["id"], json!("reviewer"));

        let run_raw: String = store
            .conn
            .query_row("SELECT data FROM workflow_runs WHERE id='run-1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let run: Value = serde_json::from_str(&run_raw).unwrap();
        assert_eq!(run["attempt_id"], json!("req-1:1"));
        assert!(run.get("request_id").is_none());

        let card_raw: String = store
            .conn
            .query_row("SELECT data FROM cards WHERE id='c1'", [], |r| r.get(0))
            .unwrap();
        let card: Value = serde_json::from_str(&card_raw).unwrap();
        assert_eq!(card["human_gate"]["attempt_id"], json!("req-1:1"));

        let rep_raw: String = store
            .conn
            .query_row("SELECT data FROM reports WHERE id='rep1'", [], |r| r.get(0))
            .unwrap();
        let rep: Value = serde_json::from_str(&rep_raw).unwrap();
        assert_eq!(rep["attempt_id"], json!("req-1:1"));
        assert_eq!(rep["actor"], json!("agent:developer"));

        let ev_raw: String = store
            .conn
            .query_row("SELECT data FROM events WHERE id='ev1'", [], |r| r.get(0))
            .unwrap();
        let ev: Value = serde_json::from_str(&ev_raw).unwrap();
        assert!(ev.get("request_id").is_none());
        assert_eq!(ev["data"]["attempt_id"], json!("req-1:1"));

        // Roundtrip 14 -> 13 -> 15 -> 14 -> 15
        apply(&mut store.conn, 13).unwrap();
        assert_eq!(version(&store.conn).unwrap(), 13);
        let req_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0))
            .unwrap();
        assert_eq!(req_count, 2);
        apply(&mut store.conn, 15).unwrap();
        assert_eq!(version(&store.conn).unwrap(), 15);
        let has_attempts: bool = store
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='attempts')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!has_attempts);

        let term_dev_raw: String = store
            .conn
            .query_row(
                "SELECT data FROM terminals WHERE id='c1:developer'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let term_dev: Value = serde_json::from_str(&term_dev_raw).unwrap();
        assert_eq!(term_dev["agent"]["profile"]["id"], json!("developer"));
        assert_eq!(term_dev["agent"]["instruction"], json!("Build it"));
        assert_eq!(term_dev["agent"]["workflow_run_id"], json!("run-1"));

        let term_rev_raw: String = store
            .conn
            .query_row(
                "SELECT data FROM terminals WHERE id='c1:reviewer'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let term_rev: Value = serde_json::from_str(&term_rev_raw).unwrap();
        assert_eq!(term_rev["agent"]["state"], json!("queued"));

        let run_raw: String = store
            .conn
            .query_row("SELECT data FROM workflow_runs WHERE id='run-1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let run: Value = serde_json::from_str(&run_raw).unwrap();
        assert_eq!(run["terminal_id"], json!("c1:developer"));
        assert!(run.get("attempt_id").is_none());

        let card_raw: String = store
            .conn
            .query_row("SELECT data FROM cards WHERE id='c1'", [], |r| r.get(0))
            .unwrap();
        let card: Value = serde_json::from_str(&card_raw).unwrap();
        assert_eq!(card["human_gate"]["terminal_id"], json!("c1:developer"));

        let rep_raw: String = store
            .conn
            .query_row("SELECT data FROM reports WHERE id='rep1'", [], |r| r.get(0))
            .unwrap();
        let rep: Value = serde_json::from_str(&rep_raw).unwrap();
        assert_eq!(rep["terminal_id"], json!("c1:developer"));

        let ev_raw: String = store
            .conn
            .query_row("SELECT data FROM events WHERE id='ev1'", [], |r| r.get(0))
            .unwrap();
        let ev: Value = serde_json::from_str(&ev_raw).unwrap();
        assert_eq!(ev["data"]["terminal_id"], json!("c1:developer"));
        assert!(ev["data"].get("attempt_id").is_none());

        apply(&mut store.conn, 14).unwrap();
        assert_eq!(version(&store.conn).unwrap(), 14);
        let att_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(att_count, 2);
        apply(&mut store.conn, 15).unwrap();
        assert_eq!(version(&store.conn).unwrap(), 15);
    }

    #[test]
    fn schema_16_short_ids_and_paths_roundtrip_and_preserves_data() {
        use serde_json::{Value, json};
        let home = tempfile::tempdir().unwrap();
        let mut store = crate::Store::open(home.path()).unwrap();
        assert_eq!(version(&store.conn).unwrap(), CURRENT);

        // Downgrade empty DB to schema 15 and insert 32-char UUID records & disk files.
        apply(&mut store.conn, 15).unwrap();
        assert_eq!(version(&store.conn).unwrap(), 15);

        let old_pid = "0afddd435c7a45f793b2c8237d74bab8";
        let old_cid = "e49615534ea24b04904bdd536ae69910";
        let old_eid = "11112222333344445555666677778888";
        let old_rep = "714d094ca4274c6da06947b11a484760";
        let old_art = "99990000aaaabbbbccccddddeeeeffff";
        let old_nid = "abcdef0123456789abcdef0123456789";
        let old_nev = "fedcba9876543210fedcba9876543210";
        let old_run = "deadbeefcafebabe1234567890abcdef";
        let old_tid = format!("{old_cid}:agy");
        let old_qid = format!("{old_eid}:agy");
        let old_report_rel =
            format!("projects/{old_pid}/cards/{old_cid}/agents/agy/reports/{old_rep}.md");
        let old_prompt_rel = format!("projects/{old_pid}/cards/{old_cid}/agents/agy/prompt.md");
        let old_art_rel = format!("projects/{old_pid}/cards/{old_cid}/resources/{old_art}.txt");
        let old_arch_rel = format!("archives/{old_pid}/{old_cid}/pack.tar.zst");

        store.conn.execute(
            "INSERT INTO projects(id,project_id,card_id,state,number,data) VALUES(?1,NULL,NULL,'',0,?2)",
            params![old_pid, json!({"id":old_pid,"name":"Repo","directory":home.path().join("repo"),"workflow":{"stages":[]},"created_at":1}).to_string()],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO cards(id,project_id,card_id,state,number,data) VALUES(?1,?2,NULL,'develop',69,?3)",
            params![old_cid, old_pid, json!({"id":old_cid,"project_id":old_pid,"number":69,"title":"Task","body":"","stage":"develop","revision":2,"archive":old_arch_rel,"human_gate":{"reason":"Check","terminal_id":old_tid,"next_stage":null},"created_at":1}).to_string()],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO events(id,project_id,card_id,state,number,data) VALUES(?1,?2,?3,'comment.added',1,?4)",
            params![old_eid, old_pid, old_cid, json!({"id":old_eid,"project_id":old_pid,"card_id":old_cid,"sequence":1,"kind":"comment.added","actor":"user","message":"hi","data":{"terminal_id":old_tid,"workflow_run_id":old_run,"report_id":old_rep},"created_at":2}).to_string()],
        ).unwrap();
        store
            .conn
            .execute("INSERT INTO event_journal(event_id) VALUES(?1)", [old_eid])
            .unwrap();
        store.conn.execute(
            "INSERT INTO conversations(id,project_id,card_id,state,number,data) VALUES(?1,?2,?3,'',0,?4)",
            params![old_tid, old_pid, old_cid, json!({"id":old_tid,"project_id":old_pid,"card_id":old_cid,"profile_id":"agy","provider":"agy","provider_session_id":"11111111-2222-4333-8444-555555555555"}).to_string()],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO terminals(id,project_id,card_id,state,number,data) VALUES(?1,?2,?3,'running',1,?4)",
            params![old_tid, old_pid, old_cid, json!({"id":old_tid,"conversation_id":old_tid,"number":1,"created_at":3,"agent":{"project_id":old_pid,"card_id":old_cid,"number":1,"profile":{"id":"agy","name":"Agy"},"input_path":old_prompt_rel,"workflow_run_id":old_run,"state":"running"}}).to_string()],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO reports(id,project_id,card_id,state,number,data) VALUES(?1,?2,?3,'completed',0,?4)",
            params![old_rep, old_pid, old_cid, json!({"id":old_rep,"project_id":old_pid,"card_id":old_cid,"terminal_id":old_tid,"workflow_run_id":old_run,"state":"completed","message":"ok","path":old_report_rel,"request_key":"rk1","fingerprint":"fp1","created_at":4}).to_string()],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO artifacts(id,project_id,card_id,state,number,data) VALUES(?1,?2,?3,'',0,?4)",
            params![old_art, old_pid, old_cid, json!({"id":old_art,"project_id":old_pid,"card_id":old_cid,"name":"log.txt","media_type":"text/plain","path":old_art_rel,"digest":"abc","bytes":3}).to_string()],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO questions(id,project_id,card_id,state,number,data) VALUES(?1,?2,?3,'pending',0,?4)",
            params![old_qid, old_pid, old_cid, json!({"id":old_qid,"project_id":old_pid,"card_id":old_cid,"profile_id":"agy","comment_id":old_eid,"terminal_id":old_tid,"body":"why?","state":"pending","created_at":4}).to_string()],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO notes(id,project_id,card_id,state,number,data) VALUES(?1,?2,NULL,'',0,?3)",
            params![old_nid, old_pid, json!({"id":old_nid,"project_id":old_pid,"title":"N","body":"B","revision":1,"created_at":1,"card_ids":[old_cid]}).to_string()],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO note_revisions(id,project_id,card_id,state,number,data) VALUES(?1,?2,NULL,'',1,?3)",
            params![format!("{old_nid}:1"), old_pid, json!({"id":format!("{old_nid}:1"),"project_id":old_pid,"note_id":old_nid,"revision":1,"title":"N","body":"B","created_at":1}).to_string()],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO note_events(id,project_id,card_id,state,number,data) VALUES(?1,?2,?3,'note.created',1,?4)",
            params![old_nev, old_pid, old_cid, json!({"id":old_nev,"project_id":old_pid,"note_id":old_nid,"sequence":1,"kind":"note.created","actor":"user","message":"created","card_id":old_cid,"revision":1,"data":{"terminal_id":old_tid},"created_at":1}).to_string()],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO workflow_sources(id,project_id,card_id,state,number,data) VALUES(?1,?2,NULL,'',0,?3)",
            params![format!("{old_pid}:default.yaml"), old_pid, json!({"id":format!("{old_pid}:default.yaml"),"project_id":old_pid,"filename":"default.yaml","workflow_id":"canban-default","revision":1,"hash":"h","revision_id":"canban-default:1","enabled":true,"rules":{}}).to_string()],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO workflow_revisions(id,project_id,card_id,state,number,data) VALUES('canban-default:1',?1,NULL,'',1,?2)",
            params![old_pid, json!({"id":"canban-default:1","project_id":old_pid,"workflow_id":"canban-default","revision":1,"definition":{"id":"canban-default","name":"Default","steps":{}},"created_at":1}).to_string()],
        ).unwrap();
        store.conn.execute(
            "INSERT INTO workflow_runs(id,project_id,card_id,state,number,data) VALUES(?1,?2,?3,'waiting_agent',0,?4)",
            params![old_run, old_pid, old_cid, json!({"id":old_run,"project_id":old_pid,"card_id":old_cid,"workflow_id":"canban-default","definition_revision":1,"definition":{"id":"canban-default","name":"Default","steps":{}},"state":"waiting_agent","visit":1,"terminal_id":old_tid,"trigger_event_id":old_eid}).to_string()],
        ).unwrap();
        store
            .conn
            .execute(
                "INSERT INTO editor_drafts(id,revision,updated_at,data) VALUES(?1,1,1,?2)",
                params![
                    format!("card:{old_cid}"),
                    json!({"project_id":old_pid,"card_id":old_cid,"body":"draft"}).to_string()
                ],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO counters(id,value) VALUES(?1,69)",
                [format!("card-number:{old_pid}")],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO operations(id,fingerprint,result) VALUES('op1','fp',?1)",
                [json!({"id":old_rep,"card_id":old_cid,"path":old_report_rel}).to_string()],
            )
            .unwrap();

        // Populate matching on-disk files, including unindexed context.json & scratch.
        for (rel, content) in [
            (old_report_rel.as_str(), "report body"),
            (old_prompt_rel.as_str(), "prompt body"),
            (old_art_rel.as_str(), "art"),
            (old_arch_rel.as_str(), "archive"),
        ] {
            let p = home.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
        let unindexed = home
            .path()
            .join(format!("projects/{old_pid}/cards/{old_cid}/context.json"));
        std::fs::write(&unindexed, "{}").unwrap();
        drop(store);

        // Migrate 15 -> 16 via migrate() so DB + disk files migrate together.
        assert_eq!(migrate(home.path(), 16).unwrap(), 16);
        let store = crate::Store::open(home.path()).unwrap();

        let new_pid = "0afddd43";
        let new_cid = "e4961553";
        let new_rep = "714d094c";
        let new_art = "99990000";
        let new_report_rel = format!("projects/{new_pid}/cards/69/agents/agy/reports/{new_rep}.md");
        let new_prompt_rel = format!("projects/{new_pid}/cards/69/agents/agy/prompt.md");
        let new_art_rel = format!("projects/{new_pid}/cards/69/resources/{new_art}.txt");
        let new_arch_rel = format!("archives/{new_pid}/69/pack.tar.zst");

        assert!(home.path().join(&new_report_rel).exists());
        assert!(home.path().join(&new_prompt_rel).exists());
        assert!(home.path().join(&new_art_rel).exists());
        assert!(home.path().join(&new_arch_rel).exists());
        assert!(
            home.path()
                .join(format!("projects/{new_pid}/cards/69/context.json"))
                .exists()
        );
        assert!(!home.path().join(&old_report_rel).exists());

        let proj_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(proj_count, 1);
        let mapped_pid: String = store
            .conn
            .query_row("SELECT id FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mapped_pid, new_pid);
        let mapped_cid: String = store
            .conn
            .query_row("SELECT id FROM cards", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mapped_cid, new_cid);

        let wf_id: String = store
            .conn
            .query_row(
                "SELECT json_extract(data,'$.workflow_id') FROM workflow_revisions",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(wf_id, "canban-default");

        let term_raw: String = store
            .conn
            .query_row("SELECT data FROM terminals", [], |r| r.get(0))
            .unwrap();
        let term: Value = serde_json::from_str(&term_raw).unwrap();
        assert_eq!(term["id"], json!(format!("{new_cid}:agy")));
        assert_eq!(term["agent"]["card_number"], json!(69));
        assert_eq!(term["agent"]["input_path"], json!(new_prompt_rel));
        assert_eq!(term["agent"]["workflow_run_id"], json!("deadbeef"));

        let cnt_id: String = store
            .conn
            .query_row("SELECT id FROM counters WHERE value=69", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cnt_id, format!("card-number:{new_pid}"));
        let draft_id: String = store
            .conn
            .query_row("SELECT id FROM editor_drafts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(draft_id, format!("card:{new_cid}"));
        drop(store);

        // Downgrade 16 -> 15 and verify full restoration of DB IDs & on-disk paths!
        assert_eq!(migrate(home.path(), 15).unwrap(), 15);
        assert!(home.path().join(&old_report_rel).exists());
        assert!(home.path().join(&old_prompt_rel).exists());
        assert!(home.path().join(&old_art_rel).exists());
        assert!(home.path().join(&old_arch_rel).exists());
        assert!(
            home.path()
                .join(format!("projects/{old_pid}/cards/{old_cid}/context.json"))
                .exists()
        );
        assert!(!home.path().join(&new_report_rel).exists());

        let conn = Connection::open(home.path().join("canban.sqlite")).unwrap();
        let restored_pid: String = conn
            .query_row("SELECT id FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(restored_pid, old_pid);
        let restored_cid: String = conn
            .query_row("SELECT id FROM cards", [], |r| r.get(0))
            .unwrap();
        assert_eq!(restored_cid, old_cid);
        let restored_term: String = conn
            .query_row("SELECT data FROM terminals", [], |r| r.get(0))
            .unwrap();
        let restored_term_v: Value = serde_json::from_str(&restored_term).unwrap();
        assert!(restored_term_v["agent"].get("card_number").is_none());
        assert_eq!(
            restored_term_v["agent"]["input_path"],
            json!(old_prompt_rel)
        );
        drop(conn);

        // Upgrade 15 -> 16 again (roundtrip idempotence).
        assert_eq!(migrate(home.path(), 16).unwrap(), 16);
        assert!(home.path().join(&new_report_rel).exists());
        assert!(
            home.path()
                .join(format!("projects/{new_pid}/cards/69/context.json"))
                .exists()
        );
    }

    #[test]
    fn schema_17_merge_batches_roundtrip_and_downgrade_guard() {
        let home = tempfile::tempdir().unwrap();
        let mut store = crate::Store::open(home.path()).unwrap();
        let batch = canban_types::MergeBatch {
            id: "merge-queue:abcd1234:11223344".into(),
            project_id: "abcd1234".into(),
            profile_id: "merger".into(),
            provider_session_id: None,
            target_branch: "main".into(),
            target_head: "deadbeef".into(),
            strategy: canban_types::MergeStrategy::Rebase,
            sync_upstream: true,
            test_command: Some("cargo test".into()),
            worktree: "/tmp/wt".into(),
            state: "running".into(),
            failure_kind: Some("conflict".into()),
            conflict_card_number: Some(12),
            conflict_files: vec!["README.md".into()],
            test_output: None,
            items: vec![canban_types::MergeBatchItem {
                card_id: "c1234567".into(),
                card_number: 12,
                workflow_run_id: "r1234567".into(),
                branch: "card/12".into(),
                worktree: "/tmp/wt12".into(),
                base_head: "base".into(),
                integrated_commit: None,
                evicted: false,
            }],
            prompt: "Resolve".into(),
            error: None,
            queued_at: None,
            started_at: None,
            created_at: 1,
            updated_at: 1,
        };
        store.write(|db| db.save(&batch)).unwrap();
        assert_eq!(store.snapshot().unwrap().merge_batches.len(), 1);
        drop(store);

        // Downgrade must fail while batch is active.
        assert!(migrate(home.path(), 16).is_err());

        // Complete batch and downgrade cleanly to 16, then back up to 17.
        let mut store = crate::Store::open(home.path()).unwrap();
        let mut completed = batch.clone();
        completed.state = "completed".into();
        store.write(|db| db.save(&completed)).unwrap();
        drop(store);

        assert_eq!(migrate(home.path(), 16).unwrap(), 16);
        assert_eq!(migrate(home.path(), 17).unwrap(), 17);
        assert_eq!(migrate(home.path(), CURRENT).unwrap(), CURRENT);
        let store = crate::Store::open(home.path()).unwrap();
        assert!(store.snapshot().unwrap().merge_batches.is_empty());
    }

    #[test]
    fn schema_18_drops_manual_terminal_counter_and_preserves_labels() {
        let home = tempfile::tempdir().unwrap();
        let store = crate::Store::open(home.path()).unwrap();
        drop(store);

        assert_eq!(migrate(home.path(), 17).unwrap(), 17);
        let conn = Connection::open(home.path().join("canban.sqlite")).unwrap();
        conn.execute(
            "INSERT INTO counters(id, value) VALUES('terminal-manual-number', 7)",
            [],
        )
        .unwrap();
        let shell_v17 = serde_json::json!({
            "id": "shell-11112222",
            "conversation_id": null,
            "number": 4,
            "name": null,
            "manual_number": 7,
            "runtime": {
                "machine": "local",
                "core_id": "c1",
                "terminal": "t1",
                "state": "running",
                "connection": "connected",
                "error": null
            },
            "created_at": 1
        });
        conn.execute(
            "INSERT INTO terminals(id, project_id, card_id, state, number, data) VALUES('shell-11112222', NULL, NULL, 'running', 4, ?1)",
            [serde_json::to_string(&shell_v17).unwrap()],
        )
        .unwrap();
        drop(conn);

        assert_eq!(migrate(home.path(), 18).unwrap(), 18);
        let store = crate::Store::open(home.path()).unwrap();
        let count: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM counters WHERE id='terminal-manual-number'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        let snap = store.snapshot().unwrap();
        assert_eq!(snap.terminals.len(), 1);
        assert_eq!(snap.terminals[0].title(), "Terminal 7");
        drop(store);

        assert_eq!(migrate(home.path(), 17).unwrap(), 17);
        assert_eq!(migrate(home.path(), 18).unwrap(), 18);
    }

    #[test]
    fn schema_19_purges_legacy_json_workflow_templates_and_keeps_valid_yaml_presets() {
        let home = tempfile::tempdir().unwrap();
        let store = crate::Store::open(home.path()).unwrap();
        drop(store);

        assert_eq!(migrate(home.path(), 18).unwrap(), 18);
        let conn = Connection::open(home.path().join("canban.sqlite")).unwrap();
        let legacy_row = serde_json::json!({
            "id": "default",
            "workflow": {
                "name": "Development",
                "stages": [
                    {
                        "id": "backlog",
                        "name": "Backlog",
                        "execute": false,
                        "instruction": "",
                        "profile": null,
                        "on_completed": null,
                        "require_human_review": false
                    }
                ]
            }
        });
        conn.execute(
            "INSERT INTO workflow_templates(id, project_id, card_id, state, number, data) VALUES('default', NULL, NULL, '', 0, ?1)",
            [serde_json::to_string(&legacy_row).unwrap()],
        )
        .unwrap();
        let valid_tpl = canban_types::WorkflowTemplate {
            id: "custom".into(),
            name: "Custom YAML".into(),
            source_project_name: Some("Demo".into()),
            sources: vec![canban_types::WorkflowTemplateFile {
                filename: "development.yaml".into(),
                source: "schema: 3\nid: demo\nname: Demo\nsteps: []\n".into(),
            }],
            updated_at: 42,
        };
        conn.execute(
            "INSERT INTO workflow_templates(id, project_id, card_id, state, number, data) VALUES('custom', NULL, NULL, '', 0, ?1)",
            [serde_json::to_string(&valid_tpl).unwrap()],
        )
        .unwrap();
        drop(conn);

        assert_eq!(migrate(home.path(), 19).unwrap(), 19);
        let store = crate::Store::open(home.path()).unwrap();
        let snap = store.snapshot().unwrap();
        assert_eq!(snap.workflow_templates.len(), 1);
        assert_eq!(snap.workflow_templates[0].id, "custom");
        drop(store);

        assert_eq!(migrate(home.path(), 18).unwrap(), 18);
        assert_eq!(migrate(home.path(), 19).unwrap(), 19);
    }

    #[test]
    fn schema_20_creates_schedule_tables_and_round_trips_cleanly() {
        let home = tempfile::tempdir().unwrap();
        let store = crate::Store::open(home.path()).unwrap();
        assert_eq!(version(&store.conn).unwrap(), CURRENT);
        let state = canban_types::ScheduleState {
            host_id: String::new(),
            id: "schedule:p1:wf1:nightly".into(),
            project_id: "p1".into(),
            workflow_id: "wf1".into(),
            workflow_name: "Nightly Audit".into(),
            filename: "nightly.yaml".into(),
            rule_id: "nightly".into(),
            cron: "0 2 * * *".into(),
            timezone: Some("UTC".into()),
            overlap: canban_types::ScheduleOverlap::Skip,
            creates_card: false,
            starts_with_script: false,
            target_column: None,
            profile_id: Some("developer".into()),
            enabled: true,
            last_scheduled_at: None,
            next_run_at: Some(1_700_000_000_000),
            last_run_id: None,
            run_count: 0,
            queued_triggers: 0,
            updated_at: 1,
        };
        store.db().save(&state).unwrap();
        let snap = store.snapshot().unwrap();
        assert_eq!(snap.schedule_states, vec![state]);
        drop(store);

        assert_eq!(migrate(home.path(), 19).unwrap(), 19);
        assert_eq!(migrate(home.path(), 20).unwrap(), 20);
    }

    #[test]
    fn schema_21_creates_activity_segments_and_round_trips_cleanly() {
        let home = tempfile::tempdir().unwrap();
        let store = crate::Store::open(home.path()).unwrap();
        assert_eq!(version(&store.conn).unwrap(), CURRENT);
        let seg = canban_types::ActivitySegment {
            id: None,
            kind: canban_types::ActivityKind::Dwell,
            start_ms: 1_000,
            end_ms: 10_000,
            project_id: Some("missing-project".into()),
            card_id: Some("missing-card".into()),
            terminal_id: None,
            open: false,
            data: canban_types::ActivityDetail::Dwell(canban_types::DwellData {
                lane: canban_types::DwellLane::Card,
                page: "card".into(),
            }),
        };
        assert_eq!(store.db().insert_activity_segments(&[seg]).unwrap(), 1);
        let (rows, truncated) = store
            .db()
            .query_activity_segments(0, 20_000, None, None, 100)
            .unwrap();
        assert!(!truncated);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].project_id.as_deref(), Some("missing-project"));
        drop(store);

        assert_eq!(migrate(home.path(), 20).unwrap(), 20);
        assert_eq!(migrate(home.path(), 21).unwrap(), 21);
    }

    #[test]
    fn schema_22_removes_legacy_remote_machine_fields_and_round_trips_cleanly() {
        use serde_json::json;
        let home = tempfile::tempdir().unwrap();
        let store = crate::Store::open(home.path()).unwrap();
        assert_eq!(version(&store.conn).unwrap(), CURRENT);
        drop(store);

        assert_eq!(migrate(home.path(), 21).unwrap(), 21);
        let conn = Connection::open(home.path().join("canban.sqlite")).unwrap();
        let project_v21 = json!({
            "id": "p1",
            "name": "Demo",
            "directory": "/repo/p1",
            "location": {"machine": "local", "directory": "/repo/p1"},
            "workflow": canban_types::Workflow::default(),
            "created_at": 1
        });
        conn.execute(
            "INSERT INTO projects(id, project_id, card_id, state, number, data) VALUES('p1', NULL, NULL, '', 0, ?1)",
            [serde_json::to_string(&project_v21).unwrap()],
        )
        .unwrap();

        let ws_v21 = json!({
            "location": {"machine": "local", "directory": "/repo/p1"},
            "path": "/wt/1",
            "branch": "card/1",
            "base_commit": "abc",
            "target_branch": "main"
        });
        let pr_v21 = json!({
            "repository": "org/repo",
            "repository_id": 42,
            "number": 7,
            "url": "https://github.com/org/repo/pull/7",
            "state": "open",
            "head": "def",
            "head_branch": "card/1",
            "base_branch": "main",
            "merged": false,
            "checks": "success",
            "workspace": ws_v21
        });
        let card_v21 = json!({
            "id": "c1",
            "project_id": "p1",
            "number": 1,
            "title": "Card 1",
            "body": "",
            "stage": "review",
            "revision": 2,
            "workspace": ws_v21,
            "workspace_history": [ws_v21],
            "pull_request": pr_v21,
            "pull_request_history": [pr_v21],
            "attention": null,
            "archived_at": null,
            "archive": null,
            "archive_error": null,
            "worktree": "/wt/1",
            "created_at": 2
        });
        conn.execute(
            "INSERT INTO cards(id, project_id, card_id, state, number, data) VALUES('c1', 'p1', NULL, 'review', 1, ?1)",
            [serde_json::to_string(&card_v21).unwrap()],
        )
        .unwrap();

        let wt_v21 = json!({
            "id": "wt1",
            "project_id": "p1",
            "branch": "feat/x",
            "base_branch": "main",
            "base_commit": "abc",
            "path": "/wt/managed",
            "location": {"machine": "local", "directory": "/repo/p1"},
            "created_at": 3
        });
        conn.execute(
            "INSERT INTO managed_worktrees(id, project_id, card_id, state, number, data) VALUES('wt1', 'p1', NULL, '', 0, ?1)",
            [serde_json::to_string(&wt_v21).unwrap()],
        )
        .unwrap();

        let agent_term_v21 = json!({
            "id": "c1:developer",
            "conversation_id": "c1:developer",
            "number": 1,
            "name": null,
            "runtime": null,
            "agent": {
                "project_id": "p1",
                "card_id": "c1",
                "number": 1,
                "state": "completed",
                "core_id": "core-1",
                "owner_core_id": "core-1",
                "runtime_home": "/home/user/.canban",
                "location": {"machine": "local", "directory": "/repo/p1"},
                "workspace": ws_v21,
                "terminal": "pty-1",
                "cwd": "/wt/1",
                "input_path": "in.md",
                "updated_at": 4
            },
            "created_at": 4
        });
        let shell_term_v21 = json!({
            "id": "shell-1",
            "conversation_id": null,
            "number": 2,
            "name": "Shell",
            "runtime": {
                "machine": "local",
                "owner_core_id": "core-1",
                "core_id": "core-1",
                "terminal": "pty-2",
                "state": "running",
                "connection": "connected",
                "error": null
            },
            "agent": null,
            "created_at": 5
        });
        for (tid, num, state, row) in [
            ("c1:developer", 1, "completed", agent_term_v21),
            ("shell-1", 2, "running", shell_term_v21),
        ] {
            conn.execute(
                "INSERT INTO terminals(id, project_id, card_id, state, number, data) VALUES(?1, 'p1', NULL, ?2, ?3, ?4)",
                params![tid, state, num, serde_json::to_string(&row).unwrap()],
            )
            .unwrap();
        }
        drop(conn);

        assert_eq!(migrate(home.path(), 22).unwrap(), 22);
        let store = crate::Store::open(home.path()).unwrap();
        let snap = store.snapshot().unwrap();
        assert_eq!(snap.projects.len(), 1);
        assert_eq!(snap.projects[0].directory, "/repo/p1");
        assert_eq!(
            snap.cards[0].workspace.as_ref().unwrap().directory,
            "/repo/p1"
        );
        assert_eq!(snap.cards[0].workspace_history[0].directory, "/repo/p1");
        assert_eq!(
            snap.cards[0]
                .pull_request
                .as_ref()
                .unwrap()
                .workspace
                .directory,
            "/repo/p1"
        );
        assert_eq!(
            snap.cards[0].pull_request_history[0].workspace.directory,
            "/repo/p1"
        );
        assert_eq!(snap.worktrees[0].directory, "/repo/p1");
        assert_eq!(
            snap.terminals[0]
                .agent
                .as_ref()
                .unwrap()
                .directory
                .as_deref(),
            Some("/repo/p1")
        );
        assert_eq!(
            snap.terminals[0]
                .agent
                .as_ref()
                .unwrap()
                .workspace
                .as_ref()
                .unwrap()
                .directory,
            "/repo/p1"
        );
        for (tbl, id) in [
            ("projects", "p1"),
            ("cards", "c1"),
            ("managed_worktrees", "wt1"),
            ("terminals", "c1:developer"),
            ("terminals", "shell-1"),
        ] {
            let raw: String = store
                .conn
                .query_row(&format!("SELECT data FROM {tbl} WHERE id=?1"), [id], |r| {
                    r.get(0)
                })
                .unwrap();
            let v: Value = serde_json::from_str(&raw).unwrap();
            let text = v.to_string();
            assert!(!text.contains("\"location\""), "{tbl}/{id}: {text}");
            assert!(!text.contains("\"machine\""), "{tbl}/{id}: {text}");
            assert!(!text.contains("\"owner_core_id\""), "{tbl}/{id}: {text}");
            assert!(!text.contains("\"runtime_home\""), "{tbl}/{id}: {text}");
        }
        drop(store);

        assert_eq!(migrate(home.path(), 21).unwrap(), 21);
        let conn = Connection::open(home.path().join("canban.sqlite")).unwrap();
        let p_raw: String = conn
            .query_row("SELECT data FROM projects WHERE id='p1'", [], |r| r.get(0))
            .unwrap();
        let p_val: Value = serde_json::from_str(&p_raw).unwrap();
        assert_eq!(
            p_val["location"],
            json!({"machine": "local", "directory": "/repo/p1"})
        );
        let t_raw: String = conn
            .query_row("SELECT data FROM terminals WHERE id='shell-1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let t_val: Value = serde_json::from_str(&t_raw).unwrap();
        assert_eq!(t_val["runtime"]["machine"], json!("local"));
        assert_eq!(t_val["runtime"]["owner_core_id"], json!("core-1"));
        drop(conn);

        assert_eq!(migrate(home.path(), 22).unwrap(), 22);
    }

    #[test]
    fn schema_23_creates_vault_and_env_entries_and_round_trips_cleanly() {
        let home = tempfile::tempdir().unwrap();
        let store = crate::Store::open(home.path()).unwrap();
        assert_eq!(version(&store.conn).unwrap(), CURRENT);
        let entries = std::collections::BTreeMap::from([(
            "GLOBAL_TOKEN".into(),
            canban_types::EnvWriteEntry::Secret {
                value: Some("tok-1234567890abcdef".into()),
                keep_existing: false,
            },
        )]);
        let meta = crate::env::apply_env_writes(
            &store.conn,
            home.path(),
            crate::env::SCOPE_GLOBAL,
            "",
            &entries,
            10,
        )
        .unwrap();
        assert!(meta.contains_key("GLOBAL_TOKEN"));
        let snap = store.snapshot().unwrap();
        assert_eq!(snap.global_env, meta);
        drop(store);

        assert_eq!(migrate(home.path(), 22).unwrap(), 22);
        let conn = Connection::open(home.path().join("canban.sqlite")).unwrap();
        assert!(!table_exists(&conn, "vault_entries").unwrap());
        assert!(!table_exists(&conn, "env_entries").unwrap());
        drop(conn);

        assert_eq!(migrate(home.path(), 23).unwrap(), 23);
        let store = crate::Store::open(home.path()).unwrap();
        assert!(store.snapshot().unwrap().global_env.is_empty());
    }
}
