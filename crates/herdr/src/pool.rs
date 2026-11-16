//! Persistent JSON-RPC control connection pool for `hc connect` and relay sockets.
use super::*;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::Instant,
};

const MAX_IDLE_PER_KEY: usize = 4;
const IDLE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    hc: String,
    hc_session: String,
    hc_env: Vec<(String, String)>,
    relay_endpoint: Option<String>,
}

impl PoolKey {
    fn from_config(config: &Config) -> Self {
        Self {
            hc: config.hc.clone(),
            hc_session: config.hc_session.clone(),
            hc_env: config
                .hc_env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            relay_endpoint: config.relay_endpoint.clone(),
        }
    }
}

struct PooledConn {
    transport: transport::Transport,
    writer: Box<dyn Write + Send>,
    replies: Receiver<Result<Value>>,
    closed: Arc<AtomicBool>,
    next_id: u64,
    last_used: Instant,
}

impl PooledConn {
    fn connect(config: &Config) -> Result<Self> {
        let mut transport = transport::connect(config)?;
        let mut writer = transport.writer.take().unwrap();
        let output = transport.reader.take().unwrap();
        let closed = Arc::new(AtomicBool::new(false));
        let reader_closed = closed.clone();
        let (tx, replies) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        reader_closed.store(true, Ordering::SeqCst);
                        let _ = tx.send(Err(anyhow::anyhow!("Herdr disconnected before replying")));
                        break;
                    }
                    Ok(_) => {
                        if let Ok(value) = serde_json::from_str::<Value>(&line)
                            && value.get("id").is_some()
                            && tx.send(Ok(value)).is_err()
                        {
                            break;
                        }
                    }
                    Err(e) => {
                        reader_closed.store(true, Ordering::SeqCst);
                        let _ = tx.send(Err(e.into()));
                        break;
                    }
                }
            }
        });
        writeln!(
            writer,
            "{}",
            json!({"jsonrpc":"2.0","id":1,"method":"hello","params":{"generation":1}})
        )?;
        writer.flush()?;
        let hello = replies
            .recv_timeout(Duration::from_secs(12))
            .context("Herdr handshake timed out")??;
        ensure!(
            hello["id"] == 1 && hello.get("error").is_none(),
            "Herdr handshake: {hello}"
        );
        Ok(Self {
            transport,
            writer,
            replies,
            closed,
            next_id: 2,
            last_used: Instant::now(),
        })
    }

    fn is_healthy(&mut self) -> bool {
        if self.last_used.elapsed() >= IDLE_TTL || self.closed.load(Ordering::SeqCst) {
            return false;
        }
        if self.transport.child_exited() {
            return false;
        }
        // Drain any unexpected/stray replies; if the reader channel disconnected or errored, evict.
        loop {
            match self.replies.try_recv() {
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => return false,
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        true
    }

    fn send_request(&mut self, method: &str, params: &Value) -> std::io::Result<u64> {
        let req_id = self.next_id;
        self.next_id += 1;
        writeln!(
            self.writer,
            "{}",
            json!({"jsonrpc":"2.0","id":req_id,"method":method,"params":params})
        )?;
        self.writer.flush()?;
        Ok(req_id)
    }

    fn wait_reply(&mut self, req_id: u64, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            ensure!(
                !remaining.is_zero(),
                "Herdr reply timed out; launch effects may be unknown"
            );
            let value = self
                .replies
                .recv_timeout(remaining)
                .context("Herdr reply timed out; launch effects may be unknown")??;
            if value["id"] == req_id {
                self.last_used = Instant::now();
                return Ok(value);
            }
        }
    }
}

fn pool() -> &'static Mutex<HashMap<PoolKey, Vec<PooledConn>>> {
    static POOL: OnceLock<Mutex<HashMap<PoolKey, Vec<PooledConn>>>> = OnceLock::new();
    POOL.get_or_init(Default::default)
}

fn take_idle(key: &PoolKey) -> Option<PooledConn> {
    let mut guard = pool().lock().unwrap();
    let bucket = guard.get_mut(key)?;
    while let Some(mut conn) = bucket.pop() {
        if conn.is_healthy() {
            return Some(conn);
        }
    }
    None
}

fn put_idle(key: PoolKey, mut conn: PooledConn) {
    if !conn.is_healthy() {
        return;
    }
    let mut guard = pool().lock().unwrap();
    let bucket = guard.entry(key).or_default();
    bucket.retain(|c| c.last_used.elapsed() < IDLE_TTL && !c.closed.load(Ordering::SeqCst));
    if bucket.len() >= MAX_IDLE_PER_KEY {
        bucket.remove(0);
    }
    bucket.push(conn);
}

fn is_idempotent_read(method: &str) -> bool {
    matches!(
        method,
        "server.status" | "terminal.get" | "terminal.list" | "terminal.read" | "machine.list"
    )
}

pub fn call(config: &Config, method: &str, params: Value) -> Result<Value> {
    let timeout = Duration::from_secs(if method == "terminal.run" {
        45
    } else if method == "machine.exec" {
        75
    } else {
        12
    });
    let key = PoolKey::from_config(config);
    let (mut conn, reused) = match take_idle(&key) {
        Some(c) => (c, true),
        None => (PooledConn::connect(config)?, false),
    };

    let req_id = match conn.send_request(method, &params) {
        Ok(id) => id,
        Err(_) if reused => {
            // Writing to an idle pooled pipe failed before delivery; open a fresh connection once.
            drop(conn);
            conn = PooledConn::connect(config)?;
            conn.send_request(method, &params)?
        }
        Err(e) => return Err(e.into()),
    };

    let reply = match conn.wait_reply(req_id, timeout) {
        Ok(v) => v,
        Err(err) => {
            drop(conn);
            if reused && is_idempotent_read(method) {
                let mut fresh = PooledConn::connect(config)?;
                let fresh_id = fresh.send_request(method, &params)?;
                let v = fresh.wait_reply(fresh_id, timeout)?;
                put_idle(key, fresh);
                return unpack_reply(method, v);
            }
            return Err(err);
        }
    };

    put_idle(key, conn);
    unpack_reply(method, reply)
}

fn unpack_reply(method: &str, reply: Value) -> Result<Value> {
    ensure!(
        reply.get("error").is_none(),
        "Herdr {method}: {}",
        reply["error"]
    );
    reply
        .get("result")
        .cloned()
        .context("Herdr reply has no result")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn pooled_control_connections_reuse_single_hc_process_and_recover_after_exit() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock-hc");
        std::fs::write(
            &script,
            r#"#!/usr/bin/python3
import sys, json, os
base = os.path.dirname(__file__)
with open(base + '/spawns', 'a') as f:
    f.write('spawn\n')
for line in sys.stdin:
    req = json.loads(line)
    m = req['method']
    if m == 'hello':
        res = {'generation': 1}
    elif m == 'server.status':
        res = {'core': {'id': 'core-pooled'}}
    elif m == 'terminal.kill':
        res = {'sent': True}
    else:
        res = {'ok': True}
    # Capture the exit instruction before publishing this reply: the caller
    # may arm the next request as soon as it receives the current response.
    close_after_reply = os.path.exists(base + '/close_after_reply')
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': res}), flush=True)
    if close_after_reply:
        os.unlink(base + '/close_after_reply')
        sys.exit(0)
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let config = Config {
            hc: script.to_string_lossy().into_owned(),
            hc_session: "pool-test".into(),
            ..Default::default()
        };
        let client = Client { config };

        // 5 sequential calls should spawn `hc connect` only ONCE!
        for _ in 0..5 {
            let status = client.status().unwrap();
            assert_eq!(status["core"]["id"], "core-pooled");
        }
        let spawns = std::fs::read_to_string(dir.path().join("spawns"))
            .unwrap()
            .lines()
            .count();
        assert_eq!(
            spawns, 1,
            "Sequential calls must reuse the pooled hc connect"
        );

        // Trigger graceful remote exit after next reply; subsequent call must cleanly reconnect.
        std::fs::write(dir.path().join("close_after_reply"), "").unwrap();
        assert_eq!(
            client
                .call("terminal.kill", json!({"terminal": "t1"}))
                .unwrap()["sent"],
            true
        );
        // Wait briefly for child EOF to propagate so `is_healthy` or read retry exercises recovery.
        std::thread::sleep(Duration::from_millis(30));
        let status2 = client.status().unwrap();
        assert_eq!(status2["core"]["id"], "core-pooled");
        let spawns_after = std::fs::read_to_string(dir.path().join("spawns"))
            .unwrap()
            .lines()
            .count();
        assert_eq!(
            spawns_after, 2,
            "Pool must open one replacement process after exit"
        );
    }
}
