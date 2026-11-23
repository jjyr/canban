//! Shared pure-remote Canban client built on Tokio async push streams (`canban-transport`),
//! offline sync + read cache (`canban-sync`), multi-address pairing, and remote PTY transport
//! (`canban-terminal`), with zero dependency on `canban-app-server`, SQLite, or SSH subprocesses.
pub mod pairing;
pub mod push;
pub mod remote_client;
pub mod store;

pub use pairing::{
    AddressSecurity, CandidateProbeResult, PairingInvite, address_priority, format_pairing_uri_v2,
    is_tailscale_ip, parse_pairing_invite, probe_candidates, select_preferred_candidate,
};
pub use remote_client::{ClientEvent, RemoteClient, RemoteClientConfig};
pub use store::{CredentialStore, FileCredentialStore, MemoryCredentialStore};
