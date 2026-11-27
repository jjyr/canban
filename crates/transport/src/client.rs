//! Client-side transport management over local Unix sockets, SSH-forwarded Unix sockets,
//! and single-connection `Noise_XX` + `yamux/1` encrypted multiplexed TCP links, plus a
//! synchronous `mux://` socketpair bridge.
use crate::{
    framing::{
        AsyncIo, MAX_FRAME_BYTES, PrefixedStream, TransportStream, read_bounded_line,
        write_json_line, write_json_line_async,
    },
    mux::{MuxClient, connect_mux},
    noise::{NoiseKeypair, NoiseRngFactory, client_handshake_xx, decode_pubkey, system_noise_rng},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    future::Future,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

/// Authentication mode presented inside an encrypted `Noise_XX` session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TcpAuthMode {
    /// Authenticate via this client's authorized Noise X25519 static public key.
    Key,
    /// Consume a one-time `pair_...` pairing code to authorize this client's static public key.
    PairingCode(String),
    /// Authenticate via Tailscale WhoIs same-account verification (`trust_tailscale_self`).
    Tailscale,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientEndpoint {
    Unix(PathBuf),
    Tcp {
        addr: String,
        server_pubkey: Option<String>,
        auth: TcpAuthMode,
    },
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

fn send_and_read_unix_rpc(
    writer: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
    req: &Value,
    read_timeout: Duration,
    closed_label: &str,
) -> Result<Value> {
    write_json_line(writer, req)?;
    let extended_timeout = read_timeout != DEFAULT_RPC_READ_TIMEOUT;
    if extended_timeout {
        writer.set_read_timeout(Some(read_timeout))?;
        reader.get_ref().set_read_timeout(Some(read_timeout))?;
    }
    let mut line = String::new();
    let read_res = reader.read_line(&mut line);
    if extended_timeout {
        let _ = writer.set_read_timeout(Some(DEFAULT_RPC_READ_TIMEOUT));
        let _ = reader
            .get_ref()
            .set_read_timeout(Some(DEFAULT_RPC_READ_TIMEOUT));
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

struct UnixRpcConn {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
}

impl UnixRpcConn {
    fn connect(path: &Path) -> Result<Self> {
        let rw = Some(DEFAULT_RPC_READ_TIMEOUT);
        let writer = connect_unix_sync(path, rw, rw)?;
        let reader = BufReader::new(writer.try_clone()?);
        Ok(Self {
            writer,
            reader,
            next_id: 1,
        })
    }

    fn call(&mut self, method: &str, params: &Value) -> Result<Value> {
        let req_id = self.next_id;
        self.next_id += 1;
        let read_timeout = rpc_read_timeout(method);
        send_and_read_unix_rpc(
            &mut self.writer,
            &mut self.reader,
            &json!({"jsonrpc":"2.0","id":req_id,"method":method,"params":params}),
            read_timeout,
            "AppServer closed RPC socket",
        )
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

fn validate_server_pubkey_policy(
    addr: &str,
    server_pubkey: Option<&str>,
) -> Result<Option<[u8; 32]>> {
    match server_pubkey.map(str::trim).filter(|s| !s.is_empty()) {
        Some(pk) => Ok(Some(decode_pubkey(pk)?)),
        None => {
            ensure!(
                canban_types::is_tailscale_addr(addr),
                "Non-Tailscale TCP endpoints require a pinned host public key (?pk=...)"
            );
            Ok(None)
        }
    }
}

async fn tcp_rpc_once_async(
    addr: String,
    server_pubkey: Option<String>,
    auth: TcpAuthMode,
    client_keypair: Arc<NoiseKeypair>,
    noise_rng: Arc<dyn NoiseRngFactory>,
    method: String,
    params: Value,
) -> Result<Value> {
    let normalized = canban_types::normalize_daemon_addr(&addr);
    let expected_pk = validate_server_pubkey_policy(&normalized, server_pubkey.as_deref())?;
    let tcp = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::TcpStream::connect(&normalized),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("Timed out connecting to {normalized}"),
        )
    })?
    .with_context(|| format!("Canban is not reachable at {normalized}"))?;
    let _ = tcp.set_nodelay(true);
    let noise_stream = client_handshake_xx(
        tcp,
        &client_keypair,
        expected_pk.as_ref(),
        noise_rng.as_ref(),
    )
    .await?;

    let (read_half, mut writer) = tokio::io::split(noise_stream);
    let mut reader = tokio::io::BufReader::new(read_half);
    let mut req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    match auth {
        TcpAuthMode::Key => {
            req["auth_mode"] = json!("key");
        }
        TcpAuthMode::Tailscale => {
            req["auth_mode"] = json!("tailscale");
        }
        TcpAuthMode::PairingCode(code) => {
            req["auth_mode"] = json!("key");
            req["pairing_code"] = json!(code);
        }
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

    let read_timeout = rpc_read_timeout(&method);
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

/// Runs a one-shot JSON-RPC call over a local Unix socket or a `Noise_XX` encrypted TCP socket.
pub fn rpc_once(
    endpoint: &ClientEndpoint,
    client_keypair: Option<Arc<NoiseKeypair>>,
    method: &str,
    params: &Value,
) -> Result<Value> {
    match endpoint {
        ClientEndpoint::Unix(path) => {
            let mut conn = UnixRpcConn::connect(path)?;
            conn.call(method, params)
        }
        ClientEndpoint::Tcp {
            addr,
            server_pubkey,
            auth,
        } => {
            let kp = match client_keypair {
                Some(k) => k,
                None => Arc::new(NoiseKeypair::generate()?),
            };
            let a = addr.clone();
            let spk = server_pubkey.clone();
            let au = auth.clone();
            let m = method.to_owned();
            let p = params.clone();
            run_on_client_rt(async move {
                tcp_rpc_once_async(a, spk, au, kp, system_noise_rng(), m, p).await
            })
        }
    }
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

fn next_mux_relay_key() -> String {
    static NEXT_KEY: AtomicU64 = AtomicU64::new(1);
    let seq = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
    format!("mux-{seq:016x}")
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
}

fn into_sync_unix_stream(
    writer: UnixStream,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
) -> io::Result<SyncStream> {
    writer.set_read_timeout(read_timeout)?;
    writer.set_write_timeout(write_timeout)?;
    let reader = writer.try_clone()?;
    Ok(SyncStream {
        reader: Box::new(reader),
        writer: Box::new(writer),
    })
}

struct MuxConnectResult {
    mux: Arc<MuxClient>,
    remote_pubkey: String,
}

async fn connect_noise_mux_on_stream<S>(
    stream: S,
    expected_server_pubkey: Option<[u8; 32]>,
    auth: TcpAuthMode,
    device_name: String,
    client_keypair: &NoiseKeypair,
    noise_rng: &dyn NoiseRngFactory,
) -> Result<MuxConnectResult>
where
    S: AsyncIo + 'static,
{
    const HANDSHAKE_MAX_BYTES: usize = 64 * 1024;
    let noise_stream = client_handshake_xx(
        stream,
        client_keypair,
        expected_server_pubkey.as_ref(),
        noise_rng,
    )
    .await?;
    let remote_pubkey = noise_stream.remote_pubkey_b64();

    let (read_half, mut writer) = tokio::io::split(noise_stream);
    let mut reader = tokio::io::BufReader::new(read_half);
    let mut params = json!({
        "protocol": "yamux/1",
        "device_name": device_name,
    });
    match auth {
        TcpAuthMode::Key => {
            params["auth_mode"] = json!("key");
        }
        TcpAuthMode::Tailscale => {
            params["auth_mode"] = json!("tailscale");
        }
        TcpAuthMode::PairingCode(code) => {
            params["auth_mode"] = json!("key");
            params["pairing_code"] = json!(code);
        }
    }
    let req = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "transport.mux",
        "params": params,
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
        bail!("Remote AppServer closed connection during transport.mux handshake");
    };
    let reply: Value =
        serde_json::from_str(&line).context("Invalid JSON reply during transport.mux handshake")?;
    if let Some(err) = reply.get("error") {
        let code = err["code"].as_i64().unwrap_or(-32000);
        let msg = err["message"].as_str().unwrap_or("Canban RPC error");
        if code == -32001 {
            bail!("auth_failed: {msg}");
        }
        bail!("{msg}");
    }
    let result = &reply["result"];
    ensure!(
        result["ready"] == true,
        "Remote AppServer rejected transport.mux (yamux/1) upgrade"
    );
    let leftover = reader.buffer().to_vec();
    let encrypted = reader.into_inner().unsplit(writer);
    Ok(MuxConnectResult {
        mux: Arc::new(connect_mux(PrefixedStream::new(leftover, encrypted))),
        remote_pubkey,
    })
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

/// Manages RPC connections, encrypted multiplexed sub-streams, and relay endpoints for a client.
pub struct ClientTransport {
    endpoint: ClientEndpoint,
    mux: Option<Arc<MuxClient>>,
    mux_relay_key: Option<String>,
    remote_pubkey: Option<String>,
    rpc_pool: Mutex<Vec<UnixRpcConn>>,
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
            remote_pubkey: None,
            rpc_pool: Mutex::new(Vec::new()),
        }
    }

    /// Connects eagerly to a local or SSH-forwarded Unix socket endpoint.
    pub fn connect_unix(path: PathBuf) -> Result<Self> {
        let conn = UnixRpcConn::connect(&path)?;
        Ok(Self {
            endpoint: ClientEndpoint::Unix(path),
            mux: None,
            mux_relay_key: None,
            remote_pubkey: None,
            rpc_pool: Mutex::new(vec![conn]),
        })
    }

    /// Connects over an already-established async byte stream (e.g. a simulated `turmoil::net::TcpStream`),
    /// running the real `Noise_XX` handshake and `transport.mux` (`yamux/1`) upgrade on the current runtime.
    pub async fn connect_stream_async<S>(
        stream: S,
        addr: String,
        server_pubkey: Option<String>,
        auth: TcpAuthMode,
        device_name: String,
        client_keypair: &NoiseKeypair,
        noise_rng: &dyn NoiseRngFactory,
    ) -> Result<Self>
    where
        S: AsyncIo + 'static,
    {
        let normalized = canban_types::normalize_daemon_addr(&addr);
        let expected_pk = validate_server_pubkey_policy(&normalized, server_pubkey.as_deref())?;
        let res = connect_noise_mux_on_stream(
            stream,
            expected_pk,
            auth.clone(),
            device_name,
            client_keypair,
            noise_rng,
        )
        .await?;
        let saved_auth = match auth {
            TcpAuthMode::Tailscale => TcpAuthMode::Tailscale,
            TcpAuthMode::Key | TcpAuthMode::PairingCode(_) => TcpAuthMode::Key,
        };
        let key = next_mux_relay_key();
        mux_relay_registry()
            .lock()
            .unwrap()
            .insert(key.clone(), res.mux.clone());
        Ok(Self {
            endpoint: ClientEndpoint::Tcp {
                addr: normalized,
                server_pubkey: Some(res.remote_pubkey.clone()),
                auth: saved_auth,
            },
            mux: Some(res.mux),
            mux_relay_key: Some(key),
            remote_pubkey: Some(res.remote_pubkey),
            rpc_pool: Mutex::new(Vec::new()),
        })
    }

    /// Connects to a remote TCP endpoint asynchronously using `Noise_XX` E2EE + `yamux/1` multiplexing.
    pub async fn connect_tcp_async(
        addr: String,
        server_pubkey: Option<String>,
        auth: TcpAuthMode,
        device_name: String,
        client_keypair: Arc<NoiseKeypair>,
        noise_rng: Arc<dyn NoiseRngFactory>,
    ) -> Result<Self> {
        let normalized = canban_types::normalize_daemon_addr(&addr);
        let _ = validate_server_pubkey_policy(&normalized, server_pubkey.as_deref())?;
        let tcp = tokio::time::timeout(
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
        let _ = tcp.set_nodelay(true);
        Self::connect_stream_async(
            tcp,
            normalized,
            server_pubkey,
            auth,
            device_name,
            &client_keypair,
            noise_rng.as_ref(),
        )
        .await
    }

    /// Synchronous wrapper around [`Self::connect_tcp_async`] running on the shared client runtime.
    pub fn connect_tcp(
        addr: String,
        server_pubkey: Option<String>,
        auth: TcpAuthMode,
        device_name: String,
        client_keypair: Arc<NoiseKeypair>,
    ) -> Result<Self> {
        run_on_client_rt(async move {
            Self::connect_tcp_async(
                addr,
                server_pubkey,
                auth,
                device_name,
                client_keypair,
                system_noise_rng(),
            )
            .await
        })
    }

    pub fn endpoint(&self) -> &ClientEndpoint {
        &self.endpoint
    }

    /// Returns the authenticated remote daemon's base64url Noise X25519 static public key.
    pub fn remote_pubkey(&self) -> Option<&str> {
        self.remote_pubkey.as_deref()
    }

    pub fn is_mux(&self) -> bool {
        self.mux.is_some()
    }

    pub fn relay_endpoint(&self) -> String {
        if let Some(key) = &self.mux_relay_key {
            return format!("mux://{key}");
        }
        match &self.endpoint {
            ClientEndpoint::Unix(path) => path.to_string_lossy().into_owned(),
            ClientEndpoint::Tcp { addr, .. } => format!("tcp://{addr}"),
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
        let stream = self.open_async_stream().await?;
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
        let ClientEndpoint::Unix(path) = &self.endpoint else {
            bail!("TCP client transport must be established via Noise_XX + yamux/1");
        };
        let pooled = self.rpc_pool.lock().unwrap().pop();
        let (mut conn, reused) = match pooled {
            Some(c) => (c, true),
            None => (UnixRpcConn::connect(path)?, false),
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
                let mut fresh = UnixRpcConn::connect(path)?;
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

    /// Opens a synchronous stream for a push subscriber or bridged `mux://` relay.
    pub fn open_sync_stream(
        &self,
        read_timeout: Option<Duration>,
        write_timeout: Option<Duration>,
    ) -> Result<SyncStream> {
        if let Some(mux) = &self.mux {
            let writer = open_bridged_mux_unix(mux)?;
            return Ok(into_sync_unix_stream(writer, read_timeout, write_timeout)?);
        }
        match &self.endpoint {
            ClientEndpoint::Unix(path) => {
                let writer = connect_unix_sync(path, read_timeout, write_timeout)?;
                Ok(into_sync_unix_stream(writer, read_timeout, write_timeout)?)
            }
            ClientEndpoint::Tcp { .. } => {
                bail!("TCP client transport requires an active Noise_XX + yamux/1 session")
            }
        }
    }

    /// Opens an asynchronous stream without blocking a Tokio runtime.
    pub async fn open_async_stream(&self) -> Result<TransportStream> {
        if let Some(mux) = &self.mux {
            let stream = mux.open_stream().await?;
            return Ok(TransportStream::Mux(stream));
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
                Ok(TransportStream::Unix(stream))
            }
            ClientEndpoint::Tcp { .. } => {
                bail!("TCP client transport requires an active Noise_XX + yamux/1 session")
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

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("rpc.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let (req_seen_tx, req_seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();

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
            let _ = done_rx.recv();
        });

        let mut conn = UnixRpcConn::connect(&sock).unwrap();
        let probe_fd = conn.reader.get_ref().try_clone().unwrap();
        assert_eq!(
            probe_fd.read_timeout().unwrap(),
            Some(DEFAULT_RPC_READ_TIMEOUT)
        );

        let caller = std::thread::spawn(move || {
            let res = conn.call("update.daemon", &json!({})).unwrap();
            let after = conn.reader.get_ref().read_timeout().unwrap();
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
        let _ = done_tx.send(());
        server.join().unwrap();
        assert_eq!(res["updated"], true);
        assert_eq!(restored_timeout, Some(DEFAULT_RPC_READ_TIMEOUT));
    }

    #[test]
    fn truncated_rpc_frame_returns_unexpected_eof_instead_of_json_error() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("trunc.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            stream
                .write_all(br#"{"jsonrpc":"2.0","id":1,"result":{"partial":"#)
                .unwrap();
            stream.flush().unwrap();
            let _ = stream.shutdown(std::net::Shutdown::Both);
        });

        let mut conn = UnixRpcConn::connect(&sock).unwrap();
        let err = conn.call("app.state", &json!({})).unwrap_err();
        server.join().unwrap();

        let io_err = err
            .downcast_ref::<io::Error>()
            .expect("truncated frame must surface as std::io::Error so pooling/respawn can retry");
        assert_eq!(io_err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn next_mux_relay_key_is_monotonic_and_collision_free_across_threads() {
        use std::collections::BTreeSet;

        let handles: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(|| (0..64).map(|_| next_mux_relay_key()).collect::<Vec<_>>())
            })
            .collect();
        let mut all = BTreeSet::new();
        for h in handles {
            let keys = h.join().unwrap();
            for pair in keys.windows(2) {
                assert!(
                    pair[0] < pair[1],
                    "keys within a thread must be strictly increasing: {:?}",
                    pair
                );
            }
            for k in keys {
                assert!(k.starts_with("mux-"));
                assert!(all.insert(k), "mux relay keys must never collide");
            }
        }
        assert_eq!(all.len(), 256);
    }
}
