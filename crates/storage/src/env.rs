//! Unified environment metadata storage (`env_entries`) backed by `crate::vault`
//! for Global, CLI Agent / `@Identity`, and Project scopes.
use crate::vault;
use anyhow::{Context, Result, bail, ensure};
use canban_types::{EnvEntryMeta, EnvWriteEntry, IdGen, Snapshot, is_valid_env_key};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
        self.to_wire_with_id_gen(tcp_token, None)
    }

    pub fn to_wire_with_id_gen(
        &self,
        tcp_token: Option<&str>,
        id_gen: Option<&dyn IdGen>,
    ) -> Result<GlobalEnvSyncWireRecord> {
        let value = match (&self.value, self.kind.as_str(), self.deleted) {
            (_, _, true) | (None, _, false) => None,
            (Some(v), "secret", false) => match tcp_token {
                Some(tok) => match id_gen {
                    Some(generator) => Some(vault::seal_transport_with_id_gen(
                        tok, &self.key, v, generator,
                    )?),
                    None => Some(vault::seal_transport(tok, &self.key, v)?),
                },
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

fn monotonic_updated_at(now: u64, prev_env_ts: Option<i64>, prev_vault_ts: Option<u64>) -> u64 {
    let prev_env = prev_env_ts.map(|v| u64::try_from(v).unwrap_or(0));
    let prev = match (prev_env, prev_vault_ts) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    match prev {
        Some(prev_ts) => now.max(prev_ts.saturating_add(1)),
        None => now,
    }
}

fn next_monotonic_env_updated_at(
    conn: &Connection,
    scope: &str,
    tid: &str,
    key: &str,
    prev_vault_ts: Option<u64>,
    now: u64,
) -> Result<u64> {
    let prev_env_ts: Option<i64> = conn
        .query_row(
            "SELECT updated_at FROM env_entries WHERE scope=?1 AND target_id=?2 AND key=?3",
            params![scope, tid, key],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    Ok(monotonic_updated_at(now, prev_env_ts, prev_vault_ts))
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
    apply_env_writes_with_options(conn, home, scope, target_id, entries, now, true, None)
}

/// Configurable durability and vault RNG variant of [`apply_env_writes`].
#[allow(clippy::too_many_arguments)]
pub fn apply_env_writes_with_options(
    conn: &Connection,
    home: &Path,
    scope: &str,
    target_id: &str,
    entries: &BTreeMap<String, EnvWriteEntry>,
    now: u64,
    fs_sync: bool,
    id_gen: Option<&dyn IdGen>,
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
        let prev_vault_ts = secret_meta.get(key).map(|(_, ts)| *ts);
        match write {
            EnvWriteEntry::Unset => {
                ensure!(
                    scope != SCOPE_GLOBAL,
                    "Unset is not supported at Global scope (remove '{key}' instead)"
                );
                vault::delete(conn, NAMESPACE_ENV, &vscope, key)?;
                let effective_ts =
                    next_monotonic_env_updated_at(conn, scope, tid, key, prev_vault_ts, now)?;
                conn.execute(
                    "INSERT INTO env_entries(scope, target_id, key, kind, value, updated_at, deleted) \
                     VALUES(?1, ?2, ?3, 'unset', NULL, ?4, 0) \
                     ON CONFLICT(scope, target_id, key) DO UPDATE SET \
                       kind='unset', value=NULL, updated_at=excluded.updated_at, deleted=0",
                    params![scope, tid, key, effective_ts as i64],
                )?;
            }
            EnvWriteEntry::Plain { value } => {
                ensure!(
                    !value.contains('\0'),
                    "Environment variable '{key}' cannot contain NUL bytes"
                );
                vault::delete(conn, NAMESPACE_ENV, &vscope, key)?;
                let existing = conn
                    .query_row(
                        "SELECT kind, value, updated_at, deleted FROM env_entries WHERE scope=?1 AND target_id=?2 AND key=?3",
                        params![scope, tid, key],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, Option<String>>(1)?,
                                r.get::<_, i64>(2)?,
                                r.get::<_, i64>(3)?,
                            ))
                        },
                    )
                    .optional()?;
                let unchanged = existing.as_ref().is_some_and(|(k, v, _, del)| {
                    *del == 0 && k == "plain" && v.as_deref() == Some(value.as_str())
                });
                if !unchanged {
                    let effective_ts =
                        monotonic_updated_at(now, existing.map(|(_, _, ts, _)| ts), prev_vault_ts);
                    conn.execute(
                        "INSERT INTO env_entries(scope, target_id, key, kind, value, updated_at, deleted) \
                         VALUES(?1, ?2, ?3, 'plain', ?4, ?5, 0) \
                         ON CONFLICT(scope, target_id, key) DO UPDATE SET \
                           kind='plain', value=excluded.value, updated_at=excluded.updated_at, deleted=0",
                        params![scope, tid, key, value, effective_ts as i64],
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
                    let effective_ts =
                        next_monotonic_env_updated_at(conn, scope, tid, key, prev_vault_ts, now)?;
                    vault::put_with_sync_and_id_gen(
                        conn,
                        home,
                        NAMESPACE_ENV,
                        &vscope,
                        key,
                        plaintext,
                        effective_ts,
                        fs_sync,
                        id_gen,
                    )?;
                    keep_secrets.insert(key.clone());
                    conn.execute(
                        "INSERT INTO env_entries(scope, target_id, key, kind, value, updated_at, deleted) \
                         VALUES(?1, ?2, ?3, 'secret', NULL, ?4, 0) \
                         ON CONFLICT(scope, target_id, key) DO UPDATE SET \
                           kind='secret', value=NULL, updated_at=excluded.updated_at, deleted=0",
                        params![scope, tid, key, effective_ts as i64],
                    )?;
                } else if *keep_existing && secret_meta.contains_key(key) {
                    keep_secrets.insert(key.clone());
                    let existing: Option<(String, i64, i64)> = conn
                        .query_row(
                            "SELECT kind, updated_at, deleted FROM env_entries WHERE scope=?1 AND target_id=?2 AND key=?3",
                            params![scope, tid, key],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                        )
                        .optional()?;
                    let already_active_secret = existing
                        .as_ref()
                        .is_some_and(|(k, _, del)| *del == 0 && k == "secret");
                    if !already_active_secret {
                        let effective_ts =
                            monotonic_updated_at(now, existing.map(|(_, ts, _)| ts), prev_vault_ts);
                        conn.execute(
                            "INSERT INTO env_entries(scope, target_id, key, kind, value, updated_at, deleted) \
                             VALUES(?1, ?2, ?3, 'secret', NULL, ?4, 0) \
                             ON CONFLICT(scope, target_id, key) DO UPDATE SET \
                               kind='secret', value=NULL, updated_at=excluded.updated_at, deleted=0",
                            params![scope, tid, key, effective_ts as i64],
                        )?;
                    }
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
                let effective_ts = next_monotonic_env_updated_at(
                    conn,
                    scope,
                    tid,
                    &old_key,
                    secret_meta.get(&old_key).map(|(_, ts)| *ts),
                    now,
                )?;
                conn.execute(
                    "UPDATE env_entries SET value=NULL, updated_at=?4, deleted=1 \
                     WHERE scope=?1 AND target_id=?2 AND key=?3",
                    params![scope, tid, old_key, effective_ts as i64],
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

fn update_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Compute a deterministic SHA-256 fingerprint of the local Global Environment & Secrets state
/// (active entries, deletion tombstones, and sealed vault nonces/ciphertexts) without decrypting secrets.
pub fn compute_global_env_fingerprint(conn: &Connection) -> Result<Option<String>> {
    let mut env_stmt = conn.prepare(
        "SELECT key, kind, value, updated_at, deleted \
         FROM env_entries WHERE scope=?1 AND target_id='' ORDER BY key ASC",
    )?;
    let mut rows = env_stmt.query([SCOPE_GLOBAL])?;
    let mut hasher = Sha256::new();
    let mut row_count = 0usize;
    while let Some(row) = rows.next()? {
        row_count += 1;
        let key: String = row.get(0)?;
        let kind: String = row.get(1)?;
        let value: Option<String> = row.get(2)?;
        let updated_at: i64 = row.get(3)?;
        let deleted: i64 = row.get(4)?;
        update_len_prefixed(&mut hasher, key.as_bytes());
        update_len_prefixed(&mut hasher, kind.as_bytes());
        match value {
            Some(v) => {
                hasher.update([1u8]);
                update_len_prefixed(&mut hasher, v.as_bytes());
            }
            None => hasher.update([0u8]),
        }
        hasher.update(updated_at.to_le_bytes());
        hasher.update(deleted.to_le_bytes());
    }
    if row_count == 0 {
        return Ok(None);
    }

    let global_vault_scope = vault_scope(SCOPE_GLOBAL, "");
    let mut vault_stmt = conn.prepare(
        "SELECT key, nonce, ciphertext, updated_at \
         FROM vault_entries WHERE namespace=?1 AND scope=?2 ORDER BY key ASC",
    )?;
    let mut vrows = vault_stmt.query(params![NAMESPACE_ENV, global_vault_scope])?;
    while let Some(row) = vrows.next()? {
        let key: String = row.get(0)?;
        let nonce: Vec<u8> = row.get(1)?;
        let ciphertext: Vec<u8> = row.get(2)?;
        let updated_at: i64 = row.get(3)?;
        hasher.update([2u8]);
        update_len_prefixed(&mut hasher, key.as_bytes());
        update_len_prefixed(&mut hasher, &nonce);
        update_len_prefixed(&mut hasher, &ciphertext);
        hasher.update(updated_at.to_le_bytes());
    }

    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{b:02x}");
    }
    Ok(Some(hex))
}

pub fn global_env_state_fingerprint_from_home(home: &Path) -> Result<Option<String>> {
    let Some(conn) = open_read_only_home_db(home)? else {
        return Ok(None);
    };
    compute_global_env_fingerprint(&conn)
}

pub fn list_global_env_for_sync_with_fingerprint_from_home(
    home: &Path,
) -> Result<(Vec<GlobalEnvSyncRecord>, Option<String>)> {
    let Some(conn) = open_read_only_home_db(home)? else {
        return Ok((Vec::new(), None));
    };
    // Read records and fingerprint inside one deferred read transaction so concurrent writes cannot
    // cause the outbox drain to attribute a newer fingerprint to an older record payload.
    let tx = conn.unchecked_transaction()?;
    let fingerprint = compute_global_env_fingerprint(&tx)?;
    let records = list_global_env_for_sync(&tx, home)?;
    tx.commit()?;
    Ok((records, fingerprint))
}

pub fn list_global_env_for_sync_from_home(home: &Path) -> Result<Vec<GlobalEnvSyncRecord>> {
    Ok(list_global_env_for_sync_with_fingerprint_from_home(home)?.0)
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
    apply_synced_global_env_with_options(conn, home, records, true, None)
}

/// Configurable durability and vault RNG variant of [`apply_synced_global_env`].
pub fn apply_synced_global_env_with_options(
    conn: &Connection,
    home: &Path,
    records: &[GlobalEnvSyncRecord],
    fs_sync: bool,
    id_gen: Option<&dyn IdGen>,
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
                vault::put_with_sync_and_id_gen(
                    conn,
                    home,
                    NAMESPACE_ENV,
                    SCOPE_GLOBAL,
                    &rec.key,
                    plain,
                    rec.updated_at,
                    fs_sync,
                    id_gen,
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

    #[test]
    fn global_env_fingerprint_tracks_plain_secret_rotation_and_tombstones() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open(home.path()).unwrap();
        assert_eq!(
            global_env_state_fingerprint_from_home(home.path()).unwrap(),
            None
        );

        apply_env_writes(
            &store.conn,
            home.path(),
            SCOPE_GLOBAL,
            "",
            &BTreeMap::from([
                (
                    "G_VAR".into(),
                    EnvWriteEntry::Plain {
                        value: "one".into(),
                    },
                ),
                (
                    "G_SEC".into(),
                    EnvWriteEntry::Secret {
                        value: Some("secret-v1".into()),
                        keep_existing: false,
                    },
                ),
            ]),
            100,
        )
        .unwrap();
        let fp1 = global_env_state_fingerprint_from_home(home.path())
            .unwrap()
            .expect("fingerprint after initial save");

        // Rotating a secret with the same timestamp still changes the sealed ciphertext/nonce.
        apply_env_writes(
            &store.conn,
            home.path(),
            SCOPE_GLOBAL,
            "",
            &BTreeMap::from([
                (
                    "G_VAR".into(),
                    EnvWriteEntry::Plain {
                        value: "one".into(),
                    },
                ),
                (
                    "G_SEC".into(),
                    EnvWriteEntry::Secret {
                        value: Some("secret-v2".into()),
                        keep_existing: false,
                    },
                ),
            ]),
            100,
        )
        .unwrap();
        let fp2 = global_env_state_fingerprint_from_home(home.path())
            .unwrap()
            .expect("fingerprint after secret rotation");
        assert_ne!(fp1, fp2);

        // Deleting all global entries leaves tombstones, producing a distinct non-None fingerprint.
        apply_env_writes(
            &store.conn,
            home.path(),
            SCOPE_GLOBAL,
            "",
            &BTreeMap::new(),
            200,
        )
        .unwrap();
        let (records, fp3) =
            list_global_env_for_sync_with_fingerprint_from_home(home.path()).unwrap();
        let fp3 = fp3.expect("tombstone rows still produce a fingerprint");
        assert_ne!(fp2, fp3);
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|r| r.deleted));
    }

    #[test]
    fn env_writes_and_tombstones_advance_monotonically_across_backward_clock_jump() {
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let store_a = Store::open(home_a.path()).unwrap();
        let store_b = Store::open(home_b.path()).unwrap();

        // 1. Initial write at t=10_000 and replicate to remote host B.
        apply_env_writes(
            &store_a.conn,
            home_a.path(),
            SCOPE_GLOBAL,
            "",
            &BTreeMap::from([
                (
                    "CHAOS_PLAIN".into(),
                    EnvWriteEntry::Plain { value: "v1".into() },
                ),
                (
                    "CHAOS_SECRET".into(),
                    EnvWriteEntry::Secret {
                        value: Some("sec-v1".into()),
                        keep_existing: false,
                    },
                ),
            ]),
            10_000,
        )
        .unwrap();
        let sync_1 = list_global_env_for_sync(&store_a.conn, home_a.path()).unwrap();
        assert_eq!(sync_1.len(), 2);
        assert!(sync_1.iter().all(|r| r.updated_at == 10_000 && !r.deleted));
        apply_synced_global_env(&store_b.conn, home_b.path(), &sync_1).unwrap();
        assert_eq!(
            list_env_meta(&store_b.conn, SCOPE_GLOBAL, "").unwrap(),
            list_env_meta(&store_a.conn, SCOPE_GLOBAL, "").unwrap()
        );

        // 2. Clock jumps backward to t=6_000: mutate plain value AND delete secret (tombstone).
        //    Both records must advance monotonically to >= 10_001 and overwrite remote host B.
        apply_env_writes(
            &store_a.conn,
            home_a.path(),
            SCOPE_GLOBAL,
            "",
            &BTreeMap::from([(
                "CHAOS_PLAIN".into(),
                EnvWriteEntry::Plain { value: "v2".into() },
            )]),
            6_000,
        )
        .unwrap();
        let sync_2 = list_global_env_for_sync(&store_a.conn, home_a.path()).unwrap();
        let plain_rec = sync_2.iter().find(|r| r.key == "CHAOS_PLAIN").unwrap();
        let secret_tomb = sync_2.iter().find(|r| r.key == "CHAOS_SECRET").unwrap();
        assert!(
            plain_rec.updated_at >= 10_001,
            "expected plain updated_at >= 10_001 after backward clock jump, got {}",
            plain_rec.updated_at
        );
        assert_eq!(plain_rec.value.as_deref().map(String::as_str), Some("v2"));
        assert!(secret_tomb.deleted);
        assert!(
            secret_tomb.updated_at >= 10_001,
            "expected tombstone updated_at >= 10_001 after backward clock jump, got {}",
            secret_tomb.updated_at
        );

        let applied = apply_synced_global_env(&store_b.conn, home_b.path(), &sync_2).unwrap();
        assert_eq!(applied, 2);
        let meta_b = list_env_meta(&store_b.conn, SCOPE_GLOBAL, "").unwrap();
        assert_eq!(
            meta_b,
            list_env_meta(&store_a.conn, SCOPE_GLOBAL, "").unwrap()
        );
        assert_eq!(
            meta_b.get("CHAOS_PLAIN"),
            Some(&EnvEntryMeta::Plain { value: "v2".into() })
        );
        assert!(!meta_b.contains_key("CHAOS_SECRET"));

        // 3. While clock is still behind at t=6_005, re-creating the tombstoned secret key must
        //    strictly advance past the tombstone timestamp and converge on remote host B.
        apply_env_writes(
            &store_a.conn,
            home_a.path(),
            SCOPE_GLOBAL,
            "",
            &BTreeMap::from([
                (
                    "CHAOS_PLAIN".into(),
                    EnvWriteEntry::Plain { value: "v2".into() },
                ),
                (
                    "CHAOS_SECRET".into(),
                    EnvWriteEntry::Secret {
                        value: Some("sec-v2".into()),
                        keep_existing: false,
                    },
                ),
            ]),
            6_005,
        )
        .unwrap();
        let sync_3 = list_global_env_for_sync(&store_a.conn, home_a.path()).unwrap();
        let revived_secret = sync_3.iter().find(|r| r.key == "CHAOS_SECRET").unwrap();
        assert!(!revived_secret.deleted);
        assert!(
            revived_secret.updated_at > secret_tomb.updated_at,
            "revived secret must outrank prior tombstone ({} vs {})",
            revived_secret.updated_at,
            secret_tomb.updated_at
        );
        apply_synced_global_env(&store_b.conn, home_b.path(), &sync_3).unwrap();
        assert_eq!(
            list_env_meta(&store_b.conn, SCOPE_GLOBAL, "").unwrap(),
            list_env_meta(&store_a.conn, SCOPE_GLOBAL, "").unwrap()
        );
        let decrypted_b =
            list_decrypted_scope(&store_b.conn, home_b.path(), SCOPE_GLOBAL, "").unwrap();
        assert_eq!(
            decrypted_b.get("CHAOS_SECRET").map(|(v, _)| v.as_str()),
            Some("sec-v2")
        );

        // 4. Scoped Unset entry also advances monotonically across backward clock jump.
        apply_env_writes(
            &store_a.conn,
            home_a.path(),
            SCOPE_AGENT,
            "codex",
            &BTreeMap::from([(
                "OVERRIDE_VAR".into(),
                EnvWriteEntry::Plain {
                    value: "enabled".into(),
                },
            )]),
            10_000,
        )
        .unwrap();
        apply_env_writes(
            &store_a.conn,
            home_a.path(),
            SCOPE_AGENT,
            "codex",
            &BTreeMap::from([("OVERRIDE_VAR".into(), EnvWriteEntry::Unset)]),
            6_000,
        )
        .unwrap();
        let unset_ts: i64 = store_a
            .conn
            .query_row(
                "SELECT updated_at FROM env_entries WHERE scope=?1 AND target_id=?2 AND key=?3",
                params![SCOPE_AGENT, "codex", "OVERRIDE_VAR"],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            unset_ts >= 10_001,
            "expected scoped Unset updated_at >= 10_001 after backward clock jump, got {unset_ts}"
        );
    }
}
