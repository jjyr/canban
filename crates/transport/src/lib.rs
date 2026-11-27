//! Pure transport layer for Canban: framed JSON lines, `Noise_XX` E2EE encryption,
//! public-key device authentication, live device revocation enforcement, and
//! single-connection `yamux/1` stream multiplexing.
pub mod auth;
pub mod client;
pub mod framing;
pub mod mux;
pub mod noise;
pub mod server;

pub use auth::{
    AuthorizedDeviceStore, NoiseAuthOutcome, PairingCodeStore, RevocationBus,
    TailscalePeerIdentity, TailscalePeerVerifier, TailscaleSessionRegistry,
    authenticate_noise_session,
};
pub use client::{
    ClientEndpoint, ClientTransport, DAEMON_UPDATE_RPC_READ_TIMEOUT, DEFAULT_RPC_READ_TIMEOUT,
    SyncStream, TcpAuthMode, open_mux_bridge, rpc_once, rpc_read_timeout,
};
pub use framing::{
    AsyncIo, MAX_FRAME_BYTES, PrefixedStream, TransportStream, read_bounded_line, write_json_line,
    write_json_line_async,
};
pub use noise::{
    DeterministicNoiseRng, HostKeyMismatchError, MAX_NOISE_CHUNK_PLAINTEXT, MAX_NOISE_MESSAGE_LEN,
    NOISE_ACK, NOISE_AEAD_TAG_LEN, NOISE_HANDSHAKE_TIMEOUT, NOISE_PATTERN, NOISE_PREFACE,
    NoiseKeypair, NoiseRngFactory, NoiseStream, ServerHandshakeOutcome, SystemNoiseRng,
    client_handshake_xx, client_key_path, decode_pubkey, encode_pubkey,
    load_or_create_client_keypair, load_or_create_keypair, load_or_create_server_keypair,
    pubkey_b64_fingerprint, pubkey_fingerprint, server_handshake_xx, server_key_path,
    system_noise_rng,
};
pub use server::{
    AcceptedStream, ServerTransportConfig, serve_authenticated_stream, spawn_tcp_listener,
    spawn_unix_listener,
};
