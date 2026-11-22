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
        Arc,
        atomic::{AtomicBool, Ordering},
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

/// A single bidirectional logical stream inside a `yamux/1` multiplexed TCP connection.
pub struct MuxStream(yamux::Stream);

impl AsyncRead for MuxStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let slice = buf.initialize_unfilled();
        match futures_io::AsyncRead::poll_read(Pin::new(&mut self.0), cx, slice) {
            Poll::Ready(Ok(n)) => {
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for MuxStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        futures_io::AsyncWrite::poll_write(Pin::new(&mut self.0), cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        futures_io::AsyncWrite::poll_flush(Pin::new(&mut self.0), cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        futures_io::AsyncWrite::poll_close(Pin::new(&mut self.0), cx)
    }
}

struct MuxDriver<T> {
    conn: yamux::Connection<TokioCompat<T>>,
    open_rx: mpsc::Receiver<oneshot::Sender<Result<MuxStream>>>,
    inbound_tx: mpsc::UnboundedSender<MuxStream>,
    close_waker: Arc<AtomicWaker>,
    closed_flag: Arc<AtomicBool>,
    pending_open: Option<oneshot::Sender<Result<MuxStream>>>,
    closing: bool,
    close_on_open_drop: bool,
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
        if self.closed_flag.load(Ordering::Acquire) {
            self.closing = true;
        }

        loop {
            if self.closing {
                self.reject_pending("Multiplexed connection closed");
                return match self.conn.poll_close(cx) {
                    Poll::Ready(_) => {
                        self.closed_flag.store(true, Ordering::Release);
                        Poll::Ready(())
                    }
                    Poll::Pending => Poll::Pending,
                };
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
                            let _ = tx.send(Ok(MuxStream(stream)));
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
                    let _ = self.inbound_tx.send(MuxStream(stream));
                    continue;
                }
                Poll::Ready(Some(Err(_))) | Poll::Ready(None) => {
                    self.closed_flag.store(true, Ordering::Release);
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
    closed_flag: Arc<AtomicBool>,
    close_waker: Arc<AtomicWaker>,
}

impl MuxClient {
    pub fn is_closed(&self) -> bool {
        self.closed_flag.load(Ordering::Acquire) || self.open_tx.is_closed()
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
        self.closed_flag.store(true, Ordering::Release);
        self.close_waker.wake();
    }
}

pub struct MuxServer {
    inbound_rx: mpsc::UnboundedReceiver<MuxStream>,
    closed_flag: Arc<AtomicBool>,
    close_waker: Arc<AtomicWaker>,
}

impl MuxServer {
    pub async fn next_stream(&mut self) -> Option<MuxStream> {
        self.inbound_rx.recv().await
    }

    pub fn close(&self) {
        self.closed_flag.store(true, Ordering::Release);
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
    let conn = yamux::Connection::new(TokioCompat(io), cfg, mode);
    let (open_tx, open_rx) = mpsc::channel(128);
    let (inbound_tx, mut inbound_rx) = mpsc::unbounded_channel();
    let closed_flag = Arc::new(AtomicBool::new(false));
    let close_waker = Arc::new(AtomicWaker::new());
    let close_on_open_drop = matches!(mode, yamux::Mode::Client);

    let mut driver = MuxDriver {
        conn,
        open_rx,
        inbound_tx,
        close_waker: close_waker.clone(),
        closed_flag: closed_flag.clone(),
        pending_open: None,
        closing: false,
        close_on_open_drop,
    };
    tokio::spawn(async move {
        poll_fn(|cx| driver.poll_drive(cx)).await;
    });

    if matches!(mode, yamux::Mode::Client) {
        tokio::spawn(async move { while inbound_rx.recv().await.is_some() {} });
        (
            Some(MuxClient {
                open_tx,
                closed_flag,
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
                closed_flag,
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
}
