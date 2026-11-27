//! Persistence for the client's Noise X25519 identity keypair, saved hosts, and offline caches.
use anyhow::Result;
use canban_transport::{NoiseKeypair, NoiseRngFactory};
use canban_types::{Config, HostsFile, SavedHost, Snapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Secure storage abstraction for this client's single Noise X25519 static private key.
/// Desktop/CLI uses [`FileCredentialStore`], while iOS/Android apps can plug in Keychain/Keystore
/// implementations without changing `MultiHostClient`.
pub trait CredentialStore: Send + Sync + 'static {
    fn get_client_noise_key(&self) -> Result<Option<String>>;
    fn set_client_noise_key(&self, private_hex: &str) -> Result<()>;
}

/// Loads this client's persistent Noise X25519 keypair from `creds`, or generates and stores one.
pub fn load_or_create_client_keypair_with_store(
    creds: &dyn CredentialStore,
    rng: &dyn NoiseRngFactory,
) -> Result<NoiseKeypair> {
    if let Some(hex) = creds.get_client_noise_key()?
        && !hex.trim().is_empty()
    {
        return NoiseKeypair::from_private_hex(&hex);
    }
    let kp = NoiseKeypair::generate_with_rng(rng)?;
    creds.set_client_noise_key(&kp.private_hex())?;
    Ok(kp)
}

/// Private file-backed credential store (`<data_dir>/client_noise.key`, mode `0o600` on Unix).
pub struct FileCredentialStore {
    path: PathBuf,
    durability: canban_context::FsDurability,
    lock: Mutex<()>,
}

impl FileCredentialStore {
    pub fn open(data_dir: &Path) -> Self {
        Self::open_with_durability(data_dir, canban_context::FsDurability::Full)
    }

    pub fn open_with_durability(data_dir: &Path, durability: canban_context::FsDurability) -> Self {
        Self {
            path: canban_transport::client_key_path(data_dir),
            durability,
            lock: Mutex::new(()),
        }
    }
}

impl CredentialStore for FileCredentialStore {
    fn get_client_noise_key(&self) -> Result<Option<String>> {
        let _guard = self.lock.lock().unwrap();
        match std::fs::read_to_string(&self.path) {
            Ok(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(trimmed.to_owned()))
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set_client_noise_key(&self, private_hex: &str) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
            set_private_mode(parent, 0o700);
        }
        let body = format!("{}\n", private_hex.trim());
        canban_context::atomic_with_durability(&self.path, body.as_bytes(), self.durability)?;
        set_private_mode(&self.path, 0o600);
        Ok(())
    }
}

/// In-memory credential store for ephemeral clients or tests.
#[derive(Default)]
pub struct MemoryCredentialStore {
    key: Mutex<Option<String>>,
}

impl MemoryCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn get_client_noise_key(&self) -> Result<Option<String>> {
        Ok(self.key.lock().unwrap().clone())
    }

    fn set_client_noise_key(&self, private_hex: &str) -> Result<()> {
        *self.key.lock().unwrap() = Some(private_hex.trim().to_owned());
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

/// Persistence adapter for saved host definitions (`hosts.json` — contains public host keys and
/// endpoints only, zero bearer secrets).
pub trait HostStore: Send + Sync + 'static {
    fn load_hosts(&self) -> Vec<SavedHost>;
    fn save_hosts(&self, hosts: &[SavedHost]) -> Result<()>;
    fn delete_credentials(&self, _host_id: &str) {}
}

/// Default mobile [`HostStore`] backed by `<data_dir>/hosts.json`.
pub struct CredentialHostStore {
    data_dir: PathBuf,
    durability: canban_context::FsDurability,
}

impl CredentialHostStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self::new_with_durability(data_dir, canban_context::FsDurability::Full)
    }

    pub fn new_with_durability(
        data_dir: PathBuf,
        durability: canban_context::FsDurability,
    ) -> Self {
        Self {
            data_dir,
            durability,
        }
    }
}

impl HostStore for CredentialHostStore {
    fn load_hosts(&self) -> Vec<SavedHost> {
        load_hosts(&self.data_dir)
    }

    fn save_hosts(&self, hosts: &[SavedHost]) -> Result<()> {
        save_hosts_with_durability(&self.data_dir, hosts, self.durability)
    }
}

/// Desktop [`HostStore`] reading and writing an explicit `hosts.json` path (e.g.
/// `<home>/config/hosts.json`).
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

pub fn load_hosts(data_dir: &Path) -> Vec<SavedHost> {
    std::fs::read(hosts_path(data_dir))
        .ok()
        .and_then(|b| serde_json::from_slice::<HostsFile>(&b).ok())
        .map(|f| f.hosts)
        .unwrap_or_default()
}

/// Persists `hosts` to `<data_dir>/hosts.json`.
pub fn save_hosts(data_dir: &Path, hosts: &[SavedHost]) -> Result<()> {
    save_hosts_with_durability(data_dir, hosts, canban_context::FsDurability::Full)
}

/// Persists `hosts` to `<data_dir>/hosts.json` using the specified [`canban_context::FsDurability`].
pub fn save_hosts_with_durability(
    data_dir: &Path,
    hosts: &[SavedHost],
    durability: canban_context::FsDurability,
) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    set_private_mode(data_dir, 0o700);
    let path = hosts_path(data_dir);
    canban_context::atomic_with_durability(
        &path,
        &serde_json::to_vec_pretty(&HostsFile {
            hosts: hosts.to_vec(),
        })?,
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

pub fn remove_host_files(data_dir: &Path, host_id: &str) {
    let _ = std::fs::remove_file(cache_path(data_dir, host_id));
    let _ = std::fs::remove_file(outbox_path(data_dir, host_id));
    let _ = std::fs::remove_dir_all(host_cache_dir(data_dir, host_id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_transport::system_noise_rng;
    use canban_types::HostTransport;

    #[test]
    fn file_credential_store_persists_client_noise_key_and_hosts_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let creds = FileCredentialStore::open(dir.path());
        let kp1 =
            load_or_create_client_keypair_with_store(&creds, system_noise_rng().as_ref()).unwrap();
        let kp2 =
            load_or_create_client_keypair_with_store(&creds, system_noise_rng().as_ref()).unwrap();
        assert_eq!(kp1, kp2);

        let host = SavedHost {
            id: "host-1".into(),
            name: "Workstation".into(),
            transport: HostTransport::Tcp {
                addr: "100.84.12.5:7680".into(),
                server_pubkey: Some(kp1.public_b64()),
                tailscale_auth: false,
            },
            ssh_target: None,
            server_id: Some("srv-1".into()),
            enabled: true,
            added_at: 42,
        };
        save_hosts(dir.path(), std::slice::from_ref(&host)).unwrap();
        let loaded = load_hosts(dir.path());
        assert_eq!(loaded, vec![host]);
    }
}
