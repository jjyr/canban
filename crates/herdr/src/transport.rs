//! One byte transport for direct Core access and AppServer's transparent relay.
use super::*;
use std::{
    io::Read,
    net::Shutdown,
    os::unix::net::UnixStream,
    process::{Child, ChildStdin, ChildStdout},
};
pub struct Transport {
    pub reader: Option<Box<dyn Read + Send>>,
    pub writer: Option<Box<dyn Write + Send>>,
    child: Option<Child>,
    socket: Option<UnixStream>,
}
impl Drop for Transport {
    fn drop(&mut self) {
        if let Some(socket) = &self.socket {
            let _ = socket.shutdown(Shutdown::Both);
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
        .envs(&config.hc_env)
        .env_remove("HC_TERMINAL_ID");
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
    connect_view(config, None)
}
pub fn connect_view(config: &Config, terminal: Option<&str>) -> Result<Transport> {
    if let Some(endpoint) = &config.relay_endpoint {
        let mut socket =
            UnixStream::connect(endpoint).context("AppServer Terminal relay unavailable")?;
        socket.set_read_timeout(Some(Duration::from_secs(90)))?;
        writeln!(
            socket,
            "{}",
            json!({"jsonrpc":"2.0","id":0,"method":"herdr.connect","params":{"terminal":terminal}})
        )?;
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
        ensure!(reply["result"]["ready"] == true, "Relay failed: {reply}");
        socket.set_read_timeout(None)?;
        return Ok(Transport {
            reader: Some(Box::new(socket.try_clone()?)),
            writer: Some(Box::new(socket.try_clone()?)),
            child: None,
            socket: Some(socket),
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
