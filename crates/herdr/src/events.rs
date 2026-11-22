//! One control-plane subscription, independent of terminal output/viewer leases.
use super::*;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};

pub struct Subscription {
    pub core: String,
    pub snapshot: Value,
    receiver: Receiver<Result<Value>>,
    _transport: transport::Transport,
}
fn request(
    input: &mut dyn Write,
    reader: &mut impl BufRead,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let mut writer = BufWriter::new(input);
    write_json_line(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )?;
    let line = read_bounded_json_line(reader, MAX_JSON_FRAME_BYTES)?;
    let reply: Value = serde_json::from_str(&line)?;
    ensure!(
        reply["id"] == id && reply.get("error").is_none(),
        "Herdr {method}: {reply}"
    );
    Ok(reply["result"].clone())
}
impl Client {
    pub fn subscribe(&self) -> Result<Subscription> {
        let mut transport = transport::connect(&self.config)?;
        let mut input = transport.writer.take().unwrap();
        let output = transport.reader.take().unwrap();
        // Setup and notifications share one reader. A bounded channel lets a slow
        // consumer reconnect for a fresh snapshot instead of growing an unbounded queue.
        let (tx, rx) = mpsc::sync_channel(256);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            let setup = (|| -> Result<Value> {
                request(
                    &mut *input,
                    &mut reader,
                    1,
                    "hello",
                    json!({"generation":1}),
                )?;
                let status = request(&mut *input, &mut reader, 2, "server.status", json!({}))?;
                let snapshot = request(&mut *input, &mut reader, 3, "events.subscribe", json!({}))?;
                Ok(json!({"core":status["core"]["id"],"snapshot":snapshot}))
            })();
            if setup.is_err() {
                let _ = tx.send(setup);
                return;
            }
            if tx.send(setup).is_err() {
                return;
            }
            loop {
                let value = (|| -> Result<Value> {
                    let line = read_bounded_json_line(&mut reader, MAX_JSON_FRAME_BYTES)?;
                    Ok(serde_json::from_str(&line)?)
                })();
                let failed = value.is_err();
                if tx.try_send(value).is_err() || failed {
                    break;
                }
            }
        });
        let setup = rx
            .recv_timeout(Duration::from_secs(12))
            .context("Herdr subscription setup timed out")??;
        Ok(Subscription {
            core: setup["core"]
                .as_str()
                .context("Core identity unavailable")?
                .into(),
            snapshot: setup["snapshot"].clone(),
            receiver: rx,
            _transport: transport,
        })
    }
}
impl Subscription {
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<Result<Value>, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn request_returns_unexpected_eof_on_truncated_or_empty_frame() {
        for payload in [
            b"".as_slice(),
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}",
        ] {
            let mut out = Vec::new();
            let mut reader = Cursor::new(payload);
            let err = request(&mut out, &mut reader, 1, "hello", json!({})).unwrap_err();
            assert_eq!(
                err.downcast_ref::<std::io::Error>().map(|e| e.kind()),
                Some(std::io::ErrorKind::UnexpectedEof),
                "expected UnexpectedEof for {payload:?}, got: {err:#}"
            );
        }
    }

    #[test]
    fn read_bounded_json_line_enforces_limit_and_newline() {
        let mut exact = Cursor::new(b"{\"a\":1}\n");
        assert_eq!(
            read_bounded_json_line(&mut exact, 8).unwrap(),
            "{\"a\":1}\n"
        );

        let mut oversized = Cursor::new(b"{\"a\":12345}\n");
        assert_eq!(
            read_bounded_json_line(&mut oversized, 8)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );

        let mut truncated = Cursor::new(b"{\"a\":1}");
        assert_eq!(
            read_bounded_json_line(&mut truncated, 8)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }
}
