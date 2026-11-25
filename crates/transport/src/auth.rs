//! Per-device revocable bearer tokens, single-use pairing codes, deprecated
//! `server.token` auto-upgrade verification, and live device revocation broadcast.
use anyhow::Result;
use canban_types::{
    Clock, DEVICE_TOKEN_PREFIX, IdGen, PAIRING_CODE_PREFIX, PairedDevice, is_device_token,
    is_pairing_code, system_clock, system_id_gen,
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
struct StoredDeviceToken {
    id: String,
    name: String,
    token_hash: String,
    created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_seen_at: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct DeviceTokensFile {
    #[serde(default)]
    devices: Vec<StoredDeviceToken>,
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

/// DEPRECATED: legacy static `server.token` path; retained only to auto-upgrade older hosts.
pub fn legacy_token_path(home: &Path) -> PathBuf {
    home.join("server.token")
}

/// DEPRECATED: read legacy static `server.token` if present.
pub fn read_legacy_server_token(home: &Path) -> Option<String> {
    std::fs::read_to_string(legacy_token_path(home))
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| s.len() >= 32)
}

/// DEPRECATED: generate legacy static `server.token` for backward-compatibility tests.
pub fn ensure_legacy_server_token(home: &Path) -> Result<String> {
    if let Some(existing) = read_legacy_server_token(home) {
        return Ok(existing);
    }
    let secret = format!("{}{}", canban_types::token_id(), canban_types::token_id());
    let path = legacy_token_path(home);
    write_private_atomic(&path, format!("{secret}\n").as_bytes(), true)?;
    Ok(secret)
}

/// DEPRECATED: constant-time token comparison for legacy `server.token` verification.
pub fn verify_legacy_token(expected: &str, provided: &str) -> bool {
    let expected = expected.trim();
    let provided = provided.trim();
    expected.len() >= 32 && ct_eq(expected, provided)
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
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() || ab.is_empty() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in ab.iter().zip(bb.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

impl From<&StoredDeviceToken> for PairedDevice {
    fn from(s: &StoredDeviceToken) -> Self {
        Self {
            id: s.id.clone(),
            name: s.name.clone(),
            created_at: s.created_at,
            last_seen_at: s.last_seen_at,
            auth_kind: String::new(),
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
    pub fn record(&self, device_id: &str, name: &str) {
        self.record_at(device_id, name, self.clock.now_ms());
    }

    /// Pure clock-injected variant of [`Self::record`].
    pub fn record_at(&self, device_id: &str, name: &str, now_ms: u64) {
        let clean = name.trim();
        let display_name = if clean.is_empty() {
            "Tailscale device"
        } else {
            clean
        };
        let mut guard = self.devices.lock().unwrap();
        if let Some(existing) = guard.get_mut(device_id) {
            existing.name = display_name.to_owned();
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

/// Generates an ephemeral per-session secret for `enc:v1:` Vault sealing on Tailscale connections.
pub fn generate_ephemeral_session_token() -> String {
    generate_ephemeral_session_token_with(&canban_types::SystemIdGen)
}

/// Deterministic / injected-`IdGen` variant of [`generate_ephemeral_session_token`].
pub fn generate_ephemeral_session_token_with(id_gen: &dyn IdGen) -> String {
    generate_prefixed_secret_with("ts_sess_", id_gen)
}

/// Broadcast bus notifying active transport sessions immediately when a device token is revoked.
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

/// File-backed per-device revocable token store (`<home>/device_tokens.json`, SHA-256 hashes).
/// Reloads from disk on each read/mutation so CLI commands and the running daemon stay consistent.
pub struct DeviceTokenStore {
    file: JsonSecretFile<DeviceTokensFile>,
    clock: Arc<dyn Clock>,
    id_gen: Arc<dyn IdGen>,
}

impl DeviceTokenStore {
    pub fn open(home: &Path) -> Self {
        Self::open_with_clock(home, system_clock())
    }

    pub fn open_with_clock(home: &Path, clock: Arc<dyn Clock>) -> Self {
        Self {
            file: JsonSecretFile::new(home.join("device_tokens.json")),
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

    /// Generates an ephemeral Tailscale session token using this store's configured [`IdGen`].
    pub fn generate_ephemeral_session_token(&self) -> String {
        generate_ephemeral_session_token_with(self.id_gen.as_ref())
    }

    pub fn issue_device(&self, name: &str) -> Result<(PairedDevice, String)> {
        self.issue_device_at(name, self.clock.now_ms())
    }

    /// Pure clock-injected variant of [`Self::issue_device`].
    pub fn issue_device_at(&self, name: &str, now_ms: u64) -> Result<(PairedDevice, String)> {
        let clean = name.trim();
        let display_name = if clean.is_empty() {
            "Remote device"
        } else {
            clean
        };
        let raw_token = generate_prefixed_secret_with(DEVICE_TOKEN_PREFIX, self.id_gen.as_ref());
        let entry = StoredDeviceToken {
            id: self.id_gen.next_id(),
            name: display_name.to_owned(),
            token_hash: hash_secret(&raw_token),
            created_at: now_ms,
            last_seen_at: None,
        };
        let view = PairedDevice::from(&entry);
        self.file.mutate(|data| {
            data.devices.push(entry);
            Ok((true, (view, raw_token)))
        })
    }

    pub fn verify(&self, candidate: &str) -> Option<PairedDevice> {
        self.verify_at(candidate, self.clock.now_ms())
    }

    /// Pure clock-injected variant of [`Self::verify`].
    pub fn verify_at(&self, candidate: &str, now_ms: u64) -> Option<PairedDevice> {
        let trimmed = candidate.trim();
        if !is_device_token(trimmed) || trimmed.len() < DEVICE_TOKEN_PREFIX.len() + 32 {
            return None;
        }
        let candidate_hash = hash_secret(trimmed);
        let mut matched = None;
        let _ = self.file.mutate(|data| {
            let mut dirty = false;
            for entry in &mut data.devices {
                if ct_eq(&entry.token_hash, &candidate_hash) {
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

pub enum TcpAuthOutcome {
    Authenticated {
        device_id: String,
        upgraded_token: Option<String>,
        deprecated_legacy: bool,
    },
    Rejected,
}

fn issue_upgraded_token_at(
    device_store: &DeviceTokenStore,
    device_name: Option<&str>,
    fallback_name: &str,
    deprecated_legacy: bool,
    now_ms: Option<u64>,
) -> Result<TcpAuthOutcome> {
    let effective_name = device_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_name);
    let (device, dev_token) = match now_ms {
        Some(ts) => device_store.issue_device_at(effective_name, ts)?,
        None => device_store.issue_device(effective_name)?,
    };
    Ok(TcpAuthOutcome::Authenticated {
        device_id: device.id,
        upgraded_token: Some(dev_token),
        deprecated_legacy,
    })
}

pub fn authenticate_tcp_token(
    home: &Path,
    device_store: &DeviceTokenStore,
    pairing_codes: &PairingCodeStore,
    raw_token: &str,
    device_name: Option<&str>,
    allow_auto_upgrade: bool,
) -> Result<TcpAuthOutcome> {
    authenticate_tcp_token_inner(
        home,
        device_store,
        pairing_codes,
        raw_token,
        device_name,
        allow_auto_upgrade,
        None,
    )
}

/// Pure clock-injected variant of [`authenticate_tcp_token`].
pub fn authenticate_tcp_token_at(
    home: &Path,
    device_store: &DeviceTokenStore,
    pairing_codes: &PairingCodeStore,
    raw_token: &str,
    device_name: Option<&str>,
    allow_auto_upgrade: bool,
    now_ms: u64,
) -> Result<TcpAuthOutcome> {
    authenticate_tcp_token_inner(
        home,
        device_store,
        pairing_codes,
        raw_token,
        device_name,
        allow_auto_upgrade,
        Some(now_ms),
    )
}

fn authenticate_tcp_token_inner(
    home: &Path,
    device_store: &DeviceTokenStore,
    pairing_codes: &PairingCodeStore,
    raw_token: &str,
    device_name: Option<&str>,
    allow_auto_upgrade: bool,
    now_ms: Option<u64>,
) -> Result<TcpAuthOutcome> {
    let trimmed = raw_token.trim();
    if trimmed.is_empty() {
        return Ok(TcpAuthOutcome::Rejected);
    }

    if is_pairing_code(trimmed) {
        let consumed = match now_ms {
            Some(ts) => allow_auto_upgrade && pairing_codes.consume_code_at(trimmed, ts)?,
            None => allow_auto_upgrade && pairing_codes.consume_code(trimmed)?,
        };
        if consumed {
            return issue_upgraded_token_at(
                device_store,
                device_name,
                "Remote device",
                false,
                now_ms,
            );
        }
        return Ok(TcpAuthOutcome::Rejected);
    }

    if is_device_token(trimmed) {
        let verified = match now_ms {
            Some(ts) => device_store.verify_at(trimmed, ts),
            None => device_store.verify(trimmed),
        };
        return Ok(match verified {
            Some(device) => TcpAuthOutcome::Authenticated {
                device_id: device.id,
                upgraded_token: None,
                deprecated_legacy: false,
            },
            None => TcpAuthOutcome::Rejected,
        });
    }

    // DEPRECATED: legacy shared static `server.token` fallback.
    if let Some(expected) = read_legacy_server_token(home)
        && verify_legacy_token(&expected, trimmed)
    {
        if allow_auto_upgrade {
            return issue_upgraded_token_at(
                device_store,
                device_name,
                "Upgraded legacy host",
                true,
                now_ms,
            );
        }
        return Ok(TcpAuthOutcome::Authenticated {
            device_id: "legacy-server-token".into(),
            upgraded_token: None,
            deprecated_legacy: true,
        });
    }

    Ok(TcpAuthOutcome::Rejected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_tokens_verify_list_and_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let store = DeviceTokenStore::open(dir.path());
        let (dev, tok) = store.issue_device("MacBook Pro").unwrap();
        assert!(is_device_token(&tok));
        assert!(store.verify("wrong").is_none());
        let verified = store.verify(&tok).expect("issued device token must verify");
        assert_eq!(verified.id, dev.id);
        assert_eq!(verified.name, "MacBook Pro");
        assert!(verified.last_seen_at.is_some());

        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, dev.id);

        assert!(store.revoke(&dev.id).unwrap());
        assert!(!store.revoke(&dev.id).unwrap());
        assert!(store.verify(&tok).is_none());
        assert!(store.list().is_empty());
    }

    #[test]
    fn pairing_code_is_single_use_and_auto_upgrades_legacy_server_token() {
        let dir = tempfile::tempdir().unwrap();
        let device_store = DeviceTokenStore::open(dir.path());
        let pairing_codes = PairingCodeStore::open(dir.path());

        let (code, _) = pairing_codes.create_code(300).unwrap();
        assert!(is_pairing_code(&code));

        // Non-upgrade RPC must not burn pairing codes.
        assert!(matches!(
            authenticate_tcp_token(
                dir.path(),
                &device_store,
                &pairing_codes,
                &code,
                Some("Work Mac"),
                false
            )
            .unwrap(),
            TcpAuthOutcome::Rejected
        ));

        // Mux upgrade exchanges the single-use pairing code for a revocable device token.
        let (dev_id, upgraded) = match authenticate_tcp_token(
            dir.path(),
            &device_store,
            &pairing_codes,
            &code,
            Some("Work Mac"),
            true,
        )
        .unwrap()
        {
            TcpAuthOutcome::Authenticated {
                device_id,
                upgraded_token,
                deprecated_legacy,
            } => {
                assert!(!deprecated_legacy);
                (device_id, upgraded_token.expect("upgraded token"))
            }
            TcpAuthOutcome::Rejected => panic!("expected pairing code authentication"),
        };
        assert!(is_device_token(&upgraded));
        assert!(device_store.is_valid_device_id(&dev_id));

        // Replay of the same pairing code fails.
        assert!(matches!(
            authenticate_tcp_token(
                dir.path(),
                &device_store,
                &pairing_codes,
                &code,
                Some("Work Mac"),
                true
            )
            .unwrap(),
            TcpAuthOutcome::Rejected
        ));

        // Legacy static server.token auto-upgrades to a per-device token on mux connect.
        let legacy = ensure_legacy_server_token(dir.path()).unwrap();
        let legacy_upgraded = match authenticate_tcp_token(
            dir.path(),
            &device_store,
            &pairing_codes,
            &legacy,
            Some("Legacy Mac"),
            true,
        )
        .unwrap()
        {
            TcpAuthOutcome::Authenticated {
                upgraded_token,
                deprecated_legacy,
                ..
            } => {
                assert!(deprecated_legacy);
                upgraded_token.expect("upgraded device token")
            }
            TcpAuthOutcome::Rejected => panic!("expected legacy token upgrade"),
        };
        assert!(is_device_token(&legacy_upgraded));
        assert_eq!(device_store.list().len(), 2);
    }

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

        let devices = DeviceTokenStore::open_with_clock(dir.path(), clock.clone())
            .with_id_gen(DeterministicIdGen::arc(7));
        let codes = PairingCodeStore::open_with_clock(dir.path(), clock.clone())
            .with_id_gen(DeterministicIdGen::arc(11));
        let ts = TailscaleSessionRegistry::new_with_clock(clock.clone());

        // Tailscale registry records created_at/last_seen_at and throttles refreshes to 30s.
        ts.record("ts:node-1", "Laptop");
        assert_eq!(ts.list()[0].created_at, T0);
        assert_eq!(ts.list()[0].last_seen_at, Some(T0));
        clock.0.store(T0 + 29_999, Ordering::SeqCst);
        ts.record("ts:node-1", "Laptop");
        assert_eq!(ts.list()[0].last_seen_at, Some(T0));
        clock.0.store(T0 + 30_000, Ordering::SeqCst);
        ts.record("ts:node-1", "Laptop");
        assert_eq!(ts.list()[0].last_seen_at, Some(T0 + 30_000));

        // Pairing code expires strictly at T0 + ttl_secs * 1000 under mock clock.
        clock.0.store(T0, Ordering::SeqCst);
        let (code_exp, expires_at) = codes.create_code(60).unwrap();
        assert_eq!(expires_at, T0 + 60_000);
        assert!(!codes.consume_code_at(&code_exp, T0 + 60_000).unwrap());

        let (code_ok, _) = codes.create_code_at(60, T0).unwrap();
        let upgraded_tok = match authenticate_tcp_token_at(
            dir.path(),
            &devices,
            &codes,
            &code_ok,
            Some("Phone"),
            true,
            T0 + 10_000,
        )
        .unwrap()
        {
            TcpAuthOutcome::Authenticated {
                upgraded_token: Some(tok),
                ..
            } => tok,
            _ => panic!("expected upgraded token"),
        };
        let listed = devices.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].created_at, T0 + 10_000);
        assert_eq!(listed[0].last_seen_at, None);

        // Device token verification updates last_seen_at and enforces 30s throttle.
        let v1 = devices.verify_at(&upgraded_tok, T0 + 12_000).unwrap();
        assert_eq!(v1.last_seen_at, Some(T0 + 12_000));
        let v2 = devices.verify_at(&upgraded_tok, T0 + 41_999).unwrap();
        assert_eq!(v2.last_seen_at, Some(T0 + 12_000));
        let v3 = devices.verify_at(&upgraded_tok, T0 + 42_000).unwrap();
        assert_eq!(v3.last_seen_at, Some(T0 + 42_000));
    }

    #[test]
    fn with_fs_sync_false_persists_reloads_and_revokes_device_tokens_and_pairing_codes() {
        let dir = tempfile::tempdir().unwrap();
        let devices = DeviceTokenStore::open(dir.path()).with_fs_sync(false);
        let codes = PairingCodeStore::open(dir.path()).with_fs_sync(false);

        let (code, _) = codes.create_code(300).unwrap();
        assert!(codes.consume_code(&code).unwrap());
        assert!(!codes.consume_code(&code).unwrap());

        let (dev, tok) = devices.issue_device("Sim Client").unwrap();
        let reloaded = DeviceTokenStore::open(dir.path());
        assert_eq!(reloaded.verify(&tok).map(|d| d.id), Some(dev.id.clone()));
        assert!(devices.revoke(&dev.id).unwrap());
        assert!(reloaded.verify(&tok).is_none());
    }

    #[test]
    fn ephemeral_tailscale_session_token_is_deterministic_with_injected_id_gen() {
        use canban_types::DeterministicIdGen;

        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let store_a = DeviceTokenStore::open(dir_a.path())
            .with_id_gen(DeterministicIdGen::arc(42))
            .with_fs_sync(false);
        let store_b = DeviceTokenStore::open(dir_b.path())
            .with_id_gen(DeterministicIdGen::arc(42))
            .with_fs_sync(false);

        let tok_a = store_a.generate_ephemeral_session_token();
        let tok_b = store_b.generate_ephemeral_session_token();
        assert!(tok_a.starts_with("ts_sess_"));
        assert_eq!(tok_a, tok_b);
    }
}
