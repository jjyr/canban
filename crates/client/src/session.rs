//! Shared single-host session handshake (`app.ping` + `app.state`), canonical endpoint parsing,
//! and lightweight one-shot RPC entry points used by CLI, Desktop, and Mobile clients.
use crate::{
    connector::{ConnectedTransport, ConnectionLease, HostConnector},
    pairing::{parse_pairing_invite, parse_tailscale_host_input},
};
use anyhow::Result;
use canban_transport::{ClientEndpoint, ClientTransport};
use canban_types::{HostTransport, normalize_daemon_addr, parse_pairing_uri};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Instant};

/// Canonical endpoint parsing shared across CLI (`CANBAN_ENDPOINT`), Desktop, and Mobile.
///
/// Accepts:
/// - Single-address pairing URIs (`canban://<addr>[?sid=...&name=...]#<secret>`, `tcp://<addr>#<secret>`, `<addr>#<secret>`)
/// - Bare Tailscale endpoints (`host.ts.net`, `100.x.y.z`, `fd7a:115c:a1e0::1`, `canban://host.ts.net`) mapped to `ts_auto`
/// - Explicit `tcp://<addr>` endpoints with optional `env_token` (`CANBAN_TOKEN`)
/// - Local Unix socket paths (`~/.canban/app.sock`)
pub fn parse_endpoint(raw: &str, env_token: Option<&str>) -> ClientEndpoint {
    let trimmed = raw.trim();
    if let Some(invite) = parse_pairing_invite(trimmed) {
        return ClientEndpoint::Tcp {
            addr: invite.addr,
            token: invite.secret,
        };
    }
    if let Some((addr, token)) = parse_pairing_uri(trimmed) {
        return ClientEndpoint::Tcp { addr, token };
    }
    if let Some(invite) = parse_tailscale_host_input(trimmed) {
        return ClientEndpoint::Tcp {
            addr: invite.addr,
            token: invite.secret,
        };
    }
    if let Some(addr) = trimmed.strip_prefix("tcp://") {
        let token = env_token
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_default()
            .to_owned();
        return ClientEndpoint::Tcp {
            addr: normalize_daemon_addr(addr),
            token,
        };
    }
    ClientEndpoint::Unix(Path::new(trimmed).to_path_buf())
}

/// Fast-path one-shot JSON-RPC call used by CLI commands and short-lived utilities without
/// spawning background multi-host supervisors or offline caches.
pub fn rpc_once(endpoint: &Path, method: &str, params: Value) -> Result<Value> {
    let env_token = std::env::var("CANBAN_TOKEN").ok();
    let parsed = parse_endpoint(&endpoint.to_string_lossy(), env_token.as_deref());
    canban_transport::rpc_once(&parsed, method, &params)
}

/// Fast-path one-shot TCP JSON-RPC call to an explicit `addr` + optional auth token.
pub fn rpc_tcp_once(addr: &str, token: Option<&str>, method: &str, params: Value) -> Result<Value> {
    let endpoint = ClientEndpoint::Tcp {
        addr: normalize_daemon_addr(addr),
        token: token.unwrap_or_default().to_owned(),
    };
    canban_transport::rpc_once(&endpoint, method, &params)
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
        after: Value,
    ) -> Result<Self> {
        Self::connect_checked_async(connector, host_id, transport, device_name, after, None).await
    }

    /// Connects to `transport` through `connector`, enforces `min_api_version` (returning
    /// [`ApiMismatchError`] before issuing `app.state` when the remote daemon is too old), and
    /// completes the canonical `app.ping` + `app.state` handshake.
    pub async fn connect_checked_async(
        connector: &dyn HostConnector,
        host_id: &str,
        transport: &HostTransport,
        device_name: &str,
        after: Value,
        min_api_version: Option<u32>,
    ) -> Result<Self> {
        let started = Instant::now();
        let connected = connector.connect(host_id, transport, device_name).await?;
        Self::handshake_checked_async(connected, after, started, min_api_version).await
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
        let rtt_ms = started.elapsed().as_millis() as u64;
        let handshake = extract_handshake_info(ping, state, rtt_ms);
        Ok(Self {
            transport,
            lease: connected.lease,
            handshake,
        })
    }

    /// Synchronous handshake helper for blocking desktop/CLI callers.
    pub fn handshake_sync(connected: ConnectedTransport, after: Value) -> Result<Self> {
        let started = Instant::now();
        let transport = connected.transport;
        let ping = transport
            .call("app.ping", &json!({}))
            .inspect_err(|_| transport.close())?;
        let state = transport
            .call("app.state", &json!({ "after": after }))
            .inspect_err(|_| transport.close())?;
        let rtt_ms = started.elapsed().as_millis() as u64;
        let handshake = extract_handshake_info(ping, state, rtt_ms);
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

    pub fn upgraded_token(&self) -> Option<&str> {
        self.transport.upgraded_token()
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

fn extract_handshake_info(ping: Value, state: Value, rtt_ms: u64) -> HandshakeInfo {
    let server_id = state["server_id"]
        .as_str()
        .or_else(|| ping["server_id"].as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned);
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
        hostname,
        home,
        rtt_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::TAILSCALE_AUTH_SENTINEL;

    #[test]
    fn parse_endpoint_handles_pairing_uri_bare_tailscale_tcp_and_unix() {
        assert_eq!(
            parse_endpoint(
                "canban://100.84.12.5:7680?name=Dev#pair_0123456789abcdef",
                None
            ),
            ClientEndpoint::Tcp {
                addr: "100.84.12.5:7680".into(),
                token: "pair_0123456789abcdef".into(),
            }
        );
        assert_eq!(
            parse_endpoint("devpc.tailc8912.ts.net", None),
            ClientEndpoint::Tcp {
                addr: "devpc.tailc8912.ts.net:7680".into(),
                token: TAILSCALE_AUTH_SENTINEL.into(),
            }
        );
        assert_eq!(
            parse_endpoint("100.127.37.78", None),
            ClientEndpoint::Tcp {
                addr: "100.127.37.78:7680".into(),
                token: TAILSCALE_AUTH_SENTINEL.into(),
            }
        );
        assert_eq!(
            parse_endpoint("tcp://192.168.1.20:7680", Some("dev_secret_token")),
            ClientEndpoint::Tcp {
                addr: "192.168.1.20:7680".into(),
                token: "dev_secret_token".into(),
            }
        );
        assert_eq!(
            parse_endpoint("/tmp/canban/app.sock", None),
            ClientEndpoint::Unix(Path::new("/tmp/canban/app.sock").to_path_buf())
        );
    }
}
