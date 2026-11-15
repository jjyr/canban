//! Network listener configuration, Bearer Token auth, Tailscale/SSH discovery,
//! TCP host probing, SSH pairing, and Linux/macOS user service management.
use super::*;
use std::{
    net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs},
    process::{Command, Stdio},
};

pub fn listen_config_path(home: &Path) -> PathBuf {
    home.join("config/server.json")
}

pub fn load_listen_config(home: &Path) -> ServerListenConfig {
    std::fs::read(listen_config_path(home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_listen_config(home: &Path, cfg: &ServerListenConfig) -> Result<()> {
    let path = listen_config_path(home);
    canban_context::atomic(&path, &serde_json::to_vec_pretty(cfg)?)
}

pub fn token_path(home: &Path) -> PathBuf {
    home.join("server.token")
}

pub fn read_server_token(home: &Path) -> Option<String> {
    std::fs::read_to_string(token_path(home))
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| s.len() >= 32)
}

pub fn ensure_server_token(home: &Path) -> Result<String> {
    if let Some(existing) = read_server_token(home) {
        return Ok(existing);
    }
    std::fs::create_dir_all(home)?;
    let secret = format!("{}{}", canban_types::token_id(), canban_types::token_id());
    let path = token_path(home);
    canban_updater::atomic(&path, format!("{secret}\n").as_bytes())?;
    Ok(secret)
}

/// Constant-time token comparison to avoid timing side-channels over TCP.
pub fn verify_token(expected: &str, provided: &str) -> bool {
    let a = expected.trim().as_bytes();
    let b = provided.trim().as_bytes();
    if a.len() < 32 || a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub fn is_tailscale_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            // 100.64.0.0/10 (RFC 6598 CGNAT range used by Tailscale)
            octets[0] == 100 && (octets[1] & 0b1100_0000) == 64
        }
        IpAddr::V6(v6) => {
            let segs = v6.segments();
            // fd7a:115c:a1e0::/48
            segs[0] == 0xfd7a && segs[1] == 0x115c && segs[2] == 0xa1e0
        }
    }
}

fn tailscale_candidates() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("tailscale")];
    for extra in [
        "/usr/bin/tailscale",
        "/usr/local/bin/tailscale",
        "/opt/homebrew/bin/tailscale",
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    ] {
        out.push(PathBuf::from(extra));
    }
    out
}

pub fn run_tailscale(args: &[&str]) -> Result<String> {
    let mut last_err = None;
    for bin in tailscale_candidates() {
        match Command::new(&bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
        {
            Ok(out) if out.status.success() => {
                return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
                last_err = Some(if stderr.is_empty() {
                    format!("tailscale exited with {}", out.status)
                } else {
                    stderr
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => last_err = Some(e.to_string()),
        }
    }
    bail!(
        "{}",
        last_err.unwrap_or_else(|| "Tailscale CLI not found".into())
    )
}

pub fn detect_tailscale_ipv4() -> Result<String> {
    if let Ok(raw) = run_tailscale(&["ip", "-4"]) {
        for line in raw.lines() {
            let candidate = line.trim();
            if let Ok(ip) = candidate.parse::<IpAddr>()
                && is_tailscale_ip(ip)
            {
                return Ok(candidate.to_owned());
            }
        }
    }
    // Fallback on Linux when tailscale0 interface is up even if CLI path differs.
    if cfg!(target_os = "linux")
        && let Ok(out) = Command::new("ip")
            .args(["-4", "-o", "addr", "show", "dev", "tailscale0"])
            .output()
        && out.status.success()
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for token in stdout.split_whitespace() {
            if let Some((addr, _)) = token.split_once('/')
                && let Ok(ip) = addr.parse::<IpAddr>()
                && is_tailscale_ip(ip)
            {
                return Ok(addr.to_owned());
            }
        }
    }
    bail!("Tailscale IPv4 address unavailable; verify Tailscale is connected")
}

/// Resolve concrete TCP bind addresses from `ServerListenConfig`.
pub fn resolve_bind_addrs(cfg: &ServerListenConfig) -> Result<Vec<SocketAddr>> {
    let mut addrs = Vec::new();
    if let Some(explicit) = cfg
        .listen
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let normalized = canban_types::normalize_daemon_addr(explicit);
        for sa in normalized
            .to_socket_addrs()
            .with_context(|| format!("Invalid listen address '{explicit}'"))?
        {
            ensure!(
                !sa.ip().is_unspecified(),
                "Refusing to bind Canban daemon to unspecified public address {sa}; bind a Tailscale IP or loopback address"
            );
            if !addrs.contains(&sa) {
                addrs.push(sa);
            }
        }
    }
    if cfg.tailscale {
        let ts_ip = detect_tailscale_ipv4()?;
        let sa: SocketAddr = format!("{ts_ip}:{}", cfg.port)
            .parse()
            .with_context(|| format!("Invalid Tailscale bind address {ts_ip}:{}", cfg.port))?;
        if !addrs.contains(&sa) {
            addrs.push(sa);
        }
    }
    Ok(addrs)
}

pub fn parse_tailscale_status_json(raw: &str) -> Result<DiscoveryCatalog> {
    let v: Value = serde_json::from_str(raw).context("Invalid tailscale status JSON")?;
    let backend_state = v["BackendState"].as_str().unwrap_or_default();
    let self_ips: Vec<String> = v["Self"]["TailscaleIPs"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let self_ip = self_ips
        .iter()
        .find(|ip| ip.contains('.'))
        .or_else(|| self_ips.first())
        .cloned();
    let self_host = v["Self"]["HostName"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let mut peers = Vec::new();
    if let Some(peer_map) = v["Peer"].as_object() {
        for peer in peer_map.values() {
            if peer["IsShareeNode"] == true {
                continue;
            }
            let ips: Vec<String> = peer["TailscaleIPs"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if ips.is_empty() {
                continue;
            }
            let raw_dns = peer["DNSName"]
                .as_str()
                .unwrap_or_default()
                .trim_end_matches('.')
                .to_owned();
            let raw_host = peer["HostName"].as_str().unwrap_or_default().trim();
            let hostname = if !raw_host.is_empty() && raw_host != "localhost" {
                raw_host.to_owned()
            } else if let Some((first, _)) = raw_dns.split_once('.') {
                first.to_owned()
            } else {
                ips[0].clone()
            };
            peers.push(TailscalePeer {
                hostname,
                dns_name: raw_dns,
                ips,
                os: peer["OS"].as_str().unwrap_or("unknown").to_owned(),
                online: peer["Online"].as_bool().unwrap_or(false),
            });
        }
    }
    peers.sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then_with(|| a.hostname.to_lowercase().cmp(&b.hostname.to_lowercase()))
    });
    Ok(DiscoveryCatalog {
        tailscale_available: backend_state == "Running" || !peers.is_empty() || self_ip.is_some(),
        tailscale_self_ip: self_ip,
        tailscale_self_host: self_host,
        tailscale_error: if backend_state.is_empty() || backend_state == "Running" {
            None
        } else {
            Some(format!("Tailscale state: {backend_state}"))
        },
        tailscale_peers: peers,
        ssh_hosts: Vec::new(),
    })
}

pub fn parse_ssh_config_str(content: &str) -> Vec<SshHostEntry> {
    let mut entries: Vec<SshHostEntry> = Vec::new();
    let mut current_aliases: Vec<String> = Vec::new();
    let mut current_hostname: Option<String> = None;
    let mut current_user: Option<String> = None;
    let mut current_port: Option<u16> = None;

    let flush = |entries: &mut Vec<SshHostEntry>,
                 aliases: &mut Vec<String>,
                 hostname: &mut Option<String>,
                 user: &mut Option<String>,
                 port: &mut Option<u16>| {
        let host_val = hostname.take();
        let user_val = user.take();
        let port_val = port.take();
        for alias in aliases.drain(..) {
            if entries.iter().any(|e| e.alias == alias) {
                continue;
            }
            entries.push(SshHostEntry {
                hostname: host_val.clone().unwrap_or_else(|| alias.clone()),
                alias,
                user: user_val.clone(),
                port: port_val,
            });
        }
    };

    for raw_line in content.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split(|c: char| c.is_whitespace() || c == '=');
        let Some(key) = parts.next().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let rest: Vec<&str> = parts.map(str::trim).filter(|s| !s.is_empty()).collect();
        if rest.is_empty() {
            continue;
        }
        match key.to_ascii_lowercase().as_str() {
            "host" => {
                flush(
                    &mut entries,
                    &mut current_aliases,
                    &mut current_hostname,
                    &mut current_user,
                    &mut current_port,
                );
                for pat in rest {
                    if !pat.contains('*')
                        && !pat.contains('?')
                        && !pat.starts_with('!')
                        && !pat.eq_ignore_ascii_case("github.com")
                        && !pat.eq_ignore_ascii_case("gitlab.com")
                        && !pat.eq_ignore_ascii_case("bitbucket.org")
                    {
                        current_aliases.push(pat.to_owned());
                    }
                }
            }
            "match" => {
                flush(
                    &mut entries,
                    &mut current_aliases,
                    &mut current_hostname,
                    &mut current_user,
                    &mut current_port,
                );
            }
            "hostname" if !current_aliases.is_empty() => {
                current_hostname = Some(rest[0].to_owned());
            }
            "user" if !current_aliases.is_empty() => {
                current_user = Some(rest[0].to_owned());
            }
            "port" if !current_aliases.is_empty() => {
                current_port = rest[0].parse::<u16>().ok();
            }
            _ => {}
        }
    }
    flush(
        &mut entries,
        &mut current_aliases,
        &mut current_hostname,
        &mut current_user,
        &mut current_port,
    );
    entries
}

pub fn discover_ssh_hosts() -> Vec<SshHostEntry> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let ssh_dir = home.join(".ssh");
    let main_cfg = ssh_dir.join("config");
    let Ok(raw) = std::fs::read_to_string(&main_cfg) else {
        return Vec::new();
    };
    let mut combined = String::new();
    // Expand simple top-level `Include` directives inside ~/.ssh/
    for line in raw.lines() {
        let trimmed = line.split('#').next().unwrap_or("").trim();
        let mut parts = trimmed.split_whitespace();
        if let (Some(k), Some(inc)) = (parts.next(), parts.next())
            && k.eq_ignore_ascii_case("include")
        {
            let clean = inc.trim_matches('"');
            let inc_path = if let Some(rest) = clean.strip_prefix("~/") {
                home.join(rest)
            } else if Path::new(clean).is_absolute() {
                PathBuf::from(clean)
            } else {
                ssh_dir.join(clean)
            };
            if let Some(file_name) = inc_path.file_name().and_then(|s| s.to_str())
                && file_name.contains('*')
                && let Some(parent) = inc_path.parent()
                && let Ok(dir_entries) = std::fs::read_dir(parent)
            {
                let mut files: Vec<PathBuf> = dir_entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file())
                    .collect();
                files.sort();
                for f in files {
                    if let Ok(sub) = std::fs::read_to_string(&f) {
                        combined.push_str(&sub);
                        combined.push('\n');
                    }
                }
                continue;
            } else if inc_path.is_file()
                && let Ok(sub) = std::fs::read_to_string(&inc_path)
            {
                combined.push_str(&sub);
                combined.push('\n');
                continue;
            }
        }
        combined.push_str(line);
        combined.push('\n');
    }
    parse_ssh_config_str(&combined)
}

pub fn discover_all() -> DiscoveryCatalog {
    let mut catalog = match run_tailscale(&["status", "--json"]) {
        Ok(json_str) => parse_tailscale_status_json(&json_str).unwrap_or_default(),
        Err(e) => DiscoveryCatalog {
            tailscale_available: false,
            tailscale_error: Some(e.to_string()),
            ..Default::default()
        },
    };
    catalog.ssh_hosts = discover_ssh_hosts();
    catalog
}

/// Probe a remote Canban TCP daemon without authentication (`host.probe`).
pub fn probe_tcp_host(raw_addr: &str, timeout: Duration) -> Result<Value> {
    let normalized = canban_types::normalize_daemon_addr(raw_addr);
    ensure!(!normalized.is_empty(), "Host address is empty");
    let sock_addr = normalized
        .to_socket_addrs()
        .with_context(|| format!("Could not resolve {normalized}"))?
        .next()
        .with_context(|| format!("No socket address resolved for {normalized}"))?;
    let start = std::time::Instant::now();
    let mut stream = TcpStream::connect_timeout(&sock_addr, timeout)
        .with_context(|| format!("Could not connect to {normalized}"))?;
    let _ = stream.set_nodelay(true);
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    writeln!(
        stream,
        "{}",
        json!({"jsonrpc":"2.0","id":1,"method":"host.probe","params":{}})
    )?;
    stream.flush()?;
    let mut line = String::new();
    BufReader::new(stream)
        .take(16 * 1024)
        .read_line(&mut line)?;
    ensure!(!line.is_empty(), "Remote daemon closed probe socket");
    let reply: Value = serde_json::from_str(&line).context("Invalid probe JSON response")?;
    if let Some(err) = reply.get("error") {
        bail!(
            "{}",
            err["message"].as_str().unwrap_or("Remote probe rejected")
        );
    }
    let mut result = reply
        .get("result")
        .cloned()
        .context("Missing probe result")?;
    result["addr"] = json!(normalized);
    result["rtt_ms"] = json!(start.elapsed().as_millis() as u64);
    Ok(result)
}

/// Attempt non-interactive SSH pairing (`BatchMode=yes`) against `ssh_target`.
/// If `canban` is already installed on the remote machine, enables `--tailscale` (when requested),
/// ensures the user service is running, and returns `{ addr, token, server_id, hostname, platform }`.
pub fn pair_via_ssh_noninteractive(
    ssh_target: &str,
    prefer_tailscale: bool,
    port: u16,
) -> Result<Value> {
    let mode_flag = if prefer_tailscale { "--tailscale" } else { "" };
    let remote_script = format!(
        r#"export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH";
if ! command -v canban >/dev/null 2>&1; then
  echo '{{"error":"canban_not_installed"}}'
  exit 0
fi
canban service install {mode_flag} --port {port} >/dev/null 2>&1 || true
canban pair {mode_flag} --port {port} --json"#
    );
    let out = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            ssh_target.trim(),
            &remote_script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to execute ssh")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        bail!(
            "Non-interactive SSH could not connect to {}: {}",
            ssh_target.trim(),
            if stderr.is_empty() {
                format!("exit {}", out.status)
            } else {
                stderr
            }
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last_json = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .context("Remote `canban pair` returned no JSON output")?;
    let parsed: Value =
        serde_json::from_str(last_json).context("Invalid JSON from remote `canban pair`")?;
    if parsed["error"] == "canban_not_installed" {
        bail!("canban_not_installed");
    }
    if let Some(err) = parsed["error"].as_str() {
        bail!("{err}");
    }
    Ok(parsed)
}

/// Build an interactive shell command string that installs/updates `hc` + `canban` on a remote
/// Linux/macOS host over SSH, enables its user daemon (`systemd --user` / `launchd`), prints
/// the pairing URI cleanly inside a setup Terminal, and drops into an interactive login shell so
/// the Terminal stays open until the user explicitly closes it.
pub fn build_ssh_bootstrap_command(
    ssh_target: &str,
    repo: &str,
    prefer_tailscale: bool,
    port: u16,
) -> String {
    let mode_flag = if prefer_tailscale { "--tailscale" } else { "" };
    let remote_inner = format!(
        r#"set -e
mkdir -p "$HOME/.local/bin"
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$HOME/.cargo/bin:$PATH"
echo "==> [1/3] Checking Herdr Core (hc)..."
if ! command -v hc >/dev/null 2>&1; then
  curl -fsSL https://releases.herdr.dev/core/install.sh | sh
fi
echo "==> [2/3] Installing latest Canban daemon for $(uname -s)-$(uname -m)..."
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$ARCH" in
  x86_64|amd64) ARCH="x86_64" ;;
  aarch64|arm64) ARCH="aarch64" ;;
esac
if [ "$OS" = "linux" ]; then
  ASSET="canban-linux-${{ARCH}}.tar.gz"
  TMP=$(mktemp -d)
  if curl -fL "https://github.com/{repo}/releases/latest/download/${{ASSET}}" -o "$TMP/canban.tar.gz" 2>/dev/null; then
    tar -xzf "$TMP/canban.tar.gz" -C "$TMP"
    install -m 0755 "$TMP/canban" "$HOME/.local/bin/canban"
    rm -rf "$TMP"
  else
    rm -rf "$TMP"
    echo "    Prebuilt ${{ASSET}} not found in latest GitHub release; building canban-cli from source..."
    if ! command -v cargo >/dev/null 2>&1 && ! command -v mise >/dev/null 2>&1; then
      echo "Error: Prebuilt ${{ASSET}} is not published yet and neither cargo nor mise is installed on remote host." >&2
      exit 1
    fi
    SRC_TMP=$(mktemp -d)
    git clone --depth 1 "https://github.com/{repo}.git" "$SRC_TMP/canban"
    (
      cd "$SRC_TMP/canban"
      if command -v mise >/dev/null 2>&1; then
        mise trust -q 2>/dev/null || true
        mise x -- cargo build --release --locked -p canban-cli
      else
        cargo build --release --locked -p canban-cli
      fi
      install -m 0755 target/release/canban "$HOME/.local/bin/canban"
    )
    rm -rf "$SRC_TMP"
  fi
elif [ "$OS" = "darwin" ] && [ -x "/Applications/Canban.app/Contents/MacOS/canban" ]; then
  ln -sf "/Applications/Canban.app/Contents/MacOS/canban" "$HOME/.local/bin/canban"
fi
echo "==> [3/3] Configuring background service & pairing..."
"$HOME/.local/bin/canban" service install {mode_flag} --port {port}
echo ""
echo "==> Pairing ready! Copy the URI below (or return to Connect a Machine and click Pair & Connect):"
"$HOME/.local/bin/canban" pair {mode_flag} --port {port}
"#
    );
    let remote_cmd = format!("sh -lc {}", shell_single_quote(&remote_inner));
    let ssh_cmd = format!(
        "ssh -t {} {}",
        shell_single_quote(ssh_target.trim()),
        shell_single_quote(&remote_cmd)
    );
    format!(
        "{ssh_cmd}; status=$?; echo ''; if [ \"$status\" -ne 0 ]; then echo \"==> Setup failed (exit $status). Scroll up to inspect the error, or troubleshoot directly below.\"; else echo \"==> Setup complete. Close this terminal tab when done.\"; fi; exec \"${{SHELL:-/bin/zsh}}\" -l"
    )
}

fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn hostname_string() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "canban-host".into())
}

pub fn service_status(home: &Path) -> Value {
    if cfg!(target_os = "linux") {
        let unit = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.to_path_buf())
            .join(".config/systemd/user/canban.service");
        let installed = unit.is_file();
        let active = Command::new("systemctl")
            .args(["--user", "is-active", "canban.service"])
            .output()
            .ok()
            .is_some_and(|o| o.status.success());
        let user = std::env::var("USER").unwrap_or_default();
        let linger = if !user.is_empty() {
            Some(Path::new(&format!("/var/lib/systemd/linger/{user}")).exists())
        } else {
            None
        };
        json!({
            "kind": "systemd-user",
            "installed": installed,
            "active": active,
            "linger": linger,
            "unit_path": unit,
        })
    } else if cfg!(target_os = "macos") {
        let plist = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.to_path_buf())
            .join("Library/LaunchAgents/com.canban.daemon.plist");
        let installed = plist.is_file();
        let active = Command::new("launchctl")
            .args(["list", "com.canban.daemon"])
            .output()
            .ok()
            .is_some_and(|o| o.status.success());
        json!({
            "kind": "launchd",
            "installed": installed,
            "active": active,
            "linger": Value::Null,
            "unit_path": plist,
        })
    } else {
        json!({
            "kind": "manual",
            "installed": false,
            "active": false,
            "linger": Value::Null,
            "unit_path": Value::Null,
        })
    }
}

pub fn install_user_service(
    home: &Path,
    cli: &Path,
    tailscale: bool,
    port: u16,
    listen: Option<String>,
) -> Result<Value> {
    let cfg = ServerListenConfig {
        tailscale,
        port,
        listen,
    };
    save_listen_config(home, &cfg)?;
    if cfg.tcp_enabled() {
        let _ = ensure_server_token(home)?;
    }
    let user_home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let cli_abs = std::fs::canonicalize(cli).unwrap_or_else(|_| cli.to_path_buf());
    let home_abs = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());

    if cfg!(target_os = "linux") {
        let unit_dir = user_home.join(".config/systemd/user");
        std::fs::create_dir_all(&unit_dir)?;
        let unit_path = unit_dir.join("canban.service");
        let path_env = format!(
            "{}/.local/bin:/usr/local/bin:/usr/bin:/bin",
            user_home.display()
        );
        let unit_body = format!(
            "[Unit]\n\
Description=Canban AppServer Daemon\n\
After=network-online.target\n\
Wants=network-online.target\n\n\
[Service]\n\
Type=simple\n\
Environment=\"PATH={path_env}\"\n\
ExecStart=\"{}\" --home \"{}\" serve\n\
Restart=always\n\
RestartSec=2\n\n\
[Install]\n\
WantedBy=default.target\n",
            cli_abs.display(),
            home_abs.display()
        );
        canban_context::atomic(&unit_path, unit_body.as_bytes())?;
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        let enabled = Command::new("systemctl")
            .args(["--user", "enable", "--now", "canban.service"])
            .status()
            .is_ok_and(|s| s.success());
        if let Ok(user) = std::env::var("USER") {
            let _ = Command::new("loginctl")
                .args(["enable-linger", &user])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let mut status = service_status(home);
        status["enabled_now"] = json!(enabled);
        return Ok(status);
    }

    if cfg!(target_os = "macos") {
        let agents_dir = user_home.join("Library/LaunchAgents");
        std::fs::create_dir_all(&agents_dir)?;
        let plist_path = agents_dir.join("com.canban.daemon.plist");
        let log_path = home_abs.join("server.log");
        let path_env = format!(
            "{}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
            user_home.display()
        );
        let plist_body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.canban.daemon</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>--home</string>
    <string>{}</string>
    <string>serve</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>{}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{}</string>
  <key>StandardErrorPath</key>
  <string>{}</string>
</dict>
</plist>
"#,
            cli_abs.display(),
            home_abs.display(),
            path_env,
            log_path.display(),
            log_path.display()
        );
        canban_context::atomic(&plist_path, plist_body.as_bytes())?;
        let _ = Command::new("launchctl")
            .args(["unload", &plist_path.to_string_lossy()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let loaded = Command::new("launchctl")
            .args(["load", "-w", &plist_path.to_string_lossy()])
            .status()
            .is_ok_and(|s| s.success());
        let mut status = service_status(home);
        status["enabled_now"] = json!(loaded);
        return Ok(status);
    }

    bail!("Automatic service installation is supported on Linux (systemd) and macOS (launchd)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_token_checks_exact_length_and_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let tok = ensure_server_token(dir.path()).unwrap();
        assert_eq!(tok.len(), 64);
        assert!(verify_token(&tok, &tok));
        assert!(verify_token(&tok, &format!("  {tok}\n")));
        let mut wrong = tok.clone();
        wrong.replace_range(0..1, if &tok[0..1] == "a" { "b" } else { "a" });
        assert!(!verify_token(&tok, &wrong));
        assert!(!verify_token(&tok, "short"));
    }

    #[test]
    fn tailscale_cgnat_ip_detection_is_accurate() {
        assert!(is_tailscale_ip("100.64.0.1".parse().unwrap()));
        assert!(is_tailscale_ip("100.84.12.5".parse().unwrap()));
        assert!(is_tailscale_ip("100.127.255.254".parse().unwrap()));
        assert!(!is_tailscale_ip("100.128.0.1".parse().unwrap()));
        assert!(!is_tailscale_ip("192.168.1.10".parse().unwrap()));
        assert!(is_tailscale_ip("fd7a:115c:a1e0::1".parse().unwrap()));
    }

    #[test]
    fn parses_tailscale_status_peers_sorted_online_first() {
        let sample = r#"{
            "BackendState": "Running",
            "Self": {
                "HostName": "macbook-pro",
                "TailscaleIPs": ["100.84.0.1", "fd7a:115c:a1e0::1"]
            },
            "Peer": {
                "nodekey:1": {
                    "HostName": "nas-offline",
                    "DNSName": "nas-offline.tailnet.ts.net.",
                    "OS": "linux",
                    "TailscaleIPs": ["100.84.0.9"],
                    "Online": false
                },
                "nodekey:2": {
                    "HostName": "home-linux",
                    "DNSName": "home-linux.tailnet.ts.net.",
                    "OS": "linux",
                    "TailscaleIPs": ["100.84.0.2", "fd7a:115c:a1e0::2"],
                    "Online": true
                }
            }
        }"#;
        let cat = parse_tailscale_status_json(sample).unwrap();
        assert!(cat.tailscale_available);
        assert_eq!(cat.tailscale_self_ip.as_deref(), Some("100.84.0.1"));
        assert_eq!(cat.tailscale_peers.len(), 2);
        assert_eq!(cat.tailscale_peers[0].hostname, "home-linux");
        assert!(cat.tailscale_peers[0].online);
        assert_eq!(cat.tailscale_peers[0].preferred_ip(), Some("100.84.0.2"));
        assert_eq!(cat.tailscale_peers[1].hostname, "nas-offline");
        assert!(!cat.tailscale_peers[1].online);
    }

    #[test]
    fn parses_ssh_config_aliases_excluding_wildcards_and_git_forges() {
        let cfg = r#"
            Host *
              AddKeysToAgent yes

            Host github.com
              User git

            Host home-linux arch-box
              HostName 100.84.0.2
              User jjy
              Port 2222

            Host cloud-vm
              HostName 203.0.113.10
        "#;
        let hosts = parse_ssh_config_str(cfg);
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].alias, "home-linux");
        assert_eq!(hosts[0].hostname, "100.84.0.2");
        assert_eq!(hosts[0].user.as_deref(), Some("jjy"));
        assert_eq!(hosts[0].port, Some(2222));
        assert_eq!(hosts[0].target_spec(), "jjy@home-linux");
        assert_eq!(hosts[1].alias, "arch-box");
        assert_eq!(hosts[2].alias, "cloud-vm");
        assert_eq!(hosts[2].hostname, "203.0.113.10");
    }

    #[test]
    fn ssh_bootstrap_command_parses_in_zsh_and_sh_and_retains_interactive_shell() {
        let cmd = build_ssh_bootstrap_command("jjy@100.127.37.78", "jjyr/canban", true, 7680);
        assert!(
            cmd.contains("exec \"${SHELL:-/bin/zsh}\" -l"),
            "bootstrap command must hand off to an interactive shell so the terminal does not auto-close"
        );
        for shell in ["/bin/sh", "/bin/zsh"] {
            if Path::new(shell).is_file() {
                let status = Command::new(shell)
                    .args(["-n", "-c", &cmd])
                    .status()
                    .expect("spawn shell syntax check");
                assert!(
                    status.success(),
                    "{shell} -n rejected bootstrap command:\n{cmd}"
                );
            }
        }
        // Also verify that the argv passed across `ssh` executes valid `sh -n` syntax on the remote side.
        // Extract the second argument passed to `ssh -t <target> <remote_cmd>` by evaluating word-splitting in `/bin/sh` with a mock `ssh` function.
        let verify_inner = format!(
            "ssh() {{ shift; shift; /bin/sh -c \"$(printf '%s' \\\"$1\\\" | sed 's/^sh -lc //')\" -- -n >/dev/null 2>&1 || eval \"set -- $1; /bin/sh -n -c \\\"\\$3\\\"\"; }}; \
             ssh_check() {{ shift; shift; eval \"check_remote $1\"; }}; \
             check_remote() {{ [ \"$1\" = \"sh\" ] && [ \"$2\" = \"-lc\" ] && /bin/sh -n -c \"$3\"; }}; \
             {}",
            cmd.replacen("ssh -t ", "ssh_check -t ", 1)
                .split("; status=$?")
                .next()
                .unwrap()
        );
        let inner_status = Command::new("/bin/sh")
            .args(["-c", &verify_inner])
            .status()
            .expect("verify inner remote script syntax");
        assert!(
            inner_status.success(),
            "remote inner sh script failed syntax check:\n{cmd}"
        );
    }
}
