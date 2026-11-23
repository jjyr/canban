//! Generation-1 Herdr framed attachment. Closing this viewer never kills its PTY.
use super::*;
use std::{
    io::Read,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
pub enum Event {
    Data(Vec<u8>),
    Resize(u16, u16),
    Closed(Value),
    Lost(String),
}
pub struct Attachment {
    viewer: Option<crate::activity::Viewer>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    receiver: mpsc::Receiver<Event>,
    transport: super::transport::Transport,
    channel: u32,
    pub automatic_input: bool,
    pub prior_size: Option<(u16, u16)>,
    pub resized_on_attach: bool,
}
/// Viewer display colors and cell metrics reported to Herdr so display/cell-size
/// queries (`CSI 14 t`, `CSI 16 t`, OSC 10/11/12) are answered accurately.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayFacts {
    pub fg: Option<[u8; 3]>,
    pub bg: Option<[u8; 3]>,
    pub cursor: Option<[u8; 3]>,
    pub cell: Option<(u32, u32)>,
}

impl DisplayFacts {
    fn hex(rgb: [u8; 3]) -> String {
        format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
    }

    pub fn to_json(&self) -> Value {
        json!({
            "fg": self.fg.map(Self::hex),
            "bg": self.bg.map(Self::hex),
            "cursor": self.cursor.map(Self::hex),
            "cell": self.cell.map(|(w, h)| [w.max(1), h.max(1)]),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.fg.is_none() && self.bg.is_none() && self.cursor.is_none() && self.cell.is_none()
    }
}

/// A dedicated input writer lets human keystrokes bypass output receive waits.
#[derive(Clone)]
pub struct InputWriter {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    channel: u32,
    automatic_input: bool,
}
impl InputWriter {
    pub fn input(&self, data: &[u8]) -> Result<()> {
        frame(&mut *self.writer.lock().unwrap(), self.channel, 0, data)
    }
    pub fn automatic_input(&self, data: &[u8]) -> Result<()> {
        let kind = if self.automatic_input { 6 } else { 0 };
        frame(&mut *self.writer.lock().unwrap(), self.channel, kind, data)
    }
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let data = [cols.to_le_bytes(), rows.to_le_bytes()].concat();
        frame(&mut *self.writer.lock().unwrap(), self.channel, 3, &data)
    }
    pub fn focus(&self, on: bool) -> Result<()> {
        frame(
            &mut *self.writer.lock().unwrap(),
            self.channel,
            4,
            &[u8::from(on)],
        )
    }
    pub fn facts(&self, display: &DisplayFacts) -> Result<()> {
        let data = serde_json::to_vec(&display.to_json())?;
        frame(&mut *self.writer.lock().unwrap(), self.channel, 5, &data)
    }
}
fn encode_frame(
    writer: &mut (impl Write + ?Sized),
    channel: u32,
    kind: u8,
    data: &[u8],
) -> Result<()> {
    ensure!(data.len() <= 16 * 1024 * 1024, "Herdr frame too large");
    writer.write_all(&channel.to_le_bytes())?;
    writer.write_all(&[kind])?;
    writer.write_all(&(data.len() as u32).to_le_bytes())?;
    writer.write_all(data)?;
    Ok(())
}
fn frame(writer: &mut (impl Write + ?Sized), channel: u32, kind: u8, data: &[u8]) -> Result<()> {
    encode_frame(writer, channel, kind, data)?;
    writer.flush()?;
    Ok(())
}
fn read_frame(reader: &mut impl Read) -> Result<(u32, u8, Vec<u8>)> {
    let mut header = [0; 9];
    reader.read_exact(&mut header)?;
    let channel = u32::from_le_bytes(header[..4].try_into().unwrap());
    let len = u32::from_le_bytes(header[5..].try_into().unwrap()) as usize;
    ensure!(len <= 16 * 1024 * 1024, "Invalid Herdr frame length");
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    Ok((channel, header[4], body))
}
impl Client {
    pub fn attach(&self, terminal: &str, cols: u16, rows: u16) -> Result<Attachment> {
        self.attach_fenced(terminal, cols, rows, None)
    }

    pub fn attach_fenced(
        &self,
        terminal: &str,
        cols: u16,
        rows: u16,
        expected_core_id: Option<&str>,
    ) -> Result<Attachment> {
        self.attach_fenced_with_display(terminal, cols, rows, expected_core_id, None)
    }

    pub fn attach_fenced_with_display(
        &self,
        terminal: &str,
        cols: u16,
        rows: u16,
        expected_core_id: Option<&str>,
        display: Option<&DisplayFacts>,
    ) -> Result<Attachment> {
        let mut transport =
            super::transport::connect_view_fenced(&self.config, Some(terminal), expected_core_id)?;
        let setup = (|| -> Result<_> {
            let mut stdin = transport.writer.take().unwrap();
            let mut reader = BufReader::new(transport.reader.take().unwrap());
            {
                let mut writer = BufWriter::new(&mut *stdin);
                write_json_line(
                    &mut writer,
                    &json!({"jsonrpc":"2.0","id":1,"method":"hello","params":{"generation":1,"framed":true,"features":["automatic_input"]}}),
                )?;
            }
            let line = read_bounded_json_line(&mut reader, MAX_JSON_FRAME_BYTES)?;
            let hello: Value = serde_json::from_str(&line)?;
            ensure!(hello.get("error").is_none(), "Herdr handshake: {hello}");
            let supports_get = hello["result"]["methods"]
                .as_array()
                .is_some_and(|v| v.iter().any(|x| x == "terminal.get"));
            let mut attach_params =
                json!({"terminal":terminal,"cols":cols,"rows":rows,"history":2000});
            if let Some(d) = display.filter(|d| !d.is_empty()) {
                attach_params["display"] = d.to_json();
            }
            let attach_id: u64 = if supports_get {
                let get_call = json!({"jsonrpc":"2.0","id":2,"method":"terminal.get","params":{"terminal":terminal}});
                let attach_call = json!({"jsonrpc":"2.0","id":3,"method":"terminal.attach","params":attach_params});
                let mut batch = Vec::new();
                encode_frame(&mut batch, 0, 0, &serde_json::to_vec(&get_call)?)?;
                encode_frame(&mut batch, 0, 0, &serde_json::to_vec(&attach_call)?)?;
                stdin.write_all(&batch)?;
                stdin.flush()?;
                3
            } else {
                let call = json!({"jsonrpc":"2.0","id":2,"method":"terminal.attach","params":attach_params});
                frame(&mut stdin, 0, 0, &serde_json::to_vec(&call)?)?;
                2
            };
            let mut prior_size: Option<(u16, u16)> = None;
            let result = loop {
                let (channel, kind, data) = read_frame(&mut reader)?;
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
                result["channel"]
                    .as_u64()
                    .context("Herdr attach channel missing")?,
            )?;
            let auto = hello["result"]["features"]
                .as_array()
                .is_some_and(|v| v.iter().any(|x| x == "automatic_input"))
                && result["automatic_input"] == true;
            let resized_on_attach = prior_size.is_some_and(|(c, r)| c != cols || r != rows);
            Ok((stdin, reader, channel, auto, prior_size, resized_on_attach))
        })();
        let (stdin, mut reader, channel, automatic_input, prior_size, resized_on_attach) =
            match setup {
                Ok(v) => v,
                Err(e) => {
                    drop(transport);
                    return Err(e);
                }
            };
        let (tx, receiver) = mpsc::sync_channel(32);
        std::thread::spawn(move || {
            loop {
                match read_frame(&mut reader) {
                    Ok((c, kind, data)) if c == channel => {
                        let event = match kind {
                            0 => Event::Data(data),
                            2 => {
                                Event::Closed(serde_json::from_slice(&data).unwrap_or(Value::Null))
                            }
                            3 if data.len() == 4 => Event::Resize(
                                u16::from_le_bytes(data[..2].try_into().unwrap()),
                                u16::from_le_bytes(data[2..].try_into().unwrap()),
                            ),
                            _ => continue,
                        };
                        let closed = matches!(event, Event::Closed(_));
                        if tx.send(event).is_err() || closed {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        let _ = tx.send(Event::Lost(e.to_string()));
                        break;
                    }
                }
            }
        });
        Ok(Attachment {
            viewer: None,
            writer: Arc::new(Mutex::new(stdin)),
            receiver,
            transport,
            channel,
            automatic_input,
            prior_size,
            resized_on_attach,
        })
    }
}
impl Attachment {
    pub fn input_writer(&self) -> InputWriter {
        InputWriter {
            writer: self.writer.clone(),
            channel: self.channel,
            automatic_input: self.automatic_input,
        }
    }
    pub fn track_view(&mut self, core: &str, terminal: &str) {
        self.viewer = Some(crate::activity::Viewer::new(core, terminal));
    }
    pub fn recv(&self, timeout: Duration) -> Result<Option<Event>> {
        match self.receiver.recv_timeout(timeout) {
            Ok(v) => Ok(Some(v)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn input(&self, bytes: &[u8], automatic: bool) -> Result<()> {
        if !automatic && let Some(viewer) = &self.viewer {
            viewer.touch();
        }
        frame(
            &mut *self.writer.lock().unwrap(),
            self.channel,
            if automatic && self.automatic_input {
                6
            } else {
                0
            },
            bytes,
        )
    }
    pub fn consumed(&self, len: usize) -> Result<()> {
        frame(
            &mut *self.writer.lock().unwrap(),
            self.channel,
            1,
            &(len as u32).to_le_bytes(),
        )
    }
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let mut data = cols.to_le_bytes().to_vec();
        data.extend(rows.to_le_bytes());
        frame(&mut *self.writer.lock().unwrap(), self.channel, 3, &data)
    }
    pub fn focus(&self, on: bool) -> Result<()> {
        frame(
            &mut *self.writer.lock().unwrap(),
            self.channel,
            4,
            &[u8::from(on)],
        )
    }
    pub fn facts(&self, display: &DisplayFacts) -> Result<()> {
        let data = serde_json::to_vec(&display.to_json())?;
        frame(&mut *self.writer.lock().unwrap(), self.channel, 5, &data)
    }
}
impl Drop for Attachment {
    fn drop(&mut self) {
        let _ = frame(&mut *self.writer.lock().unwrap(), self.channel, 2, &[]);
        // Dropping the transport closes only this viewer/relay, never its PTY.
        let _ = &self.transport;
    }
}

/// Forward a viewer's handshake and framed input; only human operations renew its lease.
pub fn relay_input(
    source: impl Read,
    mut writer: impl Write,
    target: Option<(String, String)>,
) -> Result<()> {
    use std::io::{BufRead, BufReader};
    let mut reader = BufReader::new(source);
    let mut hello = Vec::new();
    reader.by_ref().take(65536).read_until(b'\n', &mut hello)?;
    ensure!(hello.last() == Some(&b'\n'), "Invalid viewer greeting");
    writer.write_all(&hello)?;
    writer.flush()?;
    loop {
        let mut header = [0; 9];
        if let Err(e) = reader.read_exact(&mut header) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(());
            }
            return Err(e.into());
        }
        let channel = u32::from_le_bytes(header[..4].try_into().unwrap());
        let length = u32::from_le_bytes(header[5..].try_into().unwrap()) as usize;
        ensure!(length <= 16 * 1024 * 1024, "Invalid viewer frame size");
        let mut data = vec![0; length];
        reader.read_exact(&mut data)?;
        if channel != 0
            && (header[4] == 0 && !data.is_empty() || header[4] == 4 && data == [1])
            && let Some((core, terminal)) = &target
        {
            crate::activity::touch(core, terminal);
        }
        writer.write_all(&header)?;
        writer.write_all(&data)?;
        writer.flush()?;
    }
}

#[cfg(test)]
mod relay_tests {
    use super::*;
    #[test]
    fn render_acknowledgments_do_not_refresh_user_activity_but_human_input_does() {
        let _lease = crate::activity::Viewer::new("relay-test", "terminal");
        let before = crate::activity::get("relay-test", "terminal").last_at;
        let target = Some(("relay-test".into(), "terminal".into()));
        std::thread::sleep(Duration::from_millis(2));
        let mut bytes = b"{\"method\":\"hello\"}\n".to_vec();
        frame(&mut bytes, 7, 1, &[0, 0, 0, 0]).unwrap();
        frame(&mut bytes, 7, 6, b"automatic reply").unwrap();
        let mut output = Vec::new();
        relay_input(bytes.as_slice(), &mut output, target.clone()).unwrap();
        assert_eq!(output, bytes);
        assert_eq!(
            crate::activity::get("relay-test", "terminal").last_at,
            before
        );
        let mut bytes = b"{\"method\":\"hello\"}\n".to_vec();
        frame(&mut bytes, 7, 0, b"human input").unwrap();
        relay_input(bytes.as_slice(), Vec::new(), target).unwrap();
        assert!(crate::activity::get("relay-test", "terminal").last_at > before);
    }

    #[test]
    fn attach_returns_unexpected_eof_on_truncated_hello_frame() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock-hc-stream-truncated");
        std::fs::write(
            &script,
            r#"#!/usr/bin/python3
import sys, json
line = sys.stdin.readline()
req = json.loads(line)
sys.stdout.write(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': {'features': []}}))
sys.stdout.flush()
sys.exit(0)
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let client = Client {
            config: Config {
                hc: script.to_string_lossy().into_owned(),
                hc_session: "stream-truncated-test".into(),
                ..Default::default()
            },
        };
        let err = match client.attach("t1", 80, 24) {
            Ok(_) => panic!("expected truncated hello to fail"),
            Err(err) => err,
        };
        assert_eq!(
            err.downcast_ref::<std::io::Error>().map(|e| e.kind()),
            Some(std::io::ErrorKind::UnexpectedEof),
            "expected UnexpectedEof, got: {err:#}"
        );
    }

    #[test]
    fn attach_pipelines_terminal_get_to_detect_prior_size_and_falls_back_when_unsupported() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let make_script = |name: &str, advertise_get: bool, prior_cols: u16, prior_rows: u16| {
            let script = dir.path().join(name);
            let methods = if advertise_get {
                r#"["hello", "terminal.get", "terminal.attach"]"#
            } else {
                r#"["hello", "terminal.attach"]"#
            };
            std::fs::write(
                &script,
                format!(
                    r#"#!/usr/bin/python3
import sys, json, struct

line = sys.stdin.readline()
hello = json.loads(line)
sys.stdout.write(json.dumps({{"jsonrpc": "2.0", "id": hello["id"], "result": {{"features": ["automatic_input"], "methods": {methods}}}}}) + "\n")
sys.stdout.flush()

def write_frame(ch, kind, data):
    sys.stdout.buffer.write(struct.pack("<IBI", ch, kind, len(data)) + data)
    sys.stdout.buffer.flush()

def read_frame():
    hdr = sys.stdin.buffer.read(9)
    if len(hdr) < 9:
        return None
    ch, kind, ln = struct.unpack("<IBI", hdr)
    return ch, kind, sys.stdin.buffer.read(ln)

while True:
    fr = read_frame()
    if not fr:
        break
    ch, kind, body = fr
    if ch == 0 and kind == 0:
        req = json.loads(body.decode())
        if req["method"] == "terminal.get":
            res = {{"address": req["params"]["terminal"], "cols": {prior_cols}, "rows": {prior_rows}}}
            write_frame(0, 0, json.dumps({{"jsonrpc": "2.0", "id": req["id"], "result": res}}).encode())
        elif req["method"] == "terminal.attach":
            res = {{"channel": 1, "automatic_input": True, "terminal": {{"cols": req["params"]["cols"], "rows": req["params"]["rows"]}}}}
            write_frame(0, 0, json.dumps({{"jsonrpc": "2.0", "id": req["id"], "result": res}}).encode())
            write_frame(1, 0, b"initial-frame")
    elif ch == 1 and kind == 2:
        break
"#
                ),
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            script
        };

        // 1. Mismatched prior size (200x120 -> 100x30) sets resized_on_attach = true.
        let s1 = make_script("hc-mismatch", true, 200, 120);
        let c1 = Client {
            config: Config {
                hc: s1.to_string_lossy().into_owned(),
                hc_session: "stream-get-mismatch".into(),
                ..Default::default()
            },
        };
        let att1 = c1.attach("t1", 100, 30).unwrap();
        assert_eq!(att1.prior_size, Some((200, 120)));
        assert!(att1.resized_on_attach);
        assert!(
            matches!(att1.recv(Duration::from_secs(1)).unwrap(), Some(Event::Data(d)) if d == b"initial-frame")
        );
        drop(att1);

        // 2. Matching prior size (100x30 -> 100x30) sets resized_on_attach = false.
        let s2 = make_script("hc-same", true, 100, 30);
        let c2 = Client {
            config: Config {
                hc: s2.to_string_lossy().into_owned(),
                hc_session: "stream-get-same".into(),
                ..Default::default()
            },
        };
        let att2 = c2.attach("t1", 100, 30).unwrap();
        assert_eq!(att2.prior_size, Some((100, 30)));
        assert!(!att2.resized_on_attach);
        drop(att2);

        // 3. Legacy server without terminal.get falls back cleanly without prior_size.
        let s3 = make_script("hc-legacy", false, 200, 120);
        let c3 = Client {
            config: Config {
                hc: s3.to_string_lossy().into_owned(),
                hc_session: "stream-get-legacy".into(),
                ..Default::default()
            },
        };
        let att3 = c3.attach("t1", 100, 30).unwrap();
        assert_eq!(att3.prior_size, None);
        assert!(!att3.resized_on_attach);
    }
}
