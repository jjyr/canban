//! Single-connection stream multiplexing (`yamux/1`) over authenticated TCP links.
//! Allows concurrent JSON-RPC requests, push subscriptions, and multiple `herdr.connect`
//! interactive terminal relays to share one persistent TCP connection per remote host.
use anyhow::Result;
use futures_util::task::AtomicWaker;
use std::{
    future::poll_fn,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context as TaskContext, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{mpsc, oneshot},
};

struct TokioCompat<T>(T);

impl<T: AsyncRead + Unpin> futures_io::AsyncRead for TokioCompat<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut read_buf = ReadBuf::new(buf);
        match AsyncRead::poll_read(Pin::new(&mut self.0), cx, &mut read_buf) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(read_buf.filled().len())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T: AsyncWrite + Unpin> futures_io::AsyncWrite for TokioCompat<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.0), cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.0), cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.0), cx)
    }
}

type StreamWakers = (Arc<AtomicWaker>, Arc<AtomicWaker>);

/// Shared close signal that wakes every live [`MuxStream`] reader/writer as soon as the
/// underlying `yamux` connection closes.
///
/// Workaround for `yamux 0.14.1`: `Connection::poll_close` transitions `Active -> Closing`
/// via `Active::close()`, which drops `Active.streams` without calling `drop_all_streams()`,
/// leaving streams parked inside `AsyncRead::poll_read` without waking their reader wakers.
struct CloseSignal {
    closed: AtomicBool,
    next_id: AtomicU64,
    streams: Mutex<std::collections::BTreeMap<u64, StreamWakers>>,
}

impl CloseSignal {
    fn new() -> Self {
        Self {
            closed: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            streams: Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn register_stream(&self) -> (u64, Arc<AtomicWaker>, Arc<AtomicWaker>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let read_waker = Arc::new(AtomicWaker::new());
        let write_waker = Arc::new(AtomicWaker::new());
        let mut guard = self.streams.lock().unwrap();
        if !self.closed.load(Ordering::SeqCst) {
            guard.insert(id, (Arc::clone(&read_waker), Arc::clone(&write_waker)));
        }
        (id, read_waker, write_waker)
    }

    fn unregister_stream(&self, id: u64) {
        if let Ok(mut guard) = self.streams.lock() {
            guard.remove(&id);
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        std::sync::atomic::fence(Ordering::SeqCst);
        let wakers: Vec<_> = {
            let mut guard = self.streams.lock().unwrap();
            std::mem::take(&mut *guard).into_values().collect()
        };
        for (read_waker, write_waker) in wakers {
            read_waker.wake();
            write_waker.wake();
        }
    }
}

/// A single bidirectional logical stream inside a `yamux/1` multiplexed TCP connection.
pub struct MuxStream {
    inner: yamux::Stream,
    id: u64,
    close_signal: Arc<CloseSignal>,
    read_waker: Arc<AtomicWaker>,
    write_waker: Arc<AtomicWaker>,
}

impl MuxStream {
    fn new(inner: yamux::Stream, close_signal: Arc<CloseSignal>) -> Self {
        let (id, read_waker, write_waker) = close_signal.register_stream();
        Self {
            inner,
            id,
            close_signal,
            read_waker,
            write_waker,
        }
    }
}

impl Drop for MuxStream {
    fn drop(&mut self) {
        self.close_signal.unregister_stream(self.id);
    }
}

impl AsyncRead for MuxStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let slice = buf.initialize_unfilled();
        match futures_io::AsyncRead::poll_read(Pin::new(&mut self.inner), cx, slice) {
            Poll::Ready(Ok(n)) => {
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => {
                self.read_waker.register(cx.waker());
                std::sync::atomic::fence(Ordering::SeqCst);
                if self.close_signal.closed.load(Ordering::SeqCst) {
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending
            }
        }
    }
}

impl AsyncWrite for MuxStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match futures_io::AsyncWrite::poll_write(Pin::new(&mut self.inner), cx, buf) {
            Poll::Ready(res) => Poll::Ready(res),
            Poll::Pending => {
                self.write_waker.register(cx.waker());
                std::sync::atomic::fence(Ordering::SeqCst);
                if self.close_signal.closed.load(Ordering::SeqCst) {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "Multiplexed connection closed",
                    )));
                }
                Poll::Pending
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        match futures_io::AsyncWrite::poll_flush(Pin::new(&mut self.inner), cx) {
            Poll::Ready(res) => Poll::Ready(res),
            Poll::Pending => {
                self.write_waker.register(cx.waker());
                std::sync::atomic::fence(Ordering::SeqCst);
                if self.close_signal.closed.load(Ordering::SeqCst) {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "Multiplexed connection closed",
                    )));
                }
                Poll::Pending
            }
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        match futures_io::AsyncWrite::poll_close(Pin::new(&mut self.inner), cx) {
            Poll::Ready(res) => Poll::Ready(res),
            Poll::Pending => {
                self.write_waker.register(cx.waker());
                std::sync::atomic::fence(Ordering::SeqCst);
                if self.close_signal.closed.load(Ordering::SeqCst) {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "Multiplexed connection closed",
                    )));
                }
                Poll::Pending
            }
        }
    }
}

struct MuxDriver<T> {
    conn: yamux::Connection<TokioCompat<T>>,
    open_rx: mpsc::Receiver<oneshot::Sender<Result<MuxStream>>>,
    inbound_tx: mpsc::UnboundedSender<MuxStream>,
    close_waker: Arc<AtomicWaker>,
    close_signal: Arc<CloseSignal>,
    pending_open: Option<oneshot::Sender<Result<MuxStream>>>,
    closing: bool,
    close_on_open_drop: bool,
}

impl<T> Drop for MuxDriver<T> {
    fn drop(&mut self) {
        self.close_signal.close();
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> MuxDriver<T> {
    fn reject_pending(&mut self, msg: &str) {
        if let Some(tx) = self.pending_open.take() {
            let _ = tx.send(Err(
                io::Error::new(io::ErrorKind::ConnectionReset, msg).into()
            ));
        }
        while let Ok(tx) = self.open_rx.try_recv() {
            let _ = tx.send(Err(
                io::Error::new(io::ErrorKind::ConnectionReset, msg).into()
            ));
        }
    }

    fn poll_drive(&mut self, cx: &mut TaskContext<'_>) -> Poll<()> {
        self.close_waker.register(cx.waker());
        if self.close_signal.is_closed() {
            self.closing = true;
        }

        loop {
            if self.closing {
                self.close_signal.close();
                self.reject_pending("Multiplexed connection closed");
                return Poll::Ready(());
            }

            if self.pending_open.is_none() {
                match self.open_rx.poll_recv(cx) {
                    Poll::Ready(Some(tx)) => {
                        self.pending_open = Some(tx);
                    }
                    Poll::Ready(None) => {
                        if self.close_on_open_drop {
                            self.closing = true;
                            continue;
                        }
                    }
                    Poll::Pending => {}
                }
            }

            if self.pending_open.is_some() {
                match self.conn.poll_new_outbound(cx) {
                    Poll::Ready(Ok(stream)) => {
                        if let Some(tx) = self.pending_open.take() {
                            let _ =
                                tx.send(Ok(MuxStream::new(stream, Arc::clone(&self.close_signal))));
                        }
                        continue;
                    }
                    Poll::Ready(Err(e)) => {
                        if let Some(tx) = self.pending_open.take() {
                            let _ = tx.send(Err(io::Error::new(
                                io::ErrorKind::ConnectionReset,
                                format!("Yamux outbound stream failed: {e}"),
                            )
                            .into()));
                        }
                        self.closing = true;
                        continue;
                    }
                    Poll::Pending => {}
                }
            }

            match self.conn.poll_next_inbound(cx) {
                Poll::Ready(Some(Ok(stream))) => {
                    let _ = self
                        .inbound_tx
                        .send(MuxStream::new(stream, Arc::clone(&self.close_signal)));
                    continue;
                }
                Poll::Ready(Some(Err(_))) | Poll::Ready(None) => {
                    self.close_signal.close();
                    self.reject_pending("Multiplexed connection closed");
                    return Poll::Ready(());
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[derive(Clone)]
pub struct MuxClient {
    open_tx: mpsc::Sender<oneshot::Sender<Result<MuxStream>>>,
    close_signal: Arc<CloseSignal>,
    close_waker: Arc<AtomicWaker>,
}

impl MuxClient {
    pub fn is_closed(&self) -> bool {
        self.close_signal.is_closed() || self.open_tx.is_closed()
    }

    pub async fn open_stream(&self) -> Result<MuxStream> {
        if self.is_closed() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "Multiplexed connection is closed",
            )
            .into());
        }
        let (tx, rx) = oneshot::channel();
        self.open_tx.send(tx).await.map_err(|_| {
            io::Error::new(
                io::ErrorKind::ConnectionReset,
                "Multiplexed connection is closed",
            )
        })?;
        rx.await.map_err(|_| {
            io::Error::new(
                io::ErrorKind::ConnectionReset,
                "Multiplexed connection dropped",
            )
        })?
    }

    pub fn close(&self) {
        self.close_signal.close();
        self.close_waker.wake();
    }
}

pub struct MuxServer {
    inbound_rx: mpsc::UnboundedReceiver<MuxStream>,
    close_signal: Arc<CloseSignal>,
    close_waker: Arc<AtomicWaker>,
}

impl MuxServer {
    pub async fn next_stream(&mut self) -> Option<MuxStream> {
        self.inbound_rx.recv().await
    }

    pub fn close(&self) {
        self.close_signal.close();
        self.close_waker.wake();
    }
}

impl Drop for MuxServer {
    fn drop(&mut self) {
        self.close();
    }
}

fn spawn_mux<T: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    io: T,
    mode: yamux::Mode,
) -> (Option<MuxClient>, Option<MuxServer>) {
    let mut cfg = yamux::Config::default();
    cfg.set_max_num_streams(512);
    cfg.set_split_send_size(64 * 1024);
    let conn = yamux::Connection::new(TokioCompat(io), cfg, mode);
    let (open_tx, open_rx) = mpsc::channel(128);
    let (inbound_tx, mut inbound_rx) = mpsc::unbounded_channel();
    let close_signal = Arc::new(CloseSignal::new());
    let close_waker = Arc::new(AtomicWaker::new());
    let close_on_open_drop = matches!(mode, yamux::Mode::Client);

    let mut driver = MuxDriver {
        conn,
        open_rx,
        inbound_tx,
        close_waker: close_waker.clone(),
        close_signal: close_signal.clone(),
        pending_open: None,
        closing: false,
        close_on_open_drop,
    };
    tokio::spawn(async move {
        poll_fn(|cx| driver.poll_drive(cx)).await;
        if driver.closing {
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(200),
                poll_fn(|cx| driver.conn.poll_close(cx)),
            )
            .await;
            driver.close_signal.close();
        }
    });

    if matches!(mode, yamux::Mode::Client) {
        tokio::spawn(async move { while inbound_rx.recv().await.is_some() {} });
        (
            Some(MuxClient {
                open_tx,
                close_signal,
                close_waker,
            }),
            None,
        )
    } else {
        drop(open_tx);
        (
            None,
            Some(MuxServer {
                inbound_rx,
                close_signal,
                close_waker,
            }),
        )
    }
}

pub fn connect_mux<T: AsyncRead + AsyncWrite + Unpin + Send + 'static>(io: T) -> MuxClient {
    let (client, _) = spawn_mux(io, yamux::Mode::Client);
    client.expect("client mode yields MuxClient")
}

pub fn accept_mux<T: AsyncRead + AsyncWrite + Unpin + Send + 'static>(io: T) -> MuxServer {
    let (_, server) = spawn_mux(io, yamux::Mode::Server);
    server.expect("server mode yields MuxServer")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::{PrefixedStream, read_bounded_line, write_json_line_async};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn yamux_multiplexes_concurrent_bidirectional_streams() {
        let (client_io, server_io) = tokio::io::duplex(2048);
        let client = connect_mux(client_io);
        let mut server = accept_mux(server_io);

        let srv_task = tokio::spawn(async move {
            let mut handlers = Vec::new();
            while let Some(mut stream) = server.next_stream().await {
                handlers.push(tokio::spawn(async move {
                    let mut buf = Vec::new();
                    stream.read_to_end(&mut buf).await.unwrap();
                    stream.write_all(&buf).await.unwrap();
                    stream.shutdown().await.unwrap();
                }));
            }
            for h in handlers {
                h.await.unwrap();
            }
        });

        let mut workers = Vec::new();
        for i in 0u8..8 {
            let c = client.clone();
            workers.push(tokio::spawn(async move {
                let payload = vec![i.wrapping_add(1); 32 * 1024];
                let stream = c.open_stream().await.unwrap();
                let (mut r, mut w) = tokio::io::split(stream);
                let send_payload = payload.clone();
                let writer = tokio::spawn(async move {
                    w.write_all(&send_payload).await.unwrap();
                    w.shutdown().await.unwrap();
                });
                let mut echoed = Vec::new();
                r.read_to_end(&mut echoed).await.unwrap();
                writer.await.unwrap();
                assert_eq!(echoed, payload);
            }));
        }
        for w in workers {
            w.await.unwrap();
        }
        client.close();
        srv_task.await.unwrap();
    }

    #[tokio::test]
    async fn prefixed_stream_preserves_early_yamux_frames_on_upgrade() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let srv_task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let _ = tcp.set_nodelay(true);
            let (r, mut w) = tokio::io::split(tcp);
            let mut reader = tokio::io::BufReader::new(r);
            let req = read_bounded_line(&mut reader, 64 * 1024)
                .await
                .unwrap()
                .unwrap();
            assert!(req.contains("transport.mux"));
            write_json_line_async(
                &mut w,
                &json!({"jsonrpc":"2.0","id":0,"result":{"ready":true}}),
            )
            .await
            .unwrap();
            let leftover = reader.buffer().to_vec();
            let tcp = reader.into_inner().unsplit(w);
            let mut server = accept_mux(PrefixedStream::new(leftover, tcp));
            if let Some(mut stream) = server.next_stream().await {
                let mut buf = Vec::new();
                stream.read_to_end(&mut buf).await.unwrap();
                stream.write_all(&buf).await.unwrap();
                stream.shutdown().await.unwrap();
            }
        });

        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let _ = tcp.set_nodelay(true);
        let (r, mut w) = tokio::io::split(tcp);
        write_json_line_async(
            &mut w,
            &json!({"jsonrpc":"2.0","id":0,"method":"transport.mux"}),
        )
        .await
        .unwrap();
        // Give server time to reply AND flush yamux's initial frame into the same socket buffer.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let mut reader = tokio::io::BufReader::new(r);
        let reply = read_bounded_line(&mut reader, 64 * 1024)
            .await
            .unwrap()
            .unwrap();
        assert!(reply.contains("\"ready\":true"));
        let leftover = reader.buffer().to_vec();
        let tcp = reader.into_inner().unsplit(w);
        let client = connect_mux(PrefixedStream::new(leftover, tcp));

        let stream = client.open_stream().await.unwrap();
        let (mut sr, mut sw) = tokio::io::split(stream);
        sw.write_all(b"after-upgrade-hello").await.unwrap();
        sw.shutdown().await.unwrap();
        let mut echoed = Vec::new();
        sr.read_to_end(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"after-upgrade-hello");

        client.close();
        srv_task.await.unwrap();
    }

    #[tokio::test]
    async fn mux_close_on_stalled_peer_returns_promptly_and_fails_open_stream() {
        let (client_io, _server_io_stalled) = tokio::io::duplex(64);
        let client = connect_mux(client_io);

        let close_started = std::time::Instant::now();
        client.close();
        assert!(client.is_closed());
        for _ in 0..50 {
            if client.open_stream().await.is_err() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            close_started.elapsed() < std::time::Duration::from_secs(2),
            "closing a stalled mux must not block indefinitely"
        );
        assert!(client.open_stream().await.is_err());
    }

    #[tokio::test]
    async fn mux_close_wakes_in_flight_stream_reader_immediately() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let client = connect_mux(client_io);
        let mut server = accept_mux(server_io);

        let (received_tx, received_rx) = oneshot::channel::<()>();
        let (release_srv_tx, release_srv_rx) = oneshot::channel::<()>();
        let srv_task = tokio::spawn(async move {
            let mut stream = server.next_stream().await.expect("inbound mux stream");
            let mut req = [0u8; 5];
            stream.read_exact(&mut req).await.unwrap();
            assert_eq!(&req, b"ping\n");
            let _ = received_tx.send(());
            // Hold stream open without replying until the test finishes (simulating an in-flight RPC).
            let _ = release_srv_rx.await;
            // Closing server while a peer or local reader is parked must also finish cleanly.
            server.close();
        });

        let stream = client.open_stream().await.unwrap();
        let (mut r, mut w) = tokio::io::split(stream);
        w.write_all(b"ping\n").await.unwrap();
        w.flush().await.unwrap();
        received_rx.await.unwrap();

        // Spawn a reader that genuinely parks inside `MuxStream::poll_read` waiting for a reply.
        let read_task = tokio::spawn(async move {
            let mut buf = [0u8; 32];
            r.read(&mut buf).await
        });
        tokio::task::yield_now().await;

        // Closing the mux client must immediately wake the parked reader with EOF rather than
        // stalling until an outer RPC read timeout expires.
        client.close();
        let n = tokio::time::timeout(std::time::Duration::from_secs(2), read_task)
            .await
            .expect("in-flight stream reader must wake promptly on mux close")
            .unwrap()
            .unwrap();
        assert_eq!(n, 0, "closed mux stream must surface EOF to parked reader");

        let _ = release_srv_tx.send(());
        srv_task.await.unwrap();
    }

    #[tokio::test]
    async fn mux_server_close_wakes_parked_inbound_stream_reader_immediately() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let client = connect_mux(client_io);
        let mut server = accept_mux(server_io);

        let mut client_stream = client.open_stream().await.unwrap();
        client_stream.write_all(b"hi").await.unwrap();
        client_stream.flush().await.unwrap();

        let mut inbound = server.next_stream().await.expect("inbound stream");
        let mut head = [0u8; 2];
        inbound.read_exact(&mut head).await.unwrap();
        assert_eq!(&head, b"hi");

        let read_task = tokio::spawn(async move {
            let mut buf = [0u8; 16];
            inbound.read(&mut buf).await
        });
        tokio::task::yield_now().await;

        server.close();
        let n = tokio::time::timeout(std::time::Duration::from_secs(2), read_task)
            .await
            .expect("parked server-side stream reader must wake promptly on server close")
            .unwrap()
            .unwrap();
        assert_eq!(n, 0);

        client.close();
    }
}
