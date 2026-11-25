//! Bridge between `turmoil::net` simulated TCP sockets and `canban-transport` / `canban-client`,
//! with deterministic byte-stream fault injection (`FaultyStream`) and TCP connection tracking.
use anyhow::{Context, Result, bail};
use canban_app_server::AppServer;
use canban_client::{ConnectFuture, ConnectedTransport, HostConnector};
use canban_transport::ClientTransport;
use canban_types::HostTransport;
use std::{
    collections::BTreeMap,
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::Notify,
    time::Sleep,
};

/// Deterministic byte-stream fault profile applied by [`FaultyStream`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamFaultConfig {
    /// Maximum bytes returned per `poll_read` (e.g. `Some(1)` stresses 1-byte framing splits).
    pub max_read_chunk: Option<usize>,
    /// Maximum bytes written per `poll_write`.
    pub max_write_chunk: Option<usize>,
    /// Truncate reads with a clean `0`-byte EOF once this many bytes have been read.
    pub eof_after_read_bytes: Option<u64>,
    /// Return `io::ErrorKind::ConnectionReset` once this many bytes have been read.
    pub rst_after_read_bytes: Option<u64>,
    /// Return `io::ErrorKind::ConnectionReset` once this many bytes have been written.
    pub rst_after_write_bytes: Option<u64>,
    /// Half-open read blackhole: parks `poll_read` until cleared.
    pub stall_reads: bool,
    /// Half-open write blackhole: parks `poll_write` until cleared.
    pub stall_writes: bool,
    /// Extra deterministic virtual latency injected before each read operation.
    pub extra_read_latency_ms: u64,
    /// Extra deterministic virtual latency injected before each write operation.
    pub extra_write_latency_ms: u64,
}

/// Shared, live-mutable controller for [`FaultyStream`] instances across one or more hosts.
#[derive(Clone, Debug, Default)]
pub struct StreamFaultPolicy {
    inner: Arc<PolicyInner>,
}

#[derive(Debug, Default)]
struct PolicyInner {
    default_cfg: Mutex<StreamFaultConfig>,
    host_cfgs: Mutex<BTreeMap<String, StreamFaultConfig>>,
    generation: AtomicU64,
    wake: Arc<Notify>,
}

impl StreamFaultPolicy {
    pub fn new(default_cfg: StreamFaultConfig) -> Self {
        Self {
            inner: Arc::new(PolicyInner {
                default_cfg: Mutex::new(default_cfg),
                host_cfgs: Mutex::new(BTreeMap::new()),
                generation: AtomicU64::new(0),
                wake: Arc::new(Notify::new()),
            }),
        }
    }

    /// Monotonic counter incremented whenever the fault policy is updated or cleared.
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::SeqCst)
    }

    pub fn set_default(&self, cfg: StreamFaultConfig) {
        *self.inner.default_cfg.lock().unwrap() = cfg;
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        self.inner.wake.notify_waiters();
    }

    pub fn set_for_host(&self, host_id: impl Into<String>, cfg: StreamFaultConfig) {
        self.inner
            .host_cfgs
            .lock()
            .unwrap()
            .insert(host_id.into(), cfg);
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        self.inner.wake.notify_waiters();
    }

    pub fn clear_for_host(&self, host_id: &str) {
        self.inner.host_cfgs.lock().unwrap().remove(host_id);
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        self.inner.wake.notify_waiters();
    }

    pub fn clear_all(&self) {
        *self.inner.default_cfg.lock().unwrap() = StreamFaultConfig::default();
        self.inner.host_cfgs.lock().unwrap().clear();
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        self.inner.wake.notify_waiters();
    }

    pub fn effective_config(&self, host_id: Option<&str>) -> StreamFaultConfig {
        if let Some(hid) = host_id
            && let Some(cfg) = self.inner.host_cfgs.lock().unwrap().get(hid)
        {
            return cfg.clone();
        }
        self.inner.default_cfg.lock().unwrap().clone()
    }

    fn wake_handle(&self) -> Arc<Notify> {
        Arc::clone(&self.inner.wake)
    }
}

/// Deterministic fault-injecting wrapper around any `AsyncRead + AsyncWrite + Unpin` byte stream
/// (including `turmoil::net::TcpStream`).
pub struct FaultyStream<S> {
    inner: S,
    host_id: Option<String>,
    policy: StreamFaultPolicy,
    policy_gen: u64,
    bytes_read: u64,
    bytes_written: u64,
    read_sleep: Option<Pin<Box<Sleep>>>,
    write_sleep: Option<Pin<Box<Sleep>>>,
    read_stall: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
    write_stall: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

impl<S> FaultyStream<S> {
    pub fn new(inner: S, policy: StreamFaultPolicy, host_id: Option<String>) -> Self {
        let policy_gen = policy.generation();
        Self {
            inner,
            host_id,
            policy,
            policy_gen,
            bytes_read: 0,
            bytes_written: 0,
            read_sleep: None,
            write_sleep: None,
            read_stall: None,
            write_stall: None,
        }
    }

    pub fn with_config(inner: S, cfg: StreamFaultConfig) -> Self {
        Self::new(inner, StreamFaultPolicy::new(cfg), None)
    }

    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    fn current_cfg(&self) -> StreamFaultConfig {
        self.policy.effective_config(self.host_id.as_deref())
    }

    /// Resets per-arm read/write byte budgets whenever [`StreamFaultPolicy`] changes generation,
    /// so injecting e.g. `rst_after_write_bytes: Some(32)` mid-session fires after 32 new bytes
    /// rather than immediately at byte 0 on an already-warm multiplexed connection.
    fn sync_policy_gen(&mut self) {
        let current_gen = self.policy.generation();
        if current_gen != self.policy_gen {
            self.policy_gen = current_gen;
            self.bytes_read = 0;
            self.bytes_written = 0;
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for FaultyStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.sync_policy_gen();
        let cfg = this.current_cfg();

        // 1. Half-open blackhole stall.
        if cfg.stall_reads {
            loop {
                if this.read_stall.is_none() {
                    let notify = this.policy.wake_handle();
                    this.read_stall = Some(Box::pin(async move {
                        notify.notified().await;
                    }));
                }
                match this.read_stall.as_mut().unwrap().as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(()) => {
                        this.read_stall = None;
                        if !this.current_cfg().stall_reads {
                            break;
                        }
                    }
                }
            }
        } else {
            this.read_stall = None;
        }

        this.sync_policy_gen();
        let cfg = this.current_cfg();

        // 2. Deterministic virtual latency spike.
        if cfg.extra_read_latency_ms > 0 {
            if this.read_sleep.is_none() {
                this.read_sleep = Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                    cfg.extra_read_latency_ms,
                ))));
            }
            match this.read_sleep.as_mut().unwrap().as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(()) => {
                    this.read_sleep = None;
                }
            }
        }

        // 3. RST / EOF truncation limits.
        if let Some(limit) = cfg.rst_after_read_bytes
            && this.bytes_read >= limit
        {
            crate::trace::record_coverage_hit("net.fault.read_rst");
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "simulated TCP RST during read",
            )));
        }
        if let Some(limit) = cfg.eof_after_read_bytes
            && this.bytes_read >= limit
        {
            crate::trace::record_coverage_hit("net.fault.read_eof");
            return Poll::Ready(Ok(()));
        }

        // 4. Compute effective read cap (chunking + remaining bytes before EOF/RST).
        let mut cap = buf.remaining();
        if let Some(chunk) = cfg.max_read_chunk {
            cap = cap.min(chunk.max(1));
        }
        if let Some(limit) = cfg.eof_after_read_bytes {
            let rem = limit.saturating_sub(this.bytes_read) as usize;
            cap = cap.min(rem);
        }
        if let Some(limit) = cfg.rst_after_read_bytes {
            let rem = limit.saturating_sub(this.bytes_read) as usize;
            cap = cap.min(rem);
        }

        if cap == 0 || buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }

        if cap < buf.remaining() {
            let mut sub = ReadBuf::new(buf.initialize_unfilled_to(cap));
            match Pin::new(&mut this.inner).poll_read(cx, &mut sub) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Ok(())) => {
                    let n = sub.filled().len();
                    buf.advance(n);
                    this.bytes_read = this.bytes_read.saturating_add(n as u64);
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            }
        } else {
            let before = buf.filled().len();
            match Pin::new(&mut this.inner).poll_read(cx, buf) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Ok(())) => {
                    let n = buf.filled().len().saturating_sub(before);
                    this.bytes_read = this.bytes_read.saturating_add(n as u64);
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for FaultyStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.sync_policy_gen();
        let cfg = this.current_cfg();

        // 1. Half-open write stall.
        if cfg.stall_writes {
            loop {
                if this.write_stall.is_none() {
                    let notify = this.policy.wake_handle();
                    this.write_stall = Some(Box::pin(async move {
                        notify.notified().await;
                    }));
                }
                match this.write_stall.as_mut().unwrap().as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(()) => {
                        this.write_stall = None;
                        if !this.current_cfg().stall_writes {
                            break;
                        }
                    }
                }
            }
        } else {
            this.write_stall = None;
        }

        this.sync_policy_gen();
        let cfg = this.current_cfg();

        // 2. Deterministic virtual latency spike.
        if cfg.extra_write_latency_ms > 0 {
            if this.write_sleep.is_none() {
                this.write_sleep = Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                    cfg.extra_write_latency_ms,
                ))));
            }
            match this.write_sleep.as_mut().unwrap().as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(()) => {
                    this.write_sleep = None;
                }
            }
        }

        // 3. RST limit on write.
        if let Some(limit) = cfg.rst_after_write_bytes
            && this.bytes_written >= limit
        {
            crate::trace::record_coverage_hit("net.fault.write_rst");
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "simulated TCP RST during write",
            )));
        }

        let mut max_len = buf.len();
        if let Some(chunk) = cfg.max_write_chunk {
            max_len = max_len.min(chunk.max(1));
        }
        if let Some(limit) = cfg.rst_after_write_bytes {
            let rem = limit.saturating_sub(this.bytes_written) as usize;
            max_len = max_len.min(rem.max(1));
        }

        match Pin::new(&mut this.inner).poll_write(cx, &buf[..max_len]) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(n)) => {
                this.bytes_written = this.bytes_written.saturating_add(n as u64);
                Poll::Ready(Ok(n))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// RAII guard mirroring production `ActiveTcpConnGuard` in `crates/transport/src/server.rs`
/// so simulated TCP connections increment/decrement `AppServer`'s live TCP connection counter.
pub struct ActiveTcpConnGuard {
    counter: Arc<AtomicUsize>,
}

impl ActiveTcpConnGuard {
    pub fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self { counter }
    }
}

impl Drop for ActiveTcpConnGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// [`HostConnector`] establishing real `yamux/1` + JSON-RPC sessions over `turmoil::net::TcpStream`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TurmoilConnector;

impl HostConnector for TurmoilConnector {
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
                bail!("TurmoilConnector only supports TCP hosts");
            };
            let normalized = canban_types::normalize_daemon_addr(&addr);
            let stream = tokio::time::timeout(
                Duration::from_secs(10),
                turmoil::net::TcpStream::connect(normalized.as_str()),
            )
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("Timed out connecting to {normalized}"),
                )
            })?
            .with_context(|| format!("Canban is not reachable at {normalized}"))?;
            let client = Arc::new(
                ClientTransport::connect_stream_async(stream, normalized, token, device).await?,
            );
            Ok(ConnectedTransport::new(client))
        })
    }
}

/// [`HostConnector`] establishing real `yamux/1` + JSON-RPC sessions wrapped in [`FaultyStream`].
#[derive(Clone, Debug, Default)]
pub struct FaultyTurmoilConnector {
    policy: StreamFaultPolicy,
}

impl FaultyTurmoilConnector {
    pub fn new(policy: StreamFaultPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> &StreamFaultPolicy {
        &self.policy
    }
}

impl HostConnector for FaultyTurmoilConnector {
    fn supports(&self, transport: &HostTransport) -> bool {
        matches!(transport, HostTransport::Tcp { .. })
    }

    fn connect<'a>(
        &'a self,
        host_id: &'a str,
        transport: &'a HostTransport,
        device_name: &'a str,
    ) -> ConnectFuture<'a> {
        let transport_owned = transport.clone();
        let device = device_name.to_owned();
        let host = host_id.to_owned();
        let policy = self.policy.clone();
        Box::pin(async move {
            let HostTransport::Tcp { addr, token } = transport_owned else {
                bail!("FaultyTurmoilConnector only supports TCP hosts");
            };
            let normalized = canban_types::normalize_daemon_addr(&addr);
            let raw_stream = tokio::time::timeout(
                Duration::from_secs(10),
                turmoil::net::TcpStream::connect(normalized.as_str()),
            )
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("Timed out connecting to {normalized}"),
                )
            })?
            .with_context(|| format!("Canban is not reachable at {normalized}"))?;
            let stream = FaultyStream::new(raw_stream, policy, Some(host));
            let client = Arc::new(
                ClientTransport::connect_stream_async(stream, normalized, token, device).await?,
            );
            Ok(ConnectedTransport::new(client))
        })
    }
}

/// Runs the simulated TCP accept loop for an [`AppServer`] inside a `turmoil` host,
/// tracking active TCP connections via [`ActiveTcpConnGuard`] and surviving transient accept errors.
pub async fn serve_turmoil_listener(
    server: Arc<AppServer>,
    listener: turmoil::net::TcpListener,
) -> Result<()> {
    serve_turmoil_listener_with_faults(server, listener, None, None).await
}

/// Runs the simulated TCP accept loop with optional server-side [`FaultyStream`] injection.
pub async fn serve_turmoil_listener_with_faults(
    server: Arc<AppServer>,
    listener: turmoil::net::TcpListener,
    policy: Option<StreamFaultPolicy>,
    host_id: Option<String>,
) -> Result<()> {
    let mut stop_rx = server.stop_receiver();
    let active_counter = server.transport_config().active_tcp_connections.clone();
    loop {
        if *stop_rx.borrow() || server.stopped() {
            break;
        }
        tokio::select! {
            biased;
            _ = stop_rx.changed() => break,
            res = listener.accept() => {
                match res {
                    Ok((stream, peer_addr)) => {
                        let srv = Arc::clone(&server);
                        let guard = ActiveTcpConnGuard::new(Arc::clone(&active_counter));
                        let policy = policy.clone();
                        let host_id = host_id.clone();
                        tokio::spawn(async move {
                            let _guard = guard;
                            if let Some(pol) = policy {
                                let wrapped = FaultyStream::new(stream, pol, host_id);
                                let _ = srv.accept_authenticated_stream(wrapped, Some(peer_addr)).await;
                            } else {
                                let _ = srv.accept_authenticated_stream(stream, Some(peer_addr)).await;
                            }
                        });
                    }
                    Err(_) => {
                        if *stop_rx.borrow() || server.stopped() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn faulty_stream_chunking_eof_and_rst() {
        // 1. 1-byte read chunking + EOF truncation after 5 bytes
        let (mut writer, reader) = tokio::io::duplex(64);
        let mut faulty = FaultyStream::with_config(
            reader,
            StreamFaultConfig {
                max_read_chunk: Some(1),
                eof_after_read_bytes: Some(5),
                ..StreamFaultConfig::default()
            },
        );
        writer.write_all(b"hello world").await.unwrap();
        let mut buf = [0u8; 16];
        let n1 = faulty.read(&mut buf).await.unwrap();
        assert_eq!(n1, 1);
        assert_eq!(&buf[..1], b"h");

        let mut rest = Vec::new();
        faulty.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"ello");
        assert_eq!(faulty.bytes_read(), 5);

        // 2. Mid-stream RST on read
        let (mut w2, r2) = tokio::io::duplex(64);
        let mut faulty_rst = FaultyStream::with_config(
            r2,
            StreamFaultConfig {
                rst_after_read_bytes: Some(3),
                ..StreamFaultConfig::default()
            },
        );
        w2.write_all(b"abcdef").await.unwrap();
        let mut b2 = [0u8; 8];
        let n = faulty_rst.read(&mut b2).await.unwrap();
        assert_eq!(&b2[..n], b"abc");
        let err = faulty_rst.read(&mut b2).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);

        // 3. Mid-stream RST on write
        let (w3, _r3) = tokio::io::duplex(64);
        let mut faulty_w = FaultyStream::with_config(
            w3,
            StreamFaultConfig {
                rst_after_write_bytes: Some(2),
                ..StreamFaultConfig::default()
            },
        );
        let wn = faulty_w.write(b"abcd").await.unwrap();
        assert_eq!(wn, 2);
        let werr = faulty_w.write(b"cd").await.unwrap_err();
        assert_eq!(werr.kind(), io::ErrorKind::ConnectionReset);
    }

    #[tokio::test]
    async fn faulty_stream_half_open_stall_unstalls_and_guard_tracks_connections() {
        let policy = StreamFaultPolicy::new(StreamFaultConfig {
            stall_writes: true,
            ..StreamFaultConfig::default()
        });
        let (w, mut r) = tokio::io::duplex(64);
        let mut faulty = FaultyStream::new(w, policy.clone(), Some("remote-a".into()));

        let write_task = tokio::spawn(async move {
            faulty.write_all(b"ping").await.unwrap();
            faulty.bytes_written()
        });

        // Still stalled until policy clears stall_writes
        tokio::task::yield_now().await;
        assert!(!write_task.is_finished());

        policy.clear_all();
        let written = write_task.await.unwrap();
        assert_eq!(written, 4);

        let mut got = [0u8; 4];
        r.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ping");

        // ActiveTcpConnGuard increments on construction and decrements on drop.
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let _g1 = ActiveTcpConnGuard::new(Arc::clone(&counter));
            let _g2 = ActiveTcpConnGuard::new(Arc::clone(&counter));
            assert_eq!(counter.load(Ordering::SeqCst), 2);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn faulty_stream_policy_update_resets_byte_budgets() {
        let policy = StreamFaultPolicy::new(StreamFaultConfig::default());
        let (w, mut r) = tokio::io::duplex(256);
        let mut faulty = FaultyStream::new(w, policy.clone(), Some("remote-a".into()));

        // Warm the stream with 64 bytes under a clean policy (simulating prior session traffic).
        let warmup = vec![b'A'; 64];
        faulty.write_all(&warmup).await.unwrap();
        assert_eq!(faulty.bytes_written(), 64);
        let mut drain = vec![0u8; 64];
        r.read_exact(&mut drain).await.unwrap();

        // Arm a 16-byte write RST mid-session: the generation bump must reset the write budget so
        // 16 new bytes are written mid-flight before ConnectionReset triggers (NOT byte 0!).
        policy.set_for_host(
            "remote-a",
            StreamFaultConfig {
                rst_after_write_bytes: Some(16),
                ..StreamFaultConfig::default()
            },
        );
        let payload = vec![b'B'; 32];
        let wn = faulty.write(&payload).await.unwrap();
        assert_eq!(
            wn, 16,
            "mid-session policy update must allow 16 fresh bytes before RST"
        );
        assert_eq!(faulty.bytes_written(), 16);
        let werr = faulty.write(&payload[wn..]).await.unwrap_err();
        assert_eq!(werr.kind(), io::ErrorKind::ConnectionReset);
    }
}
