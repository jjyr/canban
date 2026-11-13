//! Paired database-only migrations after the legacy schema-8 baseline.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::path::Path;
pub const BASELINE: u32 = 8;
pub const CURRENT: u32 = 14;
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
];
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
    ensure!(
        CURRENT == STEPS.last().map_or(BASELINE, |s| s.version),
        "Migration registry and CURRENT schema disagree"
    );
    batch(conn, STEPS, target)
}
fn batch(conn: &mut Connection, steps: &[Migration], target: u32) -> Result<()> {
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
            (step.up)(&tx)?;
            tx.pragma_update(None, "user_version", step.version)?;
        }
    } else {
        for step in steps
            .iter()
            .rev()
            .filter(|s| s.version > target && s.version <= before)
        {
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
                if let Some(n) = att.filter(|n| *n > 0) {
                    format!("{s}:{n}")
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
                if let Some(n) = att.filter(|n| *n > 0) {
                    format!("{s}:{n}")
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
        assert_eq!(CURRENT, 14);
        assert_eq!(version(&store.conn).unwrap(), 14);

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

        // Roundtrip 14 -> 13 -> 14
        apply(&mut store.conn, 13).unwrap();
        assert_eq!(version(&store.conn).unwrap(), 13);
        let req_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0))
            .unwrap();
        assert_eq!(req_count, 2);
        apply(&mut store.conn, 14).unwrap();
        assert_eq!(version(&store.conn).unwrap(), 14);
    }
}
