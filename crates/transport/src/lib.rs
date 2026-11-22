//! Pure transport layer for Canban: framed JSON lines, per-device token authentication,
//! live device revocation enforcement, and single-connection `yamux/1` stream multiplexing.
pub mod auth;
pub mod client;
pub mod framing;
pub mod mux;
pub mod server;

pub use auth::{
    DeviceTokenStore, PairingCodeStore, RevocationBus, TcpAuthOutcome, authenticate_tcp_token,
};
pub use client::{
    ClientEndpoint, ClientTransport, DAEMON_UPDATE_RPC_READ_TIMEOUT, DEFAULT_RPC_READ_TIMEOUT,
    SyncStream, open_mux_bridge, rpc_once, rpc_read_timeout,
};
pub use framing::{
    MAX_FRAME_BYTES, PrefixedStream, TransportStream, read_bounded_line, write_json_line,
    write_json_line_async,
};
pub use server::{AcceptedStream, ServerTransportConfig, spawn_tcp_listener, spawn_unix_listener};
