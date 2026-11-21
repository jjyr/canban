pub mod migrations;
use anyhow::{Context, Result};
use canban_types::*;
use rusqlite::{Connection, params};
use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

pub trait Record: Serialize + DeserializeOwned {
    const TABLE: &'static str;
    fn data(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }
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
record!(WorkflowSource, "workflow_sources", project);
record!(WorkflowTemplate, "workflow_templates");
record!(ConfigurationSession, "configuration_sessions", project);
record!(MergeBatch, "merge_batches", project);
record!(ScheduleState, "schedule_states", project);
impl Record for ScheduledRun {
    const TABLE: &'static str = "scheduled_runs";
    fn id(&self) -> &str {
        &self.id
    }
    fn project(&self) -> Option<&str> {
        Some(&self.project_id)
    }
    fn card(&self) -> Option<&str> {
        self.card_id.as_deref()
    }
    fn state(&self) -> &str {
        &self.state
    }
    fn number(&self) -> u64 {
        self.number
    }
}
record!(WorkflowRevision, "workflow_revisions", project);
impl Record for WorkflowRun {
    const TABLE: &'static str = "workflow_runs";
    fn id(&self) -> &str {
        &self.id
    }
    fn project(&self) -> Option<&str> {
        Some(&self.project_id)
    }
    fn card(&self) -> Option<&str> {
        self.card_id.as_deref().filter(|s| !s.is_empty())
    }
    fn state(&self) -> &str {
        &self.state
    }
}
record!(EventSubscription, "event_subscriptions");
record!(EventDelivery, "event_deliveries");
record!(GitHubIssueSnapshot, "github_issues", project);
record!(ManagedWorktree, "managed_worktrees", project);
record!(Note, "notes", project);
record!(NoteRevision, "note_revisions", project);
record!(NoteEvent, "note_events", project);
impl Record for Notice {
    const TABLE: &'static str = "notifications";
    fn id(&self) -> &str {
        &self.id
    }
    fn project(&self) -> Option<&str> {
        (!self.project_id.is_empty() && !is_global_project_id(&self.project_id))
            .then_some(self.project_id.as_str())
    }
    fn card(&self) -> Option<&str> {
        (!self.card_id.is_empty()).then_some(self.card_id.as_str())
    }
}
record!(CardRead, "card_reads", card);
record!(Artifact, "artifacts", card);
impl Record for Report {
    const TABLE: &'static str = "reports";
    fn id(&self) -> &str {
        &self.id
    }
    fn project(&self) -> Option<&str> {
        (!self.project_id.is_empty() && !is_global_project_id(&self.project_id))
            .then_some(self.project_id.as_str())
    }
    fn card(&self) -> Option<&str> {
        (!self.card_id.is_empty()).then_some(self.card_id.as_str())
    }
}
record!(Conversation, "conversations", card);
record!(Question, "questions", card);
impl Record for TerminalEntry {
    const TABLE: &'static str = "terminals";
    fn id(&self) -> &str {
        &self.id
    }
    fn project(&self) -> Option<&str> {
        self.agent
            .as_ref()
            .map(|a| a.project_id.as_str())
            .or(self.origin_project_id.as_deref())
            .or_else(|| {
                self.id
                    .strip_prefix("merge-queue:")
                    .or_else(|| self.id.strip_prefix("configuration:"))
                    .or_else(|| self.id.strip_prefix("scheduled:"))
                    .and_then(|rest| rest.split(':').next())
            })
            .filter(|id| !id.is_empty() && !is_global_project_id(id))
    }
    fn card(&self) -> Option<&str> {
        self.agent.as_ref().map(|a| a.card_id.as_str())
    }
    fn state(&self) -> &str {
        if let Some(a) = &self.agent {
            &a.state
        } else if let Some(r) = &self.runtime {
            &r.state
        } else {
            ""
        }
    }
    fn number(&self) -> u64 {
        self.number
    }
}
impl Record for Card {
    const TABLE: &'static str = "cards";
    fn data(&self) -> Result<String> {
        let mut saved = self.clone();
        if let Some(source) = saved.github_issue() {
            saved.title = format!("{}#{}", source.repository, source.number);
        }
        // Delivery binding/checkpoint stays durable. Checks and review summaries are remote views.
        for pr in saved
            .pull_request
            .iter_mut()
            .chain(saved.pull_request_history.iter_mut())
        {
            pr.checks.clear();
            pr.checks_summary = None;
            pr.review_summary.clear();
            if matches!(pr.state.as_str(), "open" | "closed") {
                pr.state = "published".into();
            }
            pr.merged = false;
        }
        Ok(serde_json::to_string(&saved)?)
    }
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
    fn data(&self) -> Result<String> {
        anyhow::ensure!(
            !matches!(
                self.kind.as_str(),
                "github.comment" | "github.pr_comment" | "github.pr_review"
            ),
            "Remote discussions belong in the GitHub cache"
        );
        Ok(serde_json::to_string(self)?)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn project(&self) -> Option<&str> {
        (!self.project_id.is_empty() && !is_global_project_id(&self.project_id))
            .then_some(self.project_id.as_str())
    }
    fn card(&self) -> Option<&str> {
        (!self.card_id.is_empty()).then_some(self.card_id.as_str())
    }
    fn number(&self) -> u64 {
        self.sequence
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
        Self::open_with_progress(home, |_, _| {})
    }
    pub fn open_with_progress(
        home: &Path,
        mut on_progress: impl FnMut(&str, &str),
    ) -> Result<Self> {
        std::fs::create_dir_all(home)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))?;
        }
        on_progress("storage", "Opening canban.sqlite (WAL mode)…");
        let mut conn = Connection::open(home.join("canban.sqlite"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
        )?;
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        anyhow::ensure!(mode == "wal", "SQLite WAL could not be enabled");
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        anyhow::ensure!(
            version <= migrations::CURRENT as i64,
            "Database schema is newer than this application"
        );
        if version < migrations::CURRENT as i64 {
            on_progress(
                "migrate",
                &format!(
                    "Migrating SQLite schema (v{version} → v{})…",
                    migrations::CURRENT
                ),
            );
        }
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
        if version < 4 {
            conn.execute_batch("BEGIN IMMEDIATE; CREATE TABLE terminals (id TEXT PRIMARY KEY,project_id TEXT REFERENCES projects(id),card_id TEXT REFERENCES cards(id),state TEXT NOT NULL,number INTEGER NOT NULL UNIQUE,data TEXT NOT NULL);")?;
            let db = Db { conn: &conn };
            for c in db.list::<Conversation>()? {
                db.agent_terminal(&c)?;
            }
            conn.execute_batch("PRAGMA user_version=4; COMMIT;")?;
        }
        if version < 5 {
            conn.execute_batch("BEGIN IMMEDIATE;")?;
            let rows = {
                let mut q = conn.prepare("SELECT data FROM terminals")?;
                q.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let db = Db { conn: &conn };
            for row in rows {
                let item = TerminalEntry::from_json(serde_json::from_str(&row)?)?;
                db.save(&item)?;
            }
            conn.execute_batch("PRAGMA user_version=5; COMMIT;")?;
        }
        if version < 6 {
            conn.execute_batch("BEGIN IMMEDIATE; PRAGMA user_version=6; COMMIT;")?;
        }
        if version < 7 {
            conn.execute_batch("BEGIN IMMEDIATE; CREATE TABLE github_issues (id TEXT PRIMARY KEY, project_id TEXT REFERENCES projects(id), card_id TEXT, state TEXT NOT NULL, number INTEGER NOT NULL,data TEXT NOT NULL); CREATE UNIQUE INDEX github_card_source ON cards(project_id,json_extract(data,'$.content.source.repository_id'),json_extract(data,'$.content.source.issue_id')) WHERE json_extract(data,'$.content.kind')='github_issue'; PRAGMA user_version=7; COMMIT;")?;
        }
        if version < 8 {
            conn.execute_batch("BEGIN IMMEDIATE; CREATE TABLE card_reads (id TEXT PRIMARY KEY,project_id TEXT REFERENCES projects(id),card_id TEXT REFERENCES cards(id),state TEXT NOT NULL,number INTEGER NOT NULL,data TEXT NOT NULL); PRAGMA user_version=8;")?;
            // Existing history is a quiet baseline; upgrading must not turn every
            // historical Agent result into an unread notification.
            let db = Db { conn: &conn };
            let events = db.list::<Event>()?;
            let mut latest = std::collections::BTreeMap::<&str, u64>::new();
            for e in &events {
                latest
                    .entry(&e.card_id)
                    .and_modify(|seq| *seq = (*seq).max(e.sequence))
                    .or_insert(e.sequence);
            }
            for c in db.list::<Card>()? {
                db.save(&CardRead {
                    id: c.id.clone(),
                    project_id: c.project_id,
                    card_id: c.id.clone(),
                    sequence: latest.get(c.id.as_str()).copied().unwrap_or(0),
                })?;
            }
            conn.execute_batch("COMMIT;")?;
        }
        migrations::prepare(home, &conn, migrations::CURRENT)?;
        migrations::apply_with_progress(&mut conn, migrations::CURRENT, |step, target| {
            on_progress(
                "migrate",
                &format!("Applying schema migration v{step} / v{target}…"),
            );
        })?;
        migrations::sync_paths(home, &conn)?;
        on_progress(
            "storage_ready",
            &format!("SQLite WAL ready (schema v{})", migrations::CURRENT),
        );
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
    pub fn agent_terminal(&self, c: &Conversation) -> Result<()> {
        if !self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM terminals WHERE id=?1)",
            [&c.id],
            |r| r.get::<_, bool>(0),
        )? {
            self.save(&TerminalEntry {
                id: c.id.clone(),
                conversation_id: Some(c.id.clone()),
                number: self.counter("terminal-number")?,
                name: None,
                origin_project_id: None,
                origin_card_id: None,
                agent_engine: None,
                runtime: None,
                agent: None,
                created_at: c.created_at,
            })?;
        }
        Ok(())
    }
    pub fn save<T: Record>(&self, record: &T) -> Result<()> {
        anyhow::ensure!(
            T::TABLE != "github_issues",
            "GitHub source bodies belong in the disposable cache"
        );
        self.conn.execute(&format!("INSERT INTO {}(id,project_id,card_id,state,number,data) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id,card_id=excluded.card_id,state=excluded.state,number=excluded.number,data=excluded.data",T::TABLE),params![record.id(),record.project(),record.card(),record.state(),record.number(),record.data()?])?;
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
    /// Permanently remove a Project and all its Canban-side SQLite records in FK-safe order.
    /// Does not touch any user repository or Git worktrees on disk.
    pub fn delete_project(&self, project_id: &str) -> Result<usize> {
        let _project: Project = self.get(project_id)?;
        let card_ids: Vec<String> = {
            let mut q = self
                .conn
                .prepare("SELECT id FROM cards WHERE project_id=?1")?;
            let rows = q.query_map([project_id], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let sub_prefix = format!("source:{project_id}:");
        let sub_ids: Vec<String> = self
            .list::<EventSubscription>()?
            .into_iter()
            .filter(|s| {
                s.id.starts_with(&sub_prefix)
                    || s.selector.project_id.as_deref() == Some(project_id)
            })
            .map(|s| s.id)
            .collect();

        // 1. Delete event_journal and event_deliveries referencing project events or subscriptions.
        self.conn.execute(
            "DELETE FROM event_journal WHERE event_id IN (\
                SELECT id FROM events WHERE project_id=?1 \
                OR card_id IN (SELECT id FROM cards WHERE project_id=?1)\
             )",
            [project_id],
        )?;
        self.conn.execute(
            "DELETE FROM event_deliveries WHERE json_extract(data,'$.event_id') IN (\
                SELECT id FROM events WHERE project_id=?1 \
                OR card_id IN (SELECT id FROM cards WHERE project_id=?1)\
             )",
            [project_id],
        )?;
        for sub_id in &sub_ids {
            self.conn.execute(
                "DELETE FROM event_deliveries WHERE json_extract(data,'$.subscription_id')=?1",
                [sub_id],
            )?;
            self.conn
                .execute("DELETE FROM event_subscriptions WHERE id=?1", [sub_id])?;
        }

        // 2. Delete child tables referencing cards(id) and/or projects(id).
        for table in [
            "card_reads",
            "artifacts",
            "reports",
            "questions",
            "terminals",
            "conversations",
            "events",
            "notifications",
            "workflow_runs",
            "scheduled_runs",
            "managed_worktrees",
        ] {
            self.conn.execute(
                &format!(
                    "DELETE FROM {table} WHERE project_id=?1 \
                     OR card_id IN (SELECT id FROM cards WHERE project_id=?1)"
                ),
                [project_id],
            )?;
        }

        // 3. Delete project-scoped tables without card_id FK.
        for table in [
            "note_revisions",
            "note_events",
            "notes",
            "github_issues",
            "workflow_sources",
            "workflow_revisions",
            "configuration_sessions",
            "merge_batches",
            "schedule_states",
        ] {
            self.conn.execute(
                &format!("DELETE FROM {table} WHERE project_id=?1"),
                [project_id],
            )?;
        }

        // 4. Clean up non-FK auxiliary rows (drafts, counters, observations).
        self.conn.execute(
            "DELETE FROM editor_drafts WHERE id=?1",
            [format!("new-card:{project_id}")],
        )?;
        self.conn.execute(
            "DELETE FROM counters WHERE id=?1",
            [format!("cards:{project_id}")],
        )?;
        for cid in &card_ids {
            self.conn.execute(
                "DELETE FROM editor_drafts WHERE id=?1 OR id=?2",
                params![format!("card:{cid}"), format!("review-draft:{cid}")],
            )?;
            self.conn.execute(
                "DELETE FROM counters WHERE id=?1",
                [format!("timeline:{cid}")],
            )?;
        }
        self.conn.execute(
            "DELETE FROM source_observations WHERE id=?1 OR id LIKE ?2",
            params![
                format!("github:{project_id}"),
                format!("github:{project_id}:%")
            ],
        )?;

        // 5. Delete cards and finally the project itself.
        self.conn
            .execute("DELETE FROM cards WHERE project_id=?1", [project_id])?;
        self.conn
            .execute("DELETE FROM projects WHERE id=?1", [project_id])?;
        Ok(card_ids.len())
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
        if !event.card_id.is_empty() {
            event.sequence = self.counter(&format!("timeline:{}", event.card_id))?;
            if let Ok(card) = self.get::<Card>(&event.card_id)
                && event.data.is_object()
            {
                event.data["stage"] = serde_json::json!(card.stage);
                event.data["source_kind"] = serde_json::json!(match card.content {
                    CardContent::Local => "local",
                    CardContent::GitHubIssue { .. } => "github.issue",
                    CardContent::GitHubPr { .. } => "github.pr",
                });
                event.data["card_revision"] = serde_json::json!(card.revision);
            }
        }
        if event.data.is_object() && event.data.get("reason").is_none() {
            let reason = match event.kind.as_str() {
                "card.created" => Some("manual"),
                "card.restored" => Some("restoration"),
                "card.archived" => Some("archive"),
                "card.removed" => Some("removal"),
                "card.resolved" => Some("resolution"),
                "card.reopened" => Some("reopen"),
                _ => None,
            };
            if let Some(reason) = reason {
                event.data["reason"] = serde_json::json!(reason);
            }
        }
        if event.data.is_object() && event.data.get("origin").is_none() {
            event.data["origin"] = serde_json::json!(if event.actor == "github" {
                "github"
            } else {
                "local"
            });
        }
        if event.data.get("workflow_run_id").is_none()
            && let Some(entry) = event.data["terminal_id"]
                .as_str()
                .and_then(|id| self.get::<TerminalEntry>(id).ok())
            && let Some(agent) = entry.agent
            && let Some(run) = agent.workflow_run_id
        {
            event.data["workflow_run_id"] = serde_json::json!(run);
            event.data["workflow_step"] = serde_json::json!(agent.workflow_step);
        }
        if event.data.get("workflow_run_id").is_some() && event.data["origin"] == "local" {
            event.data["origin"] = serde_json::json!("workflow");
        }
        if let Some(revision) = event.data.get("after_revision").cloned() {
            event.data["card_revision"] = revision;
        }
        self.save(&event)?;
        self.conn.execute(
            "INSERT INTO event_journal(event_id) VALUES(?1)",
            [&event.id],
        )?;
        Ok(event)
    }
    /// The scheduler's admission inputs, excluding unrelated content/history.
    /// Keep questions: pending assignments affect idle-session slot eligibility.
    pub fn admission_snapshot(&self) -> Result<Snapshot> {
        Ok(Snapshot {
            cards: self.list()?,
            terminals: self.list()?,
            workflow_runs: self.list()?,
            configuration_sessions: self.list()?,
            merge_batches: self.list()?,
            scheduled_runs: self.list()?,
            questions: self.list()?,
            ..Default::default()
        })
    }

    /// Terminal naming and directory association inputs, without Timeline,
    /// reports, workflow revisions or other unrelated history.
    pub fn terminal_snapshot(&self) -> Result<Snapshot> {
        Ok(Snapshot {
            projects: self.list()?,
            cards: self
                .list::<Card>()?
                .into_iter()
                .filter(|c| c.removed_at.is_none())
                .collect(),
            terminals: self.list()?,
            worktrees: self.list()?,
            configuration_sessions: self.list()?,
            merge_batches: self.list()?,
            scheduled_runs: self.list()?,
            ..Default::default()
        })
    }

    /// Deadline decisions need current runtime and report timestamps, not Timeline bodies.
    pub fn runtime_snapshot(&self) -> Result<Snapshot> {
        let mut snapshot = self.admission_snapshot()?;
        snapshot.projects = self.list()?;
        snapshot.reports = self.list()?;
        snapshot.schedule_states = self.list()?;
        Ok(snapshot)
    }

    pub fn events_for_card(&self, card_id: &str) -> Result<Vec<Event>> {
        let mut q = self
            .conn
            .prepare("SELECT data FROM events WHERE card_id=?1 ORDER BY number")?;
        let rows = q.query_map([card_id], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    pub fn card_events(&self) -> Result<Vec<Event>> {
        let mut q = self.conn.prepare(
            "SELECT data FROM events WHERE card_id IS NOT NULL ORDER BY card_id, number",
        )?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    pub fn prune_cardless_events(
        &self,
        max_age_ms: u64,
        keep_recent: usize,
        max_journal_offset: u64,
    ) -> Result<usize> {
        let cutoff = now().saturating_sub(max_age_ms);
        let mut q = self.conn.prepare(
            "SELECT e.id, COALESCE(CAST(json_extract(e.data, '$.created_at') AS INTEGER), 0) AS created_at \
             FROM events e \
             JOIN event_journal j ON j.event_id = e.id \
             WHERE e.card_id IS NULL \
               AND j.offset <= ?1 \
               AND NOT EXISTS ( \
                   SELECT 1 FROM event_deliveries d \
                   WHERE json_extract(d.data, '$.event_id') = e.id \
                     AND json_extract(d.data, '$.state') = 'pending' \
               ) \
             ORDER BY j.offset DESC",
        )?;
        let rows = q
            .query_map(params![max_journal_offset], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let to_delete: Vec<String> = rows
            .into_iter()
            .enumerate()
            .filter_map(|(idx, (id, created_at))| {
                if idx >= keep_recent || created_at < cutoff {
                    Some(id)
                } else {
                    None
                }
            })
            .collect();
        if to_delete.is_empty() {
            return Ok(0);
        }
        // Delete from event_journal before events (FK event_journal.event_id REFERENCES events(id)).
        let mut del_journal = self
            .conn
            .prepare("DELETE FROM event_journal WHERE event_id = ?1")?;
        let mut del_event = self.conn.prepare("DELETE FROM events WHERE id = ?1")?;
        for id in &to_delete {
            del_journal.execute([id])?;
            del_event.execute([id])?;
        }
        Ok(to_delete.len())
    }

    pub fn timeline_page(&self, card: &str, after: u64, limit: usize) -> Result<Vec<Event>> {
        let mut span = canban_profiler::Span::new("timeline.load");
        let mut query = self.conn.prepare(
            "SELECT data FROM events WHERE card_id=?1 AND number>?2 ORDER BY number LIMIT ?3",
        )?;
        let rows = query.query_map(rusqlite::params![card, after, limit as u64], |row| {
            row.get::<_, String>(0)
        })?;
        let mut bytes = 0;
        let events = rows
            .map(|row| {
                let json = row?;
                bytes += json.len();
                Ok(serde_json::from_str(&json)?)
            })
            .collect::<Result<Vec<Event>>>()?;
        span.counts(events.len(), bytes);
        Ok(events)
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let _span = canban_profiler::Span::new("state.load_db");
        let sources: Vec<WorkflowSource> = self.list()?;
        let mut query = self.conn.prepare(
            "SELECT r.data FROM workflow_revisions r WHERE r.id IN (SELECT json_extract(data,'$.revision_id') FROM workflow_sources) ORDER BY r.rowid",
        )?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        let active = rows
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect::<Result<Vec<WorkflowRevision>>>()?;
        let review_drafts = {
            let mut q = self.conn.prepare(
                "SELECT data FROM editor_drafts WHERE id LIKE 'review-draft:%' AND data != 'null' ORDER BY updated_at DESC, id",
            )?;
            let rows = q.query_map([], |row| row.get::<_, String>(0))?;
            rows.filter_map(|row| {
                let raw = row.ok()?;
                serde_json::from_str::<ReviewDraft>(&raw).ok()
            })
            .collect::<Vec<_>>()
        };
        Ok(Snapshot {
            github_sync: Default::default(),
            workflow_runs: self.list()?,
            workflow_revisions: active,
            workflow_sources: sources,
            workflow_templates: self.list()?,
            event_subscriptions: self.list()?,
            configuration_sessions: self.list()?,
            merge_batches: self.list()?,
            schedule_states: self.list()?,
            scheduled_runs: self.list()?,
            card_reads: self.list()?,
            github: Default::default(),
            github_issues: Vec::new(),
            worktrees: self.list()?,
            terminals: self.list()?,
            terminal_contexts: Vec::new(),
            merging_cards: Vec::new(),
            projects: self.list()?,
            cards: self
                .list::<Card>()?
                .into_iter()
                .filter(|c| c.removed_at.is_none())
                .collect(),
            reports: self.list()?,
            notes: self.list()?,
            notices: self.list()?,
            artifacts: self.list()?,
            conversations: self.list()?,
            questions: self.list()?,
            note_events: self.list()?,
            review_drafts,
            ..Default::default()
        })
    }

    pub fn insert_activity_segments(&self, segments: &[ActivitySegment]) -> Result<usize> {
        if segments.is_empty() {
            return Ok(0);
        }
        let mut stmt = self.conn.prepare(
            "INSERT INTO activity_segments(kind, start_ms, end_ms, project_id, card_id, terminal_id, data) \
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for seg in segments {
            let data = serde_json::to_string(&seg.data)?;
            stmt.execute(params![
                seg.kind.as_str(),
                seg.start_ms as i64,
                seg.end_ms as i64,
                seg.project_id.as_deref(),
                seg.card_id.as_deref(),
                seg.terminal_id.as_deref(),
                data,
            ])?;
        }
        Ok(segments.len())
    }

    pub fn query_activity_segments(
        &self,
        from_ms: u64,
        to_ms: u64,
        project_id: Option<&str>,
        kinds: Option<&[ActivityKind]>,
        limit: usize,
    ) -> Result<(Vec<ActivitySegment>, bool)> {
        let mut segments = Vec::new();
        if let Some([]) = kinds {
            return Ok((segments, false));
        }
        let kind_clause = activity_kind_clause(kinds);
        let sql = format!(
            "SELECT id, kind, start_ms, end_ms, project_id, card_id, terminal_id, data \
             FROM activity_segments \
             WHERE {kind_clause} AND end_ms > ?1 AND start_ms < ?2 \
               AND (?3 IS NULL OR project_id = ?3) \
             ORDER BY start_ms ASC, id ASC \
             LIMIT ?4"
        );
        let mut q = self.conn.prepare(&sql)?;
        let rows = q.query_map(
            params![
                from_ms as i64,
                to_ms as i64,
                project_id,
                (limit.saturating_add(1)) as i64
            ],
            decode_activity_row,
        )?;
        for row in rows {
            segments.push(row?);
        }
        let truncated = segments.len() > limit;
        if truncated {
            segments.truncate(limit);
        }
        Ok((segments, truncated))
    }

    pub fn for_each_activity_segment(
        &self,
        from_ms: u64,
        to_ms: u64,
        project_id: Option<&str>,
        kinds: Option<&[ActivityKind]>,
        mut f: impl FnMut(ActivitySegment) -> Result<()>,
    ) -> Result<()> {
        if let Some([]) = kinds {
            return Ok(());
        }
        let kind_clause = activity_kind_clause(kinds);
        let sql = format!(
            "SELECT id, kind, start_ms, end_ms, project_id, card_id, terminal_id, data \
             FROM activity_segments \
             WHERE {kind_clause} AND end_ms > ?1 AND start_ms < ?2 \
               AND (?3 IS NULL OR project_id = ?3) \
             ORDER BY start_ms ASC, id ASC"
        );
        let mut q = self.conn.prepare(&sql)?;
        let rows = q.query_map(
            params![from_ms as i64, to_ms as i64, project_id],
            decode_activity_row,
        )?;
        for row in rows {
            f(row?)?;
        }
        Ok(())
    }

    pub fn prune_activity_segments(&self, max_age_ms: u64) -> Result<usize> {
        let cutoff = now().saturating_sub(max_age_ms);
        let deleted = self.conn.execute(
            "DELETE FROM activity_segments WHERE end_ms < ?1",
            params![cutoff as i64],
        )?;
        Ok(deleted)
    }
}

fn activity_kind_clause(kinds: Option<&[ActivityKind]>) -> &'static str {
    match kinds {
        Some([ActivityKind::Dwell]) => "kind = 'dwell'",
        Some([ActivityKind::Agent]) => "kind = 'agent'",
        _ => "kind IN ('dwell', 'agent')",
    }
}

fn decode_activity_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ActivitySegment> {
    let row_id: i64 = row.get(0)?;
    let kind_raw: String = row.get(1)?;
    let start_ms: i64 = row.get(2)?;
    let end_ms: i64 = row.get(3)?;
    let project_id: Option<String> = row.get(4)?;
    let card_id: Option<String> = row.get(5)?;
    let terminal_id: Option<String> = row.get(6)?;
    let data_raw: String = row.get(7)?;
    let kind = match kind_raw.as_str() {
        "dwell" => ActivityKind::Dwell,
        "agent" => ActivityKind::Agent,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                format!("unknown activity kind '{other}'").into(),
            ));
        }
    };
    let data: ActivityDetail = serde_json::from_str(&data_raw).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(ActivitySegment {
        id: Some(row_id),
        kind,
        start_ms: u64::try_from(start_ms).unwrap_or(0),
        end_ms: u64::try_from(end_ms).unwrap_or(0),
        project_id,
        card_id,
        terminal_id,
        open: false,
        data,
    })
}
