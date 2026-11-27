//! Shared single-host session handshake (`app.ping` + `app.state`), canonical endpoint parsing,
//! and lightweight one-shot RPC entry points used by CLI, Desktop, and Mobile clients.
use crate::{
    connector::{ConnectedTransport, ConnectionLease, HostConnector},
    pairing::{parse_pairing_invite, parse_tailscale_host_input},
};
use anyhow::Result;
use canban_transport::{
    ClientEndpoint, ClientTransport, NoiseKeypair, TcpAuthMode, load_or_create_client_keypair,
    system_noise_rng,
};
use canban_types::{Clock, HostTransport, is_tailscale_addr, normalize_daemon_addr};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};
use tokio::time::Instant;

/// Canonical endpoint parsing shared across CLI (`CANBAN_ENDPOINT`), Desktop, and Mobile.
///
/// Accepts:
/// - Single-address pairing URIs (`canban://<addr>?pk=<pubkey>[&sid=...&name=...]#pair_...`)
/// - Bare Tailscale endpoints (`host.ts.net`, `100.x.y.z`, `fd7a:115c:a1e0::1`, `canban://host.ts.net`)
/// - Explicit `canban://<addr>?pk=<pubkey>` or `tcp://<addr>?pk=<pubkey>` endpoints
/// - Local Unix socket paths (`~/.canban/app.sock`)
pub fn parse_endpoint(raw: &str) -> ClientEndpoint {
    let trimmed = raw.trim();
    if let Some(invite) = parse_pairing_invite(trimmed) {
        return ClientEndpoint::Tcp {
            addr: invite.addr,
            server_pubkey: invite.server_pubkey,
            auth: match invite.pairing_code {
                Some(code) => TcpAuthMode::PairingCode(code),
                None if invite.tailscale_auth => TcpAuthMode::Tailscale,
                None => TcpAuthMode::Key,
            },
        };
    }
    if let Some(invite) = parse_tailscale_host_input(trimmed) {
        return ClientEndpoint::Tcp {
            addr: invite.addr,
            server_pubkey: None,
            auth: TcpAuthMode::Tailscale,
        };
    }
    if let Some(rest) = trimmed
        .strip_prefix("tcp://")
        .or_else(|| trimmed.strip_prefix("canban://"))
    {
        let (host_part, query_opt) = match rest.split_once('?') {
            Some((h, q)) => (h.trim().trim_end_matches('/'), Some(q)),
            None => (rest.trim().trim_end_matches('/'), None),
        };
        let mut server_pubkey = None;
        if let Some(query) = query_opt {
            for pair in query.split('&') {
                if let Some(("pk", val)) = pair.split_once('=') {
                    let pk = val.trim();
                    if !pk.is_empty() {
                        server_pubkey = Some(pk.to_owned());
                    }
                }
            }
        }
        let addr = normalize_daemon_addr(host_part);
        let auth = if is_tailscale_addr(&addr) && server_pubkey.is_none() {
            TcpAuthMode::Tailscale
        } else {
            TcpAuthMode::Key
        };
        return ClientEndpoint::Tcp {
            addr,
            server_pubkey,
            auth,
        };
    }
    ClientEndpoint::Unix(Path::new(trimmed).to_path_buf())
}

/// Resolves the CLI/desktop client's persistent Noise X25519 keypair from `CANBAN_CLIENT_KEY`,
/// or `<home>/client_noise.key`.
pub fn load_cli_client_keypair(home: Option<&Path>) -> Result<Arc<NoiseKeypair>> {
    if let Ok(hex) = std::env::var("CANBAN_CLIENT_KEY")
        && !hex.trim().is_empty()
    {
        return Ok(Arc::new(NoiseKeypair::from_private_hex(&hex)?));
    }
    let dir = match home {
        Some(h) => h.to_path_buf(),
        None => std::env::var_os("CANBAN_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".canban")))
            .unwrap_or_else(|| std::path::PathBuf::from(".canban")),
    };
    Ok(Arc::new(load_or_create_client_keypair(
        &dir,
        true,
        system_noise_rng().as_ref(),
    )?))
}

/// Fast-path one-shot JSON-RPC call used by CLI commands and short-lived utilities without
/// spawning background multi-host supervisors or offline caches.
pub fn rpc_once(endpoint: &Path, method: &str, params: Value) -> Result<Value> {
    let parsed = parse_endpoint(&endpoint.to_string_lossy());
    let kp = match &parsed {
        ClientEndpoint::Unix(_) => None,
        ClientEndpoint::Tcp { .. } => Some(load_cli_client_keypair(None)?),
    };
    canban_transport::rpc_once(&parsed, kp, method, &params)
}

/// Fast-path one-shot TCP JSON-RPC call over `Noise_XX` to an explicit endpoint.
pub fn rpc_tcp_once(
    addr: &str,
    server_pubkey: Option<&str>,
    auth: TcpAuthMode,
    client_keypair: Arc<NoiseKeypair>,
    method: &str,
    params: Value,
) -> Result<Value> {
    let endpoint = ClientEndpoint::Tcp {
        addr: normalize_daemon_addr(addr),
        server_pubkey: server_pubkey.map(str::to_owned),
        auth,
    };
    canban_transport::rpc_once(&endpoint, Some(client_keypair), method, &params)
}

/// Error returned when a remote daemon's `api_version` is older than the caller's required
/// minimum API version.
#[derive(Clone, Debug)]
pub struct ApiMismatchError {
    pub server_api: u32,
    pub app_api: u32,
    pub version: String,
    pub server_id: Option<String>,
    pub ping: Value,
}

impl std::fmt::Display for ApiMismatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The remote backend is older than this App (backend API {}, App API {}).",
            self.server_api, self.app_api
        )
    }
}

impl std::error::Error for ApiMismatchError {}

/// Verified server identity and initial workspace state returned by the canonical handshake.
#[derive(Clone, Debug)]
pub struct HandshakeInfo {
    pub ping: Value,
    pub state: Value,
    pub server_id: Option<String>,
    pub server_pubkey: Option<String>,
    pub hostname: Option<String>,
    pub home: String,
    pub rtt_ms: u64,
}

/// Authenticated single-host client session built on [`ClientTransport`] + [`ConnectionLease`].
pub struct SingleHostSession {
    transport: Arc<ClientTransport>,
    lease: ConnectionLease,
    handshake: HandshakeInfo,
}

impl SingleHostSession {
    /// Connects to `transport` through `connector` and completes the canonical
    /// `app.ping` + `app.state` handshake.
    pub async fn connect_async(
        connector: &dyn HostConnector,
        host_id: &str,
        transport: &HostTransport,
        device_name: &str,
        pairing_code: Option<&str>,
        after: Value,
    ) -> Result<Self> {
        Self::connect_checked_async(
            connector,
            host_id,
            transport,
            device_name,
            pairing_code,
            after,
            None,
        )
        .await
    }

    /// Connects to `transport` through `connector`, enforces `min_api_version` (returning
    /// [`ApiMismatchError`] before issuing `app.state` when the remote daemon is too old), and
    /// completes the canonical `app.ping` + `app.state` handshake.
    #[allow(clippy::too_many_arguments)]
    pub async fn connect_checked_async(
        connector: &dyn HostConnector,
        host_id: &str,
        transport: &HostTransport,
        device_name: &str,
        pairing_code: Option<&str>,
        after: Value,
        min_api_version: Option<u32>,
    ) -> Result<Self> {
        let started = tokio::time::Instant::now();
        let connected = connector
            .connect(host_id, transport, device_name, pairing_code)
            .await?;
        Self::handshake_checked_async(connected, after, started, min_api_version).await
    }

    /// Clock-injected variant of [`Self::connect_checked_async`] that measures handshake RTT
    /// through `clock` instead of ambient clocks.
    #[allow(clippy::too_many_arguments)]
    pub async fn connect_checked_async_with_clock(
        connector: &dyn HostConnector,
        host_id: &str,
        transport: &HostTransport,
        device_name: &str,
        pairing_code: Option<&str>,
        after: Value,
        min_api_version: Option<u32>,
        clock: &dyn Clock,
    ) -> Result<Self> {
        let started_ms = clock.now_ms();
        let connected = connector
            .connect(host_id, transport, device_name, pairing_code)
            .await?;
        Self::handshake_checked_async_with_clock(
            connected,
            after,
            started_ms,
            min_api_version,
            clock,
        )
        .await
    }

    /// Runs the canonical `app.ping` + `app.state` handshake over an already-connected transport.
    pub async fn handshake_async(
        connected: ConnectedTransport,
        after: Value,
        started: Instant,
    ) -> Result<Self> {
        Self::handshake_checked_async(connected, after, started, None).await
    }

    /// Runs the canonical `app.ping` + `app.state` handshake with optional minimum API version
    /// enforcement.
    pub async fn handshake_checked_async(
        connected: ConnectedTransport,
        after: Value,
        started: Instant,
        min_api_version: Option<u32>,
    ) -> Result<Self> {
        let (transport, lease, ping, state) =
            Self::handshake_exchange_async(connected, after, min_api_version).await?;
        let rtt_ms = started.elapsed().as_millis() as u64;
        let remote_pk = transport.remote_pubkey().map(str::to_owned);
        let handshake = extract_handshake_info(ping, state, remote_pk, rtt_ms);
        Ok(Self {
            transport,
            lease,
            handshake,
        })
    }

    /// Clock-injected variant of [`Self::handshake_checked_async`].
    pub async fn handshake_checked_async_with_clock(
        connected: ConnectedTransport,
        after: Value,
        started_ms: u64,
        min_api_version: Option<u32>,
        clock: &dyn Clock,
    ) -> Result<Self> {
        let (transport, lease, ping, state) =
            Self::handshake_exchange_async(connected, after, min_api_version).await?;
        let rtt_ms = clock.now_ms().saturating_sub(started_ms);
        let remote_pk = transport.remote_pubkey().map(str::to_owned);
        let handshake = extract_handshake_info(ping, state, remote_pk, rtt_ms);
        Ok(Self {
            transport,
            lease,
            handshake,
        })
    }

    async fn handshake_exchange_async(
        connected: ConnectedTransport,
        after: Value,
        min_api_version: Option<u32>,
    ) -> Result<(Arc<ClientTransport>, ConnectionLease, Value, Value)> {
        let transport = connected.transport;
        let ping = match transport.call_async("app.ping", &json!({})).await {
            Ok(p) => p,
            Err(e) => {
                transport.close();
                return Err(e);
            }
        };
        if let Some(app_api) = min_api_version {
            let server_api = ping["api_version"].as_u64().unwrap_or(0) as u32;
            if server_api < app_api {
                transport.close();
                let version = ping["build"]["version"]
                    .as_str()
                    .or_else(|| ping["version"].as_str())
                    .unwrap_or("unknown")
                    .to_owned();
                let server_id = ping["server_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                return Err(ApiMismatchError {
                    server_api,
                    app_api,
                    version,
                    server_id,
                    ping,
                }
                .into());
            }
        }
        let state = match transport
            .call_async("app.state", &json!({ "after": after }))
            .await
        {
            Ok(s) => s,
            Err(e) => {
                transport.close();
                return Err(e);
            }
        };
        Ok((transport, connected.lease, ping, state))
    }

    /// Synchronous handshake helper for blocking desktop/CLI callers.
    pub fn handshake_sync(connected: ConnectedTransport, after: Value) -> Result<Self> {
        Self::handshake_sync_with_clock(connected, after, &canban_types::SystemClock)
    }

    /// Clock-injected variant of [`Self::handshake_sync`] that measures handshake RTT
    /// through `clock` instead of ambient clocks.
    pub fn handshake_sync_with_clock(
        connected: ConnectedTransport,
        after: Value,
        clock: &dyn Clock,
    ) -> Result<Self> {
        let started_ms = clock.now_ms();
        let transport = connected.transport;
        let ping = transport
            .call("app.ping", &json!({}))
            .inspect_err(|_| transport.close())?;
        let state = transport
            .call("app.state", &json!({ "after": after }))
            .inspect_err(|_| transport.close())?;
        let rtt_ms = clock.now_ms().saturating_sub(started_ms);
        let remote_pk = transport.remote_pubkey().map(str::to_owned);
        let handshake = extract_handshake_info(ping, state, remote_pk, rtt_ms);
        Ok(Self {
            transport,
            lease: connected.lease,
            handshake,
        })
    }

    pub fn transport(&self) -> &Arc<ClientTransport> {
        &self.transport
    }

    pub fn handshake(&self) -> &HandshakeInfo {
        &self.handshake
    }

    pub fn server_id(&self) -> Option<&str> {
        self.handshake.server_id.as_deref()
    }

    pub fn server_pubkey(&self) -> Option<&str> {
        self.handshake.server_pubkey.as_deref()
    }

    pub fn call(&self, method: &str, params: &Value) -> Result<Value> {
        self.transport.call(method, params)
    }

    pub async fn call_async(&self, method: &str, params: &Value) -> Result<Value> {
        self.transport.call_async(method, params).await
    }

    pub fn close(&self) {
        self.transport.close();
    }

    /// Consumes the session and returns its transport, lifecycle lease, and handshake metadata.
    pub fn into_parts(self) -> (Arc<ClientTransport>, ConnectionLease, HandshakeInfo) {
        (self.transport, self.lease, self.handshake)
    }
}

fn extract_handshake_info(
    ping: Value,
    state: Value,
    remote_pubkey: Option<String>,
    rtt_ms: u64,
) -> HandshakeInfo {
    let server_id = state["server_id"]
        .as_str()
        .or_else(|| ping["server_id"].as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned);
    let server_pubkey = remote_pubkey.or_else(|| {
        ping["server_pubkey"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
    });
    let hostname = ping["hostname"]
        .as_str()
        .or_else(|| state["hostname"].as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned);
    let home = state["home"]
        .as_str()
        .or_else(|| ping["home"].as_str())
        .unwrap_or_default()
        .to_owned();
    HandshakeInfo {
        ping,
        state,
        server_id,
        server_pubkey,
        hostname,
        home,
        rtt_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_endpoint_handles_pairing_uri_bare_tailscale_tcp_and_unix() {
        let pk = NoiseKeypair::generate().unwrap().public_b64();
        assert_eq!(
            parse_endpoint(&format!(
                "canban://100.84.12.5:7680?pk={pk}&name=Dev#pair_0123456789abcdef"
            )),
            ClientEndpoint::Tcp {
                addr: "100.84.12.5:7680".into(),
                server_pubkey: Some(pk.clone()),
                auth: TcpAuthMode::PairingCode("pair_0123456789abcdef".into()),
            }
        );
        assert_eq!(
            parse_endpoint("devpc.tailc8912.ts.net"),
            ClientEndpoint::Tcp {
                addr: "devpc.tailc8912.ts.net:7680".into(),
                server_pubkey: None,
                auth: TcpAuthMode::Tailscale,
            }
        );
        assert_eq!(
            parse_endpoint("100.127.37.78"),
            ClientEndpoint::Tcp {
                addr: "100.127.37.78:7680".into(),
                server_pubkey: None,
                auth: TcpAuthMode::Tailscale,
            }
        );
        assert_eq!(
            parse_endpoint(&format!("tcp://192.168.1.20:7680?pk={pk}")),
            ClientEndpoint::Tcp {
                addr: "192.168.1.20:7680".into(),
                server_pubkey: Some(pk),
                auth: TcpAuthMode::Key,
            }
        );
        assert_eq!(
            parse_endpoint("/tmp/canban/app.sock"),
            ClientEndpoint::Unix(Path::new("/tmp/canban/app.sock").to_path_buf())
        );
    }
}
