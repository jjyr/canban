//! Noise public-key device authorization (`authorized_devices.json`), single-use pairing codes
//! (`pairing_codes.json`), Tailscale WhoIs session tracking, and live device revocation broadcast.
use crate::noise::{
    ct_eq_bytes, decode_pubkey, encode_pubkey, pubkey_b64_fingerprint, pubkey_fingerprint,
};
use anyhow::Result;
use canban_types::{
    Clock, IdGen, PAIRING_CODE_PREFIX, PairedDevice, is_pairing_code, system_clock, system_id_gen,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Write,
    marker::PhantomData,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredAuthorizedDevice {
    id: String,
    name: String,
    pubkey: String,
    created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_seen_at: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AuthorizedDevicesFile {
    #[serde(default)]
    devices: Vec<StoredAuthorizedDevice>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredPairingCode {
    code_hash: String,
    created_at: u64,
    expires_at: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PairingCodesFile {
    #[serde(default)]
    codes: Vec<StoredPairingCode>,
}

fn write_private_atomic(path: &Path, content: &[u8], fs_sync: bool) -> Result<()> {
    static NEXT_TMP_SEQ: AtomicU64 = AtomicU64::new(1);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let seq = NEXT_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(
        ".{}.tmp-{}-{seq}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("secret"),
        std::process::id(),
    );
    let tmp_path = parent.join(tmp_name);
    let write_res = (|| -> Result<()> {
        let mut opts = OpenOptions::new();
        opts.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp_path)?;
        file.write_all(content)?;
        if fs_sync {
            file.sync_all()?;
        }
        Ok(())
    })();
    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn hash_secret(raw: &str) -> String {
    let digest = Sha256::digest(raw.trim().as_bytes());
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn ct_eq(a: &str, b: &str) -> bool {
    ct_eq_bytes(a.as_bytes(), b.as_bytes())
}

impl From<&StoredAuthorizedDevice> for PairedDevice {
    fn from(s: &StoredAuthorizedDevice) -> Self {
        Self {
            id: s.id.clone(),
            name: s.name.clone(),
            created_at: s.created_at,
            last_seen_at: s.last_seen_at,
            auth_kind: "key".to_owned(),
            pubkey_fingerprint: pubkey_b64_fingerprint(&s.pubkey).unwrap_or_default(),
        }
    }
}

/// Verified Tailscale peer identity resolved from `tailscaled` `whois` on the daemon host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailscalePeerIdentity {
    pub node_id: String,
    pub device_name: String,
    pub login_name: String,
}

/// Server-side verifier injected by `canban-app-server` to authorize same-account Tailscale peers.
pub trait TailscalePeerVerifier: Send + Sync + 'static {
    fn verify_peer(&self, peer_addr: SocketAddr) -> Option<TailscalePeerIdentity>;
}

/// In-memory registry of Tailscale devices connected during the daemon's current lifetime.
pub struct TailscaleSessionRegistry {
    devices: Mutex<BTreeMap<String, PairedDevice>>,
    clock: Arc<dyn Clock>,
}

impl Default for TailscaleSessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TailscaleSessionRegistry {
    pub fn new() -> Self {
        Self::new_with_clock(system_clock())
    }

    pub fn new_with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            devices: Mutex::new(BTreeMap::new()),
            clock,
        }
    }

    /// Records or refreshes a live Tailscale peer entry (last_seen throttled to 30s).
    pub fn record(&self, device_id: &str, name: &str, pubkey_fingerprint: &str) {
        self.record_at(device_id, name, pubkey_fingerprint, self.clock.now_ms());
    }

    /// Pure clock-injected variant of [`Self::record`].
    pub fn record_at(&self, device_id: &str, name: &str, pubkey_fingerprint: &str, now_ms: u64) {
        let clean = name.trim();
        let display_name = if clean.is_empty() {
            "Tailscale device"
        } else {
            clean
        };
        let mut guard = self.devices.lock().unwrap();
        if let Some(existing) = guard.get_mut(device_id) {
            existing.name = display_name.to_owned();
            if !pubkey_fingerprint.is_empty() {
                existing.pubkey_fingerprint = pubkey_fingerprint.to_owned();
            }
            if existing
                .last_seen_at
                .is_none_or(|prev| now_ms.saturating_sub(prev) >= 30_000)
            {
                existing.last_seen_at = Some(now_ms);
            }
        } else {
            guard.insert(
                device_id.to_owned(),
                PairedDevice {
                    id: device_id.to_owned(),
                    name: display_name.to_owned(),
                    created_at: now_ms,
                    last_seen_at: Some(now_ms),
                    auth_kind: "tailscale".to_owned(),
                    pubkey_fingerprint: pubkey_fingerprint.to_owned(),
                },
            );
        }
    }

    /// Returns recorded Tailscale devices sorted by most recent activity first.
    pub fn list(&self) -> Vec<PairedDevice> {
        let guard = self.devices.lock().unwrap();
        let mut items: Vec<PairedDevice> = guard.values().cloned().collect();
        items.sort_by(|a, b| {
            let ka = a.last_seen_at.unwrap_or(a.created_at);
            let kb = b.last_seen_at.unwrap_or(b.created_at);
            kb.cmp(&ka).then_with(|| a.name.cmp(&b.name))
        });
        items
    }
}

/// Broadcast bus notifying active transport sessions immediately when a device is revoked.
#[derive(Clone)]
pub struct RevocationBus {
    tx: tokio::sync::broadcast::Sender<String>,
}

impl RevocationBus {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(64);
        Self { tx }
    }

    pub fn revoke(&self, device_id: impl Into<String>) {
        let _ = self.tx.send(device_id.into());
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.tx.subscribe()
    }
}

impl Default for RevocationBus {
    fn default() -> Self {
        Self::new()
    }
}

struct JsonSecretFile<T> {
    path: PathBuf,
    fs_sync: bool,
    lock: Mutex<()>,
    _marker: PhantomData<fn() -> T>,
}

impl<T: DeserializeOwned + Default> JsonSecretFile<T> {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            fs_sync: true,
            lock: Mutex::new(()),
            _marker: PhantomData,
        }
    }

    fn load_locked(&self) -> T {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn read<R>(&self, f: impl FnOnce(T) -> R) -> R {
        let _guard = self.lock.lock().unwrap();
        f(self.load_locked())
    }
}

impl<T: Serialize + DeserializeOwned + Default> JsonSecretFile<T> {
    fn mutate<R>(&self, f: impl FnOnce(&mut T) -> Result<(bool, R)>) -> Result<R> {
        let _guard = self.lock.lock().unwrap();
        let mut data = self.load_locked();
        let (dirty, out) = f(&mut data)?;
        if dirty {
            write_private_atomic(&self.path, &serde_json::to_vec_pretty(&data)?, self.fs_sync)?;
        }
        Ok(out)
    }
}

fn generate_prefixed_secret_with(prefix: &str, id_gen: &dyn IdGen) -> String {
    format!("{prefix}{}{}", id_gen.next_token(), id_gen.next_token())
}

/// File-backed authorized client Noise public-key registry (`<home>/authorized_devices.json`).
/// Reloads from disk on each read/mutation so CLI commands and the running daemon stay consistent.
pub struct AuthorizedDeviceStore {
    file: JsonSecretFile<AuthorizedDevicesFile>,
    clock: Arc<dyn Clock>,
    id_gen: Arc<dyn IdGen>,
}

impl AuthorizedDeviceStore {
    pub fn open(home: &Path) -> Self {
        Self::open_with_clock(home, system_clock())
    }

    pub fn open_with_clock(home: &Path, clock: Arc<dyn Clock>) -> Self {
        Self {
            file: JsonSecretFile::new(home.join("authorized_devices.json")),
            clock,
            id_gen: system_id_gen(),
        }
    }

    pub fn with_id_gen(mut self, id_gen: Arc<dyn IdGen>) -> Self {
        self.id_gen = id_gen;
        self
    }

    /// Controls whether atomic file writes call `File::sync_all()`.
    /// Production defaults to `true`; ephemeral DST harnesses may disable fsync (`false`).
    pub fn with_fs_sync(mut self, fs_sync: bool) -> Self {
        self.file.fs_sync = fs_sync;
        self
    }

    /// Authorizes a client's X25519 Noise public key (idempotent by public key).
    pub fn authorize_pubkey(&self, name: &str, pubkey_b64: &str) -> Result<PairedDevice> {
        self.authorize_pubkey_at(name, pubkey_b64, self.clock.now_ms())
    }

    /// Pure clock-injected variant of [`Self::authorize_pubkey`].
    pub fn authorize_pubkey_at(
        &self,
        name: &str,
        pubkey_b64: &str,
        now_ms: u64,
    ) -> Result<PairedDevice> {
        let raw_pk = decode_pubkey(pubkey_b64)?;
        let normalized_pk = encode_pubkey(&raw_pk);
        let clean = name.trim();
        let display_name = if clean.is_empty() {
            "Remote device"
        } else {
            clean
        };
        self.file.mutate(|data| {
            if let Some(existing) = data
                .devices
                .iter_mut()
                .find(|d| ct_eq(&d.pubkey, &normalized_pk))
            {
                let mut dirty = false;
                if !clean.is_empty() && existing.name != display_name {
                    existing.name = display_name.to_owned();
                    dirty = true;
                }
                if existing
                    .last_seen_at
                    .is_none_or(|prev| now_ms.saturating_sub(prev) >= 30_000)
                {
                    existing.last_seen_at = Some(now_ms);
                    dirty = true;
                }
                return Ok((dirty, PairedDevice::from(&*existing)));
            }
            let entry = StoredAuthorizedDevice {
                id: self.id_gen.next_id(),
                name: display_name.to_owned(),
                pubkey: normalized_pk,
                created_at: now_ms,
                last_seen_at: None,
            };
            let view = PairedDevice::from(&entry);
            data.devices.push(entry);
            Ok((true, view))
        })
    }

    /// Verifies that `pubkey_b64` belongs to an authorized device and refreshes `last_seen_at`.
    pub fn verify_pubkey(&self, pubkey_b64: &str) -> Option<PairedDevice> {
        self.verify_pubkey_at(pubkey_b64, self.clock.now_ms())
    }

    /// Pure clock-injected variant of [`Self::verify_pubkey`].
    pub fn verify_pubkey_at(&self, pubkey_b64: &str, now_ms: u64) -> Option<PairedDevice> {
        let raw_pk = decode_pubkey(pubkey_b64).ok()?;
        let normalized_pk = encode_pubkey(&raw_pk);
        let mut matched = None;
        let _ = self.file.mutate(|data| {
            let mut dirty = false;
            for entry in &mut data.devices {
                if ct_eq(&entry.pubkey, &normalized_pk) {
                    if entry
                        .last_seen_at
                        .is_none_or(|prev| now_ms.saturating_sub(prev) >= 30_000)
                    {
                        entry.last_seen_at = Some(now_ms);
                        dirty = true;
                    }
                    matched = Some(PairedDevice::from(&*entry));
                    break;
                }
            }
            Ok((dirty, ()))
        });
        matched
    }

    pub fn is_valid_device_id(&self, device_id: &str) -> bool {
        self.file
            .read(|data| data.devices.iter().any(|d| d.id == device_id))
    }

    pub fn list(&self) -> Vec<PairedDevice> {
        self.file.read(|data| {
            let mut items: Vec<PairedDevice> =
                data.devices.iter().map(PairedDevice::from).collect();
            items.sort_by(|a, b| {
                let ka = a.last_seen_at.unwrap_or(a.created_at);
                let kb = b.last_seen_at.unwrap_or(b.created_at);
                kb.cmp(&ka).then_with(|| a.name.cmp(&b.name))
            });
            items
        })
    }

    pub fn revoke(&self, device_id: &str) -> Result<bool> {
        self.file.mutate(|data| {
            let before = data.devices.len();
            data.devices.retain(|d| d.id != device_id);
            let removed = data.devices.len() < before;
            Ok((removed, removed))
        })
    }
}

/// File-backed single-use short-lived pairing code store (`<home>/pairing_codes.json`).
pub struct PairingCodeStore {
    file: JsonSecretFile<PairingCodesFile>,
    clock: Arc<dyn Clock>,
    id_gen: Arc<dyn IdGen>,
}

impl PairingCodeStore {
    pub fn open(home: &Path) -> Self {
        Self::open_with_clock(home, system_clock())
    }

    pub fn open_with_clock(home: &Path, clock: Arc<dyn Clock>) -> Self {
        Self {
            file: JsonSecretFile::new(home.join("pairing_codes.json")),
            clock,
            id_gen: system_id_gen(),
        }
    }

    pub fn with_id_gen(mut self, id_gen: Arc<dyn IdGen>) -> Self {
        self.id_gen = id_gen;
        self
    }

    /// Controls whether atomic secret-file writes call `File::sync_all()`.
    /// Production defaults to `true`; ephemeral DST harnesses may disable fsync (`false`).
    pub fn with_fs_sync(mut self, fs_sync: bool) -> Self {
        self.file.fs_sync = fs_sync;
        self
    }

    pub fn create_code(&self, ttl_secs: u64) -> Result<(String, u64)> {
        self.create_code_at(ttl_secs, self.clock.now_ms())
    }

    /// Pure clock-injected variant of [`Self::create_code`].
    pub fn create_code_at(&self, ttl_secs: u64, now_ms: u64) -> Result<(String, u64)> {
        let bounded_secs = ttl_secs.clamp(30, 3600);
        let expires_at = now_ms.saturating_add(bounded_secs * 1000);
        let raw_code = generate_prefixed_secret_with(PAIRING_CODE_PREFIX, self.id_gen.as_ref());
        let entry = StoredPairingCode {
            code_hash: hash_secret(&raw_code),
            created_at: now_ms,
            expires_at,
        };
        self.file.mutate(|data| {
            data.codes.retain(|c| c.expires_at > now_ms);
            data.codes.push(entry);
            Ok((true, (raw_code, expires_at)))
        })
    }

    pub fn consume_code(&self, candidate: &str) -> Result<bool> {
        self.consume_code_at(candidate, self.clock.now_ms())
    }

    /// Pure clock-injected variant of [`Self::consume_code`].
    pub fn consume_code_at(&self, candidate: &str, now_ms: u64) -> Result<bool> {
        let trimmed = candidate.trim();
        if !is_pairing_code(trimmed) || trimmed.len() < PAIRING_CODE_PREFIX.len() + 16 {
            return Ok(false);
        }
        let candidate_hash = hash_secret(trimmed);
        self.file.mutate(|data| {
            let before = data.codes.len();
            data.codes.retain(|c| c.expires_at > now_ms);
            let matched = data
                .codes
                .iter()
                .position(|c| ct_eq(&c.code_hash, &candidate_hash));
            if let Some(idx) = matched {
                data.codes.remove(idx);
                return Ok((true, true));
            }
            Ok((data.codes.len() < before, false))
        })
    }
}

/// Result of authenticating an already-established `Noise_XX` TCP session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoiseAuthOutcome {
    Authenticated {
        device_id: String,
        auth_kind: &'static str,
    },
    Rejected {
        is_tailscale: bool,
    },
}

/// Authorizes an established `Noise_XX` peer (`client_static`) via one-time pairing code,
/// Tailscale WhoIs same-account verification, or persistent `AuthorizedDeviceStore` membership.
#[allow(clippy::too_many_arguments)]
pub fn authenticate_noise_session(
    device_store: &AuthorizedDeviceStore,
    pairing_codes: &PairingCodeStore,
    tailscale_verifier: Option<&dyn TailscalePeerVerifier>,
    tailscale_devices: &TailscaleSessionRegistry,
    peer_addr: Option<SocketAddr>,
    client_static: &[u8; 32],
    auth_mode: &str,
    pairing_code: Option<&str>,
    device_name: Option<&str>,
    allow_pairing: bool,
) -> Result<NoiseAuthOutcome> {
    let client_pk_b64 = encode_pubkey(client_static);
    let client_fp = pubkey_fingerprint(client_static);

    if let Some(raw_code) = pairing_code.map(str::trim).filter(|s| !s.is_empty()) {
        if allow_pairing && is_pairing_code(raw_code) && pairing_codes.consume_code(raw_code)? {
            let effective_name = device_name
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("Remote device");
            let device = device_store.authorize_pubkey(effective_name, &client_pk_b64)?;
            return Ok(NoiseAuthOutcome::Authenticated {
                device_id: device.id,
                auth_kind: "key",
            });
        }
        return Ok(NoiseAuthOutcome::Rejected {
            is_tailscale: false,
        });
    }

    if auth_mode == "tailscale" {
        if let (Some(verifier), Some(addr)) = (tailscale_verifier, peer_addr)
            && let Some(peer) = verifier.verify_peer(addr)
        {
            let device_id = format!("ts:{}", peer.node_id);
            let effective_name = device_name
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&peer.device_name);
            tailscale_devices.record(&device_id, effective_name, &client_fp);
            return Ok(NoiseAuthOutcome::Authenticated {
                device_id,
                auth_kind: "tailscale",
            });
        }
        return Ok(NoiseAuthOutcome::Rejected { is_tailscale: true });
    }

    if let Some(device) = device_store.verify_pubkey(&client_pk_b64) {
        return Ok(NoiseAuthOutcome::Authenticated {
            device_id: device.id,
            auth_kind: "key",
        });
    }

    Ok(NoiseAuthOutcome::Rejected {
        is_tailscale: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noise::{DeterministicNoiseRng, NoiseKeypair};

    #[test]
    fn clock_injected_auth_stores_and_tailscale_registry_throttle_and_expire_deterministically() {
        use canban_types::DeterministicIdGen;
        use std::sync::atomic::{AtomicU64, Ordering};

        struct MockClock(AtomicU64);
        impl Clock for MockClock {
            fn now_ms(&self) -> u64 {
                self.0.load(Ordering::SeqCst)
            }
        }

        const T0: u64 = 1_700_000_000_000;
        let clock = Arc::new(MockClock(AtomicU64::new(T0)));
        let dir = tempfile::tempdir().unwrap();

        let devices = AuthorizedDeviceStore::open_with_clock(dir.path(), clock.clone())
            .with_id_gen(DeterministicIdGen::arc(7));
        let codes = PairingCodeStore::open_with_clock(dir.path(), clock.clone())
            .with_id_gen(DeterministicIdGen::arc(11));
        let ts = TailscaleSessionRegistry::new_with_clock(clock.clone());
        let rng = DeterministicNoiseRng::new(DeterministicIdGen::arc(13));
        let kp = NoiseKeypair::generate_with_rng(&rng).unwrap();
        let pk_b64 = kp.public_b64();
        let fp = kp.fingerprint();

        // Tailscale registry records created_at/last_seen_at and throttles refreshes to 30s.
        ts.record("ts:node-1", "Laptop", &fp);
        assert_eq!(ts.list()[0].created_at, T0);
        assert_eq!(ts.list()[0].last_seen_at, Some(T0));
        assert_eq!(ts.list()[0].pubkey_fingerprint, fp);
        clock.0.store(T0 + 29_999, Ordering::SeqCst);
        ts.record("ts:node-1", "Laptop", &fp);
        assert_eq!(ts.list()[0].last_seen_at, Some(T0));
        clock.0.store(T0 + 30_000, Ordering::SeqCst);
        ts.record("ts:node-1", "Laptop", &fp);
        assert_eq!(ts.list()[0].last_seen_at, Some(T0 + 30_000));

        // Pairing code expires strictly at T0 + ttl_secs * 1000 under mock clock.
        clock.0.store(T0, Ordering::SeqCst);
        let (code_exp, expires_at) = codes.create_code(60).unwrap();
        assert_eq!(expires_at, T0 + 60_000);
        assert!(!codes.consume_code_at(&code_exp, T0 + 60_000).unwrap());

        let (code_ok, _) = codes.create_code_at(60, T0).unwrap();
        clock.0.store(T0 + 10_000, Ordering::SeqCst);
        assert_eq!(
            authenticate_noise_session(
                &devices,
                &codes,
                None,
                &ts,
                None,
                kp.public_bytes(),
                "key",
                Some(&code_ok),
                Some("Phone"),
                true,
            )
            .unwrap(),
            NoiseAuthOutcome::Authenticated {
                device_id: devices.list()[0].id.clone(),
                auth_kind: "key",
            }
        );
        let listed = devices.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].created_at, T0 + 10_000);
        assert_eq!(listed[0].last_seen_at, None);
        assert_eq!(listed[0].pubkey_fingerprint, fp);

        // Device pubkey verification updates last_seen_at and enforces 30s throttle.
        let v1 = devices.verify_pubkey_at(&pk_b64, T0 + 12_000).unwrap();
        assert_eq!(v1.last_seen_at, Some(T0 + 12_000));
        let v2 = devices.verify_pubkey_at(&pk_b64, T0 + 41_999).unwrap();
        assert_eq!(v2.last_seen_at, Some(T0 + 12_000));
        let v3 = devices.verify_pubkey_at(&pk_b64, T0 + 42_000).unwrap();
        assert_eq!(v3.last_seen_at, Some(T0 + 42_000));
    }

    #[test]
    fn with_fs_sync_false_persists_reloads_and_revokes_authorized_devices_and_pairing_codes() {
        let dir = tempfile::tempdir().unwrap();
        let devices = AuthorizedDeviceStore::open(dir.path()).with_fs_sync(false);
        let codes = PairingCodeStore::open(dir.path()).with_fs_sync(false);
        let kp = NoiseKeypair::generate().unwrap();
        let pk_b64 = kp.public_b64();

        let (code, _) = codes.create_code(300).unwrap();
        assert!(codes.consume_code(&code).unwrap());
        assert!(!codes.consume_code(&code).unwrap());

        let dev = devices.authorize_pubkey("Sim Client", &pk_b64).unwrap();
        let reloaded = AuthorizedDeviceStore::open(dir.path());
        assert_eq!(
            reloaded.verify_pubkey(&pk_b64).map(|d| d.id),
            Some(dev.id.clone())
        );
        assert!(devices.revoke(&dev.id).unwrap());
        assert!(reloaded.verify_pubkey(&pk_b64).is_none());
    }
}
