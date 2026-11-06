//! Generation-1 Herdr framed attachment. Closing this viewer never kills its PTY.
use super::*;
use std::{
    io::Read,
    process::{Child, ChildStdin},
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
    writer: Arc<Mutex<ChildStdin>>,
    receiver: mpsc::Receiver<Event>,
    child: Child,
    channel: u32,
    pub automatic_input: bool,
}
fn frame(writer: &mut impl Write, channel: u32, kind: u8, data: &[u8]) -> Result<()> {
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
        let mut child = Command::new(&self.config.hc)
            .arg("connect")
            .env("HC_SESSION", &self.config.hc_session)
            .envs(&self.config.hc_env)
            .env_remove("HC_TERMINAL_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let setup = (|| -> Result<_> {
            let mut stdin = child.stdin.take().unwrap();
            let mut reader = BufReader::new(child.stdout.take().unwrap());
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
                let _ = child.kill();
                let _ = child.wait();
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
            writer: Arc::new(Mutex::new(stdin)),
            receiver,
            child,
            channel,
            automatic_input,
        })
    }
}
impl Attachment {
    pub fn recv(&self, timeout: Duration) -> Result<Option<Event>> {
        match self.receiver.recv_timeout(timeout) {
            Ok(v) => Ok(Some(v)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn input(&self, bytes: &[u8], automatic: bool) -> Result<()> {
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
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
