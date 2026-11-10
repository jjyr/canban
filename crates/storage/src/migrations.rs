//! Paired database-only migrations after the legacy schema-8 baseline.
use anyhow::{Result, ensure};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::path::Path;
pub const BASELINE: u32 = 8;
pub const CURRENT: u32 = BASELINE;
pub struct Migration {
    pub version: u32,
    pub up: fn(&Transaction<'_>) -> Result<()>,
    pub down: fn(&Transaction<'_>) -> Result<()>,
}
// Append paired steps here; retain down steps for the supported rollback floor.
const STEPS: &[Migration] = &[];
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

#[cfg(test)]
mod tests {
    use super::*;
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
