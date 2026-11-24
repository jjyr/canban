//! Client-side transport management over local Unix sockets, SSH-forwarded Unix sockets,
//! and single-connection `yamux/1` multiplexed TCP links (with fallback to direct TCP on
//! legacy daemons), plus a synchronous `mux://` socketpair bridge.
use crate::{
    framing::{
        MAX_FRAME_BYTES, PrefixedStream, TransportStream, read_bounded_line, write_json_line,
        write_json_line_async,
    },
    mux::{MuxClient, connect_mux},
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    future::Future,
    io::{self, BufRead, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientEndpoint {
    Unix(PathBuf),
    Tcp { addr: String, token: String },
}

pub const DEFAULT_RPC_READ_TIMEOUT: Duration = Duration::from_secs(90);
pub const DAEMON_UPDATE_RPC_READ_TIMEOUT: Duration = Duration::from_secs(20 * 60);
pub const PROJECT_BUNDLE_RPC_READ_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub fn rpc_read_timeout(method: &str) -> Duration {
    match method {
        "update.daemon" => DAEMON_UPDATE_RPC_READ_TIMEOUT,
        "project.export" | "project.import" | "project.archive" | "project.restore" => {
            PROJECT_BUNDLE_RPC_READ_TIMEOUT
        }
        _ => DEFAULT_RPC_READ_TIMEOUT,
    }
}

trait SyncRpcSocket: Read + Write + Send + Sized + 'static {
    fn try_clone_socket(&self) -> io::Result<Self>;
    fn set_socket_read_timeout(&self, dur: Option<Duration>) -> io::Result<()>;
    fn set_socket_write_timeout(&self, dur: Option<Duration>) -> io::Result<()>;
}

impl SyncRpcSocket for UnixStream {
    fn try_clone_socket(&self) -> io::Result<Self> {
        self.try_clone()
    }

    fn set_socket_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(dur)
    }

    fn set_socket_write_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.set_write_timeout(dur)
    }
}

impl SyncRpcSocket for TcpStream {
    fn try_clone_socket(&self) -> io::Result<Self> {
        self.try_clone()
    }

    fn set_socket_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(dur)
    }

    fn set_socket_write_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.set_write_timeout(dur)
    }
}

fn connect_unix_sync(
    path: &Path,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
) -> Result<UnixStream> {
    let stream = UnixStream::connect(path)
        .with_context(|| format!("Canban is not running at {}", path.display()))?;
    stream.set_read_timeout(read_timeout)?;
    stream.set_write_timeout(write_timeout)?;
    Ok(stream)
}

fn connect_tcp_sync(
    addr: &str,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
) -> Result<TcpStream> {
    let sock_addr = addr
        .to_socket_addrs()
        .with_context(|| format!("Invalid Canban TCP address {addr}"))?
        .next()
        .with_context(|| format!("Could not resolve Canban TCP address {addr}"))?;
    let stream = TcpStream::connect_timeout(&sock_addr, Duration::from_secs(10))
        .with_context(|| format!("Canban is not reachable at {addr}"))?;
    let _ = stream.set_nodelay(true);
    stream.set_read_timeout(read_timeout)?;
    stream.set_write_timeout(write_timeout)?;
    Ok(stream)
}

fn send_and_read_rpc<S: SyncRpcSocket>(
    writer: &mut S,
    reader: &mut BufReader<S>,
    req: &Value,
    read_timeout: Duration,
    closed_label: &str,
) -> Result<Value> {
    write_json_line(writer, req)?;
    let extended_timeout = read_timeout != DEFAULT_RPC_READ_TIMEOUT;
    if extended_timeout {
        writer.set_socket_read_timeout(Some(read_timeout))?;
        reader
            .get_ref()
            .set_socket_read_timeout(Some(read_timeout))?;
    }
    let mut line = String::new();
    let read_res = reader.read_line(&mut line);
    if extended_timeout {
        let _ = writer.set_socket_read_timeout(Some(DEFAULT_RPC_READ_TIMEOUT));
        let _ = reader
            .get_ref()
            .set_socket_read_timeout(Some(DEFAULT_RPC_READ_TIMEOUT));
    }
    let n = read_res?;
    if n == 0 || !line.ends_with('\n') {
        let msg = if n == 0 {
            closed_label.to_owned()
        } else {
            format!("{closed_label} mid-frame")
        };
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, msg).into());
    }
    decode_rpc_reply(&line)
}

enum SyncClientStream {
    Unix {
        writer: UnixStream,
        reader: BufReader<UnixStream>,
    },
    Tcp {
        writer: TcpStream,
        reader: BufReader<TcpStream>,
        token: String,
    },
}

struct RpcConn {
    stream: SyncClientStream,
    next_id: u64,
}

impl RpcConn {
    fn connect(endpoint: &ClientEndpoint) -> Result<Self> {
        let rw = Some(DEFAULT_RPC_READ_TIMEOUT);
        let stream = match endpoint {
            ClientEndpoint::Unix(path) => {
                let writer = connect_unix_sync(path, rw, rw)?;
                let reader = BufReader::new(writer.try_clone()?);
                SyncClientStream::Unix { writer, reader }
            }
            ClientEndpoint::Tcp { addr, token } => {
                let writer = connect_tcp_sync(addr, rw, rw)?;
                let reader = BufReader::new(writer.try_clone()?);
                SyncClientStream::Tcp {
                    writer,
                    reader,
                    token: token.clone(),
                }
            }
        };
        Ok(Self { stream, next_id: 1 })
    }

    fn call(&mut self, method: &str, params: &Value) -> Result<Value> {
        let req_id = self.next_id;
        self.next_id += 1;
        let read_timeout = rpc_read_timeout(method);
        match &mut self.stream {
            SyncClientStream::Unix { writer, reader } => send_and_read_rpc(
                writer,
                reader,
                &json!({"jsonrpc":"2.0","id":req_id,"method":method,"params":params}),
                read_timeout,
                "AppServer closed RPC socket",
            ),
            SyncClientStream::Tcp {
                writer,
                reader,
                token,
            } => send_and_read_rpc(
                writer,
                reader,
                &json!({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "method": method,
                    "params": params,
                    "auth": token
                }),
                read_timeout,
                "Remote AppServer closed TCP socket",
            ),
        }
    }
}

fn decode_rpc_reply(line: &str) -> Result<Value> {
    let mut reply: Value = serde_json::from_str(line)?;
    if let Some(err) = reply.get("error") {
        let code = err["code"].as_i64().unwrap_or(-32000);
        let msg = err["message"].as_str().unwrap_or("Canban RPC error");
        if code == -32001 {
            bail!("auth_failed: {msg}");
        }
        bail!("{msg}");
    }
    reply
        .get_mut("result")
        .map(Value::take)
        .context("Invalid Canban response")
}

pub fn rpc_once(endpoint: &ClientEndpoint, method: &str, params: &Value) -> Result<Value> {
    let mut conn = RpcConn::connect(endpoint)?;
    conn.call(method, params)
}

fn shared_client_runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("canban-client-mux")
            .build()
            .expect("build shared client mux Tokio runtime")
    })
}

fn run_on_client_rt<F, T>(fut: F) -> T
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::sync_channel(1);
    shared_client_runtime().spawn(async move {
        let _ = tx.send(fut.await);
    });
    rx.recv().expect("client mux task completed")
}

type MuxRelayMap = Mutex<HashMap<String, Arc<MuxClient>>>;

fn mux_relay_registry() -> &'static MuxRelayMap {
    static REGISTRY: OnceLock<MuxRelayMap> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

fn open_bridged_mux_unix(mux: &Arc<MuxClient>) -> Result<UnixStream> {
    let mux_clone = mux.clone();
    let mut mux_stream = run_on_client_rt(async move { mux_clone.open_stream().await })?;
    let (app_side, bridge_side) = UnixStream::pair()
        .context("Could not create local socketpair for multiplexed stream bridge")?;
    bridge_side.set_nonblocking(true)?;
    shared_client_runtime().spawn(async move {
        if let Ok(mut tokio_bridge) = tokio::net::UnixStream::from_std(bridge_side) {
            let _ = tokio::io::copy_bidirectional(&mut tokio_bridge, &mut mux_stream).await;
        }
    });
    Ok(app_side)
}

/// Opens a synchronous OS `UnixStream` bridged to a logical sub-stream on the
/// registered `yamux/1` connection identified by `key` (`mux://<key>`).
pub fn open_mux_bridge(key: &str) -> Result<UnixStream> {
    let mux = mux_relay_registry()
        .lock()
        .unwrap()
        .get(key)
        .cloned()
        .context("Multiplexed host connection is not active")?;
    open_bridged_mux_unix(&mux)
}

pub struct SyncStream {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub auth_token: Option<String>,
}

fn into_sync_stream<S: SyncRpcSocket>(
    writer: S,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
    auth_token: Option<String>,
) -> io::Result<SyncStream> {
    writer.set_socket_read_timeout(read_timeout)?;
    writer.set_socket_write_timeout(write_timeout)?;
    let reader = writer.try_clone_socket()?;
    Ok(SyncStream {
        reader: Box::new(reader),
        writer: Box::new(writer),
        auth_token,
    })
}

struct MuxConnectResult {
    mux: Arc<MuxClient>,
    upgraded_token: Option<String>,
    session_token: Option<String>,
}

async fn try_connect_mux(
    addr: String,
    token: String,
    device_name: String,
) -> Result<Option<MuxConnectResult>> {
    const HANDSHAKE_MAX_BYTES: usize = 64 * 1024;
    let tcp = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("Timed out connecting to {addr}"),
        )
    })?
    .with_context(|| format!("Canban is not reachable at {addr}"))?;
    let _ = tcp.set_nodelay(true);

    let (read_half, mut writer) = tokio::io::split(tcp);
    let mut reader = tokio::io::BufReader::new(read_half);
    let req = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "transport.mux",
        "auth": token,
        "params": {
            "protocol": "yamux/1",
            "device_name": device_name,
        },
    });
    tokio::time::timeout(
        Duration::from_secs(10),
        write_json_line_async(&mut writer, &req),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "Timed out sending transport.mux request",
        )
    })??;

    let line_opt = tokio::time::timeout(
        Duration::from_secs(10),
        read_bounded_line(&mut reader, HANDSHAKE_MAX_BYTES),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "Timed out waiting for transport.mux reply",
        )
    })??;
    let Some(line) = line_opt else {
        return Ok(None);
    };
    let Ok(reply) = serde_json::from_str::<Value>(&line) else {
        return Ok(None);
    };
    if let Some(err) = reply.get("error") {
        let code = err["code"].as_i64().unwrap_or(-32000);
        let msg = err["message"].as_str().unwrap_or("Canban RPC error");
        if code == -32001 {
            bail!("auth_failed: {msg}");
        }
        return Ok(None);
    }
    let result = &reply["result"];
    if result["ready"] != true {
        return Ok(None);
    }
    let upgraded_token = result["upgraded_token"].as_str().map(str::to_owned);
    let session_token = result["session_token"].as_str().map(str::to_owned);
    let leftover = reader.buffer().to_vec();
    let tcp = reader.into_inner().unsplit(writer);
    Ok(Some(MuxConnectResult {
        mux: Arc::new(connect_mux(PrefixedStream::new(leftover, tcp))),
        upgraded_token,
        session_token,
    }))
}

async fn mux_rpc_call_async(mux: Arc<MuxClient>, method: String, params: Value) -> Result<Value> {
    let stream = mux.open_stream().await?;
    let (read_half, mut writer) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(read_half);
    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    tokio::time::timeout(
        Duration::from_secs(30),
        write_json_line_async(&mut writer, &req),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "Timed out sending multiplexed RPC request",
        )
    })??;

    let read_timeout = rpc_read_timeout(&method);
    let line_opt = tokio::time::timeout(
        read_timeout,
        read_bounded_line(&mut reader, MAX_FRAME_BYTES),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("Timed out waiting for multiplexed RPC reply to {method}"),
        )
    })??;
    let Some(line) = line_opt else {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Remote AppServer closed multiplexed RPC stream",
        )
        .into());
    };
    decode_rpc_reply(&line)
}

/// Manages RPC connections, multiplexed sub-streams, and relay endpoints for a client session.
pub struct ClientTransport {
    endpoint: ClientEndpoint,
    mux: Option<Arc<MuxClient>>,
    mux_relay_key: Option<String>,
    upgraded_token: Option<String>,
    rpc_pool: Mutex<Vec<RpcConn>>,
}

impl Drop for ClientTransport {
    fn drop(&mut self) {
        if let Some(key) = self.mux_relay_key.take() {
            mux_relay_registry().lock().unwrap().remove(&key);
        }
        if let Some(mux) = &self.mux {
            mux.close();
        }
    }
}

impl ClientTransport {
    /// Creates a lazy local Unix socket transport (pooling connections on demand).
    pub fn unix(path: PathBuf) -> Self {
        Self {
            endpoint: ClientEndpoint::Unix(path),
            mux: None,
            mux_relay_key: None,
            upgraded_token: None,
            rpc_pool: Mutex::new(Vec::new()),
        }
    }

    /// Connects eagerly to a non-multiplexed endpoint (`Unix` or legacy `Tcp`).
    pub fn connect_direct(endpoint: ClientEndpoint) -> Result<Self> {
        let conn = RpcConn::connect(&endpoint)?;
        Ok(Self {
            endpoint,
            mux: None,
            mux_relay_key: None,
            upgraded_token: None,
            rpc_pool: Mutex::new(vec![conn]),
        })
    }

    /// Connects to a remote TCP endpoint asynchronously, upgrading to `yamux/1` multiplexing
    /// when supported by the daemon and falling back to direct TCP RPC on older daemons.
    pub async fn connect_tcp_async(
        addr: String,
        token: String,
        device_name: String,
    ) -> Result<Self> {
        let normalized = canban_types::normalize_daemon_addr(&addr);
        if let Some(res) = try_connect_mux(normalized.clone(), token.clone(), device_name).await? {
            let effective_token = res
                .session_token
                .clone()
                .or_else(|| res.upgraded_token.clone())
                .unwrap_or(token);
            let key = canban_types::id();
            mux_relay_registry()
                .lock()
                .unwrap()
                .insert(key.clone(), res.mux.clone());
            return Ok(Self {
                endpoint: ClientEndpoint::Tcp {
                    addr: normalized,
                    token: effective_token,
                },
                mux: Some(res.mux),
                mux_relay_key: Some(key),
                upgraded_token: res.upgraded_token,
                rpc_pool: Mutex::new(Vec::new()),
            });
        }
        if canban_types::is_tailscale_auth(&token) {
            bail!(
                "auth_failed: Remote daemon does not support Tailscale automatic sign-in. Update the remote daemon or pair with a canban:// URI."
            );
        }
        let stream = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::net::TcpStream::connect(normalized.as_str()),
        )
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("Timed out connecting to {normalized}"),
            )
        })?
        .with_context(|| format!("Canban is not reachable at {normalized}"))?;
        drop(stream);
        Ok(Self {
            endpoint: ClientEndpoint::Tcp {
                addr: normalized,
                token,
            },
            mux: None,
            mux_relay_key: None,
            upgraded_token: None,
            rpc_pool: Mutex::new(Vec::new()),
        })
    }

    /// Connects to a remote TCP endpoint, upgrading to `yamux/1` multiplexing when
    /// supported by the daemon and falling back to direct TCP RPC on older daemons.
    pub fn connect_tcp(addr: String, token: String, device_name: String) -> Result<Self> {
        let normalized = canban_types::normalize_daemon_addr(&addr);
        let res_opt = run_on_client_rt({
            let a = normalized.clone();
            let t = token.clone();
            async move { try_connect_mux(a, t, device_name).await }
        })?;
        if let Some(res) = res_opt {
            let effective_token = res
                .session_token
                .clone()
                .or_else(|| res.upgraded_token.clone())
                .unwrap_or(token);
            let key = canban_types::id();
            mux_relay_registry()
                .lock()
                .unwrap()
                .insert(key.clone(), res.mux.clone());
            return Ok(Self {
                endpoint: ClientEndpoint::Tcp {
                    addr: normalized,
                    token: effective_token,
                },
                mux: Some(res.mux),
                mux_relay_key: Some(key),
                upgraded_token: res.upgraded_token,
                rpc_pool: Mutex::new(Vec::new()),
            });
        }
        if canban_types::is_tailscale_auth(&token) {
            bail!(
                "auth_failed: Remote daemon does not support Tailscale automatic sign-in. Update the remote daemon or pair with a canban:// URI."
            );
        }
        Self::connect_direct(ClientEndpoint::Tcp {
            addr: normalized,
            token,
        })
    }

    pub fn endpoint(&self) -> &ClientEndpoint {
        &self.endpoint
    }

    /// Returns the active TCP token/session secret for this transport, if any.
    pub fn active_token(&self) -> Option<&str> {
        match &self.endpoint {
            ClientEndpoint::Tcp { token, .. } if !token.trim().is_empty() => Some(token.as_str()),
            _ => None,
        }
    }

    pub fn is_mux(&self) -> bool {
        self.mux.is_some()
    }

    pub fn upgraded_token(&self) -> Option<&str> {
        self.upgraded_token.as_deref()
    }

    pub fn relay_endpoint_and_token(&self) -> (String, Option<String>) {
        if let Some(key) = &self.mux_relay_key {
            return (format!("mux://{key}"), None);
        }
        match &self.endpoint {
            ClientEndpoint::Unix(path) => (path.to_string_lossy().into_owned(), None),
            ClientEndpoint::Tcp { addr, token } => (format!("tcp://{addr}"), Some(token.clone())),
        }
    }

    pub fn clear_pool(&self) {
        self.rpc_pool.lock().unwrap().clear();
    }

    pub fn close(&self) {
        if let Some(mux) = &self.mux {
            mux.close();
        }
        self.clear_pool();
    }

    pub async fn call_async(&self, method: &str, params: &Value) -> Result<Value> {
        if let Some(mux) = &self.mux {
            return mux_rpc_call_async(mux.clone(), method.to_owned(), params.clone()).await;
        }
        let (stream, auth_token) = self.open_async_stream().await?;
        let (read_half, mut writer) = tokio::io::split(stream);
        let mut reader = tokio::io::BufReader::new(read_half);
        let mut req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        if let Some(token) = auth_token {
            req["auth"] = json!(token);
        }
        tokio::time::timeout(
            Duration::from_secs(30),
            write_json_line_async(&mut writer, &req),
        )
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("Timed out sending RPC request to {method}"),
            )
        })??;

        let read_timeout = rpc_read_timeout(method);
        let line_opt = tokio::time::timeout(
            read_timeout,
            read_bounded_line(&mut reader, MAX_FRAME_BYTES),
        )
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("Timed out waiting for RPC reply to {method}"),
            )
        })??;
        let Some(line) = line_opt else {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("Remote AppServer closed RPC stream for {method}"),
            )
            .into());
        };
        decode_rpc_reply(&line)
    }

    pub fn call(&self, method: &str, params: &Value) -> Result<Value> {
        if let Some(mux) = &self.mux {
            let m = mux.clone();
            let meth = method.to_owned();
            let p = params.clone();
            return run_on_client_rt(async move { mux_rpc_call_async(m, meth, p).await });
        }
        let pooled = self.rpc_pool.lock().unwrap().pop();
        let (mut conn, reused) = match pooled {
            Some(c) => (c, true),
            None => (RpcConn::connect(&self.endpoint)?, false),
        };
        match conn.call(method, params) {
            Ok(v) => {
                let mut pool = self.rpc_pool.lock().unwrap();
                if pool.len() < 4 {
                    pool.push(conn);
                }
                Ok(v)
            }
            Err(err)
                if reused
                    && !err.to_string().starts_with("auth_failed:")
                    && (err.downcast_ref::<io::Error>().is_some()
                        || err.to_string().contains("closed")) =>
            {
                let mut fresh = RpcConn::connect(&self.endpoint)?;
                let v = fresh.call(method, params)?;
                let mut pool = self.rpc_pool.lock().unwrap();
                if pool.len() < 4 {
                    pool.push(fresh);
                }
                Ok(v)
            }
            Err(err) => Err(err),
        }
    }

    /// Opens a synchronous stream + optional legacy/direct-TCP auth token for a push subscriber.
    pub fn open_sync_stream(
        &self,
        read_timeout: Option<Duration>,
        write_timeout: Option<Duration>,
    ) -> Result<SyncStream> {
        if let Some(mux) = &self.mux {
            let writer = open_bridged_mux_unix(mux)?;
            return Ok(into_sync_stream(writer, read_timeout, write_timeout, None)?);
        }
        match &self.endpoint {
            ClientEndpoint::Unix(path) => {
                let writer = connect_unix_sync(path, read_timeout, write_timeout)?;
                Ok(into_sync_stream(writer, read_timeout, write_timeout, None)?)
            }
            ClientEndpoint::Tcp { addr, token } => {
                let writer = connect_tcp_sync(addr, read_timeout, write_timeout)?;
                Ok(into_sync_stream(
                    writer,
                    read_timeout,
                    write_timeout,
                    Some(token.clone()),
                )?)
            }
        }
    }

    /// Opens an asynchronous stream + optional auth token without blocking a Tokio runtime.
    pub async fn open_async_stream(&self) -> Result<(TransportStream, Option<String>)> {
        if let Some(mux) = &self.mux {
            let stream = mux.open_stream().await?;
            return Ok((TransportStream::Mux(stream), None));
        }
        match &self.endpoint {
            ClientEndpoint::Unix(path) => {
                let stream = tokio::time::timeout(
                    Duration::from_secs(10),
                    tokio::net::UnixStream::connect(path),
                )
                .await
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("Timed out connecting to {}", path.display()),
                    )
                })?
                .with_context(|| format!("Canban is not running at {}", path.display()))?;
                Ok((TransportStream::Unix(stream), None))
            }
            ClientEndpoint::Tcp { addr, token } => {
                let stream = tokio::time::timeout(
                    Duration::from_secs(10),
                    tokio::net::TcpStream::connect(addr.as_str()),
                )
                .await
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("Timed out connecting to {addr}"),
                    )
                })?
                .with_context(|| format!("Canban is not reachable at {addr}"))?;
                let _ = stream.set_nodelay(true);
                Ok((TransportStream::tcp(stream), Some(token.clone())))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_daemon_extends_read_timeout_and_restores_default_after_reply() {
        assert_eq!(
            rpc_read_timeout("update.daemon"),
            DAEMON_UPDATE_RPC_READ_TIMEOUT
        );
        assert_eq!(rpc_read_timeout("app.ping"), DEFAULT_RPC_READ_TIMEOUT);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (req_seen_tx, req_seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            req_seen_tx.send(line).unwrap();
            release_rx.recv().unwrap();
            writeln!(
                stream,
                "{}",
                json!({"jsonrpc":"2.0","id":1,"result":{"updated":true}})
            )
            .unwrap();
            stream.flush().unwrap();
        });

        let mut conn = RpcConn::connect(&ClientEndpoint::Tcp {
            addr,
            token: "test-token".into(),
        })
        .unwrap();
        let probe_fd = match &conn.stream {
            SyncClientStream::Tcp { reader, .. } => reader.get_ref().try_clone().unwrap(),
            SyncClientStream::Unix { .. } => unreachable!(),
        };
        assert_eq!(
            probe_fd.read_timeout().unwrap(),
            Some(DEFAULT_RPC_READ_TIMEOUT)
        );

        let caller = std::thread::spawn(move || {
            let res = conn.call("update.daemon", &json!({})).unwrap();
            let after = match &conn.stream {
                SyncClientStream::Tcp { reader, .. } => reader.get_ref().read_timeout().unwrap(),
                SyncClientStream::Unix { .. } => unreachable!(),
            };
            (res, after)
        });

        let raw_req = req_seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(raw_req.contains("\"update.daemon\""));
        assert_eq!(
            probe_fd.read_timeout().unwrap(),
            Some(DAEMON_UPDATE_RPC_READ_TIMEOUT)
        );
        release_tx.send(()).unwrap();

        let (res, restored_timeout) = caller.join().unwrap();
        server.join().unwrap();
        assert_eq!(res["updated"], true);
        assert_eq!(restored_timeout, Some(DEFAULT_RPC_READ_TIMEOUT));
    }

    #[test]
    fn truncated_rpc_frame_returns_unexpected_eof_instead_of_json_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            // Write a partial JSON frame without trailing newline, then close.
            stream
                .write_all(br#"{"jsonrpc":"2.0","id":1,"result":{"partial":"#)
                .unwrap();
            stream.flush().unwrap();
            let _ = stream.shutdown(std::net::Shutdown::Both);
        });

        let mut conn = RpcConn::connect(&ClientEndpoint::Tcp {
            addr,
            token: "test-token".into(),
        })
        .unwrap();
        let err = conn.call("app.state", &json!({})).unwrap_err();
        server.join().unwrap();

        let io_err = err
            .downcast_ref::<io::Error>()
            .expect("truncated frame must surface as std::io::Error so pooling/respawn can retry");
        assert_eq!(io_err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
