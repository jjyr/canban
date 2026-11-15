//! One byte transport for direct Core access and AppServer's transparent relay
//! over either local Unix sockets or authenticated Tailscale/TCP sockets.
use super::*;
use std::{
    io::Read,
    net::{Shutdown, TcpStream, ToSocketAddrs},
    os::unix::net::UnixStream,
    process::{Child, ChildStdin, ChildStdout},
};

enum RelaySocket {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl RelaySocket {
    fn shutdown(&self) {
        match self {
            Self::Unix(s) => {
                let _ = s.shutdown(Shutdown::Both);
            }
            Self::Tcp(s) => {
                let _ = s.shutdown(Shutdown::Both);
            }
        }
    }
}

pub struct Transport {
    pub reader: Option<Box<dyn Read + Send>>,
    pub writer: Option<Box<dyn Write + Send>>,
    child: Option<Child>,
    socket: Option<RelaySocket>,
}
impl Transport {
    pub fn child_exited(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|c| !matches!(c.try_wait(), Ok(None)))
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        if let Some(socket) = &self.socket {
            socket.shutdown();
        }
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
pub fn command(config: &Config) -> Command {
    let mut cmd = Command::new(&config.hc);
    cmd.arg("connect")
        .env("HC_SESSION", &config.hc_session)
        .env_remove("HC_TERMINAL_ID");
    super::apply_default_terminal_env_to_command(&mut cmd, &config.hc_env);
    cmd
}
pub fn child(config: &Config) -> Result<(Child, ChildStdin, ChildStdout)> {
    let mut child = command(config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Could not start hc connect")?;
    let input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    Ok((child, input, output))
}
pub fn connect(config: &Config) -> Result<Transport> {
    connect_view_fenced(config, None, None)
}
pub fn connect_view(config: &Config, terminal: Option<&str>) -> Result<Transport> {
    connect_view_fenced(config, terminal, None)
}

fn handshake_relay<S: Read + Write>(
    socket: &mut S,
    terminal: Option<&str>,
    expected_core_id: Option<&str>,
    auth_token: Option<&str>,
) -> Result<()> {
    let mut params = json!({"terminal":terminal});
    if let Some(core_id) = expected_core_id {
        params["expected_core_id"] = json!(core_id);
    }
    let mut req = json!({"jsonrpc":"2.0","id":0,"method":"herdr.connect","params":params});
    if let Some(token) = auth_token.filter(|t| !t.is_empty()) {
        req["auth"] = json!(token);
    }
    writeln!(socket, "{req}")?;
    socket.flush()?;
    // Consume precisely one line; no buffered reader may eat Core bytes.
    let mut bytes = Vec::new();
    let mut byte = [0];
    while socket.read(&mut byte)? == 1 {
        if byte[0] == b'\n' {
            break;
        }
        bytes.push(byte[0]);
        ensure!(bytes.len() < 65536, "Invalid relay greeting");
    }
    let reply: Value = serde_json::from_slice(&bytes)?;
    if let Some(err) = reply.get("error") {
        anyhow::bail!(
            "Relay error: {}",
            err["message"].as_str().unwrap_or("unavailable")
        );
    }
    ensure!(reply["result"]["ready"] == true, "Relay failed: {reply}");
    Ok(())
}

pub fn connect_view_fenced(
    config: &Config,
    terminal: Option<&str>,
    expected_core_id: Option<&str>,
) -> Result<Transport> {
    if let Some(endpoint) = &config.relay_endpoint {
        if let Some(addr_str) = endpoint.strip_prefix("tcp://") {
            let sock_addr = addr_str
                .to_socket_addrs()
                .with_context(|| format!("Invalid TCP relay address {addr_str}"))?
                .next()
                .with_context(|| format!("Could not resolve TCP relay address {addr_str}"))?;
            let mut socket = TcpStream::connect_timeout(&sock_addr, Duration::from_secs(10))
                .with_context(|| {
                    format!("AppServer TCP Terminal relay unavailable at {addr_str}")
                })?;
            let _ = socket.set_nodelay(true);
            socket.set_read_timeout(Some(Duration::from_secs(90)))?;
            socket.set_write_timeout(Some(Duration::from_secs(90)))?;
            handshake_relay(
                &mut socket,
                terminal,
                expected_core_id,
                config.relay_token.as_deref(),
            )?;
            socket.set_read_timeout(None)?;
            socket.set_write_timeout(None)?;
            return Ok(Transport {
                reader: Some(Box::new(socket.try_clone()?)),
                writer: Some(Box::new(socket.try_clone()?)),
                child: None,
                socket: Some(RelaySocket::Tcp(socket)),
            });
        }
        let mut socket =
            UnixStream::connect(endpoint).context("AppServer Terminal relay unavailable")?;
        socket.set_read_timeout(Some(Duration::from_secs(90)))?;
        handshake_relay(
            &mut socket,
            terminal,
            expected_core_id,
            config.relay_token.as_deref(),
        )?;
        socket.set_read_timeout(None)?;
        return Ok(Transport {
            reader: Some(Box::new(socket.try_clone()?)),
            writer: Some(Box::new(socket.try_clone()?)),
            child: None,
            socket: Some(RelaySocket::Unix(socket)),
        });
    }
    let (child, input, output) = child(config)?;
    Ok(Transport {
        reader: Some(Box::new(output)),
        writer: Some(Box::new(input)),
        child: Some(child),
        socket: None,
    })
}
