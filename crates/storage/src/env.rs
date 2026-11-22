//! Unified environment metadata storage (`env_entries`) backed by `crate::vault`
//! for Global, CLI Agent / `@Identity`, and Project scopes.
use crate::vault;
use anyhow::{Context, Result, bail, ensure};
use canban_types::{EnvEntryMeta, EnvWriteEntry, Snapshot, is_valid_env_key};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use zeroize::Zeroizing;

pub const NAMESPACE_ENV: &str = "env";
pub const SCOPE_GLOBAL: &str = "global";
pub const SCOPE_AGENT: &str = "agent";
pub const SCOPE_PROJECT: &str = "project";

pub fn normalize_target_id<'a>(scope: &str, target_id: &'a str) -> &'a str {
    if scope == SCOPE_GLOBAL { "" } else { target_id }
}

pub fn vault_scope(scope: &str, target_id: &str) -> String {
    match scope {
        SCOPE_GLOBAL => SCOPE_GLOBAL.to_owned(),
        _ => format!("{scope}:{}", normalize_target_id(scope, target_id)),
    }
}

#[derive(Clone, Debug)]
pub struct GlobalEnvSyncRecord {
    pub key: String,
    pub kind: String,
    pub value: Option<Zeroizing<String>>,
    pub updated_at: u64,
    pub deleted: bool,
}

/// Wire payload used exclusively by `env.global.sync` to replicate Global env & secrets.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GlobalEnvSyncWireRecord {
    pub key: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub updated_at: u64,
    #[serde(default)]
    pub deleted: bool,
}

impl GlobalEnvSyncRecord {
    pub fn to_wire(&self, tcp_token: Option<&str>) -> Result<GlobalEnvSyncWireRecord> {
        let value = match (&self.value, self.kind.as_str(), self.deleted) {
            (_, _, true) | (None, _, false) => None,
            (Some(v), "secret", false) => match tcp_token {
                Some(tok) => Some(vault::seal_transport(tok, &self.key, v)?),
                None => Some(v.as_str().to_owned()),
            },
            (Some(v), _, false) => Some(v.as_str().to_owned()),
        };
        Ok(GlobalEnvSyncWireRecord {
            key: self.key.clone(),
            kind: self.kind.clone(),
            value,
            updated_at: self.updated_at,
            deleted: self.deleted,
        })
    }

    pub fn from_wire(wire: GlobalEnvSyncWireRecord, tcp_token: Option<&str>) -> Result<Self> {
        ensure!(
            is_valid_env_key(&wire.key),
            "Invalid synced environment key '{}'",
            wire.key
        );
        ensure!(
            matches!(wire.kind.as_str(), "plain" | "secret"),
            "Unsupported synced environment kind '{}'",
            wire.kind
        );
        let value = match wire.value {
            None => None,
            Some(raw) => {
                if raw.starts_with(vault::TRANSPORT_SEAL_PREFIX) {
                    let tok = tcp_token.context(
                        "Received transport-encrypted secret without server authentication token",
                    )?;
                    Some(vault::open_transport(tok, &wire.key, &raw)?)
                } else {
                    Some(Zeroizing::new(raw))
                }
            }
        };
        Ok(Self {
            key: wire.key,
            kind: wire.kind,
            value,
            updated_at: wire.updated_at,
            deleted: wire.deleted,
        })
    }
}

pub fn list_decrypted_scope(
    conn: &Connection,
    home: &Path,
    scope: &str,
    target_id: &str,
) -> Result<BTreeMap<String, (Zeroizing<String>, u64)>> {
    vault::list_decrypted(conn, home, NAMESPACE_ENV, &vault_scope(scope, target_id))
}

pub fn delete_scope(conn: &Connection, scope: &str, target_id: &str) -> Result<()> {
    let tid = normalize_target_id(scope, target_id);
    conn.execute(
        "DELETE FROM env_entries WHERE scope=?1 AND target_id=?2",
        params![scope, tid],
    )?;
    vault::delete_scope(conn, NAMESPACE_ENV, &vault_scope(scope, tid))
}

pub fn list_env_meta(
    conn: &Connection,
    scope: &str,
    target_id: &str,
) -> Result<BTreeMap<String, EnvEntryMeta>> {
    let tid = normalize_target_id(scope, target_id);
    let hints = vault::list_meta(conn, NAMESPACE_ENV, &vault_scope(scope, tid))?;
    let mut q = conn.prepare(
        "SELECT key, kind, value FROM env_entries \
         WHERE scope=?1 AND target_id=?2 AND deleted=0 ORDER BY key",
    )?;
    let rows = q.query_map(params![scope, tid], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (key, kind, value) = row?;
        let meta = match kind.as_str() {
            "plain" => EnvEntryMeta::Plain {
                value: value.unwrap_or_default(),
            },
            "secret" => match hints.get(&key) {
                Some((hint, _)) => EnvEntryMeta::Secret {
                    configured: true,
                    hint: hint.clone(),
                },
                None => EnvEntryMeta::Secret {
                    configured: false,
                    hint: String::new(),
                },
            },
            "unset" => EnvEntryMeta::Unset,
            _ => continue,
        };
        out.insert(key, meta);
    }
    Ok(out)
}

pub type SnapshotEnvMaps = (
    BTreeMap<String, EnvEntryMeta>,
    BTreeMap<String, BTreeMap<String, EnvEntryMeta>>,
);

/// Returns `(global_env, scoped_env)` for `Snapshot` in two indexed queries.
pub fn list_all_snapshot_env(conn: &Connection) -> Result<SnapshotEnvMaps> {
    let mut hints: BTreeMap<(String, String), String> = BTreeMap::new();
    {
        let mut q =
            conn.prepare("SELECT scope, key, hint FROM vault_entries WHERE namespace=?1")?;
        for r in q.query_map([NAMESPACE_ENV], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })? {
            let (vscope, key, hint) = r?;
            hints.insert((vscope, key), hint);
        }
    }

    let mut global_env = BTreeMap::new();
    let mut scoped_env: BTreeMap<String, BTreeMap<String, EnvEntryMeta>> = BTreeMap::new();
    let mut q = conn.prepare(
        "SELECT scope, target_id, key, kind, value FROM env_entries WHERE deleted=0 ORDER BY scope, target_id, key",
    )?;
    for r in q.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })? {
        let (scope, target_id, key, kind, val) = r?;
        let vscope = vault_scope(&scope, &target_id);
        let meta = match kind.as_str() {
            "plain" => EnvEntryMeta::Plain {
                value: val.unwrap_or_default(),
            },
            "secret" => match hints.get(&(vscope, key.clone())) {
                Some(hint) => EnvEntryMeta::Secret {
                    configured: true,
                    hint: hint.clone(),
                },
                None => EnvEntryMeta::Secret {
                    configured: false,
                    hint: String::new(),
                },
            },
            "unset" => EnvEntryMeta::Unset,
            _ => continue,
        };
        match scope.as_str() {
            SCOPE_GLOBAL => {
                global_env.insert(key, meta);
            }
            SCOPE_PROJECT => {
                scoped_env
                    .entry(Snapshot::project_env_key(&target_id))
                    .or_default()
                    .insert(key, meta);
            }
            SCOPE_AGENT => {
                scoped_env
                    .entry(Snapshot::agent_env_key("local", &target_id))
                    .or_default()
                    .insert(key, meta);
            }
            _ => {}
        }
    }
    Ok((global_env, scoped_env))
}

/// Apply writes uniformly across `SCOPE_GLOBAL`, `SCOPE_AGENT`, and `SCOPE_PROJECT`.
/// Global deletions preserve tombstones (`deleted=1`) for offline/LWW host replication.
pub fn apply_env_writes(
    conn: &Connection,
    home: &Path,
    scope: &str,
    target_id: &str,
    entries: &BTreeMap<String, EnvWriteEntry>,
    now: u64,
) -> Result<BTreeMap<String, EnvEntryMeta>> {
    ensure!(
        matches!(scope, SCOPE_GLOBAL | SCOPE_AGENT | SCOPE_PROJECT),
        "Unsupported env scope '{scope}'"
    );
    let tid = normalize_target_id(scope, target_id);
    if scope != SCOPE_GLOBAL {
        ensure!(
            !tid.trim().is_empty(),
            "target_id is required for {scope} env"
        );
    }
    let vscope = vault_scope(scope, tid);
    let secret_meta = vault::list_meta(conn, NAMESPACE_ENV, &vscope)?;
    let mut active_before = BTreeSet::new();
    {
        let mut q = conn
            .prepare("SELECT key FROM env_entries WHERE scope=?1 AND target_id=?2 AND deleted=0")?;
        for r in q.query_map(params![scope, tid], |row| row.get::<_, String>(0))? {
            active_before.insert(r?);
        }
    }

    let mut keep_secrets = BTreeSet::new();
    for (key, write) in entries {
        ensure!(
            is_valid_env_key(key),
            "Invalid environment variable '{key}'"
        );
        match write {
            EnvWriteEntry::Unset => {
                ensure!(
                    scope != SCOPE_GLOBAL,
                    "Unset is not supported at Global scope (remove '{key}' instead)"
                );
                vault::delete(conn, NAMESPACE_ENV, &vscope, key)?;
                conn.execute(
                    "INSERT INTO env_entries(scope, target_id, key, kind, value, updated_at, deleted) \
                     VALUES(?1, ?2, ?3, 'unset', NULL, ?4, 0) \
                     ON CONFLICT(scope, target_id, key) DO UPDATE SET \
                       kind='unset', value=NULL, updated_at=excluded.updated_at, deleted=0",
                    params![scope, tid, key, now as i64],
                )?;
            }
            EnvWriteEntry::Plain { value } => {
                ensure!(
                    !value.contains('\0'),
                    "Environment variable '{key}' cannot contain NUL bytes"
                );
                vault::delete(conn, NAMESPACE_ENV, &vscope, key)?;
                let unchanged = conn
                    .query_row(
                        "SELECT kind, value, deleted FROM env_entries WHERE scope=?1 AND target_id=?2 AND key=?3",
                        params![scope, tid, key],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?)),
                    )
                    .optional()?
                    .is_some_and(|(k, v, del)| del == 0 && k == "plain" && v.as_deref() == Some(value.as_str()));
                if !unchanged {
                    conn.execute(
                        "INSERT INTO env_entries(scope, target_id, key, kind, value, updated_at, deleted) \
                         VALUES(?1, ?2, ?3, 'plain', ?4, ?5, 0) \
                         ON CONFLICT(scope, target_id, key) DO UPDATE SET \
                           kind='plain', value=excluded.value, updated_at=excluded.updated_at, deleted=0",
                        params![scope, tid, key, value, now as i64],
                    )?;
                }
            }
            EnvWriteEntry::Secret {
                value,
                keep_existing,
            } => {
                if let Some(plaintext) = value {
                    ensure!(
                        !plaintext.contains('\0'),
                        "Secret '{key}' cannot contain NUL bytes"
                    );
                    vault::put(conn, home, NAMESPACE_ENV, &vscope, key, plaintext, now)?;
                    keep_secrets.insert(key.clone());
                    conn.execute(
                        "INSERT INTO env_entries(scope, target_id, key, kind, value, updated_at, deleted) \
                         VALUES(?1, ?2, ?3, 'secret', NULL, ?4, 0) \
                         ON CONFLICT(scope, target_id, key) DO UPDATE SET \
                           kind='secret', value=NULL, updated_at=excluded.updated_at, deleted=0",
                        params![scope, tid, key, now as i64],
                    )?;
                } else if *keep_existing && secret_meta.contains_key(key) {
                    keep_secrets.insert(key.clone());
                    conn.execute(
                        "UPDATE env_entries SET deleted=0 WHERE scope=?1 AND target_id=?2 AND key=?3",
                        params![scope, tid, key],
                    )?;
                } else {
                    bail!("Secret '{key}' requires a value");
                }
            }
        }
    }

    for old_key in active_before {
        if !entries.contains_key(&old_key) {
            vault::delete(conn, NAMESPACE_ENV, &vscope, &old_key)?;
            if scope == SCOPE_GLOBAL {
                conn.execute(
                    "UPDATE env_entries SET value=NULL, updated_at=?4, deleted=1 \
                     WHERE scope=?1 AND target_id=?2 AND key=?3",
                    params![scope, tid, old_key, now as i64],
                )?;
            } else {
                conn.execute(
                    "DELETE FROM env_entries WHERE scope=?1 AND target_id=?2 AND key=?3",
                    params![scope, tid, old_key],
                )?;
            }
        }
    }
    vault::retain(conn, NAMESPACE_ENV, &vscope, &keep_secrets)?;
    list_env_meta(conn, scope, tid)
}

pub fn list_global_env_for_sync(
    conn: &Connection,
    home: &Path,
) -> Result<Vec<GlobalEnvSyncRecord>> {
    let decrypted = list_decrypted_scope(conn, home, SCOPE_GLOBAL, "")?;
    let mut q = conn.prepare(
        "SELECT key, kind, value, updated_at, deleted FROM env_entries \
         WHERE scope=?1 AND target_id='' ORDER BY key",
    )?;
    let rows = q.query_map([SCOPE_GLOBAL], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (key, kind, plain_val, updated_at, deleted_i) = row?;
        let deleted = deleted_i != 0;
        let value = if deleted {
            None
        } else if kind == "secret" {
            decrypted.get(&key).map(|(v, _)| v.clone())
        } else {
            Some(Zeroizing::new(plain_val.unwrap_or_default()))
        };
        out.push(GlobalEnvSyncRecord {
            key,
            kind,
            value,
            updated_at: u64::try_from(updated_at).unwrap_or(0),
            deleted,
        });
    }
    Ok(out)
}

fn open_read_only_home_db(home: &Path) -> Result<Option<Connection>> {
    let db_path = home.join("canban.sqlite");
    if !db_path.exists() {
        return Ok(None);
    }
    let flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(&db_path, flags)
        .with_context(|| format!("Failed opening read-only {}", db_path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let has_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='env_entries'",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    Ok(has_table.then_some(conn))
}

pub fn list_global_env_for_sync_from_home(home: &Path) -> Result<Vec<GlobalEnvSyncRecord>> {
    let Some(conn) = open_read_only_home_db(home)? else {
        return Ok(Vec::new());
    };
    list_global_env_for_sync(&conn, home)
}

pub fn has_global_env_sync_rows_from_home(home: &Path) -> Result<bool> {
    let Some(conn) = open_read_only_home_db(home)? else {
        return Ok(false);
    };
    Ok(conn
        .query_row(
            "SELECT 1 FROM env_entries WHERE scope=?1 AND target_id='' LIMIT 1",
            [SCOPE_GLOBAL],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false))
}

pub fn apply_synced_global_env(
    conn: &Connection,
    home: &Path,
    records: &[GlobalEnvSyncRecord],
) -> Result<usize> {
    let mut applied = 0usize;
    for rec in records {
        ensure!(
            is_valid_env_key(&rec.key),
            "Invalid synced global environment key '{}'",
            rec.key
        );
        let local_ts: Option<u64> = conn
            .query_row(
                "SELECT updated_at FROM env_entries WHERE scope=?1 AND target_id='' AND key=?2",
                params![SCOPE_GLOBAL, rec.key],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .map(|v| u64::try_from(v).unwrap_or(0));
        if let Some(existing_ts) = local_ts
            && rec.updated_at < existing_ts
        {
            continue;
        }
        if rec.deleted {
            vault::delete(conn, NAMESPACE_ENV, SCOPE_GLOBAL, &rec.key)?;
            conn.execute(
                "INSERT INTO env_entries(scope, target_id, key, kind, value, updated_at, deleted) \
                 VALUES(?1, '', ?2, ?3, NULL, ?4, 1) \
                 ON CONFLICT(scope, target_id, key) DO UPDATE SET \
                   kind=excluded.kind, value=NULL, updated_at=excluded.updated_at, deleted=1",
                params![SCOPE_GLOBAL, rec.key, rec.kind, rec.updated_at as i64],
            )?;
            applied += 1;
            continue;
        }
        match rec.kind.as_str() {
            "plain" => {
                let val = rec.value.as_deref().map(String::as_str).unwrap_or("");
                ensure!(
                    !val.contains('\0'),
                    "Synced environment variable '{}' contains NUL",
                    rec.key
                );
                vault::delete(conn, NAMESPACE_ENV, SCOPE_GLOBAL, &rec.key)?;
                conn.execute(
                    "INSERT INTO env_entries(scope, target_id, key, kind, value, updated_at, deleted) \
                     VALUES(?1, '', ?2, 'plain', ?3, ?4, 0) \
                     ON CONFLICT(scope, target_id, key) DO UPDATE SET \
                       kind='plain', value=excluded.value, updated_at=excluded.updated_at, deleted=0",
                    params![SCOPE_GLOBAL, rec.key, val, rec.updated_at as i64],
                )?;
                applied += 1;
            }
            "secret" => {
                let Some(plain) = rec.value.as_deref() else {
                    bail!(
                        "Synced global secret '{}' is missing plaintext payload",
                        rec.key
                    );
                };
                ensure!(
                    !plain.contains('\0'),
                    "Synced secret '{}' contains NUL",
                    rec.key
                );
                vault::put(
                    conn,
                    home,
                    NAMESPACE_ENV,
                    SCOPE_GLOBAL,
                    &rec.key,
                    plain,
                    rec.updated_at,
                )?;
                conn.execute(
                    "INSERT INTO env_entries(scope, target_id, key, kind, value, updated_at, deleted) \
                     VALUES(?1, '', ?2, 'secret', NULL, ?3, 0) \
                     ON CONFLICT(scope, target_id, key) DO UPDATE SET \
                       kind='secret', value=NULL, updated_at=excluded.updated_at, deleted=0",
                    params![SCOPE_GLOBAL, rec.key, rec.updated_at as i64],
                )?;
                applied += 1;
            }
            other => bail!("Unsupported synced global env kind '{other}'"),
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    #[test]
    fn unified_env_writes_global_tombstones_lww_and_snapshot_index() {
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let store_a = Store::open(home_a.path()).unwrap();
        let store_b = Store::open(home_b.path()).unwrap();

        let meta_a = apply_env_writes(
            &store_a.conn,
            home_a.path(),
            SCOPE_GLOBAL,
            "",
            &BTreeMap::from([
                (
                    "LOG".into(),
                    EnvWriteEntry::Plain {
                        value: "info".into(),
                    },
                ),
                (
                    "API_KEY".into(),
                    EnvWriteEntry::Secret {
                        value: Some("sk-global-123456".into()),
                        keep_existing: false,
                    },
                ),
            ]),
            100,
        )
        .unwrap();
        assert!(meta_a.contains_key("LOG"));
        assert!(matches!(
            meta_a.get("API_KEY"),
            Some(EnvEntryMeta::Secret {
                configured: true,
                ..
            })
        ));

        apply_env_writes(
            &store_a.conn,
            home_a.path(),
            SCOPE_AGENT,
            "codex",
            &BTreeMap::from([("LOG".into(), EnvWriteEntry::Unset)]),
            100,
        )
        .unwrap();

        let (g, scoped) = list_all_snapshot_env(&store_a.conn).unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(
            scoped
                .get(&Snapshot::agent_env_key("local", "codex"))
                .and_then(|m| m.get("LOG")),
            Some(&EnvEntryMeta::Unset)
        );

        // Delete API_KEY globally -> tombstone replicates across hosts and respects LWW.
        apply_env_writes(
            &store_a.conn,
            home_a.path(),
            SCOPE_GLOBAL,
            "",
            &BTreeMap::from([(
                "LOG".into(),
                EnvWriteEntry::Plain {
                    value: "info".into(),
                },
            )]),
            200,
        )
        .unwrap();
        let recs = list_global_env_for_sync(&store_a.conn, home_a.path()).unwrap();
        let token = "tok-fedcba9876543210fedcba9876543210";
        let wire: Vec<_> = recs
            .iter()
            .map(|r| r.to_wire(Some(token)).unwrap())
            .collect();
        let decoded: Vec<_> = wire
            .into_iter()
            .map(|w| GlobalEnvSyncRecord::from_wire(w, Some(token)).unwrap())
            .collect();
        apply_synced_global_env(&store_b.conn, home_b.path(), &decoded).unwrap();
        assert_eq!(
            list_env_meta(&store_b.conn, SCOPE_GLOBAL, "")
                .unwrap()
                .len(),
            1
        );

        // Older stale record does not overwrite newer deletion tombstone.
        apply_synced_global_env(
            &store_b.conn,
            home_b.path(),
            &[GlobalEnvSyncRecord {
                key: "API_KEY".into(),
                kind: "secret".into(),
                value: Some(Zeroizing::new("stale-secret".into())),
                updated_at: 150,
                deleted: false,
            }],
        )
        .unwrap();
        assert!(
            !list_env_meta(&store_b.conn, SCOPE_GLOBAL, "")
                .unwrap()
                .contains_key("API_KEY")
        );
    }
}
