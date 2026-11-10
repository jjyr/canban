//! Paired database-only migrations after the legacy schema-8 baseline.
use anyhow::{Result, ensure};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::path::Path;
pub const BASELINE: u32 = 8;
pub const CURRENT: u32 = 10;
pub struct Migration {
    pub version: u32,
    pub up: fn(&Transaction<'_>) -> Result<()>,
    pub down: fn(&Transaction<'_>) -> Result<()>,
}
// Append paired steps here; retain down steps for the supported rollback floor.
const STEPS: &[Migration] = &[
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
];
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
    let mut conn = Connection::open(home.join("canban.sqlite"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    prepare(home, &conn, target)?;
    apply(&mut conn, target)?;
    let result = version(&conn)?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(result)
}
pub fn database_version(home: &Path) -> Result<u32> {
    let conn = Connection::open_with_flags(
        home.join("canban.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    version(&conn)
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
}
