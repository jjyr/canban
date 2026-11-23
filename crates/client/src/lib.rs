//! Shared pure-remote Canban client built on Tokio async push streams (`canban-transport`),
//! offline sync + read cache (`canban-sync`), single-address pairing, and remote PTY transport
//! (`canban-terminal`), with zero dependency on `canban-app-server`, SQLite, or SSH subprocesses.
pub mod pairing;
pub mod push;
pub mod remote_client;
pub mod store;

pub use pairing::{
    AddrProbeResult, PairingInvite, TransportSecurity, format_pairing_uri, is_tailscale_ip,
    parse_pairing_invite, probe_addr,
};
pub use remote_client::{ClientEvent, RemoteClient, RemoteClientConfig};
pub use store::{CredentialStore, FileCredentialStore, MemoryCredentialStore};
