//! Shared Canban client core built on `canban-transport` (`yamux/1`, `ts_auto`, per-device tokens),
//! pluggable [`HostConnector`] transport adapters, [`SingleHostSession`] for lightweight CLI calls,
//! [`MultiHostClient`] (aliased as [`RemoteClient`]) for supervised multi-host UIs, runtime
//! [`ClientCapabilities`], and `canban-sync` offline synchronization.
pub mod capabilities;
pub mod connector;
pub mod multi_host;
pub mod pairing;
pub mod push;
pub mod session;
pub mod store;

pub use capabilities::{
    ClientCapabilities, GlobalEnvSyncProvider, LocalDiscoveryProvider, LocalHostDriver,
    SshCapabilityProvider, TailscalePeerConnectResult,
};
pub use connector::{
    CompositeConnector, ConnectFuture, ConnectedTransport, ConnectionLease, HostConnector,
    TcpConnector,
};
pub use multi_host::{
    ClientEvent, MultiHostClient, MultiHostClientOptions, RemoteClient, RemoteClientConfig,
};
pub use pairing::{
    AddrProbeResult, PairingInvite, TransportSecurity, format_pairing_uri, is_tailscale_ip,
    parse_pairing_invite, parse_tailscale_host_input, probe_addr, probe_addr_blocking,
};
pub use session::{
    ApiMismatchError, HandshakeInfo, SingleHostSession, parse_endpoint, rpc_once, rpc_tcp_once,
};
pub use store::{
    CredentialHostStore, CredentialStore, FileCredentialStore, HostStore, InlineHostsFileStore,
    MemoryCredentialStore,
};
