use anyhow::{Context, Result};
use canban_types::*;
use rusqlite::{Connection, params};
use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

pub trait Record: Serialize + DeserializeOwned {
    const TABLE: &'static str;
    fn id(&self) -> &str;
    fn project(&self) -> Option<&str> {
        None
    }
    fn card(&self) -> Option<&str> {
        None
    }
    fn state(&self) -> &str {
        ""
    }
    fn number(&self) -> u64 {
        0
    }
}
macro_rules! record {
    ($ty:ty,$table:literal) => {
        impl Record for $ty {
            const TABLE: &'static str = $table;
            fn id(&self) -> &str {
                &self.id
            }
        }
    };
    ($ty:ty,$table:literal,project) => {
        impl Record for $ty {
            const TABLE: &'static str = $table;
            fn id(&self) -> &str {
                &self.id
            }
            fn project(&self) -> Option<&str> {
                Some(&self.project_id)
            }
        }
    };
    ($ty:ty,$table:literal,card) => {
        impl Record for $ty {
            const TABLE: &'static str = $table;
            fn id(&self) -> &str {
                &self.id
            }
            fn project(&self) -> Option<&str> {
                Some(&self.project_id)
            }
            fn card(&self) -> Option<&str> {
                Some(&self.card_id)
            }
        }
    };
}
record!(Project, "projects");
record!(Note, "notes", project);
record!(NoteRevision, "note_revisions", project);
record!(NoteEvent, "note_events", project);
record!(Notice, "notifications", card);
record!(Artifact, "artifacts", card);
record!(Report, "reports", card);
record!(Conversation, "conversations", card);
record!(Question, "questions", card);
impl Record for Card {
    const TABLE: &'static str = "cards";
    fn id(&self) -> &str {
        &self.id
    }
    fn project(&self) -> Option<&str> {
        Some(&self.project_id)
    }
    fn state(&self) -> &str {
        &self.stage
    }
    fn number(&self) -> u64 {
        self.number
    }
}
impl Record for Event {
    const TABLE: &'static str = "events";
    fn id(&self) -> &str {
        &self.id
    }
    fn project(&self) -> Option<&str> {
        Some(&self.project_id)
    }
    fn card(&self) -> Option<&str> {
        Some(&self.card_id)
    }
    fn number(&self) -> u64 {
        self.sequence
    }
}
impl Record for Request {
    const TABLE: &'static str = "requests";
    fn id(&self) -> &str {
        &self.id
    }
    fn project(&self) -> Option<&str> {
        Some(&self.project_id)
    }
    fn card(&self) -> Option<&str> {
        Some(&self.card_id)
    }
    fn state(&self) -> &str {
        &self.state
    }
}
impl Record for Attempt {
    const TABLE: &'static str = "attempts";
    fn id(&self) -> &str {
        &self.id
    }
    fn project(&self) -> Option<&str> {
        Some(&self.project_id)
    }
    fn card(&self) -> Option<&str> {
        Some(&self.card_id)
    }
    fn state(&self) -> &str {
        &self.state
    }
}

pub struct Store {
    conn: Connection,
    pub home: PathBuf,
}
#[derive(Clone, Copy)]
pub struct Db<'a> {
    pub conn: &'a Connection,
}
impl Store {
    pub fn open(home: &Path) -> Result<Self> {
        std::fs::create_dir_all(home)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))?;
        }
        let conn = Connection::open(home.join("canban.sqlite"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
        )?;
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        anyhow::ensure!(mode == "wal", "SQLite WAL could not be enabled");
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        anyhow::ensure!(
            version <= 3,
            "Database schema is newer than this application"
        );
        if version == 0 {
            conn.execute_batch("BEGIN IMMEDIATE; CREATE TABLE counters (id TEXT PRIMARY KEY, value INTEGER NOT NULL); CREATE TABLE operations (id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, result TEXT NOT NULL);")?;
            for table in [
                "projects",
                "cards",
                "events",
                "requests",
                "attempts",
                "reports",
                "notes",
                "notifications",
                "artifacts",
            ] {
                let refs = if table == "projects" {
                    ""
                } else {
                    ", FOREIGN KEY(project_id) REFERENCES projects(id)"
                };
                let card_ref = if matches!(table, "projects" | "cards" | "notes") {
                    ""
                } else {
                    ", FOREIGN KEY(card_id) REFERENCES cards(id)"
                };
                conn.execute_batch(&format!("CREATE TABLE {table} (id TEXT PRIMARY KEY, project_id TEXT, card_id TEXT, state TEXT NOT NULL, number INTEGER NOT NULL, data TEXT NOT NULL{refs}{card_ref}); CREATE INDEX {table}_scope ON {table}(project_id,card_id,state);"))?;
            }
            conn.execute_batch("CREATE UNIQUE INDEX card_numbers ON cards(project_id,number); CREATE UNIQUE INDEX event_order ON events(card_id,number); CREATE UNIQUE INDEX report_keys ON reports(json_extract(data,'$.request_key')); PRAGMA user_version=1; COMMIT;")?;
        }
        if version < 2 {
            conn.execute_batch("BEGIN IMMEDIATE; CREATE TABLE conversations (id TEXT PRIMARY KEY, project_id TEXT REFERENCES projects(id),card_id TEXT REFERENCES cards(id),state TEXT NOT NULL,number INTEGER NOT NULL,data TEXT NOT NULL); CREATE UNIQUE INDEX conversation_profiles ON conversations(card_id,json_extract(data,'$.profile_id')); CREATE TABLE questions (id TEXT PRIMARY KEY,project_id TEXT REFERENCES projects(id),card_id TEXT REFERENCES cards(id),state TEXT NOT NULL,number INTEGER NOT NULL,data TEXT NOT NULL); PRAGMA user_version=2; COMMIT;")?;
        }
        if version < 3 {
            conn.execute_batch("BEGIN IMMEDIATE; CREATE TABLE note_revisions (id TEXT PRIMARY KEY,project_id TEXT REFERENCES projects(id),card_id TEXT,state TEXT NOT NULL,number INTEGER NOT NULL,data TEXT NOT NULL); CREATE TABLE note_events (id TEXT PRIMARY KEY,project_id TEXT REFERENCES projects(id),card_id TEXT,state TEXT NOT NULL,number INTEGER NOT NULL,data TEXT NOT NULL); CREATE UNIQUE INDEX note_revision_order ON note_revisions(json_extract(data,'$.note_id'),json_extract(data,'$.revision')); CREATE UNIQUE INDEX note_event_order ON note_events(json_extract(data,'$.note_id'),json_extract(data,'$.sequence'));")?;
            let db = Db { conn: &conn };
            for n in db.list::<Note>()? {
                db.save(&NoteRevision {
                    id: format!("{}:{}", n.id, n.revision),
                    project_id: n.project_id.clone(),
                    note_id: n.id.clone(),
                    revision: n.revision,
                    title: n.title,
                    body: n.body,
                    created_at: n.created_at,
                })?;
            }
            conn.execute_batch("PRAGMA user_version=3; COMMIT;")?;
        }
        Ok(Self {
            conn,
            home: home.to_owned(),
        })
    }
    pub fn db(&self) -> Db<'_> {
        Db { conn: &self.conn }
    }
    pub fn write<T>(&mut self, f: impl FnOnce(Db<'_>) -> Result<T>) -> Result<T> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let out = f(Db { conn: &tx })?;
        tx.commit()?;
        Ok(out)
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.db().snapshot()
    }
}
impl Db<'_> {
    pub fn save<T: Record>(&self, record: &T) -> Result<()> {
        self.conn.execute(&format!("INSERT INTO {}(id,project_id,card_id,state,number,data) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id,card_id=excluded.card_id,state=excluded.state,number=excluded.number,data=excluded.data",T::TABLE),params![record.id(),record.project(),record.card(),record.state(),record.number(),serde_json::to_string(record)?])?;
        Ok(())
    }
    pub fn get<T: Record>(&self, id: &str) -> Result<T> {
        let json: String = self
            .conn
            .query_row(
                &format!("SELECT data FROM {} WHERE id=?1", T::TABLE),
                [id],
                |r| r.get(0),
            )
            .with_context(|| format!("{} not found: {id}", T::TABLE))?;
        Ok(serde_json::from_str(&json)?)
    }
    pub fn list<T: Record>(&self) -> Result<Vec<T>> {
        let mut q = self
            .conn
            .prepare(&format!("SELECT data FROM {} ORDER BY rowid", T::TABLE))?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn counter(&self, key: &str) -> Result<u64> {
        self.conn.execute("INSERT INTO counters(id,value) VALUES(?1,1) ON CONFLICT(id) DO UPDATE SET value=value+1",[key])?;
        Ok(self
            .conn
            .query_row("SELECT value FROM counters WHERE id=?1", [key], |r| {
                r.get(0)
            })?)
    }
    pub fn append(&self, mut event: Event) -> Result<Event> {
        event.sequence = self.counter(&format!("timeline:{}", event.card_id))?;
        self.save(&event)?;
        Ok(event)
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        Ok(Snapshot {
            projects: self.list()?,
            cards: self.list()?,
            events: self.list()?,
            requests: self.list()?,
            attempts: self.list()?,
            reports: self.list()?,
            notes: self.list()?,
            notices: self.list()?,
            artifacts: self.list()?,
            conversations: self.list()?,
            questions: self.list()?,
            note_events: self.list()?,
        })
    }
}
