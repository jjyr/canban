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
}
/// A dedicated input writer lets human keystrokes bypass output receive waits.
#[derive(Clone)]
pub struct InputWriter {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    channel: u32,
}
impl InputWriter {
    pub fn input(&self, data: &[u8]) -> Result<()> {
        frame(&mut *self.writer.lock().unwrap(), self.channel, 0, data)
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
}
fn frame(writer: &mut (impl Write + ?Sized), channel: u32, kind: u8, data: &[u8]) -> Result<()> {
    ensure!(data.len() <= 16 * 1024 * 1024, "Herdr frame too large");
    writer.write_all(&channel.to_le_bytes())?;
    writer.write_all(&[kind])?;
    writer.write_all(&(data.len() as u32).to_le_bytes())?;
    writer.write_all(data)?;
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
        let mut transport =
            super::transport::connect_view_fenced(&self.config, Some(terminal), expected_core_id)?;
        let setup = (|| -> Result<_> {
            let mut stdin = transport.writer.take().unwrap();
            let mut reader = BufReader::new(transport.reader.take().unwrap());
            writeln!(
                stdin,
                "{}",
                json!({"jsonrpc":"2.0","id":1,"method":"hello","params":{"generation":1,"framed":true,"features":["automatic_input"]}})
            )?;
            stdin.flush()?;
            let mut line = String::new();
            reader.read_line(&mut line)?;
            let hello: Value = serde_json::from_str(&line)?;
            ensure!(hello.get("error").is_none(), "Herdr handshake: {hello}");
            let call = json!({"jsonrpc":"2.0","id":2,"method":"terminal.attach","params":{"terminal":terminal,"cols":cols,"rows":rows,"history":2000}});
            frame(&mut stdin, 0, 0, &serde_json::to_vec(&call)?)?;
            let result = loop {
                let (channel, kind, data) = read_frame(&mut reader)?;
                if channel == 0 && kind == 0 {
                    let value: Value = serde_json::from_slice(&data)?;
                    if value["id"] == 2 {
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
            Ok((stdin, reader, channel, auto))
        })();
        let (stdin, mut reader, channel, automatic_input) = match setup {
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
        })
    }
}
impl Attachment {
    pub fn input_writer(&self) -> InputWriter {
        InputWriter {
            writer: self.writer.clone(),
            channel: self.channel,
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
}
