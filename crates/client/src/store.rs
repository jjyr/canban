//! Persistence for credentials, saved hosts, offline host caches,
//! and TCP transport secret sealing (`enc:v1:`).
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use canban_types::{Config, HostTransport, HostsFile, IdGen, SavedHost, Snapshot, SystemIdGen};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};
use zeroize::Zeroizing;

pub const TRANSPORT_SEAL_PREFIX: &str = "enc:v1:";

/// Secure storage abstraction for per-host bearer tokens (`dev_...`).
/// Desktop/CLI or file-backed runners can use [`FileCredentialStore`], while iOS/Android apps can
/// swap in Keychain/Keystore implementations later without changing `RemoteClient`.
pub trait CredentialStore: Send + Sync + 'static {
    fn get_token(&self, host_id: &str) -> Result<Option<String>>;
    fn set_token(&self, host_id: &str, token: &str) -> Result<()>;
    fn delete_token(&self, host_id: &str) -> Result<()>;
}

/// Private file-backed credential store (`<data_dir>/credentials.json`, mode `0o600` on Unix).
pub struct FileCredentialStore {
    path: PathBuf,
    durability: canban_context::FsDurability,
    lock: Mutex<()>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct CredentialsFile {
    #[serde(default)]
    tokens: BTreeMap<String, String>,
}

impl FileCredentialStore {
    pub fn open(data_dir: &Path) -> Self {
        Self::open_with_durability(data_dir, canban_context::FsDurability::Full)
    }

    pub fn open_with_durability(data_dir: &Path, durability: canban_context::FsDurability) -> Self {
        Self {
            path: data_dir.join("credentials.json"),
            durability,
            lock: Mutex::new(()),
        }
    }

    fn load_unlocked(&self) -> CredentialsFile {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_unlocked(&self, file: &CredentialsFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
            set_private_mode(parent, 0o700);
        }
        canban_context::atomic_with_durability(
            &self.path,
            &serde_json::to_vec_pretty(file)?,
            self.durability,
        )?;
        set_private_mode(&self.path, 0o600);
        Ok(())
    }
}

impl CredentialStore for FileCredentialStore {
    fn get_token(&self, host_id: &str) -> Result<Option<String>> {
        let _guard = self.lock.lock().unwrap();
        Ok(self
            .load_unlocked()
            .tokens
            .get(host_id)
            .filter(|s| !s.trim().is_empty())
            .cloned())
    }

    fn set_token(&self, host_id: &str, token: &str) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        let mut file = self.load_unlocked();
        file.tokens.insert(host_id.to_owned(), token.to_owned());
        self.save_unlocked(&file)
    }

    fn delete_token(&self, host_id: &str) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        let mut file = self.load_unlocked();
        if file.tokens.remove(host_id).is_some() {
            self.save_unlocked(&file)?;
        }
        Ok(())
    }
}

/// In-memory credential store for ephemeral clients or unit tests.
#[derive(Default)]
pub struct MemoryCredentialStore {
    tokens: Mutex<BTreeMap<String, String>>,
}

impl MemoryCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn get_token(&self, host_id: &str) -> Result<Option<String>> {
        Ok(self.tokens.lock().unwrap().get(host_id).cloned())
    }

    fn set_token(&self, host_id: &str, token: &str) -> Result<()> {
        self.tokens
            .lock()
            .unwrap()
            .insert(host_id.to_owned(), token.to_owned());
        Ok(())
    }

    fn delete_token(&self, host_id: &str) -> Result<()> {
        self.tokens.lock().unwrap().remove(host_id);
        Ok(())
    }
}

fn set_private_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedHostData {
    pub saved_at: u64,
    pub snapshot: Snapshot,
    pub config: Config,
}

/// Persistence adapter for saved host definitions (`hosts.json` + optional credential vault).
///
/// Mobile clients use [`CredentialHostStore`] (`<data_dir>/hosts.json` with inline TCP tokens
/// stripped into a [`CredentialStore`]), while desktop clients use [`InlineHostsFileStore`]
/// (`<home>/config/hosts.json`) for backward compatibility with existing desktop configs.
pub trait HostStore: Send + Sync + 'static {
    fn load_hosts(&self) -> Vec<SavedHost>;
    fn save_hosts(&self, hosts: &[SavedHost]) -> Result<()>;
    fn delete_credentials(&self, _host_id: &str) {}
}

/// Default mobile [`HostStore`] backed by `<data_dir>/hosts.json` and a [`CredentialStore`].
pub struct CredentialHostStore {
    data_dir: PathBuf,
    creds: std::sync::Arc<dyn CredentialStore>,
    durability: canban_context::FsDurability,
}

impl CredentialHostStore {
    pub fn new(data_dir: PathBuf, creds: std::sync::Arc<dyn CredentialStore>) -> Self {
        Self::new_with_durability(data_dir, creds, canban_context::FsDurability::Full)
    }

    pub fn new_with_durability(
        data_dir: PathBuf,
        creds: std::sync::Arc<dyn CredentialStore>,
        durability: canban_context::FsDurability,
    ) -> Self {
        Self {
            data_dir,
            creds,
            durability,
        }
    }
}

impl HostStore for CredentialHostStore {
    fn load_hosts(&self) -> Vec<SavedHost> {
        load_hosts(&self.data_dir, self.creds.as_ref())
    }

    fn save_hosts(&self, hosts: &[SavedHost]) -> Result<()> {
        save_hosts_with_durability(&self.data_dir, hosts, self.creds.as_ref(), self.durability)
    }

    fn delete_credentials(&self, host_id: &str) {
        let _ = self.creds.delete_token(host_id);
    }
}

/// Desktop [`HostStore`] reading and writing an explicit `hosts.json` path (e.g.
/// `<home>/config/hosts.json`) with tokens stored inline.
pub struct InlineHostsFileStore {
    path: PathBuf,
    durability: canban_context::FsDurability,
}

impl InlineHostsFileStore {
    pub fn new(path: PathBuf) -> Self {
        Self::new_with_durability(path, canban_context::FsDurability::Full)
    }

    pub fn new_with_durability(path: PathBuf, durability: canban_context::FsDurability) -> Self {
        Self { path, durability }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl HostStore for InlineHostsFileStore {
    fn load_hosts(&self) -> Vec<SavedHost> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice::<HostsFile>(&b).ok())
            .map(|f| f.hosts)
            .unwrap_or_default()
    }

    fn save_hosts(&self, hosts: &[SavedHost]) -> Result<()> {
        let file = HostsFile {
            hosts: hosts.to_vec(),
        };
        canban_context::atomic_with_durability(
            &self.path,
            &serde_json::to_vec_pretty(&file)?,
            self.durability,
        )
    }
}

pub fn hosts_path(data_dir: &Path) -> PathBuf {
    data_dir.join("hosts.json")
}

pub fn global_env_sync_state_path(data_dir: &Path, host_id: &str) -> PathBuf {
    data_dir
        .join("cache/hosts")
        .join(host_id)
        .join("global-env-sync.json")
}

pub fn load_global_env_sync_fingerprint(data_dir: &Path, host_id: &str) -> Option<String> {
    std::fs::read(global_env_sync_state_path(data_dir, host_id))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| {
            v.get("fingerprint")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

pub fn save_global_env_sync_fingerprint(data_dir: &Path, host_id: &str, fingerprint: Option<&str>) {
    save_global_env_sync_fingerprint_with_durability(
        data_dir,
        host_id,
        fingerprint,
        canban_context::FsDurability::Full,
    );
}

pub fn save_global_env_sync_fingerprint_with_durability(
    data_dir: &Path,
    host_id: &str,
    fingerprint: Option<&str>,
    durability: canban_context::FsDurability,
) {
    let path = global_env_sync_state_path(data_dir, host_id);
    match fingerprint {
        Some(fp) => {
            if let Ok(bytes) = serde_json::to_vec(&serde_json::json!({ "fingerprint": fp })) {
                let _ = canban_context::atomic_with_durability(&path, &bytes, durability);
            }
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn cache_path(data_dir: &Path, host_id: &str) -> PathBuf {
    data_dir.join("cache/hosts").join(format!("{host_id}.json"))
}

pub fn outbox_path(data_dir: &Path, host_id: &str) -> PathBuf {
    data_dir
        .join("cache/hosts")
        .join(format!("{host_id}.outbox.json"))
}

pub fn reads_dir(data_dir: &Path, host_id: &str) -> PathBuf {
    data_dir.join("cache/hosts").join(host_id).join("reads")
}

pub fn host_cache_dir(data_dir: &Path, host_id: &str) -> PathBuf {
    data_dir.join("cache/hosts").join(host_id)
}

pub fn load_hosts(data_dir: &Path, creds: &dyn CredentialStore) -> Vec<SavedHost> {
    let mut hosts = std::fs::read(hosts_path(data_dir))
        .ok()
        .and_then(|b| serde_json::from_slice::<HostsFile>(&b).ok())
        .map(|f| f.hosts)
        .unwrap_or_default();
    for host in &mut hosts {
        if let HostTransport::Tcp { token, .. } = &mut host.transport
            && let Ok(Some(stored)) = creds.get_token(&host.id)
            && !stored.trim().is_empty()
        {
            *token = stored;
        }
    }
    hosts
}

/// Persists `hosts` to `<data_dir>/hosts.json` with inline TCP tokens stripped into `creds`.
pub fn save_hosts(data_dir: &Path, hosts: &[SavedHost], creds: &dyn CredentialStore) -> Result<()> {
    save_hosts_with_durability(data_dir, hosts, creds, canban_context::FsDurability::Full)
}

/// Persists `hosts` to `<data_dir>/hosts.json` using the specified [`canban_context::FsDurability`].
pub fn save_hosts_with_durability(
    data_dir: &Path,
    hosts: &[SavedHost],
    creds: &dyn CredentialStore,
    durability: canban_context::FsDurability,
) -> Result<()> {
    let mut stripped = Vec::with_capacity(hosts.len());
    for host in hosts {
        let mut copy = host.clone();
        if let HostTransport::Tcp { token, .. } = &mut copy.transport {
            let trimmed = token.trim();
            if !trimmed.is_empty() {
                creds.set_token(&copy.id, trimmed)?;
            }
            *token = String::new();
        }
        stripped.push(copy);
    }
    if let Err(e) = std::fs::create_dir_all(data_dir) {
        return Err(e.into());
    }
    set_private_mode(data_dir, 0o700);
    let path = hosts_path(data_dir);
    canban_context::atomic_with_durability(
        &path,
        &serde_json::to_vec_pretty(&HostsFile { hosts: stripped })?,
        durability,
    )?;
    set_private_mode(&path, 0o600);
    Ok(())
}

pub fn load_cache(data_dir: &Path, host_id: &str) -> Option<CachedHostData> {
    std::fs::read(cache_path(data_dir, host_id))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

pub fn save_cache(data_dir: &Path, host_id: &str, data: &CachedHostData) {
    save_cache_with_durability(data_dir, host_id, data, canban_context::FsDurability::Full);
}

pub fn save_cache_with_durability(
    data_dir: &Path,
    host_id: &str,
    data: &CachedHostData,
    durability: canban_context::FsDurability,
) {
    if let Ok(bytes) = serde_json::to_vec(data) {
        let _ = canban_context::atomic_with_durability(
            &cache_path(data_dir, host_id),
            &bytes,
            durability,
        );
    }
}

pub fn remove_host_files(data_dir: &Path, host_id: &str, creds: &dyn CredentialStore) {
    let _ = creds.delete_token(host_id);
    let _ = std::fs::remove_file(cache_path(data_dir, host_id));
    let _ = std::fs::remove_file(outbox_path(data_dir, host_id));
    let _ = std::fs::remove_dir_all(host_cache_dir(data_dir, host_id));
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

fn seal_transport_secret_with_id_gen(
    server_token: &str,
    key_name: &str,
    plaintext: &str,
    id_gen: &dyn IdGen,
) -> Result<String> {
    let key = transport_key(server_token)?;
    let cipher = XChaCha20Poly1305::new((&*key).into());
    // Derive a 192-bit XChaCha20 nonce from two IdGen UUIDs (244 bits of OS CSPRNG entropy
    // under SystemIdGen, and fully seed-deterministic under DeterministicIdGen).
    let mut nonce_hasher = Sha256::new();
    nonce_hasher.update(b"canban-vault-nonce-v1\0");
    nonce_hasher.update(id_gen.next_uuid().as_bytes());
    nonce_hasher.update(id_gen.next_uuid().as_bytes());
    let nonce_digest = nonce_hasher.finalize();
    let nonce = XNonce::from_slice(&nonce_digest[..24]);
    let aad = format!("canban-vault-rpc-v1\0{key_name}").into_bytes();
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext.as_bytes(),
                aad: &aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("AEAD encryption failed"))?;
    let mut combined = Zeroizing::new(Vec::with_capacity(nonce.len() + ciphertext.len()));
    combined.extend_from_slice(nonce);
    combined.extend_from_slice(&ciphertext);
    Ok(format!(
        "{TRANSPORT_SEAL_PREFIX}{}",
        BASE64_STANDARD.encode(&*combined)
    ))
}

/// Seals `secret` entries in outgoing `env.save` TCP payloads using `enc:v1:` XChaCha20-Poly1305,
/// wire-compatible with `canban_storage::vault::open_transport`.
pub fn seal_params_for_tcp(
    method: &str,
    params: &mut Value,
    tcp_token: Option<&str>,
) -> Result<()> {
    seal_params_for_tcp_with_id_gen(method, params, tcp_token, &SystemIdGen)
}

/// Deterministic/injected-`IdGen` variant of [`seal_params_for_tcp`].
pub fn seal_params_for_tcp_with_id_gen(
    method: &str,
    params: &mut Value,
    tcp_token: Option<&str>,
    id_gen: &dyn IdGen,
) -> Result<()> {
    let Some(token) = tcp_token else {
        return Ok(());
    };
    if method == "env.save"
        && let Some(Value::Object(obj)) = params.get_mut("entries")
    {
        for (key, entry) in obj.iter_mut() {
            if entry.get("kind").and_then(Value::as_str) == Some("secret")
                && let Some(raw_val) = entry.get("value").and_then(Value::as_str)
                && !raw_val.starts_with(TRANSPORT_SEAL_PREFIX)
            {
                let sealed = seal_transport_secret_with_id_gen(token, key, raw_val, id_gen)
                    .context("Failed to seal secret entry for TCP transport")?;
                entry["value"] = Value::String(sealed);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn file_credential_store_and_hosts_strip_inline_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let creds = FileCredentialStore::open(dir.path());
        let host = SavedHost {
            id: "host-1".into(),
            name: "Workstation".into(),
            transport: HostTransport::Tcp {
                addr: "100.84.12.5:7680".into(),
                token: "dev_secret_token_0123456789abcdef".into(),
            },
            ssh_target: None,
            server_id: Some("srv-1".into()),
            enabled: true,
            added_at: 42,
        };
        save_hosts(dir.path(), &[host], &creds).unwrap();

        let raw_hosts = std::fs::read_to_string(hosts_path(dir.path())).unwrap();
        assert!(
            !raw_hosts.contains("dev_secret_token"),
            "hosts.json must not store plaintext TCP tokens: {raw_hosts}"
        );
        assert_eq!(
            creds.get_token("host-1").unwrap().as_deref(),
            Some("dev_secret_token_0123456789abcdef")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("credentials.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        let loaded = load_hosts(dir.path(), &creds);
        assert_eq!(loaded.len(), 1);
        match &loaded[0].transport {
            HostTransport::Tcp { token, .. } => {
                assert_eq!(token, "dev_secret_token_0123456789abcdef");
            }
            other => panic!("unexpected transport: {other:?}"),
        }

        creds.delete_token("host-1").unwrap();
        assert_eq!(creds.get_token("host-1").unwrap(), None);
    }

    #[test]
    fn seal_params_for_tcp_is_wire_compatible_with_server_vault() {
        let token = "dev_0123456789abcdef0123456789abcdef";
        let mut params = json!({
            "scope": "project",
            "entries": {
                "API_KEY": { "kind": "secret", "value": "super-secret-value" },
                "PUBLIC_URL": { "kind": "plain", "value": "https://example.com" },
            }
        });
        seal_params_for_tcp("env.save", &mut params, Some(token)).unwrap();
        let sealed = params["entries"]["API_KEY"]["value"].as_str().unwrap();
        assert!(sealed.starts_with(TRANSPORT_SEAL_PREFIX));
        assert_eq!(
            params["entries"]["PUBLIC_URL"]["value"].as_str().unwrap(),
            "https://example.com"
        );
        let opened = canban_storage::vault::open_transport(token, "API_KEY", sealed).unwrap();
        assert_eq!(opened.as_str(), "super-secret-value");
    }

    #[test]
    fn seal_params_for_tcp_with_id_gen_is_deterministic_and_unique_per_call() {
        let token = "dev_0123456789abcdef0123456789abcdef";
        let template = json!({
            "scope": "project",
            "entries": {
                "API_KEY": { "kind": "secret", "value": "deterministic-secret" },
            }
        });
        let g1 = canban_types::DeterministicIdGen::new(42);
        let g2 = canban_types::DeterministicIdGen::new(42);

        let mut p1a = template.clone();
        let mut p1b = template.clone();
        let mut p2a = template;
        seal_params_for_tcp_with_id_gen("env.save", &mut p1a, Some(token), &g1).unwrap();
        seal_params_for_tcp_with_id_gen("env.save", &mut p1b, Some(token), &g1).unwrap();
        seal_params_for_tcp_with_id_gen("env.save", &mut p2a, Some(token), &g2).unwrap();

        let s1a = p1a["entries"]["API_KEY"]["value"].as_str().unwrap();
        let s1b = p1b["entries"]["API_KEY"]["value"].as_str().unwrap();
        let s2a = p2a["entries"]["API_KEY"]["value"].as_str().unwrap();
        assert_eq!(s1a, s2a, "same IdGen seed must seal identically");
        assert_ne!(s1a, s1b, "consecutive calls must advance nonce");
        assert_eq!(
            canban_storage::vault::open_transport(token, "API_KEY", s1a)
                .unwrap()
                .as_str(),
            "deterministic-secret"
        );
        assert_eq!(
            canban_storage::vault::open_transport(token, "API_KEY", s1b)
                .unwrap()
                .as_str(),
            "deterministic-secret"
        );
    }

    #[test]
    fn corrupt_and_truncated_json_files_recover_gracefully_across_client_stores_and_outbox() {
        use canban_sync::outbox::{Mutation, MutationKind, MutationState, Outbox, UpdateBase};

        let corrupt_payloads: &[(&str, &[u8])] = &[
            ("truncated_object", br#"{"hosts":[{"id":"h1","name":"Mac"#),
            ("truncated_outbox", br#"{"ops":[{"id":"m1","host_id":"h1""#),
            ("binary_nul_garbage", b"\x00\xff\xfe\xfdnot-valid-json"),
            ("empty_file", b""),
        ];

        for (label, bytes) in corrupt_payloads {
            let dir = tempfile::tempdir().unwrap();
            let data_dir = dir.path();
            let host_id = "remote-crash";

            // 1. Corrupt CredentialHostStore (`hosts.json`) + `FileCredentialStore` (`credentials.json`).
            std::fs::write(hosts_path(data_dir), bytes).unwrap();
            std::fs::write(data_dir.join("credentials.json"), bytes).unwrap();
            let creds = FileCredentialStore::open_with_durability(
                data_dir,
                canban_context::FsDurability::NoSync,
            );
            assert!(
                load_hosts(data_dir, &creds).is_empty(),
                "{label}: corrupt hosts.json must fall back to empty list"
            );
            assert_eq!(
                creds.get_token(host_id).unwrap(),
                None,
                "{label}: corrupt credentials.json must fall back to None"
            );

            // Recovery: saving fresh host + credentials overwrites corrupt files cleanly.
            let fresh_host = SavedHost {
                id: host_id.into(),
                name: "Recovered Host".into(),
                transport: HostTransport::Tcp {
                    addr: "100.64.0.9:7680".into(),
                    token: "dev_recovered_token_0123456789abcdef".into(),
                },
                ssh_target: None,
                server_id: Some("srv-recovered".into()),
                enabled: true,
                added_at: 77,
            };
            save_hosts_with_durability(
                data_dir,
                std::slice::from_ref(&fresh_host),
                &creds,
                canban_context::FsDurability::NoSync,
            )
            .unwrap();
            let reloaded_hosts = load_hosts(data_dir, &creds);
            assert_eq!(reloaded_hosts.len(), 1, "{label}: recovered host reload");
            assert_eq!(reloaded_hosts[0].name, "Recovered Host");
            assert_eq!(
                creds.get_token(host_id).unwrap().as_deref(),
                Some("dev_recovered_token_0123456789abcdef")
            );

            // 2. Corrupt InlineHostsFileStore (desktop `config/hosts.json`).
            let inline_path = data_dir.join("config/hosts.json");
            std::fs::create_dir_all(inline_path.parent().unwrap()).unwrap();
            std::fs::write(&inline_path, bytes).unwrap();
            let inline_store = InlineHostsFileStore::new_with_durability(
                inline_path,
                canban_context::FsDurability::NoSync,
            );
            assert!(
                inline_store.load_hosts().is_empty(),
                "{label}: corrupt inline hosts.json must fall back to empty"
            );
            inline_store.save_hosts(&[fresh_host]).unwrap();
            assert_eq!(inline_store.load_hosts().len(), 1);

            // 3. Corrupt offline snapshot cache & global env sync fingerprint.
            let cpath = cache_path(data_dir, host_id);
            std::fs::create_dir_all(cpath.parent().unwrap()).unwrap();
            std::fs::write(&cpath, bytes).unwrap();
            assert!(
                load_cache(data_dir, host_id).is_none(),
                "{label}: corrupt cache JSON must return None"
            );
            let fp_path = global_env_sync_state_path(data_dir, host_id);
            std::fs::create_dir_all(fp_path.parent().unwrap()).unwrap();
            std::fs::write(&fp_path, bytes).unwrap();
            assert_eq!(
                load_global_env_sync_fingerprint(data_dir, host_id),
                None,
                "{label}: corrupt global-env-sync.json must return None"
            );
            save_global_env_sync_fingerprint_with_durability(
                data_dir,
                host_id,
                Some("fp-recovered"),
                canban_context::FsDurability::NoSync,
            );
            assert_eq!(
                load_global_env_sync_fingerprint(data_dir, host_id).as_deref(),
                Some("fp-recovered")
            );

            // 4. Corrupt persistent mutation outbox (`<host>.outbox.json`).
            let opath = outbox_path(data_dir, host_id);
            std::fs::create_dir_all(opath.parent().unwrap()).unwrap();
            std::fs::write(&opath, bytes).unwrap();
            let mut outbox =
                Outbox::load_with_durability(opath.clone(), canban_context::FsDurability::NoSync);
            assert!(
                outbox.is_empty(),
                "{label}: corrupt outbox.json must fall back to empty queue"
            );
            outbox
                .enqueue(Mutation {
                    id: "mut-recovered".into(),
                    host_id: host_id.into(),
                    card_id: "card-1".into(),
                    kind: MutationKind::AddComment,
                    params: json!({"card_id": "card-1", "body": "after crash", "request_key": "mut-recovered"}),
                    base: UpdateBase::default(),
                    sent_params: json!({"card_id": "card-1", "body": "after crash", "request_key": "mut-recovered"}),
                    retry_same_params: false,
                    state: MutationState::Queued,
                    created_at: 200,
                    attempts: 0,
                })
                .unwrap();
            let reloaded_outbox =
                Outbox::load_with_durability(opath, canban_context::FsDurability::NoSync);
            assert_eq!(reloaded_outbox.len(), 1, "{label}: outbox must recover");
            assert_eq!(reloaded_outbox.ops[0].id, "mut-recovered");
        }
    }
}
