//! Framed JSON-line helpers and unified async stream wrapper (`Unix`, `Tcp`, `Mux`).
use serde_json::Value;
use std::{
    io::{self, Write},
    pin::Pin,
    task::{Context as TaskContext, Poll},
};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

pub const MAX_FRAME_BYTES: usize = 48 * 1024 * 1024;

/// Prepends already-buffered prefix bytes ahead of an async stream.
/// Used when switching a buffered TCP connection into `yamux/1` multiplexing
/// or when preserving leftover bytes read past an initial JSON line.
pub struct PrefixedStream<T> {
    prefix: io::Cursor<Vec<u8>>,
    inner: T,
}

impl<T> PrefixedStream<T> {
    pub fn new(prefix: Vec<u8>, inner: T) -> Self {
        Self {
            prefix: io::Cursor::new(prefix),
            inner,
        }
    }

    pub fn has_buffered(&self) -> bool {
        (self.prefix.position() as usize) < self.prefix.get_ref().len()
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for PrefixedStream<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let pos = self.prefix.position() as usize;
        let len = self.prefix.get_ref().len();
        if pos < len && buf.remaining() > 0 {
            let remaining = &self.prefix.get_ref()[pos..];
            let to_copy = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..to_copy]);
            self.prefix.set_position((pos + to_copy) as u64);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for PrefixedStream<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Marker trait for type-erased async bidirectional byte streams (e.g., simulated TCP in DST).
pub trait AsyncIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncIo for T {}

/// Unified asynchronous bidirectional stream over Unix sockets, TCP sockets, `yamux/1` sub-streams,
/// or type-erased custom async streams.
pub enum TransportStream {
    Unix(tokio::net::UnixStream),
    Tcp(PrefixedStream<tokio::net::TcpStream>),
    Mux(crate::mux::MuxStream),
    Boxed(PrefixedStream<Box<dyn AsyncIo>>),
}

impl TransportStream {
    pub fn tcp(stream: tokio::net::TcpStream) -> Self {
        Self::Tcp(PrefixedStream::new(Vec::new(), stream))
    }

    pub fn tcp_prefixed(prefix: Vec<u8>, stream: tokio::net::TcpStream) -> Self {
        Self::Tcp(PrefixedStream::new(prefix, stream))
    }

    pub fn boxed<S: AsyncIo + 'static>(stream: S) -> Self {
        Self::Boxed(PrefixedStream::new(Vec::new(), Box::new(stream)))
    }

    pub fn boxed_prefixed<S: AsyncIo + 'static>(prefix: Vec<u8>, stream: S) -> Self {
        Self::Boxed(PrefixedStream::new(prefix, Box::new(stream)))
    }
}

impl AsyncRead for TransportStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Unix(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tcp(s) => Pin::new(s).poll_read(cx, buf),
            Self::Mux(s) => Pin::new(s).poll_read(cx, buf),
            Self::Boxed(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for TransportStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Unix(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tcp(s) => Pin::new(s).poll_write(cx, buf),
            Self::Mux(s) => Pin::new(s).poll_write(cx, buf),
            Self::Boxed(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Unix(s) => Pin::new(s).poll_flush(cx),
            Self::Tcp(s) => Pin::new(s).poll_flush(cx),
            Self::Mux(s) => Pin::new(s).poll_flush(cx),
            Self::Boxed(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Unix(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
            Self::Mux(s) => Pin::new(s).poll_shutdown(cx),
            Self::Boxed(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

pub fn write_json_line<W: Write>(writer: &mut W, value: &Value) -> io::Result<()> {
    let mut buf = io::BufWriter::with_capacity(64 * 1024, writer);
    serde_json::to_writer(&mut buf, value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    buf.write_all(b"\n")?;
    buf.flush()
}

pub async fn write_json_line_async<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> io::Result<()> {
    let mut bytes =
        serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await
}

pub async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(None);
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            let take_len = pos + 1;
            if buf.len() + take_len > max_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "JSON-RPC frame exceeds 48 MiB limit",
                ));
            }
            buf.extend_from_slice(&available[..take_len]);
            reader.consume(take_len);
            let s = String::from_utf8(buf)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            return Ok(Some(s));
        }
        if buf.len() + available.len() > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JSON-RPC frame exceeds 48 MiB limit",
            ));
        }
        buf.extend_from_slice(available);
        let len = available.len();
        reader.consume(len);
    }
}
