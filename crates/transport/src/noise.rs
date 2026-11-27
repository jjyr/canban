//! End-to-end encrypted TCP transport using `Noise_XX_25519_ChaChaPoly_BLAKE2s` (`snow`),
//! with `b"CBN1"` protocol preface, strict pre-message-3 server static key pinning,
//! persistent X25519 identity keypairs, and deterministic RNG injection for DST.
use crate::framing::AsyncIo;
use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64URL};
use canban_types::IdGen;
use sha2::{Digest, Sha256};
use snow::{
    TransportState,
    params::{CipherChoice, DHChoice, HashChoice, NoiseParams},
    resolvers::{CryptoResolver, DefaultResolver},
    types::{Cipher, Dh, Hash, Random},
};
use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use zeroize::Zeroizing;

/// Noise handshake pattern and cipher suite used on every Canban TCP link.
pub const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
/// 4-byte wire preface sent by clients before Noise message 1 (`CBN1`, never starts with `{`).
pub const NOISE_PREFACE: &[u8; 4] = b"CBN1";
/// Single-byte acknowledgement written by the server before Noise message 2.
pub const NOISE_ACK: u8 = 0x00;
/// Noise protocol specification maximum message size (ciphertext + 16-byte Poly1305 tag).
pub const MAX_NOISE_MESSAGE_LEN: usize = 65_535;
/// Poly1305 AEAD tag length in bytes.
pub const NOISE_AEAD_TAG_LEN: usize = 16;
/// Plaintext chunk size per Noise transport frame (32 KiB, well under 65519-byte Noise limit).
pub const MAX_NOISE_CHUNK_PLAINTEXT: usize = 32 * 1024;
/// Maximum handshake message size accepted during Noise_XX negotiation.
const MAX_HANDSHAKE_MSG_LEN: usize = 4096;
/// Handshake read/write timeout.
pub const NOISE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

fn noise_params() -> NoiseParams {
    NOISE_PATTERN
        .parse()
        .expect("static Noise_XX_25519_ChaChaPoly_BLAKE2s params are valid")
}

/// Constant-time byte slice equality check.
pub fn ct_eq_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Encodes a 32-byte X25519 public key as 43-character URL-safe unpadded base64.
pub fn encode_pubkey(pubkey: &[u8; 32]) -> String {
    B64URL.encode(pubkey)
}

/// Decodes a 43-character URL-safe unpadded base64 X25519 public key into 32 raw bytes.
pub fn decode_pubkey(raw: &str) -> Result<[u8; 32]> {
    let trimmed = raw.trim();
    ensure!(!trimmed.is_empty(), "Missing Noise public key");
    let decoded = B64URL
        .decode(trimmed)
        .context("Invalid base64url Noise public key")?;
    let bytes: [u8; 32] = decoded
        .try_into()
        .map_err(|_| anyhow!("Invalid Noise public key length (expected 32 bytes)"))?;
    ensure!(
        bytes.iter().any(|&b| b != 0),
        "Invalid all-zero Noise public key"
    );
    Ok(bytes)
}

/// Computes a short human-readable key fingerprint (`XXXX-XXXX-XXXX`, 48-bit SHA-256 prefix).
pub fn pubkey_fingerprint(pubkey: &[u8; 32]) -> String {
    let digest = Sha256::digest(pubkey);
    format!(
        "{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5]
    )
}

/// Computes a human-readable fingerprint from an encoded base64url public key string.
pub fn pubkey_b64_fingerprint(pubkey_b64: &str) -> Option<String> {
    decode_pubkey(pubkey_b64)
        .ok()
        .map(|pk| pubkey_fingerprint(&pk))
}

/// Factory providing a `snow` [`CryptoResolver`] for keypair generation and Noise handshakes.
///
/// Production uses [`SystemNoiseRng`] (`getrandom` CSPRNG); DST harnesses inject
/// [`DeterministicNoiseRng`] backed by `canban_types::IdGen` so keys and ephemeral handshakes
/// are 100% seed-deterministic.
pub trait NoiseRngFactory: Send + Sync + 'static {
    fn build_resolver(&self) -> Box<dyn CryptoResolver + Send>;
}

/// Production OS CSPRNG resolver backed by `snow::resolvers::DefaultResolver`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemNoiseRng;

impl NoiseRngFactory for SystemNoiseRng {
    fn build_resolver(&self) -> Box<dyn CryptoResolver + Send> {
        Box::new(DefaultResolver)
    }
}

/// Returns the default OS CSPRNG [`NoiseRngFactory`].
pub fn system_noise_rng() -> Arc<dyn NoiseRngFactory> {
    Arc::new(SystemNoiseRng)
}

struct DeterministicRandom {
    seed: [u8; 32],
    counter: u64,
}

impl Random for DeterministicRandom {
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), snow::Error> {
        let mut offset = 0usize;
        while offset < dest.len() {
            let mut hasher = Sha256::new();
            hasher.update(b"canban-noise-dst-drbg-v1\0");
            hasher.update(self.seed);
            hasher.update(self.counter.to_le_bytes());
            self.counter = self.counter.wrapping_add(1);
            let block = hasher.finalize();
            let take = (dest.len() - offset).min(block.len());
            dest[offset..offset + take].copy_from_slice(&block[..take]);
            offset += take;
        }
        Ok(())
    }
}

struct DeterministicCryptoResolver {
    seed: [u8; 32],
}

impl CryptoResolver for DeterministicCryptoResolver {
    fn resolve_rng(&self) -> Option<Box<dyn Random>> {
        Some(Box::new(DeterministicRandom {
            seed: self.seed,
            counter: 0,
        }))
    }

    fn resolve_dh(&self, choice: &DHChoice) -> Option<Box<dyn Dh>> {
        DefaultResolver.resolve_dh(choice)
    }

    fn resolve_hash(&self, choice: &HashChoice) -> Option<Box<dyn Hash>> {
        DefaultResolver.resolve_hash(choice)
    }

    fn resolve_cipher(&self, choice: &CipherChoice) -> Option<Box<dyn Cipher>> {
        DefaultResolver.resolve_cipher(choice)
    }
}

/// Deterministic [`NoiseRngFactory`] deriving per-operation DRBG seeds from an injected [`IdGen`].
pub struct DeterministicNoiseRng {
    id_gen: Arc<dyn IdGen>,
    seq: AtomicU64,
}

impl DeterministicNoiseRng {
    pub fn new(id_gen: Arc<dyn IdGen>) -> Self {
        Self {
            id_gen,
            seq: AtomicU64::new(0),
        }
    }

    pub fn arc(id_gen: Arc<dyn IdGen>) -> Arc<dyn NoiseRngFactory> {
        Arc::new(Self::new(id_gen))
    }
}

impl NoiseRngFactory for DeterministicNoiseRng {
    fn build_resolver(&self) -> Box<dyn CryptoResolver + Send> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let mut hasher = Sha256::new();
        hasher.update(b"canban-noise-dst-seed-v1\0");
        hasher.update(self.id_gen.next_uuid().as_bytes());
        hasher.update(seq.to_le_bytes());
        let digest = hasher.finalize();
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&digest);
        Box::new(DeterministicCryptoResolver { seed })
    }
}

/// Persistent static X25519 identity keypair for a Canban daemon or client.
#[derive(Clone)]
pub struct NoiseKeypair {
    private: Zeroizing<[u8; 32]>,
    public: [u8; 32],
}

impl fmt::Debug for NoiseKeypair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NoiseKeypair")
            .field("public", &self.public_b64())
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

impl PartialEq for NoiseKeypair {
    fn eq(&self, other: &Self) -> bool {
        ct_eq_bytes(&self.public, &other.public) && ct_eq_bytes(&*self.private, &*other.private)
    }
}

impl Eq for NoiseKeypair {}

impl NoiseKeypair {
    /// Generates a new X25519 keypair using the OS CSPRNG.
    pub fn generate() -> Result<Self> {
        Self::generate_with_rng(&SystemNoiseRng)
    }

    /// Generates a new X25519 keypair using the provided [`NoiseRngFactory`].
    pub fn generate_with_rng(rng: &dyn NoiseRngFactory) -> Result<Self> {
        let kp = snow::Builder::with_resolver(noise_params(), rng.build_resolver())
            .generate_keypair()
            .context("Failed to generate Noise X25519 keypair")?;
        let priv_bytes: [u8; 32] = kp
            .private
            .try_into()
            .map_err(|_| anyhow!("Unexpected X25519 private key length"))?;
        let pub_bytes: [u8; 32] = kp
            .public
            .try_into()
            .map_err(|_| anyhow!("Unexpected X25519 public key length"))?;
        Ok(Self {
            private: Zeroizing::new(priv_bytes),
            public: pub_bytes,
        })
    }

    /// Reconstructs a keypair from a 32-byte X25519 private key, deriving the public key.
    pub fn from_private_bytes(private: [u8; 32]) -> Result<Self> {
        let mut dh = DefaultResolver
            .resolve_dh(&DHChoice::Curve25519)
            .context("Curve25519 DH resolver unavailable")?;
        dh.set(&private);
        let pub_bytes: [u8; 32] = dh
            .pubkey()
            .try_into()
            .map_err(|_| anyhow!("Unexpected derived X25519 public key length"))?;
        ensure!(
            pub_bytes.iter().any(|&b| b != 0),
            "Invalid X25519 private key (derived zero public key)"
        );
        Ok(Self {
            private: Zeroizing::new(private),
            public: pub_bytes,
        })
    }

    /// Encodes the 32-byte private key as 64 lowercase hex characters.
    pub fn private_hex(&self) -> Zeroizing<String> {
        use std::fmt::Write as _;
        let mut out = Zeroizing::new(String::with_capacity(64));
        for b in self.private.iter() {
            let _ = write!(&mut *out, "{b:02x}");
        }
        out
    }

    /// Decodes a 64-character hex X25519 private key and derives its matching public key.
    pub fn from_private_hex(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        ensure!(
            trimmed.len() == 64,
            "Invalid Noise private key hex length (expected 64 hex chars)"
        );
        let mut key = Zeroizing::new([0u8; 32]);
        for (i, slot) in key.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&trimmed[i * 2..i * 2 + 2], 16)
                .context("Invalid hex in Noise private key")?;
        }
        Self::from_private_bytes(*key)
    }

    pub fn private_bytes(&self) -> &[u8; 32] {
        &self.private
    }

    pub fn public_bytes(&self) -> &[u8; 32] {
        &self.public
    }

    pub fn public_b64(&self) -> String {
        encode_pubkey(&self.public)
    }

    pub fn fingerprint(&self) -> String {
        pubkey_fingerprint(&self.public)
    }
}

/// Canonical daemon static Noise key path (`<home>/noise.key`).
pub fn server_key_path(home: &Path) -> PathBuf {
    home.join("noise.key")
}

/// Canonical client static Noise key path (`<data_dir>/client_noise.key`).
pub fn client_key_path(data_dir: &Path) -> PathBuf {
    data_dir.join("client_noise.key")
}

fn ensure_not_symlink(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) => {
            ensure!(!m.file_type().is_symlink(), "{label} must not be a symlink");
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn write_key_atomic(path: &Path, content: &[u8], fs_sync: bool) -> Result<()> {
    static NEXT_TMP_SEQ: AtomicU64 = AtomicU64::new(1);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let seq = NEXT_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(
        ".{}.tmp-{}-{seq}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("noise.key"),
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
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Loads an existing Noise keypair from `path`, or creates and persists a fresh `0600` keypair.
pub fn load_or_create_keypair(
    path: &Path,
    fs_sync: bool,
    rng: &dyn NoiseRngFactory,
) -> Result<NoiseKeypair> {
    ensure_not_symlink(path, "noise.key")?;
    if path.exists() {
        ensure!(
            fs::symlink_metadata(path)?.is_file(),
            "{} must be a regular file",
            path.display()
        );
        let raw = Zeroizing::new(
            fs::read_to_string(path)
                .with_context(|| format!("Failed reading {}", path.display()))?,
        );
        return NoiseKeypair::from_private_hex(&raw);
    }
    let kp = NoiseKeypair::generate_with_rng(rng)?;
    let mut encoded = kp.private_hex();
    encoded.push('\n');
    write_key_atomic(path, encoded.as_bytes(), fs_sync)?;
    Ok(kp)
}

/// Loads or creates the daemon's static Noise identity keypair (`<home>/noise.key`).
pub fn load_or_create_server_keypair(
    home: &Path,
    fs_sync: bool,
    rng: &dyn NoiseRngFactory,
) -> Result<NoiseKeypair> {
    load_or_create_keypair(&server_key_path(home), fs_sync, rng)
}

/// Loads or creates a client's static Noise identity keypair (`<data_dir>/client_noise.key`).
pub fn load_or_create_client_keypair(
    data_dir: &Path,
    fs_sync: bool,
    rng: &dyn NoiseRngFactory,
) -> Result<NoiseKeypair> {
    load_or_create_keypair(&client_key_path(data_dir), fs_sync, rng)
}

/// Error returned when the remote daemon's Noise static public key does not match the expected
/// pinned key (from `?pk=` or saved `hosts.json`). Handshake aborts before message 3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyMismatchError {
    pub expected_pubkey: String,
    pub actual_pubkey: String,
    pub expected_fingerprint: String,
    pub actual_fingerprint: String,
}

impl fmt::Display for HostKeyMismatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "host_key_mismatch: Host identity key changed (expected {}, got {}). Verify the host fingerprint and re-pair.",
            self.expected_fingerprint, self.actual_fingerprint
        )
    }
}

impl std::error::Error for HostKeyMismatchError {}

/// Outcome of the server-side `Noise_XX` handshake.
pub enum ServerHandshakeOutcome<S> {
    Established(Box<NoiseStream<S>>),
    RejectedLegacyPlaintext,
    Closed,
}

async fn write_handshake_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
) -> Result<()> {
    ensure!(
        payload.len() <= MAX_HANDSHAKE_MSG_LEN,
        "Noise handshake message exceeds limit ({} bytes)",
        payload.len()
    );
    let len_be = (payload.len() as u16).to_be_bytes();
    writer.write_all(&len_be).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_handshake_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>> {
    let mut len_be = [0u8; 2];
    reader.read_exact(&mut len_be).await?;
    let len = u16::from_be_bytes(len_be) as usize;
    ensure!(
        len > 0 && len <= MAX_HANDSHAKE_MSG_LEN,
        "Invalid Noise handshake message length: {len}"
    );
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Performs the client-side `Noise_XX_25519_ChaChaPoly_BLAKE2s` handshake over `stream`.
///
/// If `expected_server_pubkey` is `Some`, verifies the server's static public key immediately
/// after receiving message 2 — aborting with [`HostKeyMismatchError`] **before** sending
/// message 3 or any application credentials.
pub async fn client_handshake_xx<S: AsyncIo>(
    mut stream: S,
    client_keypair: &NoiseKeypair,
    expected_server_pubkey: Option<&[u8; 32]>,
    rng: &dyn NoiseRngFactory,
) -> Result<NoiseStream<S>> {
    tokio::time::timeout(NOISE_HANDSHAKE_TIMEOUT, async {
        let mut hs = snow::Builder::with_resolver(noise_params(), rng.build_resolver())
            .local_private_key(client_keypair.private_bytes())?
            .build_initiator()
            .context("Failed to initialize Noise_XX initiator")?;

        // Preface + Message 1: -> e
        let mut msg1 = vec![0u8; MAX_HANDSHAKE_MSG_LEN];
        let n1 = hs
            .write_message(&[], &mut msg1)
            .context("Noise_XX write msg1 failed")?;
        msg1.truncate(n1);

        let mut preface_and_msg1 = Vec::with_capacity(NOISE_PREFACE.len() + 2 + msg1.len());
        preface_and_msg1.extend_from_slice(NOISE_PREFACE);
        preface_and_msg1.extend_from_slice(&(msg1.len() as u16).to_be_bytes());
        preface_and_msg1.extend_from_slice(&msg1);
        stream.write_all(&preface_and_msg1).await?;
        stream.flush().await?;

        // Server ack byte (distinguishes Noise_XX daemons from legacy plaintext daemons).
        let mut ack = [0u8; 1];
        if let Err(err) = stream.read_exact(&mut ack).await {
            if err.kind() == io::ErrorKind::UnexpectedEof {
                bail!(
                    "Remote daemon closed connection during Noise E2EE handshake (update the remote daemon)"
                );
            }
            return Err(err.into());
        }
        if ack[0] == b'{' {
            bail!(
                "Remote daemon does not support Noise E2EE transport (update the remote daemon)"
            );
        }
        ensure!(
            ack[0] == NOISE_ACK,
            "Unsupported Noise transport acknowledgement byte: 0x{:02x}",
            ack[0]
        );

        // Message 2: <- e, ee, s, es
        let msg2 = read_handshake_frame(&mut stream)
            .await
            .context("Failed reading Noise_XX handshake message 2")?;
        let mut payload_out = vec![0u8; MAX_HANDSHAKE_MSG_LEN];
        hs.read_message(&msg2, &mut payload_out)
            .context("Noise_XX message 2 authentication failed")?;

        let remote_static: [u8; 32] = hs
            .get_remote_static()
            .context("Noise_XX missing server static key")?
            .try_into()
            .map_err(|_| anyhow!("Invalid server static key length"))?;

        // Strict server identity verification BEFORE sending client static key or pairing code!
        if let Some(expected) = expected_server_pubkey
            && !ct_eq_bytes(&remote_static, expected)
        {
            let _ = stream.shutdown().await;
            return Err(HostKeyMismatchError {
                expected_pubkey: encode_pubkey(expected),
                actual_pubkey: encode_pubkey(&remote_static),
                expected_fingerprint: pubkey_fingerprint(expected),
                actual_fingerprint: pubkey_fingerprint(&remote_static),
            }
            .into());
        }

        // Message 3: -> s, se
        let mut msg3 = vec![0u8; MAX_HANDSHAKE_MSG_LEN];
        let n3 = hs
            .write_message(&[], &mut msg3)
            .context("Noise_XX write msg3 failed")?;
        msg3.truncate(n3);
        write_handshake_frame(&mut stream, &msg3).await?;

        let transport = hs
            .into_transport_mode()
            .context("Failed transitioning Noise_XX initiator to transport mode")?;
        Ok(NoiseStream::new(stream, transport, remote_static))
    })
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "Timed out completing Noise E2EE handshake",
        )
    })?
}

/// Performs the server-side `Noise_XX_25519_ChaChaPoly_BLAKE2s` handshake over `stream`.
///
/// Legacy plaintext clients sending `{...}` receive a clean JSON-RPC `-32002` error and close.
pub async fn server_handshake_xx<S: AsyncIo>(
    mut stream: S,
    server_keypair: &NoiseKeypair,
    rng: &dyn NoiseRngFactory,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<ServerHandshakeOutcome<S>> {
    if *stop_rx.borrow() {
        return Ok(ServerHandshakeOutcome::Closed);
    }

    let first_byte = tokio::select! {
        biased;
        _ = stop_rx.changed() => return Ok(ServerHandshakeOutcome::Closed),
        res = tokio::time::timeout(NOISE_HANDSHAKE_TIMEOUT, async {
            let mut b = [0u8; 1];
            match stream.read(&mut b).await {
                Ok(0) => Ok(None),
                Ok(_) => Ok(Some(b[0])),
                Err(e) => Err(e),
            }
        }) => {
            match res {
                Ok(Ok(Some(b))) => b,
                Ok(Ok(None)) => return Ok(ServerHandshakeOutcome::Closed),
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => return Ok(ServerHandshakeOutcome::Closed),
            }
        }
    };

    if first_byte == b'{' {
        let err_reply = serde_json::json!({
            "jsonrpc": "2.0",
            "id": serde_json::Value::Null,
            "error": {
                "code": -32002,
                "message": "Encryption required: update your Canban client to connect using Noise E2EE."
            }
        });
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            crate::framing::write_json_line_async(&mut stream, &err_reply),
        )
        .await;
        let _ = stream.shutdown().await;
        return Ok(ServerHandshakeOutcome::RejectedLegacyPlaintext);
    }

    let outcome = tokio::select! {
        biased;
        _ = stop_rx.changed() => Ok(ServerHandshakeOutcome::Closed),
        res = tokio::time::timeout(NOISE_HANDSHAKE_TIMEOUT, async {
            let mut rest = [0u8; 3];
            stream.read_exact(&mut rest).await?;
            let preface = [first_byte, rest[0], rest[1], rest[2]];
            ensure!(
                &preface == NOISE_PREFACE,
                "Invalid Noise transport preface"
            );

            let mut hs = snow::Builder::with_resolver(noise_params(), rng.build_resolver())
                .local_private_key(server_keypair.private_bytes())?
                .build_responder()
                .context("Failed to initialize Noise_XX responder")?;

            // Message 1: -> e
            let msg1 = read_handshake_frame(&mut stream)
                .await
                .context("Failed reading Noise_XX handshake message 1")?;
            let mut payload_out = vec![0u8; MAX_HANDSHAKE_MSG_LEN];
            hs.read_message(&msg1, &mut payload_out)
                .context("Noise_XX message 1 failed")?;

            // Ack + Message 2: <- e, ee, s, es
            let mut msg2 = vec![0u8; MAX_HANDSHAKE_MSG_LEN];
            let n2 = hs
                .write_message(&[], &mut msg2)
                .context("Noise_XX write msg2 failed")?;
            msg2.truncate(n2);
            let mut ack_and_msg2 = Vec::with_capacity(1 + 2 + msg2.len());
            ack_and_msg2.push(NOISE_ACK);
            ack_and_msg2.extend_from_slice(&(msg2.len() as u16).to_be_bytes());
            ack_and_msg2.extend_from_slice(&msg2);
            stream.write_all(&ack_and_msg2).await?;
            stream.flush().await?;

            // Message 3: -> s, se
            let msg3 = read_handshake_frame(&mut stream)
                .await
                .context("Failed reading Noise_XX handshake message 3")?;
            hs.read_message(&msg3, &mut payload_out)
                .context("Noise_XX message 3 authentication failed")?;

            let remote_static: [u8; 32] = hs
                .get_remote_static()
                .context("Noise_XX missing client static key")?
                .try_into()
                .map_err(|_| anyhow!("Invalid client static key length"))?;

            let transport = hs
                .into_transport_mode()
                .context("Failed transitioning Noise_XX responder to transport mode")?;
            Ok::<_, anyhow::Error>(ServerHandshakeOutcome::Established(Box::new(
                NoiseStream::new(stream, transport, remote_static),
            )))
        }) => {
            match res {
                Ok(inner) => inner,
                Err(_) => Ok(ServerHandshakeOutcome::Closed),
            }
        }
    };
    outcome
}

/// Encrypted full-duplex async byte stream over a completed `Noise_XX` transport state.
///
/// Wire format after handshake: repeated `[u16 BE cipher_len][AEAD ciphertext (incl. 16B tag)]`.
///
/// Write path is cancel-safe: pending ciphertext is always drained before encrypting a new caller
/// slice, so returning `Poll::Pending` never advances the Noise send nonce for unacknowledged bytes.
pub struct NoiseStream<S> {
    inner: S,
    transport: TransportState,
    remote_static: [u8; 32],
    // Read state machine
    read_hdr: [u8; 2],
    read_hdr_pos: usize,
    read_cipher_buf: Vec<u8>,
    read_cipher_pos: usize,
    read_cipher_target: usize,
    read_plain_buf: Vec<u8>,
    read_plain_pos: usize,
    read_eof: bool,
    // Write state machine
    write_cipher_buf: Vec<u8>,
    write_cipher_pos: usize,
    // Fail-closed poison flag on any AEAD or framing error
    poisoned: bool,
}

impl<S> NoiseStream<S> {
    fn new(inner: S, transport: TransportState, remote_static: [u8; 32]) -> Self {
        Self {
            inner,
            transport,
            remote_static,
            read_hdr: [0u8; 2],
            read_hdr_pos: 0,
            read_cipher_buf: Vec::new(),
            read_cipher_pos: 0,
            read_cipher_target: 0,
            read_plain_buf: Vec::new(),
            read_plain_pos: 0,
            read_eof: false,
            write_cipher_buf: Vec::new(),
            write_cipher_pos: 0,
            poisoned: false,
        }
    }

    /// Returns the authenticated peer's 32-byte X25519 static public key.
    pub fn remote_static(&self) -> &[u8; 32] {
        &self.remote_static
    }

    /// Returns the authenticated peer's base64url-encoded X25519 static public key.
    pub fn remote_pubkey_b64(&self) -> String {
        encode_pubkey(&self.remote_static)
    }

    /// Returns the authenticated peer's short human-readable key fingerprint.
    pub fn remote_fingerprint(&self) -> String {
        pubkey_fingerprint(&self.remote_static)
    }
}

impl<S: AsyncWrite + Unpin> NoiseStream<S> {
    fn poll_drain_write(&mut self, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        while self.write_cipher_pos < self.write_cipher_buf.len() {
            match Pin::new(&mut self.inner)
                .poll_write(cx, &self.write_cipher_buf[self.write_cipher_pos..])
            {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(0)) => {
                    self.poisoned = true;
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "Noise underlying stream returned 0 bytes on write",
                    )));
                }
                Poll::Ready(Ok(n)) => {
                    self.write_cipher_pos += n;
                }
                Poll::Ready(Err(e)) => {
                    self.poisoned = true;
                    return Poll::Ready(Err(e));
                }
            }
        }
        self.write_cipher_buf.clear();
        self.write_cipher_pos = 0;
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for NoiseStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.poisoned {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Noise stream poisoned after cryptographic/framing error",
            )));
        }
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }

        loop {
            // 1. Serve already-decrypted plaintext bytes first.
            if this.read_plain_pos < this.read_plain_buf.len() {
                let avail = &this.read_plain_buf[this.read_plain_pos..];
                let to_copy = avail.len().min(buf.remaining());
                buf.put_slice(&avail[..to_copy]);
                this.read_plain_pos += to_copy;
                if this.read_plain_pos == this.read_plain_buf.len() {
                    this.read_plain_buf.clear();
                    this.read_plain_pos = 0;
                }
                return Poll::Ready(Ok(()));
            }
            if this.read_eof {
                return Poll::Ready(Ok(()));
            }

            // 2. Read the 2-byte big-endian frame length header if we don't have a frame target yet.
            while this.read_cipher_target == 0 && this.read_hdr_pos < 2 {
                let mut hdr_buf = ReadBuf::new(&mut this.read_hdr[this.read_hdr_pos..]);
                match Pin::new(&mut this.inner).poll_read(cx, &mut hdr_buf) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(e)) => {
                        this.poisoned = true;
                        return Poll::Ready(Err(e));
                    }
                    Poll::Ready(Ok(())) => {
                        let n = hdr_buf.filled().len();
                        if n == 0 {
                            if this.read_hdr_pos == 0 {
                                this.read_eof = true;
                                return Poll::Ready(Ok(()));
                            }
                            this.poisoned = true;
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "Truncated Noise frame header",
                            )));
                        }
                        this.read_hdr_pos += n;
                    }
                }
            }
            if this.read_cipher_target == 0 {
                let frame_len = u16::from_be_bytes(this.read_hdr) as usize;
                this.read_hdr_pos = 0;
                if !(NOISE_AEAD_TAG_LEN..=MAX_NOISE_MESSAGE_LEN).contains(&frame_len) {
                    this.poisoned = true;
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Invalid Noise transport frame length: {frame_len}"),
                    )));
                }
                this.read_cipher_target = frame_len;
                this.read_cipher_buf.resize(frame_len, 0);
                this.read_cipher_pos = 0;
            }

            // 3. Read the encrypted frame body until complete.
            while this.read_cipher_pos < this.read_cipher_target {
                let mut body_buf = ReadBuf::new(
                    &mut this.read_cipher_buf[this.read_cipher_pos..this.read_cipher_target],
                );
                match Pin::new(&mut this.inner).poll_read(cx, &mut body_buf) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(e)) => {
                        this.poisoned = true;
                        return Poll::Ready(Err(e));
                    }
                    Poll::Ready(Ok(())) => {
                        let n = body_buf.filled().len();
                        if n == 0 {
                            this.poisoned = true;
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "Truncated Noise encrypted frame",
                            )));
                        }
                        this.read_cipher_pos += n;
                    }
                }
            }

            // 4. Decrypt and authenticate the completed AEAD frame.
            let max_plain = this.read_cipher_target - NOISE_AEAD_TAG_LEN;
            this.read_plain_buf.resize(max_plain, 0);
            let plain_len = match this.transport.read_message(
                &this.read_cipher_buf[..this.read_cipher_target],
                &mut this.read_plain_buf,
            ) {
                Ok(n) => n,
                Err(e) => {
                    this.poisoned = true;
                    this.read_plain_buf.clear();
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Noise AEAD decryption failed: {e}"),
                    )));
                }
            };
            this.read_plain_buf.truncate(plain_len);
            this.read_plain_pos = 0;
            this.read_cipher_target = 0;
            this.read_cipher_pos = 0;
            // Loop to copy decrypted plaintext (or read next frame if a 0-byte frame arrived).
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for NoiseStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.poisoned {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Noise stream poisoned after cryptographic/framing error",
            )));
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        // Cancel-safe: drain any prior encrypted frame BEFORE touching Noise state for `buf`.
        match this.poll_drain_write(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }

        let chunk_len = buf.len().min(MAX_NOISE_CHUNK_PLAINTEXT);
        let max_cipher = chunk_len + NOISE_AEAD_TAG_LEN;
        this.write_cipher_buf.resize(2 + max_cipher, 0);
        let cipher_len = match this
            .transport
            .write_message(&buf[..chunk_len], &mut this.write_cipher_buf[2..])
        {
            Ok(n) => n,
            Err(e) => {
                this.poisoned = true;
                this.write_cipher_buf.clear();
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Noise AEAD encryption failed: {e}"),
                )));
            }
        };
        this.write_cipher_buf[..2].copy_from_slice(&(cipher_len as u16).to_be_bytes());
        this.write_cipher_buf.truncate(2 + cipher_len);
        this.write_cipher_pos = 0;

        // Opportunistically push ciphertext now; any remainder flushes on next poll_write/poll_flush.
        if let Poll::Ready(Err(e)) = this.poll_drain_write(cx) {
            return Poll::Ready(Err(e));
        }
        Poll::Ready(Ok(chunk_len))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.poisoned {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Noise stream poisoned after cryptographic/framing error",
            )));
        }
        match this.poll_drain_write(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.poisoned {
            match this.poll_drain_write(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {}
            }
        }
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}
