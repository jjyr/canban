//! One control-plane subscription, independent of terminal output/viewer leases.
use super::*;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};

pub struct Subscription {
    pub core: String,
    pub machines: Value,
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
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
    )?;
    input.flush()?;
    let mut line = String::new();
    reader.read_line(&mut line)?;
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
                let machines = request(&mut *input, &mut reader, 3, "machine.list", json!({}))?;
                let snapshot = request(&mut *input, &mut reader, 4, "events.subscribe", json!({}))?;
                Ok(json!({"core":status["core"]["id"],"machines":machines,"snapshot":snapshot}))
            })();
            if setup.is_err() {
                let _ = tx.send(setup);
                return;
            }
            if tx.send(setup).is_err() {
                return;
            }
            loop {
                let mut line = String::new();
                let value = (|| -> Result<Value> {
                    ensure!(
                        reader.read_line(&mut line)? > 0,
                        "Herdr event connection closed"
                    );
                    ensure!(line.len() <= 48 * 1024 * 1024, "Herdr event too large");
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
            machines: setup["machines"].clone(),
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
