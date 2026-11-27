//! Pluggable transport connectors turning a [`HostTransport`] specification into a ready
//! [`ClientTransport`] plus an optional RAII [`ConnectionLease`] (e.g. an SSH tunnel guard).
use anyhow::{Result, bail};
use canban_transport::{
    ClientTransport, NoiseKeypair, NoiseRngFactory, TcpAuthMode, system_noise_rng,
};
use canban_types::HostTransport;
use std::{any::Any, future::Future, pin::Pin, sync::Arc};

/// Opaque RAII lease kept alive for as long as a [`ClientTransport`] remains open
/// (for example, a desktop SSH tunnel subprocess guard that terminates on drop).
#[derive(Default)]
pub struct ConnectionLease {
    _guard: Option<Box<dyn Any + Send + Sync>>,
}

impl std::fmt::Debug for ConnectionLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionLease")
            .field("active", &self._guard.is_some())
            .finish()
    }
}

impl ConnectionLease {
    /// Empty lease for transports with no external process or socket resource to hold.
    pub fn none() -> Self {
        Self { _guard: None }
    }

    /// Wraps a transport-owned resource guard whose `Drop` cleans up the underlying link.
    pub fn from_guard<T: Send + Sync + 'static>(guard: T) -> Self {
        Self {
            _guard: Some(Box::new(guard)),
        }
    }

    /// Returns `true` when this lease holds an underlying resource guard.
    pub fn is_active(&self) -> bool {
        self._guard.is_some()
    }
}

/// Ready-to-use [`ClientTransport`] paired with its optional [`ConnectionLease`].
pub struct ConnectedTransport {
    pub transport: Arc<ClientTransport>,
    pub lease: ConnectionLease,
}

impl ConnectedTransport {
    pub fn new(transport: Arc<ClientTransport>) -> Self {
        Self {
            transport,
            lease: ConnectionLease::none(),
        }
    }

    pub fn with_lease(transport: Arc<ClientTransport>, lease: ConnectionLease) -> Self {
        Self { transport, lease }
    }
}

pub type ConnectFuture<'a> = Pin<Box<dyn Future<Output = Result<ConnectedTransport>> + Send + 'a>>;

/// Establishes a [`ConnectedTransport`] for one or more [`HostTransport`] variants.
pub trait HostConnector: Send + Sync + 'static {
    /// Returns `true` if this connector can establish links for `transport`.
    fn supports(&self, transport: &HostTransport) -> bool;

    /// Connects `host_id` using `transport` and optional one-time `pairing_code`, returning a
    /// ready [`ConnectedTransport`].
    fn connect<'a>(
        &'a self,
        host_id: &'a str,
        transport: &'a HostTransport,
        device_name: &'a str,
        pairing_code: Option<&'a str>,
    ) -> ConnectFuture<'a>;
}

/// Built-in cross-platform TCP connector supporting `Noise_XX` E2EE + `yamux/1`,
/// public-key authentication, one-time pairing codes (`pair_...`), and Tailscale WhoIs auth.
#[derive(Clone)]
pub struct TcpConnector {
    client_keypair: Arc<NoiseKeypair>,
    noise_rng: Arc<dyn NoiseRngFactory>,
}

impl TcpConnector {
    pub fn new(client_keypair: Arc<NoiseKeypair>) -> Self {
        Self {
            client_keypair,
            noise_rng: system_noise_rng(),
        }
    }

    pub fn with_rng(mut self, noise_rng: Arc<dyn NoiseRngFactory>) -> Self {
        self.noise_rng = noise_rng;
        self
    }

    pub fn client_keypair(&self) -> &Arc<NoiseKeypair> {
        &self.client_keypair
    }
}

impl HostConnector for TcpConnector {
    fn supports(&self, transport: &HostTransport) -> bool {
        matches!(transport, HostTransport::Tcp { .. })
    }

    fn connect<'a>(
        &'a self,
        _host_id: &'a str,
        transport: &'a HostTransport,
        device_name: &'a str,
        pairing_code: Option<&'a str>,
    ) -> ConnectFuture<'a> {
        let transport_owned = transport.clone();
        let device = device_name.to_owned();
        let pairing = pairing_code.map(str::to_owned);
        let keypair = self.client_keypair.clone();
        let rng = self.noise_rng.clone();
        Box::pin(async move {
            let HostTransport::Tcp {
                addr,
                server_pubkey,
                tailscale_auth,
            } = transport_owned
            else {
                bail!("TcpConnector only supports TCP hosts");
            };
            let auth = if let Some(code) = pairing.filter(|s| !s.trim().is_empty()) {
                TcpAuthMode::PairingCode(code)
            } else if tailscale_auth {
                TcpAuthMode::Tailscale
            } else {
                TcpAuthMode::Key
            };
            let client = Arc::new(
                ClientTransport::connect_tcp_async(addr, server_pubkey, auth, device, keypair, rng)
                    .await?,
            );
            Ok(ConnectedTransport::new(client))
        })
    }
}

/// Dispatches connection requests across a prioritized list of [`HostConnector`] implementations.
#[derive(Default)]
pub struct CompositeConnector {
    connectors: Vec<Arc<dyn HostConnector>>,
}

impl CompositeConnector {
    pub fn new() -> Self {
        Self {
            connectors: Vec::new(),
        }
    }

    /// Creates a composite connector pre-populated with a [`TcpConnector`] using `client_keypair`.
    pub fn with_tcp(client_keypair: Arc<NoiseKeypair>) -> Self {
        Self {
            connectors: vec![Arc::new(TcpConnector::new(client_keypair))],
        }
    }

    /// Registers an additional connector (evaluated before earlier connectors when prepended,
    /// or after when pushed).
    pub fn push(mut self, connector: Arc<dyn HostConnector>) -> Self {
        self.connectors.push(connector);
        self
    }

    /// Registers a connector at highest priority.
    pub fn prepend(mut self, connector: Arc<dyn HostConnector>) -> Self {
        self.connectors.insert(0, connector);
        self
    }
}

impl HostConnector for CompositeConnector {
    fn supports(&self, transport: &HostTransport) -> bool {
        self.connectors.iter().any(|c| c.supports(transport))
    }

    fn connect<'a>(
        &'a self,
        host_id: &'a str,
        transport: &'a HostTransport,
        device_name: &'a str,
        pairing_code: Option<&'a str>,
    ) -> ConnectFuture<'a> {
        for connector in &self.connectors {
            if connector.supports(transport) {
                return connector.connect(host_id, transport, device_name, pairing_code);
            }
        }
        let kind = match transport {
            HostTransport::Local => "Local Unix socket",
            HostTransport::Tcp { .. } => "TCP",
            HostTransport::Ssh { .. } => "SSH tunnel",
        };
        Box::pin(async move {
            bail!("{kind} transport is not supported by this client's configured connectors")
        })
    }
}
