//! Server-side transport listeners (`Unix` and `Tcp`), TCP first-frame authentication,
//! `transport.mux` (`yamux/1`) session demultiplexing, and live device revocation enforcement.
use crate::{
    auth::{
        self, DeviceTokenStore, PairingCodeStore, RevocationBus, TailscalePeerVerifier,
        TailscaleSessionRegistry, TcpAuthOutcome,
    },
    framing::{
        AsyncIo, MAX_FRAME_BYTES, PrefixedStream, TransportStream, read_bounded_line,
        write_json_line_async,
    },
};
use anyhow::Result;
use serde_json::{Value, json};
use std::{
    future::Future,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncWriteExt;

pub struct ServerTransportConfig {
    pub home: PathBuf,
    pub server_id: String,
    pub api_version: u32,
    pub device_store: Arc<DeviceTokenStore>,
    pub pairing_codes: Arc<PairingCodeStore>,
    pub revocations: RevocationBus,
    pub active_tcp_connections: Arc<AtomicUsize>,
    pub tailscale_verifier: Option<Arc<dyn TailscalePeerVerifier>>,
    pub tailscale_devices: Arc<TailscaleSessionRegistry>,
}

/// An authenticated bidirectional application stream handed to the server dispatcher.
pub struct AcceptedStream {
    pub stream: TransportStream,
    /// Present for authenticated non-mux TCP connections where frame 1 was read during auth.
    pub initial_request: Option<Value>,
    /// True if frame 1 on a non-mux TCP stream was followed immediately by pipelined bytes.
    pub has_pipelined_bytes: bool,
    pub device_id: Option<String>,
    /// Effective bearer token for the authenticated TCP session (or `None` on Unix sockets).
    pub auth_token: Option<zeroize::Zeroizing<String>>,
    pub stop_rx: tokio::sync::watch::Receiver<bool>,
}

struct ActiveTcpConnGuard {
    counter: Arc<AtomicUsize>,
}

impl ActiveTcpConnGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self { counter }
    }
}

impl Drop for ActiveTcpConnGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn spawn_unix_listener<H, Fut>(
    handle: &tokio::runtime::Handle,
    listener: tokio::net::UnixListener,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    handler: H,
) where
    H: Fn(AcceptedStream) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let handler = Arc::new(handler);
    handle.spawn(async move {
        loop {
            if *stop_rx.borrow() {
                break;
            }
            tokio::select! {
                biased;
                _ = stop_rx.changed() => break,
                res = listener.accept() => {
                    match res {
                        Ok((stream, _)) => {
                            let h = handler.clone();
                            let conn_stop = stop_rx.clone();
                            tokio::spawn(async move {
                                h(AcceptedStream {
                                    stream: TransportStream::Unix(stream),
                                    initial_request: None,
                                    has_pipelined_bytes: false,
                                    device_id: None,
                                    auth_token: None,
                                    stop_rx: conn_stop,
                                })
                                .await;
                            });
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    });
}

pub fn spawn_tcp_listener<H, Fut>(
    handle: &tokio::runtime::Handle,
    listener: tokio::net::TcpListener,
    cfg: Arc<ServerTransportConfig>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    handler: H,
) where
    H: Fn(AcceptedStream) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let handler = Arc::new(handler);
    handle.spawn(async move {
        loop {
            if *stop_rx.borrow() {
                break;
            }
            tokio::select! {
                biased;
                _ = stop_rx.changed() => break,
                res = listener.accept() => {
                    match res {
                        Ok((stream, _)) => {
                            let _ = stream.set_nodelay(true);
                            let guard = ActiveTcpConnGuard::new(cfg.active_tcp_connections.clone());
                            let cfg = cfg.clone();
                            let h = handler.clone();
                            let conn_stop = stop_rx.clone();
                            tokio::spawn(async move {
                                let _guard = guard;
                                let peer_addr = stream.peer_addr().ok();
                                let _ = serve_authenticated_stream(
                                    stream, peer_addr, cfg, conn_stop, false, h,
                                )
                                .await;
                            });
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    });
}

async fn wait_device_revoked(
    device_store: &Arc<DeviceTokenStore>,
    revoke_rx: &mut tokio::sync::broadcast::Receiver<String>,
    device_id: &str,
    inline_auth: bool,
) {
    loop {
        match revoke_rx.recv().await {
            Ok(revoked_id) if revoked_id == device_id => return,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                if device_id != "legacy-server-token" && !device_id.starts_with("ts:") {
                    let valid = if inline_auth {
                        device_store.is_valid_device_id(device_id)
                    } else {
                        let store = device_store.clone();
                        let did = device_id.to_owned();
                        tokio::task::spawn_blocking(move || store.is_valid_device_id(&did))
                            .await
                            .unwrap_or(false)
                    };
                    if !valid {
                        return;
                    }
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            Ok(_) => {}
        }
    }
}

async fn send_rpc_error<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    id: &Value,
    code: i64,
    message: impl AsRef<str>,
) {
    let err_reply = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.as_ref(),
        }
    });
    let _ = write_json_line_async(writer, &err_reply).await;
}

async fn watch_stop_and_revocation(
    device_store: Arc<DeviceTokenStore>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    mut revoke_rx: tokio::sync::broadcast::Receiver<String>,
    device_id: String,
    inline_auth: bool,
    conn_stop_tx: tokio::sync::watch::Sender<bool>,
) {
    if !*stop_rx.borrow() {
        tokio::select! {
            biased;
            _ = stop_rx.changed() => {}
            _ = wait_device_revoked(&device_store, &mut revoke_rx, &device_id, inline_auth) => {}
        }
    }
    let _ = conn_stop_tx.send(true);
}

/// Runs first-frame authentication, `transport.mux` (`yamux/1`) upgrade, and device revocation
/// enforcement over an arbitrary async byte stream (such as `tokio::net::TcpStream` or a
/// simulated `turmoil::net::TcpStream`).
///
/// Set `inline_auth = true` in deterministic simulations to avoid `spawn_blocking` thread jitter.
pub async fn serve_authenticated_stream<S, H, Fut>(
    stream: S,
    peer_addr: Option<std::net::SocketAddr>,
    cfg: Arc<ServerTransportConfig>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    inline_auth: bool,
    handler: Arc<H>,
) -> Result<()>
where
    S: AsyncIo + 'static,
    H: Fn(AcceptedStream) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    if *stop_rx.borrow() {
        return Ok(());
    }
    let (read_half, mut writer) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(read_half);
    let line_opt = tokio::select! {
        biased;
        _ = stop_rx.changed() => return Ok(()),
        res = tokio::time::timeout(Duration::from_secs(20), read_bounded_line(&mut reader, MAX_FRAME_BYTES)) => {
            match res {
                Ok(Ok(opt)) => opt,
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => return Ok(()),
            }
        }
    };
    let Some(line) = line_opt else {
        return Ok(());
    };
    let input: Value = serde_json::from_str(&line)?;
    let id = input["id"].clone();
    let method = input
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Missing string field: method"))?
        .to_owned();

    let home = cfg.home.clone();
    let device_store = cfg.device_store.clone();
    let pairing_codes = cfg.pairing_codes.clone();
    let ts_verifier = cfg.tailscale_verifier.clone();
    let ts_devices = cfg.tailscale_devices.clone();
    let provided = input["auth"].as_str().unwrap_or("").to_owned();
    let provided_auth = provided.clone();
    let is_ts_request = canban_types::is_tailscale_auth(&provided_auth);
    let device_name = input["params"]["device_name"].as_str().map(str::to_owned);
    let allow_upgrade = method == "transport.mux";

    let auth_work = move || {
        if canban_types::is_tailscale_auth(&provided_auth) {
            if let (Some(verifier), Some(addr)) = (ts_verifier.as_deref(), peer_addr)
                && let Some(peer) = verifier.verify_peer(addr)
            {
                let device_id = format!("ts:{}", peer.node_id);
                let effective_name = device_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&peer.device_name);
                ts_devices.record(&device_id, effective_name);
                let session_secret = auth::generate_ephemeral_session_token();
                return Ok((
                    TcpAuthOutcome::Authenticated {
                        device_id,
                        upgraded_token: None,
                        deprecated_legacy: false,
                    },
                    Some(session_secret),
                    true,
                ));
            }
            return Ok((TcpAuthOutcome::Rejected, None, false));
        }
        let res = auth::authenticate_tcp_token(
            &home,
            &device_store,
            &pairing_codes,
            &provided_auth,
            device_name.as_deref(),
            allow_upgrade,
        )?;
        Ok::<_, anyhow::Error>((res, None, false))
    };

    let (outcome, ephemeral_session_token, is_tailscale_session) = if inline_auth {
        auth_work()?
    } else {
        tokio::task::spawn_blocking(auth_work).await??
    };

    let (device_id, upgraded_token, deprecated_legacy) = match outcome {
        TcpAuthOutcome::Authenticated {
            device_id,
            upgraded_token,
            deprecated_legacy,
        } => (device_id, upgraded_token, deprecated_legacy),
        TcpAuthOutcome::Rejected => {
            let msg = if is_ts_request {
                "Tailscale authentication failed: ensure both devices are signed into the same personal Tailscale account"
            } else {
                "Authentication required: invalid or missing device token"
            };
            send_rpc_error(&mut writer, &id, -32001, msg).await;
            let _ = writer.shutdown().await;
            return Ok(());
        }
    };
    let session_token = zeroize::Zeroizing::new(
        ephemeral_session_token
            .clone()
            .or_else(|| upgraded_token.clone())
            .unwrap_or(provided),
    );

    if method == "transport.mux" {
        if !reader.buffer().is_empty() {
            send_rpc_error(
                &mut writer,
                &id,
                -32000,
                "Unexpected bytes pipelined after transport.mux",
            )
            .await;
            return Ok(());
        }
        let protocol = input["params"]["protocol"].as_str().unwrap_or("yamux/1");
        if protocol != "yamux/1" {
            send_rpc_error(
                &mut writer,
                &id,
                -32000,
                format!("Unsupported mux protocol: {protocol}"),
            )
            .await;
            return Ok(());
        }
        let mut res_obj = json!({
            "ready": true,
            "protocol": "yamux/1",
            "server_id": cfg.server_id,
            "api_version": cfg.api_version,
            "device_id": device_id,
            "deprecated_legacy_token": deprecated_legacy,
        });
        if let Some(tok) = upgraded_token {
            res_obj["upgraded_token"] = json!(tok);
        }
        if is_tailscale_session {
            res_obj["auth_mode"] = json!("tailscale");
            if let Some(st) = ephemeral_session_token {
                res_obj["session_token"] = json!(st);
            }
        }
        let reply = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": res_obj,
        });
        tokio::time::timeout(
            Duration::from_secs(30),
            write_json_line_async(&mut writer, &reply),
        )
        .await??;

        let leftover = reader.buffer().to_vec();
        let inner = reader.into_inner().unsplit(writer);
        return handle_mux_session(
            inner,
            leftover,
            cfg,
            device_id,
            session_token,
            stop_rx,
            inline_auth,
            handler,
        )
        .await;
    }

    let leftover = reader.buffer().to_vec();
    let has_pipelined_bytes = !leftover.is_empty();
    let inner = reader.into_inner().unsplit(writer);
    let (conn_stop_tx, conn_stop_rx) = tokio::sync::watch::channel(false);
    let watcher = tokio::spawn(watch_stop_and_revocation(
        cfg.device_store.clone(),
        stop_rx,
        cfg.revocations.subscribe(),
        device_id.clone(),
        inline_auth,
        conn_stop_tx,
    ));
    handler(AcceptedStream {
        stream: TransportStream::boxed_prefixed(leftover, inner),
        initial_request: Some(input),
        has_pipelined_bytes,
        device_id: Some(device_id),
        auth_token: Some(session_token),
        stop_rx: conn_stop_rx,
    })
    .await;
    watcher.abort();
    let _ = watcher.await;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_mux_session<S, H, Fut>(
    stream: S,
    leftover: Vec<u8>,
    cfg: Arc<ServerTransportConfig>,
    device_id: String,
    session_token: zeroize::Zeroizing<String>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    inline_auth: bool,
    handler: Arc<H>,
) -> Result<()>
where
    S: AsyncIo + 'static,
    H: Fn(AcceptedStream) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut mux_server = crate::mux::accept_mux(PrefixedStream::new(leftover, stream));
    let mut revoke_rx = cfg.revocations.subscribe();
    let (conn_stop_tx, conn_stop_rx) = tokio::sync::watch::channel(false);

    loop {
        if *stop_rx.borrow() {
            break;
        }
        tokio::select! {
            biased;
            _ = stop_rx.changed() => break,
            _ = wait_device_revoked(&cfg.device_store, &mut revoke_rx, &device_id, inline_auth) => break,
            stream_opt = mux_server.next_stream() => {
                let Some(mux_stream) = stream_opt else {
                    break;
                };
                let h = handler.clone();
                let child_stop = conn_stop_rx.clone();
                let did = device_id.clone();
                let tok = session_token.clone();
                tokio::spawn(async move {
                    h(AcceptedStream {
                        stream: TransportStream::Mux(mux_stream),
                        initial_request: None,
                        has_pipelined_bytes: false,
                        device_id: Some(did),
                        auth_token: Some(tok),
                        stop_rx: child_stop,
                    })
                    .await;
                });
            }
        }
    }

    let _ = conn_stop_tx.send(true);
    mux_server.close();
    Ok(())
}
