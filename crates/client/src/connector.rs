//! Pluggable transport connectors turning a [`HostTransport`] specification into a ready
//! [`ClientTransport`] plus an optional RAII [`ConnectionLease`] (e.g. an SSH tunnel guard).
use anyhow::{Result, bail};
use canban_transport::ClientTransport;
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

    /// Connects `host_id` using `transport` and returns a ready [`ConnectedTransport`].
    fn connect<'a>(
        &'a self,
        host_id: &'a str,
        transport: &'a HostTransport,
        device_name: &'a str,
    ) -> ConnectFuture<'a>;
}

/// Built-in cross-platform TCP connector supporting `yamux/1`, Tailscale WhoIs (`ts_auto`),
/// one-time pairing codes (`pair_...`), and per-device tokens (`dev_...`).
#[derive(Clone, Copy, Debug, Default)]
pub struct TcpConnector;

impl HostConnector for TcpConnector {
    fn supports(&self, transport: &HostTransport) -> bool {
        matches!(transport, HostTransport::Tcp { .. })
    }

    fn connect<'a>(
        &'a self,
        _host_id: &'a str,
        transport: &'a HostTransport,
        device_name: &'a str,
    ) -> ConnectFuture<'a> {
        let transport_owned = transport.clone();
        let device = device_name.to_owned();
        Box::pin(async move {
            let HostTransport::Tcp { addr, token } = transport_owned else {
                bail!("TcpConnector only supports TCP hosts");
            };
            let client = Arc::new(ClientTransport::connect_tcp_async(addr, token, device).await?);
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

    /// Creates a composite connector pre-populated with the built-in [`TcpConnector`].
    pub fn with_default_tcp() -> Self {
        Self {
            connectors: vec![Arc::new(TcpConnector)],
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
    ) -> ConnectFuture<'a> {
        for connector in &self.connectors {
            if connector.supports(transport) {
                return connector.connect(host_id, transport, device_name);
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
