//! Encrypted per-host Vault storage (`<CANBAN_HOME>/vault.key` + SQLite `vault_entries`)
//! and TCP transport envelope helpers (`enc:v1:`).
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use canban_types::{IdGen, secret_hint};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use zeroize::Zeroizing;

pub const TRANSPORT_SEAL_PREFIX: &str = "enc:v1:";
const KEY_FILENAME: &str = "vault.key";
const XNONCE_LEN: usize = 24;

pub fn key_path(home: &Path) -> PathBuf {
    home.join(KEY_FILENAME)
}

fn ensure_not_symlink(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) => {
            ensure!(!m.file_type().is_symlink(), "{label} must not be a symlink");
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn set_private_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

fn decode_hex_key(raw: &str) -> Result<Zeroizing<[u8; 32]>> {
    let trimmed = raw.trim();
    ensure!(trimmed.len() == 64, "Invalid vault.key length");
    let mut key = Zeroizing::new([0u8; 32]);
    for (i, slot) in key.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&trimmed[i * 2..i * 2 + 2], 16)
            .context("Invalid hex in vault.key")?;
    }
    Ok(key)
}

fn encode_hex_key(bytes: &[u8; 32]) -> Zeroizing<String> {
    use std::fmt::Write;
    let mut out = Zeroizing::new(String::with_capacity(65));
    for b in bytes {
        let _ = write!(&mut *out, "{b:02x}");
    }
    out.push('\n');
    out
}

fn generate_master_key(id_gen: Option<&dyn IdGen>) -> Zeroizing<[u8; 32]> {
    match id_gen {
        None => Zeroizing::new(XChaCha20Poly1305::generate_key(&mut OsRng).into()),
        Some(generator) => {
            let mut hasher = Sha256::new();
            hasher.update(b"canban-vault-master-key-v1\0");
            hasher.update(generator.next_uuid().as_bytes());
            hasher.update(generator.next_uuid().as_bytes());
            hasher.update(generator.next_uuid().as_bytes());
            let digest = hasher.finalize();
            let mut key = Zeroizing::new([0u8; 32]);
            key.copy_from_slice(&digest);
            key
        }
    }
}

fn generate_nonce(id_gen: Option<&dyn IdGen>) -> XNonce {
    match id_gen {
        None => XChaCha20Poly1305::generate_nonce(&mut OsRng),
        Some(generator) => {
            let mut hasher = Sha256::new();
            hasher.update(b"canban-vault-nonce-v1\0");
            hasher.update(generator.next_uuid().as_bytes());
            hasher.update(generator.next_uuid().as_bytes());
            let digest = hasher.finalize();
            *XNonce::from_slice(&digest[..XNONCE_LEN])
        }
    }
}

pub fn load_or_create_master_key(home: &Path) -> Result<Zeroizing<[u8; 32]>> {
    load_or_create_master_key_with_sync(home, true)
}

/// Loads `<home>/vault.key`, or atomically creates a new 256-bit master key.
/// When `fs_sync` is `false` (DST `FsDurability::NoSync`), skips `sync_all()` barriers
/// while preserving atomic `create_new` + `hard_link` race-free creation and `0o600` permissions.
pub fn load_or_create_master_key_with_sync(
    home: &Path,
    fs_sync: bool,
) -> Result<Zeroizing<[u8; 32]>> {
    load_or_create_master_key_with_sync_and_id_gen(home, fs_sync, None)
}

/// Loads `<home>/vault.key`, or atomically creates a new 256-bit master key using `OsRng`
/// (`id_gen == None`, production default) or a deterministic derivation from `id_gen` (DST).
pub fn load_or_create_master_key_with_sync_and_id_gen(
    home: &Path,
    fs_sync: bool,
    id_gen: Option<&dyn IdGen>,
) -> Result<Zeroizing<[u8; 32]>> {
    if !home.exists() {
        fs::create_dir_all(home)?;
    }
    ensure_not_symlink(home, "Vault home directory")?;
    set_private_mode(home, 0o700)?;

    let path = key_path(home);
    ensure_not_symlink(&path, "vault.key")?;
    if path.exists() {
        ensure!(
            fs::symlink_metadata(&path)?.is_file(),
            "vault.key must be a regular file"
        );
        set_private_mode(&path, 0o600)?;
        let raw = Zeroizing::new(fs::read_to_string(&path).context("Failed reading vault.key")?);
        return decode_hex_key(&raw);
    }

    static NEXT_TMP_SEQ: AtomicU64 = AtomicU64::new(1);
    let key = generate_master_key(id_gen);
    let encoded = encode_hex_key(&key);
    let tmp_seq = NEXT_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = home.join(format!(".vault-key-{}-{tmp_seq}.tmp", std::process::id()));
    let mut f = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
    set_private_mode(&tmp, 0o600)?;
    f.write_all(encoded.as_bytes())?;
    if fs_sync {
        f.sync_all()?;
    }
    drop(f);

    match fs::hard_link(&tmp, &path) {
        Ok(()) => {
            let _ = fs::remove_file(&tmp);
            if fs_sync && let Ok(dir) = fs::File::open(home) {
                let _ = dir.sync_all();
            }
            Ok(key)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&tmp);
            ensure_not_symlink(&path, "vault.key")?;
            let raw = Zeroizing::new(
                fs::read_to_string(&path).context("Failed reading raced vault.key")?,
            );
            decode_hex_key(&raw)
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e.into())
        }
    }
}

fn read_existing_master_key(home: &Path) -> Result<Option<Zeroizing<[u8; 32]>>> {
    let path = key_path(home);
    ensure_not_symlink(&path, "vault.key")?;
    if !path.exists() {
        return Ok(None);
    }
    ensure!(
        fs::symlink_metadata(&path)?.is_file(),
        "vault.key must be a regular file"
    );
    let raw = Zeroizing::new(fs::read_to_string(&path).context("Failed reading vault.key")?);
    Ok(Some(decode_hex_key(&raw)?))
}

fn entry_aad(namespace: &str, scope: &str, key: &str) -> Vec<u8> {
    format!("canban-vault-v1\0{namespace}\0{scope}\0{key}").into_bytes()
}

fn encrypt_with_key(
    key: &[u8; 32],
    aad: &[u8],
    plaintext: &str,
    id_gen: Option<&dyn IdGen>,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = generate_nonce(id_gen);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext.as_bytes(),
                aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("AEAD encryption failed"))?;
    Ok((nonce.to_vec(), ciphertext))
}

fn decrypt_with_key(
    key: &[u8; 32],
    aad: &[u8],
    nonce: &[u8],
    ciphertext: &[u8],
) -> Result<Zeroizing<String>> {
    ensure!(nonce.len() == XNONCE_LEN, "Invalid vault nonce length");
    let cipher = XChaCha20Poly1305::new(key.into());
    let plain_bytes = Zeroizing::new(
        cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("Vault decryption failed (wrong key or tampered AAD)"))?,
    );
    let text = String::from_utf8(plain_bytes.to_vec())
        .context("Decrypted vault value is not valid UTF-8")?;
    Ok(Zeroizing::new(text))
}

fn transport_key(server_token: &str) -> Result<Zeroizing<[u8; 32]>> {
    let tok = server_token.trim();
    ensure!(
        tok.len() >= 16,
        "Invalid server token for transport secret sealing"
    );
    let mut hasher = Sha256::new();
    hasher.update(b"canban-vault-transport-v1\0");
    hasher.update(tok.as_bytes());
    let digest = hasher.finalize();
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&digest);
    Ok(key)
}

fn transport_aad(key_name: &str) -> Vec<u8> {
    format!("canban-vault-rpc-v1\0{key_name}").into_bytes()
}

pub fn seal_transport(server_token: &str, key_name: &str, plaintext: &str) -> Result<String> {
    seal_transport_inner(server_token, key_name, plaintext, None)
}

/// Deterministic/injected-`IdGen` variant of [`seal_transport`].
pub fn seal_transport_with_id_gen(
    server_token: &str,
    key_name: &str,
    plaintext: &str,
    id_gen: &dyn IdGen,
) -> Result<String> {
    seal_transport_inner(server_token, key_name, plaintext, Some(id_gen))
}

fn seal_transport_inner(
    server_token: &str,
    key_name: &str,
    plaintext: &str,
    id_gen: Option<&dyn IdGen>,
) -> Result<String> {
    let key = transport_key(server_token)?;
    let (nonce, ciphertext) = encrypt_with_key(&key, &transport_aad(key_name), plaintext, id_gen)?;
    let mut combined = Zeroizing::new(Vec::with_capacity(nonce.len() + ciphertext.len()));
    combined.extend_from_slice(&nonce);
    combined.extend_from_slice(&ciphertext);
    Ok(format!(
        "{TRANSPORT_SEAL_PREFIX}{}",
        BASE64_STANDARD.encode(&*combined)
    ))
}

pub fn open_transport(
    server_token: &str,
    key_name: &str,
    sealed: &str,
) -> Result<Zeroizing<String>> {
    let encoded = sealed
        .strip_prefix(TRANSPORT_SEAL_PREFIX)
        .context("Missing transport secret seal prefix")?;
    let decoded = Zeroizing::new(
        BASE64_STANDARD
            .decode(encoded)
            .context("Invalid base64 in transport secret")?,
    );
    ensure!(
        decoded.len() > XNONCE_LEN,
        "Truncated transport secret envelope"
    );
    let (nonce, ciphertext) = decoded.split_at(XNONCE_LEN);
    let key = transport_key(server_token)?;
    decrypt_with_key(&key, &transport_aad(key_name), nonce, ciphertext)
}

pub fn put(
    conn: &Connection,
    home: &Path,
    namespace: &str,
    scope: &str,
    key: &str,
    plaintext: &str,
    updated_at: u64,
) -> Result<String> {
    put_with_sync(
        conn, home, namespace, scope, key, plaintext, updated_at, true,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn put_with_sync(
    conn: &Connection,
    home: &Path,
    namespace: &str,
    scope: &str,
    key: &str,
    plaintext: &str,
    updated_at: u64,
    fs_sync: bool,
) -> Result<String> {
    put_with_sync_and_id_gen(
        conn, home, namespace, scope, key, plaintext, updated_at, fs_sync, None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn put_with_sync_and_id_gen(
    conn: &Connection,
    home: &Path,
    namespace: &str,
    scope: &str,
    key: &str,
    plaintext: &str,
    updated_at: u64,
    fs_sync: bool,
    id_gen: Option<&dyn IdGen>,
) -> Result<String> {
    let master = load_or_create_master_key_with_sync_and_id_gen(home, fs_sync, id_gen)?;
    let aad = entry_aad(namespace, scope, key);
    let (nonce, ciphertext) = encrypt_with_key(&master, &aad, plaintext, id_gen)?;
    let hint = secret_hint(plaintext);
    conn.execute(
        "INSERT INTO vault_entries(namespace, scope, key, nonce, ciphertext, hint, updated_at) \
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(namespace, scope, key) DO UPDATE SET \
           nonce=excluded.nonce, ciphertext=excluded.ciphertext, hint=excluded.hint, updated_at=excluded.updated_at",
        params![namespace, scope, key, nonce, ciphertext, hint, updated_at as i64],
    )?;
    Ok(hint)
}

pub fn get(
    conn: &Connection,
    home: &Path,
    namespace: &str,
    scope: &str,
    key: &str,
) -> Result<Option<Zeroizing<String>>> {
    let row: Option<(Vec<u8>, Vec<u8>)> = conn
        .query_row(
            "SELECT nonce, ciphertext FROM vault_entries WHERE namespace=?1 AND scope=?2 AND key=?3",
            params![namespace, scope, key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((nonce, ciphertext)) = row else {
        return Ok(None);
    };
    let master = read_existing_master_key(home)?
        .context("vault.key is missing while vault_entries contains encrypted records")?;
    Ok(Some(decrypt_with_key(
        &master,
        &entry_aad(namespace, scope, key),
        &nonce,
        &ciphertext,
    )?))
}

pub fn list_meta(
    conn: &Connection,
    namespace: &str,
    scope: &str,
) -> Result<BTreeMap<String, (String, u64)>> {
    let mut q = conn.prepare(
        "SELECT key, hint, updated_at FROM vault_entries WHERE namespace=?1 AND scope=?2 ORDER BY key",
    )?;
    let rows = q.query_map(params![namespace, scope], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (k, hint, ts) = row?;
        out.insert(k, (hint, u64::try_from(ts).unwrap_or(0)));
    }
    Ok(out)
}

pub fn list_decrypted(
    conn: &Connection,
    home: &Path,
    namespace: &str,
    scope: &str,
) -> Result<BTreeMap<String, (Zeroizing<String>, u64)>> {
    let mut q = conn.prepare(
        "SELECT key, nonce, ciphertext, updated_at FROM vault_entries WHERE namespace=?1 AND scope=?2 ORDER BY key",
    )?;
    let rows = q
        .query_map(params![namespace, scope], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Ok(BTreeMap::new());
    }
    let master = read_existing_master_key(home)?
        .context("vault.key is missing while vault_entries contains encrypted records")?;
    let mut out = BTreeMap::new();
    for (k, nonce, ciphertext, ts) in rows {
        let plain = decrypt_with_key(
            &master,
            &entry_aad(namespace, scope, &k),
            &nonce,
            &ciphertext,
        )?;
        out.insert(k, (plain, u64::try_from(ts).unwrap_or(0)));
    }
    Ok(out)
}

pub fn delete(conn: &Connection, namespace: &str, scope: &str, key: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM vault_entries WHERE namespace=?1 AND scope=?2 AND key=?3",
        params![namespace, scope, key],
    )?;
    Ok(())
}

pub fn retain(
    conn: &Connection,
    namespace: &str,
    scope: &str,
    keep_keys: &BTreeSet<String>,
) -> Result<()> {
    let existing = list_meta(conn, namespace, scope)?;
    for key in existing.keys() {
        if !keep_keys.contains(key) {
            delete(conn, namespace, scope, key)?;
        }
    }
    Ok(())
}

pub fn delete_scope(conn: &Connection, namespace: &str, scope: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM vault_entries WHERE namespace=?1 AND scope=?2",
        params![namespace, scope],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    #[test]
    fn vault_round_trip_aad_binding_tamper_and_anti_symlink() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open(home.path()).unwrap();

        let hint = put(
            &store.conn,
            home.path(),
            "env",
            "project:p1",
            "OPENAI_API_KEY",
            "sk-live-secret-9876",
            42,
        )
        .unwrap();
        assert_eq!(hint, "••••9876");
        assert_eq!(
            get(
                &store.conn,
                home.path(),
                "env",
                "project:p1",
                "OPENAI_API_KEY"
            )
            .unwrap()
            .as_deref()
            .map(String::as_str),
            Some("sk-live-secret-9876")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(key_path(home.path()))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        // Cross-scope ciphertext copy fails AEAD AAD check.
        store
            .conn
            .execute(
                "INSERT INTO vault_entries(namespace, scope, key, nonce, ciphertext, hint, updated_at) \
                 SELECT namespace, 'project:p2', key, nonce, ciphertext, hint, updated_at FROM vault_entries",
                [],
            )
            .unwrap();
        assert!(
            get(
                &store.conn,
                home.path(),
                "env",
                "project:p2",
                "OPENAI_API_KEY"
            )
            .is_err()
        );

        // Symlink key path is refused.
        #[cfg(unix)]
        {
            let other = tempfile::tempdir().unwrap();
            let linked_home = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(other.path(), linked_home.path().join(KEY_FILENAME))
                .unwrap();
            assert!(load_or_create_master_key(linked_home.path()).is_err());
        }
    }

    #[test]
    fn transport_seal_and_open_round_trips_and_rejects_wrong_token_or_key() {
        let token = "tok-0123456789abcdef0123456789abcdef";
        let sealed = seal_transport(token, "MY_SECRET", "super-secret-value-123").unwrap();
        assert!(sealed.starts_with(TRANSPORT_SEAL_PREFIX));
        assert!(!sealed.contains("super-secret-value-123"));
        assert_eq!(
            open_transport(token, "MY_SECRET", &sealed)
                .unwrap()
                .as_str(),
            "super-secret-value-123"
        );
        assert!(open_transport("wrong-token-0123456789", "MY_SECRET", &sealed).is_err());
        assert!(open_transport(token, "OTHER_KEY", &sealed).is_err());
    }

    #[test]
    fn load_or_create_master_key_with_sync_false_persists_private_key_and_round_trips() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open(home.path()).unwrap();

        let key1 = load_or_create_master_key_with_sync(home.path(), false).unwrap();
        let key2 = load_or_create_master_key_with_sync(home.path(), false).unwrap();
        assert_eq!(*key1, *key2);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(key_path(home.path()))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let hint = put_with_sync(
            &store.conn,
            home.path(),
            "env",
            "global",
            "SIM_TOKEN",
            "sim-secret-1234",
            100,
            false,
        )
        .unwrap();
        assert_eq!(hint, "••••1234");
        assert_eq!(
            get(&store.conn, home.path(), "env", "global", "SIM_TOKEN")
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("sim-secret-1234")
        );
    }

    #[test]
    fn deterministic_id_gen_reproduces_vault_master_key_nonces_and_transport_seal() {
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let home_c = tempfile::tempdir().unwrap();
        let store_a = Store::open(home_a.path()).unwrap();
        let store_b = Store::open(home_b.path()).unwrap();
        let store_c = Store::open(home_c.path()).unwrap();

        let g_a = canban_types::DeterministicIdGen::new(2026);
        let g_b = canban_types::DeterministicIdGen::new(2026);
        let g_c = canban_types::DeterministicIdGen::new(2027);

        let key_a =
            load_or_create_master_key_with_sync_and_id_gen(home_a.path(), false, Some(&g_a))
                .unwrap();
        let key_b =
            load_or_create_master_key_with_sync_and_id_gen(home_b.path(), false, Some(&g_b))
                .unwrap();
        let key_c =
            load_or_create_master_key_with_sync_and_id_gen(home_c.path(), false, Some(&g_c))
                .unwrap();
        assert_eq!(
            *key_a, *key_b,
            "same IdGen seed must derive identical master key"
        );
        assert_ne!(
            *key_a, *key_c,
            "different IdGen seeds must derive distinct master keys"
        );
        assert_eq!(
            fs::read(key_path(home_a.path())).unwrap(),
            fs::read(key_path(home_b.path())).unwrap(),
            "vault.key files must be bit-for-bit identical"
        );

        put_with_sync_and_id_gen(
            &store_a.conn,
            home_a.path(),
            "env",
            "global",
            "SECRET_1",
            "alpha-value",
            100,
            false,
            Some(&g_a),
        )
        .unwrap();
        put_with_sync_and_id_gen(
            &store_a.conn,
            home_a.path(),
            "env",
            "global",
            "SECRET_2",
            "alpha-value",
            100,
            false,
            Some(&g_a),
        )
        .unwrap();

        put_with_sync_and_id_gen(
            &store_b.conn,
            home_b.path(),
            "env",
            "global",
            "SECRET_1",
            "alpha-value",
            100,
            false,
            Some(&g_b),
        )
        .unwrap();
        put_with_sync_and_id_gen(
            &store_b.conn,
            home_b.path(),
            "env",
            "global",
            "SECRET_2",
            "alpha-value",
            100,
            false,
            Some(&g_b),
        )
        .unwrap();

        put_with_sync_and_id_gen(
            &store_c.conn,
            home_c.path(),
            "env",
            "global",
            "SECRET_1",
            "alpha-value",
            100,
            false,
            Some(&g_c),
        )
        .unwrap();

        let fetch_raw = |conn: &Connection, key: &str| -> (Vec<u8>, Vec<u8>) {
            conn.query_row(
                "SELECT nonce, ciphertext FROM vault_entries WHERE namespace='env' AND scope='global' AND key=?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };

        let a1 = fetch_raw(&store_a.conn, "SECRET_1");
        let a2 = fetch_raw(&store_a.conn, "SECRET_2");
        let b1 = fetch_raw(&store_b.conn, "SECRET_1");
        let b2 = fetch_raw(&store_b.conn, "SECRET_2");
        let c1 = fetch_raw(&store_c.conn, "SECRET_1");

        assert_eq!(a1, b1, "same seed must produce identical nonce+ciphertext");
        assert_eq!(a2, b2, "same seed must produce identical second entry");
        assert_ne!(
            a1.0, a2.0,
            "consecutive writes must never reuse the same XChaCha20 nonce"
        );
        assert_ne!(a1, c1, "different seed must produce different ciphertext");
        assert_eq!(
            get(&store_a.conn, home_a.path(), "env", "global", "SECRET_1")
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("alpha-value")
        );
        assert_eq!(
            get(&store_b.conn, home_b.path(), "env", "global", "SECRET_2")
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("alpha-value")
        );

        let token = "tok-0123456789abcdef0123456789abcdef";
        let seal_a1 = seal_transport_with_id_gen(token, "MY_SEC", "wire-secret", &g_a).unwrap();
        let seal_a2 = seal_transport_with_id_gen(token, "MY_SEC", "wire-secret", &g_a).unwrap();
        let seal_b1 = seal_transport_with_id_gen(token, "MY_SEC", "wire-secret", &g_b).unwrap();
        assert_eq!(seal_a1, seal_b1);
        assert_ne!(seal_a1, seal_a2);
        assert_eq!(
            open_transport(token, "MY_SEC", &seal_a1).unwrap().as_str(),
            "wire-secret"
        );
        assert_eq!(
            open_transport(token, "MY_SEC", &seal_a2).unwrap().as_str(),
            "wire-secret"
        );
    }
}
