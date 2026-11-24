//! Remote Herdr PTY transport speaking Generation-1 binary framing over `canban-transport`
//! (`yamux/1` sub-streams or authenticated TCP via `herdr.connect`), without local `hc`/`exec`.
use crate::session::{
    DisplayFacts, PtyAttachParams, PtyAttachment, PtyEvent, PtyTransport, PtyWriter,
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use canban_transport::{
    ClientTransport, MAX_FRAME_BYTES, TransportStream, read_bounded_line, write_json_line_async,
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf},
    sync::mpsc,
    task::AbortHandle,
};

const MAX_HERDR_FRAME_BYTES: usize = 16 * 1024 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

fn shared_remote_runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("canban-remote-pty")
            .build()
            .expect("build remote PTY Tokio runtime")
    })
}

fn encode_frame_into(buf: &mut Vec<u8>, channel: u32, kind: u8, payload: &[u8]) -> Result<()> {
    ensure!(
        payload.len() <= MAX_HERDR_FRAME_BYTES,
        "Herdr frame too large"
    );
    buf.extend_from_slice(&channel.to_le_bytes());
    buf.push(kind);
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(payload);
    Ok(())
}

async fn write_frame_async<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    channel: u32,
    kind: u8,
    payload: &[u8],
) -> Result<()> {
    ensure!(
        payload.len() <= MAX_HERDR_FRAME_BYTES,
        "Herdr frame too large"
    );
    let mut header = [0u8; 9];
    header[..4].copy_from_slice(&channel.to_le_bytes());
    header[4] = kind;
    header[5..].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    writer.write_all(&header).await?;
    if !payload.is_empty() {
        writer.write_all(payload).await?;
    }
    writer.flush().await?;
    Ok(())
}

async fn read_frame_async<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<(u32, u8, Vec<u8>)> {
    let mut header = [0u8; 9];
    reader.read_exact(&mut header).await?;
    let channel = u32::from_le_bytes(header[..4].try_into().unwrap());
    let len = u32::from_le_bytes(header[5..].try_into().unwrap()) as usize;
    ensure!(len <= MAX_HERDR_FRAME_BYTES, "Invalid Herdr frame length");
    let mut body = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut body).await?;
    }
    Ok((channel, header[4], body))
}

async fn connect_and_attach(
    transport: &ClientTransport,
    params: &PtyAttachParams,
) -> Result<(
    BufReader<ReadHalf<TransportStream>>,
    WriteHalf<TransportStream>,
    u32,
    bool,
    bool,
)> {
    let (stream, auth_token) = transport.open_async_stream().await?;
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);

    // `herdr.connect` MUST be the first frame with zero pipelined bytes.
    let mut hc_params = serde_json::Map::new();
    hc_params.insert("terminal".into(), json!(params.terminal));
    if !params.core_id.is_empty() {
        hc_params.insert("expected_core_id".into(), json!(params.core_id));
    }
    let mut connect_req = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "herdr.connect",
        "params": Value::Object(hc_params),
    });
    if let Some(token) = auth_token {
        connect_req["auth"] = json!(token);
    }
    write_json_line_async(&mut write_half, &connect_req).await?;

    let connect_line = read_bounded_line(&mut reader, MAX_FRAME_BYTES)
        .await?
        .context("Remote daemon closed connection during herdr.connect")?;
    let connect_reply: Value = serde_json::from_str(&connect_line)?;
    if let Some(err) = connect_reply.get("error") {
        let msg = err["message"].as_str().unwrap_or("herdr.connect rejected");
        bail!("{msg}");
    }
    ensure!(
        connect_reply["result"]["ready"] == true,
        "herdr.connect handshake failed: {connect_reply}"
    );

    // Generation-1 Herdr hello + framed upgrade on the relayed PTY socket.
    let hello_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "hello",
        "params": {
            "generation": 1,
            "framed": true,
            "features": ["automatic_input"],
        },
    });
    write_json_line_async(&mut write_half, &hello_req).await?;
    let hello_line = read_bounded_line(&mut reader, MAX_FRAME_BYTES)
        .await?
        .context("Remote daemon closed connection during Herdr handshake")?;
    let hello_reply: Value = serde_json::from_str(&hello_line)?;
    ensure!(
        hello_reply.get("error").is_none(),
        "Herdr handshake: {hello_reply}"
    );
    let hello_auto = hello_reply["result"]["features"]
        .as_array()
        .is_some_and(|v| v.iter().any(|x| x == "automatic_input"));
    let supports_get = hello_reply["result"]["methods"]
        .as_array()
        .is_some_and(|v| v.iter().any(|x| x == "terminal.get"));

    let mut attach_params = json!({
        "terminal": params.terminal,
        "cols": params.cols,
        "rows": params.rows,
        "history": 2000,
    });
    if !params.display.is_empty() {
        attach_params["display"] = params.display.to_json();
    }
    let attach_id: u64 = if supports_get {
        let get_req = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "terminal.get",
            "params": { "terminal": params.terminal },
        });
        let attach_req = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "terminal.attach",
            "params": attach_params,
        });
        let mut batch = Vec::new();
        encode_frame_into(&mut batch, 0, 0, &serde_json::to_vec(&get_req)?)?;
        encode_frame_into(&mut batch, 0, 0, &serde_json::to_vec(&attach_req)?)?;
        write_half.write_all(&batch).await?;
        write_half.flush().await?;
        3
    } else {
        let attach_req = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "terminal.attach",
            "params": attach_params,
        });
        write_frame_async(&mut write_half, 0, 0, &serde_json::to_vec(&attach_req)?).await?;
        2
    };

    let mut prior_size: Option<(u16, u16)> = None;
    let attach_result = loop {
        let (channel, kind, data) = read_frame_async(&mut reader).await?;
        if channel == 0 && kind == 0 {
            let value: Value = serde_json::from_slice(&data)?;
            if supports_get && value["id"] == 2 {
                if value.get("error").is_none()
                    && let (Some(c), Some(r)) = (
                        value["result"]["cols"]
                            .as_u64()
                            .and_then(|v| u16::try_from(v).ok()),
                        value["result"]["rows"]
                            .as_u64()
                            .and_then(|v| u16::try_from(v).ok()),
                    )
                    && c > 0
                    && r > 0
                {
                    prior_size = Some((c, r));
                }
                continue;
            }
            if value["id"] == attach_id {
                ensure!(value.get("error").is_none(), "Herdr attach: {value}");
                break value["result"].clone();
            }
        }
    };
    let channel = u32::try_from(
        attach_result["channel"]
            .as_u64()
            .context("Herdr attach channel missing")?,
    )?;
    let automatic_input = hello_auto && attach_result["automatic_input"] == true;
    let resized_on_attach = prior_size.is_some_and(|(c, r)| c != params.cols || r != params.rows);
    Ok((
        reader,
        write_half,
        channel,
        automatic_input,
        resized_on_attach,
    ))
}

enum WriteCommand {
    Frame { kind: u8, payload: Vec<u8> },
    Detach,
}

struct RemoteWriter {
    tx: mpsc::UnboundedSender<WriteCommand>,
    automatic_input: bool,
    detached: AtomicBool,
    error: Arc<Mutex<Option<String>>>,
}

impl RemoteWriter {
    fn send_frame(&self, kind: u8, payload: Vec<u8>) -> Result<()> {
        if let Some(err) = self.error.lock().unwrap().as_ref() {
            bail!("{err}");
        }
        self.tx
            .send(WriteCommand::Frame { kind, payload })
            .map_err(|_| anyhow!("Remote PTY stream writer closed"))
    }

    fn consumed(&self, len: usize) -> Result<()> {
        let clamped = len.min(u32::MAX as usize) as u32;
        self.send_frame(1, clamped.to_le_bytes().to_vec())
    }

    fn send_detach(&self) {
        if !self.detached.swap(true, Ordering::SeqCst) {
            let _ = self.tx.send(WriteCommand::Detach);
        }
    }
}

impl PtyWriter for RemoteWriter {
    fn write(&self, bytes: &[u8]) -> Result<()> {
        self.send_frame(0, bytes.to_vec())
    }

    fn write_auto(&self, bytes: &[u8]) -> Result<()> {
        let kind = if self.automatic_input { 6 } else { 0 };
        self.send_frame(kind, bytes.to_vec())
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let mut data = Vec::with_capacity(4);
        data.extend_from_slice(&cols.to_le_bytes());
        data.extend_from_slice(&rows.to_le_bytes());
        self.send_frame(3, data)
    }

    fn focus(&self, focused: bool) -> Result<()> {
        self.send_frame(4, vec![u8::from(focused)])
    }

    fn facts(&self, display: &DisplayFacts) -> Result<()> {
        self.send_frame(5, serde_json::to_vec(&display.to_json())?)
    }
}

struct RemoteAttachment {
    writer: Arc<RemoteWriter>,
    event_rx: mpsc::Receiver<PtyEvent>,
    runtime: tokio::runtime::Handle,
    reader_abort: AbortHandle,
    resized_on_attach: bool,
}

impl PtyAttachment for RemoteAttachment {
    fn writer(&self) -> Arc<dyn PtyWriter> {
        self.writer.clone()
    }

    fn recv(&mut self, timeout: Duration) -> Result<Option<PtyEvent>> {
        match self.event_rx.try_recv() {
            Ok(ev) => return Ok(Some(ev)),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                bail!("Remote PTY stream receiver disconnected");
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
        }
        if timeout.is_zero() {
            return Ok(None);
        }
        let Ok(res) = self
            .runtime
            .block_on(async { tokio::time::timeout(timeout, self.event_rx.recv()).await })
        else {
            return Ok(None);
        };
        res.map(Some)
            .ok_or_else(|| anyhow!("Remote PTY stream receiver disconnected"))
    }

    fn consumed(&mut self, len: usize) -> Result<()> {
        self.writer.consumed(len)
    }

    fn resized_on_attach(&self) -> bool {
        self.resized_on_attach
    }

    fn detach(self: Box<Self>) {
        // Explicit detach delegates to `Drop`, which sends the Herdr detach frame cleanly.
    }
}

impl Drop for RemoteAttachment {
    fn drop(&mut self) {
        self.writer.send_detach();
        self.reader_abort.abort();
    }
}

/// [`PtyTransport`] backed by [`canban_transport::ClientTransport`] (`yamux/1` sub-streams
/// or direct TCP + per-device bearer token) using `herdr.connect`.
pub struct RemotePtyTransport {
    transport: Arc<ClientTransport>,
    runtime: tokio::runtime::Handle,
}

impl RemotePtyTransport {
    /// Creates a transport executing stream handshakes and I/O tasks on `runtime`.
    pub fn new(transport: Arc<ClientTransport>, runtime: tokio::runtime::Handle) -> Self {
        Self { transport, runtime }
    }

    /// Creates a transport using Canban's shared remote-PTY Tokio runtime.
    pub fn with_transport(transport: Arc<ClientTransport>) -> Result<Self> {
        Ok(Self {
            transport,
            runtime: shared_remote_runtime().handle().clone(),
        })
    }

    /// Exposes the underlying [`ClientTransport`].
    pub fn client_transport(&self) -> &Arc<ClientTransport> {
        &self.transport
    }
}

impl PtyTransport for RemotePtyTransport {
    fn attach(&self, params: &PtyAttachParams) -> Result<Box<dyn PtyAttachment>> {
        let transport = self.transport.clone();
        let params = params.clone();
        let runtime = self.runtime.clone();

        let connect_fut = async move {
            tokio::time::timeout(HANDSHAKE_TIMEOUT, connect_and_attach(&transport, &params))
                .await
                .map_err(|_| anyhow!("Timed out attaching to remote PTY {}", params.terminal))?
        };
        let (mut reader, mut write_half, channel, automatic_input, resized_on_attach) =
            if tokio::runtime::Handle::try_current().is_ok() {
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                runtime.spawn(async move {
                    let _ = tx.send(connect_fut.await);
                });
                rx.recv()
                    .map_err(|_| anyhow!("Remote PTY attach task aborted"))??
            } else {
                runtime.block_on(connect_fut)?
            };

        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<WriteCommand>();
        let write_error = Arc::new(Mutex::new(None::<String>));
        let writer_err = write_error.clone();
        runtime.spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    WriteCommand::Frame { kind, payload } => {
                        if let Err(e) =
                            write_frame_async(&mut write_half, channel, kind, &payload).await
                        {
                            *writer_err.lock().unwrap() = Some(e.to_string());
                            break;
                        }
                    }
                    WriteCommand::Detach => {
                        let _ = write_frame_async(&mut write_half, channel, 2, &[]).await;
                        break;
                    }
                }
            }
            let _ = write_half.shutdown().await;
        });

        let (event_tx, event_rx) = mpsc::channel(64);
        let reader_handle = runtime.spawn(async move {
            loop {
                match read_frame_async(&mut reader).await {
                    Ok((c, kind, data)) if c == channel => {
                        let event = match kind {
                            0 => PtyEvent::Data(data),
                            2 => PtyEvent::Closed(
                                serde_json::from_slice(&data).unwrap_or(Value::Null),
                            ),
                            3 if data.len() == 4 => PtyEvent::Resize(
                                u16::from_le_bytes(data[..2].try_into().unwrap()),
                                u16::from_le_bytes(data[2..].try_into().unwrap()),
                            ),
                            _ => continue,
                        };
                        let closed = matches!(event, PtyEvent::Closed(_));
                        if event_tx.send(event).await.is_err() || closed {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        let _ = event_tx.send(PtyEvent::Lost(e.to_string())).await;
                        break;
                    }
                }
            }
        });

        let writer = Arc::new(RemoteWriter {
            tx: cmd_tx,
            automatic_input,
            detached: AtomicBool::new(false),
            error: write_error,
        });

        Ok(Box::new(RemoteAttachment {
            writer,
            event_rx,
            runtime,
            reader_abort: reader_handle.abort_handle(),
            resized_on_attach,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionInit, evict_session, get_or_spawn_session};
    use canban_transport::{
        AcceptedStream, ClientEndpoint, DeviceTokenStore, PairingCodeStore, RevocationBus,
        ServerTransportConfig, TailscaleSessionRegistry, spawn_tcp_listener,
    };
    use canban_types::Config;
    use std::sync::atomic::AtomicUsize;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct RecordedFrame {
        terminal: String,
        kind: u8,
        payload: Vec<u8>,
    }

    struct MockDaemon {
        addr: String,
        token: String,
        stop_tx: tokio::sync::watch::Sender<bool>,
        active_tcp: Arc<AtomicUsize>,
        frames_rx: Mutex<Option<mpsc::UnboundedReceiver<RecordedFrame>>>,
        auth_tokens_seen: Arc<Mutex<Vec<Option<String>>>>,
        _home: tempfile::TempDir,
    }

    impl Drop for MockDaemon {
        fn drop(&mut self) {
            let _ = self.stop_tx.send(true);
        }
    }

    fn start_mock_daemon(expected_core: &str, reject_mismatch: bool) -> MockDaemon {
        start_mock_daemon_with_get(expected_core, reject_mismatch, None)
    }

    fn start_mock_daemon_with_get(
        expected_core: &str,
        reject_mismatch: bool,
        prior_size: Option<(u16, u16)>,
    ) -> MockDaemon {
        let rt = shared_remote_runtime().handle().clone();
        let home = tempfile::tempdir().unwrap();
        let store = Arc::new(DeviceTokenStore::open(home.path()));
        let (_dev, token) = store.issue_device("test-device").unwrap();
        let pairing_codes = Arc::new(PairingCodeStore::open(home.path()));
        let revocations = RevocationBus::new();
        let active_tcp = Arc::new(AtomicUsize::new(0));
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let (frames_tx, frames_rx) = mpsc::unbounded_channel();
        let auth_tokens_seen = Arc::new(Mutex::new(Vec::new()));
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let cfg = Arc::new(ServerTransportConfig {
            home: home.path().to_path_buf(),
            server_id: "srv-remote-pty-test".into(),
            api_version: 1,
            device_store: store,
            pairing_codes,
            revocations,
            active_tcp_connections: active_tcp.clone(),
            tailscale_verifier: None,
            tailscale_devices: Arc::new(TailscaleSessionRegistry::new()),
        });

        let expected_core = expected_core.to_owned();
        let ftx = frames_tx.clone();
        let tokens_seen = auth_tokens_seen.clone();
        spawn_tcp_listener(
            &rt,
            listener,
            cfg,
            stop_rx,
            move |accepted: AcceptedStream| {
                let expected_core = expected_core.clone();
                let ftx = ftx.clone();
                let tokens_seen = tokens_seen.clone();
                async move {
                    let _ = handle_mock_stream(
                        accepted,
                        expected_core,
                        reject_mismatch,
                        prior_size,
                        ftx,
                        tokens_seen,
                    )
                    .await;
                }
            },
        );

        MockDaemon {
            addr,
            token,
            stop_tx,
            active_tcp,
            frames_rx: Mutex::new(Some(frames_rx)),
            auth_tokens_seen,
            _home: home,
        }
    }

    async fn handle_mock_stream(
        accepted: AcceptedStream,
        expected_core: String,
        reject_mismatch: bool,
        prior_size: Option<(u16, u16)>,
        frames_tx: mpsc::UnboundedSender<RecordedFrame>,
        auth_tokens_seen: Arc<Mutex<Vec<Option<String>>>>,
    ) -> Result<()> {
        let (read_half, mut writer) = tokio::io::split(accepted.stream);
        let mut reader = BufReader::new(read_half);

        let connect_val: Value = match accepted.initial_request {
            Some(v) => {
                ensure!(!accepted.has_pipelined_bytes, "unexpected pipelined bytes");
                v
            }
            None => {
                let line = read_bounded_line(&mut reader, MAX_FRAME_BYTES)
                    .await?
                    .context("EOF before herdr.connect")?;
                serde_json::from_str(&line)?
            }
        };
        auth_tokens_seen
            .lock()
            .unwrap()
            .push(connect_val["auth"].as_str().map(str::to_owned));

        ensure!(connect_val["method"] == "herdr.connect");
        let terminal = connect_val["params"]["terminal"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        if reject_mismatch
            && let Some(exp) = connect_val["params"]["expected_core_id"].as_str()
            && exp != expected_core
        {
            write_json_line_async(
                &mut writer,
                &json!({"jsonrpc":"2.0","id":0,"error":{"code":-32000,"message":"Original core identity changed"}}),
            )
            .await?;
            return Ok(());
        }
        write_json_line_async(&mut writer, &json!({"result":{"ready":true}})).await?;

        let hello_line = read_bounded_line(&mut reader, MAX_FRAME_BYTES)
            .await?
            .context("EOF before hello")?;
        let hello: Value = serde_json::from_str(&hello_line)?;
        ensure!(hello["method"] == "hello");
        let methods = if prior_size.is_some() {
            json!(["hello", "terminal.get", "terminal.attach"])
        } else {
            json!(["hello", "terminal.attach"])
        };
        write_json_line_async(
            &mut writer,
            &json!({"jsonrpc":"2.0","id":hello["id"],"result":{"generation":1,"framed":true,"features":["automatic_input"],"methods":methods}}),
        )
        .await?;

        let channel: u32 = 7;
        loop {
            let (ch0, kind0, req_bytes) = read_frame_async(&mut reader).await?;
            ensure!(ch0 == 0 && kind0 == 0);
            let req: Value = serde_json::from_slice(&req_bytes)?;
            if req["method"] == "terminal.get" {
                let (c, r) = prior_size.context("unexpected terminal.get")?;
                let get_reply = json!({
                    "jsonrpc": "2.0",
                    "id": req["id"],
                    "result": {
                        "address": req["params"]["terminal"],
                        "cols": c,
                        "rows": r,
                    }
                });
                write_frame_async(&mut writer, 0, 0, &serde_json::to_vec(&get_reply)?).await?;
                continue;
            }
            ensure!(req["method"] == "terminal.attach");
            let attach_reply = json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": {
                    "channel": channel,
                    "automatic_input": true,
                }
            });
            write_frame_async(&mut writer, 0, 0, &serde_json::to_vec(&attach_reply)?).await?;
            break;
        }

        // Emit initial Resize + Data banner so attach clients immediately see both events.
        let mut rsz = Vec::with_capacity(4);
        rsz.extend_from_slice(&120u16.to_le_bytes());
        rsz.extend_from_slice(&36u16.to_le_bytes());
        write_frame_async(&mut writer, channel, 3, &rsz).await?;
        let banner = format!("banner:{terminal}\r\n");
        write_frame_async(&mut writer, channel, 0, banner.as_bytes()).await?;

        while let Ok((c, kind, payload)) = read_frame_async(&mut reader).await {
            if c != channel {
                continue;
            }
            let _ = frames_tx.send(RecordedFrame {
                terminal: terminal.clone(),
                kind,
                payload: payload.clone(),
            });
            if kind == 2 {
                break;
            }
            if kind == 0 && payload == b"close-me" {
                write_frame_async(
                    &mut writer,
                    channel,
                    2,
                    &serde_json::to_vec(&json!({"reason": "exited"}))?,
                )
                .await?;
                break;
            }
            if kind == 0 {
                let echoed = format!("echo:{}:{}", terminal, String::from_utf8_lossy(&payload));
                write_frame_async(&mut writer, channel, 0, echoed.as_bytes()).await?;
            }
        }
        Ok(())
    }

    fn recv_matching<F>(rx: &mut mpsc::UnboundedReceiver<RecordedFrame>, pred: F) -> RecordedFrame
    where
        F: Fn(&RecordedFrame) -> bool,
    {
        let rt = shared_remote_runtime().handle();
        let dl = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let now = std::time::Instant::now();
            assert!(now < dl, "timed out waiting for recorded Herdr frame");
            let remaining = dl - now;
            if let Ok(Some(frame)) =
                rt.block_on(async { tokio::time::timeout(remaining, rx.recv()).await })
                && pred(&frame)
            {
                return frame;
            }
        }
    }

    #[test]
    fn remote_pty_transport_handles_mux_frames_facts_auto_input_and_clean_detach() {
        let daemon = start_mock_daemon("core-remote-1", true);
        let mut rx = daemon.frames_rx.lock().unwrap().take().unwrap();
        let client = Arc::new(
            ClientTransport::connect_tcp(
                daemon.addr.clone(),
                daemon.token.clone(),
                "ios-test".into(),
            )
            .unwrap(),
        );
        assert!(client.is_mux());
        let remote = RemotePtyTransport::with_transport(client.clone()).unwrap();

        let mut attach = remote
            .attach(&PtyAttachParams {
                core_id: "core-remote-1".into(),
                terminal: "tty-1".into(),
                cols: 90,
                rows: 30,
                display: DisplayFacts {
                    fg: Some([220, 224, 230]),
                    bg: Some([18, 20, 26]),
                    cursor: Some([90, 200, 250]),
                    cell: Some((9, 18)),
                },
            })
            .unwrap();

        let mut saw_resize = false;
        let mut saw_banner = false;
        for _ in 0..20 {
            if let Some(ev) = attach.recv(Duration::from_millis(200)).unwrap() {
                match ev {
                    PtyEvent::Resize(120, 36) => saw_resize = true,
                    PtyEvent::Data(bytes) => {
                        attach.consumed(bytes.len()).unwrap();
                        if bytes == b"banner:tty-1\r\n" {
                            saw_banner = true;
                        }
                    }
                    other => panic!("unexpected initial event: {other:?}"),
                }
            }
            if saw_resize && saw_banner {
                break;
            }
        }
        assert!(saw_resize && saw_banner);

        let writer = attach.writer();
        writer.write(b"hi").unwrap();
        writer.write_auto(b"\x1b[?1;2c").unwrap();
        writer.resize(100, 40).unwrap();
        writer.focus(true).unwrap();
        writer
            .facts(&DisplayFacts {
                fg: Some([1, 2, 3]),
                bg: None,
                cursor: None,
                cell: Some((8, 16)),
            })
            .unwrap();

        let f_consumed = recv_matching(&mut rx, |f| f.kind == 1);
        assert_eq!(
            u32::from_le_bytes(f_consumed.payload.try_into().unwrap()),
            "banner:tty-1\r\n".len() as u32
        );
        let f_input = recv_matching(&mut rx, |f| f.kind == 0 && f.payload == b"hi");
        assert_eq!(f_input.terminal, "tty-1");
        let f_auto = recv_matching(&mut rx, |f| f.kind == 6);
        assert_eq!(f_auto.payload, b"\x1b[?1;2c");
        let f_resize = recv_matching(&mut rx, |f| f.kind == 3);
        assert_eq!(
            (
                u16::from_le_bytes(f_resize.payload[..2].try_into().unwrap()),
                u16::from_le_bytes(f_resize.payload[2..].try_into().unwrap()),
            ),
            (100, 40)
        );
        let f_focus = recv_matching(&mut rx, |f| f.kind == 4);
        assert_eq!(f_focus.payload, vec![1]);
        let f_facts = recv_matching(&mut rx, |f| f.kind == 5);
        let facts_json: Value = serde_json::from_slice(&f_facts.payload).unwrap();
        assert_eq!(facts_json["fg"], "#010203");
        assert_eq!(facts_json["cell"], json!([8, 16]));

        // Check echo reply arrived.
        let mut saw_echo = false;
        for _ in 0..20 {
            if let Some(PtyEvent::Data(data)) = attach.recv(Duration::from_millis(200)).unwrap()
                && data == b"echo:tty-1:hi"
            {
                saw_echo = true;
                break;
            }
        }
        assert!(saw_echo);

        // Clean detach sends kind=2 and shuts down the sub-stream without breaking TCP mux.
        drop(attach);
        let f_detach = recv_matching(&mut rx, |f| f.kind == 2);
        assert_eq!(f_detach.terminal, "tty-1");
        assert_eq!(daemon.active_tcp.load(Ordering::SeqCst), 1);

        // Server-initiated close frame also maps cleanly to PtyEvent::Closed.
        let mut attach2 = remote
            .attach(&PtyAttachParams {
                core_id: "core-remote-1".into(),
                terminal: "tty-close".into(),
                cols: 80,
                rows: 24,
                display: DisplayFacts::default(),
            })
            .unwrap();
        attach2.write(b"close-me", false).unwrap();
        let mut saw_closed = false;
        for _ in 0..20 {
            if let Some(PtyEvent::Closed(v)) = attach2.recv(Duration::from_millis(200)).unwrap() {
                assert_eq!(v["reason"], "exited");
                saw_closed = true;
                break;
            }
        }
        assert!(saw_closed);
    }

    #[test]
    fn remote_pty_transport_concurrent_mux_streams_and_error_and_direct_tcp() {
        let daemon = start_mock_daemon("core-conc", true);
        let client = Arc::new(
            ClientTransport::connect_tcp(
                daemon.addr.clone(),
                daemon.token.clone(),
                "ios-conc".into(),
            )
            .unwrap(),
        );
        let remote = Arc::new(RemotePtyTransport::with_transport(client.clone()).unwrap());

        // Core ID mismatch propagates server error clearly.
        let err = remote
            .attach(&PtyAttachParams {
                core_id: "wrong-core".into(),
                terminal: "tty-err".into(),
                cols: 80,
                rows: 24,
                display: DisplayFacts::default(),
            })
            .err()
            .expect("mismatch must error");
        assert!(
            err.to_string().contains("Original core identity changed"),
            "unexpected error: {err:#}"
        );

        // Two concurrent mux streams + closing #1 leaves #2 completely functional on the single TCP conn.
        let mut a1 = remote
            .attach(&PtyAttachParams {
                core_id: "core-conc".into(),
                terminal: "tty-a".into(),
                cols: 80,
                rows: 24,
                display: DisplayFacts::default(),
            })
            .unwrap();
        let mut a2 = remote
            .attach(&PtyAttachParams {
                core_id: "core-conc".into(),
                terminal: "tty-b".into(),
                cols: 80,
                rows: 24,
                display: DisplayFacts::default(),
            })
            .unwrap();
        assert_eq!(daemon.active_tcp.load(Ordering::SeqCst), 1);

        drop(a1.recv(Duration::from_millis(200)));
        drop(a1);

        a2.write(b"still-alive", false).unwrap();
        let mut got_a2 = false;
        for _ in 0..20 {
            if let Some(PtyEvent::Data(data)) = a2.recv(Duration::from_millis(200)).unwrap()
                && data == b"echo:tty-b:still-alive"
            {
                got_a2 = true;
                break;
            }
        }
        assert!(got_a2);
        assert_eq!(daemon.active_tcp.load(Ordering::SeqCst), 1);
        drop(a2);

        // Direct TCP (non-mux) includes bearer token on `herdr.connect`.
        let direct_client = Arc::new(
            ClientTransport::connect_direct(ClientEndpoint::Tcp {
                addr: daemon.addr.clone(),
                token: daemon.token.clone(),
            })
            .unwrap(),
        );
        assert!(!direct_client.is_mux());
        let direct_remote = RemotePtyTransport::with_transport(direct_client).unwrap();
        let mut direct_attach = direct_remote
            .attach(&PtyAttachParams {
                core_id: "core-conc".into(),
                terminal: "tty-direct".into(),
                cols: 80,
                rows: 24,
                display: DisplayFacts::default(),
            })
            .unwrap();
        direct_attach.write(b"ping", false).unwrap();
        let mut saw_direct_echo = false;
        for _ in 0..20 {
            if let Some(PtyEvent::Data(data)) =
                direct_attach.recv(Duration::from_millis(200)).unwrap()
                && data == b"echo:tty-direct:ping"
            {
                saw_direct_echo = true;
                break;
            }
        }
        assert!(saw_direct_echo);
        assert!(
            daemon
                .auth_tokens_seen
                .lock()
                .unwrap()
                .iter()
                .any(|t| t.as_deref() == Some(daemon.token.as_str()))
        );
        drop(direct_attach);

        // End-to-end LiveSession integration using RemotePtyTransport.
        let session = get_or_spawn_session(SessionInit {
            config: Config::default(),
            core: "core-conc".into(),
            terminal: "tty-session".into(),
            initial_cols: 80,
            initial_rows: 24,
            initial_cell: (8, 16),
            size_ready: true,
            focused: true,
            theme: None,
            transport: Some(remote),
        });
        for _ in 0..80 {
            if session
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("banner:tty-session")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            session
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("banner:tty-session")
        );
        evict_session("core-conc", "tty-session");
    }

    #[test]
    fn remote_pty_transport_pipelines_terminal_get_and_tracks_resized_on_attach() {
        let params_90x30 = PtyAttachParams {
            core_id: "core-get".into(),
            terminal: "tty-get".into(),
            cols: 90,
            rows: 30,
            display: DisplayFacts::default(),
        };

        // 1. Mismatched prior size (200x120 -> 90x30) sets resized_on_attach() = true.
        let daemon_mismatch = start_mock_daemon_with_get("core-get", true, Some((200, 120)));
        let client_mismatch = Arc::new(
            ClientTransport::connect_tcp(
                daemon_mismatch.addr.clone(),
                daemon_mismatch.token.clone(),
                "ios-get".into(),
            )
            .unwrap(),
        );
        let remote_mismatch = RemotePtyTransport::with_transport(client_mismatch).unwrap();
        let att_mismatch = remote_mismatch.attach(&params_90x30).unwrap();
        assert!(att_mismatch.resized_on_attach());
        drop(att_mismatch);

        // 2. Matching prior size (90x30 -> 90x30) sets resized_on_attach() = false.
        let daemon_same = start_mock_daemon_with_get("core-get", true, Some((90, 30)));
        let client_same = Arc::new(
            ClientTransport::connect_tcp(
                daemon_same.addr.clone(),
                daemon_same.token.clone(),
                "ios-get".into(),
            )
            .unwrap(),
        );
        let remote_same = RemotePtyTransport::with_transport(client_same).unwrap();
        let att_same = remote_same.attach(&params_90x30).unwrap();
        assert!(!att_same.resized_on_attach());
        drop(att_same);

        // 3. Legacy server without terminal.get advertised falls back with resized_on_attach() = false.
        let daemon_legacy = start_mock_daemon_with_get("core-get", true, None);
        let client_legacy = Arc::new(
            ClientTransport::connect_tcp(
                daemon_legacy.addr.clone(),
                daemon_legacy.token.clone(),
                "ios-get".into(),
            )
            .unwrap(),
        );
        let remote_legacy = RemotePtyTransport::with_transport(client_legacy).unwrap();
        let att_legacy = remote_legacy.attach(&params_90x30).unwrap();
        assert!(!att_legacy.resized_on_attach());
        drop(att_legacy);
    }
}
