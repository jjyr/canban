//! Persistence for credentials, saved hosts, offline host caches,
//! and TCP transport secret sealing (`enc:v1:`).
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use canban_types::{Config, HostTransport, HostsFile, SavedHost, Snapshot};
use chacha20poly1305::{
    XChaCha20Poly1305,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
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
    lock: Mutex<()>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct CredentialsFile {
    #[serde(default)]
    tokens: BTreeMap<String, String>,
}

impl FileCredentialStore {
    pub fn open(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join("credentials.json"),
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
        canban_context::atomic(&self.path, &serde_json::to_vec_pretty(file)?)?;
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
}

impl CredentialHostStore {
    pub fn new(data_dir: PathBuf, creds: std::sync::Arc<dyn CredentialStore>) -> Self {
        Self { data_dir, creds }
    }
}

impl HostStore for CredentialHostStore {
    fn load_hosts(&self) -> Vec<SavedHost> {
        load_hosts(&self.data_dir, self.creds.as_ref())
    }

    fn save_hosts(&self, hosts: &[SavedHost]) -> Result<()> {
        save_hosts(&self.data_dir, hosts, self.creds.as_ref())
    }

    fn delete_credentials(&self, host_id: &str) {
        let _ = self.creds.delete_token(host_id);
    }
}

/// Desktop [`HostStore`] reading and writing an explicit `hosts.json` path (e.g.
/// `<home>/config/hosts.json`) with tokens stored inline.
pub struct InlineHostsFileStore {
    path: PathBuf,
}

impl InlineHostsFileStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
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
        canban_context::atomic(&self.path, &serde_json::to_vec_pretty(&file)?)
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
    let path = global_env_sync_state_path(data_dir, host_id);
    match fingerprint {
        Some(fp) => {
            if let Ok(bytes) = serde_json::to_vec(&serde_json::json!({ "fingerprint": fp })) {
                let _ = canban_context::atomic(&path, &bytes);
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
    canban_context::atomic(
        &path,
        &serde_json::to_vec_pretty(&HostsFile { hosts: stripped })?,
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
    if let Ok(bytes) = serde_json::to_vec(data) {
        let _ = canban_context::atomic(&cache_path(data_dir, host_id), &bytes);
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

fn seal_transport_secret(server_token: &str, key_name: &str, plaintext: &str) -> Result<String> {
    let key = transport_key(server_token)?;
    let cipher = XChaCha20Poly1305::new((&*key).into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let aad = format!("canban-vault-rpc-v1\0{key_name}").into_bytes();
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext.as_bytes(),
                aad: &aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("AEAD encryption failed"))?;
    let mut combined = Zeroizing::new(Vec::with_capacity(nonce.len() + ciphertext.len()));
    combined.extend_from_slice(&nonce);
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
                let sealed = seal_transport_secret(token, key, raw_val)
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
}
